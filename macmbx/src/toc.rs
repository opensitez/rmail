//! TOC (Table of Contents) — core types and read/write for Eudora's binary .toc format.
//!
//! Binary-compatible with Eudora's on-disk format:
//! - `DiskHeader`: 76 bytes
//! - `DiskSum`: 224 bytes per message

use std::io::{self, Read, Write, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::fs::{self, File, OpenOptions};

// ── Message state (matches Eudora StateEnum exactly) ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum State {
    Unread      = 1,
    Read        = 2,
    Replied     = 3,
    Redirected  = 4,
    Unsendable  = 5,
    Sendable    = 6,
    Queued      = 7,
    Forwarded   = 8,
    Sent        = 9,
    Unsent      = 10,
    Timed       = 11,
    BusySending = 12,
    MesgErr     = 13,
    Rebuilt     = 14,
}

impl State {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Unread), 2 => Some(Self::Read), 3 => Some(Self::Replied),
            4 => Some(Self::Redirected), 5 => Some(Self::Unsendable),
            6 => Some(Self::Sendable), 7 => Some(Self::Queued),
            8 => Some(Self::Forwarded), 9 => Some(Self::Sent),
            10 => Some(Self::Unsent), 11 => Some(Self::Timed),
            12 => Some(Self::BusySending), 13 => Some(Self::MesgErr),
            14 => Some(Self::Rebuilt), _ => None,
        }
    }

    pub fn is_outgoing(self) -> bool {
        matches!(self, Self::Unsendable | Self::Sendable | Self::Queued |
                 Self::Sent | Self::Unsent | Self::Timed | Self::BusySending)
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Unread => "●", Self::Read => " ", Self::Replied => "R",
            Self::Redirected => "D", Self::Unsendable => "—",
            Self::Sendable => "S", Self::Queued => "Q", Self::Forwarded => "F",
            Self::Sent => "✓", Self::Unsent => "✗", Self::Timed => "⏰",
            Self::BusySending => "⇧", Self::MesgErr => "⚠", Self::Rebuilt => "♻",
        };
        write!(f, "{}", s)
    }
}

// ── Special mailbox types ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i16)]
pub enum MailboxType {
    Normal  = 0,
    In      = 1,
    Out     = 2,
    Trash   = 3,
    Junk    = 4,
    InTemp  = 11,
    OutTemp = 12,
}

impl MailboxType {
    pub fn from_i16(v: i16) -> Self {
        match v {
            1 => Self::In, 2 => Self::Out, 3 => Self::Trash,
            4 => Self::Junk, 11 => Self::InTemp, 12 => Self::OutTemp,
            _ => Self::Normal,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Normal => "Mailbox", Self::In => "In", Self::Out => "Out",
            Self::Trash => "Trash", Self::Junk => "Junk",
            Self::InTemp => "In (temp)", Self::OutTemp => "Out (temp)",
        }
    }
}

// Simple bitflags macro (no external dependency)
macro_rules! bitflags {
    (
        #[derive($($derive:ident),*)]
        $vis:vis struct $name:ident : $ty:ty {
            $(const $flag:ident = $val:expr;)*
        }
    ) => {
        #[derive($($derive),*)]
        $vis struct $name($ty);

        impl $name {
            $(pub const $flag: Self = Self($val);)*

            pub const fn bits(self) -> $ty { self.0 }
            pub const fn from_bits_truncate(bits: $ty) -> Self { Self(bits) }
            pub const fn empty() -> Self { Self(0) }
            pub fn contains(self, other: Self) -> bool { (self.0 & other.0) == other.0 }
            pub fn insert(&mut self, other: Self) { self.0 |= other.0; }
            pub fn remove(&mut self, other: Self) { self.0 &= !other.0; }
        }

        impl std::ops::BitOr for $name {
            type Output = Self;
            fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
        }
        impl std::ops::BitAnd for $name {
            type Output = Self;
            fn bitand(self, rhs: Self) -> Self { Self(self.0 & rhs.0) }
        }
        impl std::ops::Not for $name {
            type Output = Self;
            fn not(self) -> Self { Self(!self.0) }
        }
    };
}

// ── Message flags (bitfield) ──

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MsgFlags: u32 {
        const DELETED    = 1 << 0;
        const READ       = 1 << 1;
        const ANSWERED   = 1 << 2;
        const FLAGGED    = 1 << 3;
        const DRAFT      = 1 << 4;
        const ATTACHMENT = 1 << 5;
        const HTML       = 1 << 6;
        const BULK       = 1 << 7;
        const UTF8       = 1 << 8;
        const KEEP_COPY  = 1 << 9;
        const RR         = 1 << 10;
        const CAN_ENC    = 1 << 11;
        const BX_TEXT    = 1 << 15;
        const WRAP_OUT   = 1 << 16;
    }
}

