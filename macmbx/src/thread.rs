//! Message threading, deduplication, import/export.
//!
//! Ported from `macmbx_thread.c`.
//!
//! - **Threading**: Simplified JWZ algorithm using `msg_id_hash` and `in_reply_to_hash`
//!   to build a tree of conversations.
//! - **Dedup**: Detect/skip duplicate messages by `Message-ID` hash.
//! - **Export**: EML files, Unix mbox, Mozilla mbox (with `X-Mozilla-Status` headers).
//! - **Import**: EML files, EML directories, Maildir, mbox files.
//! - **Signature import**: Copy signature text files between directories.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::toc::{MsgFlags, MsgSum, State, Toc};

// ══════════════════════════════════════════════════════════════════
// Hash helper (DJB2, same as macmbx_build.c)
// ══════════════════════════════════════════════════════════════════

fn hash_str(s: &[u8]) -> i32 {
    let mut h: u32 = 5381;
    for &b in s {
        h = h.wrapping_shl(5).wrapping_add(h).wrapping_add(b as u32);
    }
    if h == 0 { 1 } else { h as i32 }
}

/// Extract `Message-ID` hash from raw RFC-822 headers.
pub fn extract_msgid_hash(msg: &[u8]) -> i32 {
    let mut pos = 0;
    while pos < msg.len() {
        // Check for Message-ID header (case-insensitive)
        if msg.len() - pos >= 11 {
            let hdr = &msg[pos..pos + 11];
            if hdr.eq_ignore_ascii_case(b"Message-ID:") {
                let mut p = pos + 11;
                // skip whitespace
                while p < msg.len() && (msg[p] == b' ' || msg[p] == b'\t') {
                    p += 1;
                }
                let mut id_start = p;
                if id_start < msg.len() && msg[id_start] == b'<' {
                    id_start += 1;
                }
                let mut id_end = id_start;
                while id_end < msg.len()
                    && msg[id_end] != b'>'
                    && msg[id_end] != b'\r'
                    && msg[id_end] != b'\n'
                {
                    id_end += 1;
                }
                if id_end > id_start {
                    return hash_str(&msg[id_start..id_end]);
                }
                break;
            }
        }
        // Skip to next line
        while pos < msg.len() && msg[pos] != b'\n' {
            pos += 1;
        }
        if pos < msg.len() {
            pos += 1;
        }
        // End of headers (blank line)
        if pos < msg.len() && (msg[pos] == b'\r' || msg[pos] == b'\n') {
            break;
        }
    }
    0
}

// ══════════════════════════════════════════════════════════════════
// Thread tree
// ══════════════════════════════════════════════════════════════════

/// A node in the message thread tree.
#[derive(Debug)]
pub struct ThreadNode {
    /// Message index in the TOC (-1 for dummy/phantom root).
    pub index: i32,
    /// Nesting depth (0 = root).
    pub depth: i32,
    /// Children (first child, then siblings via the parent's children vec).
    pub children: Vec<ThreadNode>,
}

impl ThreadNode {
    fn new(index: i32) -> Self {
        Self {
            index,
            depth: 0,
            children: Vec::new(),
        }
    }
}

/// Build a thread tree from a TOC.
///
/// Algorithm (simplified JWZ threading):
/// 1. Build hash map: `msg_id_hash` -> node index
/// 2. For each message with `in_reply_to_hash`, link child to parent
/// 3. Collect root nodes (no parent)
///
/// Returns a list of root [`ThreadNode`]s (top-level conversations).
pub fn build_threads(toc: &Toc) -> Vec<ThreadNode> {
    if toc.msgs.is_empty() {
        return Vec::new();
    }

    // Map: msg_id_hash -> index in nodes[]
    let mut hash_to_idx: HashMap<i32, usize> = HashMap::new();
    // All nodes, indexed by their position in this vec
    let mut nodes: Vec<Option<usize>> = vec![None; toc.msgs.len()]; // TOC index -> nodes_data index
    let mut nodes_data: Vec<(i32, i32, Vec<usize>)> = Vec::new(); // (toc_index, parent_hash, children_indices)

    // Pass 1: create nodes, index by msg_id_hash
    for (i, msg) in toc.msgs.iter().enumerate() {
        if msg.flags.contains(MsgFlags::DELETED) {
            continue;
        }
        let data_idx = nodes_data.len();
        nodes_data.push((i as i32, msg.in_reply_to_hash, Vec::new()));
        nodes[i] = Some(data_idx);
        if msg.msg_id_hash != 0 {
            hash_to_idx.insert(msg.msg_id_hash, data_idx);
        }
    }

    // Pass 2: link children to parents via in_reply_to_hash
    // We need to collect links first because we can't mutably borrow nodes_data twice
    let links: Vec<(usize, usize)> = nodes_data
        .iter()
        .enumerate()
        .filter_map(|(idx, (_, parent_hash, _))| {
            if *parent_hash != 0 {
                if let Some(&parent_idx) = hash_to_idx.get(parent_hash) {
                    if parent_idx != idx {
                        return Some((parent_idx, idx));
                    }
                }
            }
            None
        })
        .collect();

    let mut has_parent = vec![false; nodes_data.len()];
    for &(parent_idx, child_idx) in &links {
        nodes_data[parent_idx].2.push(child_idx);
        has_parent[child_idx] = true;
    }

    // Pass 3: collect roots and build tree recursively
    fn build_subtree(
        data: &[(i32, i32, Vec<usize>)],
        idx: usize,
        depth: i32,
    ) -> ThreadNode {
        let mut node = ThreadNode::new(data[idx].0);
        node.depth = depth;
        for &child_idx in &data[idx].2 {
            node.children.push(build_subtree(data, child_idx, depth + 1));
        }
        node
    }

    let mut roots = Vec::new();
    for (idx, is_child) in has_parent.iter().enumerate() {
        if !is_child {
            roots.push(build_subtree(&nodes_data, idx, 0));
        }
    }
    roots
}

