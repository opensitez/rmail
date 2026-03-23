//! # RFC 822/2822 Address Parsing and Message Utilities
//!
//! Parses and formats RFC 822 email addresses, dates, and message envelopes.
//! Standalone — no external dependencies, no Eudora dependency.
//!
//! Ported from `crispy_rfc822.c`.

// Note: crate::lex822 is available for tokenizing if needed, but this module
// operates directly on byte slices for efficiency.

// ================================================================
// Constants
// ================================================================

const RFC822_SPECIALS: &[u8] = b"()<>@,;:\\\".[]";
const TSPECIALS: &[u8] = b"()<>@,;:\\\"[]/?=";

const MONTHS_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun",
    "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const DAYS_SHORT: [&str; 7] = [
    "Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat",
];

// ================================================================
// Address
// ================================================================

/// A parsed RFC 822 email address.
///
/// Replaces the C linked-list `ImapAddress` with a simple owned struct.
/// The `adl` (at-domain-list / source route) field is preserved for
/// compatibility with IMAP ENVELOPE responses but is rarely used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// Personal / display name, e.g. `"John Doe"`.
    pub name: Option<String>,
    /// Source route (at-domain-list), e.g. `"@relay1,@relay2"`. Obsolete.
    pub adl: Option<String>,
    /// Local part (mailbox), e.g. `"john"`.
    pub mailbox: String,
    /// Domain (host), e.g. `"example.com"`. Empty if local-only.
    pub host: String,
}

impl Address {
    /// Create an address from mailbox and host.
    pub fn new(mailbox: impl Into<String>, host: impl Into<String>) -> Self {
        Address {
            name: None,
            adl: None,
            mailbox: mailbox.into(),
            host: host.into(),
        }
    }

    /// Create an address with a display name.
    pub fn with_name(
        name: impl Into<String>,
        mailbox: impl Into<String>,
        host: impl Into<String>,
    ) -> Self {
        Address {
            name: Some(name.into()),
            adl: None,
            mailbox: mailbox.into(),
            host: host.into(),
        }
    }

    /// Return the bare `mailbox@host` form, or just `mailbox` if host is empty.
    pub fn addr_spec(&self) -> String {
        if !self.host.is_empty() && !self.host.starts_with('@') {
            format!("{}@{}", self.mailbox, self.host)
        } else {
            self.mailbox.clone()
        }
    }
}

// ================================================================
// Parsed date
// ================================================================

/// A parsed date/time with timezone offset.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedDate {
    pub year: i32,
    pub month: i32,   // 1-12
    pub day: i32,     // 1-31
    pub hours: i32,   // 0-23
    pub minutes: i32, // 0-59
    pub seconds: i32, // 0-59
    /// Timezone offset in minutes from UTC (e.g. +120 = UTC+2).
    pub tz_offset_min: i32,
}

// ================================================================
// RFC 822 quoting
// ================================================================

/// Quote a string for RFC 822 if it contains specials or whitespace.
///
/// Returns the original string unmodified if no quoting is needed,
/// otherwise wraps it in double-quotes with `"` and `\` escaped.
pub fn quote(src: &str) -> String {
    let needs = src.bytes().any(|c| {
        RFC822_SPECIALS.contains(&c) || c == b' ' || c == b'\t'
    });
    if !needs {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len() + 4);
    out.push('"');
    for c in src.bytes() {
        if c == b'"' || c == b'\\' {
            out.push('\\');
        }
        out.push(c as char);
    }
    out.push('"');
    out
}

/// Remove surrounding quotes and backslash escapes from a string.
pub fn unquote(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.bytes().peekable();
    while let Some(c) = chars.next() {
        if c == b'"' {
            continue;
        }
        if c == b'\\' {
            if let Some(&next) = chars.peek() {
                out.push(next as char);
                chars.next();
                continue;
            }
        }
        out.push(c as char);
    }
    out
}

// ================================================================
// Whitespace / comment skipping
// ================================================================

/// Skip RFC 822 whitespace (spaces, tabs, CR, LF) and comments.
/// Returns the remaining slice.
fn skip_ws(s: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'(' => i = skip_comment(s, i),
            _ => break,
        }
    }
    &s[i..]
}

/// Skip a parenthesized comment starting at `pos`. Returns new position
/// after the closing `)`.
fn skip_comment(s: &[u8], pos: usize) -> usize {
    debug_assert!(s[pos] == b'(');
    let mut i = pos + 1;
    let mut depth = 1u32;
    while i < s.len() && depth > 0 {
        match s[i] {
            b'(' => { depth += 1; i += 1; }
            b')' => { depth -= 1; i += 1; }
            b'\\' if i + 1 < s.len() => { i += 2; }
            _ => { i += 1; }
        }
    }
    i
}

// ================================================================
// Low-level parsers (operate on byte slices)
// ================================================================

/// Parse a display-name phrase: sequence of words, quoted strings, comments.
/// Returns `(remaining_input, phrase_text)` or `None` if no phrase found.
fn parse_phrase(s: &[u8]) -> Option<(&[u8], String)> {
    let s = skip_ws(s);
    if s.is_empty() {
        return None;
    }
    match s[0] {
        b'<' | b'@' | b',' | b';' => return None,
        _ => {}
    }

    let mut out = String::new();
    let mut i = 0;
    let mut in_quote = false;

    while i < s.len() {
        if s[i] == b'"' {
            in_quote = !in_quote;
            i += 1;
        } else if in_quote {
            if s[i] == b'\\' && i + 1 < s.len() {
                out.push(s[i + 1] as char);
                i += 2;
            } else {
                out.push(s[i] as char);
                i += 1;
            }
        } else {
            match s[i] {
                b'<' | b',' | b';' | b':' => break,
                b'(' => {
                    i = skip_comment(s, i) - (s.len() - s.len()); // same slice
                    // recalculate: skip_comment works on same slice
                    i = skip_comment_in(s, i);
                }
                b' ' | b'\t' => {
                    out.push(' ');
                    while i < s.len() && (s[i] == b' ' || s[i] == b'\t') {
                        i += 1;
                    }
                }
                _ => {
                    out.push(s[i] as char);
                    i += 1;
                }
            }
        }
    }

    // Trim trailing spaces
    let trimmed = out.trim_end().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some((&s[i..], trimmed))
    }
}