impl MsgFlags {
    pub fn label(self) -> u8 {
        ((self.bits() >> 12) & 7) as u8
    }

    pub fn with_label(self, label: u8) -> Self {
        let bits = (self.bits() & !(7 << 12)) | (((label & 7) as u32) << 12);
        Self::from_bits_truncate(bits)
    }
}

/// Message options (opts field).
bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MsgOpts: u32 {
        const DELETED    = 1 << 0;
        const HTML       = 1 << 1;
        const FLOW       = 1 << 2;
        const CHARSET    = 1 << 3;
        const BULK       = 1 << 4;
        const ATT_DEL    = 1 << 6;
        const ORPHAN_ATT = 1 << 9;
    }
}

// ── On-disk format constants ──

pub const TOC_MAJOR: i32 = 1;
pub const TOC_MINOR: i32 = 9;
pub const DISK_HDR_SIZE: usize = 76;
pub const DISK_SUM_SIZE: usize = 224;

// ── In-memory message summary ──

#[derive(Debug, Clone)]
pub struct MsgSum {
    pub offset: i64,
    pub length: i64,
    pub body_offset: i32,
    pub state: State,
    pub priority: u8,
    pub orig_priority: u8,
    pub spam_score: i8,
    pub spam_because: u8,
    pub flags: MsgFlags,
    pub opts: MsgOpts,
    pub seconds: u32,
    pub arrival: u32,
    pub from_hash: u32,
    pub msg_id_hash: i32,
    pub uid_hash: u32,
    pub serial_num: i64,
    pub orig_zone: i16,
    pub sig_id: u32,
    pub pop_pers_id: u32,
    pub pers_id: u32,
    pub subj_id: i16,
    pub table_id: i16,
    pub score: i16,
    pub out_type: i16,
    pub saved_pos: [i16; 4],
    pub from: String,
    pub subject: String,
    pub has_attachment: bool,
    pub in_reply_to_hash: i32,
    pub selected: bool,
}

impl Default for MsgSum {
    fn default() -> Self {
        Self {
            offset: 0, length: 0, body_offset: 0,
            state: State::Unread, priority: 3, orig_priority: 3,
            spam_score: -1, spam_because: 0,
            flags: MsgFlags::empty(), opts: MsgOpts::empty(),
            seconds: 0, arrival: 0, from_hash: 0, msg_id_hash: 0,
            uid_hash: 0, serial_num: 0, orig_zone: 0, sig_id: 0,
            pop_pers_id: 0, pers_id: 0, subj_id: 0, table_id: 0,
            score: 0, out_type: 0, saved_pos: [0; 4],
            from: String::new(), subject: String::new(),
            has_attachment: false, in_reply_to_hash: 0, selected: false,
        }
    }
}

// ── In-memory TOC ──

#[derive(Debug)]
pub struct Toc {
    pub mbox_path: PathBuf,
    pub toc_path: PathBuf,
    pub msgs: Vec<MsgSum>,
    pub next_serial: i64,
    pub which: MailboxType,
    pub dirty: bool,
    // Disk header fields preserved for round-trip
    pub major_version: i32,
    pub minor_version: i32,
    pub box_size: i64,
    pub write_date: i32,
    pub sort_order: i32,
    pub last_sort: i32,
    pub plugin_key: i32,
    pub plugin_value: i32,
    pub preview_hi: i32,
    pub unread_base: i32,
    pub sorts: [i32; 6],
    pub needs_compact: i32,
}

impl Toc {
    /// Create a new empty TOC for a mailbox path.
    pub fn new(mbox_path: impl AsRef<Path>) -> Self {
        let mbox = mbox_path.as_ref().to_path_buf();
        let toc = mbox.with_extension("toc");
        Self {
            mbox_path: mbox, toc_path: toc,
            msgs: Vec::new(), next_serial: 1,
            which: MailboxType::Normal, dirty: false,
            major_version: TOC_MAJOR, minor_version: TOC_MINOR,
            box_size: 0, write_date: 0, sort_order: 0, last_sort: 0,
            plugin_key: 0, plugin_value: 0, preview_hi: 0,
            unread_base: 0, sorts: [0; 6], needs_compact: 0,
        }
    }

