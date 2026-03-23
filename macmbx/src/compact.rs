//! Compact mailbox — remove deleted messages, rewrite mbox + TOC atomically.
//!
//! Mirrors `macmbx_compact.c`: copies non-deleted messages to a temp file,
//! renames it over the original mbox, then updates the TOC with new offsets.

use std::fs::{self, File};
use std::io::{self, Read, Write, Seek, SeekFrom, BufReader, BufWriter};
use std::path::PathBuf;

use crate::toc::{Toc, MsgFlags};

/// Count messages marked as deleted.
pub fn count_deleted(toc: &Toc) -> usize {
    toc.msgs.iter().filter(|m| m.flags.contains(MsgFlags::DELETED)).count()
}

/// Total bytes reclaimable by compacting (sum of deleted message lengths).
pub fn reclaimable(toc: &Toc) -> i64 {
    toc.msgs.iter()
        .filter(|m| m.flags.contains(MsgFlags::DELETED))
        .map(|m| m.length)
        .sum()
}

/// Compact the mailbox: remove deleted messages by rewriting the mbox file,
/// then update the TOC with corrected offsets.
///
/// Returns the number of messages removed, or 0 if nothing to do.
///
/// The operation is atomic with respect to the mbox file: a temp file is
/// written first, then renamed over the original.  If anything fails before
/// the rename, the original mbox is untouched.
pub fn compact(toc: &mut Toc) -> io::Result<usize> {
    let deleted = count_deleted(toc);
    if deleted == 0 {
        return Ok(0);
    }

    // Open source mbox
    let src = File::open(&toc.mbox_path)?;
    let mut src = BufReader::new(src);

    // Create temp file alongside the mbox
    let tmp_path: PathBuf = {
        let mut p = toc.mbox_path.as_os_str().to_os_string();
        p.push(".compact.tmp");
        PathBuf::from(p)
    };

    let dst = File::create(&tmp_path)?;
    let mut dst = BufWriter::new(dst);

    let mut buf = [0u8; 8192];
    // Build indices of live messages and copy their bytes
    let mut live_indices: Vec<usize> = Vec::with_capacity(toc.msgs.len() - deleted);

    for i in 0..toc.msgs.len() {
        if toc.msgs[i].flags.contains(MsgFlags::DELETED) {
            continue;
        }

        let msg_offset = toc.msgs[i].offset;
        let msg_length = toc.msgs[i].length;

        // Seek to message in source
        src.seek(SeekFrom::Start(msg_offset as u64))?;

        // Copy message bytes
        let mut remaining = msg_length;
        while remaining > 0 {
            let chunk = (remaining as usize).min(buf.len());
            let got = src.read(&mut buf[..chunk])?;
            if got == 0 {
                // Unexpected EOF -- clean up and fail
                drop(dst);
                let _ = fs::remove_file(&tmp_path);
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("short read at message {}: wanted {} more bytes", i, remaining),
                ));
            }
            dst.write_all(&buf[..got])?;
            remaining -= got as i64;
        }

        live_indices.push(i);
    }

    // Flush and close destination
    dst.flush()?;
    drop(dst);
    drop(src);

    // Atomic replace: rename temp over original
    if fs::rename(&tmp_path, &toc.mbox_path).is_err() {
        // rename can fail across filesystems — fall back to copy
        let mut s = BufReader::new(File::open(&tmp_path)?);
        let mut d = BufWriter::new(File::create(&toc.mbox_path)?);
        let mut copy_buf = [0u8; 8192];
        loop {
            let n = s.read(&mut copy_buf)?;
            if n == 0 { break; }
            d.write_all(&copy_buf[..n])?;
        }
        d.flush()?;
        drop(d);
        drop(s);
        let _ = fs::remove_file(&tmp_path);
    }

    // Rebuild the msgs vec with corrected offsets
    let mut running_offset: i64 = 0;
    let mut new_msgs = Vec::with_capacity(live_indices.len());
    for &idx in &live_indices {
        let mut msg = toc.msgs[idx].clone();
        msg.offset = running_offset;
        running_offset += msg.length;
        new_msgs.push(msg);
    }

    toc.msgs = new_msgs;
    toc.dirty = true;

    // Save updated TOC
    toc.save()?;

    Ok(deleted)
}