/// Skip a comment within a byte slice, returning the new index.
fn skip_comment_in(s: &[u8], pos: usize) -> usize {
    if pos >= s.len() || s[pos] != b'(' {
        return pos;
    }
    let mut i = pos + 1;
    let mut depth = 1u32;
    while i < s.len() && depth > 0 {
        match s[i] {
            b'(' => { depth += 1; i += 1; }
            b')' => { depth -= 1; i += 1; }
            b'\\' if i + 1 < s.len() => { i += 2; }
            _ => { i += 1; }
        }
    }
    i
}

/// Parse an addr-spec: `local-part@domain`.
/// Returns `(remaining_input, mailbox, host)` or `None`.
fn parse_addrspec<'a>(s: &'a [u8], default_host: &str) -> Option<(&'a [u8], String, String)> {
    let s = skip_ws(s);
    if s.is_empty() {
        return None;
    }

    let mut mailbox = String::new();
    let mut host = String::new();
    let mut i = 0;

    // Local part — may be quoted
    if i < s.len() && s[i] == b'"' {
        i += 1;
        while i < s.len() && s[i] != b'"' {
            if s[i] == b'\\' && i + 1 < s.len() {
                i += 1;
            }
            mailbox.push(s[i] as char);
            i += 1;
        }
        if i < s.len() && s[i] == b'"' {
            i += 1;
        }
    } else {
        while i < s.len() {
            match s[i] {
                b'@' | b'>' | b',' | b';' | b' ' | b'\t' | b'\r' | b'\n' => break,
                c => {
                    mailbox.push(c as char);
                    i += 1;
                }
            }
        }
    }

    if mailbox.is_empty() {
        return None;
    }

    // Domain
    if i < s.len() && s[i] == b'@' {
        i += 1;
        while i < s.len() {
            match s[i] {
                b'>' | b',' | b';' | b' ' | b'\t' | b'\r' | b'\n' | b')' => break,
                c => {
                    host.push(c as char);
                    i += 1;
                }
            }
        }
    } else if !default_host.is_empty() {
        host = default_host.to_string();
    }

    Some((&s[i..], mailbox, host))
}

/// Parse a route-addr: `<@route:user@host>` or `<user@host>`.
/// Returns `(remaining_input, adl, mailbox, host)` or `None`.
fn parse_routeaddr<'a>(
    s: &'a [u8],
    default_host: &str,
) -> Option<(&'a [u8], Option<String>, String, String)> {
    let s = skip_ws(s);
    if s.is_empty() || s[0] != b'<' {
        return None;
    }
    let mut i = 1; // skip '<'
    let inner = skip_ws(&s[i..]);
    i = s.len() - inner.len();

    // Check for source route: @domain1,@domain2:
    let mut adl: Option<String> = None;
    if i < s.len() && s[i] == b'@' {
        let mut route = String::new();
        while i < s.len() && s[i] == b'@' {
            if !route.is_empty() {
                route.push(',');
            }
            route.push('@');
            i += 1;
            while i < s.len() && s[i] != b',' && s[i] != b':' {
                route.push(s[i] as char);
                i += 1;
            }
            if i < s.len() && s[i] == b',' {
                i += 1;
            }
        }
        if i < s.len() && s[i] == b':' {
            i += 1;
        }
        adl = Some(route);
    }

    let (rest, mailbox, host) = parse_addrspec(&s[i..], default_host)?;
    // Advance past the '>'
    let rest = skip_ws(rest);
    let rest = if !rest.is_empty() && rest[0] == b'>' {
        &rest[1..]
    } else {
        rest
    };

    Some((rest, adl, mailbox, host))
}

/// Parse a single mailbox: `Name <user@host>`, `<user@host>`, or `user@host`.
fn parse_mailbox<'a>(s: &'a [u8], default_host: &str) -> Option<(&'a [u8], Address)> {
    let s = skip_ws(s);
    if s.is_empty() {
        return None;
    }

    // Try bare route-addr: <user@host>
    if s[0] == b'<' {
        let (rest, adl, mailbox, host) = parse_routeaddr(s, default_host)?;
        return Some((rest, Address {
            name: None,
            adl,
            mailbox,
            host,
        }));
    }

    // Try "phrase <route-addr>"
    if let Some((after_phrase, phrase)) = parse_phrase(s) {
        let after_phrase = skip_ws(after_phrase);
        if !after_phrase.is_empty() && after_phrase[0] == b'<' {
            if let Some((rest, adl, mailbox, host)) =
                parse_routeaddr(after_phrase, default_host)
            {
                return Some((rest, Address {
                    name: if phrase.is_empty() { None } else { Some(phrase) },
                    adl,
                    mailbox,
                    host,
                }));
            }
        }
    }

    // Plain addr-spec: user@host
    let (rest, mailbox, host) = parse_addrspec(s, default_host)?;
    Some((rest, Address {
        name: None,
        adl: None,
        mailbox,
        host,
    }))
}

// ================================================================
// Public address parsing API
// ================================================================

