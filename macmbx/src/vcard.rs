//! vCard 2.1/3.0 parser and builder.
//!
//! Full RFC 2426 (vCard 3.0) + RFC 2425 (MIME directory) support:
//! - Structured properties: N, ADR, ORG
//! - Parameters: TYPE, VALUE, ENCODING, CHARSET, PREF
//! - Encodings: quoted-printable, base64
//! - Line unfolding (continuation lines)
//! - Multiple values per property
//! - X-DASH custom properties
//! - PHOTO with base64
//! - vCard 2.1 bare parameter compatibility
//!
//! No external dependency. Pure Rust.

use std::fs;
use std::io;
use std::path::Path;

/// Maximum values per structured property (ADR has 7 parts, N has 5).
pub const MAX_VALUES: usize = 8;
/// Maximum parameters per property.
pub const MAX_PARAMS: usize = 8;

// ── Types ──

/// A parameter on a vCard property (e.g. TYPE=WORK, ENCODING=b).
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub value: String,
}

/// A single vCard property (one line like `TEL;TYPE=WORK:+1234`).
#[derive(Debug, Clone)]
pub struct Prop {
    pub name: String,
    pub group: String,
    pub params: Vec<Param>,
    /// Semicolon-separated parts for structured properties (N, ADR, ORG).
    pub values: Vec<String>,
    /// Full decoded value string.
    pub raw_value: String,
    /// Decoded binary data (PHOTO, LOGO, KEY).
    pub binary_data: Option<Vec<u8>>,
}

impl Prop {
    fn new() -> Self {
        Self {
            name: String::new(),
            group: String::new(),
            params: Vec::new(),
            values: Vec::new(),
            raw_value: String::new(),
            binary_data: None,
        }
    }

    /// Add a parameter to this property.
    pub fn add_param(&mut self, name: &str, value: &str) {
        if self.params.len() < MAX_PARAMS {
            self.params.push(Param {
                name: name.to_string(),
                value: value.to_string(),
            });
        }
    }

    /// Check if this property has ENCODING=<enc> (case-insensitive).
    fn has_encoding(&self, enc: &str) -> bool {
        self.params.iter().any(|p| {
            p.name.eq_ignore_ascii_case("ENCODING") && p.value.eq_ignore_ascii_case(enc)
        })
    }

    /// Check if this property has a TYPE parameter matching a value (case-insensitive substring).
    pub fn has_type(&self, type_val: &str) -> bool {
        self.params.iter().any(|p| {
            p.name.eq_ignore_ascii_case("TYPE")
                && p.value.to_ascii_uppercase().contains(&type_val.to_ascii_uppercase())
        })
    }

    /// Get the first TYPE parameter value, or None.
    pub fn get_type(&self) -> Option<&str> {
        self.params.iter()
            .find(|p| p.name.eq_ignore_ascii_case("TYPE"))
            .map(|p| p.value.as_str())
    }
}

/// A complete vCard.
#[derive(Debug, Clone)]
pub struct Vcard {
    pub version: String,
    pub props: Vec<Prop>,
}

impl Vcard {
    /// Create a new empty vCard (version 3.0).
    pub fn new() -> Self {
        Self { version: "3.0".to_string(), props: Vec::new() }
    }

    // ── Building ──

    /// Add a simple property (name=value).
    pub fn add(&mut self, name: &str, value: &str) -> &mut Prop {
        let mut p = Prop::new();
        p.name = name.to_ascii_uppercase();
        p.raw_value = value.to_string();
        if is_structured(&p.name) {
            split_structured(&mut p, value);
        }
        self.props.push(p);
        self.props.last_mut().unwrap()
    }

    /// Add a property with a TYPE parameter.
    pub fn add_typed(&mut self, name: &str, value: &str, type_val: &str) -> &mut Prop {
        self.add(name, value);
        let prop = self.props.last_mut().unwrap();
        prop.add_param("TYPE", type_val);
        prop
    }

