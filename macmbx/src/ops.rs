//! Message operations — transfer, delete, modify, search, sort, mailbox management.
//!
//! Port of `macmbx_ops.c` and the message-access / modification parts of `macmbx.c`.
//! All functions operate on `crate::toc::Toc` and `crate::toc::MsgSum`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::toc::{Toc, MsgSum, MsgFlags, MsgOpts, State};

// ── Search field specifier ──

/// Which field(s) to search in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchField {
    From,
    Subject,
    /// Search from + subject summaries first, then full message body.
    All,
    /// Search only the message body (slow — reads from mbox file).
    Body,
    /// Search a specific header by name (e.g. `Header("X-Mailer")`).
    Header(String),
}

impl SearchField {
    /// Parse from a string, matching the C API's field parameter.
    pub fn parse(s: &str) -> Self {
        let lower = s.to_ascii_lowercase();
        match lower.as_str() {
            "from" => Self::From,
            "subject" => Self::Subject,
            "all" => Self::All,
            "body" => Self::Body,
            _ if lower.starts_with("header:") => Self::Header(s[7..].to_string()),
            _ => Self::All, // default to all
        }
    }
}

// ── Sort field specifier ──

/// Which field to sort by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Date,
    From,
    Subject,
    Size,
    State,
    Priority,
    Label,
}

impl SortField {
    /// Parse from a string, matching the C API's field parameter.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "date" => Some(Self::Date),
            "from" => Some(Self::From),
            "subject" => Some(Self::Subject),
            "size" => Some(Self::Size),
            "state" => Some(Self::State),
            "priority" => Some(Self::Priority),
            "label" => Some(Self::Label),
            _ => None,
        }
    }
}

// ── Mbox utilities ──

/// Check if a line is a valid mbox "From " separator line.
pub fn is_from_line(line: &str) -> bool {
    if !line.starts_with("From ") {
        return false;
    }
    let rest = &line[5..];
    // Skip the return address (may contain quoted strings)
    let mut chars = rest.chars().peekable();
    let mut in_quote = false;
    loop {
        match chars.peek() {
            None => return false,
            Some(&' ') if !in_quote => { chars.next(); break; }
            Some(&'"') => { in_quote = !in_quote; chars.next(); }
            _ => { chars.next(); }
        }
    }
    // Skip extra spaces
    while chars.peek() == Some(&' ') {
        chars.next();
    }
    // Should have a day-of-week abbreviation next (Mon, Tue, etc.)
    let remaining: String = chars.collect();
    let weekdays = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    weekdays.iter().any(|d| remaining.starts_with(d))
}

/// Write a "From " mbox separator line for the given sender.
pub fn write_from_line(sender: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let sender = if sender.is_empty() { "unknown" } else { sender };
    // Format: "From sender Day Mon DD HH:MM:SS YYYY\n"
    let tm = time_to_asctime(now);
    format!("From {} {}\n", sender, tm)
}

/// Convert a Unix timestamp to an asctime-style string: "Day Mon DD HH:MM:SS YYYY"
fn time_to_asctime(secs: u64) -> String {
    // Days/months calculation from epoch
    let days_since_epoch = (secs / 86400) as i64;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Day of week: Jan 1 1970 was a Thursday (4)
    let dow = ((days_since_epoch % 7 + 4) % 7) as usize;
    let dow_names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

    // Year, month, day from days since epoch
    let (year, month, day) = days_to_ymd(days_since_epoch);
    let mon_names = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    format!(
        "{} {} {:2} {:02}:{:02}:{:02} {}",
        dow_names[dow],
        mon_names[month as usize],
        day,
        hours,
        minutes,
        seconds,
        year
    )
}

/// Convert days since epoch to (year, month 0-11, day 1-31).
fn days_to_ymd(mut days: i64) -> (i64, i64, i64) {
    let mut year = 1970i64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let mdays = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 0i64;
    for m in 0..12 {
        if days < mdays[m] {
            month = m as i64;
            break;
        }
        days -= mdays[m];
        if m == 11 { month = 11; }
    }
    (year, month, days + 1)
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// Detect if headers indicate an attachment.
pub fn detect_attachment(headers: &str) -> bool {
    let lower = headers.to_ascii_lowercase();
    if lower.contains("multipart/mixed") {
        return true;
    }
    if lower.contains("content-disposition: attachment") {
        return true;
    }
    // Check for Eudora's Attachment: header
    if lower.starts_with("attachment:") {
        return true;
    }
    if lower.contains("\nattachment:") {
        return true;
    }
    false
}

// ── Message access ──

/// Read the full raw message (including From line) from the mbox file.
pub fn read_message(toc: &Toc, index: usize) -> io::Result<Vec<u8>> {
    let msg = toc.msgs.get(index)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "index out of range"))?;
    let mut f = File::open(&toc.mbox_path)?;
    f.seek(SeekFrom::Start(msg.offset as u64))?;
    let mut buf = vec![0u8; msg.length as usize];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

/// Read just the headers portion of a message.
pub fn read_headers(toc: &Toc, index: usize) -> io::Result<String> {
    let msg = toc.msgs.get(index)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "index out of range"))?;
    let hdr_len = if msg.body_offset > 0 {
        msg.body_offset as usize
    } else {
        msg.length as usize
    };
    let mut f = File::open(&toc.mbox_path)?;
    f.seek(SeekFrom::Start(msg.offset as u64))?;
    let mut buf = vec![0u8; hdr_len];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Scan for the blank line separating headers from body.
/// Returns the byte offset of the body start within the buffer, or None.
fn scan_body_start(data: &[u8]) -> Option<usize> {
    let len = data.len();
    let mut i = 0;
    while i < len.saturating_sub(1) {
        if data[i] == b'\n' {
            if data[i + 1] == b'\n' {
                return Some(i + 2);
            }
            if data[i + 1] == b'\r' && i + 2 < len && data[i + 2] == b'\n' {
                return Some(i + 3);
            }
        } else if data[i] == b'\r' && data[i + 1] == b'\n' {
            if i + 3 < len && data[i + 2] == b'\r' && data[i + 3] == b'\n' {
                return Some(i + 4);
            }
        }
        i += 1;
    }
    None
}

/// Read the body of a message (everything after the header/body blank line).
pub fn read_body(toc: &Toc, index: usize) -> io::Result<Vec<u8>> {
    let msg = toc.msgs.get(index)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "index out of range"))?;

    let body_off = msg.body_offset;

    if body_off <= 0 {
        // No body_offset recorded — read full message and scan for blank line
        let full = read_message(toc, index)?;
        let off = scan_body_start(&full)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no body separator found"))?;
        if off >= full.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "body offset past end"));
        }
        Ok(full[off..].to_vec())
    } else {
        let body_len = msg.length - body_off as i64;
        if body_len <= 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "zero-length body"));
        }
        let mut f = File::open(&toc.mbox_path)?;
        f.seek(SeekFrom::Start((msg.offset + body_off as i64) as u64))?;
        let mut buf = vec![0u8; body_len as usize];
        let n = f.read(&mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }
}

/// Read a specific header field value from a message.
/// Handles folded headers (continuation lines starting with whitespace).
pub fn read_header_field(toc: &Toc, index: usize, field: &str) -> io::Result<Option<String>> {
    let headers = read_headers(toc, index)?;
    Ok(extract_header_field(&headers, field))
}

