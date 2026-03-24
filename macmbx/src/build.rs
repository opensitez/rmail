//! Build TOC from mbox file scan (macmbx_build.c port).
//!
//! Scans mbox file for "From " separator lines, parses headers
//! (Date, From, Subject, Priority, Message-ID, Status, etc.),
//! builds per-message summaries.
//!
//! Ported from macmbx_build.c.

use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::fs::File;
use std::path::Path;

use crate::toc::{Toc, MsgSum, State, MsgFlags, TOC_MAJOR, TOC_MINOR};

// ── "From " line validation (ported from Eudora buildtoc.c) ──

static MONTH_NAMES: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun",
    "jul", "aug", "sep", "oct", "nov", "dec",
];

static DAY_NAMES: [&str; 7] = [
    "mon", "tue", "wed", "thu", "fri", "sat", "sun",
];

/// Check whether a line is a valid mbox "From " separator.
///
/// Validates the date portion after the sender address,
/// requiring day-of-month, month name, year, and time components.
pub fn is_from_line(line: &str) -> bool {
    if !line.starts_with("From ") {
        return false;
    }

    let lp = &line[5..]; // skip "From "

    // Skip return address (may contain quoted strings)
    let mut quote = false;
    let mut addr_end = 0;
    for (i, ch) in lp.char_indices() {
        if ch == '"' {
            quote = !quote;
        } else if ch == ' ' && !quote {
            addr_end = i;
            break;
        }
        addr_end = i + ch.len_utf8(); // keep advancing past end
    }

    // Must have space after address
    if addr_end >= lp.len() {
        return false;
    }
    let rest = &lp[addr_end..];
    let rest = rest.trim_start_matches(' ');
    if rest.is_empty() {
        return false;
    }

    // Validate date portion: need day, month, year, time
    let mut week_day = 0u32;
    let mut day = 0u32;
    let mut month = 0u32;
    let mut year = 0u32;
    let mut tym = 0u32;
    let mut other = 0u32;

    for token in rest.split(|c: char| c == ' ' || c == '\t' || c == '\r' || c == '\n' || c == ',') {
        if token.is_empty() {
            continue;
        }
        let len = token.len();
        let num: i32 = token.parse().unwrap_or(0);

        // Time: "HH:MM" or "HH:MM:SS"
        if num < 24 && len >= 5 && token.as_bytes()[2] == b':'
            && (len == 5 || (len == 8 && token.as_bytes()[5] == b':'))
        {
            tym += 1;
            if tym > 1 {
                return false;
            }
        }
        // 2-digit year after day has been seen
        else if year == 0 && day > 0 && len == 2
            && token.as_bytes()[len - 1].is_ascii_digit()
        {
            year += 1;
            if year > 1 {
                return false;
            }
        }
        // Day of month: 1 or 2 digits, 1-31
        else if len <= 2 && num > 0 && num < 32 {
            day += 1;
            if day > 1 {
                return false;
            }
        }
        // 4-digit year > 1900
        else if len == 4 && num > 1900 {
            year += 1;
            if year > 1 {
                return false;
            }
        }
        // 3-letter token: day name or month name
        else if len == 3 {
            let lower = token.to_ascii_lowercase();
            let is_day_name = DAY_NAMES.iter().any(|&d| d == lower);
            let is_month_name = MONTH_NAMES.iter().any(|&m| m == lower);
            if is_day_name {
                week_day += 1;
                if week_day > 1 {
                    return false;
                }
            } else if is_month_name {
                month += 1;
                if month > 1 {
                    return false;
                }
            } else {
                other += 1;
            }
        } else {
            other += 1;
        }
    }

    day > 0 && year > 0 && month > 0 && tym > 0 && other <= 2
}

// ── Date parsing helpers ──

fn month_num(s: &str) -> Option<u32> {
    if s.len() < 3 {
        return None;
    }
    let lower: String = s[..3].to_ascii_lowercase();
    MONTH_NAMES.iter().position(|&m| m == lower).map(|i| i as u32)
}

fn parse_tz_offset(s: &str) -> i64 {
    let s = s.trim_start();
    if s.is_empty() {
        return 0;
    }

    let first = s.as_bytes()[0];
    if first == b'+' || first == b'-' {
        let sign: i64 = if first == b'-' { -1 } else { 1 };
        let rest = &s[1..];
        if let Ok(val) = rest.trim().parse::<i64>() {
            if val.abs() > 2400 {
                return 0;
            }
            return sign * ((val / 100) * 3600 + (val % 100) * 60);
        }
        return 0;
    }

    // Common symbolic timezones
    let upper = s.to_ascii_uppercase();
    if upper.starts_with("GMT") || upper.starts_with("UTC") {
        return 0;
    }
    if upper.starts_with("CEST") {
        return 7200;
    }
    match upper.get(..3).map(|s| s.as_ref()) {
        Some("EST") => -5 * 3600,
        Some("EDT") => -4 * 3600,
        Some("CST") => -6 * 3600,
        Some("CDT") => -5 * 3600,
        Some("MST") => -7 * 3600,
        Some("MDT") => -6 * 3600,
        Some("PST") => -8 * 3600,
        Some("PDT") => -7 * 3600,
        Some("CET") => 3600,
        _ => 0,
    }
}

