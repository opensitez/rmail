//! UUencode / UUdecode
//!
//! Encodes and decodes the classic Unix-to-Unix encoding format:
//!
//! ```text
//! begin 644 filename
//! M<encoded lines>
//! `
//! end
//! ```
//!
//! Standalone — no external crate dependencies.

/// Result of a successful UU decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UuDecoded {
    /// The decoded binary data.
    pub data: Vec<u8>,
    /// The filename extracted from the `begin` header, if present.
    pub filename: Option<String>,
}

/// Errors that can occur during UU decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UuError {
    /// Input is empty or too short to contain a valid UU block.
    InputTooShort,
    /// No `begin NNN filename` header was found.
    NoBeginLine,
    /// Encoded line is truncated (not enough characters for the declared byte count).
    TruncatedLine,
}

impl std::fmt::Display for UuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UuError::InputTooShort => write!(f, "input too short for UU data"),
            UuError::NoBeginLine => write!(f, "no 'begin' header found"),
            UuError::TruncatedLine => write!(f, "truncated encoded line"),
        }
    }
}

impl std::error::Error for UuError {}

/// Decode a single 6-bit UU character back to its value.
#[inline]
fn dec(c: u8) -> u8 {
    (c.wrapping_sub(b' ')) & 0o77
}

/// Encode a 6-bit value to a UU character.
#[inline]
fn enc(c: u8) -> u8 {
    (c & 0o77) + b' '
}

/// Detect whether `data` contains a UU-encoded block.
///
/// Scans for a line starting with `begin ` followed by an octal mode digit.
/// Returns `true` if such a line is found.
///
/// # Examples
///
/// ```
/// use crispy::uudecode::detect;
///
/// let encoded = b"begin 644 test.txt\n#0V%T\n`\nend\n";
/// assert!(detect(encoded));
/// assert!(!detect(b"no uu data here"));
/// ```
pub fn detect(data: &[u8]) -> bool {
    if data.len() < 10 {
        return false;
    }
    for line in data.split(|&b| b == b'\n') {
        if line.len() >= 7
            && line.starts_with(b"begin ")
            && line[6].is_ascii_digit()
        {
            return true;
        }
    }
    false
}

/// Decode UU-encoded data, returning the binary payload and optional filename.
///
/// Expects standard UU format with `begin`/`end` framing.
///
/// # Errors
///
/// Returns [`UuError::InputTooShort`] if the input is under 10 bytes,
/// [`UuError::NoBeginLine`] if no valid `begin` header is found, or
/// [`UuError::TruncatedLine`] if an encoded line lacks enough characters.
///
/// # Examples
///
/// ```
/// use crispy::uudecode::decode;
///
/// let encoded = b"begin 644 hello.txt\n#2&5L\n`\nend\n";
/// let result = decode(encoded).unwrap();
/// assert_eq!(result.filename.as_deref(), Some("hello.txt"));
/// ```
pub fn decode(data: &[u8]) -> Result<UuDecoded, UuError> {
    if data.len() < 10 {
        return Err(UuError::InputTooShort);
    }

    let mut pos = 0;
    let end = data.len();
    let mut filename: Option<String> = None;

    // Find "begin NNN filename" line
    loop {
        if pos >= end.saturating_sub(10) {
            return Err(UuError::NoBeginLine);
        }
        let line_start = pos;
        // Find end of line
        let line_end = data[pos..].iter().position(|&b| b == b'\n')
            .map(|i| pos + i)
            .unwrap_or(end);

        let line = &data[line_start..line_end];
        if line.len() >= 7 && line.starts_with(b"begin ") && line[6].is_ascii_digit() {
            // Skip past "begin " then digits then spaces to get filename
            let mut i = 6;
            while i < line.len() && line[i].is_ascii_digit() {
                i += 1;
            }
            while i < line.len() && line[i] == b' ' {
                i += 1;
            }
            // Trim trailing \r
            let mut fname_end = line.len();
            while fname_end > i && data[line_start + fname_end - 1] == b'\r' {
                fname_end -= 1;
            }
            if fname_end > i {
                filename = Some(String::from_utf8_lossy(&line[i..fname_end]).into_owned());
            }
            pos = line_end;
            // Skip past \n
            if pos < end && data[pos] == b'\n' {
                pos += 1;
            }
            break;
        }

        pos = line_end;
        if pos < end {
            pos += 1; // skip \n
        }
    }

    // Decode lines
    let mut out: Vec<u8> = Vec::with_capacity(data.len());

    while pos < end {
        // Check for "end" line
        if data[pos..].starts_with(b"end")
            && (pos + 3 >= end
                || data[pos + 3] == b'\n'
                || data[pos + 3] == b'\r')
        {
            break;
        }

        // Backtick or blank line — skip
        if data[pos] == b'`' || data[pos] == b'\n' || data[pos] == b'\r' {
            while pos < end && data[pos] != b'\n' {
                pos += 1;
            }
            if pos < end {
                pos += 1;
            }
            continue;
        }

        // First character is the count of decoded bytes on this line
        let n = dec(data[pos]) as usize;
        pos += 1;

        let mut remaining = n;
        while remaining > 0 {
            if pos + 3 >= end {
                return Err(UuError::TruncatedLine);
            }
            let c0 = dec(data[pos]);
            let c1 = dec(data[pos + 1]);
            let c2 = dec(data[pos + 2]);
            let c3 = dec(data[pos + 3]);

            if remaining >= 1 {
                out.push((c0 << 2) | (c1 >> 4));
            }
            if remaining >= 2 {
                out.push((c1 << 4) | (c2 >> 2));
            }
            if remaining >= 3 {
                out.push((c2 << 6) | c3);
            }

            pos += 4;
            remaining = remaining.saturating_sub(3);
        }

        // Skip to next line
        while pos < end && data[pos] != b'\n' {
            pos += 1;
        }
        if pos < end {
            pos += 1;
        }
    }

    Ok(UuDecoded {
        data: out,
        filename,
    })
}