/// Flatten a thread tree depth-first into a list of `(index, depth)` pairs.
pub fn thread_flatten(threads: &[ThreadNode]) -> Vec<(i32, i32)> {
    let mut result = Vec::new();
    fn walk(node: &ThreadNode, result: &mut Vec<(i32, i32)>) {
        result.push((node.index, node.depth));
        for child in &node.children {
            walk(child, result);
        }
    }
    for root in threads {
        walk(root, &mut result);
    }
    result
}

/// Find the root thread containing a message index.
/// Returns a reference to the root [`ThreadNode`], or `None`.
pub fn thread_find(threads: &[ThreadNode], index: i32) -> Option<&ThreadNode> {
    fn find_in(node: &ThreadNode, index: i32) -> bool {
        if node.index == index {
            return true;
        }
        node.children.iter().any(|c| find_in(c, index))
    }
    threads.iter().find(|root| find_in(root, index))
}

/// Count top-level threads (conversations).
pub fn thread_count(threads: &[ThreadNode]) -> usize {
    threads.len()
}

// ══════════════════════════════════════════════════════════════════
// Deduplication
// ══════════════════════════════════════════════════════════════════

/// Find a message in a TOC by its `msg_id_hash`.
/// Returns the index if found, `None` if not found.
pub fn find_duplicate(toc: &Toc, msg_id_hash: i32) -> Option<usize> {
    if msg_id_hash == 0 {
        return None;
    }
    toc.msgs.iter().enumerate().find_map(|(i, msg)| {
        if !msg.flags.contains(MsgFlags::DELETED) && msg.msg_id_hash == msg_id_hash {
            Some(i)
        } else {
            None
        }
    })
}

/// Check if a raw message is a duplicate of an existing message in the TOC.
/// Returns the index of the existing message, or `None`.
pub fn is_duplicate(toc: &Toc, message: &[u8]) -> Option<usize> {
    let hash = extract_msgid_hash(message);
    if hash == 0 {
        return None;
    }
    find_duplicate(toc, hash)
}

/// Append a message only if not a duplicate.
///
/// Returns `Ok(index)` with the new message index, or `Err(existing_index)` if duplicate.
/// Returns `Ok(index)` from the underlying append on success.
///
/// `append_fn` is called to actually append the message and should return the new index.
pub fn append_unique<F>(
    toc: &Toc,
    message: &[u8],
    append_fn: F,
) -> Result<usize, DedupResult>
where
    F: FnOnce() -> Option<usize>,
{
    if let Some(existing) = is_duplicate(toc, message) {
        return Err(DedupResult::Duplicate(existing));
    }
    match append_fn() {
        Some(idx) => Ok(idx),
        None => Err(DedupResult::AppendFailed),
    }
}

/// Result when deduplication rejects a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupResult {
    /// Message is a duplicate of the message at this index.
    Duplicate(usize),
    /// The append function itself failed.
    AppendFailed,
}

// ══════════════════════════════════════════════════════════════════
// Export — EML
// ══════════════════════════════════════════════════════════════════

/// Export a single message as a `.eml` file.
///
/// Reads the message from the mbox, strips the `From ` envelope line if present,
/// writes the rest to `eml_path`.
pub fn export_eml(toc: &Toc, index: usize, eml_path: &Path) -> io::Result<()> {
    let raw = read_message_raw(toc, index)?;
    let body = strip_from_line(&raw);
    fs::write(eml_path, body)
}

/// Export multiple messages to a directory as individual `.eml` files.
/// Filenames use the serial number: `00001.eml`, etc.
pub fn export_eml_multi(toc: &Toc, indices: &[usize], dir_path: &Path) -> io::Result<usize> {
    fs::create_dir_all(dir_path)?;
    let mut exported = 0;
    for &i in indices {
        if i >= toc.msgs.len() {
            continue;
        }
        let path = dir_path.join(format!("{:05}.eml", toc.msgs[i].serial_num));
        if export_eml(toc, i, &path).is_ok() {
            exported += 1;
        }
    }
    Ok(exported)
}

/// Export all non-deleted messages as `.eml` files.
pub fn export_all_eml(toc: &Toc, dir_path: &Path) -> io::Result<usize> {
    fs::create_dir_all(dir_path)?;
    let mut exported = 0;
    for (i, msg) in toc.msgs.iter().enumerate() {
        if msg.flags.contains(MsgFlags::DELETED) {
            continue;
        }
        let path = dir_path.join(format!("{:05}.eml", msg.serial_num));
        if export_eml(toc, i, &path).is_ok() {
            exported += 1;
        }
    }
    Ok(exported)
}

