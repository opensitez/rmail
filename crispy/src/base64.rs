//! Base64 encoding and decoding (RFC 4648).

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_decode_byte(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Encode bytes to base64 string.
pub fn encode(input: &[u8]) -> String {
    let mut out = Vec::with_capacity(4 * ((input.len() + 2) / 3));

    for chunk in input.chunks(3) {
        let a = chunk[0] as u32;
        let b = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let c = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (a << 16) | (b << 8) | c;

        out.push(B64_TABLE[((triple >> 18) & 0x3f) as usize]);
        out.push(B64_TABLE[((triple >> 12) & 0x3f) as usize]);
        out.push(if chunk.len() > 1 { B64_TABLE[((triple >> 6) & 0x3f) as usize] } else { b'=' });
        out.push(if chunk.len() > 2 { B64_TABLE[(triple & 0x3f) as usize] } else { b'=' });
    }

    // SAFETY: B64_TABLE and '=' are all ASCII
    unsafe { String::from_utf8_unchecked(out) }
}

/// Decode base64 string to bytes. Skips whitespace and invalid characters.
pub fn decode(input: &str) -> Result<Vec<u8>, DecodeError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(3 * (bytes.len() / 4));
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    let mut pad: usize = 0;

    for &c in bytes {
        if c == b'=' {
            pad += 1;
            buf <<= 6;
            bits += 6;
        } else if let Some(val) = b64_decode_byte(c) {
            buf = (buf << 6) | val as u32;
            bits += 6;
        } else if c == b' ' || c == b'\t' || c == b'\r' || c == b'\n' {
            continue;
        } else {
            continue; // skip invalid chars like the C version
        }

        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }

    // Remove padding bytes
    if pad > 0 && out.len() >= pad {
        out.truncate(out.len() - pad);
    }

    Ok(out)
}

/// Error type for base64 decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    InvalidInput,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::InvalidInput => write!(f, "invalid base64 input"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_empty() {
        assert_eq!(encode(b""), "");
    }

    #[test]
    fn encode_one_byte() {
        assert_eq!(encode(b"f"), "Zg==");
    }

    #[test]
    fn encode_two_bytes() {
        assert_eq!(encode(b"fo"), "Zm8=");
    }

    #[test]
    fn encode_three_bytes() {
        assert_eq!(encode(b"foo"), "Zm9v");
    }

    #[test]
    fn encode_hello_world() {
        assert_eq!(encode(b"Hello, World!"), "SGVsbG8sIFdvcmxkIQ==");
    }

    #[test]
    fn encode_binary() {
        assert_eq!(encode(&[0x00, 0xFF, 0x80]), "AP+A");
    }

    #[test]
    fn decode_empty() {
        assert_eq!(decode("").unwrap(), b"");
    }

    #[test]
    fn decode_padded() {
        assert_eq!(decode("Zg==").unwrap(), b"f");
        assert_eq!(decode("Zm8=").unwrap(), b"fo");
        assert_eq!(decode("Zm9v").unwrap(), b"foo");
    }

    #[test]
    fn decode_hello_world() {
        assert_eq!(decode("SGVsbG8sIFdvcmxkIQ==").unwrap(), b"Hello, World!");
    }

    #[test]
    fn decode_with_whitespace() {
        assert_eq!(decode("SGVs\nbG8s\r\nIFdv\tcmxkIQ==").unwrap(), b"Hello, World!");
    }

    #[test]
    fn roundtrip() {
        let inputs: &[&[u8]] = &[
            b"",
            b"a",
            b"ab",
            b"abc",
            b"The quick brown fox jumps over the lazy dog",
            &[0u8; 256],
            &(0..=255).collect::<Vec<u8>>(),
        ];
        for input in inputs {
            let encoded = encode(input);
            let decoded = decode(&encoded).unwrap();
            assert_eq!(&decoded, input, "roundtrip failed for {:?}", input);
        }
    }

    #[test]
    fn rfc4648_vectors() {
        // Test vectors from RFC 4648
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }
}