/// Encode binary data into UU format with the given filename.
///
/// Produces a complete UU block including `begin 644 <filename>` header,
/// encoded body (45 raw bytes per line), and `end` footer.
///
/// # Examples
///
/// ```
/// use crispy::uudecode::{encode, decode};
///
/// let original = b"Hello, world!";
/// let encoded = encode("test.bin", original);
/// let decoded = decode(encoded.as_bytes()).unwrap();
/// assert_eq!(decoded.data, original);
/// assert_eq!(decoded.filename.as_deref(), Some("test.bin"));
/// ```
pub fn encode(filename: &str, data: &[u8]) -> String {
    let capacity = (data.len() * 4 / 3) + (data.len() / 45) * 2 + 256;
    let mut out: Vec<u8> = Vec::with_capacity(capacity);

    // Header
    out.extend_from_slice(b"begin 644 ");
    out.extend_from_slice(filename.as_bytes());
    out.push(b'\n');

    // Encode 45 bytes per line
    let mut offset = 0;
    while offset < data.len() {
        let n = std::cmp::min(45, data.len() - offset);

        // Length character
        out.push(enc(n as u8));

        // Encode 3 bytes at a time into 4 characters
        let mut i = 0;
        while i < n {
            let c0 = data[offset + i];
            let c1 = if i + 1 < n { data[offset + i + 1] } else { 0 };
            let c2 = if i + 2 < n { data[offset + i + 2] } else { 0 };

            out.push(enc(c0 >> 2));
            out.push(enc(((c0 << 4) & 0o60) | ((c1 >> 4) & 0o17)));
            out.push(enc(((c1 << 2) & 0o74) | ((c2 >> 6) & 0o03)));
            out.push(enc(c2 & 0o77));

            i += 3;
        }

        out.push(b'\n');
        offset += n;
    }

    // Footer: zero-length line (backtick) + "end"
    out.extend_from_slice(b"`\nend\n");

    // All UU characters are ASCII, so this is safe.
    String::from_utf8(out).expect("UU output is always valid ASCII")
}