/// Parsed date result with UTC seconds and timezone offset in seconds.
struct ParsedDate {
    seconds: u32,
    tz_secs: i64,
}

/// Parse an RFC822/mbox date string into UTC seconds since epoch.
///
/// Handles formats like:
/// - "Wed, 14 Jun 2023 12:36:18 -0400"  (RFC822)
/// - "Wed Jun 14 12:36:18 2023"          (mbox envelope)
/// - "14 Jun 2023 12:36:18 +0000"
fn parse_date(date: &str) -> Option<ParsedDate> {
    if date.is_empty() {
        return None;
    }

    let mut s = date;

    // Skip "day, " prefix or bare day name
    if s.len() >= 3
        && s.as_bytes()[0].is_ascii_alphabetic()
        && s.as_bytes()[1].is_ascii_alphabetic()
        && s.as_bytes()[2].is_ascii_alphabetic()
    {
        if let Some(comma_pos) = s.find(',') {
            if comma_pos <= 10 {
                s = &s[comma_pos + 1..];
            }
        } else {
            // Mbox format: "Wed Jun 14 12:36:18 2023"
            s = &s[3..];
        }
    }

    let s = s.trim_start();

    // Tokenize remaining: split on whitespace and date separators (- /),
    // but preserve +HHMM / -HHMM timezone offsets.
    let mut tokens: Vec<&str> = Vec::new();
    {
        let bytes = s.as_bytes();
        let mut start = 0;
        let len = bytes.len();
        while start < len {
            // skip delimiters
            if bytes[start] == b' ' || bytes[start] == b'\t' || bytes[start] == b'/' {
                start += 1;
                continue;
            }
            // '-' is a delimiter UNLESS it's a timezone offset (followed by digits)
            if bytes[start] == b'-' {
                if start + 1 < len && bytes[start + 1].is_ascii_digit() {
                    // looks like -HHMM timezone, keep as one token
                    let end = start + 1 + bytes[start + 1..].iter().take_while(|b| b.is_ascii_digit()).count();
                    tokens.push(&s[start..end]);
                    start = end;
                    continue;
                }
                start += 1;
                continue;
            }
            // '+' followed by digits: timezone offset
            if bytes[start] == b'+' && start + 1 < len && bytes[start + 1].is_ascii_digit() {
                let end = start + 1 + bytes[start + 1..].iter().take_while(|b| b.is_ascii_digit()).count();
                tokens.push(&s[start..end]);
                start = end;
                continue;
            }
            // regular token: consume until next delimiter
            let end = bytes[start..].iter().position(|&b| b == b' ' || b == b'\t' || b == b'-' || b == b'/' || b == b'+')
                .map(|p| start + p)
                .unwrap_or(len);
            if end > start {
                tokens.push(&s[start..end]);
            }
            start = end;
        }
    }

    let mut day: i32 = 0;
    let mut mon: i32 = -1;
    let mut year: i32 = 0;
    let mut hour: i32 = 0;
    let mut min: i32 = 0;
    let mut sec: i32 = 0;
    let mut tz_str: Option<&str> = None;
    let mut time_seen = false;

    let mut idx = 0;
    while idx < tokens.len() {
        let tok = tokens[idx];
        idx += 1;

        // Time token: "HH:MM" or "HH:MM:SS"
        if !time_seen && tok.contains(':') && tok.as_bytes()[0].is_ascii_digit() {
            let parts: Vec<&str> = tok.split(':').collect();
            if parts.len() >= 2 {
                hour = parts[0].parse().unwrap_or(0);
                min = parts[1].parse().unwrap_or(0);
                if parts.len() >= 3 {
                    sec = parts[2].parse().unwrap_or(0);
                }
                time_seen = true;
                continue;
            }
        }

        // Month name
        if mon < 0 {
            if let Some(m) = month_num(tok) {
                mon = m as i32;
                continue;
            }
        }

        // Numeric token (but not +/- timezone offsets)
        if !tok.starts_with('+') && !tok.starts_with('-') && tok.parse::<i32>().is_ok() {
            let num: i32 = tok.parse().unwrap();
            if num > 1900 && tok.len() == 4 {
                year = num;
            } else if day == 0 && num >= 1 && num <= 31 {
                day = num;
            } else if year == 0 && num >= 0 && num <= 99 && tok.len() <= 2 {
                year = if num < 70 { 2000 + num } else { 1900 + num };
            } else if year == 0 {
                year = num;
            }
            continue;
        }

        // Timezone token: +HHMM, -HHMM, or symbolic
        if tok.starts_with('+') || tok.starts_with('-') || tok.as_bytes()[0].is_ascii_alphabetic() {
            if tz_str.is_none() && time_seen {
                tz_str = Some(tok);
            }
        }
    }

    if mon < 0 || day < 1 || year < 1970 {
        return None;
    }

    let tz = tz_str.map(parse_tz_offset).unwrap_or(0);

    // Convert to UTC seconds using a simple calculation (no libc dependency)
    let secs = datetime_to_epoch(year, mon as u32 + 1, day as u32, hour as u32, min as u32, sec as u32);
    if secs == 0 {
        return None;
    }

    // Subtract tz offset to get UTC
    let utc_secs = (secs as i64) - tz;
    if utc_secs < 0 {
        return None;
    }

    Some(ParsedDate {
        seconds: utc_secs as u32,
        tz_secs: tz,
    })
}