// ══════════════════════════════════════════════════════════════════
// Export — Mbox
// ══════════════════════════════════════════════════════════════════

/// Export a mailbox as a single Unix mbox file.
pub fn export_mbox(toc: &Toc, output_path: &Path) -> io::Result<usize> {
    let mut out = fs::File::create(output_path)?;
    let mut exported = 0;
    for (i, msg) in toc.msgs.iter().enumerate() {
        let raw = match read_message_raw(toc, i) {
            Ok(r) => r,
            Err(_) => continue,
        };
        // Ensure message starts with a From line
        if !raw.starts_with(b"From ") {
            let from_line = write_from_line(if msg.from.is_empty() {
                "unknown"
            } else {
                &msg.from
            });
            out.write_all(from_line.as_bytes())?;
        }
        out.write_all(&raw)?;
        // Ensure ends with newline
        if !raw.is_empty() && raw[raw.len() - 1] != b'\n' {
            out.write_all(b"\n")?;
        }
        exported += 1;
    }
    Ok(exported)
}

/// Mozilla flag constants.
const MOZ_FLAG_READ: u32 = 0x0001;
const MOZ_FLAG_REPLIED: u32 = 0x0002;
const MOZ_FLAG_MARKED: u32 = 0x0004;
const MOZ_FLAG_FORWARDED: u32 = 0x1000;

fn state_to_mozilla(state: State, _priority: u8) -> u32 {
    let mut flags: u32 = 0;
    match state {
        State::Read => flags |= MOZ_FLAG_READ,
        State::Replied => flags |= MOZ_FLAG_READ | MOZ_FLAG_REPLIED,
        State::Forwarded => flags |= MOZ_FLAG_READ | MOZ_FLAG_FORWARDED,
        _ => {}
    }
    flags
}

/// Export a mailbox as a Mozilla-compatible mbox file.
///
/// Inserts `X-Mozilla-Status` and `X-Mozilla-Status2` headers after
/// the `From ` envelope line, mapping message state and priority
/// to Mozilla flag format.
pub fn export_mbox_mozilla(toc: &Toc, output_path: &Path) -> io::Result<usize> {
    let mut out = fs::File::create(output_path)?;
    let mut exported = 0;
    for (i, msg) in toc.msgs.iter().enumerate() {
        let raw = match read_message_raw(toc, i) {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Ensure From line
        if raw.len() < 5 || &raw[..5] != b"From " {
            let from_line = write_from_line(if msg.from.is_empty() {
                "unknown"
            } else {
                &msg.from
            });
            out.write_all(from_line.as_bytes())?;
        }

        // Find end of first line and insert Mozilla headers after it
        if let Some(nl_pos) = raw.iter().position(|&b| b == b'\n') {
            out.write_all(&raw[..nl_pos + 1])?;
            let moz_flags = state_to_mozilla(msg.state, msg.priority);
            write!(out, "X-Mozilla-Status: {:04x}\r\n", moz_flags & 0xFFFF)?;
            write!(out, "X-Mozilla-Status2: {:08x}\r\n", 0u32)?;
            out.write_all(&raw[nl_pos + 1..])?;
        } else {
            out.write_all(&raw)?;
        }

        if !raw.is_empty() && raw[raw.len() - 1] != b'\n' {
            out.write_all(b"\n")?;
        }
        exported += 1;
    }
    Ok(exported)
}

// ══════════════════════════════════════════════════════════════════
// Import — EML
// ══════════════════════════════════════════════════════════════════

/// Extract the `From:` header value from raw message bytes.
fn extract_from_header(msg: &[u8]) -> String {
    let mut pos = 0;
    while pos < msg.len() {
        if msg.len() - pos >= 5 && msg[pos..pos + 5].eq_ignore_ascii_case(b"From:") {
            let mut p = pos + 5;
            while p < msg.len() && (msg[p] == b' ' || msg[p] == b'\t') {
                p += 1;
            }
            let start = p;
            while p < msg.len() && msg[p] != b'\r' && msg[p] != b'\n' {
                p += 1;
            }
            return String::from_utf8_lossy(&msg[start..p]).into_owned();
        }
        while pos < msg.len() && msg[pos] != b'\n' {
            pos += 1;
        }
        if pos < msg.len() {
            pos += 1;
        }
        if pos < msg.len() && (msg[pos] == b'\r' || msg[pos] == b'\n') {
            break;
        }
    }
    "unknown".to_string()
}

/// Import a single `.eml` file into a TOC's mbox.
///
/// Returns the new message index on success.
pub fn import_eml(toc: &mut Toc, eml_path: &Path) -> io::Result<usize> {
    let msg = fs::read(eml_path)?;
    let sender = extract_from_header(&msg);
    append_to_mbox(toc, &msg, &sender, State::Unread, 3)
}

/// Import all `.eml` files from a directory.
/// Returns the count imported.
pub fn import_eml_dir(toc: &mut Toc, dir_path: &Path) -> io::Result<usize> {
    let mut imported = 0;
    for entry in fs::read_dir(dir_path)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.len() < 5 || !name_str.to_ascii_lowercase().ends_with(".eml") {
            continue;
        }
        if import_eml(toc, &entry.path()).is_ok() {
            imported += 1;
        }
    }
    Ok(imported)
}