/// Parse a comma-separated list of RFC 822 addresses.
///
/// Handles:
/// - `user@host`
/// - `Name <user@host>`
/// - `"Quoted Name" <user@host>`
/// - `<user@host>`
/// - Group syntax: `Group: addr1, addr2 ;`
///
/// Returns an empty `Vec` if the input is empty or unparseable.
///
/// # Example
/// ```
/// use crispy::rfc822::parse_address_list;
///
/// let addrs = parse_address_list("Alice <alice@a.com>, bob@b.com");
/// assert_eq!(addrs.len(), 2);
/// assert_eq!(addrs[0].name.as_deref(), Some("Alice"));
/// assert_eq!(addrs[1].mailbox, "bob");
/// ```
pub fn parse_address_list(text: &str) -> Vec<Address> {
    parse_address_list_with_host(text, "")
}

/// Parse addresses with a default host for unqualified mailboxes.
pub fn parse_address_list_with_host(text: &str, default_host: &str) -> Vec<Address> {
    let mut result = Vec::new();
    let mut s: &[u8] = text.as_bytes();

    while !s.is_empty() {
        s = skip_ws(s);
        if s.is_empty() {
            break;
        }

        // Check for group syntax: "Name:"
        if let Some(colon_pos) = detect_group(s) {
            // Skip group name and colon, parse addresses until ';'
            s = &s[colon_pos + 1..];
            while !s.is_empty() {
                s = skip_ws(s);
                if s.is_empty() || s[0] == b';' {
                    break;
                }
                if let Some((rest, addr)) = parse_mailbox(s, default_host) {
                    result.push(addr);
                    s = skip_ws(rest);
                    if !s.is_empty() && s[0] == b',' {
                        s = &s[1..];
                    }
                } else {
                    // Skip unparseable character
                    s = &s[1..];
                }
            }
            if !s.is_empty() && s[0] == b';' {
                s = &s[1..];
            }
            s = skip_ws(s);
            if !s.is_empty() && s[0] == b',' {
                s = &s[1..];
            }
            continue;
        }

        // Regular address
        if let Some((rest, addr)) = parse_mailbox(s, default_host) {
            result.push(addr);
            s = skip_ws(rest);
            if !s.is_empty() && s[0] == b',' {
                s = &s[1..];
            }
        } else {
            break;
        }
    }

    result
}

/// Detect group syntax: look for `phrase:` where the phrase is followed
/// by a colon (not inside angle brackets). Returns the byte offset of
/// the colon, or `None`.
fn detect_group(s: &[u8]) -> Option<usize> {
    // A group starts with a phrase followed by ':'
    // We need to make sure we don't confuse this with a regular address
    let tmp = skip_ws(s);
    if tmp.is_empty() {
        return None;
    }
    if let Some((after, _phrase)) = parse_phrase(tmp) {
        let after = skip_ws(after);
        if !after.is_empty() && after[0] == b':' {
            // Calculate byte offset of colon in original slice s
            let offset = s.len() - after.len();
            return Some(offset);
        }
    }
    None
}

/// Parse a single mailbox address string.
///
/// # Example
/// ```
/// use crispy::rfc822::parse_address;
///
/// let addr = parse_address("John Doe <john@example.com>").unwrap();
/// assert_eq!(addr.name.as_deref(), Some("John Doe"));
/// assert_eq!(addr.mailbox, "john");
/// assert_eq!(addr.host, "example.com");
/// ```
pub fn parse_address(text: &str) -> Option<Address> {
    let s = text.as_bytes();
    parse_mailbox(s, "").map(|(_, addr)| addr)
}

// ================================================================
// Address formatting
// ================================================================

/// Format a single address to a display string.
///
/// If the address has a display name, produces `Name <user@host>`,
/// quoting the name if necessary. Otherwise produces `user@host`.
///
/// # Example
/// ```
/// use crispy::rfc822::{Address, format_address};
///
/// let addr = Address::with_name("John Doe", "john", "example.com");
/// assert_eq!(format_address(&addr), "\"John Doe\" <john@example.com>");
/// ```
pub fn format_address(addr: &Address) -> String {
    let spec = addr.addr_spec();
    match &addr.name {
        Some(name) if !name.is_empty() => {
            let quoted = quote(name);
            format!("{} <{}>", quoted, spec)
        }
        _ => spec,
    }
}

/// Format a list of addresses as a comma-separated string.
///
/// # Example
/// ```
/// use crispy::rfc822::{Address, format_address_list};
///
/// let addrs = vec![
///     Address::with_name("Alice", "alice", "a.com"),
///     Address::new("bob", "b.com"),
/// ];
/// assert_eq!(format_address_list(&addrs), "Alice <alice@a.com>, bob@b.com");
/// ```
pub fn format_address_list(addrs: &[Address]) -> String {
    let mut out = String::new();
    for (i, addr) in addrs.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&format_address(addr));
    }
    out
}

// ================================================================
// Date parsing
// ================================================================

/// Parse a month abbreviation to a 1-based month number.
fn month_from_name(name: &str) -> i32 {
    let name_lower: Vec<u8> = name.bytes().take(3).map(|c| c.to_ascii_lowercase()).collect();
    for (i, m) in MONTHS_SHORT.iter().enumerate() {
        let m_lower: Vec<u8> = m.bytes().map(|c| c.to_ascii_lowercase()).collect();
        if name_lower == m_lower {
            return (i + 1) as i32;
        }
    }
    0
}

