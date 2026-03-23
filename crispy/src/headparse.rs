//! RFC 822 / 2822 header parsing for raw message text.
//!
//! Extracts header values from raw email text, handling:
//! - Case-insensitive header name matching
//! - RFC 2822 continuation lines (folded headers unfolded on retrieval)
//! - Standard header lookup by [`HeaderId`]
//! - Arbitrary header lookup by name string
//!
//! # Example
//! ```
//! use crispy::headparse::{HeaderId, find_header, get_value, get_by_id};
//!
//! let text = "From: alice@example.com\r\nSubject: Hello\r\n\r\nBody";
//! assert_eq!(get_by_id(text, HeaderId::Subject), Some("Hello".into()));
//! ```

use std::fmt;

// ---------------------------------------------------------------------------
// Standard header IDs
// ---------------------------------------------------------------------------

/// Well-known RFC 2822 header identifiers.
///
/// Each variant maps to the canonical header name string (including the
/// trailing colon-space) via [`HeaderId::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum HeaderId {
    To = 0,
    From,
    Subject,
    Cc,
    Bcc,
    ReplyTo,
    Date,
    MessageId,
    InReplyTo,
    References,
    ContentType,
    ContentTransferEncoding,
    ContentDisposition,
    MimeVersion,
    ReturnPath,
    Received,
    XMailer,
    XSender,
    XPriority,
}

/// Total number of standard header IDs.
pub const HEADER_ID_COUNT: usize = 19;

static HEADER_NAMES: [&str; HEADER_ID_COUNT] = [
    "To: ",
    "From: ",
    "Subject: ",
    "Cc: ",
    "Bcc: ",
    "Reply-To: ",
    "Date: ",
    "Message-Id: ",
    "In-Reply-To: ",
    "References: ",
    "Content-Type: ",
    "Content-Transfer-Encoding: ",
    "Content-Disposition: ",
    "MIME-Version: ",
    "Return-Path: ",
    "Received: ",
    "X-Mailer: ",
    "X-Sender: ",
    "X-Priority: ",
];

impl HeaderId {
    /// Canonical header name including trailing `": "`, e.g. `"Subject: "`.
    pub fn as_str(self) -> &'static str {
        HEADER_NAMES[self as usize]
    }

    /// Try to convert a `u8` ordinal back to a [`HeaderId`].
    pub fn from_u8(v: u8) -> Option<Self> {
        if (v as usize) < HEADER_ID_COUNT {
            // SAFETY: repr(u8) with contiguous values 0..HEADER_ID_COUNT-1
            Some(unsafe { std::mem::transmute(v) })
        } else {
            None
        }
    }
}

impl fmt::Display for HeaderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// HeaderSpec — position descriptor
// ---------------------------------------------------------------------------

/// Byte-offset descriptor locating a header inside raw message text.
///
/// All offsets are relative to the start of the text slice passed to
/// [`find_header`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderSpec {
    /// Byte offset where the header line starts (the first character of the
    /// header name).
    pub start: usize,
    /// Byte offset where the header value starts (after the name, colon, and
    /// any leading whitespace).
    pub value: usize,
    /// Byte offset one past the last character of the header value (end of
    /// the last continuation line, before the terminating CRLF/LF).
    pub end: usize,
}

impl HeaderSpec {
    /// Total length of the header (name + value) in bytes.
    pub fn total_len(&self) -> usize {
        self.end - self.start
    }

    /// Length of the raw (still-folded) value portion.
    pub fn value_len(&self) -> usize {
        self.end - self.value
    }
}

// ---------------------------------------------------------------------------
// Core parsing
// ---------------------------------------------------------------------------