/// Convert date/time components to seconds since Unix epoch (UTC assumed).
fn datetime_to_epoch(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> u64 {
    if year < 1970 || month < 1 || month > 12 || day < 1 || day > 31 {
        return 0;
    }

    // Days in each month (non-leap)
    let days_in_month: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    let is_leap = |y: i32| -> bool {
        (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
    };

    // Days from epoch to start of year
    let mut days: i64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }

    // Days from start of year to start of month
    for m in 0..(month - 1) as usize {
        days += days_in_month[m] as i64;
        if m == 1 && is_leap(year) {
            days += 1;
        }
    }

    days += (day - 1) as i64;

    (days * 86400 + hour as i64 * 3600 + min as i64 * 60 + sec as i64) as u64
}

// ── Simple string hash (djb2, matching C implementation) ──

fn hash_string(s: &[u8]) -> u32 {
    let mut h: u32 = 5381;
    for &b in s {
        h = h.wrapping_shl(5).wrapping_add(h).wrapping_add(b as u32);
    }
    if h == 0 { 1 } else { h }
}

// ── Header value extraction ──

/// Extract the value portion after "Header-Name:" — trims whitespace and trailing CRLF.
fn extract_header_value(line: &str) -> &str {
    if let Some(colon_pos) = line.find(':') {
        let val = &line[colon_pos + 1..];
        val.trim_start().trim_end_matches(|c| c == '\r' || c == '\n')
    } else {
        ""
    }
}

/// Beautify a From address: "Name <addr>" -> "Name", "<addr>" -> "addr".
fn beautify_from(from: &str) -> String {
    let s = from.trim();
    if s.is_empty() {
        return String::new();
    }

    if let Some(lt_pos) = s.find('<') {
        if let Some(gt_pos) = s[lt_pos..].find('>') {
            let gt_pos = lt_pos + gt_pos;
            // Check for display name before <
            let name = s[..lt_pos].trim();
            if !name.is_empty() {
                // Strip surrounding quotes
                let name = if name.starts_with('"') && name.ends_with('"') && name.len() > 1 {
                    &name[1..name.len() - 1]
                } else {
                    name
                };
                if !name.is_empty() {
                    return name.to_string();
                }
            }
            // No name — use address between < >
            let addr = &s[lt_pos + 1..gt_pos];
            if !addr.is_empty() {
                return addr.to_string();
            }
        }
    }

    s.to_string()
}

// ── Build TOC from mbox ──

/// Build a TOC by scanning an mbox file.
///
/// Opens the mbox file, scans for "From " separator lines, parses
/// headers (Date, From, Subject, Priority, Message-ID, Status,
/// Content-Type, Precedence, etc.), and builds per-message summaries.
pub fn build_toc(mbox_path: &Path) -> io::Result<Toc> {
    let f = File::open(mbox_path)?;
    let file_len = f.metadata()?.len() as i64;
    let mut reader = BufReader::new(f);

    let mut toc = Toc::new(mbox_path);
    toc.major_version = TOC_MAJOR;
    toc.minor_version = TOC_MINOR;
    toc.dirty = true;

    let mut line = String::new();
    let mut offset: i64 = 0;
    let mut cur = MsgSum::default();
    let mut in_header = false;
    let mut have_msg = false;
    let mut sender_priority = 99u8;

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            break; // EOF
        }
        let line_start = offset;
        offset += bytes_read as i64;

        if is_from_line(&line) {
            // Save previous message if any
            if have_msg {
                cur.length = line_start - cur.offset;
                if cur.from.is_empty() {
                    cur.from = "unknown".to_string();
                }
                cur.from = beautify_from(&cur.from);
                cur.serial_num = toc.next_serial;
                toc.next_serial += 1;
                toc.msgs.push(cur);
            }

            // Start new message
            cur = MsgSum::default();
            cur.offset = line_start;
            cur.state = State::Unread;
            cur.priority = 3;
            cur.spam_score = -1;
            cur.flags = MsgFlags::UTF8;
            sender_priority = 99;

            // Extract sender and date from "From " line
            let after_from = &line[5..]; // skip "From "
            let addr_end = after_from.find(' ').unwrap_or(after_from.len());
            let addr = &after_from[..addr_end];
            // Truncate to reasonable size (matches C 64-char field)
            cur.from = if addr.len() > 63 {
                addr[..63].to_string()
            } else {
                addr.to_string()
            };

            let date_part = after_from[addr_end..].trim();
            let date_part = date_part.trim_end_matches(|c| c == '\r' || c == '\n');
            if let Some(parsed) = parse_date(date_part) {
                cur.seconds = parsed.seconds;
                cur.orig_zone = (parsed.tz_secs / 60) as i16;
                cur.arrival = cur.seconds;
            }

            in_header = true;
            have_msg = true;
            continue;
        }

        if in_header {
            // Blank line = end of headers
            if line.starts_with('\r') || line.starts_with('\n') || line == "\r\n" {
                in_header = false;
                cur.body_offset = (offset - cur.offset) as i32;
                continue;
            }

            if line.len() > 5 && line[..5].eq_ignore_ascii_case("Date:") {
                let val = extract_header_value(&line);
                if let Some(parsed) = parse_date(val) {
                    cur.seconds = parsed.seconds;
                    cur.orig_zone = (parsed.tz_secs / 60) as i16;
                }
            } else if line.len() > 8 && line[..8].eq_ignore_ascii_case("Subject:") {
                let val = extract_header_value(&line);
                cur.subject = val.to_string();
            } else if line.len() > 5 && line[..5].eq_ignore_ascii_case("From:") && sender_priority > 1 {
                let val = extract_header_value(&line);
                cur.from = val.to_string();
                sender_priority = 1;
            } else if line.len() > 7 && line[..7].eq_ignore_ascii_case("Sender:") && sender_priority > 2 {
                let val = extract_header_value(&line);
                cur.from = val.to_string();
                sender_priority = 2;
            } else if line.len() > 9 && line[..9].eq_ignore_ascii_case("Reply-To:") && sender_priority > 3 {
                let val = extract_header_value(&line);
                cur.from = val.to_string();
                sender_priority = 3;
            } else if line.len() > 11 && line[..11].eq_ignore_ascii_case("X-Priority:") {
                let val = extract_header_value(&line);
                if let Ok(pri) = val.trim().parse::<u8>() {
                    if (1..=5).contains(&pri) {
                        cur.priority = pri;
                    }
                }
            } else if line.len() > 11 && line[..11].eq_ignore_ascii_case("Importance:") {
                let val = extract_header_value(&line).to_ascii_lowercase();
                if val.contains("high") {
                    cur.priority = 1;
                } else if val.contains("low") {
                    cur.priority = 5;
                }
            } else if line.len() > 11
                && (line[..11].eq_ignore_ascii_case("Message-ID:") || line[..11].eq_ignore_ascii_case("Message-Id:"))
            {
                let val = extract_header_value(&line);
                let id = val.trim_start_matches('<').trim_end_matches('>');
                if !id.is_empty() {
                    let h = hash_string(id.as_bytes());
                    cur.msg_id_hash = h as i32;
                    if cur.uid_hash == 0 {
                        cur.uid_hash = h;
                    }
                }
            } else if line.len() > 12 && line[..12].eq_ignore_ascii_case("In-Reply-To:") {
                let val = extract_header_value(&line);
                let id = val.trim_start_matches('<').trim_end_matches('>');
                if !id.is_empty() {
                    cur.in_reply_to_hash = hash_string(id.as_bytes()) as i32;
                }
            } else if line.len() > 7 && line[..7].eq_ignore_ascii_case("Status:") {
                let val = extract_header_value(&line);
                if val.contains('R') {
                    cur.state = State::Read;
                }
            } else if line.len() > 13 && line[..13].eq_ignore_ascii_case("Content-Type:") {
                let lower = line.to_ascii_lowercase();
                if lower.contains("text/html") {
                    cur.flags.insert(MsgFlags::HTML);
                }
            } else if line.len() > 11 && line[..11].eq_ignore_ascii_case("Precedence:") {
                let val = extract_header_value(&line).to_ascii_lowercase();
                if val.contains("bulk") || val.contains("list") || val.contains("junk") {
                    cur.flags.insert(MsgFlags::BULK);
                }
            }
        }
    }

    // Save last message
    if have_msg {
        cur.length = file_len - cur.offset;
        if cur.from.is_empty() {
            cur.from = "unknown".to_string();
        }
        cur.from = beautify_from(&cur.from);
        cur.serial_num = toc.next_serial;
        toc.next_serial += 1;
        toc.msgs.push(cur);
    }

    Ok(toc)
}