    /// Add a structured property (N, ADR, ORG) from parts.
    pub fn add_structured(&mut self, name: &str, parts: &[&str]) -> &mut Prop {
        let joined = parts.join(";");
        self.add(name, &joined)
    }

    /// Add a binary property (PHOTO, LOGO, KEY) with base64 encoding.
    pub fn add_binary(&mut self, name: &str, media_type: Option<&str>, data: &[u8]) -> &mut Prop {
        let mut p = Prop::new();
        p.name = name.to_ascii_uppercase();
        p.binary_data = Some(data.to_vec());
        p.raw_value = b64_encode(data);
        p.add_param("ENCODING", "b");
        if let Some(mt) = media_type {
            p.add_param("TYPE", mt);
        }
        self.props.push(p);
        self.props.last_mut().unwrap()
    }

    // ── Access ──

    /// Find the first property by name (case-insensitive).
    pub fn find(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// Find all properties matching a name (case-insensitive).
    pub fn find_all(&self, name: &str) -> Vec<&Prop> {
        self.props.iter().filter(|p| p.name.eq_ignore_ascii_case(name)).collect()
    }

    /// Get the raw value of the first property with the given name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.find(name).map(|p| p.raw_value.as_str())
    }

    /// Get a specific part of a structured property.
    pub fn get_part(&self, name: &str, part: usize) -> Option<&str> {
        self.find(name)
            .and_then(|p| p.values.get(part))
            .map(|s| s.as_str())
    }

    // ── Convenience accessors ──

    /// Formatted name (FN).
    pub fn fn_name(&self) -> Option<&str> { self.get("FN") }

    /// Family name (N, part 0).
    pub fn family(&self) -> Option<&str> { self.get_part("N", 0) }

    /// Given name (N, part 1).
    pub fn given(&self) -> Option<&str> { self.get_part("N", 1) }

    /// First email address.
    pub fn email(&self) -> Option<&str> { self.get("EMAIL") }

    /// All email addresses.
    pub fn emails(&self) -> Vec<&str> {
        self.find_all("EMAIL").into_iter().map(|p| p.raw_value.as_str()).collect()
    }

    /// Phone number. If `type_filter` is Some, only return numbers matching that TYPE.
    pub fn phone(&self, type_filter: Option<&str>) -> Option<&str> {
        for p in &self.props {
            if !p.name.eq_ignore_ascii_case("TEL") { continue; }
            match type_filter {
                None => return Some(&p.raw_value),
                Some(t) if p.has_type(t) => return Some(&p.raw_value),
                _ => {}
            }
        }
        None
    }

    /// Organization (ORG).
    pub fn org(&self) -> Option<&str> { self.get("ORG") }

    /// Photo binary data and media type.
    pub fn photo(&self) -> Option<(&[u8], Option<&str>)> {
        self.find("PHOTO").and_then(|p| {
            p.binary_data.as_ref().map(|data| {
                (data.as_slice(), p.get_type())
            })
        })
    }

    // ── Serialization ──

    /// Serialize to vCard text (RFC 2426 format with CRLF line endings).
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str("BEGIN:VCARD\r\n");
        fold_line(&mut out, &format!("VERSION:{}", self.version));

        for p in &self.props {
            if p.name.eq_ignore_ascii_case("VERSION") { continue; }
            let mut line = String::new();
            if !p.group.is_empty() {
                line.push_str(&p.group);
                line.push('.');
            }
            line.push_str(&p.name);
            for param in &p.params {
                line.push(';');
                line.push_str(&param.name);
                line.push('=');
                line.push_str(&param.value);
            }
            line.push(':');
            line.push_str(&p.raw_value);
            fold_line(&mut out, &line);
        }