/// Check whether a mailbox needs compacting (has any deleted messages).
pub fn needs_compact(toc: &Toc) -> bool {
    toc.msgs.iter().any(|m| m.flags.contains(MsgFlags::DELETED))
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toc::{Toc, MsgSum, MsgFlags, State};
    use std::io::Write;

    use std::sync::atomic::{AtomicU64, Ordering};
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Helper: create a temp directory with a test mbox + TOC containing `n`
    /// messages.  Messages are simple "From " + header + body blocks.
    /// Returns (toc, temp_dir_path) -- caller should clean up temp_dir.
    fn make_test_mailbox(n: usize) -> (Toc, PathBuf) {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "macmbx_compact_test_{}_{}", std::process::id(), id
        ));
        let _ = fs::create_dir_all(&dir);

        let mbox_path = dir.join("test.mbx");
        let mut mbox = File::create(&mbox_path).unwrap();

        let mut toc = Toc::new(&mbox_path);
        let mut offset: i64 = 0;

        for i in 0..n {
            let body = format!(
                "From sender{}@test.com Thu Jan  1 00:00:00 2000\r\n\
                 From: sender{}@test.com\r\n\
                 Subject: Message {}\r\n\
                 \r\n\
                 Body of message {}.\r\n",
                i, i, i, i
            );
            let bytes = body.as_bytes();
            mbox.write_all(bytes).unwrap();

            let mut msg = MsgSum::default();
            msg.offset = offset;
            msg.length = bytes.len() as i64;
            msg.body_offset = body.find("\r\n\r\n").unwrap() as i32 + 4;
            msg.state = State::Unread;
            msg.serial_num = toc.next_serial_num();
            msg.from = format!("sender{}@test.com", i);
            msg.subject = format!("Message {}", i);
            toc.msgs.push(msg);

            offset += bytes.len() as i64;
        }
        mbox.flush().unwrap();
        drop(mbox);

        toc.dirty = true;
        toc.save().unwrap();

        (toc, dir)
    }

    /// Clean up test directory.
    fn cleanup(dir: &PathBuf) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn count_deleted_empty() {
        let toc = Toc::new("/tmp/nonexistent.mbx");
        assert_eq!(count_deleted(&toc), 0);
        assert_eq!(reclaimable(&toc), 0);
    }

    #[test]
    fn count_deleted_some() {
        let mut toc = Toc::new("/tmp/nonexistent.mbx");
        for i in 0..5 {
            let mut m = MsgSum::default();
            m.length = 100;
            if i % 2 == 0 {
                m.flags.insert(MsgFlags::DELETED);
            }
            toc.msgs.push(m);
        }
        assert_eq!(count_deleted(&toc), 3);
        assert_eq!(reclaimable(&toc), 300);
    }

    #[test]
    fn needs_compact_flag() {
        let mut toc = Toc::new("/tmp/x.mbx");
        assert!(!needs_compact(&toc));

        let mut m = MsgSum::default();
        m.flags.insert(MsgFlags::DELETED);
        toc.msgs.push(m);
        assert!(needs_compact(&toc));
    }

    #[test]
    fn compact_no_deleted() {
        let (mut toc, dir) = make_test_mailbox(3);
        let result = compact(&mut toc).unwrap();
        assert_eq!(result, 0);
        assert_eq!(toc.msgs.len(), 3);
        cleanup(&dir);
    }

    #[test]
    fn compact_all_deleted() {
        let (mut toc, dir) = make_test_mailbox(3);
        for msg in &mut toc.msgs {
            msg.flags.insert(MsgFlags::DELETED);
        }
        let result = compact(&mut toc).unwrap();
        assert_eq!(result, 3);
        assert_eq!(toc.msgs.len(), 0);

        // Mbox file should be empty
        let mbox_data = fs::read(&toc.mbox_path).unwrap();
        assert_eq!(mbox_data.len(), 0);

        cleanup(&dir);
    }

    #[test]
    fn compact_middle_deleted() {
        let (mut toc, dir) = make_test_mailbox(5);

        // Delete messages 1 and 3
        toc.msgs[1].flags.insert(MsgFlags::DELETED);
        toc.msgs[3].flags.insert(MsgFlags::DELETED);

        // Remember surviving message properties
        let surviving_subjects: Vec<String> = toc.msgs.iter()
            .filter(|m| !m.flags.contains(MsgFlags::DELETED))
            .map(|m| m.subject.clone())
            .collect();
        let surviving_lengths: Vec<i64> = toc.msgs.iter()
            .filter(|m| !m.flags.contains(MsgFlags::DELETED))
            .map(|m| m.length)
            .collect();

        let result = compact(&mut toc).unwrap();
        assert_eq!(result, 2);
        assert_eq!(toc.msgs.len(), 3);

        // Verify subjects preserved
        let new_subjects: Vec<&str> = toc.msgs.iter().map(|m| m.subject.as_str()).collect();
        assert_eq!(new_subjects, vec!["Message 0", "Message 2", "Message 4"]);

        // Verify offsets are contiguous
        let mut expected_offset: i64 = 0;
        for (i, msg) in toc.msgs.iter().enumerate() {
            assert_eq!(msg.offset, expected_offset,
                "message {} offset mismatch", i);
            assert_eq!(msg.length, surviving_lengths[i]);
            expected_offset += msg.length;
        }

        // Verify mbox file size matches sum of surviving lengths
        let mbox_data = fs::read(&toc.mbox_path).unwrap();
        assert_eq!(mbox_data.len() as i64, expected_offset);

        // Verify actual message content is correct by reading each message
        for msg in &toc.msgs {
            let start = msg.offset as usize;
            let end = start + msg.length as usize;
            let content = std::str::from_utf8(&mbox_data[start..end]).unwrap();
            assert!(content.starts_with("From sender"),
                "message at offset {} doesn't start with From line", msg.offset);
            assert!(content.contains(&msg.subject),
                "message content doesn't contain subject '{}'", msg.subject);
        }

        // Verify TOC was saved (no longer dirty after save)
        assert!(!toc.dirty);

        // Re-open TOC from disk and verify
        let reloaded = Toc::open(&toc.mbox_path).unwrap();
        assert_eq!(reloaded.msgs.len(), 3);
        for (i, msg) in reloaded.msgs.iter().enumerate() {
            assert_eq!(msg.subject, surviving_subjects[i]);
            assert_eq!(msg.offset, toc.msgs[i].offset);
            assert_eq!(msg.length, toc.msgs[i].length);
        }

        cleanup(&dir);
    }

    #[test]
    fn compact_first_deleted() {
        let (mut toc, dir) = make_test_mailbox(3);
        toc.msgs[0].flags.insert(MsgFlags::DELETED);

        let result = compact(&mut toc).unwrap();
        assert_eq!(result, 1);
        assert_eq!(toc.msgs.len(), 2);

        // First surviving message should start at offset 0
        assert_eq!(toc.msgs[0].offset, 0);
        assert_eq!(toc.msgs[0].subject, "Message 1");
        assert_eq!(toc.msgs[1].offset, toc.msgs[0].length);
        assert_eq!(toc.msgs[1].subject, "Message 2");

        cleanup(&dir);
    }

    #[test]
    fn compact_last_deleted() {
        let (mut toc, dir) = make_test_mailbox(3);
        toc.msgs[2].flags.insert(MsgFlags::DELETED);

        let result = compact(&mut toc).unwrap();
        assert_eq!(result, 1);
        assert_eq!(toc.msgs.len(), 2);
        assert_eq!(toc.msgs[0].subject, "Message 0");
        assert_eq!(toc.msgs[1].subject, "Message 1");

        // File should be smaller
        let mbox_size = fs::metadata(&toc.mbox_path).unwrap().len() as i64;
        let expected: i64 = toc.msgs.iter().map(|m| m.length).sum();
        assert_eq!(mbox_size, expected);

        cleanup(&dir);
    }

    #[test]
    fn compact_preserves_message_fields() {
        let (mut toc, dir) = make_test_mailbox(3);

        // Set various fields on message 1 (which will survive)
        toc.msgs[1].state = State::Replied;
        toc.msgs[1].priority = 1;
        toc.msgs[1].flags.insert(MsgFlags::FLAGGED);
        toc.msgs[1].serial_num = 999;

        // Delete message 0
        toc.msgs[0].flags.insert(MsgFlags::DELETED);

        compact(&mut toc).unwrap();

        // Message 1 is now at index 0
        let msg = &toc.msgs[0];
        assert_eq!(msg.state, State::Replied);
        assert_eq!(msg.priority, 1);
        assert!(msg.flags.contains(MsgFlags::FLAGGED));
        assert_eq!(msg.serial_num, 999);
        assert_eq!(msg.subject, "Message 1");

        cleanup(&dir);
    }

    #[test]
    fn compact_idempotent() {
        let (mut toc, dir) = make_test_mailbox(3);
        toc.msgs[1].flags.insert(MsgFlags::DELETED);

        let r1 = compact(&mut toc).unwrap();
        assert_eq!(r1, 1);
        assert_eq!(toc.msgs.len(), 2);

        // Compact again — should be a no-op
        let r2 = compact(&mut toc).unwrap();
        assert_eq!(r2, 0);
        assert_eq!(toc.msgs.len(), 2);

        cleanup(&dir);
    }

    #[test]
    fn compact_single_message_deleted() {
        let (mut toc, dir) = make_test_mailbox(1);
        toc.msgs[0].flags.insert(MsgFlags::DELETED);

        let result = compact(&mut toc).unwrap();
        assert_eq!(result, 1);
        assert_eq!(toc.msgs.len(), 0);

        let mbox_data = fs::read(&toc.mbox_path).unwrap();
        assert_eq!(mbox_data.len(), 0);

        cleanup(&dir);
    }

    #[test]
    fn reclaimable_matches_deleted_sizes() {
        let (mut toc, dir) = make_test_mailbox(4);
        toc.msgs[0].flags.insert(MsgFlags::DELETED);
        toc.msgs[2].flags.insert(MsgFlags::DELETED);

        let expected = toc.msgs[0].length + toc.msgs[2].length;
        assert_eq!(reclaimable(&toc), expected);
        assert_eq!(count_deleted(&toc), 2);

        cleanup(&dir);
    }
}