// ── Rebuild TOC with salvage from old ──

/// Rebuild a TOC from an mbox file, salvaging state from an old TOC.
///
/// After building a fresh TOC, matches messages from the old TOC
/// by offset+length or msg_id_hash, and copies over state, priority,
/// and flags from the old TOC entries.
pub fn rebuild_toc(mbox_path: &Path, old: Option<&Toc>) -> io::Result<Toc> {
    let mut toc = build_toc(mbox_path)?;

    let old = match old {
        Some(o) if !o.msgs.is_empty() => o,
        _ => return Ok(toc),
    };

    for n in &mut toc.msgs {
        let mut found = false;

        // Binary search in old by offset
        let result = old.msgs.binary_search_by(|m| m.offset.cmp(&n.offset));
        if let Ok(mid) = result {
            if old.msgs[mid].length == n.length {
                // Match — salvage state, priority, flags from old
                let bo = n.body_offset;
                let secs = n.seconds;
                let serial = n.serial_num;
                let offset = n.offset;
                let length = n.length;
                let from = std::mem::take(&mut n.from);
                let subject = std::mem::take(&mut n.subject);

                *n = old.msgs[mid].clone();
                n.offset = offset;
                n.length = length;
                n.body_offset = bo;
                n.serial_num = serial;
                n.from = from;
                n.subject = subject;
                if n.state != State::Timed {
                    n.seconds = secs;
                }
                found = true;
            }
        }

        // If not found by offset, try msg_id_hash
        if !found && n.msg_id_hash != 0 {
            for o in &old.msgs {
                if o.msg_id_hash == n.msg_id_hash && o.length == n.length {
                    n.state = o.state;
                    n.priority = o.priority;
                    n.flags = o.flags;
                    break;
                }
            }
        }
    }

    toc.dirty = true;
    Ok(toc)
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;

    // ── is_from_line ──

    #[test]
    fn from_line_valid_standard() {
        assert!(is_from_line("From user@example.com Wed Jun 14 12:36:18 2023\n"));
    }

    #[test]
    fn from_line_valid_with_timezone() {
        assert!(is_from_line("From sender@test.org Mon Jan  2 08:15:00 2023 -0500\n"));
    }

    #[test]
    fn from_line_valid_minimal() {
        assert!(is_from_line("From x@y.z Thu Mar 1 00:00:00 2001\n"));
    }

    #[test]
    fn from_line_invalid_no_prefix() {
        assert!(!is_from_line("Subject: test\n"));
    }

    #[test]
    fn from_line_invalid_no_date() {
        assert!(!is_from_line("From user@example.com\n"));
    }

    #[test]
    fn from_line_invalid_empty() {
        assert!(!is_from_line(""));
    }

    #[test]
    fn from_line_invalid_just_from() {
        assert!(!is_from_line("From "));
    }

    #[test]
    fn from_line_with_quoted_address() {
        assert!(is_from_line("From \"John Doe\" Wed Jun 14 12:36:18 2023\n"));
    }

    // ── hash_string ──

    #[test]
    fn hash_deterministic() {
        let a = hash_string(b"hello@world.com");
        let b = hash_string(b"hello@world.com");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_different_inputs() {
        let a = hash_string(b"abc");
        let b = hash_string(b"def");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_never_zero() {
        // djb2 of empty string is 5381 so not zero,
        // but test the guard anyway
        assert_ne!(hash_string(b""), 0);
    }

    #[test]
    fn hash_matches_c_djb2() {
        // djb2("abc") = known value
        // h = 5381
        // h = 5381*33 + 97 = 177670
        // h = 177670*33 + 98 = 5863208
        // h = 5863208*33 + 99 = 193485963
        assert_eq!(hash_string(b"abc"), 193485963);
    }

    // ── extract_header_value ──

    #[test]
    fn extract_value_simple() {
        assert_eq!(extract_header_value("Subject: Hello World\r\n"), "Hello World");
    }

    #[test]
    fn extract_value_with_spaces() {
        assert_eq!(extract_header_value("From:   user@test.com\n"), "user@test.com");
    }

    #[test]
    fn extract_value_no_colon() {
        assert_eq!(extract_header_value("NoColon"), "");
    }

    // ── beautify_from ──

    #[test]
    fn beautify_name_and_addr() {
        assert_eq!(beautify_from("John Doe <john@example.com>"), "John Doe");
    }

    #[test]
    fn beautify_quoted_name() {
        assert_eq!(beautify_from("\"Jane Smith\" <jane@example.com>"), "Jane Smith");
    }

    #[test]
    fn beautify_addr_only() {
        assert_eq!(beautify_from("<user@test.com>"), "user@test.com");
    }

    #[test]
    fn beautify_bare_addr() {
        assert_eq!(beautify_from("user@test.com"), "user@test.com");
    }

    #[test]
    fn beautify_empty() {
        assert_eq!(beautify_from(""), "");
    }

    // ── parse_tz_offset ──

    #[test]
    fn tz_positive() {
        assert_eq!(parse_tz_offset("+0530"), 5 * 3600 + 30 * 60);
    }

    #[test]
    fn tz_negative() {
        assert_eq!(parse_tz_offset("-0500"), -5 * 3600);
    }

    #[test]
    fn tz_symbolic() {
        assert_eq!(parse_tz_offset("PST"), -8 * 3600);
        assert_eq!(parse_tz_offset("UTC"), 0);
        assert_eq!(parse_tz_offset("CET"), 3600);
        assert_eq!(parse_tz_offset("CEST"), 7200);
    }

    #[test]
    fn tz_out_of_range() {
        assert_eq!(parse_tz_offset("+9999"), 0);
    }

    // ── parse_date ──

    #[test]
    fn parse_rfc822_date() {
        let d = parse_date("Wed, 14 Jun 2023 12:36:18 +0000").unwrap();
        assert!(d.seconds > 0);
        assert_eq!(d.tz_secs, 0);
    }

    #[test]
    fn parse_mbox_date() {
        let d = parse_date("Wed Jun 14 12:36:18 2023").unwrap();
        assert!(d.seconds > 0);
    }

    #[test]
    fn parse_date_with_tz() {
        let d = parse_date("14 Jun 2023 12:36:18 -0500").unwrap();
        assert_eq!(d.tz_secs, -5 * 3600);
    }

    #[test]
    fn parse_date_invalid() {
        assert!(parse_date("").is_none());
        assert!(parse_date("not a date").is_none());
    }

    #[test]
    fn parse_date_two_digit_year() {
        // 2-digit year < 70 => 2000s
        let d = parse_date("14 Jun 23 12:00:00 +0000").unwrap();
        assert!(d.seconds > 0);
    }

    // ── datetime_to_epoch ──

    #[test]
    fn epoch_known_date() {
        // 2023-06-14 12:36:18 UTC
        let secs = datetime_to_epoch(2023, 6, 14, 12, 36, 18);
        // Should be around 1686746178
        assert_eq!(secs, 1686746178);
    }

    #[test]
    fn epoch_unix_epoch() {
        assert_eq!(datetime_to_epoch(1970, 1, 1, 0, 0, 0), 0);
    }

    #[test]
    fn epoch_leap_year() {
        // 2024-02-29 00:00:00 should be valid
        let secs = datetime_to_epoch(2024, 2, 29, 0, 0, 0);
        assert!(secs > 0);
    }

    #[test]
    fn epoch_invalid() {
        assert_eq!(datetime_to_epoch(1969, 1, 1, 0, 0, 0), 0);
        assert_eq!(datetime_to_epoch(2023, 0, 1, 0, 0, 0), 0);
        assert_eq!(datetime_to_epoch(2023, 13, 1, 0, 0, 0), 0);
    }

    // ── build_toc integration tests ──

    fn write_mbox(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn build_empty_mbox() {
        let f = write_mbox("");
        let toc = build_toc(f.path()).unwrap();
        assert_eq!(toc.msgs.len(), 0);
    }

    #[test]
    fn build_single_message() {
        let mbox = "\
From user@example.com Wed Jun 14 12:36:18 2023
From: John Doe <john@example.com>
Subject: Test Message
Date: Wed, 14 Jun 2023 12:36:18 +0000
Message-ID: <abc123@example.com>

Hello, this is a test message.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_eq!(toc.msgs.len(), 1);

        let msg = &toc.msgs[0];
        assert_eq!(msg.from, "John Doe");
        assert_eq!(msg.subject, "Test Message");
        assert_eq!(msg.state, State::Unread);
        assert_eq!(msg.priority, 3);
        assert_ne!(msg.msg_id_hash, 0);
        assert!(msg.seconds > 0);
        assert_eq!(msg.serial_num, 1);
        assert!(msg.flags.contains(MsgFlags::UTF8));
    }

    #[test]
    fn build_two_messages() {
        let mbox = "\
From alice@test.com Wed Jun 14 10:00:00 2023
From: Alice <alice@test.com>
Subject: First
Date: Wed, 14 Jun 2023 10:00:00 +0000

Body 1.

From bob@test.com Thu Jun 15 11:00:00 2023
From: Bob <bob@test.com>
Subject: Second
Date: Thu, 15 Jun 2023 11:00:00 +0000

Body 2.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_eq!(toc.msgs.len(), 2);

        assert_eq!(toc.msgs[0].from, "Alice");
        assert_eq!(toc.msgs[0].subject, "First");
        assert_eq!(toc.msgs[0].serial_num, 1);

        assert_eq!(toc.msgs[1].from, "Bob");
        assert_eq!(toc.msgs[1].subject, "Second");
        assert_eq!(toc.msgs[1].serial_num, 2);

        // First message length should equal offset of second message minus first offset
        assert_eq!(toc.msgs[0].length, toc.msgs[1].offset - toc.msgs[0].offset);
    }

    #[test]
    fn build_status_read() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Subject: Read message
Status: RO

Read body.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_eq!(toc.msgs[0].state, State::Read);
    }

    #[test]
    fn build_priority_header() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Subject: High priority
X-Priority: 1

Urgent!
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_eq!(toc.msgs[0].priority, 1);
    }

    #[test]
    fn build_importance_high() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Subject: Important
Importance: High

Very important.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_eq!(toc.msgs[0].priority, 1);
    }

    #[test]
    fn build_importance_low() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Subject: Low
Importance: low

Not important.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_eq!(toc.msgs[0].priority, 5);
    }

    #[test]
    fn build_html_content_type() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Subject: HTML message
