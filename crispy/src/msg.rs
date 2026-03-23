//! RFC 5322 message construction and parsing.
//!
//! Build complete MIME messages from parts (text, HTML, attachments).
//! Parse received messages into structured parts.

use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::base64;
use crate::encode;
use crate::headparse;
use crate::md5;
use crate::richtext;

// ============================================================
// MIME type detection
// ============================================================

/// Guess MIME type from a filename extension. Returns a static string.
pub fn mime_type(filename: &str) -> &'static str {
    let dot = match filename.rfind('.') {
        Some(i) => &filename[i..],
        None => return "application/octet-stream",
    };
    let lower = dot.to_ascii_lowercase();
    match lower.as_str() {
        ".txt" => "text/plain",
        ".html" | ".htm" => "text/html",
        ".css" => "text/css",
        ".csv" => "text/csv",
        ".xml" => "text/xml",
        ".json" => "application/json",
        ".pdf" => "application/pdf",
        ".zip" => "application/zip",
        ".gz" => "application/gzip",
        ".tar" => "application/x-tar",
        ".doc" => "application/msword",
        ".docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ".xls" => "application/vnd.ms-excel",
        ".xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ".ppt" => "application/vnd.ms-powerpoint",
        ".png" => "image/png",
        ".jpg" | ".jpeg" => "image/jpeg",
        ".gif" => "image/gif",
        ".bmp" => "image/bmp",
        ".svg" => "image/svg+xml",
        ".ico" => "image/x-icon",
        ".webp" => "image/webp",
        ".mp3" => "audio/mpeg",
        ".wav" => "audio/wav",
        ".mp4" => "video/mp4",
        ".avi" => "video/x-msvideo",
        ".mov" => "video/quicktime",
        ".eml" => "message/rfc822",
        _ => "application/octet-stream",
    }
}

// ============================================================
// Boundary and Message-ID generation
// ============================================================

/// Generate a unique MIME boundary string.
pub fn generate_boundary() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let r = simple_rand();
    format!("----=_CrispyBoundary_{:x}_{:x}", ts, r)
}

/// Generate a Message-ID using MD5 of timestamp + random + pid.
pub fn generate_message_id(domain: &str) -> String {
    let domain = if domain.is_empty() { "crispy.local" } else { domain };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let r = simple_rand();
    let pid = std::process::id();
    let seed = format!("{:x}.{:x}.{}", ts, r, pid);
    let hex = md5::md5_hex(seed.as_bytes());
    format!("<{}@{}>", hex, domain)
}

/// Simple pseudo-random based on time nanos (no external deps).
fn simple_rand() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    // Mix with a constant to reduce predictability
    nanos.wrapping_mul(2654435761)
}

// ============================================================
// Date formatting (RFC 5322)
// ============================================================

/// Format a Unix timestamp with timezone offset into RFC 5322 date.
pub fn format_date(unix_time: i64, tz_offset_secs: i64) -> String {
    const DAY_NAMES: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTH_NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    // Adjust for timezone
    let t = unix_time + tz_offset_secs;

    // Days since epoch (1970-01-01 is Thursday)
    let mut days = t / 86400;
    let mut rem = t % 86400;
    if rem < 0 {
        days -= 1;
        rem += 86400;
    }
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    // Day of week (epoch was Thursday = index 0)
    let wday = ((days % 7) + 7) % 7;
    let day_name = DAY_NAMES[wday as usize];

    // Convert days to y/m/d
    let (year, month, mday) = days_to_ymd(days);

    let tz_h = tz_offset_secs.abs() / 3600;
    let tz_m = (tz_offset_secs.abs() % 3600) / 60;
    let sign = if tz_offset_secs < 0 { '-' } else { '+' };

    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} {}{:02}{:02}",
        day_name,
        mday,
        MONTH_NAMES[(month - 1) as usize],
        year,
        hour,
        min,
        sec,
        sign,
        tz_h,
        tz_m
    )
}