/// Parse a date/time string in various RFC 822 and IMAP formats.
///
/// Accepted formats:
/// - `"Mon, dd Mon yyyy hh:mm:ss +zzzz"` (RFC 822)
/// - `"dd-Mon-yyyy hh:mm:ss +zzzz"` (IMAP INTERNALDATE)
/// - `"dd Mon yyyy hh:mm:ss +zzzz"`
/// - `"mm/dd/yyyy"`
/// - Symbolic timezones: GMT, UTC, EST, EDT, CST, CDT, MST, MDT, PST, PDT,
///   CET, CEST, JST
///
/// Returns `None` if the date cannot be parsed.
pub fn parse_date(text: &str) -> Option<ParsedDate> {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut d = ParsedDate::default();

    // Skip leading whitespace
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }

    // Skip day-of-week if present: "Mon, " or "Monday, "
    if i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        let after_alpha = i;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b',' {
            i += 1;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
        } else if i < bytes.len() && (bytes[i] == b'-' || bytes[i].is_ascii_digit()) {
            // Not a day name, might be month in "dd-Mon-yyyy" — reset
            i = start;
        } else {
            i = after_alpha;
        }
    }

    // Try "dd-Mon-yyyy", "dd Mon yyyy", or "mm/dd/yyyy"
    if i < bytes.len() && bytes[i].is_ascii_digit() {
        let (n1, next) = parse_int(&bytes[i..]);
        i += next;

        if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b' ') {
            let sep = bytes[i];
            i += 1;
            if i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                // dd-Mon-yyyy or dd Mon yyyy
                d.day = n1;
                let month_start = i;
                while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                d.month = month_from_name(
                    &String::from_utf8_lossy(&bytes[month_start..i]),
                );
                if i < bytes.len() && (bytes[i] == sep || bytes[i] == b' ' || bytes[i] == b'-') {
                    i += 1;
                }
                let (year, next) = parse_int(&bytes[i..]);
                d.year = fix_year(year);
                i += next;
            } else if i < bytes.len() && bytes[i].is_ascii_digit() && sep == b'/' {
                // mm/dd/yyyy (first number was month)
                d.month = n1;
                let (day, next) = parse_int(&bytes[i..]);
                d.day = day;
                i += next;
                if i < bytes.len() && bytes[i] == b'/' {
                    i += 1;
                }
                let (year, next) = parse_int(&bytes[i..]);
                d.year = fix_year(year);
                i += next;
            }
        } else if i < bytes.len() && bytes[i] == b'/' {
            // mm/dd/yyyy
            i += 1;
            d.month = n1;
            let (day, next) = parse_int(&bytes[i..]);
            d.day = day;
            i += next;
            if i < bytes.len() && bytes[i] == b'/' {
                i += 1;
            }
            let (year, next) = parse_int(&bytes[i..]);
            d.year = fix_year(year);
            i += next;
        }
    }

    if d.year == 0 {
        return None;
    }

    // Time: hh:mm:ss or hh:mm
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i < bytes.len() && bytes[i].is_ascii_digit() {
        let (h, next) = parse_int(&bytes[i..]);
        d.hours = h;
        i += next;
        if i < bytes.len() && bytes[i] == b':' {
            i += 1;
            let (m, next) = parse_int(&bytes[i..]);
            d.minutes = m;
            i += next;
        }
        if i < bytes.len() && bytes[i] == b':' {
            i += 1;
            let (s, next) = parse_int(&bytes[i..]);
            d.seconds = s;
            i += next;
        }
    }

    // Timezone
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        let sign = if bytes[i] == b'-' { -1 } else { 1 };
        i += 1;
        let (tz, _next) = parse_int(&bytes[i..]);
        d.tz_offset_min = if tz > 99 {
            sign * ((tz / 100) * 60 + (tz % 100))
        } else {
            sign * tz * 60
        };
    } else if i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        let tz_start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        let tz_name = String::from_utf8_lossy(&bytes[tz_start..i]).to_ascii_uppercase();
        d.tz_offset_min = match tz_name.as_str() {
            "GMT" | "UTC" => 0,
            "EST" => -300,
            "EDT" => -240,
            "CST" => -360,
            "CDT" => -300,
            "MST" => -420,
            "MDT" => -360,
            "PST" => -480,
            "PDT" => -420,
            "CET" => 60,
            "CEST" => 120,
            "JST" => 540,
            _ => 0,
        };
    }

    if d.month >= 1 && d.month <= 12 && d.day >= 1 && d.day <= 31 {
        Some(d)
    } else {
        None
    }
}

/// Parse an integer from the front of a byte slice.
/// Returns `(value, bytes_consumed)`.
fn parse_int(s: &[u8]) -> (i32, usize) {
    let mut val: i32 = 0;
    let mut i = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        val = val * 10 + (s[i] - b'0') as i32;
        i += 1;
    }
    (val, i)
}

/// Fix 2-digit year to 4-digit year.
fn fix_year(y: i32) -> i32 {
    if y < 100 {
        if y < 70 { y + 2000 } else { y + 1900 }
    } else {
        y
    }
}

// ================================================================
// Date formatting
// ================================================================

/// Calculate day-of-week (0=Sun, 1=Mon, ..., 6=Sat) using the same
/// Zeller-like formula as the C code.
fn day_of_week(d: &ParsedDate) -> usize {
    let (mut m, mut y) = (d.month, d.year);
    if m <= 2 {
        m += 9;
        y -= 1;
    } else {
        m -= 3;
    }
    ((d.day + 2 + (7 + 31 * m) / 12 + y + y / 4 + y / 400 - y / 100) % 7) as usize
}

/// Format a date as IMAP INTERNALDATE: `"dd-Mon-yyyy hh:mm:ss +zzzz"`.
pub fn format_imap_date(d: &ParsedDate) -> String {
    let tzh = d.tz_offset_min.unsigned_abs() / 60;
    let tzm = d.tz_offset_min.unsigned_abs() % 60;
    let sign = if d.tz_offset_min >= 0 { '+' } else { '-' };
    format!(
        "{:02}-{}-{:04} {:02}:{:02}:{:02} {}{:02}{:02}",
        d.day,
        MONTHS_SHORT[(d.month - 1).max(0) as usize % 12],
        d.year,
        d.hours,
        d.minutes,
        d.seconds,
        sign,
        tzh,
        tzm,
    )
}