Content-Type: text/html; charset=utf-8

<html><body>Hello</body></html>
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert!(toc.msgs[0].flags.contains(MsgFlags::HTML));
    }

    #[test]
    fn build_bulk_precedence() {
        let mbox = "\
From list@test.com Wed Jun 14 12:00:00 2023
From: List <list@test.com>
Subject: Mailing list
Precedence: bulk

List message.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert!(toc.msgs[0].flags.contains(MsgFlags::BULK));
    }

    #[test]
    fn build_in_reply_to() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Subject: Reply
In-Reply-To: <original@example.com>
Message-ID: <reply@example.com>

Reply body.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_ne!(toc.msgs[0].in_reply_to_hash, 0);
        assert_ne!(toc.msgs[0].msg_id_hash, 0);
    }

    #[test]
    fn build_sender_priority_from_overrides_reply_to() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
Reply-To: reply@test.com
Sender: sender@test.com
From: from@test.com
Subject: Priority test

Body.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        // From: has highest priority (1), should override Sender (2) and Reply-To (3)
        assert_eq!(toc.msgs[0].from, "from@test.com");
    }

    #[test]
    fn build_sender_when_no_from_header() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
Sender: sender@test.com
Subject: No From header

Body.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_eq!(toc.msgs[0].from, "sender@test.com");
    }

    #[test]
    fn build_fallback_from_envelope() {
        // No From/Sender/Reply-To headers — should use envelope address
        let mbox = "\
From envelope@test.com Wed Jun 14 12:00:00 2023
Subject: Only envelope

Body.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_eq!(toc.msgs[0].from, "envelope@test.com");
    }

    #[test]
    fn build_body_offset_set() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Subject: Test