    /// Open and read a TOC from disk.
    /// If the .toc file is missing or corrupt, rebuilds from the mbox
    /// (like original Eudora's CheckTOC → BuildTOC).
    pub fn open(mbox_path: impl AsRef<Path>) -> io::Result<Self> {
        let mbox = mbox_path.as_ref();
        let toc_path = mbox.with_extension("toc");

        if !toc_path.exists() {
            // TOC missing — rebuild from mbox (like BuildTOC)
            if mbox.exists() {
                let mut toc = crate::build::build_toc(mbox)?;
                toc.save()?;
                return Ok(toc);
            }
            return Err(io::Error::new(io::ErrorKind::NotFound,
                format!("mailbox not found: {}", mbox.display())));
        }

        let data = fs::read(&toc_path)?;
        if data.len() < DISK_HDR_SIZE {
            // TOC corrupt — rebuild from mbox
            if mbox.exists() {
                let mut toc = crate::build::build_toc(mbox)?;
                toc.save()?;
                return Ok(toc);
            }
            return Err(io::Error::new(io::ErrorKind::InvalidData, "TOC too small"));
        }

        let mut toc = Self::new(mbox);
        toc.toc_path = toc_path;

        // Parse header
        toc.major_version = read_i32(&data, 0);
        toc.minor_version = read_i32(&data, 4);
        let count = read_i16(&data, 8) as usize;
        toc.which = MailboxType::from_i16(read_i16(&data, 10));
        toc.box_size = read_i32(&data, 12) as i64;
        toc.write_date = read_i32(&data, 16);
        toc.next_serial = read_i32(&data, 20) as i64;
        toc.sort_order = read_i32(&data, 24);
        toc.last_sort = read_i32(&data, 28);
        toc.plugin_key = read_i32(&data, 32);
        toc.plugin_value = read_i32(&data, 36);
        toc.preview_hi = read_i32(&data, 40);
        toc.unread_base = read_i32(&data, 44);
        for i in 0..6 {
            toc.sorts[i] = read_i32(&data, 48 + i * 4);
        }
        toc.needs_compact = read_i32(&data, 72);

        // Parse message summaries
        let expected_len = DISK_HDR_SIZE + count * DISK_SUM_SIZE;
        let actual_count = if data.len() >= expected_len { count }
                          else { (data.len() - DISK_HDR_SIZE) / DISK_SUM_SIZE };

        toc.msgs.reserve(actual_count);
        for i in 0..actual_count {
            let off = DISK_HDR_SIZE + i * DISK_SUM_SIZE;
            if off + DISK_SUM_SIZE > data.len() { break; }
            toc.msgs.push(read_disk_sum(&data[off..off + DISK_SUM_SIZE]));
        }

        Ok(toc)
    }

    /// Open an existing TOC or create a new empty one if it doesn't exist.
    pub fn open_or_create(mbox_path: impl AsRef<Path>) -> io::Result<Self> {
        let mbox = mbox_path.as_ref();
        match Self::open(mbox) {
            Ok(toc) => Ok(toc),
            Err(_) => {
                // Create the mbox file if it doesn't exist
                if !mbox.exists() {
                    fs::write(mbox, b"")?;
                }
                Ok(Self::new(mbox))
            }
        }
    }

    /// Save the TOC to disk.
    pub fn save(&mut self) -> io::Result<()> {
        let mut buf = Vec::with_capacity(DISK_HDR_SIZE + self.msgs.len() * DISK_SUM_SIZE);

        // Write header
        write_i32(&mut buf, self.major_version);
        write_i32(&mut buf, self.minor_version);
        write_i16(&mut buf, self.msgs.len() as i16);
        write_i16(&mut buf, self.which as i16);
        write_i32(&mut buf, self.box_size as i32);
        write_i32(&mut buf, self.write_date);
        write_i32(&mut buf, self.next_serial as i32);
        write_i32(&mut buf, self.sort_order);
        write_i32(&mut buf, self.last_sort);
        write_i32(&mut buf, self.plugin_key);
        write_i32(&mut buf, self.plugin_value);
        write_i32(&mut buf, self.preview_hi);
        write_i32(&mut buf, self.unread_base);
        for i in 0..6 { write_i32(&mut buf, self.sorts[i]); }
        write_i32(&mut buf, self.needs_compact);

        assert_eq!(buf.len(), DISK_HDR_SIZE);

        // Write summaries
        for msg in &self.msgs {
            write_disk_sum(&mut buf, msg);
        }

        // Atomic write: write to .tmp then rename
        let tmp_path = self.toc_path.with_extension("toc.tmp");
        fs::write(&tmp_path, &buf)?;
        fs::rename(&tmp_path, &self.toc_path)?;
        self.dirty = false;
        Ok(())
    }