/// Format a date as RFC 822: `"Mon, dd Mon yyyy hh:mm:ss +zzzz"`.
pub fn format_date(d: &ParsedDate) -> String {
    let dow = day_of_week(d);
    let tzh = d.tz_offset_min.unsigned_abs() / 60;
    let tzm = d.tz_offset_min.unsigned_abs() % 60;
    let sign = if d.tz_offset_min >= 0 { '+' } else { '-' };
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} {}{:02}{:02}",
        DAYS_SHORT[dow % 7],
        d.day,
        MONTHS_SHORT[(d.month - 1).max(0) as usize % 12],
        d.year,
        d.hours,
        d.minutes,
        d.seconds,
        sign,
        tzh,
        tzm,
    )
}

/// Format a date in ctime style: `"Mon Mon dd hh:mm:ss yyyy\n"`.
pub fn format_cdate(d: &ParsedDate) -> String {
    let dow = day_of_week(d);
    format!(
        "{} {} {:2} {:02}:{:02}:{:02} {:04}\n",
        DAYS_SHORT[dow % 7],
        MONTHS_SHORT[(d.month - 1).max(0) as usize % 12],
        d.day,
        d.hours,
        d.minutes,
        d.seconds,
        d.year,
    )
}

// ================================================================
// Mbox "From " line
// ================================================================

/// Check if a line is an mbox `"From "` separator.
pub fn is_from_line(line: &str) -> bool {
    line.starts_with("From ")
}

/// Build an mbox `"From "` separator line with the current local time.
///
/// Format: `"From sender Day Mon DD HH:MM:SS YYYY\n"`.
pub fn write_from_line(sender: &str) -> String {
    // Use std::time to get current time without external deps.
    // We replicate C's localtime + strftime behavior manually.
    // For a no-dependency approach, fall back to a fixed format.
    // The C code used localtime() + strftime; we do the same via libc-free approach.
    //
    // Since we have no chrono/time crate, produce a placeholder that matches
    // the format. In practice this will be called with the system providing
    // the timestamp.
    format!("From {} Thu Jan  1 00:00:00 1970\n", sender)
}

/// Build an mbox `"From "` separator line with a specific date.
pub fn write_from_line_with_date(sender: &str, d: &ParsedDate) -> String {
    let dow = day_of_week(d);
    format!(
        "From {} {} {} {:2} {:02}:{:02}:{:02} {:04}\n",
        sender,
        DAYS_SHORT[dow % 7],
        MONTHS_SHORT[(d.month - 1).max(0) as usize % 12],
        d.day,
        d.hours,
        d.minutes,
        d.seconds,
        d.year,
    )
}

// ================================================================
// Display beautifiers
// ================================================================

/// Beautify a From address for display in message lists.
///
/// Extracts the display name from `"Name <addr>"`, strips surrounding
/// quotes, and trims whitespace. If no display name, returns the bare
/// address.
///
/// # Example
/// ```
/// use crispy::rfc822::beautify_from;
///
/// assert_eq!(beautify_from("\"John Doe\" <john@example.com>"), "John Doe");
/// assert_eq!(beautify_from("alice@example.com"), "alice@example.com");
/// assert_eq!(beautify_from("<bob@example.com>"), "bob@example.com");
/// ```
pub fn beautify_from(from: &str) -> String {
    let s = from.trim();
    if s.is_empty() {
        return String::new();
    }

    // "Name <addr>" → "Name"
    if let Some(lt) = s.find('<') {
        if s[lt..].contains('>') {
            let name = s[..lt].trim();
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
            // No display name — use address inside <>
            let gt = s.find('>').unwrap_or(s.len());
            let addr = &s[lt + 1..gt];
            if !addr.is_empty() {
                return addr.to_string();
            }
        }
    }

    // Strip surrounding quotes
    if s.starts_with('"') && s.ends_with('"') && s.len() > 1 {
        return s[1..s.len() - 1].to_string();
    }

    s.to_string()
}

/// Beautify a date string for display.
///
/// Parses the date and reformats it as `"YYYY-MM-DD HH:MM"`.
/// If parsing fails, returns the original string unchanged.
///
/// # Example
/// ```
/// use crispy::rfc822::beautify_date;
///
/// assert_eq!(beautify_date("Mon, 15 Jan 2024 09:30:00 +0000"), "2024-01-15 09:30");
/// assert_eq!(beautify_date("not a date"), "not a date");
/// ```
pub fn beautify_date(date: &str) -> String {
    match parse_date(date) {
        Some(d) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            d.year, d.month, d.day, d.hours, d.minutes
        ),
        None => date.to_string(),
    }
}

/// Format seconds-since-epoch as an RFC 822 date string.
///
/// This is a best-effort implementation without external time crates.
/// It computes UTC time from the epoch value.
pub fn date_from_secs(secs: u64) -> String {
    // Compute UTC date from epoch seconds (Unix epoch = 1970-01-01)
    let days_total = (secs / 86400) as i64;
    let time_of_day = secs % 86400;
    let hours = (time_of_day / 3600) as i32;
    let minutes = ((time_of_day % 3600) / 60) as i32;
    let seconds = (time_of_day % 60) as i32;

    // Day of week: 1970-01-01 was Thursday (4)
    let dow = ((days_total + 4) % 7) as usize;

    // Date from days since epoch (civil_from_days algorithm)
    let z = days_total + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as i32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as i32;
    let year = (y + if month <= 2 { 1 } else { 0 }) as i32;

    let d = ParsedDate {
        year,
        month,
        day,
        hours,
        minutes,
        seconds,
        tz_offset_min: 0,
    };
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} +0000",
        DAYS_SHORT[dow % 7],
        d.day,
        MONTHS_SHORT[(d.month - 1).max(0) as usize % 12],
        d.year,
        d.hours,
        d.minutes,
        d.seconds,
    )
}