/// Find a header by name in raw RFC 822 text.
///
/// `name` should include the colon (and optional trailing space), e.g.
/// `"Subject: "` or `"To:"`. Matching is case-insensitive on the name
/// portion. Continuation lines (starting with whitespace) are included in
/// the span.
///
/// Returns a [`HeaderSpec`] describing the byte offsets, or `None` if the
/// header is not found.
pub fn find_header(text: &str, name: &str) -> Option<HeaderSpec> {
    let bytes = text.as_bytes();
    let name_bytes = name.as_bytes();
    let name_len = name_bytes.len();
    let text_len = bytes.len();

    let mut pos = 0usize;

    while pos < text_len {
        // Check for end-of-headers (blank line or end of text).
        if bytes[pos] == b'\r' || bytes[pos] == b'\n' {
            break;
        }

        // Case-insensitive prefix match.
        if pos + name_len <= text_len
            && bytes[pos..pos + name_len]
                .iter()
                .zip(name_bytes.iter())
                .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
        {
            let header_start = pos;

            // Skip optional whitespace after the name prefix.
            let mut val_start = pos + name_len;
            while val_start < text_len
                && (bytes[val_start] == b' ' || bytes[val_start] == b'\t')
            {
                val_start += 1;
            }

            // Walk to end of value, consuming continuation lines.
            let mut val_end = val_start;
            while val_end < text_len {
                if bytes[val_end] == b'\r' || bytes[val_end] == b'\n' {
                    let mut next = val_end;
                    if next < text_len && bytes[next] == b'\r' {
                        next += 1;
                    }
                    if next < text_len && bytes[next] == b'\n' {
                        next += 1;
                    }
                    // Continuation line?
                    if next < text_len
                        && (bytes[next] == b' ' || bytes[next] == b'\t')
                    {
                        val_end = next;
                        continue;
                    }
                    break;
                }
                val_end += 1;
            }

            return Some(HeaderSpec {
                start: header_start,
                value: val_start,
                end: val_end,
            });
        }

        // Advance to next line.
        while pos < text_len && bytes[pos] != b'\n' {
            pos += 1;
        }
        if pos < text_len && bytes[pos] == b'\n' {
            pos += 1;
        }
    }

    None
}

/// Extract the raw header value substring from `text` using a previously
/// obtained [`HeaderSpec`].
///
/// The returned slice still contains any embedded CRLF + whitespace from
/// continuation lines. Use [`get_value_unfolded`] if you need a clean
/// single-line string.
pub fn get_value_raw<'a>(text: &'a str, spec: &HeaderSpec) -> &'a str {
    &text[spec.value..spec.end]
}

/// Extract the header value and unfold continuation lines.
///
/// RFC 2822 continuation lines (CRLF followed by whitespace) are replaced
/// with a single space, producing a clean single-line value.
pub fn get_value_unfolded(text: &str, spec: &HeaderSpec) -> String {
    unfold(&text[spec.value..spec.end])
}

/// Extract the value of a well-known header by [`HeaderId`].
///
/// Returns `Some(value)` with continuation lines unfolded, or `None` if the
/// header is not present.
pub fn get_by_id(text: &str, id: HeaderId) -> Option<String> {
    let spec = find_header(text, id.as_str())?;
    Some(get_value_unfolded(text, &spec))
}

/// Extract the raw (still folded) value of a well-known header.
pub fn get_by_id_raw<'a>(text: &'a str, id: HeaderId) -> Option<&'a str> {
    let spec = find_header(text, id.as_str())?;
    Some(get_value_raw(text, &spec))
}

// ---------------------------------------------------------------------------
// Iterator over all headers
// ---------------------------------------------------------------------------

/// An iterator that yields `(name, unfolded_value)` pairs for every header
/// in raw message text.
pub struct HeaderIter<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> HeaderIter<'a> {
    pub fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }
}