Body starts here.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert!(toc.msgs[0].body_offset > 0);
    }

    #[test]
    fn build_message_id_sets_uid_hash() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Message-ID: <unique123@example.com>
Subject: UID test

Body.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_ne!(toc.msgs[0].uid_hash, 0);
        // uid_hash should equal msg_id_hash when no uid_hash was set prior
        assert_eq!(toc.msgs[0].uid_hash, toc.msgs[0].msg_id_hash as u32);
    }

    #[test]
    fn build_unknown_sender_when_empty() {
        // Edge case: From line with no address (shouldn't normally happen but test defensively)
        // We'll use a valid From line but no From: header, and the envelope addr is empty-ish
        let mbox = "\
From - Wed Jun 14 12:00:00 2023
Subject: No sender

Body.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        // The "-" should be parsed as the from address
        assert!(!toc.msgs[0].from.is_empty());
    }

    #[test]
    fn build_next_serial_advances() {
        let mbox = "\
From a@b.c Wed Jun 14 10:00:00 2023
Subject: One

Body.

From d@e.f Wed Jun 14 11:00:00 2023
Subject: Two

Body.

From g@h.i Wed Jun 14 12:00:00 2023
Subject: Three

Body.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_eq!(toc.msgs.len(), 3);
        assert_eq!(toc.msgs[0].serial_num, 1);
        assert_eq!(toc.msgs[1].serial_num, 2);
        assert_eq!(toc.msgs[2].serial_num, 3);
        assert_eq!(toc.next_serial, 4);
    }

    #[test]
    fn build_toc_dirty_flag() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