/// Convert days since epoch to (year, month 1-12, day 1-31).
fn days_to_ymd(days: i64) -> (i64, i64, i64) {
    // Algorithm from Howard Hinnant
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe =
        (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ============================================================
// Attachment
// ============================================================

/// An attachment to include in a message.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// File path (reads file at build time). Mutually exclusive with `data`.
    pub path: Option<String>,
    /// Display filename. If None, derived from path.
    pub filename: Option<String>,
    /// MIME type. If None, auto-detected from filename.
    pub mime_type: Option<String>,
    /// Raw data (alternative to path).
    pub data: Option<Vec<u8>>,
    /// True for inline (Content-ID) vs regular attachment.
    pub is_inline: bool,
    /// Content-ID for inline references in HTML (without angle brackets).
    pub content_id: Option<String>,
}

impl Default for Attachment {
    fn default() -> Self {
        Self {
            path: None,
            filename: None,
            mime_type: None,
            data: None,
            is_inline: false,
            content_id: None,
        }
    }
}

impl Attachment {
    /// Create an attachment from file path.
    pub fn from_path(path: &str) -> Self {
        Self {
            path: Some(path.to_string()),
            ..Default::default()
        }
    }

    /// Create an attachment from raw data.
    pub fn from_data(data: Vec<u8>, filename: &str) -> Self {
        Self {
            data: Some(data),
            filename: Some(filename.to_string()),
            ..Default::default()
        }
    }

    /// Create an inline attachment (for HTML img src="cid:...").
    pub fn inline_from_data(data: Vec<u8>, filename: &str, content_id: &str) -> Self {
        Self {
            data: Some(data),
            filename: Some(filename.to_string()),
            is_inline: true,
            content_id: Some(content_id.to_string()),
            ..Default::default()
        }
    }

    /// Resolve the display filename.
    fn resolved_filename(&self) -> &str {
        if let Some(ref f) = self.filename {
            return f;
        }
        if let Some(ref p) = self.path {
            if let Some(pos) = p.rfind('/') {
                return &p[pos + 1..];
            }
            return p;
        }
        "attachment"
    }

    /// Resolve the MIME type.
    fn resolved_mime_type(&self) -> &str {
        if let Some(ref m) = self.mime_type {
            return m;
        }
        mime_type(self.resolved_filename())
    }

    /// Load the raw bytes (from data or file).
    fn load_data(&self) -> Option<Vec<u8>> {
        if let Some(ref d) = self.data {
            return Some(d.clone());
        }
        if let Some(ref p) = self.path {
            return fs::read(p).ok();
        }
        None
    }
}

// ============================================================
// Message (for building)
// ============================================================

/// A message to build into an RFC 5322 string.
#[derive(Debug, Clone, Default)]
pub struct Message {
    /// From address: "Name <addr>" or just "addr".
    pub from: String,
    /// To addresses, comma-separated.
    pub to: String,
    /// Cc addresses, comma-separated.
    pub cc: Option<String>,
    /// Bcc addresses, comma-separated. Not written to headers.
    pub bcc: Option<String>,
    /// Reply-To address.
    pub reply_to: Option<String>,
    /// Subject line.
    pub subject: String,
    /// Plain text body.
    pub body_plain: Option<String>,
    /// HTML body.
    pub body_html: Option<String>,
    /// Attachments.
    pub attachments: Vec<Attachment>,
    /// Pre-set Message-ID (None = auto-generate).
    pub message_id: Option<String>,
    /// In-Reply-To header.
    pub in_reply_to: Option<String>,
    /// References header.
    pub references: Option<String>,
    /// X-Mailer header (None = "Crispy Mail Library").
    pub x_mailer: Option<String>,
    /// Priority: 0=normal, 1=highest, 5=lowest.
    pub priority: u8,
    /// Timezone offset in seconds for Date header.
    pub tz_offset: i64,
}

impl Message {
    /// Build the complete RFC 5322 message as a string.
    pub fn build(&self) -> Option<String> {
        if self.from.is_empty() || self.to.is_empty() {
            return None;
        }

        let mut buf = String::with_capacity(4096);

        // Date
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let _ = write!(buf, "Date: {}\r\n", format_date(now, self.tz_offset));

        // From / To / Cc
        let _ = write!(buf, "From: {}\r\n", self.from);
        let _ = write!(buf, "To: {}\r\n", self.to);
        if let Some(ref cc) = self.cc {
            if !cc.is_empty() {
                let _ = write!(buf, "Cc: {}\r\n", cc);
            }
        }
        // BCC not written to headers (envelope only)

        // Subject - encode if non-ASCII
        let subj_encoded = encode::encode_header(&self.subject);
        let _ = write!(buf, "Subject: {}\r\n", subj_encoded);

        // Message-ID
        if let Some(ref mid) = self.message_id {
            let _ = write!(buf, "Message-ID: {}\r\n", mid);
        } else {
            let domain = extract_domain(&self.from);
            let mid = generate_message_id(&domain);
            let _ = write!(buf, "Message-ID: {}\r\n", mid);
        }

        // In-Reply-To / References / Reply-To
        if let Some(ref irt) = self.in_reply_to {
            let _ = write!(buf, "In-Reply-To: {}\r\n", irt);
        }
        if let Some(ref refs) = self.references {
            let _ = write!(buf, "References: {}\r\n", refs);
        }
        if let Some(ref rto) = self.reply_to {
            let _ = write!(buf, "Reply-To: {}\r\n", rto);
        }

        // Priority
        if self.priority > 0 && self.priority != 3 {
            let _ = write!(buf, "X-Priority: {}\r\n", self.priority);
        }

        // X-Mailer
        let mailer = self
            .x_mailer
            .as_deref()
            .unwrap_or("Crispy Mail Library");
        let _ = write!(buf, "X-Mailer: {}\r\n", mailer);

        buf.push_str("MIME-Version: 1.0\r\n");

        // Decide structure
        let has_html = self.body_html.as_ref().map_or(false, |h| !h.is_empty());
        let has_attach = !self.attachments.is_empty();

        if has_attach {
            let boundary_mixed = generate_boundary();

            let _ = write!(
                buf,
                "Content-Type: multipart/mixed;\r\n boundary=\"{}\"\r\n",
                boundary_mixed
            );
            buf.push_str("\r\n");
            buf.push_str("This is a multi-part message in MIME format.\r\n");

            if has_html {
                // mixed { alternative { plain, html }, attachments }
                let boundary_alt = generate_boundary();
                let _ = write!(buf, "\r\n--{}\r\n", boundary_mixed);
                let _ = write!(
                    buf,
                    "Content-Type: multipart/alternative;\r\n boundary=\"{}\"\r\n",
                    boundary_alt
                );

                // Plain part
                let _ = write!(buf, "\r\n--{}\r\n", boundary_alt);
                write_text_part(
                    &mut buf,
                    "text/plain",
                    self.body_plain.as_deref().unwrap_or(""),
                );

                // HTML part
                let _ = write!(buf, "\r\n--{}\r\n", boundary_alt);
                write_text_part(
                    &mut buf,
                    "text/html",
                    self.body_html.as_deref().unwrap_or(""),
                );

                let _ = write!(buf, "\r\n--{}--\r\n", boundary_alt);
            } else {
                // mixed { plain, attachments }
                let _ = write!(buf, "\r\n--{}\r\n", boundary_mixed);
                write_text_part(
                    &mut buf,
                    "text/plain",
                    self.body_plain.as_deref().unwrap_or(""),
                );
            }

            // Attachments
            for att in &self.attachments {
                write_attachment(&mut buf, att, &boundary_mixed);
            }

            let _ = write!(buf, "\r\n--{}--\r\n", boundary_mixed);
        } else if has_html {
            // alternative { plain, html }
            let boundary_alt = generate_boundary();
            let _ = write!(
                buf,
                "Content-Type: multipart/alternative;\r\n boundary=\"{}\"\r\n",
                boundary_alt
            );
            buf.push_str("\r\n");

            // Plain
            let _ = write!(buf, "\r\n--{}\r\n", boundary_alt);
            write_text_part(
                &mut buf,
                "text/plain",
                self.body_plain.as_deref().unwrap_or(""),
            );

            // HTML
            let _ = write!(buf, "\r\n--{}\r\n", boundary_alt);
            write_text_part(
                &mut buf,
                "text/html",
                self.body_html.as_deref().unwrap_or(""),
            );

            let _ = write!(buf, "\r\n--{}--\r\n", boundary_alt);
        } else {
            // Simple plain text
            write_text_part(
                &mut buf,
                "text/plain",
                self.body_plain.as_deref().unwrap_or(""),
            );
        }

        Some(buf)
    }

    /// Extract all envelope recipients (To + Cc + Bcc) as bare addresses.
    pub fn recipients(&self) -> Vec<String> {
        let mut out = Vec::new();
        split_addresses(&self.to, &mut out);
        if let Some(ref cc) = self.cc {
            split_addresses(cc, &mut out);
        }
        if let Some(ref bcc) = self.bcc {
            split_addresses(bcc, &mut out);
        }
        out
    }
}

// ============================================================
// Text part / attachment writing helpers
// ============================================================

/// Check if text has non-ASCII or long lines requiring QP encoding.
fn needs_encoding(text: &str) -> bool {
    let mut line_len = 0;
    for b in text.bytes() {
        if b > 126 {
            return true;
        }
        if b == b'\n' {
            line_len = 0;
        } else {
            line_len += 1;
            if line_len > 76 {
                return true;
            }
        }
    }
    false
}

/// Write a text/* MIME part with automatic Content-Transfer-Encoding selection.
fn write_text_part(buf: &mut String, content_type: &str, text: &str) {
    let _ = write!(buf, "Content-Type: {}; charset=UTF-8\r\n", content_type);

    if needs_encoding(text) {
        buf.push_str("Content-Transfer-Encoding: quoted-printable\r\n\r\n");
        let qp = encode::qp_encode(text.as_bytes());
        buf.push_str(&qp);
    } else {
        buf.push_str("Content-Transfer-Encoding: 7bit\r\n\r\n");
        buf.push_str(text);
    }
    buf.push_str("\r\n");
}

/// Write an attachment as a base64-encoded MIME part.
fn write_attachment(buf: &mut String, att: &Attachment, boundary: &str) {
    let data = match att.load_data() {
        Some(d) => d,
        None => return,
    };

    let fname = att.resolved_filename();
    let mt = att.resolved_mime_type();

    let _ = write!(buf, "\r\n--{}\r\n", boundary);

    if att.is_inline {
        if let Some(ref cid) = att.content_id {
            let _ = write!(buf, "Content-Type: {}; name=\"{}\"\r\n", mt, fname);
            buf.push_str("Content-Transfer-Encoding: base64\r\n");
            let _ = write!(buf, "Content-ID: <{}>\r\n", cid);
            buf.push_str("Content-Disposition: inline\r\n");
        }
    } else {
        let _ = write!(buf, "Content-Type: {}; name=\"{}\"\r\n", mt, fname);
        buf.push_str("Content-Transfer-Encoding: base64\r\n");
        let _ = write!(buf, "Content-Disposition: attachment; filename=\"{}\"\r\n", fname);
    }
    buf.push_str("\r\n");

    // Base64 encode and write in 76-char lines
    let b64 = base64::encode(&data);
    let b64_bytes = b64.as_bytes();
    let mut i = 0;
    while i < b64_bytes.len() {
        let end = std::cmp::min(i + 76, b64_bytes.len());
        // SAFETY: base64 output is always valid ASCII
        buf.push_str(&b64[i..end]);
        buf.push_str("\r\n");
        i = end;
    }
}

// ============================================================
// Address helpers
// ============================================================

/// Extract bare email address from "Name <addr>" or just "addr".
fn extract_bare_address(full: &str) -> String {
    let full = full.trim();
    if let Some(lt) = full.find('<') {
        if let Some(gt) = full[lt..].find('>') {
            return full[lt + 1..lt + gt].to_string();
        }
    }
    full.to_string()
}

/// Extract domain from an email address (after @, stripping >).
fn extract_domain(addr: &str) -> String {
    // Try <addr@domain> first
    let bare = extract_bare_address(addr);
    if let Some(at) = bare.find('@') {
        let domain = &bare[at + 1..];
        // Strip trailing >
        return domain.trim_end_matches('>').to_string();
    }
    "localhost".to_string()
}

/// Split a comma-separated address list into bare addresses.
fn split_addresses(list: &str, out: &mut Vec<String>) {
    if list.is_empty() {
        return;
    }
    let mut depth = 0i32;
    let mut start = 0;
    let bytes = list.as_bytes();

    for i in 0..bytes.len() {
        match bytes[i] {
            b'<' => depth += 1,
            b'>' => depth -= 1,
            b',' if depth == 0 => {
                let part = list[start..i].trim();
                if !part.is_empty() {
                    let addr = extract_bare_address(part);
                    if !addr.is_empty() {
                        out.push(addr);
                    }
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    // Last entry
    let part = list[start..].trim();
    if !part.is_empty() {
        let addr = extract_bare_address(part);
        if !addr.is_empty() {
            out.push(addr);
        }
    }
}

// ============================================================
// Parsed message structures
// ============================================================

/// A single part of a parsed MIME message.
#[derive(Debug, Clone)]
pub struct ParsedPart {
    /// MIME type, e.g. "text/plain".
    pub mime_type: String,
    /// Charset, e.g. "UTF-8".
    pub charset: Option<String>,
    /// Filename for attachments.
    pub filename: Option<String>,
    /// Content-ID for inline parts (without angle brackets).
    pub content_id: Option<String>,
    /// Decoded content.
    pub data: Vec<u8>,
    /// Whether this is an attachment (vs body part).
    pub is_attachment: bool,
}

impl ParsedPart {
    /// Convenience: get data as a UTF-8 string (if it is text).
    pub fn as_text(&self) -> Option<&str> {
        std::str::from_utf8(&self.data).ok()
    }
}

/// A fully parsed RFC 5322 message.
#[derive(Debug, Clone, Default)]
pub struct ParsedMessage {
    pub from: Option<String>,
    pub to: Option<String>,
    pub cc: Option<String>,
    pub subject: Option<String>,
    pub date: Option<String>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,

    /// All decoded parts.
    pub parts: Vec<ParsedPart>,

    /// Index into `parts` of the first text/plain body (not attachment).
    pub body_plain_index: Option<usize>,
    /// Index into `parts` of the first text/html body (not attachment).
    pub body_html_index: Option<usize>,
}

impl ParsedMessage {
    /// Convenience: get the plain text body content.
    pub fn body_plain(&self) -> Option<&str> {
        self.body_plain_index
            .and_then(|i| self.parts.get(i))
            .and_then(|p| p.as_text())
    }

    /// Convenience: get the HTML body content.
    pub fn body_html(&self) -> Option<&str> {
        self.body_html_index
            .and_then(|i| self.parts.get(i))
            .and_then(|p| p.as_text())
    }

    /// Get all attachment parts.
    pub fn attachments(&self) -> Vec<&ParsedPart> {
        self.parts.iter().filter(|p| p.is_attachment).collect()
    }
}

// ============================================================
// Parsing
// ============================================================

/// Parse a raw RFC 5322 message into structured parts.
///
/// Handles multipart/mixed, multipart/alternative, multipart/related,
/// multipart/digest, message/rfc822 (nested messages), text/enriched,
/// text/richtext, base64, quoted-printable, charset conversion to UTF-8.
pub fn parse(raw: &[u8]) -> Option<ParsedMessage> {
    if raw.is_empty() {
        return None;
    }

    let text = String::from_utf8_lossy(raw);
    let mut out = ParsedMessage::default();

    // Parse standard headers with RFC 2047 decoding
    macro_rules! parse_header {
        ($name:expr, $field:ident) => {
            if let Some(spec) = headparse::find_header(&text, $name) {
                let raw_val = headparse::get_value_unfolded(&text, &spec);
                out.$field = Some(encode::decode_header(&raw_val));
            }
        };
    }

    parse_header!("From:", from);
    parse_header!("To:", to);
    parse_header!("Cc:", cc);
    parse_header!("Subject:", subject);
    parse_header!("Date:", date);
    parse_header!("Message-Id:", message_id);
    // Also try Message-ID (case variant)
    if out.message_id.is_none() {
        parse_header!("Message-ID:", message_id);
    }
    parse_header!("In-Reply-To:", in_reply_to);

    // Find header/body boundary
    let body_start = find_body_offset(raw);

    // Check Content-Type
    let ct = headparse::find_header(&text, "Content-Type:")
        .map(|spec| headparse::get_value_unfolded(&text, &spec));

    if let Some(ref ct_val) = ct {
        if ct_val.to_ascii_lowercase().starts_with("multipart/") {
            if let Some(boundary) = extract_param(ct_val, "boundary") {
                if let Some(boff) = body_start {
                    parse_multipart(&raw[boff..], &boundary, &mut out.parts);
                }
            }
        } else if let Some(boff) = body_start {
            if boff < raw.len() {
                let part = parse_one_part(raw, raw.len());
                out.parts.push(part);
            }
        }
    } else if let Some(boff) = body_start {
        // No Content-Type: treat as text/plain
        if boff < raw.len() {
            let part = parse_one_part(raw, raw.len());
            out.parts.push(part);
        }
    }

    // Set convenience indices
    for (i, part) in out.parts.iter().enumerate() {
        if !part.is_attachment {
            if out.body_plain_index.is_none()
                && part.mime_type.eq_ignore_ascii_case("text/plain")
            {
                out.body_plain_index = Some(i);
            }
            if out.body_html_index.is_none()
                && part.mime_type.eq_ignore_ascii_case("text/html")
            {
                out.body_html_index = Some(i);
            }
        }
    }

    Some(out)
}

/// Find the byte offset where the body starts (after blank line).
fn find_body_offset(data: &[u8]) -> Option<usize> {
    let len = data.len();
    let mut i = 0;
    while i < len.saturating_sub(1) {
        if data[i] == b'\r' && i + 3 < len && data[i + 1] == b'\n' && data[i + 2] == b'\r' && data[i + 3] == b'\n'
        {
            return Some(i + 4);
        }
        if data[i] == b'\n' && data[i + 1] == b'\n' {
            return Some(i + 2);
        }
        i += 1;
    }
    None
}

/// Parse a single MIME part (headers + body) into a ParsedPart.
fn parse_one_part(part_data: &[u8], _part_len: usize) -> ParsedPart {
    let text = String::from_utf8_lossy(part_data);
    let body_off = find_body_offset(part_data);
    let head_end = body_off.unwrap_or(part_data.len());
    let headers = &text[..std::cmp::min(head_end, text.len())];
    let body_bytes = match body_off {
        Some(off) => &part_data[off..],
        None => part_data,
    };

    // Parse Content-Type
    let ct_spec = headparse::find_header(headers, "Content-Type:");
    let ct_val = ct_spec
        .as_ref()
        .map(|s| headparse::get_value_unfolded(headers, s));

    let (mime_type_str, charset, mut filename) = if let Some(ref ct) = ct_val {
        let mt = extract_mime_type(ct);
        let cs = extract_param(ct, "charset");
        let nm = extract_param(ct, "name");
        (mt, cs, nm)
    } else {
        ("text/plain".to_string(), None, None)
    };

    let mut is_attachment = filename.is_some();

    // Parse Content-Transfer-Encoding
    let cte = headparse::find_header(headers, "Content-Transfer-Encoding:")
        .map(|s| headparse::get_value_unfolded(headers, &s));

    // Parse Content-Disposition
    let cd = headparse::find_header(headers, "Content-Disposition:")
        .map(|s| headparse::get_value_unfolded(headers, &s));
    if let Some(ref cd_val) = cd {
        if cd_val.to_ascii_lowercase().starts_with("attachment") {
            is_attachment = true;
            if filename.is_none() {
                filename = extract_param(cd_val, "filename");
            }
        }
    }

    // Parse Content-ID
    let content_id = headparse::find_header(headers, "Content-ID:")
        .map(|s| headparse::get_value_unfolded(headers, &s))
        .map(|v| {
            let v = v.trim();
            if v.starts_with('<') && v.ends_with('>') {
                v[1..v.len() - 1].to_string()
            } else {
                v.to_string()
            }
        });

    // Decode body
    let body_str = String::from_utf8_lossy(body_bytes);
    let mut decoded = if let Some(ref cte_val) = cte {
        let cte_lower = cte_val.trim().to_ascii_lowercase();
        if cte_lower == "base64" {
            match base64::decode(body_str.trim()) {
                Ok(d) => d,
                Err(_) => body_bytes.to_vec(),
            }
        } else if cte_lower == "quoted-printable" {
            encode::qp_decode(&body_str)
        } else {
            body_bytes.to_vec()
        }
    } else {
        body_bytes.to_vec()
    };

    let mut final_mime = mime_type_str.clone();

    // Convert charset to UTF-8 for text parts
    if final_mime.to_ascii_lowercase().starts_with("text/") {
        if let Some(ref cs) = charset {
            let cs_upper = cs.to_ascii_uppercase();
            if cs_upper != "UTF-8" && cs_upper != "US-ASCII" {
                if let Some(utf8) = encode::charset_to_utf8(&decoded, cs) {
                    decoded = utf8.into_bytes();
                }
            }
        }

        // Convert text/enriched and text/richtext to plain text
        let ml = final_mime.to_ascii_lowercase();
        if ml == "text/enriched" || ml == "text/richtext" {
            if let Ok(s) = std::str::from_utf8(&decoded) {
                let plain = richtext::enriched_to_plain(s);
                decoded = plain.into_bytes();
                final_mime = "text/plain".to_string();
            }
        }
    }

    ParsedPart {
        mime_type: final_mime,
        charset,
        filename,
        content_id,
        data: decoded,
        is_attachment,
    }
}

/// Extract just the MIME type from a Content-Type value
/// (e.g. "text/plain" from "text/plain; charset=UTF-8").
fn extract_mime_type(ct: &str) -> String {
    let ct = ct.trim();
    if let Some(semi) = ct.find(';') {
        ct[..semi].trim().to_ascii_lowercase()
    } else {
        ct.trim()
            .trim_end_matches(|c: char| c.is_whitespace())
            .to_ascii_lowercase()
    }
}

/// Extract a parameter from a header value string.
/// e.g. extract_param("multipart/mixed; boundary=\"abc\"", "boundary") => Some("abc")
fn extract_param(value: &str, param: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let param_lower = param.to_ascii_lowercase();
    let mut search_from = 0;

    while let Some(pos) = lower[search_from..].find(&param_lower) {
        let abs_pos = search_from + pos + param_lower.len();
        let rest = &value[abs_pos..];
        let rest = rest.trim_start();
        if !rest.starts_with('=') {
            search_from = abs_pos;
            continue;
        }
        let rest = rest[1..].trim_start();
        if rest.starts_with('"') {
            let inner = &rest[1..];
            if let Some(end) = inner.find('"') {
                return Some(inner[..end].to_string());
            }
            return None;
        } else {
            let end = rest
                .find(|c: char| c == ';' || c == ' ' || c == '\t' || c == '\r' || c == '\n')
                .unwrap_or(rest.len());
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// Recursively parse multipart MIME, appending parts to the vec.
fn parse_multipart(body: &[u8], boundary: &str, parts: &mut Vec<ParsedPart>) {
    let text = String::from_utf8_lossy(body);
    let delim = format!("--{}", boundary);
    let end_delim = format!("--{}--", boundary);

    // Find all boundary positions and extract sections between them
    let mut boundary_positions: Vec<usize> = Vec::new();
    let mut is_final: Vec<bool> = Vec::new();

    for line_start in LineStarts::new(&text) {
        let remaining = &text[line_start..];
        if remaining.starts_with(&end_delim) {
            boundary_positions.push(line_start);
            is_final.push(true);
        } else if remaining.starts_with(&delim) {
            boundary_positions.push(line_start);
            is_final.push(false);
        }
    }

    // Each part is between consecutive boundary lines
    for i in 0..boundary_positions.len() {
        if is_final[i] {
            break;
        }
        // Skip to end of boundary line
        let bpos = boundary_positions[i];
        let line_end = text[bpos..]
            .find('\n')
            .map(|p| bpos + p + 1)
            .unwrap_or(text.len());

        let part_end = if i + 1 < boundary_positions.len() {
            boundary_positions[i + 1]
        } else {
            text.len()
        };

        if line_end >= part_end {
            continue;
        }

        let part_text = &text[line_end..part_end];
        // Strip trailing \r\n before boundary
        let part_text = part_text.trim_end_matches(|c: char| c == '\r' || c == '\n');
        if part_text.is_empty() {
            continue;
        }

        let part_bytes = part_text.as_bytes();

        // Check if this part is itself multipart
        let part_str = String::from_utf8_lossy(part_bytes);
        let sub_ct = headparse::find_header(&part_str, "Content-Type:")
            .map(|s| headparse::get_value_unfolded(&part_str, &s));

        if let Some(ref ct_val) = sub_ct {
            let ct_lower = ct_val.to_ascii_lowercase();
            if ct_lower.starts_with("multipart/") {
                if let Some(sub_boundary) = extract_param(ct_val, "boundary") {
                    if let Some(sub_body_off) = find_body_offset(part_bytes) {
                        parse_multipart(&part_bytes[sub_body_off..], &sub_boundary, parts);
                        continue;
                    }
                }
            }
            if ct_lower.starts_with("message/rfc822") {
                if let Some(nested_off) = find_body_offset(part_bytes) {
                    let nested_data = &part_bytes[nested_off..];
                    // Check if nested message is multipart
                    let nested_str = String::from_utf8_lossy(nested_data);
                    let nested_ct = headparse::find_header(&nested_str, "Content-Type:")
                        .map(|s| headparse::get_value_unfolded(&nested_str, &s));

                    if let Some(ref nct) = nested_ct {
                        if nct.to_ascii_lowercase().starts_with("multipart/") {
                            if let Some(nb) = extract_param(nct, "boundary") {
                                if let Some(nb_off) = find_body_offset(nested_data) {
                                    parse_multipart(&nested_data[nb_off..], &nb, parts);
                                    continue;
                                }
                            }
                        }
                    }

                    // Store as a single message/rfc822 part
                    parts.push(ParsedPart {
                        mime_type: "message/rfc822".to_string(),
                        charset: None,
                        filename: Some("message.eml".to_string()),
                        content_id: None,
                        data: nested_data.to_vec(),
                        is_attachment: true,
                    });
                    continue;
                }
            }
        }

        // Regular part
        let part = parse_one_part(part_bytes, part_bytes.len());
        parts.push(part);
    }
}

/// Iterator that yields byte offsets of the start of each line.
struct LineStarts<'a> {
    text: &'a str,
    pos: usize,
    first: bool,
}

impl<'a> LineStarts<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            pos: 0,
            first: true,
        }
    }
}

impl<'a> Iterator for LineStarts<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if self.first {
            self.first = false;
            if self.pos < self.text.len() {
                return Some(0);
            }
            return None;
        }
        // Find next newline
        let remaining = &self.text[self.pos..];
        if let Some(nl) = remaining.find('\n') {
            self.pos = self.pos + nl + 1;
            if self.pos < self.text.len() {
                Some(self.pos)
            } else {
                None
            }
        } else {
            None
        }
    }
}

// ============================================================
// Save attachments
// ============================================================

/// Save all attachments from a parsed message to a directory.
/// Creates the directory if needed. Returns the number of attachments saved.
pub fn save_attachments(parsed: &ParsedMessage, dir: &str) -> usize {
    let dir_path = Path::new(dir);
    if !dir_path.exists() {
        let _ = fs::create_dir_all(dir_path);
    }

    let mut saved = 0;
    for part in &parsed.parts {
        if !part.is_attachment || part.data.is_empty() {
            continue;
        }

        let fname = part.filename.as_deref().unwrap_or("attachment");
        let mut path = dir_path.join(fname);

        // Avoid overwriting
        if path.exists() {
            let stem = Path::new(fname)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("attachment");
            let ext = Path::new(fname)
                .extension()
                .and_then(|s| s.to_str())
                .map(|e| format!(".{}", e))
                .unwrap_or_default();

            for n in 1..1000 {
                let candidate = dir_path.join(format!("{}_{}{}", stem, n, ext));
                if !candidate.exists() {
                    path = candidate;
                    break;
                }
            }
        }

        if fs::write(&path, &part.data).is_ok() {
            saved += 1;
        }
    }
    saved
}

// ============================================================
// Reply builder
// ============================================================

/// Build a reply message from a parsed original.
///
/// `reply_all`: if true, includes Cc recipients.
/// `body`: new reply text to prepend (can be empty).
pub fn build_reply(
    orig: &ParsedMessage,
    my_from: &str,
    reply_all: bool,
    body: &str,
) -> Option<String> {
    // Build Re: subject
    let subject = match orig.subject.as_deref() {
        Some(s) if !s.is_empty() => {
            if s.to_ascii_lowercase().starts_with("re:") {
                s.to_string()
            } else {
                format!("Re: {}", s)
            }
        }
        _ => "Re: ".to_string(),
    };

    // Build quoted body
    let mut quoted = String::new();
    if let Some(plain) = orig.body_plain() {
        if !plain.is_empty() {
            if let Some(ref from) = orig.from {
                let date_str = orig.date.as_deref().unwrap_or("");
                let _ = write!(quoted, "On {}, {} wrote:\r\n", date_str, from);
            }
            quoted.push_str("\r\n");
            for line in plain.split('\n') {
                let line = line.trim_end_matches('\r');
                let _ = write!(quoted, "> {}\r\n", line);
            }
        }
    }

    // Combine user body + quoted text
    let full_body = if !body.is_empty() && !quoted.is_empty() {
        format!("{}\r\n\r\n{}", body, quoted)
    } else if !quoted.is_empty() {
        quoted
    } else {
        body.to_string()
    };

    let msg = Message {
        from: my_from.to_string(),
        to: orig.from.clone().unwrap_or_default(),
        cc: if reply_all { orig.cc.clone() } else { None },
        subject,
        body_plain: Some(full_body),
        in_reply_to: orig.message_id.clone(),
        references: orig.message_id.clone(),
        ..Default::default()
    };

    msg.build()
}

// ============================================================
// Forward builder
// ============================================================

/// Build a forward message from a parsed original.
///
/// `body`: new text to prepend (can be empty).
/// Includes original attachments.
pub fn build_forward(
    orig: &ParsedMessage,
    my_from: &str,
    to: &str,
    body: &str,
) -> Option<String> {
    // Build Fwd: subject
    let subject = match orig.subject.as_deref() {
        Some(s) if !s.is_empty() => format!("Fwd: {}", s),
        _ => "Fwd: ".to_string(),
    };

    // Build forwarded body
    let mut full_body = String::new();
    if !body.is_empty() {
        let _ = write!(full_body, "{}\r\n\r\n", body);
    }
    full_body.push_str("---------- Forwarded message ----------\r\n");
    if let Some(ref from) = orig.from {
        let _ = write!(full_body, "From: {}\r\n", from);
    }
    if let Some(ref date) = orig.date {
        let _ = write!(full_body, "Date: {}\r\n", date);
    }
    if let Some(ref subj) = orig.subject {
        let _ = write!(full_body, "Subject: {}\r\n", subj);
    }
    if let Some(ref to_orig) = orig.to {
        let _ = write!(full_body, "To: {}\r\n", to_orig);
    }
    full_body.push_str("\r\n");
    if let Some(plain) = orig.body_plain() {
        full_body.push_str(plain);
    }

    // Collect original attachments
    let attachments: Vec<Attachment> = orig
        .parts
        .iter()
        .filter(|p| p.is_attachment && !p.data.is_empty())
        .map(|p| Attachment {
            data: Some(p.data.clone()),
            filename: p.filename.clone(),
            mime_type: Some(p.mime_type.clone()),
            ..Default::default()
        })
        .collect();

    let msg = Message {
        from: my_from.to_string(),
        to: to.to_string(),
        subject,
        body_plain: Some(full_body),
        attachments,
        ..Default::default()
    };

    msg.build()
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- MIME type detection ----

    #[test]
    fn test_mime_type_detection() {
        assert_eq!(mime_type("photo.png"), "image/png");
        assert_eq!(mime_type("photo.PNG"), "image/png");
        assert_eq!(mime_type("doc.pdf"), "application/pdf");
        assert_eq!(mime_type("page.html"), "text/html");
        assert_eq!(mime_type("page.htm"), "text/html");
        assert_eq!(mime_type("archive.zip"), "application/zip");
        assert_eq!(mime_type("song.mp3"), "audio/mpeg");
        assert_eq!(mime_type("video.mp4"), "video/mp4");
        assert_eq!(mime_type("noext"), "application/octet-stream");
        assert_eq!(mime_type("weird.xyz"), "application/octet-stream");
        assert_eq!(mime_type("mail.eml"), "message/rfc822");
    }

    // ---- Boundary and Message-ID generation ----

    #[test]
    fn test_boundary_generation() {
        let b1 = generate_boundary();
        let b2 = generate_boundary();
        assert!(b1.starts_with("----=_CrispyBoundary_"));
        // They should be different (timing + random)
        // (In rare cases they might collide, but practically never)
        assert!(b1.len() > 20);
        let _ = b2; // just ensure it doesn't panic
    }

    #[test]
    fn test_message_id_generation() {
        let mid = generate_message_id("example.com");
        assert!(mid.starts_with('<'));
        assert!(mid.ends_with('>'));
        assert!(mid.contains("@example.com"));
    }

    #[test]
    fn test_message_id_default_domain() {
        let mid = generate_message_id("");
        assert!(mid.contains("@crispy.local"));
    }

    // ---- Date formatting ----

    #[test]
    fn test_format_date() {
        // 2024-01-15 12:30:00 UTC = 1705318200
        let date = format_date(1705318200, 0);
        assert!(date.contains("2024"));
        assert!(date.contains("Jan"));
        assert!(date.contains("+0000"));
    }

    #[test]
    fn test_format_date_with_offset() {
        let date = format_date(1705318200, -18000); // EST = UTC-5
        assert!(date.contains("-0500"));
    }

    // ---- Address helpers ----

    #[test]
    fn test_extract_bare_address() {
        assert_eq!(extract_bare_address("user@example.com"), "user@example.com");
        assert_eq!(
            extract_bare_address("John Doe <john@example.com>"),
            "john@example.com"
        );
        assert_eq!(
            extract_bare_address("  spaced@example.com  "),
            "spaced@example.com"
        );
    }

    #[test]
    fn test_extract_domain() {
        assert_eq!(extract_domain("user@example.com"), "example.com");
        assert_eq!(
            extract_domain("John <john@mail.example.com>"),
            "mail.example.com"
        );
        assert_eq!(extract_domain("noat"), "localhost");
    }

    #[test]
    fn test_split_addresses() {
        let mut out = Vec::new();
        split_addresses("a@b.com, c@d.com, \"Name\" <e@f.com>", &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], "a@b.com");
        assert_eq!(out[1], "c@d.com");
        assert_eq!(out[2], "e@f.com");
    }

    #[test]
    fn test_split_addresses_angle_brackets() {
        let mut out = Vec::new();
        split_addresses("Alice <a@b.com>, Bob <c@d.com>", &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], "a@b.com");
        assert_eq!(out[1], "c@d.com");
    }

    // ---- Simple plain text message build ----

    #[test]
    fn test_build_plain_text() {
        let msg = Message {
            from: "sender@example.com".into(),
            to: "recipient@example.com".into(),
            subject: "Hello".into(),
            body_plain: Some("Hello world!".into()),
            message_id: Some("<test123@example.com>".into()),
            ..Default::default()
        };

        let raw = msg.build().unwrap();
        assert!(raw.contains("From: sender@example.com\r\n"));
        assert!(raw.contains("To: recipient@example.com\r\n"));
        assert!(raw.contains("Subject: Hello\r\n"));
        assert!(raw.contains("Message-ID: <test123@example.com>\r\n"));
        assert!(raw.contains("MIME-Version: 1.0\r\n"));
        assert!(raw.contains("Content-Type: text/plain; charset=UTF-8\r\n"));
        assert!(raw.contains("Hello world!"));
        // Should NOT contain multipart boundaries
        assert!(!raw.contains("boundary"));
    }

    #[test]
    fn test_build_returns_none_without_from() {
        let msg = Message {
            to: "r@example.com".into(),
            ..Default::default()
        };
        assert!(msg.build().is_none());
    }

    #[test]
    fn test_build_returns_none_without_to() {
        let msg = Message {
            from: "s@example.com".into(),
            ..Default::default()
        };
        assert!(msg.build().is_none());
    }

    // ---- Alternative (plain + HTML) ----

    #[test]
    fn test_build_alternative() {
        let msg = Message {
            from: "s@e.com".into(),
            to: "r@e.com".into(),
            subject: "Alt".into(),
            body_plain: Some("Plain text".into()),
            body_html: Some("<p>HTML text</p>".into()),
            message_id: Some("<alt@e.com>".into()),
            ..Default::default()
        };

        let raw = msg.build().unwrap();
        assert!(raw.contains("multipart/alternative"));
        assert!(raw.contains("text/plain"));
        assert!(raw.contains("text/html"));
        assert!(raw.contains("Plain text"));
        assert!(raw.contains("<p>HTML text</p>"));
    }

    // ---- Mixed (attachments) ----

    #[test]
    fn test_build_with_attachment() {
        let msg = Message {
            from: "s@e.com".into(),
            to: "r@e.com".into(),
            subject: "Attached".into(),
            body_plain: Some("See attached.".into()),
            attachments: vec![Attachment::from_data(
                b"file content here".to_vec(),
                "test.txt",
            )],
            message_id: Some("<att@e.com>".into()),
            ..Default::default()
        };

        let raw = msg.build().unwrap();
        assert!(raw.contains("multipart/mixed"));
        assert!(raw.contains("See attached."));
        assert!(raw.contains("Content-Disposition: attachment; filename=\"test.txt\""));
        assert!(raw.contains("Content-Transfer-Encoding: base64"));
    }

    // ---- Mixed with alternative + attachments ----

    #[test]
    fn test_build_mixed_alternative_attachments() {
        let msg = Message {
            from: "s@e.com".into(),
            to: "r@e.com".into(),
            subject: "Full".into(),
            body_plain: Some("Plain body".into()),
            body_html: Some("<b>HTML body</b>".into()),
            attachments: vec![Attachment::from_data(
                vec![0u8; 10],
                "data.bin",
            )],
            message_id: Some("<full@e.com>".into()),
            ..Default::default()
        };

        let raw = msg.build().unwrap();
        assert!(raw.contains("multipart/mixed"));
        assert!(raw.contains("multipart/alternative"));
        assert!(raw.contains("Plain body"));
        assert!(raw.contains("<b>HTML body</b>"));
        assert!(raw.contains("data.bin"));
    }

    // ---- Inline attachment ----

    #[test]
    fn test_build_inline_attachment() {
        let msg = Message {
            from: "s@e.com".into(),
            to: "r@e.com".into(),
            subject: "Inline".into(),
            body_plain: Some("See image.".into()),
            body_html: Some("<img src=\"cid:img1\">".into()),
            attachments: vec![Attachment::inline_from_data(
                vec![0x89, 0x50, 0x4E, 0x47],
                "image.png",
                "img1",
            )],
            message_id: Some("<inl@e.com>".into()),
            ..Default::default()
        };

        let raw = msg.build().unwrap();
        assert!(raw.contains("Content-ID: <img1>"));
        assert!(raw.contains("Content-Disposition: inline"));
    }

    // ---- Recipients extraction ----

    #[test]
    fn test_recipients() {
        let msg = Message {
            from: "s@e.com".into(),
            to: "a@b.com, c@d.com".into(),
            cc: Some("e@f.com".into()),
            bcc: Some("hidden@x.com".into()),
            ..Default::default()
        };

        let rcpts = msg.recipients();
        assert_eq!(rcpts.len(), 4);
        assert!(rcpts.contains(&"a@b.com".to_string()));
        assert!(rcpts.contains(&"c@d.com".to_string()));
        assert!(rcpts.contains(&"e@f.com".to_string()));
        assert!(rcpts.contains(&"hidden@x.com".to_string()));
    }

    // ---- Optional headers ----

    #[test]
    fn test_build_optional_headers() {
        let msg = Message {
            from: "s@e.com".into(),
            to: "r@e.com".into(),
            subject: "Headers".into(),
            cc: Some("cc@e.com".into()),
            reply_to: Some("reply@e.com".into()),
            in_reply_to: Some("<orig@e.com>".into()),
            references: Some("<orig@e.com>".into()),
            priority: 1,
            x_mailer: Some("TestMailer".into()),
            message_id: Some("<hdr@e.com>".into()),
            body_plain: Some("test".into()),
            ..Default::default()
        };

        let raw = msg.build().unwrap();
        assert!(raw.contains("Cc: cc@e.com\r\n"));
        assert!(raw.contains("Reply-To: reply@e.com\r\n"));
        assert!(raw.contains("In-Reply-To: <orig@e.com>\r\n"));
        assert!(raw.contains("References: <orig@e.com>\r\n"));
        assert!(raw.contains("X-Priority: 1\r\n"));
        assert!(raw.contains("X-Mailer: TestMailer\r\n"));
    }

    // ---- QP encoding for non-ASCII ----

    #[test]
    fn test_build_qp_for_non_ascii() {
        let msg = Message {
            from: "s@e.com".into(),
            to: "r@e.com".into(),
            subject: "UTF8".into(),
            body_plain: Some("Caf\u{00e9} et cr\u{00e8}me".into()),
            message_id: Some("<qp@e.com>".into()),
            ..Default::default()
        };

        let raw = msg.build().unwrap();
        assert!(raw.contains("Content-Transfer-Encoding: quoted-printable"));
    }

    // ---- BCC not in headers ----

    #[test]
    fn test_bcc_not_in_headers() {
        let msg = Message {
            from: "s@e.com".into(),
            to: "r@e.com".into(),
            subject: "BCC".into(),
            bcc: Some("secret@e.com".into()),
            body_plain: Some("test".into()),
            message_id: Some("<bcc@e.com>".into()),
            ..Default::default()
        };

        let raw = msg.build().unwrap();
        assert!(!raw.contains("Bcc:"));
        assert!(!raw.contains("secret@e.com"));
    }

    // ---- Parse simple plain text ----

    #[test]
    fn test_parse_simple_plain() {
        let raw = b"From: alice@example.com\r\n\
                     To: bob@example.com\r\n\
                     Subject: Hello\r\n\
                     Date: Mon, 15 Jan 2024 12:00:00 +0000\r\n\
                     Message-ID: <abc@example.com>\r\n\
                     Content-Type: text/plain; charset=UTF-8\r\n\
                     Content-Transfer-Encoding: 7bit\r\n\
                     \r\n\
                     Hello Bob!";

        let parsed = parse(raw).unwrap();
        assert_eq!(parsed.from.as_deref(), Some("alice@example.com"));
        assert_eq!(parsed.to.as_deref(), Some("bob@example.com"));
        assert_eq!(parsed.subject.as_deref(), Some("Hello"));
        assert_eq!(parsed.message_id.as_deref(), Some("<abc@example.com>"));
        assert_eq!(parsed.parts.len(), 1);
        assert_eq!(parsed.parts[0].mime_type, "text/plain");
        assert!(parsed.body_plain().unwrap().contains("Hello Bob!"));
    }

    // ---- Parse multipart/alternative ----

    #[test]
    fn test_parse_multipart_alternative() {
        let raw = b"From: alice@example.com\r\n\
To: bob@example.com\r\n\
Subject: Alt Message\r\n\
Content-Type: multipart/alternative; boundary=\"alt_boundary\"\r\n\
\r\n\
--alt_boundary\r\n\
Content-Type: text/plain; charset=UTF-8\r\n\
Content-Transfer-Encoding: 7bit\r\n\
\r\n\
Plain text version\r\n\
--alt_boundary\r\n\
Content-Type: text/html; charset=UTF-8\r\n\
Content-Transfer-Encoding: 7bit\r\n\
\r\n\
<p>HTML version</p>\r\n\
--alt_boundary--\r\n";

        let parsed = parse(raw).unwrap();
        assert_eq!(parsed.parts.len(), 2);
        assert!(parsed.body_plain().unwrap().contains("Plain text version"));
        assert!(parsed.body_html().unwrap().contains("<p>HTML version</p>"));
    }

    // ---- Parse multipart/mixed with attachment ----

    #[test]
    fn test_parse_multipart_mixed_attachment() {
        let raw = b"From: alice@example.com\r\n\
To: bob@example.com\r\n\
Subject: With Attachment\r\n\
Content-Type: multipart/mixed; boundary=\"mixed_bound\"\r\n\
\r\n\
--mixed_bound\r\n\
Content-Type: text/plain; charset=UTF-8\r\n\
Content-Transfer-Encoding: 7bit\r\n\
\r\n\
Body text here\r\n\
--mixed_bound\r\n\
Content-Type: application/octet-stream; name=\"test.bin\"\r\n\
Content-Transfer-Encoding: base64\r\n\
Content-Disposition: attachment; filename=\"test.bin\"\r\n\
\r\n\
AQIDBA==\r\n\
--mixed_bound--\r\n";

        let parsed = parse(raw).unwrap();
        assert_eq!(parsed.parts.len(), 2);
        assert!(parsed.body_plain().unwrap().contains("Body text here"));

        let atts = parsed.attachments();
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].filename.as_deref(), Some("test.bin"));
        assert_eq!(atts[0].data, vec![1, 2, 3, 4]);
    }

    // ---- Parse base64-encoded body ----

    #[test]
    fn test_parse_base64_body() {
        // "Hello World" in base64 = "SGVsbG8gV29ybGQ="
        let raw = b"From: a@b.com\r\n\
Content-Type: text/plain; charset=UTF-8\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
SGVsbG8gV29ybGQ=\r\n";

        let parsed = parse(raw).unwrap();
        assert_eq!(parsed.body_plain().unwrap(), "Hello World");
    }

    // ---- Parse quoted-printable body ----

    #[test]
    fn test_parse_qp_body() {
        let raw = b"From: a@b.com\r\n\
Content-Type: text/plain; charset=UTF-8\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
Caf=C3=A9\r\n";

        let parsed = parse(raw).unwrap();
        let plain = parsed.body_plain().unwrap();
        assert!(plain.contains("Caf\u{00e9}"));
    }

    // ---- Parse nested multipart ----

    #[test]
    fn test_parse_nested_multipart() {
        let raw = b"Content-Type: multipart/mixed; boundary=\"outer\"\r\n\
\r\n\
--outer\r\n\
Content-Type: multipart/alternative; boundary=\"inner\"\r\n\
\r\n\
--inner\r\n\
Content-Type: text/plain\r\n\
\r\n\
Plain\r\n\
--inner\r\n\
Content-Type: text/html\r\n\
\r\n\
<b>HTML</b>\r\n\
--inner--\r\n\
--outer\r\n\
Content-Type: application/pdf; name=\"doc.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
Content-Disposition: attachment; filename=\"doc.pdf\"\r\n\
\r\n\
AAAA\r\n\
--outer--\r\n";

        let parsed = parse(raw).unwrap();
        assert_eq!(parsed.parts.len(), 3); // plain, html, pdf
        assert!(parsed.body_plain().unwrap().contains("Plain"));
        assert!(parsed.body_html().unwrap().contains("<b>HTML</b>"));
        let atts = parsed.attachments();
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].filename.as_deref(), Some("doc.pdf"));
    }

    // ---- Parse with Content-ID (inline) ----

    #[test]
    fn test_parse_content_id() {
        let raw = b"Content-Type: multipart/mixed; boundary=\"bd\"\r\n\
\r\n\
--bd\r\n\
Content-Type: text/html\r\n\
\r\n\
<img src=\"cid:img1\">\r\n\
--bd\r\n\
Content-Type: image/png; name=\"pic.png\"\r\n\
Content-ID: <img1>\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
iVBO\r\n\
--bd--\r\n";

        let parsed = parse(raw).unwrap();
        // Find the inline image part
        let img_part = parsed
            .parts
            .iter()
            .find(|p| p.content_id.as_deref() == Some("img1"));
        assert!(img_part.is_some());
    }

    // ---- Round-trip: build then parse ----

    #[test]
    fn test_round_trip_plain() {
        let msg = Message {
            from: "alice@example.com".into(),
            to: "bob@example.com".into(),
            subject: "Round Trip".into(),
            body_plain: Some("Hello from the round trip test!".into()),
            message_id: Some("<rt@example.com>".into()),
            ..Default::default()
        };

        let raw = msg.build().unwrap();
        let parsed = parse(raw.as_bytes()).unwrap();
        assert_eq!(parsed.from.as_deref(), Some("alice@example.com"));
        assert_eq!(parsed.to.as_deref(), Some("bob@example.com"));
        assert_eq!(parsed.subject.as_deref(), Some("Round Trip"));
        assert!(parsed
            .body_plain()
            .unwrap()
            .contains("Hello from the round trip test!"));
    }

    #[test]
    fn test_round_trip_alternative() {
        let msg = Message {
            from: "alice@example.com".into(),
            to: "bob@example.com".into(),
            subject: "RT Alt".into(),
            body_plain: Some("Plain version".into()),
            body_html: Some("<p>HTML version</p>".into()),
            message_id: Some("<rta@example.com>".into()),
            ..Default::default()
        };

        let raw = msg.build().unwrap();
        let parsed = parse(raw.as_bytes()).unwrap();
        assert!(parsed.body_plain().unwrap().contains("Plain version"));
        assert!(parsed.body_html().unwrap().contains("<p>HTML version</p>"));
    }

    #[test]
    fn test_round_trip_with_attachment() {
        let msg = Message {
            from: "alice@example.com".into(),
            to: "bob@example.com".into(),
            subject: "RT Att".into(),
            body_plain: Some("See file.".into()),
            attachments: vec![Attachment::from_data(
                b"Test file content 12345".to_vec(),
                "test.txt",
            )],
            message_id: Some("<rtatt@example.com>".into()),
            ..Default::default()
        };

        let raw = msg.build().unwrap();
        let parsed = parse(raw.as_bytes()).unwrap();
        assert!(parsed.body_plain().unwrap().contains("See file."));
        let atts = parsed.attachments();
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].filename.as_deref(), Some("test.txt"));
        assert_eq!(atts[0].data, b"Test file content 12345");
    }

    // ---- Reply builder ----

    #[test]
    fn test_build_reply() {
        let orig_raw = b"From: alice@example.com\r\n\
To: bob@example.com\r\n\
Cc: carol@example.com\r\n\
Subject: Original\r\n\
Date: Mon, 15 Jan 2024 12:00:00 +0000\r\n\
Message-ID: <orig@example.com>\r\n\
Content-Type: text/plain\r\n\
\r\n\
Original body text.";

        let orig = parse(orig_raw).unwrap();
        let reply = build_reply(&orig, "bob@example.com", false, "My reply.").unwrap();

        assert!(reply.contains("From: bob@example.com"));
        assert!(reply.contains("To: alice@example.com"));
        assert!(reply.contains("Subject: Re: Original"));
        assert!(reply.contains("In-Reply-To: <orig@example.com>"));
        assert!(reply.contains("My reply."));
        assert!(reply.contains("> Original body text."));
        // Not reply-all, so no Cc
        assert!(!reply.contains("carol@example.com"));
    }

    #[test]
    fn test_build_reply_all() {
        let orig_raw = b"From: alice@example.com\r\n\
To: bob@example.com\r\n\
Cc: carol@example.com\r\n\
Subject: Original\r\n\
Message-ID: <orig@example.com>\r\n\
Content-Type: text/plain\r\n\
\r\n\
Text.";

        let orig = parse(orig_raw).unwrap();
        let reply = build_reply(&orig, "bob@example.com", true, "").unwrap();

        assert!(reply.contains("Cc: carol@example.com"));
    }

    #[test]
    fn test_reply_re_not_doubled() {
        let orig_raw = b"From: a@b.com\r\n\
To: c@d.com\r\n\
Subject: Re: Already replied\r\n\
Content-Type: text/plain\r\n\
\r\n\
Text.";

        let orig = parse(orig_raw).unwrap();
        let reply = build_reply(&orig, "c@d.com", false, "").unwrap();
        // Should NOT add another Re:
        assert!(reply.contains("Subject: Re: Already replied"));
        assert!(!reply.contains("Re: Re:"));
    }

    // ---- Forward builder ----

    #[test]
    fn test_build_forward() {
        let orig_raw = b"From: alice@example.com\r\n\
To: bob@example.com\r\n\
Subject: Important\r\n\
Date: Mon, 15 Jan 2024 12:00:00 +0000\r\n\
Content-Type: text/plain\r\n\
\r\n\
Important message content.";

        let orig = parse(orig_raw).unwrap();
        let fwd =
            build_forward(&orig, "bob@example.com", "dave@example.com", "FYI").unwrap();

        assert!(fwd.contains("From: bob@example.com"));
        assert!(fwd.contains("To: dave@example.com"));
        assert!(fwd.contains("Subject: Fwd: Important"));
        assert!(fwd.contains("FYI"));
        assert!(fwd.contains("---------- Forwarded message ----------"));
        assert!(fwd.contains("Important message content."));
    }

    #[test]
    fn test_forward_with_attachment() {
        let orig_raw = b"Content-Type: multipart/mixed; boundary=\"fwdbd\"\r\n\
From: alice@example.com\r\n\
To: bob@example.com\r\n\
Subject: File\r\n\
\r\n\
--fwdbd\r\n\
Content-Type: text/plain\r\n\
\r\n\
See attached.\r\n\
--fwdbd\r\n\
Content-Type: application/pdf; name=\"doc.pdf\"\r\n\
Content-Disposition: attachment; filename=\"doc.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
AQIDBA==\r\n\
--fwdbd--\r\n";

        let orig = parse(orig_raw).unwrap();
        let fwd = build_forward(&orig, "bob@e.com", "carol@e.com", "").unwrap();

        // Forward should include the attachment
        assert!(fwd.contains("multipart/mixed"));
        assert!(fwd.contains("doc.pdf"));
    }

    // ---- Extract param ----

    #[test]
    fn test_extract_param_quoted() {
        let val = "multipart/mixed; boundary=\"abc123\"";
        assert_eq!(extract_param(val, "boundary"), Some("abc123".into()));
    }

    #[test]
    fn test_extract_param_unquoted() {
        let val = "multipart/mixed; boundary=abc123";
        assert_eq!(extract_param(val, "boundary"), Some("abc123".into()));
    }

    #[test]
    fn test_extract_param_charset() {
        let val = "text/plain; charset=ISO-8859-1";
        assert_eq!(extract_param(val, "charset"), Some("ISO-8859-1".into()));
    }

    #[test]
    fn test_extract_param_missing() {
        let val = "text/plain";
        assert_eq!(extract_param(val, "charset"), None);
    }

    #[test]
    fn test_extract_param_case_insensitive() {
        let val = "text/plain; Charset=\"UTF-8\"";
        assert_eq!(extract_param(val, "charset"), Some("UTF-8".into()));
    }

    // ---- needs_encoding ----

    #[test]
    fn test_needs_encoding_ascii() {
        assert!(!needs_encoding("Hello world\nAnother line\n"));
    }

    #[test]
    fn test_needs_encoding_non_ascii() {
        assert!(needs_encoding("Caf\u{00e9}"));
    }

    #[test]
    fn test_needs_encoding_long_line() {
        let long_line = "a".repeat(80);
        assert!(needs_encoding(&long_line));
    }

    // ---- Parse edge cases ----

    #[test]
    fn test_parse_no_content_type() {
        let raw = b"From: a@b.com\r\n\
To: c@d.com\r\n\
Subject: No CT\r\n\
\r\n\
Body without content-type header.";

        let parsed = parse(raw).unwrap();
        assert!(parsed.body_plain().is_some());
        assert!(parsed
            .body_plain()
            .unwrap()
            .contains("Body without content-type header."));
    }

    #[test]
    fn test_parse_empty() {
        assert!(parse(b"").is_none());
    }

    // ---- Line starts iterator ----

    #[test]
    fn test_line_starts() {
        let text = "line1\nline2\nline3\n";
        let starts: Vec<usize> = LineStarts::new(text).collect();
        assert_eq!(starts, vec![0, 6, 12]);
    }

    // ---- ParsedPart helpers ----

    #[test]
    fn test_parsed_part_as_text() {
        let part = ParsedPart {
            mime_type: "text/plain".into(),
            charset: None,
            filename: None,
            content_id: None,
            data: b"Hello".to_vec(),
            is_attachment: false,
        };
        assert_eq!(part.as_text(), Some("Hello"));
    }

    #[test]
    fn test_parsed_part_as_text_binary() {
        let part = ParsedPart {
            mime_type: "image/png".into(),
            charset: None,
            filename: None,
            content_id: None,
            data: vec![0xFF, 0xFE, 0x00],
            is_attachment: true,
        };
        assert!(part.as_text().is_none());
    }

    // ---- Attachment helpers ----

    #[test]
    fn test_attachment_resolved_filename_from_data() {
        let att = Attachment::from_data(vec![], "report.pdf");
        assert_eq!(att.resolved_filename(), "report.pdf");
    }

    #[test]
    fn test_attachment_resolved_filename_from_path() {
        let att = Attachment::from_path("/home/user/docs/report.pdf");
        assert_eq!(att.resolved_filename(), "report.pdf");
    }

    #[test]
    fn test_attachment_resolved_mime_type() {
        let att = Attachment::from_data(vec![], "image.png");
        assert_eq!(att.resolved_mime_type(), "image/png");
    }

    #[test]
    fn test_attachment_explicit_mime_type() {
        let mut att = Attachment::from_data(vec![], "file.bin");
        att.mime_type = Some("application/x-custom".into());
        assert_eq!(att.resolved_mime_type(), "application/x-custom");
    }

    // ---- days_to_ymd ----

    #[test]
    fn test_days_to_ymd_epoch() {
        // Day 0 = 1970-01-01
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn test_days_to_ymd_known_date() {
        // 2024-01-15 = day 19737
        let (y, m, d) = days_to_ymd(19737);
        assert_eq!(y, 2024);
        assert_eq!(m, 1);
        assert_eq!(d, 15);
    }
}