/// Import from Maildir format (`cur/`, `new/` subdirectories).
/// Returns the count imported.
pub fn import_maildir(toc: &mut Toc, maildir_path: &Path) -> io::Result<usize> {
    let mut imported = 0;
    let subdirs = ["new", "cur"];
    for (s_idx, subdir) in subdirs.iter().enumerate() {
        let subpath = maildir_path.join(subdir);
        let entries = match fs::read_dir(&subpath) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy().into_owned();
            if name_str.starts_with('.') {
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !meta.is_file() {
                continue;
            }
            let msg = match fs::read(entry.path()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mut state = if s_idx == 0 {
                State::Unread
            } else {
                State::Read
            };
            // Check maildir flags in filename: `:2,S` means Seen
            if let Some(flags_pos) = name_str.find(":2,") {
                if name_str[flags_pos..].contains('S') {
                    state = State::Read;
                }
            }
            if append_to_mbox(toc, &msg, "", state, 3).is_ok() {
                imported += 1;
            }
        }
    }
    Ok(imported)
}

/// Import from another mbox file. If `dedup` is true, skip duplicates by `Message-ID`.
/// Returns the count imported.
pub fn import_mbox(toc: &mut Toc, mbox_path: &Path, dedup: bool) -> io::Result<usize> {
    let data = fs::read(mbox_path)?;
    let messages = split_mbox(&data);
    let mut imported = 0;
    for raw_msg in &messages {
        let body = strip_from_line(raw_msg);
        if dedup {
            if is_duplicate(toc, body).is_some() {
                continue;
            }
        }
        // Extract sender from the raw message's From header
        let sender = extract_from_header(body);
        if append_to_mbox(toc, body, &sender, State::Unread, 3).is_ok() {
            imported += 1;
        }
    }
    Ok(imported)
}

// ══════════════════════════════════════════════════════════════════
// Signature import
// ══════════════════════════════════════════════════════════════════

/// Import signature files from one directory to another.
/// Each regular file is copied. Returns the count imported.
pub fn sig_import_dir(sig_dir: &Path, import_from: &Path) -> io::Result<usize> {
    fs::create_dir_all(sig_dir)?;
    let mut imported = 0;
    for entry in fs::read_dir(import_from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let meta = entry.metadata()?;
        if !meta.is_file() {
            continue;
        }
        let dst = sig_dir.join(&name);
        if fs::copy(entry.path(), &dst).is_ok() {
            imported += 1;
        }
    }
    Ok(imported)
}

// ══════════════════════════════════════════════════════════════════
// Internal helpers — mbox read/write
// ══════════════════════════════════════════════════════════════════

/// Read a message from the mbox file by index.
fn read_message_raw(toc: &Toc, index: usize) -> io::Result<Vec<u8>> {
    if index >= toc.msgs.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "index out of range"));
    }
    let msg = &toc.msgs[index];
    let data = fs::read(&toc.mbox_path)?;
    let offset = msg.offset as usize;
    let length = msg.length as usize;
    if offset + length > data.len() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "message beyond mbox"));
    }
    Ok(data[offset..offset + length].to_vec())
}

/// Strip the leading `From ` envelope line from a message.
fn strip_from_line(msg: &[u8]) -> &[u8] {
    if msg.len() > 5 && &msg[..5] == b"From " {
        if let Some(nl) = msg.iter().position(|&b| b == b'\n') {
            return &msg[nl + 1..];
        }
    }
    msg
}

/// Write a `From ` envelope line.
fn write_from_line(sender: &str) -> String {
    format!("From {} Thu Jan  1 00:00:00 1970\n", sender)
}

/// Append a message to a TOC's mbox file and update the TOC.
///
/// This creates the `From ` line, appends to the mbox file,
/// creates a new `MsgSum`, and pushes it into `toc.msgs`.
fn append_to_mbox(
    toc: &mut Toc,
    message: &[u8],
    sender: &str,
    state: State,
    priority: u8,
) -> io::Result<usize> {
    use std::io::Seek;

    let extracted;
    let from_addr = if sender.is_empty() {
        extracted = extract_from_header(message);
        &extracted
    } else {
        sender
    };
    let from_line = write_from_line(from_addr);

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&toc.mbox_path)?;

    let offset = file.seek(io::SeekFrom::End(0))? as i64;
    file.write_all(from_line.as_bytes())?;
    file.write_all(message)?;
    if !message.is_empty() && message[message.len() - 1] != b'\n' {
        file.write_all(b"\n")?;
    }
    let end = file.seek(io::SeekFrom::End(0))? as i64;
    let length = end - offset;

    let serial = toc.next_serial;
    toc.next_serial += 1;

    // Find body offset (blank line separating headers from body)
    let body_offset = find_body_offset(message) as i32 + from_line.len() as i32;

    // Extract subject
    let subject = extract_header_field(message, b"Subject:");

    let mut sum = MsgSum::default();
    sum.offset = offset;
    sum.length = length;
    sum.body_offset = body_offset;
    sum.state = state;
    sum.priority = priority;
    sum.orig_priority = priority;
    sum.serial_num = serial;
    sum.msg_id_hash = extract_msgid_hash(message);
    sum.in_reply_to_hash = extract_in_reply_to_hash(message);
    sum.from = if sender.is_empty() {
        extract_from_header(message)
    } else {
        sender.to_string()
    };
    sum.subject = subject;

    let idx = toc.msgs.len();
    toc.msgs.push(sum);
    toc.dirty = true;
    Ok(idx)
}