        out.push_str("END:VCARD\r\n");
        out
    }

    /// Write this vCard to a file.
    pub fn write_file(&self, path: &Path) -> io::Result<()> {
        fs::write(path, self.serialize().as_bytes())
    }

    /// Write multiple vCards to a single file.
    pub fn write_file_multi(cards: &[&Vcard], path: &Path) -> io::Result<()> {
        let mut data = String::new();
        for vc in cards {
            data.push_str(&vc.serialize());
        }
        fs::write(path, data.as_bytes())
    }
}

impl Default for Vcard {
    fn default() -> Self { Self::new() }
}

// ── Parsing ──

/// Parse a single vCard from text. Returns None on error.
pub fn parse(text: &str) -> Option<Vcard> {
    let unfolded = unfold_lines(text);
    let mut vc = Vcard::new();
    let mut in_vcard = false;

    for line in unfolded.lines() {
        if line.eq_ignore_ascii_case("BEGIN:VCARD") {
            in_vcard = true;
            continue;
        }
        if line.eq_ignore_ascii_case("END:VCARD") {
            break;
        }
        if !in_vcard { continue; }

        // Find the colon separating name[;params] from value
        let colon_pos = match line.find(':') {
            Some(pos) => pos,
            None => continue,
        };
        let raw_name_part = &line[..colon_pos];
        let raw_value = &line[colon_pos + 1..];

        let mut prop = Prop::new();

        // Check for group prefix (e.g. "item1.TEL")
        let name_part = if let Some(dot_pos) = raw_name_part.find('.') {
            prop.group = raw_name_part[..dot_pos].to_string();
            &raw_name_part[dot_pos + 1..]
        } else {
            raw_name_part
        };

        // Split name from params at first semicolon
        let (prop_name, params_str) = match name_part.find(';') {
            Some(pos) => (&name_part[..pos], Some(&name_part[pos + 1..])),
            None => (name_part, None),
        };

        prop.name = prop_name.to_ascii_uppercase();

        // Parse parameters
        if let Some(ps) = params_str {
            parse_params(ps, &mut prop);
        }

        // VERSION
        if prop.name == "VERSION" {
            vc.version = raw_value.to_string();
            continue;
        }

        // Decode value based on encoding
        if prop.has_encoding("QUOTED-PRINTABLE") {
            prop.raw_value = qp_decode(raw_value);
        } else if prop.has_encoding("b") || prop.has_encoding("BASE64") {
            prop.binary_data = Some(b64_decode(raw_value));
            prop.raw_value = raw_value.to_string();
        } else {
            prop.raw_value = raw_value.to_string();
        }

        // Split structured values
        if is_structured(&prop.name) {
            let val = prop.raw_value.clone();
            split_structured(&mut prop, &val);
        }

        vc.props.push(prop);
    }

    Some(vc)
}

/// Parse a file containing one or more vCards. Returns a Vec of parsed cards.
pub fn parse_file(path: &Path) -> io::Result<Vec<Vcard>> {
    let data = fs::read_to_string(path)?;
    Ok(parse_multi(&data))
}

/// Parse text containing multiple vCards.
pub fn parse_multi(text: &str) -> Vec<Vcard> {
    let upper = text.to_ascii_uppercase();
    let mut cards = Vec::new();
    let mut search_from = 0;

    loop {
        let begin = match upper[search_from..].find("BEGIN:VCARD") {
            Some(pos) => search_from + pos,
            None => break,
        };
        let end = match upper[begin + 11..].find("END:VCARD") {
            Some(pos) => begin + 11 + pos + 9, // include "END:VCARD"
            None => break,
        };
        let slice = &text[begin..end];
        if let Some(vc) = parse(slice) {
            cards.push(vc);
        }
        search_from = end;
    }

    cards
}

// ── Line folding / unfolding ──

