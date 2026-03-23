//! RFC 2047 encoded-word, quoted-printable, and charset conversion.

use crate::base64;

// ── Charset conversion tables ──

/// Windows-1252 bytes 0x80–0x9F → Unicode codepoints.
const CP1252_MAP: [u32; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x017D, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

/// ISO-8859-15 byte positions that differ from 8859-1.
const ISO15_DIFF: &[(u8, u32)] = &[
    (0xA4, 0x20AC), (0xA6, 0x0160), (0xA8, 0x0161),
    (0xB4, 0x017D), (0xB8, 0x017E),
    (0xBC, 0x0152), (0xBD, 0x0153), (0xBE, 0x0178),
];

// ── Charset conversion ──

/// Check if a charset name is supported for conversion to UTF-8.
pub fn charset_supported(charset: &str) -> bool {
    matches!(charset.to_ascii_uppercase().as_str(),
        "UTF-8" | "US-ASCII" | "ASCII" |
        "ISO-8859-1" | "LATIN1" |
        "ISO-8859-15" | "LATIN9" |
        "WINDOWS-1252" | "CP1252"
    )
}

/// Convert bytes from the given charset to a UTF-8 string.
pub fn charset_to_utf8(data: &[u8], charset: &str) -> Option<String> {
    let cs = charset.to_ascii_uppercase();
    match cs.as_str() {
        "UTF-8" | "US-ASCII" | "ASCII" => String::from_utf8(data.to_vec()).ok(),
        "WINDOWS-1252" | "CP1252" => Some(cp1252_to_utf8(data)),
        "ISO-8859-15" | "LATIN9" => Some(iso8859_15_to_utf8(data)),
        "ISO-8859-1" | "LATIN1" => Some(latin1_to_utf8(data)),
        s if s.starts_with("ISO-8859-") => Some(latin1_to_utf8(data)), // fallback
        _ => None,
    }
}

fn latin1_to_utf8(data: &[u8]) -> String {
    data.iter().map(|&b| b as char).collect()
}

fn cp1252_to_utf8(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for &b in data {
        let cp = if (0x80..=0x9F).contains(&b) {
            CP1252_MAP[(b - 0x80) as usize]
        } else {
            b as u32
        };
        if let Some(c) = char::from_u32(cp) {
            out.push(c);
        }
    }
    out
}

fn iso8859_15_to_utf8(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for &b in data {
        let cp = ISO15_DIFF.iter()
            .find(|&&(byte, _)| byte == b)
            .map(|&(_, cp)| cp)
            .unwrap_or(b as u32);
        if let Some(c) = char::from_u32(cp) {
            out.push(c);
        }
    }
    out
}

// ── Quoted-Printable ──

const HEX_CHARS: &[u8; 16] = b"0123456789ABCDEF";

/// Encode data as Quoted-Printable (RFC 2045).
pub fn qp_encode(data: &[u8]) -> String {
    let mut out = Vec::with_capacity(data.len() * 2);
    let mut line_len = 0;

    let mut i = 0;
    while i < data.len() {
        let c = data[i];

        if (33..=126).contains(&c) && c != b'=' || c == b'\t'
            || (c == b' ' && i + 1 < data.len() && data[i + 1] != b'\r' && data[i + 1] != b'\n')
        {
            if line_len >= 75 {
                out.extend_from_slice(b"=\r\n");
                line_len = 0;
            }
            out.push(c);
            line_len += 1;
        } else if c == b'\r' && i + 1 < data.len() && data[i + 1] == b'\n' {
            out.extend_from_slice(b"\r\n");
            i += 1;
            line_len = 0;
        } else if c == b'\n' {
            out.extend_from_slice(b"\r\n");
            line_len = 0;
        } else {
            if line_len >= 73 {
                out.extend_from_slice(b"=\r\n");
                line_len = 0;
            }
            out.push(b'=');
            out.push(HEX_CHARS[(c >> 4) as usize]);
            out.push(HEX_CHARS[(c & 0x0F) as usize]);
            line_len += 3;
        }
        i += 1;
    }

    unsafe { String::from_utf8_unchecked(out) }
}

/// Decode Quoted-Printable data.
pub fn qp_decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'=' && i + 2 < bytes.len() {
            if bytes[i + 1] == b'\r' || bytes[i + 1] == b'\n' {
                // Soft line break
                i += 1;
                if i < bytes.len() && bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 1;
                }
                i += 1;
            } else if bytes[i + 1].is_ascii_hexdigit() && bytes[i + 2].is_ascii_hexdigit() {
                let hi = hex_val(bytes[i + 1]);
                let lo = hex_val(bytes[i + 2]);
                out.push((hi << 4) | lo);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }

    out
}

fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'A'..=b'F' => c - b'A' + 10,
        b'a'..=b'f' => c - b'a' + 10,
        _ => 0,
    }
}

// ── RFC 2047 encoded-word ──

/// Decode RFC 2047 encoded-words in a header value.
///
/// Handles `=?charset?B?base64?=` and `=?charset?Q?qp?=` tokens.
/// Whitespace between consecutive encoded-words is collapsed per RFC 2047.
pub fn decode_header(input: &str) -> String {
    if !input.contains("=?") {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    let mut prev_was_encoded = false;

    while let Some(ew_start) = rest.find("=?") {
        let before = &rest[..ew_start];

        // RFC 2047: skip whitespace between consecutive encoded-words
        if prev_was_encoded && before.chars().all(|c| c == ' ' || c == '\t' || c == '\r' || c == '\n') {
            // skip
        } else {
            out.push_str(before);
        }

        let after_prefix = &rest[ew_start + 2..];

        // Find closing ?=  (need 3 question marks total: charset?E?text?=)
        if let Some(decoded) = parse_encoded_word(after_prefix) {
            out.push_str(&decoded.text);
            rest = &rest[ew_start + 2 + decoded.consumed..];
            prev_was_encoded = true;
        } else {
            out.push_str("=?");
            rest = after_prefix;
            prev_was_encoded = false;
        }
    }

    out.push_str(rest);
    out
}

struct DecodedWord {
    text: String,
    consumed: usize, // bytes consumed after "=?"
}

fn parse_encoded_word(s: &str) -> Option<DecodedWord> {
    // Format: charset?E?encoded_text?=
    let cs_end = s.find('?')?;
    let charset = &s[..cs_end];

    let encoding_start = cs_end + 1;
    if encoding_start >= s.len() { return None; }
    let encoding = s.as_bytes()[encoding_start].to_ascii_uppercase();

    if encoding_start + 1 >= s.len() || s.as_bytes()[encoding_start + 1] != b'?' {
        return None;
    }
    let text_start = encoding_start + 2;

    // Find closing ?=
    let text_end = s[text_start..].find("?=")?;
    let encoded_text = &s[text_start..text_start + text_end];
    let total_consumed = text_start + text_end + 2; // includes ?=

    let decoded_bytes = match encoding {
        b'B' => base64::decode(encoded_text).ok()?,
        b'Q' => {
            // Q encoding: _ means space, =XX is hex
            let replaced: String = encoded_text.replace('_', " ");
            qp_decode(&replaced)
        }
        _ => return None,
    };

    let text = charset_to_utf8(&decoded_bytes, charset)
        .unwrap_or_else(|| String::from_utf8_lossy(&decoded_bytes).into_owned());

    Some(DecodedWord { text, consumed: total_consumed })
}

/// Encode a UTF-8 string as RFC 2047 encoded-words if needed.
///
/// Uses `=?UTF-8?B?...?=` format. Returns the input unchanged if no encoding needed.
pub fn encode_header(utf8: &str) -> String {
    if utf8.bytes().all(|b| (32..=126).contains(&b)) {
        return utf8.to_string();
    }

    let mut out = String::new();
    let mut remaining = utf8.as_bytes();
    let mut first = true;

    while !remaining.is_empty() {
        // Chunk at ~45 bytes, don't split UTF-8 sequences
        let mut chunk_len = remaining.len().min(45);
        while chunk_len < remaining.len() && chunk_len > 0
            && (remaining[chunk_len] & 0xC0) == 0x80
        {
            chunk_len -= 1;
        }
        if chunk_len == 0 { chunk_len = remaining.len(); }

        let chunk = &remaining[..chunk_len];
        let b64 = base64::encode(chunk);

        if !first {
            out.push_str("\r\n ");
        }
        out.push_str("=?UTF-8?B?");
        out.push_str(&b64);
        out.push_str("?=");

        remaining = &remaining[chunk_len..];
        first = false;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Charset conversion ──

    #[test]
    fn latin1_basic() {
        let bytes = [0x48, 0x65, 0x6C, 0x6C, 0x6F]; // "Hello"
        assert_eq!(charset_to_utf8(&bytes, "ISO-8859-1").unwrap(), "Hello");
    }

    #[test]
    fn latin1_high() {
        let bytes = [0xE9, 0xE8, 0xEA]; // éèê in Latin-1
        assert_eq!(charset_to_utf8(&bytes, "ISO-8859-1").unwrap(), "éèê");
    }

    #[test]
    fn cp1252_special() {
        let bytes = [0x93, 0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x94]; // "Hello"
        let result = charset_to_utf8(&bytes, "Windows-1252").unwrap();
        assert_eq!(result, "\u{201C}Hello\u{201D}");
    }

    #[test]
    fn cp1252_euro() {
        assert_eq!(charset_to_utf8(&[0x80], "CP1252").unwrap(), "€");
    }

    #[test]
    fn iso15_euro() {
        assert_eq!(charset_to_utf8(&[0xA4], "ISO-8859-15").unwrap(), "€");
    }

    #[test]
    fn charset_unsupported() {
        assert!(charset_to_utf8(b"test", "EBCDIC").is_none());
    }

    #[test]
    fn charset_supported_check() {
        assert!(charset_supported("UTF-8"));
        assert!(charset_supported("windows-1252"));
        assert!(charset_supported("ISO-8859-1"));
        assert!(!charset_supported("EBCDIC"));
    }

    // ── Quoted-Printable ──

    #[test]
    fn qp_encode_ascii() {
        assert_eq!(qp_encode(b"Hello World"), "Hello World");
    }

    #[test]
    fn qp_encode_special() {
        assert_eq!(qp_encode(b"="), "=3D");
    }

    #[test]
    fn qp_encode_high_byte() {
        assert_eq!(qp_encode(&[0xC0]), "=C0");
    }

    #[test]
    fn qp_decode_basic() {
        assert_eq!(qp_decode("Hello=20World"), b"Hello World");
    }

    #[test]
    fn qp_decode_soft_break() {
        assert_eq!(qp_decode("Hello=\r\nWorld"), b"HelloWorld");
    }

    #[test]
    fn qp_decode_equals() {
        assert_eq!(qp_decode("=3D"), b"=");
    }

    #[test]
    fn qp_roundtrip() {
        let input = b"Hello, World! Special: =, \xC0\xE9";
        let encoded = qp_encode(input);
        let decoded = qp_decode(&encoded);
        assert_eq!(decoded, input);
    }

    // ── RFC 2047 ──

    #[test]
    fn decode_header_plain() {
        assert_eq!(decode_header("Hello World"), "Hello World");
    }

    #[test]
    fn decode_header_base64() {
        assert_eq!(decode_header("=?UTF-8?B?SGVsbG8=?="), "Hello");
    }

    #[test]
    fn decode_header_qp() {
        assert_eq!(decode_header("=?UTF-8?Q?Hello_World?="), "Hello World");
    }

    #[test]
    fn decode_header_latin1() {
        // =?ISO-8859-1?Q?caf=E9?= → "café"
        assert_eq!(decode_header("=?ISO-8859-1?Q?caf=E9?="), "café");
    }

    #[test]
    fn decode_header_adjacent() {
        // Adjacent encoded-words with whitespace between — whitespace should be collapsed
        assert_eq!(
            decode_header("=?UTF-8?B?SGVs?= =?UTF-8?B?bG8=?="),
            "Hello"
        );
    }

    #[test]
    fn decode_header_mixed() {
        assert_eq!(
            decode_header("Re: =?UTF-8?B?SGVsbG8=?= there"),
            "Re: Hello there"
        );
    }

    #[test]
    fn encode_header_ascii() {
        assert_eq!(encode_header("Hello"), "Hello");
    }

    #[test]
    fn encode_header_utf8() {
        let encoded = encode_header("café");
        assert!(encoded.starts_with("=?UTF-8?B?"));
        assert!(encoded.ends_with("?="));
        assert_eq!(decode_header(&encoded), "café");
    }

    #[test]
    fn encode_header_roundtrip() {
        let inputs = &["Hello", "café", "日本語テスト", "Ünïcödé"];
        for &input in inputs {
            let encoded = encode_header(input);
            let decoded = decode_header(&encoded);
            assert_eq!(decoded, input, "roundtrip failed for {}", input);
        }
    }
}