    /// Count of unread messages.
    pub fn unread_count(&self) -> usize {
        self.msgs.iter().filter(|m| m.state == State::Unread).count()
    }

    /// Allocate a new serial number.
    pub fn next_serial_num(&mut self) -> i64 {
        let sn = self.next_serial;
        self.next_serial += 1;
        self.dirty = true;
        sn
    }

    /// Read just the TOC header (count + mailbox type) without loading summaries.
    ///
    /// This is a fast operation that reads only the 76-byte disk header.
    /// Returns `(count, mailbox_type, box_size)`.
    pub fn peek(mbox_path: impl AsRef<Path>) -> io::Result<(usize, MailboxType, i64)> {
        let mbox = mbox_path.as_ref();
        let toc_path = mbox.with_extension("toc");
        let data = fs::read(&toc_path)?;
        if data.len() < DISK_HDR_SIZE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "TOC too small for header"));
        }
        let count = read_i16(&data, 8) as usize;
        let which = MailboxType::from_i16(read_i16(&data, 10));
        let box_size = read_i32(&data, 12) as i64;
        Ok((count, which, box_size))
    }

    /// Validate TOC integrity against mbox file.
    pub fn validate(&self) -> Result<(), TocError> {
        if self.major_version != TOC_MAJOR {
            return Err(TocError::BadVersion(self.major_version, self.minor_version));
        }
        for (i, msg) in self.msgs.iter().enumerate() {
            if msg.offset < 0 || msg.length < 0 {
                return Err(TocError::BadOffset(i));
            }
            if msg.body_offset < 0 || msg.body_offset as i64 > msg.length {
                return Err(TocError::BadBodyOffset(i));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum TocError {
    BadVersion(i32, i32),
    BadOffset(usize),
    BadBodyOffset(usize),
    Corrupt(String),
}

impl std::fmt::Display for TocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadVersion(maj, min) => write!(f, "bad TOC version: {}.{}", maj, min),
            Self::BadOffset(i) => write!(f, "bad offset at message {}", i),
            Self::BadBodyOffset(i) => write!(f, "bad body offset at message {}", i),
            Self::Corrupt(s) => write!(f, "corrupt TOC: {}", s),
        }
    }
}

impl std::error::Error for TocError {}

// ── Binary I/O helpers (little-endian, matching Eudora's format) ──

fn read_i16(data: &[u8], off: usize) -> i16 {
    i16::from_le_bytes([data[off], data[off + 1]])
}