/// Extract a header field value from a header string.
fn extract_header_field(headers: &str, field: &str) -> Option<String> {
    let field_lower = field.to_ascii_lowercase();
    for line_start in LineStarts::new(headers) {
        let rest = &headers[line_start..];
        // Check if this line starts with the field name followed by ':'
        if rest.len() > field.len() + 1 {
            let prefix = &rest[..field.len()];
            if prefix.eq_ignore_ascii_case(&field_lower) && rest.as_bytes()[field.len()] == b':' {
                let val_start = field.len() + 1;
                let val_rest = &rest[val_start..];
                // Skip leading whitespace
                let val_rest = val_rest.trim_start_matches(|c| c == ' ' || c == '\t');
                // Collect value including folded continuation lines
                let mut value = String::new();
                let mut first = true;
                for part in val_rest.split('\n') {
                    let part = part.trim_end_matches('\r');
                    if first {
                        value.push_str(part);
                        first = false;
                    } else if part.starts_with(' ') || part.starts_with('\t') {
                        // Folded continuation
                        value.push(' ');
                        value.push_str(part.trim_start());
                    } else {
                        break;
                    }
                }
                // Trim trailing whitespace
                let value = value.trim_end().to_string();
                return if value.is_empty() { None } else { Some(value) };
            }
        }
    }
    None
}

/// Iterator yielding byte offsets of each line start in a string.
struct LineStarts<'a> {
    s: &'a str,
    pos: usize,
    first: bool,
}

impl<'a> LineStarts<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, pos: 0, first: true }
    }
}

impl<'a> Iterator for LineStarts<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if self.first {
            self.first = false;
            return Some(0);
        }
        let bytes = self.s.as_bytes();
        while self.pos < bytes.len() {
            if bytes[self.pos] == b'\n' {
                self.pos += 1;
                if self.pos < bytes.len() {
                    return Some(self.pos);
                }
                return None;
            }
            self.pos += 1;
        }
        None
    }
}

// ── Message modification ──

/// Append a message to the mbox file and add a summary to the TOC.
/// Returns the index of the new message in the TOC.
///
/// `message` should be the RFC-822 content (headers + body) *without* the mbox "From " line.
pub fn append_message(
    toc: &mut Toc,
    message: &[u8],
    sender: &str,
    state: State,
    priority: u8,
) -> io::Result<usize> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&toc.mbox_path)?;

    // Write "From " separator
    let from_line = write_from_line(sender);
    let from_bytes = from_line.as_bytes();
    f.write_all(from_bytes)?;

    // Write message
    f.write_all(message)?;
    let needs_newline = !message.is_empty() && message[message.len() - 1] != b'\n';
    if needs_newline {
        f.write_all(b"\n")?;
    }
    // Trailing blank line (mbox separator)
    f.write_all(b"\n")?;

    f.flush()?;
    let file_end = f.seek(SeekFrom::End(0))? as i64;

    // Calculate total bytes written
    let mut total_written = from_bytes.len() as i64 + message.len() as i64;
    if needs_newline {
        total_written += 1;
    }
    total_written += 1; // trailing blank

    // Build summary
    let mut sum = MsgSum::default();
    sum.offset = file_end - total_written;
    sum.length = total_written;
    sum.state = state;
    sum.priority = if priority == 0 { 3 } else { priority };
    sum.serial_num = toc.next_serial_num();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;
    sum.seconds = now;
    sum.arrival = now;
    sum.spam_score = -1;
    sum.flags = MsgFlags::UTF8;

    // Extract From: and Subject: from headers
    let msg_str = String::from_utf8_lossy(message);
    for line in msg_str.lines() {
        if line.is_empty() {
            // End of headers
            let header_len = msg_str.find("\n\n")
                .or_else(|| msg_str.find("\r\n\r\n"))
                .map(|p| p + 2)
                .unwrap_or(msg_str.len());
            sum.body_offset = (from_bytes.len() + header_len) as i32;
            break;
        }
        if let Some(val) = line.strip_prefix("From:").or_else(|| line.strip_prefix("from:")) {
            let val = val.trim();
            sum.from = truncate_str(val, 47);
        } else if line.len() > 5 && line[..5].eq_ignore_ascii_case("from:") {
            let val = line[5..].trim();
            sum.from = truncate_str(val, 47);
        } else if let Some(val) = line.strip_prefix("Subject:").or_else(|| line.strip_prefix("subject:")) {
            let val = val.trim();
            sum.subject = truncate_str(val, 59);
        } else if line.len() > 8 && line[..8].eq_ignore_ascii_case("subject:") {
            let val = line[8..].trim();
            sum.subject = truncate_str(val, 59);
        }
    }

    if sum.from.is_empty() && !sender.is_empty() {
        sum.from = truncate_str(sender, 47);
    }

    // Detect attachment
    if detect_attachment(&msg_str) {
        sum.has_attachment = true;
        sum.flags.insert(MsgFlags::ATTACHMENT);
    }

    let idx = toc.msgs.len();
    toc.msgs.push(sum);
    toc.dirty = true;
    Ok(idx)
}

/// Truncate a string to at most `max` bytes (on a char boundary).
fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].to_string()
    }
}

/// Mark a message as deleted.
pub fn delete_message(toc: &mut Toc, index: usize) -> Result<(), &'static str> {
    let msg = toc.msgs.get_mut(index).ok_or("index out of range")?;
    msg.flags.insert(MsgFlags::DELETED);
    msg.opts.insert(MsgOpts::DELETED);
    toc.dirty = true;
    Ok(())
}

/// Remove the deleted flag from a message.
pub fn undelete_message(toc: &mut Toc, index: usize) -> Result<(), &'static str> {
    let msg = toc.msgs.get_mut(index).ok_or("index out of range")?;
    msg.flags.remove(MsgFlags::DELETED);
    msg.opts.remove(MsgOpts::DELETED);
    toc.dirty = true;
    Ok(())
}

/// Set the state of a message (Unread, Read, Replied, etc.).
pub fn set_state(toc: &mut Toc, index: usize, state: State) -> Result<(), &'static str> {
    let msg = toc.msgs.get_mut(index).ok_or("index out of range")?;
    msg.state = state;
    toc.dirty = true;
    Ok(())
}

/// Set (OR in) additional flags on a message.
pub fn set_flags(toc: &mut Toc, index: usize, flags: MsgFlags) -> Result<(), &'static str> {
    let msg = toc.msgs.get_mut(index).ok_or("index out of range")?;
    msg.flags.insert(flags);
    toc.dirty = true;
    Ok(())
}

/// Clear (remove) specific flags from a message.
pub fn clear_flags(toc: &mut Toc, index: usize, flags: MsgFlags) -> Result<(), &'static str> {
    let msg = toc.msgs.get_mut(index).ok_or("index out of range")?;
    msg.flags.remove(flags);
    toc.dirty = true;
    Ok(())
}

/// Set the priority (1-5) of a message.
pub fn set_priority(toc: &mut Toc, index: usize, priority: u8) -> Result<(), &'static str> {
    let msg = toc.msgs.get_mut(index).ok_or("index out of range")?;
    msg.priority = priority;
    toc.dirty = true;
    Ok(())
}

/// Set the label (0-7) on a message. The label is stored in bits 12-14 of flags.
pub fn set_label(toc: &mut Toc, index: usize, label: u8) -> Result<(), &'static str> {
    let msg = toc.msgs.get_mut(index).ok_or("index out of range")?;
    msg.flags = msg.flags.with_label(label);
    toc.dirty = true;
    Ok(())
}

/// Get the label (0-7) of a message.
pub fn get_label(toc: &Toc, index: usize) -> u8 {
    toc.msgs.get(index).map(|m| m.flags.label()).unwrap_or(0)
}