// ================================================================
// Tests
// ================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- detect ----------------------------------------------------------

    #[test]
    fn detect_valid() {
        assert!(detect(b"begin 644 file.txt\nM...\n`\nend\n"));
    }

    #[test]
    fn detect_with_leading_text() {
        assert!(detect(b"some preamble\nbegin 755 data.bin\nM...\n`\nend\n"));
    }

    #[test]
    fn detect_no_uu() {
        assert!(!detect(b"just some random text without uu data"));
    }

    #[test]
    fn detect_too_short() {
        assert!(!detect(b"begin 6"));
    }

    #[test]
    fn detect_begin_no_digit() {
        assert!(!detect(b"begin xyz filename\ndata\nend\n"));
    }

    // -- decode ----------------------------------------------------------

    #[test]
    fn decode_simple() {
        // "Cat" encoded
        let input = b"begin 644 test.txt\n#0V%T\n`\nend\n";
        let result = decode(input).unwrap();
        assert_eq!(result.data, b"Cat");
        assert_eq!(result.filename.as_deref(), Some("test.txt"));
    }

    #[test]
    fn decode_empty_payload() {
        let input = b"begin 644 empty.bin\n`\nend\n";
        let result = decode(input).unwrap();
        assert!(result.data.is_empty());
        assert_eq!(result.filename.as_deref(), Some("empty.bin"));
    }

    #[test]
    fn decode_input_too_short() {
        assert_eq!(decode(b"short"), Err(UuError::InputTooShort));
    }

    #[test]
    fn decode_no_begin() {
        let input = b"this has no begin header at all, none whatsoever!";
        assert_eq!(decode(input), Err(UuError::NoBeginLine));
    }

    #[test]
    fn decode_with_preamble() {
        let input = b"some header junk\nbegin 644 f.dat\n#0V%T\n`\nend\n";
        let result = decode(input).unwrap();
        assert_eq!(result.data, b"Cat");
    }

    #[test]
    fn decode_filename_with_spaces_trimmed_cr() {
        let input = b"begin 644 my file.txt\r\n#0V%T\n`\nend\n";
        let result = decode(input).unwrap();
        assert_eq!(result.filename.as_deref(), Some("my file.txt"));
    }

    // -- encode ----------------------------------------------------------

    #[test]
    fn encode_basic_structure() {
        let encoded = encode("foo.bin", b"AB");
        assert!(encoded.starts_with("begin 644 foo.bin\n"));
        assert!(encoded.ends_with("`\nend\n"));
    }

    #[test]
    fn encode_empty() {
        let encoded = encode("empty.dat", b"");
        assert_eq!(encoded, "begin 644 empty.dat\n`\nend\n");
    }

    // -- round-trip ------------------------------------------------------

    #[test]
    fn roundtrip_short() {
        let original = b"Hello, world!";
        let encoded = encode("hello.txt", original);
        let decoded = decode(encoded.as_bytes()).unwrap();
        assert_eq!(decoded.data, original);
        assert_eq!(decoded.filename.as_deref(), Some("hello.txt"));
    }

    #[test]
    fn roundtrip_exact_45_bytes() {
        // Exactly one full UU line
        let original: Vec<u8> = (0..45).collect();
        let encoded = encode("line.bin", &original);
        let decoded = decode(encoded.as_bytes()).unwrap();
        assert_eq!(decoded.data, original);
    }

    #[test]
    fn roundtrip_multi_line() {
        // More than 45 bytes forces multiple encoded lines
        let original: Vec<u8> = (0..=255).cycle().take(200).collect();
        let encoded = encode("big.bin", &original);
        let decoded = decode(encoded.as_bytes()).unwrap();
        assert_eq!(decoded.data, original);
    }

    #[test]
    fn roundtrip_binary_all_bytes() {
        let original: Vec<u8> = (0..=255).collect();
        let encoded = encode("allbytes.bin", &original);
        let decoded = decode(encoded.as_bytes()).unwrap();
        assert_eq!(decoded.data, original);
    }

    #[test]
    fn roundtrip_large() {
        let original: Vec<u8> = (0..10_000).map(|i| (i & 0xFF) as u8).collect();
        let encoded = encode("large.dat", &original);
        let decoded = decode(encoded.as_bytes()).unwrap();
        assert_eq!(decoded.data, original);
    }

    #[test]
    fn roundtrip_single_byte() {
        for b in 0..=255u8 {
            let original = [b];
            let encoded = encode("byte.bin", &original);
            let decoded = decode(encoded.as_bytes()).unwrap();
            assert_eq!(decoded.data, original.to_vec(), "failed for byte {}", b);
        }
    }

    // -- encode line structure -------------------------------------------

    #[test]
    fn encode_line_lengths() {
        // 90 bytes = two lines: 45 + 45
        let data: Vec<u8> = vec![0x41; 90];
        let encoded = encode("a.bin", &data);
        let lines: Vec<&str> = encoded.lines().collect();
        // header, line1, line2, backtick, end
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "begin 644 a.bin");
        assert_eq!(lines[3], "`");
        assert_eq!(lines[4], "end");
        // Each data line should start with 'M' (enc(45) = ' ' + 45 = 'M')
        assert!(lines[1].starts_with('M'));
        assert!(lines[2].starts_with('M'));
    }
}