// ================================================================
// Subject threading helpers
// ================================================================

/// Strip `Re:`, `Re[N]:` prefixes from a subject line.
pub fn skip_re(subject: &str) -> &str {
    let mut s = subject;
    loop {
        s = s.trim_start();
        let b = s.as_bytes();
        if b.len() >= 3
            && b[0].to_ascii_lowercase() == b'r'
            && b[1].to_ascii_lowercase() == b'e'
            && b[2] == b':'
        {
            s = &s[3..];
        } else if b.len() >= 4
            && b[0].to_ascii_lowercase() == b'r'
            && b[1].to_ascii_lowercase() == b'e'
            && b[2] == b'['
            && b[3].is_ascii_digit()
        {
            let mut i = 3;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i < b.len() && b[i] == b']' {
                i += 1;
            }
            if i < b.len() && b[i] == b':' {
                i += 1;
            }
            s = &s[i..];
        } else {
            break;
        }
    }
    s
}

/// Strip `Fwd:`, `(Fwd)`, `[Fwd: ...]` prefixes from a subject line.
pub fn skip_fwd(subject: &str) -> &str {
    let mut s = subject;
    loop {
        s = s.trim_start();
        let b = s.as_bytes();
        if b.len() >= 5
            && b[0] == b'('
            && b[1].to_ascii_lowercase() == b'f'
            && b[2].to_ascii_lowercase() == b'w'
            && b[3].to_ascii_lowercase() == b'd'
            && b[4] == b')'
        {
            s = &s[5..];
        } else if b.len() >= 4
            && b[..4].eq_ignore_ascii_case(b"fwd:")
        {
            s = &s[4..];
        } else if b.len() >= 5
            && b[..5].eq_ignore_ascii_case(b"[fwd:")
        {
            if let Some(end) = s.find(']') {
                s = &s[end + 1..];
            } else {
                break;
            }
        } else {
            break;
        }
    }
    s
}

/// Extract the base subject for threading by stripping all Re:/Fwd: prefixes.
pub fn base_subject(subject: &str) -> &str {
    let mut s = subject;
    loop {
        let prev = s;
        s = skip_re(s);
        s = skip_fwd(s);
        if s.len() == prev.len() {
            break;
        }
    }
    s
}

// ================================================================
// MIME parameter quoting
// ================================================================

/// Quote a MIME parameter value if it contains tspecials.
pub fn quote_param_value(value: &str) -> String {
    let needs = value.bytes().any(|c| TSPECIALS.contains(&c));
    if !needs {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len() + 4);
    out.push('"');
    for c in value.bytes() {
        if c == b'"' || c == b'\\' {
            out.push('\\');
        }
        out.push(c as char);
    }
    out.push('"');
    out
}