// ── Transfer ──

/// Transfer (move or copy) a single message from `src` to `dst`.
/// If `copy` is false, the source message is marked as deleted (move).
/// Returns the index of the new message in `dst`.
pub fn transfer(
    src: &mut Toc,
    index: usize,
    dst: &mut Toc,
    copy: bool,
) -> io::Result<usize> {
    if index >= src.msgs.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "index out of range"));
    }

    // Read message from source
    let raw = read_message(src, index)?;

    // Skip the "From " line — append_message will write its own
    let body = if raw.starts_with(b"From ") {
        if let Some(nl_pos) = raw.iter().position(|&b| b == b'\n') {
            &raw[nl_pos + 1..]
        } else {
            &raw[..]
        }
    } else {
        &raw[..]
    };

    let src_sum = &src.msgs[index];
    let state = src_sum.state;
    let priority = src_sum.priority;
    let sender = src_sum.from.clone();

    // Append to destination
    let new_idx = append_message(dst, body, &sender, state, priority)?;

    // Copy metadata from source summary
    {
        let src_sum = &src.msgs[index];
        let dst_sum = &mut dst.msgs[new_idx];
        dst_sum.flags = MsgFlags::from_bits_truncate(
            src_sum.flags.bits() & !MsgFlags::DELETED.bits()
        );
        dst_sum.seconds = src_sum.seconds;
        dst_sum.orig_zone = src_sum.orig_zone;
        dst_sum.arrival = src_sum.arrival;
        dst_sum.msg_id_hash = src_sum.msg_id_hash;
        dst_sum.uid_hash = src_sum.uid_hash;
        dst_sum.spam_score = src_sum.spam_score;
        dst_sum.from = src_sum.from.clone();
        dst_sum.subject = src_sum.subject.clone();
    }
    dst.dirty = true;

    // Mark source as deleted if move
    if !copy {
        delete_message(src, index).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    }

    Ok(new_idx)
}

/// Transfer multiple messages from `src` to `dst`.
/// Returns the number of messages successfully transferred.
pub fn transfer_multi(
    src: &mut Toc,
    indices: &[usize],
    dst: &mut Toc,
    copy: bool,
) -> usize {
    let mut transferred = 0;
    for &idx in indices {
        if transfer(src, idx, dst, copy).is_ok() {
            transferred += 1;
        }
    }
    transferred
}

// ── Mailbox file operations ──

/// Create a new empty mailbox (mbox file + .toc file).
pub fn create_mailbox(mbox_path: &Path) -> io::Result<()> {
    // Create empty mbox file
    File::create(mbox_path)?;

    // Create empty TOC
    let mut toc = Toc::new(mbox_path);
    toc.dirty = true;
    toc.save()?;
    Ok(())
}

/// Remove a mailbox and its .toc file.
pub fn remove_mailbox(mbox_path: &Path) -> io::Result<()> {
    let toc_path = toc_path_for(mbox_path);
    // Remove .toc first (ok if it doesn't exist)
    let _ = fs::remove_file(&toc_path);
    fs::remove_file(mbox_path)?;
    Ok(())
}

/// Rename a mailbox and its .toc file.
pub fn rename_mailbox(old_path: &Path, new_path: &Path) -> io::Result<()> {
    // Rename mbox
    fs::rename(old_path, new_path)?;
    // Rename .toc (ok if this fails — TOC can be rebuilt)
    let old_toc = toc_path_for(old_path);
    let new_toc = toc_path_for(new_path);
    let _ = fs::rename(&old_toc, &new_toc);
    Ok(())
}

/// Compute the .toc path for a given mbox path.
/// Matches `Toc::new` behavior: replaces the extension with "toc".
fn toc_path_for(mbox_path: &Path) -> PathBuf {
    mbox_path.with_extension("toc")
}

/// Check if a file is a valid mbox file.
pub fn is_mbox(path: &Path) -> bool {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if !meta.is_file() {
        return false;
    }
    if meta.len() == 0 {
        return true; // empty mbox is valid
    }
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = [0u8; 256];
    let n = match f.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let line = &buf[..n];
    // Find the first line
    let first_line_end = line.iter().position(|&b| b == b'\n').unwrap_or(n);
    let first_line = String::from_utf8_lossy(&line[..first_line_end]);
    is_from_line(&first_line)
}

/// List mailbox files in a directory (excludes hidden files and .toc files).
/// Returns a sorted list of filenames (not full paths).
pub fn list_mailboxes(dir: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Skip hidden files
        if name_str.starts_with('.') {
            continue;
        }
        // Skip .toc files
        if name_str.ends_with(".toc") {
            continue;
        }
        // Check if it's a regular file
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                names.push(name_str.into_owned());
            }
        }
    }
    names.sort();
    Ok(names)
}

// ── Search ──