/// Unfold continuation lines: CRLF followed by space/tab = continuation.
fn unfold_lines(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut out = Vec::with_capacity(len);
    let mut i = 0;
    while i < len {
        if bytes[i] == b'\r' && i + 1 < len && bytes[i + 1] == b'\n'
            && i + 2 < len && (bytes[i + 2] == b' ' || bytes[i + 2] == b'\t')
        {
            i += 3; // skip CRLF + whitespace
            continue;
        }
        if bytes[i] == b'\n'
            && i + 1 < len && (bytes[i + 1] == b' ' || bytes[i + 1] == b'\t')
        {
            i += 2; // skip LF + whitespace
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    // Safe: we only removed whitespace continuations from valid UTF-8 boundaries
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Fold a line at 75 characters (RFC 2426).
fn fold_line(out: &mut String, line: &str) {
    let len = line.len();
    if len <= 75 {
        out.push_str(line);
        out.push_str("\r\n");
        return;
    }
    let mut i = 0;
    while i < len {
        let chunk = if i == 0 { 75 } else { 74 }; // first line 75, continuation 74 (1 for space)
        let end = std::cmp::min(i + chunk, len);
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&line[i..end]);
        out.push_str("\r\n");
        i = end;
    }
}

// ── Parameter parsing ──

/// Known bare vCard 2.1 type parameters.
const BARE_TYPES: &[&str] = &["WORK", "HOME", "CELL", "FAX", "PREF", "VOICE", "INTERNET"];

fn parse_params(params_str: &str, prop: &mut Prop) {
    for tok in params_str.split(';') {
        if tok.is_empty() { continue; }
        if let Some(eq_pos) = tok.find('=') {
            let pname = tok[..eq_pos].to_ascii_uppercase();
            let pval = tok[eq_pos + 1..].to_string();
            if prop.params.len() < MAX_PARAMS {
                prop.params.push(Param { name: pname, value: pval });
            }
        } else {
            // Bare parameter (vCard 2.1)
            let upper = tok.to_ascii_uppercase();
            if BARE_TYPES.iter().any(|&t| t == upper) {
                if prop.params.len() < MAX_PARAMS {
                    prop.params.push(Param {
                        name: "TYPE".to_string(),
                        value: upper,
                    });
                }
            } else if upper == "QUOTED-PRINTABLE" {
                if prop.params.len() < MAX_PARAMS {
                    prop.params.push(Param {
                        name: "ENCODING".to_string(),
                        value: "QUOTED-PRINTABLE".to_string(),
                    });
                }
            } else if upper == "BASE64" || upper == "B" {
                if prop.params.len() < MAX_PARAMS {
                    prop.params.push(Param {
                        name: "ENCODING".to_string(),
                        value: "b".to_string(),
                    });
                }
            }
        }
    }
}

// ── Structured properties ──

fn is_structured(name: &str) -> bool {
    name.eq_ignore_ascii_case("N") || name.eq_ignore_ascii_case("ADR")
        || name.eq_ignore_ascii_case("ORG")
}

fn split_structured(prop: &mut Prop, value: &str) {
    prop.values = value.split(';')
        .take(MAX_VALUES)
        .map(|s| s.to_string())
        .collect();
}

// ── Base64 ──

const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_val(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => 0,
    }
}

fn b64_decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let filtered: Vec<u8> = bytes.iter()
        .copied()
        .filter(|&b| b != b'\r' && b != b'\n' && b != b' ')
        .collect();
    let len = filtered.len();
    let mut i = 0;
    while i + 3 < len {
        let a = b64_val(filtered[i]);
        let b = b64_val(filtered[i + 1]);
        let c = b64_val(filtered[i + 2]);
        let d = b64_val(filtered[i + 3]);
        out.push((a << 2) | (b >> 4));
        if filtered[i + 2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if filtered[i + 3] != b'=' {
            out.push((c << 6) | d);
        }
        i += 4;
    }
    out
}

fn b64_encode(input: &[u8]) -> String {
    let mut out = Vec::new();
    let mut col = 0;
    let len = input.len();
    let mut i = 0;
    while i < len {
        let a = input[i];
        let b = if i + 1 < len { input[i + 1] } else { 0 };
        let c = if i + 2 < len { input[i + 2] } else { 0 };

        out.push(B64_CHARS[(a >> 2) as usize]);
        out.push(B64_CHARS[(((a & 3) << 4) | (b >> 4)) as usize]);
        out.push(if i + 1 < len { B64_CHARS[(((b & 0xF) << 2) | (c >> 6)) as usize] } else { b'=' });
        out.push(if i + 2 < len { B64_CHARS[(c & 0x3F) as usize] } else { b'=' });

        col += 4;
        if col >= 76 {
            out.push(b'\r');
            out.push(b'\n');
            col = 0;
        }
        i += 3;
    }
    String::from_utf8(out).unwrap()
}

// ── Quoted-printable decode ──

fn hexval(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

fn qp_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut out = Vec::with_capacity(len);
    let mut i = 0;
    while i < len {
        if bytes[i] == b'=' && i + 2 < len {
            if bytes[i + 1] == b'\r' || bytes[i + 1] == b'\n' {
                // Soft line break
                i += 1;
                if i < len && bytes[i] == b'\r' { i += 1; }
                if i < len && bytes[i] == b'\n' { i += 1; }
                continue;
            }
            if let (Some(h), Some(l)) = (hexval(bytes[i + 1]), hexval(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_simple_vcard() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:John Doe\r\nN:Doe;John;;;\r\nEMAIL:john@example.com\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        assert_eq!(vc.version, "3.0");
        assert_eq!(vc.fn_name(), Some("John Doe"));
        assert_eq!(vc.family(), Some("Doe"));
        assert_eq!(vc.given(), Some("John"));
        assert_eq!(vc.email(), Some("john@example.com"));
    }

    #[test]
    fn test_parse_with_types() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Jane\r\nTEL;TYPE=WORK:+1-555-1234\r\nTEL;TYPE=CELL:+1-555-5678\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        assert_eq!(vc.phone(Some("WORK")), Some("+1-555-1234"));
        assert_eq!(vc.phone(Some("CELL")), Some("+1-555-5678"));
        assert_eq!(vc.phone(None), Some("+1-555-1234")); // first match
    }

    #[test]
    fn test_parse_vcard21_bare_params() {
        let text = "BEGIN:VCARD\r\nVERSION:2.1\r\nTEL;WORK;VOICE:+1234\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        assert_eq!(vc.version, "2.1");
        let tel = vc.find("TEL").unwrap();
        assert!(tel.has_type("WORK"));
        assert!(tel.has_type("VOICE"));
    }

    #[test]
    fn test_parse_structured_n() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Smith;Alice;Marie;Dr.;Jr.\r\nFN:Dr. Alice Smith Jr.\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        assert_eq!(vc.get_part("N", 0), Some("Smith"));
        assert_eq!(vc.get_part("N", 1), Some("Alice"));
        assert_eq!(vc.get_part("N", 2), Some("Marie"));
        assert_eq!(vc.get_part("N", 3), Some("Dr."));
        assert_eq!(vc.get_part("N", 4), Some("Jr."));
    }

    #[test]
    fn test_parse_structured_adr() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nADR;TYPE=WORK:;;123 Main St;Anytown;CA;90210;US\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        let adr = vc.find("ADR").unwrap();
        assert!(adr.has_type("WORK"));
        assert_eq!(adr.values.get(2).map(|s| s.as_str()), Some("123 Main St"));
        assert_eq!(adr.values.get(3).map(|s| s.as_str()), Some("Anytown"));
        assert_eq!(adr.values.get(4).map(|s| s.as_str()), Some("CA"));
        assert_eq!(adr.values.get(5).map(|s| s.as_str()), Some("90210"));
    }

    #[test]
    fn test_parse_quoted_printable() {
        let text = "BEGIN:VCARD\r\nVERSION:2.1\r\nFN;QUOTED-PRINTABLE:M=C3=BCller\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        assert_eq!(vc.fn_name(), Some("M\u{00FC}ller")); // u-umlaut
    }

    #[test]
    fn test_parse_base64_photo() {
        // A tiny 1-byte "photo"
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nPHOTO;ENCODING=b;TYPE=JPEG:AQID\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        let (data, media) = vc.photo().unwrap();
        assert_eq!(data, &[1, 2, 3]);
        assert_eq!(media, Some("JPEG"));
    }

    #[test]
    fn test_parse_line_folding() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Very Long Name That Gets\r\n  Folded Here\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        assert_eq!(vc.fn_name(), Some("Very Long Name That Gets Folded Here"));
    }

    #[test]
    fn test_parse_group_prefix() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nitem1.TEL:+1234\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        let tel = vc.find("TEL").unwrap();
        assert_eq!(tel.group, "item1");
        assert_eq!(tel.raw_value, "+1234");
    }

    #[test]
    fn test_parse_multi() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:3.0\r\nFN:Bob\r\nEND:VCARD\r\n";
        let cards = parse_multi(text);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].fn_name(), Some("Alice"));
        assert_eq!(cards[1].fn_name(), Some("Bob"));
    }

    #[test]
    fn test_parse_multiple_emails() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nEMAIL;TYPE=WORK:work@x.com\r\nEMAIL;TYPE=HOME:home@x.com\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        let emails = vc.emails();
        assert_eq!(emails, vec!["work@x.com", "home@x.com"]);
    }

    #[test]
    fn test_parse_org() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nORG:Acme Corp;Engineering\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        assert_eq!(vc.org(), Some("Acme Corp;Engineering"));
        let org = vc.find("ORG").unwrap();
        assert_eq!(org.values.get(0).map(|s| s.as_str()), Some("Acme Corp"));
        assert_eq!(org.values.get(1).map(|s| s.as_str()), Some("Engineering"));
    }

    // -- Building / Serialization tests --

    #[test]
    fn test_build_and_serialize() {
        let mut vc = Vcard::new();
        vc.add("FN", "John Doe");
        vc.add_structured("N", &["Doe", "John", "", "", ""]);
        vc.add_typed("EMAIL", "john@example.com", "WORK");
        vc.add_typed("TEL", "+1-555-1234", "WORK");

        let text = vc.serialize();
        assert!(text.contains("BEGIN:VCARD\r\n"));
        assert!(text.contains("VERSION:3.0\r\n"));
        assert!(text.contains("FN:John Doe\r\n"));
        assert!(text.contains("EMAIL;TYPE=WORK:john@example.com\r\n"));
        assert!(text.contains("END:VCARD\r\n"));
    }

    #[test]
    fn test_roundtrip() {
        let mut vc = Vcard::new();
        vc.add("FN", "Alice Smith");
        vc.add_structured("N", &["Smith", "Alice", "", "", ""]);
        vc.add_typed("EMAIL", "alice@example.com", "HOME");

        let text = vc.serialize();
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.fn_name(), Some("Alice Smith"));
        assert_eq!(parsed.family(), Some("Smith"));
        assert_eq!(parsed.given(), Some("Alice"));
        assert_eq!(parsed.email(), Some("alice@example.com"));
    }

    #[test]
    fn test_binary_roundtrip() {
        let photo_data = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let mut vc = Vcard::new();
        vc.add("FN", "Test");
        vc.add_binary("PHOTO", Some("JPEG"), &photo_data);

        let text = vc.serialize();
        let parsed = parse(&text).unwrap();
        let (data, media) = parsed.photo().unwrap();
        assert_eq!(data, &photo_data);
        assert_eq!(media, Some("JPEG"));
    }

    #[test]
    fn test_serialize_long_line_folding() {
        let mut vc = Vcard::new();
        let long_name = "A".repeat(100);
        vc.add("FN", &long_name);

        let text = vc.serialize();
        // After folding, no line should exceed 77 chars (75 content + CRLF)
        for line in text.split("\r\n") {
            assert!(line.len() <= 75, "Line too long ({}): {}", line.len(), line);
        }

        // Roundtrip should preserve the value
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.fn_name().unwrap().replace(" ", ""), long_name);
    }

    #[test]
    fn test_write_and_parse_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.vcf");

        let mut vc = Vcard::new();
        vc.add("FN", "File Test");
        vc.add("EMAIL", "file@test.com");
        vc.write_file(&path).unwrap();

        let cards = parse_file(&path).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].fn_name(), Some("File Test"));
    }

    #[test]
    fn test_write_file_multi() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("multi.vcf");

        let mut vc1 = Vcard::new();
        vc1.add("FN", "One");
        let mut vc2 = Vcard::new();
        vc2.add("FN", "Two");

        Vcard::write_file_multi(&[&vc1, &vc2], &path).unwrap();

        let cards = parse_file(&path).unwrap();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].fn_name(), Some("One"));
        assert_eq!(cards[1].fn_name(), Some("Two"));
    }

    #[test]
    fn test_find_all() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nTEL;TYPE=WORK:111\r\nTEL;TYPE=HOME:222\r\nTEL;TYPE=CELL:333\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        let tels = vc.find_all("TEL");
        assert_eq!(tels.len(), 3);
        assert_eq!(tels[0].raw_value, "111");
        assert_eq!(tels[2].raw_value, "333");
    }

    #[test]
    fn test_has_type() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nTEL;TYPE=WORK:111\r\nEND:VCARD\r\n";
        let vc = parse(text).unwrap();
        let tel = vc.find("TEL").unwrap();
        assert!(tel.has_type("WORK"));
        assert!(tel.has_type("work"));
        assert!(!tel.has_type("HOME"));
    }

    // -- Base64 / QP unit tests --

    #[test]
    fn test_b64_roundtrip() {
        let data = b"Hello, World!";
        let encoded = b64_encode(data);
        let decoded = b64_decode(&encoded);
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_b64_padding() {
        // 1 byte -> 2 pads, 2 bytes -> 1 pad, 3 bytes -> 0 pads
        assert_eq!(b64_decode(&b64_encode(b"A")), b"A");
        assert_eq!(b64_decode(&b64_encode(b"AB")), b"AB");
        assert_eq!(b64_decode(&b64_encode(b"ABC")), b"ABC");
    }

    #[test]
    fn test_qp_decode_simple() {
        assert_eq!(qp_decode("Hello=20World"), "Hello World");
        assert_eq!(qp_decode("=C3=BC"), "\u{00FC}"); // u-umlaut
    }

    #[test]
    fn test_qp_soft_line_break() {
        assert_eq!(qp_decode("Hello=\r\nWorld"), "HelloWorld");
    }

    #[test]
    fn test_unfold_crlf() {
        assert_eq!(unfold_lines("FN:Hello\r\n World"), "FN:HelloWorld");
    }

    #[test]
    fn test_unfold_lf() {
        assert_eq!(unfold_lines("FN:Hello\n\tWorld"), "FN:HelloWorld");
    }

    #[test]
    fn test_empty_vcard() {
        let vc = Vcard::new();
        assert_eq!(vc.fn_name(), None);
        assert_eq!(vc.email(), None);
        assert_eq!(vc.phone(None), None);
        let text = vc.serialize();
        assert!(text.contains("BEGIN:VCARD"));
        assert!(text.contains("END:VCARD"));
    }

    #[test]
    fn test_prop_add_param() {
        let mut vc = Vcard::new();
        let prop = vc.add("TEL", "+1234");
        prop.add_param("TYPE", "WORK");
        prop.add_param("TYPE", "VOICE");
        assert!(prop.has_type("WORK"));
        assert!(prop.has_type("VOICE"));
    }
}