Subject: test

Body.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert!(toc.dirty);
    }

    // ── rebuild_toc tests ──

    #[test]
    fn rebuild_no_old_toc() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Subject: Test

Body.
";
        let f = write_mbox(mbox);
        let toc = rebuild_toc(f.path(), None).unwrap();
        assert_eq!(toc.msgs.len(), 1);
        assert_eq!(toc.msgs[0].state, State::Unread);
    }

    #[test]
    fn rebuild_salvages_state_by_offset() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Subject: Test
Message-ID: <test@example.com>

Body.
";
        let f = write_mbox(mbox);

        // Build initial TOC
        let mut old_toc = build_toc(f.path()).unwrap();
        // Mark as read
        old_toc.msgs[0].state = State::Read;
        old_toc.msgs[0].priority = 1;
        old_toc.msgs[0].flags.insert(MsgFlags::FLAGGED);

        // Rebuild
        let new_toc = rebuild_toc(f.path(), Some(&old_toc)).unwrap();
        assert_eq!(new_toc.msgs.len(), 1);
        // State should be salvaged from old
        assert_eq!(new_toc.msgs[0].state, State::Read);
        assert_eq!(new_toc.msgs[0].priority, 1);
        assert!(new_toc.msgs[0].flags.contains(MsgFlags::FLAGGED));
    }

    #[test]
    fn rebuild_salvages_by_msg_id_hash() {
        // Build two mboxes with same message-id but different offsets
        let mbox1 = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Subject: Test
Message-ID: <unique42@example.com>

Body.
";
        let f1 = write_mbox(mbox1);
        let mut old_toc = build_toc(f1.path()).unwrap();
        old_toc.msgs[0].state = State::Replied;

        // Different mbox with same message-id but an extra message prepended
        let mbox2 = "\
From other@test.com Wed Jun 13 09:00:00 2023
From: Other <other@test.com>
Subject: Prepended
Message-ID: <other@example.com>

Prepended body.

From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Subject: Test
Message-ID: <unique42@example.com>

Body.
";
        let f2 = write_mbox(mbox2);
        let new_toc = rebuild_toc(f2.path(), Some(&old_toc)).unwrap();
        assert_eq!(new_toc.msgs.len(), 2);
        // The second message should have salvaged state via msg_id_hash
        // (offset won't match since there's a prepended message)
        // But lengths also need to match — they should since bodies are identical
        assert_eq!(new_toc.msgs[1].state, State::Replied);
    }

    // ── parse_date edge cases ──

    #[test]
    fn parse_date_no_timezone() {
        let d = parse_date("14 Jun 2023 12:00:00").unwrap();
        assert_eq!(d.tz_secs, 0);
        assert!(d.seconds > 0);
    }

    #[test]
    fn parse_date_symbolic_tz() {
        let d = parse_date("14 Jun 2023 12:00:00 PST").unwrap();
        assert_eq!(d.tz_secs, -8 * 3600);
    }

    #[test]
    fn parse_date_mbox_format() {
        // "Wed Jun 14 12:36:18 2023" — day name, month, day, time, year
        let d = parse_date("Wed Jun 14 12:36:18 2023").unwrap();
        assert!(d.seconds > 0);
    }

    // ── datetime_to_epoch additional ──

    #[test]
    fn epoch_end_of_year() {
        let secs = datetime_to_epoch(2023, 12, 31, 23, 59, 59);
        assert!(secs > 0);
    }

    #[test]
    fn epoch_feb_29_non_leap() {
        // 2023 is not a leap year, but we don't validate day vs month
        // The C code also doesn't — it just passes through to mktime
        let secs = datetime_to_epoch(2023, 2, 29, 0, 0, 0);
        assert!(secs > 0); // will compute something, even if "invalid"
    }

    // ── Offsets and lengths ──

    #[test]
    fn build_offsets_contiguous() {
        let mbox = "\
From a@b.c Wed Jun 14 10:00:00 2023
Subject: One

Body one.

From d@e.f Wed Jun 14 11:00:00 2023
Subject: Two

Body two.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert_eq!(toc.msgs.len(), 2);

        // Messages should be contiguous: offset[1] = offset[0] + length[0]
        assert_eq!(toc.msgs[1].offset, toc.msgs[0].offset + toc.msgs[0].length);

        // Last message length should reach end of file
        let file_len = std::fs::metadata(f.path()).unwrap().len() as i64;
        assert_eq!(
            toc.msgs[1].offset + toc.msgs[1].length,
            file_len
        );
    }

    #[test]
    fn build_nonexistent_file() {
        let result = build_toc(Path::new("/nonexistent/path/mailbox"));
        assert!(result.is_err());
    }

    // ── Date consistency between envelope and headers ──

    #[test]
    fn date_header_overrides_envelope() {
        let mbox = "\
From user@test.com Wed Jun 14 12:00:00 2023
From: User <user@test.com>
Date: Thu, 15 Jun 2023 08:30:00 +0000
Subject: Date test

Body.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        // The Date: header value should override the envelope date
        let expected = datetime_to_epoch(2023, 6, 15, 8, 30, 0) as u32;
        assert_eq!(toc.msgs[0].seconds, expected);
    }

    #[test]
    fn build_precedence_list() {
        let mbox = "\
From list@test.com Wed Jun 14 12:00:00 2023
From: List <list@test.com>
Subject: List post
Precedence: list

List message.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert!(toc.msgs[0].flags.contains(MsgFlags::BULK));
    }

    #[test]
    fn build_precedence_junk() {
        let mbox = "\
From junk@test.com Wed Jun 14 12:00:00 2023
From: Junk <junk@test.com>
Subject: Spam
Precedence: junk

Spam message.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        assert!(toc.msgs[0].flags.contains(MsgFlags::BULK));
    }

    #[test]
    fn build_x_priority_range() {
        for pri in 1u8..=5 {
            let mbox = format!(
                "From u@t.c Wed Jun 14 12:00:00 2023\n\
                 X-Priority: {}\n\
                 Subject: P{}\n\n\
                 Body.\n",
                pri, pri
            );
            let f = write_mbox(&mbox);
            let toc = build_toc(f.path()).unwrap();
            assert_eq!(toc.msgs[0].priority, pri, "priority {} mismatch", pri);
        }
    }

    #[test]
    fn build_x_priority_out_of_range() {
        let mbox = "\
From u@t.c Wed Jun 14 12:00:00 2023
X-Priority: 9
Subject: Bad priority

Body.
";
        let f = write_mbox(mbox);
        let toc = build_toc(f.path()).unwrap();
        // Out of range: should keep default priority of 3
        assert_eq!(toc.msgs[0].priority, 3);
    }
}