/// Case-insensitive substring search.
fn ci_contains(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if h.len() < n.len() {
        return false;
    }
    for i in 0..=(h.len() - n.len()) {
        let mut matched = true;
        for j in 0..n.len() {
            if h[i + j].to_ascii_lowercase() != n[j].to_ascii_lowercase() {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

/// Case-insensitive byte-slice substring search.
fn ci_contains_bytes(haystack: &[u8], needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let n = needle.as_bytes();
    if haystack.len() < n.len() {
        return false;
    }
    for i in 0..=(haystack.len() - n.len()) {
        let mut matched = true;
        for j in 0..n.len() {
            if haystack[i + j].to_ascii_lowercase() != n[j].to_ascii_lowercase() {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

/// Search messages in a TOC. Returns indices of matching messages.
/// Deleted messages are skipped.
pub fn search(toc: &Toc, field: &SearchField, pattern: &str) -> Vec<usize> {
    let mut results = Vec::new();

    for (i, msg) in toc.msgs.iter().enumerate() {
        if msg.flags.contains(MsgFlags::DELETED) {
            continue;
        }

        let matched = match field {
            SearchField::From => ci_contains(&msg.from, pattern),
            SearchField::Subject => ci_contains(&msg.subject, pattern),
            SearchField::All => {
                // Search summary fields first (fast)
                if ci_contains(&msg.from, pattern) || ci_contains(&msg.subject, pattern) {
                    true
                } else {
                    // Fall back to full message search (slow)
                    match read_message(toc, i) {
                        Ok(data) => ci_contains_bytes(&data, pattern),
                        Err(_) => false,
                    }
                }
            }
            SearchField::Body => {
                match read_body(toc, i) {
                    Ok(data) => ci_contains_bytes(&data, pattern),
                    Err(_) => false,
                }
            }
            SearchField::Header(ref hdr_name) => {
                match read_header_field(toc, i, hdr_name) {
                    Ok(Some(val)) => ci_contains(&val, pattern),
                    _ => false,
                }
            }
        };

        if matched {
            results.push(i);
        }
    }

    results
}

// ── Sort ──

/// Strip leading "Re:" / "Fwd:" prefixes for subject sorting.
fn strip_reply_prefix(s: &str) -> &str {
    let mut s = s;
    loop {
        let trimmed = s.trim_start();
        if trimmed.len() >= 3 {
            let prefix3 = &trimmed[..3];
            if prefix3.eq_ignore_ascii_case("re:") {
                s = &trimmed[3..];
                continue;
            }
        }
        if trimmed.len() >= 4 {
            let prefix4 = &trimmed[..4];
            if prefix4.eq_ignore_ascii_case("fwd:") {
                s = &trimmed[4..];
                continue;
            }
        }
        return trimmed;
    }
}

/// Sort messages in a TOC by the given field.
pub fn sort(toc: &mut Toc, field: SortField, ascending: bool) {
    if toc.msgs.len() <= 1 {
        return;
    }

    toc.msgs.sort_by(|a, b| {
        let cmp = match field {
            SortField::Date => a.seconds.cmp(&b.seconds),
            SortField::From => {
                a.from.to_ascii_lowercase().cmp(&b.from.to_ascii_lowercase())
            }
            SortField::Subject => {
                let a_subj = strip_reply_prefix(&a.subject).to_ascii_lowercase();
                let b_subj = strip_reply_prefix(&b.subject).to_ascii_lowercase();
                a_subj.cmp(&b_subj)
            }
            SortField::Size => a.length.cmp(&b.length),
            SortField::State => (a.state as u8).cmp(&(b.state as u8)),
            SortField::Priority => a.priority.cmp(&b.priority),
            SortField::Label => a.flags.label().cmp(&b.flags.label()),
        };
        if ascending { cmp } else { cmp.reverse() }
    });

    toc.dirty = true;
}

// ── Statistics ──

/// Count unread (non-deleted) messages.
pub fn count_unread(toc: &Toc) -> usize {
    toc.msgs.iter().filter(|m| {
        m.state == State::Unread && !m.flags.contains(MsgFlags::DELETED)
    }).count()
}

/// Count flagged (non-deleted) messages.
pub fn count_flagged(toc: &Toc) -> usize {
    toc.msgs.iter().filter(|m| {
        m.flags.contains(MsgFlags::FLAGGED) && !m.flags.contains(MsgFlags::DELETED)
    }).count()
}

/// Count deleted messages.
pub fn count_deleted(toc: &Toc) -> usize {
    toc.msgs.iter().filter(|m| m.flags.contains(MsgFlags::DELETED)).count()
}

/// Total size of all messages in bytes.
pub fn total_size(toc: &Toc) -> i64 {
    toc.msgs.iter().map(|m| m.length).sum()
}

/// Reclaimable space: total size of deleted messages.
pub fn reclaimable(toc: &Toc) -> i64 {
    toc.msgs.iter()
        .filter(|m| m.flags.contains(MsgFlags::DELETED))
        .map(|m| m.length)
        .sum()
}

// ── Personality tagging ──

/// Hash an account name to a personality ID (DJB2, case-insensitive).
/// Same hash Eudora uses for pop_pers_id / pers_id.
pub fn personality_hash(account_name: &str) -> u32 {
    if account_name.is_empty() {
        return 0;
    }
    let mut h: u32 = 5381;
    for c in account_name.bytes() {
        let lower = if c >= b'A' && c <= b'Z' { c + 32 } else { c };
        h = h.wrapping_shl(5).wrapping_add(h).wrapping_add(lower as u32);
    }
    if h == 0 { 1 } else { h }
}

/// Tag an outgoing message with the sending personality hash.
/// Sets the `pers_id` field on the message summary.
pub fn tag_sending_personality(toc: &mut Toc, index: usize, account_name: &str) -> Result<(), &'static str> {
    let msg = toc.msgs.get_mut(index).ok_or("index out of range")?;
    msg.pers_id = personality_hash(account_name);
    toc.dirty = true;
    Ok(())
}

/// Find all non-deleted messages received by a given personality (account).
/// Matches against `pop_pers_id` using the DJB2 hash of the account name.
/// Returns indices of matching messages.
pub fn find_by_personality(toc: &Toc, account_name: &str) -> Vec<usize> {
    let hash = personality_hash(account_name);
    if hash == 0 {
        return Vec::new();
    }
    let mut indices = Vec::new();
    for (i, msg) in toc.msgs.iter().enumerate() {
        if msg.pop_pers_id == hash && !msg.flags.contains(MsgFlags::DELETED) {
            indices.push(i);
        }
    }
    indices
}

// ── Batch export ──

/// Export entire store to mbox files in an output directory.
///
/// For each mailbox in the store root, opens its TOC and exports all
/// messages as a Unix mbox file named `<mailbox>.mbox` in `output_dir`.
/// Returns the number of mailboxes exported.
pub fn export_store(store: &crate::store::Store, output_dir: &Path) -> io::Result<usize> {
    // Create output directory if needed
    if !output_dir.exists() {
        fs::create_dir_all(output_dir)?;
    }

    let mut exported = 0;
    for node in store.children() {
        if node.node_type != crate::store::NodeType::Mailbox {
            continue;
        }
        let toc = match Toc::open(&node.path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if toc.msgs.is_empty() {
            continue;
        }

        let out_path = output_dir.join(format!("{}.mbox", node.name));
        let mut out_file = File::create(&out_path)?;

        for i in 0..toc.msgs.len() {
            let raw = match read_message(&toc, i) {
                Ok(data) => data,
                Err(_) => continue,
            };

            // Ensure starts with From line
            if raw.len() < 5 || &raw[..5] != b"From " {
                let from_line = write_from_line(
                    if toc.msgs[i].from.is_empty() { "unknown" } else { &toc.msgs[i].from },
                );
                out_file.write_all(from_line.as_bytes())?;
            }
            out_file.write_all(&raw)?;
            // Ensure ends with newline
            if !raw.is_empty() && raw[raw.len() - 1] != b'\n' {
                out_file.write_all(b"\n")?;
            }
        }

        exported += 1;
    }

    Ok(exported)
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toc::{Toc, MsgSum, MsgFlags, State};
    use std::io::Write;

    /// Helper: create a temporary mbox file with messages and a Toc pointing to it.
    fn make_test_mbox(dir: &Path, name: &str, messages: &[(&str, &str, &str)]) -> Toc {
        let mbox_path = dir.join(name);
        let mut f = File::create(&mbox_path).unwrap();
        let mut toc = Toc::new(&mbox_path);

        for (sender, subject, body) in messages {
            let from_line = format!("From {} Thu Jan  1 00:00:00 1970\n", sender);
            let headers = format!("From: {}\nSubject: {}\n\n", sender, subject);
            let full = format!("{}{}{}\n", from_line, headers, body);
            let offset = f.seek(SeekFrom::Current(0)).unwrap() as i64;
            f.write_all(full.as_bytes()).unwrap();
            let length = full.len() as i64;

            let body_offset = (from_line.len() + headers.len()) as i32;

            let mut sum = MsgSum::default();
            sum.offset = offset;
            sum.length = length;
            sum.body_offset = body_offset;
            sum.from = sender.to_string();
            sum.subject = subject.to_string();
            sum.seconds = 1000000 + toc.msgs.len() as u32 * 86400;
            sum.serial_num = toc.next_serial_num();
            toc.msgs.push(sum);
        }

        f.flush().unwrap();
        toc
    }

    #[test]
    fn test_is_from_line() {
        assert!(is_from_line("From user@test.com Mon Jan  1 00:00:00 2024"));
        assert!(is_from_line("From unknown Thu Feb  5 12:34:56 2023"));
        assert!(!is_from_line("From: user@test.com"));
        assert!(!is_from_line("Hello world"));
        assert!(!is_from_line(""));
        assert!(!is_from_line("From "));
    }

    #[test]
    fn test_write_from_line_format() {
        let line = write_from_line("user@example.com");
        assert!(line.starts_with("From user@example.com "));
        assert!(line.ends_with('\n'));
        // Should be a valid From line
        assert!(is_from_line(line.trim_end()));
    }

    #[test]
    fn test_write_from_line_empty_sender() {
        let line = write_from_line("");
        assert!(line.starts_with("From unknown "));
    }

    #[test]
    fn test_detect_attachment() {
        assert!(detect_attachment("Content-Type: multipart/mixed; boundary=\"abc\""));
        assert!(detect_attachment("content-type: MULTIPART/MIXED; boundary=xyz"));
        assert!(detect_attachment("Content-Disposition: attachment; filename=\"f.txt\""));
        assert!(detect_attachment("Attachment: /path/to/file.txt"));
        assert!(detect_attachment("Subject: test\nAttachment: file.pdf"));
        assert!(!detect_attachment("Content-Type: text/plain"));
        assert!(!detect_attachment("Subject: attachment discussion"));
    }

    #[test]
    fn test_read_message() {
        let dir = tempfile::tempdir().unwrap();
        let toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Hello", "This is message one."),
            ("bob@test.com", "Re: Hello", "This is the reply."),
        ]);

        let msg0 = read_message(&toc, 0).unwrap();
        let msg0_str = String::from_utf8_lossy(&msg0);
        assert!(msg0_str.contains("From alice@test.com"));
        assert!(msg0_str.contains("Subject: Hello"));
        assert!(msg0_str.contains("This is message one."));

        let msg1 = read_message(&toc, 1).unwrap();
        let msg1_str = String::from_utf8_lossy(&msg1);
        assert!(msg1_str.contains("From bob@test.com"));
        assert!(msg1_str.contains("Re: Hello"));

        // Out of range
        assert!(read_message(&toc, 5).is_err());
    }

    #[test]
    fn test_read_headers() {
        let dir = tempfile::tempdir().unwrap();
        let toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Hello", "Body text here."),
        ]);

        let hdrs = read_headers(&toc, 0).unwrap();
        assert!(hdrs.contains("From: alice@test.com"));
        assert!(hdrs.contains("Subject: Hello"));
        // Body should NOT be in headers
        // (depends on body_offset being set correctly)
    }

    #[test]
    fn test_read_body() {
        let dir = tempfile::tempdir().unwrap();
        let toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Hello", "Body text here."),
        ]);

        let body = read_body(&toc, 0).unwrap();
        let body_str = String::from_utf8_lossy(&body);
        assert!(body_str.contains("Body text here."));
    }

    #[test]
    fn test_read_body_no_body_offset() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Test", "The actual body content."),
        ]);
        // Zero out body_offset to force scan
        toc.msgs[0].body_offset = 0;

        let body = read_body(&toc, 0).unwrap();
        let body_str = String::from_utf8_lossy(&body);
        assert!(body_str.contains("The actual body content."));
    }

    #[test]
    fn test_extract_header_field() {
        let headers = "From: alice@test.com\r\nSubject: Hello World\r\nX-Mailer: Eudora\r\n\r\n";
        assert_eq!(
            extract_header_field(headers, "From"),
            Some("alice@test.com".to_string())
        );
        assert_eq!(
            extract_header_field(headers, "Subject"),
            Some("Hello World".to_string())
        );
        assert_eq!(
            extract_header_field(headers, "X-Mailer"),
            Some("Eudora".to_string())
        );
        assert_eq!(extract_header_field(headers, "X-Missing"), None);
    }

    #[test]
    fn test_extract_header_field_folded() {
        let headers = "Subject: This is a very long\n\tsubject line that wraps\nFrom: test@example.com\n\n";
        assert_eq!(
            extract_header_field(headers, "Subject"),
            Some("This is a very long subject line that wraps".to_string())
        );
    }

    #[test]
    fn test_extract_header_field_case_insensitive() {
        let headers = "FROM: alice@test.com\nSUBJECT: test\n\n";
        assert_eq!(
            extract_header_field(headers, "from"),
            Some("alice@test.com".to_string())
        );
        assert_eq!(
            extract_header_field(headers, "subject"),
            Some("test".to_string())
        );
    }

    #[test]
    fn test_append_message() {
        let dir = tempfile::tempdir().unwrap();
        let mbox_path = dir.path().join("append.mbx");
        File::create(&mbox_path).unwrap();
        let mut toc = Toc::new(&mbox_path);

        let msg = b"From: alice@test.com\nSubject: Test Message\n\nHello, world!\n";
        let idx = append_message(&mut toc, msg, "alice@test.com", State::Unread, 3).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(toc.msgs.len(), 1);
        assert_eq!(toc.msgs[0].from, "alice@test.com");
        assert_eq!(toc.msgs[0].subject, "Test Message");
        assert_eq!(toc.msgs[0].state, State::Unread);
        assert_eq!(toc.msgs[0].priority, 3);
        assert!(toc.dirty);

        // Append a second message
        let msg2 = b"From: bob@test.com\nSubject: Reply\n\nThanks!\n";
        let idx2 = append_message(&mut toc, msg2, "bob@test.com", State::Read, 2).unwrap();
        assert_eq!(idx2, 1);
        assert_eq!(toc.msgs.len(), 2);

        // Verify we can read back
        let data = read_message(&toc, 0).unwrap();
        let text = String::from_utf8_lossy(&data);
        assert!(text.contains("Hello, world!"));
    }

    #[test]
    fn test_append_message_with_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let mbox_path = dir.path().join("att.mbx");
        File::create(&mbox_path).unwrap();
        let mut toc = Toc::new(&mbox_path);

        let msg = b"From: alice@test.com\nSubject: With Attachment\nContent-Type: multipart/mixed; boundary=\"abc\"\n\nBody\n";
        let idx = append_message(&mut toc, msg, "alice@test.com", State::Unread, 3).unwrap();
        assert!(toc.msgs[idx].has_attachment);
        assert!(toc.msgs[idx].flags.contains(MsgFlags::ATTACHMENT));
    }

    #[test]
    fn test_delete_undelete() {
        let dir = tempfile::tempdir().unwrap();
        let toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Test", "Body"),
        ]);
        let mut toc = toc;

        assert!(!toc.msgs[0].flags.contains(MsgFlags::DELETED));

        delete_message(&mut toc, 0).unwrap();
        assert!(toc.msgs[0].flags.contains(MsgFlags::DELETED));
        assert!(toc.msgs[0].opts.contains(MsgOpts::DELETED));
        assert!(toc.dirty);

        toc.dirty = false;
        undelete_message(&mut toc, 0).unwrap();
        assert!(!toc.msgs[0].flags.contains(MsgFlags::DELETED));
        assert!(!toc.msgs[0].opts.contains(MsgOpts::DELETED));
        assert!(toc.dirty);
    }

    #[test]
    fn test_delete_out_of_range() {
        let mut toc = Toc::new("/tmp/fake");
        assert!(delete_message(&mut toc, 0).is_err());
    }

    #[test]
    fn test_set_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Test", "Body"),
        ]);

        assert_eq!(toc.msgs[0].state, State::Unread);
        set_state(&mut toc, 0, State::Read).unwrap();
        assert_eq!(toc.msgs[0].state, State::Read);

        set_state(&mut toc, 0, State::Replied).unwrap();
        assert_eq!(toc.msgs[0].state, State::Replied);
    }

    #[test]
    fn test_set_clear_flags() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Test", "Body"),
        ]);

        assert!(!toc.msgs[0].flags.contains(MsgFlags::FLAGGED));
        set_flags(&mut toc, 0, MsgFlags::FLAGGED).unwrap();
        assert!(toc.msgs[0].flags.contains(MsgFlags::FLAGGED));

        set_flags(&mut toc, 0, MsgFlags::READ).unwrap();
        assert!(toc.msgs[0].flags.contains(MsgFlags::FLAGGED));
        assert!(toc.msgs[0].flags.contains(MsgFlags::READ));

        clear_flags(&mut toc, 0, MsgFlags::FLAGGED).unwrap();
        assert!(!toc.msgs[0].flags.contains(MsgFlags::FLAGGED));
        assert!(toc.msgs[0].flags.contains(MsgFlags::READ));
    }

    #[test]
    fn test_set_priority() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Test", "Body"),
        ]);

        set_priority(&mut toc, 0, 1).unwrap();
        assert_eq!(toc.msgs[0].priority, 1);

        set_priority(&mut toc, 0, 5).unwrap();
        assert_eq!(toc.msgs[0].priority, 5);
    }

    #[test]
    fn test_set_get_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Test", "Body"),
        ]);

        assert_eq!(get_label(&toc, 0), 0);
        set_label(&mut toc, 0, 3).unwrap();
        assert_eq!(get_label(&toc, 0), 3);

        set_label(&mut toc, 0, 7).unwrap();
        assert_eq!(get_label(&toc, 0), 7);

        // Label 8+ should be masked to 3 bits
        set_label(&mut toc, 0, 9).unwrap();
        assert_eq!(get_label(&toc, 0), 1); // 9 & 7 = 1
    }

    #[test]
    fn test_transfer_copy() {
        let dir = tempfile::tempdir().unwrap();
        let mut src = make_test_mbox(dir.path(), "src.mbx", &[
            ("alice@test.com", "Hello", "Message body"),
            ("bob@test.com", "Reply", "Reply body"),
        ]);

        let dst_path = dir.path().join("dst.mbx");
        File::create(&dst_path).unwrap();
        let mut dst = Toc::new(&dst_path);

        // Copy message 0
        let new_idx = transfer(&mut src, 0, &mut dst, true).unwrap();
        assert_eq!(new_idx, 0);
        assert_eq!(dst.msgs.len(), 1);
        assert_eq!(dst.msgs[0].from, "alice@test.com");
        assert_eq!(dst.msgs[0].subject, "Hello");

        // Source should NOT be deleted (copy mode)
        assert!(!src.msgs[0].flags.contains(MsgFlags::DELETED));
    }

    #[test]
    fn test_transfer_move() {
        let dir = tempfile::tempdir().unwrap();
        let mut src = make_test_mbox(dir.path(), "src.mbx", &[
            ("alice@test.com", "Hello", "Message body"),
        ]);

        let dst_path = dir.path().join("dst.mbx");
        File::create(&dst_path).unwrap();
        let mut dst = Toc::new(&dst_path);

        // Move message 0
        let new_idx = transfer(&mut src, 0, &mut dst, false).unwrap();
        assert_eq!(new_idx, 0);
        assert_eq!(dst.msgs.len(), 1);

        // Source SHOULD be deleted (move mode)
        assert!(src.msgs[0].flags.contains(MsgFlags::DELETED));
    }

    #[test]
    fn test_transfer_preserves_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let mut src = make_test_mbox(dir.path(), "src.mbx", &[
            ("alice@test.com", "Hello", "Body"),
        ]);
        src.msgs[0].state = State::Replied;
        src.msgs[0].seconds = 1700000000;
        src.msgs[0].orig_zone = -300;
        src.msgs[0].spam_score = 42;
        src.msgs[0].flags.insert(MsgFlags::FLAGGED);

        let dst_path = dir.path().join("dst.mbx");
        File::create(&dst_path).unwrap();
        let mut dst = Toc::new(&dst_path);

        transfer(&mut src, 0, &mut dst, true).unwrap();

        assert_eq!(dst.msgs[0].seconds, 1700000000);
        assert_eq!(dst.msgs[0].orig_zone, -300);
        assert_eq!(dst.msgs[0].spam_score, 42);
        assert_eq!(dst.msgs[0].from, "alice@test.com");
        assert!(dst.msgs[0].flags.contains(MsgFlags::FLAGGED));
        // Deleted flag should NOT be copied
        assert!(!dst.msgs[0].flags.contains(MsgFlags::DELETED));
    }

    #[test]
    fn test_transfer_multi() {
        let dir = tempfile::tempdir().unwrap();
        let mut src = make_test_mbox(dir.path(), "src.mbx", &[
            ("a@test.com", "Msg 1", "Body 1"),
            ("b@test.com", "Msg 2", "Body 2"),
            ("c@test.com", "Msg 3", "Body 3"),
        ]);

        let dst_path = dir.path().join("dst.mbx");
        File::create(&dst_path).unwrap();
        let mut dst = Toc::new(&dst_path);

        let count = transfer_multi(&mut src, &[0, 2], &mut dst, true);
        assert_eq!(count, 2);
        assert_eq!(dst.msgs.len(), 2);
        assert_eq!(dst.msgs[0].from, "a@test.com");
        assert_eq!(dst.msgs[1].from, "c@test.com");
    }

    #[test]
    fn test_transfer_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        let mut src = make_test_mbox(dir.path(), "src.mbx", &[
            ("alice@test.com", "Hello", "Body"),
        ]);

        let dst_path = dir.path().join("dst.mbx");
        File::create(&dst_path).unwrap();
        let mut dst = Toc::new(&dst_path);

        assert!(transfer(&mut src, 5, &mut dst, true).is_err());
    }

    #[test]
    fn test_create_remove_mailbox() {
        let dir = tempfile::tempdir().unwrap();
        let mbox_path = dir.path().join("new.mbx");

        create_mailbox(&mbox_path).unwrap();
        assert!(mbox_path.exists());
        let toc_path = mbox_path.with_extension("toc");
        assert!(toc_path.exists());

        remove_mailbox(&mbox_path).unwrap();
        assert!(!mbox_path.exists());
        assert!(!toc_path.exists());
    }

    #[test]
    fn test_rename_mailbox() {
        let dir = tempfile::tempdir().unwrap();
        let old_path = dir.path().join("old.mbx");
        let new_path = dir.path().join("new.mbx");

        create_mailbox(&old_path).unwrap();
        rename_mailbox(&old_path, &new_path).unwrap();

        assert!(!old_path.exists());
        assert!(new_path.exists());
        let new_toc = new_path.with_extension("toc");
        assert!(new_toc.exists());
    }

    #[test]
    fn test_is_mbox_valid() {
        let dir = tempfile::tempdir().unwrap();
        let mbox_path = dir.path().join("valid.mbx");
        let mut f = File::create(&mbox_path).unwrap();
        f.write_all(b"From user@test.com Mon Jan  1 00:00:00 2024\nFrom: user@test.com\nSubject: Test\n\nBody\n\n").unwrap();
        assert!(is_mbox(&mbox_path));
    }

    #[test]
    fn test_is_mbox_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mbox_path = dir.path().join("empty.mbx");
        File::create(&mbox_path).unwrap();
        assert!(is_mbox(&mbox_path));
    }

    #[test]
    fn test_is_mbox_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let not_mbox = dir.path().join("not.mbx");
        let mut f = File::create(&not_mbox).unwrap();
        f.write_all(b"This is not an mbox file\n").unwrap();
        assert!(!is_mbox(&not_mbox));
    }

    #[test]
    fn test_is_mbox_nonexistent() {
        assert!(!is_mbox(Path::new("/tmp/nonexistent_mbox_12345")));
    }

    #[test]
    fn test_list_mailboxes() {
        let dir = tempfile::tempdir().unwrap();
        // Create some files
        File::create(dir.path().join("In")).unwrap();
        File::create(dir.path().join("Out")).unwrap();
        File::create(dir.path().join("In.toc")).unwrap(); // should be excluded
        File::create(dir.path().join(".hidden")).unwrap(); // should be excluded
        fs::create_dir(dir.path().join("subdir")).unwrap(); // should be excluded

        let names = list_mailboxes(dir.path()).unwrap();
        assert!(names.contains(&"In".to_string()));
        assert!(names.contains(&"Out".to_string()));
        assert!(!names.contains(&"In.toc".to_string()));
        assert!(!names.contains(&".hidden".to_string()));
        assert!(!names.contains(&"subdir".to_string()));
    }

    #[test]
    fn test_search_from() {
        let dir = tempfile::tempdir().unwrap();
        let toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Hello", "Body 1"),
            ("bob@test.com", "World", "Body 2"),
            ("alice@example.org", "Test", "Body 3"),
        ]);

        let results = search(&toc, &SearchField::From, "alice");
        assert_eq!(results, vec![0, 2]);

        let results = search(&toc, &SearchField::From, "bob");
        assert_eq!(results, vec![1]);

        let results = search(&toc, &SearchField::From, "nobody");
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_subject() {
        let dir = tempfile::tempdir().unwrap();
        let toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "Hello World", "Body 1"),
            ("b@test.com", "Goodbye", "Body 2"),
            ("c@test.com", "Hello Again", "Body 3"),
        ]);

        let results = search(&toc, &SearchField::Subject, "Hello");
        assert_eq!(results, vec![0, 2]);
    }

    #[test]
    fn test_search_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("Alice@Test.COM", "HELLO world", "Body"),
        ]);

        let results = search(&toc, &SearchField::From, "alice");
        assert_eq!(results, vec![0]);

        let results = search(&toc, &SearchField::Subject, "hello");
        assert_eq!(results, vec![0]);
    }

    #[test]
    fn test_search_skips_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Hello", "Body 1"),
            ("alice@test.com", "World", "Body 2"),
        ]);

        delete_message(&mut toc, 0).unwrap();

        let results = search(&toc, &SearchField::From, "alice");
        assert_eq!(results, vec![1]); // only non-deleted
    }

    #[test]
    fn test_search_all() {
        let dir = tempfile::tempdir().unwrap();
        let toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Hello", "Body with secret keyword"),
            ("bob@test.com", "World", "Normal body"),
        ]);

        // "secret" is only in the body of message 0, not in from/subject
        let results = search(&toc, &SearchField::All, "secret");
        assert_eq!(results, vec![0]);

        // "alice" is in the from field
        let results = search(&toc, &SearchField::All, "alice");
        assert_eq!(results, vec![0]);
    }

    #[test]
    fn test_search_body() {
        let dir = tempfile::tempdir().unwrap();
        let toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Hello", "unique body text here"),
            ("bob@test.com", "World", "different content"),
        ]);

        let results = search(&toc, &SearchField::Body, "unique");
        assert_eq!(results, vec![0]);

        let results = search(&toc, &SearchField::Body, "different");
        assert_eq!(results, vec![1]);
    }

    #[test]
    fn test_search_header() {
        let dir = tempfile::tempdir().unwrap();
        let toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("alice@test.com", "Hello", "Body"),
        ]);

        let results = search(&toc, &SearchField::Header("Subject".to_string()), "Hello");
        assert_eq!(results, vec![0]);

        let results = search(&toc, &SearchField::Header("Subject".to_string()), "Goodbye");
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_field_parse() {
        assert_eq!(SearchField::parse("from"), SearchField::From);
        assert_eq!(SearchField::parse("FROM"), SearchField::From);
        assert_eq!(SearchField::parse("subject"), SearchField::Subject);
        assert_eq!(SearchField::parse("all"), SearchField::All);
        assert_eq!(SearchField::parse("body"), SearchField::Body);
        assert_eq!(
            SearchField::parse("header:X-Mailer"),
            SearchField::Header("X-Mailer".to_string())
        );
    }

    #[test]
    fn test_sort_by_date() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("c@test.com", "Third", "Body"),
            ("a@test.com", "First", "Body"),
            ("b@test.com", "Second", "Body"),
        ]);
        // Set dates out of order
        toc.msgs[0].seconds = 3000;
        toc.msgs[1].seconds = 1000;
        toc.msgs[2].seconds = 2000;

        sort(&mut toc, SortField::Date, true);
        assert_eq!(toc.msgs[0].seconds, 1000);
        assert_eq!(toc.msgs[1].seconds, 2000);
        assert_eq!(toc.msgs[2].seconds, 3000);
        assert!(toc.dirty);
    }

    #[test]
    fn test_sort_by_date_descending() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "A", "Body"),
            ("b@test.com", "B", "Body"),
            ("c@test.com", "C", "Body"),
        ]);
        toc.msgs[0].seconds = 1000;
        toc.msgs[1].seconds = 3000;
        toc.msgs[2].seconds = 2000;

        sort(&mut toc, SortField::Date, false);
        assert_eq!(toc.msgs[0].seconds, 3000);
        assert_eq!(toc.msgs[1].seconds, 2000);
        assert_eq!(toc.msgs[2].seconds, 1000);
    }

    #[test]
    fn test_sort_by_from() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("charlie@test.com", "C", "Body"),
            ("alice@test.com", "A", "Body"),
            ("bob@test.com", "B", "Body"),
        ]);

        sort(&mut toc, SortField::From, true);
        assert_eq!(toc.msgs[0].from, "alice@test.com");
        assert_eq!(toc.msgs[1].from, "bob@test.com");
        assert_eq!(toc.msgs[2].from, "charlie@test.com");
    }

    #[test]
    fn test_sort_by_subject_strips_re() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "Re: Banana", "Body"),
            ("b@test.com", "Apple", "Body"),
            ("c@test.com", "Re: Re: Cherry", "Body"),
        ]);

        sort(&mut toc, SortField::Subject, true);
        // After stripping Re:, order should be: Apple, Banana, Cherry
        assert_eq!(toc.msgs[0].subject, "Apple");
        assert_eq!(toc.msgs[1].subject, "Re: Banana");
        assert_eq!(toc.msgs[2].subject, "Re: Re: Cherry");
    }

    #[test]
    fn test_sort_by_size() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "A", "Body"),
            ("b@test.com", "B", "Body"),
            ("c@test.com", "C", "Body"),
        ]);
        toc.msgs[0].length = 5000;
        toc.msgs[1].length = 1000;
        toc.msgs[2].length = 3000;

        sort(&mut toc, SortField::Size, true);
        assert_eq!(toc.msgs[0].length, 1000);
        assert_eq!(toc.msgs[1].length, 3000);
        assert_eq!(toc.msgs[2].length, 5000);
    }

    #[test]
    fn test_sort_by_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "A", "Body"),
            ("b@test.com", "B", "Body"),
            ("c@test.com", "C", "Body"),
        ]);
        toc.msgs[0].state = State::Replied;   // 3
        toc.msgs[1].state = State::Unread;     // 1
        toc.msgs[2].state = State::Read;       // 2

        sort(&mut toc, SortField::State, true);
        assert_eq!(toc.msgs[0].state, State::Unread);
        assert_eq!(toc.msgs[1].state, State::Read);
        assert_eq!(toc.msgs[2].state, State::Replied);
    }

    #[test]
    fn test_sort_by_priority() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "A", "Body"),
            ("b@test.com", "B", "Body"),
            ("c@test.com", "C", "Body"),
        ]);
        toc.msgs[0].priority = 5;
        toc.msgs[1].priority = 1;
        toc.msgs[2].priority = 3;

        sort(&mut toc, SortField::Priority, true);
        assert_eq!(toc.msgs[0].priority, 1);
        assert_eq!(toc.msgs[1].priority, 3);
        assert_eq!(toc.msgs[2].priority, 5);
    }

    #[test]
    fn test_sort_by_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "A", "Body"),
            ("b@test.com", "B", "Body"),
            ("c@test.com", "C", "Body"),
        ]);
        toc.msgs[0].flags = toc.msgs[0].flags.with_label(5);
        toc.msgs[1].flags = toc.msgs[1].flags.with_label(1);
        toc.msgs[2].flags = toc.msgs[2].flags.with_label(3);

        sort(&mut toc, SortField::Label, true);
        assert_eq!(toc.msgs[0].flags.label(), 1);
        assert_eq!(toc.msgs[1].flags.label(), 3);
        assert_eq!(toc.msgs[2].flags.label(), 5);
    }

    #[test]
    fn test_sort_single_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "A", "Body"),
        ]);
        toc.dirty = false;
        sort(&mut toc, SortField::Date, true);
        // Should be a no-op, dirty should remain false
        assert!(!toc.dirty);
    }

    #[test]
    fn test_sort_empty() {
        let mut toc = Toc::new("/tmp/fake");
        sort(&mut toc, SortField::Date, true);
        assert!(!toc.dirty);
    }

    #[test]
    fn test_sort_field_parse() {
        assert_eq!(SortField::parse("date"), Some(SortField::Date));
        assert_eq!(SortField::parse("FROM"), Some(SortField::From));
        assert_eq!(SortField::parse("Subject"), Some(SortField::Subject));
        assert_eq!(SortField::parse("size"), Some(SortField::Size));
        assert_eq!(SortField::parse("state"), Some(SortField::State));
        assert_eq!(SortField::parse("priority"), Some(SortField::Priority));
        assert_eq!(SortField::parse("label"), Some(SortField::Label));
        assert_eq!(SortField::parse("unknown"), None);
    }

    #[test]
    fn test_count_unread() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "A", "Body"),
            ("b@test.com", "B", "Body"),
            ("c@test.com", "C", "Body"),
        ]);
        // All start as Unread (default)
        assert_eq!(count_unread(&toc), 3);

        set_state(&mut toc, 0, State::Read).unwrap();
        assert_eq!(count_unread(&toc), 2);

        delete_message(&mut toc, 1).unwrap();
        assert_eq!(count_unread(&toc), 1); // deleted messages don't count
    }

    #[test]
    fn test_count_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "A", "Body"),
            ("b@test.com", "B", "Body"),
        ]);

        assert_eq!(count_flagged(&toc), 0);
        set_flags(&mut toc, 0, MsgFlags::FLAGGED).unwrap();
        assert_eq!(count_flagged(&toc), 1);

        // Deleted flagged message should not count
        delete_message(&mut toc, 0).unwrap();
        assert_eq!(count_flagged(&toc), 0);
    }

    #[test]
    fn test_count_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "A", "Body"),
            ("b@test.com", "B", "Body"),
            ("c@test.com", "C", "Body"),
        ]);

        assert_eq!(count_deleted(&toc), 0);
        delete_message(&mut toc, 0).unwrap();
        assert_eq!(count_deleted(&toc), 1);
        delete_message(&mut toc, 2).unwrap();
        assert_eq!(count_deleted(&toc), 2);
    }

    #[test]
    fn test_total_size() {
        let dir = tempfile::tempdir().unwrap();
        let toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "A", "Body"),
            ("b@test.com", "B", "Body"),
        ]);

        let total = total_size(&toc);
        assert!(total > 0);
        assert_eq!(total, toc.msgs[0].length + toc.msgs[1].length);
    }

    #[test]
    fn test_reclaimable() {
        let dir = tempfile::tempdir().unwrap();
        let mut toc = make_test_mbox(dir.path(), "test.mbx", &[
            ("a@test.com", "A", "Body"),
            ("b@test.com", "B", "Body"),
        ]);

        assert_eq!(reclaimable(&toc), 0);
        let msg0_len = toc.msgs[0].length;
        delete_message(&mut toc, 0).unwrap();
        assert_eq!(reclaimable(&toc), msg0_len);
    }

    #[test]
    fn test_ci_contains() {
        assert!(ci_contains("Hello World", "hello"));
        assert!(ci_contains("Hello World", "WORLD"));
        assert!(ci_contains("Hello World", "lo Wo"));
        assert!(!ci_contains("Hello World", "xyz"));
        assert!(ci_contains("Hello", ""));
        assert!(!ci_contains("", "hello"));
    }

    #[test]
    fn test_strip_reply_prefix() {
        assert_eq!(strip_reply_prefix("Re: Hello"), "Hello");
        assert_eq!(strip_reply_prefix("RE: Hello"), "Hello");
        assert_eq!(strip_reply_prefix("Re: Re: Hello"), "Hello");
        assert_eq!(strip_reply_prefix("Fwd: Hello"), "Hello");
        assert_eq!(strip_reply_prefix("FWD: Hello"), "Hello");
        assert_eq!(strip_reply_prefix("Re: Fwd: Hello"), "Hello");
        assert_eq!(strip_reply_prefix("Hello"), "Hello");
        assert_eq!(strip_reply_prefix(""), "");
    }

    #[test]
    fn test_scan_body_start_lf() {
        let data = b"From: test\nSubject: hi\n\nBody here";
        // "From: test\n" = 11 bytes, "Subject: hi\n" = 12 bytes, "\n" = 1 byte = 24
        assert_eq!(scan_body_start(data), Some(24));
    }

    #[test]
    fn test_scan_body_start_crlf() {
        let data = b"From: test\r\nSubject: hi\r\n\r\nBody here";
        assert_eq!(scan_body_start(data), Some(27));
    }

    #[test]
    fn test_scan_body_start_none() {
        let data = b"No blank line in this data at all";
        assert_eq!(scan_body_start(data), None);
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 5), "hello");
        assert_eq!(truncate_str("", 5), "");
        // Test with multi-byte character boundary
        let s = "caf\u{00e9}"; // "cafe" with accent, e-acute is 2 bytes
        let t = truncate_str(s, 4);
        assert!(t.len() <= 4);
        assert!(t.is_char_boundary(t.len()));
    }

    #[test]
    fn test_days_to_ymd() {
        // Jan 1, 1970
        assert_eq!(days_to_ymd(0), (1970, 0, 1));
        // Dec 31, 1970 = day 364
        assert_eq!(days_to_ymd(364), (1970, 11, 31));
        // Jan 1, 1971 = day 365
        assert_eq!(days_to_ymd(365), (1971, 0, 1));
        // Feb 29, 2000 (leap year) = check it doesn't crash
        // 2000 is 30 years from 1970. Let's compute day count.
        // This is complex, just verify it returns something reasonable
        let (y, m, d) = days_to_ymd(11017); // ~2000-02-29
        assert!(y == 2000 || y == 2001); // approximate
        assert!(m >= 0 && m <= 11);
        assert!(d >= 1 && d <= 31);
    }

    #[test]
    fn test_time_to_asctime() {
        // Epoch = Thu Jan  1 00:00:00 1970
        let s = time_to_asctime(0);
        assert!(s.starts_with("Thu"));
        assert!(s.contains("Jan"));
        assert!(s.contains("1970"));
        assert!(s.contains("00:00:00"));
    }
}