/// Find the offset of the body (after the blank line separating headers).
fn find_body_offset(msg: &[u8]) -> usize {
    let mut i = 0;
    while i < msg.len() {
        if i + 1 < msg.len() && msg[i] == b'\n' && msg[i + 1] == b'\n' {
            return i + 2;
        }
        if i + 3 < msg.len()
            && msg[i] == b'\r'
            && msg[i + 1] == b'\n'
            && msg[i + 2] == b'\r'
            && msg[i + 3] == b'\n'
        {
            return i + 4;
        }
        i += 1;
    }
    msg.len()
}

/// Extract a header field value (first occurrence).
fn extract_header_field(msg: &[u8], field: &[u8]) -> String {
    let mut pos = 0;
    while pos < msg.len() {
        if msg.len() - pos >= field.len()
            && msg[pos..pos + field.len()].eq_ignore_ascii_case(field)
        {
            let mut p = pos + field.len();
            while p < msg.len() && (msg[p] == b' ' || msg[p] == b'\t') {
                p += 1;
            }
            let start = p;
            while p < msg.len() && msg[p] != b'\r' && msg[p] != b'\n' {
                p += 1;
            }
            return String::from_utf8_lossy(&msg[start..p]).into_owned();
        }
        while pos < msg.len() && msg[pos] != b'\n' {
            pos += 1;
        }
        if pos < msg.len() {
            pos += 1;
        }
        if pos < msg.len() && (msg[pos] == b'\r' || msg[pos] == b'\n') {
            break;
        }
    }
    String::new()
}

/// Extract In-Reply-To message-id hash.
fn extract_in_reply_to_hash(msg: &[u8]) -> i32 {
    let mut pos = 0;
    while pos < msg.len() {
        if msg.len() - pos >= 12
            && msg[pos..pos + 12].eq_ignore_ascii_case(b"In-Reply-To:")
        {
            let mut p = pos + 12;
            while p < msg.len() && (msg[p] == b' ' || msg[p] == b'\t') {
                p += 1;
            }
            let mut id_start = p;
            if id_start < msg.len() && msg[id_start] == b'<' {
                id_start += 1;
            }
            let mut id_end = id_start;
            while id_end < msg.len()
                && msg[id_end] != b'>'
                && msg[id_end] != b'\r'
                && msg[id_end] != b'\n'
            {
                id_end += 1;
            }
            if id_end > id_start {
                return hash_str(&msg[id_start..id_end]);
            }
            break;
        }
        while pos < msg.len() && msg[pos] != b'\n' {
            pos += 1;
        }
        if pos < msg.len() {
            pos += 1;
        }
        if pos < msg.len() && (msg[pos] == b'\r' || msg[pos] == b'\n') {
            break;
        }
    }
    0
}

/// Split an mbox file into individual messages (each starting with `From `).
fn split_mbox(data: &[u8]) -> Vec<&[u8]> {
    let mut messages = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < data.len() {
        // A "From " at the start of a line marks a new message
        let at_line_start = i == 0 || (i > 0 && data[i - 1] == b'\n');
        if at_line_start && data.len() - i >= 5 && &data[i..i + 5] == b"From " {
            if i > start {
                messages.push(&data[start..i]);
            }
            start = i;
        }
        i += 1;
    }
    if start < data.len() {
        messages.push(&data[start..]);
    }
    messages
}