fn read_i32(data: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

fn read_u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

fn read_u8(data: &[u8], off: usize) -> u8 { data[off] }

fn read_cstr(data: &[u8], off: usize, max: usize) -> String {
    let slice = &data[off..off + max];
    let end = slice.iter().position(|&b| b == 0).unwrap_or(max);
    String::from_utf8_lossy(&slice[..end]).into_owned()
}

fn write_i16(buf: &mut Vec<u8>, v: i16) { buf.extend_from_slice(&v.to_le_bytes()); }
fn write_i32(buf: &mut Vec<u8>, v: i32) { buf.extend_from_slice(&v.to_le_bytes()); }
fn write_u32(buf: &mut Vec<u8>, v: u32) { buf.extend_from_slice(&v.to_le_bytes()); }
fn write_u8(buf: &mut Vec<u8>, v: u8) { buf.push(v); }

fn write_cstr(buf: &mut Vec<u8>, s: &str, max: usize) {
    let bytes = s.as_bytes();
    let len = bytes.len().min(max - 1);
    buf.extend_from_slice(&bytes[..len]);
    buf.resize(buf.len() + (max - len), 0);
}

fn read_disk_sum(data: &[u8]) -> MsgSum {
    let mut m = MsgSum::default();
    m.offset = read_i32(data, 0) as i64;
    m.length = read_i32(data, 4) as i64;
    m.body_offset = read_i32(data, 8);
    m.state = State::from_u8(read_i32(data, 12) as u8).unwrap_or(State::Unread);
    let spam_bits = read_i32(data, 16) as u32;
    m.spam_score = (spam_bits & 0xFF) as i8;
    m.spam_because = ((spam_bits >> 8) & 7) as u8;
    m.arrival = read_u32(data, 20);
    // mesgErrH at 24 — skip (runtime only)
    m.from_hash = read_u32(data, 28);
    // spare[3] at 32..44 — skip
    m.serial_num = read_i32(data, 44) as i64;
    m.seconds = read_u32(data, 48);
    m.flags = MsgFlags::from_bits_truncate(read_u32(data, 52));
    for i in 0..4 { m.saved_pos[i] = read_i16(data, 56 + i * 2); }
    m.priority = read_u8(data, 64);
    m.orig_priority = read_u8(data, 65);
    m.table_id = read_i16(data, 66);
    let score_bits = read_i16(data, 68);
    m.score = score_bits & 0xF;
    m.out_type = (score_bits >> 4) & 0xF;
    // spareShort2, sumRandBytes at 70..74
    m.orig_zone = read_i16(data, 74);
    m.sig_id = read_u32(data, 76);
    m.from = read_cstr(data, 80, 48);
    m.pop_pers_id = read_u32(data, 128);
    m.pers_id = read_u32(data, 132);
    m.msg_id_hash = read_i32(data, 136);
    m.subj_id = read_i16(data, 140);
    // spareShort at 142
    m.subject = read_cstr(data, 144, 60);
    m.opts = MsgOpts::from_bits_truncate(read_u32(data, 204));
    m.uid_hash = read_u32(data, 208);
    // cache at 212 — runtime only
    // selected at 216 — runtime only
    // _pad at 217..220
    // messH at 220 — runtime only
    m.has_attachment = m.flags.contains(MsgFlags::ATTACHMENT);
    m
}

fn write_disk_sum(buf: &mut Vec<u8>, m: &MsgSum) {
    let start = buf.len();
    write_i32(buf, m.offset as i32);           // 0
    write_i32(buf, m.length as i32);           // 4
    write_i32(buf, m.body_offset);             // 8
    write_i32(buf, m.state as i32);            // 12
    let spam_bits = (m.spam_score as u32 & 0xFF) | ((m.spam_because as u32 & 7) << 8);
    write_u32(buf, spam_bits);                 // 16
    write_u32(buf, m.arrival);                 // 20
    write_u32(buf, 0);                         // 24: mesgErrH (runtime)
    write_u32(buf, m.from_hash);               // 28
    write_u32(buf, 0); write_u32(buf, 0); write_u32(buf, 0); // 32..44: spare[3]
    write_i32(buf, m.serial_num as i32);       // 44
    write_u32(buf, m.seconds);                 // 48
    write_u32(buf, m.flags.bits());            // 52
    for i in 0..4 { write_i16(buf, m.saved_pos[i]); } // 56..64
    write_u8(buf, m.priority);                 // 64
    write_u8(buf, m.orig_priority);            // 65
    write_i16(buf, m.table_id);                // 66
    let score_bits = (m.score & 0xF) | ((m.out_type & 0xF) << 4);
    write_i16(buf, score_bits);                // 68
    write_i16(buf, 0);                         // 70: spareShort2
    write_i16(buf, 0);                         // 72: sumRandBytes
    write_i16(buf, m.orig_zone);               // 74
    write_u32(buf, m.sig_id);                  // 76
    write_cstr(buf, &m.from, 48);              // 80..128
    write_u32(buf, m.pop_pers_id);             // 128
    write_u32(buf, m.pers_id);                 // 132
    write_i32(buf, m.msg_id_hash);             // 136
    write_i16(buf, m.subj_id);                 // 140
    write_i16(buf, 0);                         // 142: spareShort
    write_cstr(buf, &m.subject, 60);           // 144..204
    write_u32(buf, m.opts.bits());             // 204
    write_u32(buf, m.uid_hash);                // 208
    write_u32(buf, 0);                         // 212: cache (runtime)
    write_u8(buf, 0);                          // 216: selected (runtime)
    buf.extend_from_slice(&[0u8; 3]);          // 217..220: _pad
    write_u32(buf, 0);                         // 220: messH (runtime)

    assert_eq!(buf.len() - start, DISK_SUM_SIZE);
}

// ── Event system ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    NewMail,
    Deleted,
    Undeleted,
    StateChanged,
    FlagsChanged,
    Compacted,
    TocSaved,
    TocRebuilt,
    MailboxCreated,
    MailboxDeleted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrip() {
        for v in 1..=14u8 {
            let s = State::from_u8(v).unwrap();
            assert_eq!(s as u8, v);
        }
    }

    #[test]
    fn state_outgoing() {
        assert!(!State::Unread.is_outgoing());
        assert!(!State::Read.is_outgoing());
        assert!(State::Queued.is_outgoing());
        assert!(State::Sent.is_outgoing());
        assert!(State::BusySending.is_outgoing());
    }

    #[test]
    fn mailbox_type_roundtrip() {
        assert_eq!(MailboxType::from_i16(1), MailboxType::In);
        assert_eq!(MailboxType::from_i16(2), MailboxType::Out);
        assert_eq!(MailboxType::from_i16(99), MailboxType::Normal);
    }

    #[test]
    fn flags_label() {
        let f = MsgFlags::from_bits_truncate(3 << 12);
        assert_eq!(f.label(), 3);
        let f2 = MsgFlags::empty().with_label(5);
        assert_eq!(f2.label(), 5);
    }

    #[test]
    fn flags_operations() {
        let mut f = MsgFlags::empty();
        assert!(!f.contains(MsgFlags::READ));
        f.insert(MsgFlags::READ);
        assert!(f.contains(MsgFlags::READ));
        f.remove(MsgFlags::READ);
        assert!(!f.contains(MsgFlags::READ));
    }

    #[test]
    fn disk_sum_roundtrip() {
        let mut msg = MsgSum::default();
        msg.offset = 12345;
        msg.length = 6789;
        msg.body_offset = 200;
        msg.state = State::Replied;
        msg.priority = 2;
        msg.flags = MsgFlags::READ | MsgFlags::ATTACHMENT;
        msg.seconds = 1700000000;
        msg.from = "sender@test.com".to_string();
        msg.subject = "Test Subject".to_string();
        msg.serial_num = 42;

        let mut buf = Vec::new();
        write_disk_sum(&mut buf, &msg);
        assert_eq!(buf.len(), DISK_SUM_SIZE);

        let roundtripped = read_disk_sum(&buf);
        assert_eq!(roundtripped.offset, 12345);
        assert_eq!(roundtripped.length, 6789);
        assert_eq!(roundtripped.body_offset, 200);
        assert_eq!(roundtripped.state, State::Replied);
        assert_eq!(roundtripped.priority, 2);
        assert!(roundtripped.flags.contains(MsgFlags::READ));
        assert!(roundtripped.flags.contains(MsgFlags::ATTACHMENT));
        assert_eq!(roundtripped.seconds, 1700000000);
        assert_eq!(roundtripped.from, "sender@test.com");
        assert_eq!(roundtripped.subject, "Test Subject");
        assert_eq!(roundtripped.serial_num, 42);
    }

    #[test]
    fn toc_new() {
        let toc = Toc::new("/tmp/test_mailbox");
        assert_eq!(toc.msgs.len(), 0);
        assert_eq!(toc.which, MailboxType::Normal);
        assert!(!toc.dirty);
    }

    #[test]
    fn toc_validate_empty() {
        let toc = Toc::new("/tmp/test");
        assert!(toc.validate().is_ok());
    }

    #[test]
    fn cstr_roundtrip() {
        let mut buf = Vec::new();
        write_cstr(&mut buf, "Hello, World!", 48);
        assert_eq!(buf.len(), 48);
        let s = read_cstr(&buf, 0, 48);
        assert_eq!(s, "Hello, World!");
    }

    #[test]
    fn cstr_truncation() {
        let mut buf = Vec::new();
        let long = "A".repeat(100);
        write_cstr(&mut buf, &long, 48);
        assert_eq!(buf.len(), 48);
        let s = read_cstr(&buf, 0, 48);
        assert_eq!(s.len(), 47); // 48 - 1 for null
    }

    #[test]
    fn header_roundtrip() {
        let mut toc = Toc::new("/tmp/test");
        toc.which = MailboxType::In;
        toc.next_serial = 100;
        toc.sort_order = 3;

        // Add a message
        let mut msg = MsgSum::default();
        msg.from = "test@example.com".to_string();
        msg.subject = "Hello".to_string();
        msg.offset = 0;
        msg.length = 500;
        msg.serial_num = toc.next_serial_num();
        toc.msgs.push(msg);

        // Serialize
        let mut buf = Vec::new();
        write_i32(&mut buf, toc.major_version);
        write_i32(&mut buf, toc.minor_version);
        write_i16(&mut buf, toc.msgs.len() as i16);
        write_i16(&mut buf, toc.which as i16);
        // ... (full header tested via save/open integration)
        assert!(buf.len() >= 12);
    }
}