impl<'a> Iterator for HeaderIter<'a> {
    type Item = (&'a str, String);

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.text.as_bytes();
        let len = bytes.len();

        // Skip past any blank lines / end-of-headers marker.
        if self.pos >= len {
            return None;
        }
        if bytes[self.pos] == b'\r' || bytes[self.pos] == b'\n' {
            return None; // end of header block
        }

        let line_start = self.pos;

        // Find the colon that separates name from value.
        let mut colon = line_start;
        while colon < len && bytes[colon] != b':' && bytes[colon] != b'\r' && bytes[colon] != b'\n' {
            colon += 1;
        }
        if colon >= len || bytes[colon] != b':' {
            // Malformed line — skip it.
            while self.pos < len && bytes[self.pos] != b'\n' {
                self.pos += 1;
            }
            if self.pos < len {
                self.pos += 1;
            }
            return self.next();
        }

        let name = &self.text[line_start..colon];

        // Skip colon and optional whitespace.
        let mut val_start = colon + 1;
        while val_start < len && (bytes[val_start] == b' ' || bytes[val_start] == b'\t') {
            val_start += 1;
        }

        // Walk to end of value, consuming continuation lines.
        let mut val_end = val_start;
        while val_end < len {
            if bytes[val_end] == b'\r' || bytes[val_end] == b'\n' {
                let mut next = val_end;
                if next < len && bytes[next] == b'\r' {
                    next += 1;
                }
                if next < len && bytes[next] == b'\n' {
                    next += 1;
                }
                if next < len && (bytes[next] == b' ' || bytes[next] == b'\t') {
                    val_end = next;
                    continue;
                }
                // Update pos to the start of the next line.
                self.pos = next;
                let value = unfold(&self.text[val_start..val_end]);
                return Some((name, value));
            }
            val_end += 1;
        }

        // Reached end of text.
        self.pos = len;
        let value = unfold(&self.text[val_start..val_end]);
        Some((name, value))
    }
}

/// Convenience function: iterate over all headers in `text`.
pub fn headers(text: &str) -> HeaderIter<'_> {
    HeaderIter::new(text)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Unfold RFC 2822 continuation lines: replace each CRLF/LF followed by