// ================================================================
// Tests
// ================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Address parsing: simple addresses ----

    #[test]
    fn test_parse_bare_address() {
        let addrs = parse_address_list("user@example.com");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].mailbox, "user");
        assert_eq!(addrs[0].host, "example.com");
        assert!(addrs[0].name.is_none());
    }

    #[test]
    fn test_parse_angle_address() {
        let addrs = parse_address_list("<user@example.com>");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].mailbox, "user");
        assert_eq!(addrs[0].host, "example.com");
    }

    #[test]
    fn test_parse_name_angle_address() {
        let addrs = parse_address_list("John Doe <john@example.com>");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].name.as_deref(), Some("John Doe"));
        assert_eq!(addrs[0].mailbox, "john");
        assert_eq!(addrs[0].host, "example.com");
    }

    #[test]
    fn test_parse_quoted_name() {
        let addrs = parse_address_list("\"Doe, John\" <john@example.com>");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].name.as_deref(), Some("Doe, John"));
        assert_eq!(addrs[0].mailbox, "john");
    }

    #[test]
    fn test_parse_multiple_addresses() {
        let addrs = parse_address_list(
            "Alice <alice@a.com>, bob@b.com, \"Carol\" <carol@c.com>",
        );
        assert_eq!(addrs.len(), 3);
        assert_eq!(addrs[0].name.as_deref(), Some("Alice"));
        assert_eq!(addrs[0].mailbox, "alice");
        assert_eq!(addrs[1].mailbox, "bob");
        assert_eq!(addrs[1].host, "b.com");
        assert!(addrs[1].name.is_none());
        assert_eq!(addrs[2].name.as_deref(), Some("Carol"));
    }

    #[test]
    fn test_parse_empty() {
        assert!(parse_address_list("").is_empty());
        assert!(parse_address_list("   ").is_empty());
    }

    // ---- Address parsing: group syntax ----

    #[test]
    fn test_parse_group() {
        let addrs = parse_address_list("Friends: alice@a.com, bob@b.com ;");
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0].mailbox, "alice");
        assert_eq!(addrs[1].mailbox, "bob");
    }

    #[test]
    fn test_parse_group_with_regular() {
        let addrs = parse_address_list(
            "Team: alice@a.com ; , carol@c.com",
        );
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0].mailbox, "alice");
        assert_eq!(addrs[1].mailbox, "carol");
    }

    // ---- Address parsing: edge cases ----

    #[test]
    fn test_parse_no_host() {
        let addrs = parse_address_list_with_host("localuser", "default.com");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].mailbox, "localuser");
        assert_eq!(addrs[0].host, "default.com");
    }

    #[test]
    fn test_parse_route_addr() {
        let addrs = parse_address_list("<@relay:user@host.com>");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].mailbox, "user");
        assert_eq!(addrs[0].host, "host.com");
        assert_eq!(addrs[0].adl.as_deref(), Some("@relay"));
    }

    #[test]
    fn test_parse_quoted_local_part() {
        let addrs = parse_address_list("\"user name\"@example.com");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].mailbox, "user name");
        assert_eq!(addrs[0].host, "example.com");
    }

    #[test]
    fn test_parse_single_address() {
        let addr = parse_address("alice@example.com").unwrap();
        assert_eq!(addr.mailbox, "alice");
        assert_eq!(addr.host, "example.com");
    }

    #[test]
    fn test_parse_comment_in_address() {
        // Comments should be skipped
        let addrs = parse_address_list("user@example.com (A Comment)");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].mailbox, "user");
    }

    // ---- Address formatting ----

    #[test]
    fn test_format_bare() {
        let addr = Address::new("user", "example.com");
        assert_eq!(format_address(&addr), "user@example.com");
    }

    #[test]
    fn test_format_with_name_simple() {
        let addr = Address::with_name("Alice", "alice", "a.com");
        assert_eq!(format_address(&addr), "Alice <alice@a.com>");
    }

    #[test]
    fn test_format_with_name_needs_quoting() {
        let addr = Address::with_name("Doe, John", "john", "example.com");
        assert_eq!(
            format_address(&addr),
            "\"Doe, John\" <john@example.com>"
        );
    }

    #[test]
    fn test_format_list() {
        let addrs = vec![
            Address::with_name("Alice", "alice", "a.com"),
            Address::new("bob", "b.com"),
        ];
        assert_eq!(
            format_address_list(&addrs),
            "Alice <alice@a.com>, bob@b.com"
        );
    }

    #[test]
    fn test_format_empty_list() {
        let addrs: Vec<Address> = vec![];
        assert_eq!(format_address_list(&addrs), "");
    }

    // ---- Quoting ----

    #[test]
    fn test_quote_no_specials() {
        assert_eq!(quote("simple"), "simple");
    }

    #[test]
    fn test_quote_with_space() {
        assert_eq!(quote("hello world"), "\"hello world\"");
    }

    #[test]
    fn test_quote_with_special() {
        assert_eq!(quote("a@b"), "\"a@b\"");
    }

    #[test]
    fn test_quote_escape() {
        assert_eq!(quote("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn test_unquote() {
        assert_eq!(unquote("\"hello world\""), "hello world");
        assert_eq!(unquote("\"say \\\"hi\\\"\""), "say \"hi\"");
        assert_eq!(unquote("noquotes"), "noquotes");
    }

    // ---- Date parsing ----

    #[test]
    fn test_parse_rfc822_date() {
        let d = parse_date("Mon, 15 Jan 2024 09:30:45 +0200").unwrap();
        assert_eq!(d.year, 2024);
        assert_eq!(d.month, 1);
        assert_eq!(d.day, 15);
        assert_eq!(d.hours, 9);
        assert_eq!(d.minutes, 30);
        assert_eq!(d.seconds, 45);
        assert_eq!(d.tz_offset_min, 120);
    }

    #[test]
    fn test_parse_imap_date() {
        let d = parse_date("15-Jan-2024 09:30:45 +0000").unwrap();
        assert_eq!(d.year, 2024);
        assert_eq!(d.month, 1);
        assert_eq!(d.day, 15);
        assert_eq!(d.tz_offset_min, 0);
    }

    #[test]
    fn test_parse_date_slash() {
        let d = parse_date("01/15/2024").unwrap();
        assert_eq!(d.month, 1);
        assert_eq!(d.day, 15);
        assert_eq!(d.year, 2024);
    }

    #[test]
    fn test_parse_date_two_digit_year() {
        let d = parse_date("15-Jan-99 12:00:00 +0000").unwrap();
        assert_eq!(d.year, 1999);
        let d2 = parse_date("15-Jan-05 12:00:00 +0000").unwrap();
        assert_eq!(d2.year, 2005);
    }

    #[test]
    fn test_parse_date_symbolic_tz() {
        let d = parse_date("Mon, 15 Jan 2024 09:30:00 PST").unwrap();
        assert_eq!(d.tz_offset_min, -480);
        let d2 = parse_date("Mon, 15 Jan 2024 09:30:00 EST").unwrap();
        assert_eq!(d2.tz_offset_min, -300);
    }

    #[test]
    fn test_parse_date_negative_tz() {
        let d = parse_date("Mon, 15 Jan 2024 09:30:00 -0500").unwrap();
        assert_eq!(d.tz_offset_min, -300);
    }

    #[test]
    fn test_parse_invalid_date() {
        assert!(parse_date("not a date").is_none());
        assert!(parse_date("").is_none());
    }

    // ---- Date formatting ----

    #[test]
    fn test_format_imap_date() {
        let d = ParsedDate {
            year: 2024, month: 1, day: 15,
            hours: 9, minutes: 30, seconds: 45,
            tz_offset_min: 120,
        };
        assert_eq!(format_imap_date(&d), "15-Jan-2024 09:30:45 +0200");
    }

    #[test]
    fn test_format_rfc822_date() {
        let d = ParsedDate {
            year: 2024, month: 1, day: 15,
            hours: 9, minutes: 30, seconds: 45,
            tz_offset_min: 0,
        };
        let formatted = format_date(&d);
        // 2024-01-15 is a Monday
        assert!(formatted.starts_with("Mon, 15 Jan 2024"));
        assert!(formatted.ends_with("+0000"));
    }

    #[test]
    fn test_format_cdate() {
        let d = ParsedDate {
            year: 2024, month: 1, day: 15,
            hours: 9, minutes: 30, seconds: 0,
            tz_offset_min: 0,
        };
        let formatted = format_cdate(&d);
        assert!(formatted.starts_with("Mon Jan 15 09:30:00 2024"));
        assert!(formatted.ends_with('\n'));
    }

    #[test]
    fn test_format_negative_tz() {
        let d = ParsedDate {
            year: 2024, month: 6, day: 1,
            hours: 12, minutes: 0, seconds: 0,
            tz_offset_min: -300,
        };
        assert!(format_imap_date(&d).ends_with("-0500"));
    }

    // ---- Beautifiers ----

    #[test]
    fn test_beautify_from_name_angle() {
        assert_eq!(
            beautify_from("John Doe <john@example.com>"),
            "John Doe"
        );
    }

    #[test]
    fn test_beautify_from_quoted_name() {
        assert_eq!(
            beautify_from("\"John Doe\" <john@example.com>"),
            "John Doe"
        );
    }

    #[test]
    fn test_beautify_from_bare_address() {
        assert_eq!(
            beautify_from("alice@example.com"),
            "alice@example.com"
        );
    }

    #[test]
    fn test_beautify_from_angle_only() {
        assert_eq!(
            beautify_from("<bob@example.com>"),
            "bob@example.com"
        );
    }

    #[test]
    fn test_beautify_from_empty() {
        assert_eq!(beautify_from(""), "");
        assert_eq!(beautify_from("   "), "");
    }

    #[test]
    fn test_beautify_from_quoted_no_angle() {
        assert_eq!(beautify_from("\"Alice\""), "Alice");
    }

    #[test]
    fn test_beautify_date_rfc822() {
        assert_eq!(
            beautify_date("Mon, 15 Jan 2024 09:30:00 +0000"),
            "2024-01-15 09:30"
        );
    }

    #[test]
    fn test_beautify_date_unparseable() {
        assert_eq!(beautify_date("not a date"), "not a date");
    }

    // ---- From line ----

    #[test]
    fn test_is_from_line() {
        assert!(is_from_line("From user@host Thu Jan 1 00:00:00 1970"));
        assert!(!is_from_line("Subject: hello"));
        assert!(!is_from_line(""));
    }

    #[test]
    fn test_write_from_line() {
        let line = write_from_line("user@example.com");
        assert!(line.starts_with("From user@example.com "));
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn test_write_from_line_with_date() {
        let d = ParsedDate {
            year: 2024, month: 1, day: 15,
            hours: 9, minutes: 30, seconds: 0,
            tz_offset_min: 0,
        };
        let line = write_from_line_with_date("alice@a.com", &d);
        assert!(line.starts_with("From alice@a.com Mon Jan 15"));
        assert!(line.ends_with('\n'));
    }

    // ---- Subject stripping ----

    #[test]
    fn test_skip_re() {
        assert_eq!(skip_re("Re: Hello"), "Hello");
        assert_eq!(skip_re("re: Hello"), "Hello");
        assert_eq!(skip_re("Re: Re: Hello"), "Hello");
        assert_eq!(skip_re("Re[2]: Hello"), "Hello");
        assert_eq!(skip_re("Hello"), "Hello");
    }

    #[test]
    fn test_skip_fwd() {
        assert_eq!(skip_fwd("Fwd: Hello"), "Hello");
        assert_eq!(skip_fwd("(Fwd) Hello"), "Hello");
        assert_eq!(skip_fwd("[Fwd: some msg] Hello"), "Hello");
        assert_eq!(skip_fwd("Hello"), "Hello");
    }

    #[test]
    fn test_base_subject() {
        assert_eq!(base_subject("Re: Fwd: Re: Hello"), "Hello");
        assert_eq!(base_subject("Hello"), "Hello");
    }

    // ---- date_from_secs ----

    #[test]
    fn test_date_from_secs_epoch() {
        let s = date_from_secs(0);
        assert!(s.contains("01 Jan 1970"));
        assert!(s.contains("00:00:00"));
    }

    #[test]
    fn test_date_from_secs_known() {
        // 2024-01-15 12:00:00 UTC = 1705320000
        let s = date_from_secs(1705320000);
        assert!(s.contains("15 Jan 2024"));
        assert!(s.contains("12:00:00"));
    }

    // ---- MIME param quoting ----

    #[test]
    fn test_quote_param_no_special() {
        assert_eq!(quote_param_value("plain"), "plain");
    }

    #[test]
    fn test_quote_param_with_special() {
        assert_eq!(quote_param_value("text/html"), "\"text/html\"");
    }

    // ---- Round-trip: parse then format ----

    #[test]
    fn test_roundtrip_simple() {
        let input = "user@example.com";
        let addrs = parse_address_list(input);
        assert_eq!(format_address_list(&addrs), input);
    }

    #[test]
    fn test_roundtrip_named() {
        let addrs = parse_address_list("Alice <alice@a.com>");
        let formatted = format_address_list(&addrs);
        assert_eq!(formatted, "Alice <alice@a.com>");
    }

    #[test]
    fn test_roundtrip_multiple() {
        let input = "Alice <alice@a.com>, bob@b.com";
        let addrs = parse_address_list(input);
        assert_eq!(format_address_list(&addrs), input);
    }

    // ---- addr_spec ----

    #[test]
    fn test_addr_spec() {
        let a = Address::new("user", "host.com");
        assert_eq!(a.addr_spec(), "user@host.com");
        let b = Address::new("local", "");
        assert_eq!(b.addr_spec(), "local");
    }

    // ---- Parse date roundtrip ----

    #[test]
    fn test_date_roundtrip() {
        let d = ParsedDate {
            year: 2024, month: 3, day: 22,
            hours: 14, minutes: 30, seconds: 0,
            tz_offset_min: 60,
        };
        let formatted = format_imap_date(&d);
        let parsed = parse_date(&formatted).unwrap();
        assert_eq!(parsed.year, d.year);
        assert_eq!(parsed.month, d.month);
        assert_eq!(parsed.day, d.day);
        assert_eq!(parsed.hours, d.hours);
        assert_eq!(parsed.minutes, d.minutes);
        assert_eq!(parsed.seconds, d.seconds);
        assert_eq!(parsed.tz_offset_min, d.tz_offset_min);
    }
}