// ══════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn make_test_toc(dir: &Path) -> Toc {
        let mbox_path = dir.join("test.mbx");
        // Create empty mbox
        fs::write(&mbox_path, b"").unwrap();
        Toc::new(&mbox_path)
    }

    fn make_message(msg_id: &str, in_reply_to: Option<&str>, subject: &str) -> Vec<u8> {
        let mut msg = format!(
            "Message-ID: <{}>\r\nSubject: {}\r\nFrom: test@example.com\r\n",
            msg_id, subject
        );
        if let Some(irt) = in_reply_to {
            msg.push_str(&format!("In-Reply-To: <{}>\r\n", irt));
        }
        msg.push_str("\r\nBody text here.\r\n");
        msg.into_bytes()
    }

    #[test]
    fn test_hash_str_basic() {
        let h1 = hash_str(b"abc@example.com");
        let h2 = hash_str(b"abc@example.com");
        assert_eq!(h1, h2);
        assert_ne!(h1, 0);

        let h3 = hash_str(b"def@example.com");
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_extract_msgid_hash() {
        let msg = b"From: test@example.com\r\nMessage-ID: <abc123@example.com>\r\n\r\nbody";
        let h = extract_msgid_hash(msg);
        assert_ne!(h, 0);

        // Same ID should give same hash
        let msg2 = b"Subject: test\r\nMessage-Id: <abc123@example.com>\r\n\r\nbody";
        let h2 = extract_msgid_hash(msg2);
        assert_eq!(h, h2);

        // No Message-ID
        let msg3 = b"From: test@example.com\r\nSubject: test\r\n\r\nbody";
        assert_eq!(extract_msgid_hash(msg3), 0);
    }

    #[test]
    fn test_extract_in_reply_to_hash() {
        let msg = b"In-Reply-To: <parent@example.com>\r\n\r\nbody";
        let h = extract_in_reply_to_hash(msg);
        assert_ne!(h, 0);
        // Should match the msg_id hash for the same ID
        let expected = hash_str(b"parent@example.com");
        assert_eq!(h, expected);
    }

    #[test]
    fn test_threading_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg1 = make_message("root@ex.com", None, "Root message");
        let msg2 = make_message("reply@ex.com", Some("root@ex.com"), "Reply");
        let msg3 = make_message("standalone@ex.com", None, "Standalone");

        append_to_mbox(&mut toc, &msg1, "a@ex.com", State::Read, 3).unwrap();
        append_to_mbox(&mut toc, &msg2, "b@ex.com", State::Unread, 3).unwrap();
        append_to_mbox(&mut toc, &msg3, "c@ex.com", State::Read, 3).unwrap();

        let threads = build_threads(&toc);
        // Should have 2 root threads: one for root+reply, one for standalone
        assert_eq!(thread_count(&threads), 2);

        // Find the root thread
        let root_thread = thread_find(&threads, 0).unwrap();
        assert_eq!(root_thread.index, 0);
        assert_eq!(root_thread.children.len(), 1);
        assert_eq!(root_thread.children[0].index, 1);

        // Standalone should be its own root
        let standalone = thread_find(&threads, 2).unwrap();
        assert_eq!(standalone.index, 2);
        assert!(standalone.children.is_empty());
    }

    #[test]
    fn test_threading_deep() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg1 = make_message("a@ex.com", None, "Root");
        let msg2 = make_message("b@ex.com", Some("a@ex.com"), "Reply 1");
        let msg3 = make_message("c@ex.com", Some("b@ex.com"), "Reply 2");

        append_to_mbox(&mut toc, &msg1, "x@ex.com", State::Read, 3).unwrap();
        append_to_mbox(&mut toc, &msg2, "y@ex.com", State::Read, 3).unwrap();
        append_to_mbox(&mut toc, &msg3, "z@ex.com", State::Read, 3).unwrap();

        let threads = build_threads(&toc);
        assert_eq!(thread_count(&threads), 1);

        let flat = thread_flatten(&threads);
        assert_eq!(flat.len(), 3);
        assert_eq!(flat[0], (0, 0)); // root, depth 0
        assert_eq!(flat[1], (1, 1)); // reply 1, depth 1
        assert_eq!(flat[2], (2, 2)); // reply 2, depth 2
    }

    #[test]
    fn test_thread_flatten() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg1 = make_message("r@ex.com", None, "Root");
        let msg2 = make_message("c1@ex.com", Some("r@ex.com"), "Child 1");
        let msg3 = make_message("c2@ex.com", Some("r@ex.com"), "Child 2");

        append_to_mbox(&mut toc, &msg1, "a@ex.com", State::Read, 3).unwrap();
        append_to_mbox(&mut toc, &msg2, "b@ex.com", State::Read, 3).unwrap();
        append_to_mbox(&mut toc, &msg3, "c@ex.com", State::Read, 3).unwrap();

        let threads = build_threads(&toc);
        let flat = thread_flatten(&threads);
        assert_eq!(flat.len(), 3);
        // Root at depth 0
        assert_eq!(flat[0].1, 0);
        // Both children at depth 1
        assert_eq!(flat[1].1, 1);
        assert_eq!(flat[2].1, 1);
    }

    #[test]
    fn test_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg1 = make_message("unique@ex.com", None, "First");
        append_to_mbox(&mut toc, &msg1, "a@ex.com", State::Read, 3).unwrap();

        // Same message-id should be detected as duplicate
        let msg2 = make_message("unique@ex.com", None, "Same ID");
        assert_eq!(is_duplicate(&toc, &msg2), Some(0));

        // Different message-id should not be duplicate
        let msg3 = make_message("different@ex.com", None, "Different");
        assert_eq!(is_duplicate(&toc, &msg3), None);
    }

    #[test]
    fn test_find_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg = make_message("test@ex.com", None, "Test");
        append_to_mbox(&mut toc, &msg, "a@ex.com", State::Read, 3).unwrap();

        let hash = toc.msgs[0].msg_id_hash;
        assert_eq!(find_duplicate(&toc, hash), Some(0));
        assert_eq!(find_duplicate(&toc, 0), None);
        assert_eq!(find_duplicate(&toc, 99999), None);
    }

    #[test]
    fn test_append_unique() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg1 = make_message("u1@ex.com", None, "First");
        append_to_mbox(&mut toc, &msg1, "a@ex.com", State::Read, 3).unwrap();

        // Try to append same message-id
        let msg2 = make_message("u1@ex.com", None, "Dupe");
        let result = append_unique(&toc, &msg2, || None);
        assert_eq!(result, Err(DedupResult::Duplicate(0)));

        // Different message-id should succeed
        let msg3 = make_message("u2@ex.com", None, "New");
        let result = append_unique(&toc, &msg3, || Some(1));
        assert_eq!(result, Ok(1));
    }

    #[test]
    fn test_export_import_eml() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg = make_message("exp@ex.com", None, "Export test");
        append_to_mbox(&mut toc, &msg, "sender@ex.com", State::Read, 3).unwrap();

        let eml_path = tmp.path().join("test.eml");
        export_eml(&toc, 0, &eml_path).unwrap();
        assert!(eml_path.exists());

        // Verify exported content doesn't have "From " line
        let exported = fs::read(&eml_path).unwrap();
        assert!(!exported.starts_with(b"From "));
        assert!(exported.windows(10).any(|w| w == b"Message-ID"));

        // Import it into a new TOC
        fs::create_dir_all(tmp.path().join("import")).unwrap();
        let mut toc2 = make_test_toc(&tmp.path().join("import"));
        let idx = import_eml(&mut toc2, &eml_path).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(toc2.msgs.len(), 1);
    }

    #[test]
    fn test_export_mbox() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg1 = make_message("m1@ex.com", None, "Message 1");
        let msg2 = make_message("m2@ex.com", None, "Message 2");
        append_to_mbox(&mut toc, &msg1, "a@ex.com", State::Read, 3).unwrap();
        append_to_mbox(&mut toc, &msg2, "b@ex.com", State::Unread, 3).unwrap();

        let out = tmp.path().join("export.mbox");
        let count = export_mbox(&toc, &out).unwrap();
        assert_eq!(count, 2);

        let data = fs::read(&out).unwrap();
        let text = String::from_utf8_lossy(&data);
        // Both messages should start with From
        let from_count = text.matches("\nFrom ").count() + if text.starts_with("From ") { 1 } else { 0 };
        assert!(from_count >= 2);
    }

    #[test]
    fn test_export_mbox_mozilla() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg = make_message("moz@ex.com", None, "Mozilla test");
        append_to_mbox(&mut toc, &msg, "test@ex.com", State::Replied, 2).unwrap();

        let out = tmp.path().join("mozilla.mbox");
        let count = export_mbox_mozilla(&toc, &out).unwrap();
        assert_eq!(count, 1);

        let data = String::from_utf8(fs::read(&out).unwrap()).unwrap();
        assert!(data.contains("X-Mozilla-Status: 0003")); // READ | REPLIED
        assert!(data.contains("X-Mozilla-Status2: 00000000"));
    }

    #[test]
    fn test_import_mbox() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a source mbox with two messages
        let src = tmp.path().join("source.mbox");
        {
            let mut f = fs::File::create(&src).unwrap();
            write!(f, "From sender1@ex.com Thu Jan  1 00:00:00 1970\n").unwrap();
            write!(f, "Message-ID: <im1@ex.com>\r\nFrom: s1@ex.com\r\nSubject: M1\r\n\r\nBody1\n").unwrap();
            write!(f, "From sender2@ex.com Thu Jan  1 00:00:00 1970\n").unwrap();
            write!(f, "Message-ID: <im2@ex.com>\r\nFrom: s2@ex.com\r\nSubject: M2\r\n\r\nBody2\n").unwrap();
        }

        let mut toc = make_test_toc(tmp.path());
        let count = import_mbox(&mut toc, &src, false).unwrap();
        assert_eq!(count, 2);
        assert_eq!(toc.msgs.len(), 2);

        // Dedup: importing same file again should add 0
        let count2 = import_mbox(&mut toc, &src, true).unwrap();
        assert_eq!(count2, 0);
    }

    #[test]
    fn test_import_maildir() {
        let tmp = tempfile::tempdir().unwrap();
        let maildir = tmp.path().join("maildir");
        fs::create_dir_all(maildir.join("new")).unwrap();
        fs::create_dir_all(maildir.join("cur")).unwrap();

        // Write a new message
        fs::write(
            maildir.join("new/1234567890.1234.host"),
            b"Message-ID: <md1@ex.com>\r\nFrom: new@ex.com\r\nSubject: New\r\n\r\nNew body\n",
        ).unwrap();

        // Write a cur message with Seen flag
        fs::write(
            maildir.join("cur/1234567891.1235.host:2,S"),
            b"Message-ID: <md2@ex.com>\r\nFrom: cur@ex.com\r\nSubject: Cur\r\n\r\nCur body\n",
        ).unwrap();

        let mut toc = make_test_toc(tmp.path());
        let count = import_maildir(&mut toc, &maildir).unwrap();
        assert_eq!(count, 2);
        // New message should be Unread
        assert_eq!(toc.msgs[0].state, State::Unread);
        // Cur message with S flag should be Read
        assert_eq!(toc.msgs[1].state, State::Read);
    }

    #[test]
    fn test_export_import_eml_multi() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg1 = make_message("em1@ex.com", None, "Multi 1");
        let msg2 = make_message("em2@ex.com", None, "Multi 2");
        let msg3 = make_message("em3@ex.com", None, "Multi 3");
        append_to_mbox(&mut toc, &msg1, "a@ex.com", State::Read, 3).unwrap();
        append_to_mbox(&mut toc, &msg2, "b@ex.com", State::Read, 3).unwrap();
        append_to_mbox(&mut toc, &msg3, "c@ex.com", State::Read, 3).unwrap();

        let export_dir = tmp.path().join("eml_export");
        let count = export_eml_multi(&toc, &[0, 2], &export_dir).unwrap();
        assert_eq!(count, 2);

        // Import them back
        fs::create_dir_all(tmp.path().join("import2")).unwrap();
        let mut toc2 = make_test_toc(&tmp.path().join("import2"));
        let count2 = import_eml_dir(&mut toc2, &export_dir).unwrap();
        assert_eq!(count2, 2);
    }

    #[test]
    fn test_export_all_eml() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg1 = make_message("all1@ex.com", None, "All 1");
        let msg2 = make_message("all2@ex.com", None, "All 2");
        append_to_mbox(&mut toc, &msg1, "a@ex.com", State::Read, 3).unwrap();
        append_to_mbox(&mut toc, &msg2, "b@ex.com", State::Read, 3).unwrap();

        // Mark first as deleted
        toc.msgs[0].flags.insert(MsgFlags::DELETED);

        let dir = tmp.path().join("all_eml");
        let count = export_all_eml(&toc, &dir).unwrap();
        // Only non-deleted should export
        assert_eq!(count, 1);
    }

    #[test]
    fn test_sig_import_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("sigs_src");
        let dst = tmp.path().join("sigs_dst");
        fs::create_dir_all(&src).unwrap();

        fs::write(src.join("personal.txt"), b"Best regards,\nJohn").unwrap();
        fs::write(src.join("work.txt"), b"Regards,\nJohn Doe, Corp.").unwrap();

        let count = sig_import_dir(&dst, &src).unwrap();
        assert_eq!(count, 2);
        assert!(dst.join("personal.txt").exists());
        assert!(dst.join("work.txt").exists());
    }

    #[test]
    fn test_split_mbox() {
        let data = b"From sender1 Thu Jan  1 00:00:00 1970\nHello\n\nFrom sender2 Thu Jan  1 00:00:00 1970\nWorld\n";
        let msgs = split_mbox(data);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0].starts_with(b"From sender1"));
        assert!(msgs[1].starts_with(b"From sender2"));
    }

    #[test]
    fn test_strip_from_line() {
        let msg = b"From sender@ex.com Thu Jan  1 00:00:00 1970\nSubject: test\n\nbody";
        let stripped = strip_from_line(msg);
        assert!(stripped.starts_with(b"Subject:"));

        // No From line
        let msg2 = b"Subject: test\n\nbody";
        let stripped2 = strip_from_line(msg2);
        assert_eq!(stripped2, msg2);
    }

    #[test]
    fn test_extract_from_header() {
        let msg = b"Subject: Test\r\nFrom: John Doe <john@example.com>\r\n\r\nbody";
        assert_eq!(extract_from_header(msg), "John Doe <john@example.com>");

        let msg2 = b"Subject: Test\r\n\r\nbody";
        assert_eq!(extract_from_header(msg2), "unknown");
    }

    #[test]
    fn test_state_to_mozilla() {
        assert_eq!(state_to_mozilla(State::Read, 3), MOZ_FLAG_READ);
        assert_eq!(
            state_to_mozilla(State::Replied, 0),
            MOZ_FLAG_READ | MOZ_FLAG_REPLIED
        );
        assert_eq!(
            state_to_mozilla(State::Forwarded, 2),
            MOZ_FLAG_READ | MOZ_FLAG_FORWARDED
        );
        assert_eq!(state_to_mozilla(State::Unread, 0), 0);
    }

    #[test]
    fn test_empty_toc_threads() {
        let tmp = tempfile::tempdir().unwrap();
        let toc = make_test_toc(tmp.path());
        let threads = build_threads(&toc);
        assert!(threads.is_empty());
        assert_eq!(thread_count(&threads), 0);
        assert_eq!(thread_flatten(&threads).len(), 0);
    }

    #[test]
    fn test_thread_find_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());
        let msg = make_message("tf@ex.com", None, "Test");
        append_to_mbox(&mut toc, &msg, "a@ex.com", State::Read, 3).unwrap();

        let threads = build_threads(&toc);
        assert!(thread_find(&threads, 0).is_some());
        assert!(thread_find(&threads, 99).is_none());
    }

    #[test]
    fn test_deleted_messages_skipped_in_threads() {
        let tmp = tempfile::tempdir().unwrap();
        let mut toc = make_test_toc(tmp.path());

        let msg1 = make_message("d1@ex.com", None, "First");
        let msg2 = make_message("d2@ex.com", None, "Deleted");
        append_to_mbox(&mut toc, &msg1, "a@ex.com", State::Read, 3).unwrap();
        append_to_mbox(&mut toc, &msg2, "b@ex.com", State::Read, 3).unwrap();
        toc.msgs[1].flags.insert(MsgFlags::DELETED);

        let threads = build_threads(&toc);
        assert_eq!(thread_count(&threads), 1);
    }
}