/// whitespace with a single space.
fn unfold(s: &str) -> String {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;

    while i < len {
        if bytes[i] == b'\r' || bytes[i] == b'\n' {
            let mut j = i;
            if j < len && bytes[j] == b'\r' {
                j += 1;
            }
            if j < len && bytes[j] == b'\n' {
                j += 1;
            }
            if j < len && (bytes[j] == b' ' || bytes[j] == b'\t') {
                // Replace fold with single space.
                out.push(' ');
                // Skip all leading whitespace on the continuation.
                while j < len && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }
                i = j;
                continue;
            }
            // Bare newline inside value (shouldn't happen in well-formed
            // headers, but handle gracefully).
            out.push(bytes[i] as char);
            i += 1;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }

    out
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_HEADERS: &str = "\
From: alice@example.com\r\n\
To: bob@example.com\r\n\
Subject: Hello World\r\n\
Date: Mon, 1 Jan 2024 12:00:00 +0000\r\n\
\r\n\
Body text here.\r\n";

    #[test]
    fn find_simple_header() {
        let spec = find_header(SIMPLE_HEADERS, "Subject: ").unwrap();
        assert_eq!(get_value_raw(SIMPLE_HEADERS, &spec), "Hello World");
    }

    #[test]
    fn find_header_case_insensitive() {
        let spec = find_header(SIMPLE_HEADERS, "subject: ").unwrap();
        assert_eq!(get_value_raw(SIMPLE_HEADERS, &spec), "Hello World");

        let spec = find_header(SIMPLE_HEADERS, "SUBJECT: ").unwrap();
        assert_eq!(get_value_raw(SIMPLE_HEADERS, &spec), "Hello World");
    }

    #[test]
    fn find_header_not_found() {
        assert!(find_header(SIMPLE_HEADERS, "X-Missing: ").is_none());
    }

    #[test]
    fn get_by_id_from() {
        assert_eq!(
            get_by_id(SIMPLE_HEADERS, HeaderId::From),
            Some("alice@example.com".into())
        );
    }

    #[test]
    fn get_by_id_to() {
        assert_eq!(
            get_by_id(SIMPLE_HEADERS, HeaderId::To),
            Some("bob@example.com".into())
        );
    }

    #[test]
    fn get_by_id_date() {
        assert_eq!(
            get_by_id(SIMPLE_HEADERS, HeaderId::Date),
            Some("Mon, 1 Jan 2024 12:00:00 +0000".into())
        );
    }

    #[test]
    fn get_by_id_missing() {
        assert_eq!(get_by_id(SIMPLE_HEADERS, HeaderId::Bcc), None);
    }

    // --- Continuation / folded headers ---

    const FOLDED_HEADERS: &str = "\
From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Subject: This is a very long subject line\r\n \
that has been folded across\r\n\
\tmultiple lines\r\n\
References: <msg1@example.com>\r\n \
<msg2@example.com>\r\n \
<msg3@example.com>\r\n\
\r\n";

    #[test]
    fn folded_subject() {
        let val = get_by_id(FOLDED_HEADERS, HeaderId::Subject).unwrap();
        assert_eq!(
            val,
            "This is a very long subject line that has been folded across multiple lines"
        );
    }

    #[test]
    fn folded_references() {
        let val = get_by_id(FOLDED_HEADERS, HeaderId::References).unwrap();
        assert_eq!(
            val,
            "<msg1@example.com> <msg2@example.com> <msg3@example.com>"
        );
    }

    #[test]
    fn raw_value_preserves_folds() {
        let raw = get_by_id_raw(FOLDED_HEADERS, HeaderId::Subject).unwrap();
        assert!(raw.contains("\r\n"));
    }

    // --- LF-only line endings ---

    const LF_HEADERS: &str = "\
From: sender@example.com\n\
Subject: LF only\n\
\n\
body\n";

    #[test]
    fn lf_only_line_endings() {
        assert_eq!(
            get_by_id(LF_HEADERS, HeaderId::Subject),
            Some("LF only".into())
        );
    }

    // --- Mixed and edge cases ---

    #[test]
    fn empty_value() {
        let text = "Subject: \r\nFrom: a@b\r\n\r\n";
        assert_eq!(get_by_id(text, HeaderId::Subject), Some("".into()));
    }

    #[test]
    fn header_at_end_without_trailing_newline() {
        let text = "Subject: no newline";
        assert_eq!(
            get_by_id(text, HeaderId::Subject),
            Some("no newline".into())
        );
    }

    #[test]
    fn content_type_with_params() {
        let text = "Content-Type: multipart/mixed;\r\n boundary=\"----=_Part_123\"\r\n\r\n";
        let val = get_by_id(text, HeaderId::ContentType).unwrap();
        assert_eq!(val, "multipart/mixed; boundary=\"----=_Part_123\"");
    }

    // --- HeaderSpec geometry ---

    #[test]
    fn header_spec_offsets() {
        let spec = find_header(SIMPLE_HEADERS, "Subject: ").unwrap();
        assert_eq!(spec.start, SIMPLE_HEADERS.find("Subject:").unwrap());
        assert_eq!(
            &SIMPLE_HEADERS[spec.value..spec.end],
            "Hello World"
        );
        assert_eq!(spec.total_len(), "Subject: Hello World".len());
    }

    // --- HeaderId round-trip ---

    #[test]
    fn header_id_from_u8() {
        for i in 0..HEADER_ID_COUNT {
            let id = HeaderId::from_u8(i as u8).unwrap();
            assert_eq!(id as u8, i as u8);
        }
        assert!(HeaderId::from_u8(HEADER_ID_COUNT as u8).is_none());
        assert!(HeaderId::from_u8(255).is_none());
    }

    #[test]
    fn header_id_display() {
        assert_eq!(HeaderId::Subject.to_string(), "Subject: ");
    }

    // --- Iterator ---

    #[test]
    fn iterate_all_headers() {
        let hdrs: Vec<_> = headers(SIMPLE_HEADERS).collect();
        assert_eq!(hdrs.len(), 4);
        assert_eq!(hdrs[0], ("From", "alice@example.com".into()));
        assert_eq!(hdrs[1], ("To", "bob@example.com".into()));
        assert_eq!(hdrs[2], ("Subject", "Hello World".into()));
        assert_eq!(hdrs[3], ("Date", "Mon, 1 Jan 2024 12:00:00 +0000".into()));
    }

    #[test]
    fn iterate_folded_headers() {
        let hdrs: Vec<_> = headers(FOLDED_HEADERS).collect();
        assert_eq!(hdrs.len(), 4);
        assert_eq!(
            hdrs[2].1,
            "This is a very long subject line that has been folded across multiple lines"
        );
    }

    // --- Real-world email header block ---

    const REAL_HEADERS: &str = "\
Return-Path: <bounces@lists.example.org>\r\n\
Received: from mx.example.org (mx.example.org [192.0.2.1])\r\n\
\tby mail.local (Postfix) with ESMTPS id ABC123\r\n\
\tfor <user@local.example>; Mon, 15 Jan 2024 08:30:00 -0500\r\n\
From: \"Alice Wonderland\" <alice@example.com>\r\n\
To: Bob <bob@example.com>,\r\n \
Carol <carol@example.com>\r\n\
Cc: dave@example.com\r\n\
Subject: Re: Meeting notes\r\n\
Date: Mon, 15 Jan 2024 13:30:00 +0000\r\n\
Message-Id: <20240115133000.ABCDEF@example.com>\r\n\
In-Reply-To: <prev-msg@example.com>\r\n\
References: <original@example.com>\r\n \
<prev-msg@example.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
X-Mailer: Eudora/8.0\r\n\
X-Priority: 3\r\n\
X-Sender: alice@example.com\r\n\
\r\n\
This is the body.\r\n";

    #[test]
    fn real_return_path() {
        assert_eq!(
            get_by_id(REAL_HEADERS, HeaderId::ReturnPath),
            Some("<bounces@lists.example.org>".into())
        );
    }

    #[test]
    fn real_received_folded() {
        let val = get_by_id(REAL_HEADERS, HeaderId::Received).unwrap();
        assert!(val.starts_with("from mx.example.org"));
        assert!(val.contains("for <user@local.example>"));
        // Continuation lines should be unfolded.
        assert!(!val.contains('\n'));
    }

    #[test]
    fn real_to_folded() {
        let val = get_by_id(REAL_HEADERS, HeaderId::To).unwrap();
        assert_eq!(val, "Bob <bob@example.com>, Carol <carol@example.com>");
    }

    #[test]
    fn real_references_folded() {
        let val = get_by_id(REAL_HEADERS, HeaderId::References).unwrap();
        assert_eq!(val, "<original@example.com> <prev-msg@example.com>");
    }

    #[test]
    fn real_xmailer() {
        assert_eq!(
            get_by_id(REAL_HEADERS, HeaderId::XMailer),
            Some("Eudora/8.0".into())
        );
    }

    #[test]
    fn real_content_type() {
        assert_eq!(
            get_by_id(REAL_HEADERS, HeaderId::ContentType),
            Some("text/plain; charset=\"utf-8\"".into())
        );
    }

    #[test]
    fn real_content_transfer_encoding() {
        assert_eq!(
            get_by_id(REAL_HEADERS, HeaderId::ContentTransferEncoding),
            Some("quoted-printable".into())
        );
    }

    #[test]
    fn real_mime_version() {
        assert_eq!(
            get_by_id(REAL_HEADERS, HeaderId::MimeVersion),
            Some("1.0".into())
        );
    }

    #[test]
    fn real_iterate_count() {
        let count = headers(REAL_HEADERS).count();
        assert_eq!(count, 16);
    }
}
