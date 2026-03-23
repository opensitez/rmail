//! BinHex 4.0 encode and decode.
//!
//! BinHex 4.0 is a legacy Mac encoding that wraps a file (with metadata)
//! in a printable 6-bit encoding with run-length compression and CRC-16
//! integrity checks.
//!
//! # Format overview
//!
//! The outer text looks like:
//!
//! ```text
//! (This file must be converted with BinHex 4.0)
//! :<base64-like encoded data>:
//! ```
//!
//! After decoding the 6-bit encoding and expanding RLE, the binary
//! structure is:
//!
//! | Field               | Size           |
//! |---------------------|----------------|
//! | filename length     | 1 byte         |
//! | filename            | N bytes        |
//! | version             | 1 byte (0)     |
//! | type                | 4 bytes        |
//! | creator             | 4 bytes        |
//! | flags               | 2 bytes        |
//! | data fork length    | 4 bytes (BE)   |
//! | resource fork length| 4 bytes (BE)   |
//! | header CRC          | 2 bytes        |
//! | data fork           | N bytes        |
//! | data CRC            | 2 bytes        |
//! | resource fork       | N bytes        |
//! | resource CRC        | 2 bytes        |
//!
//! We extract the data fork only (the resource fork is Mac-specific).

use std::fmt;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during BinHex processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinHexError {
    /// Input too short to be valid BinHex.
    InputTooShort,
    /// Could not find the opening `:` delimiter.
    NoStartMarker,
    /// Could not find the closing `:` delimiter.
    NoEndMarker,
    /// Decoded binary is too short to contain a valid header.
    HeaderTruncated,
    /// Filename length in the header is invalid (0 or > 63).
    BadFilenameLength,
    /// Data fork is completely missing.
    DataForkMissing,
    /// Encode was called with an empty filename.
    EmptyFilename,
}

impl fmt::Display for BinHexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooShort => write!(f, "input too short for BinHex"),
            Self::NoStartMarker => write!(f, "no opening ':' delimiter found"),
            Self::NoEndMarker => write!(f, "no closing ':' delimiter found"),
            Self::HeaderTruncated => write!(f, "decoded header is truncated"),
            Self::BadFilenameLength => write!(f, "invalid filename length in header"),
            Self::DataForkMissing => write!(f, "data fork missing"),
            Self::EmptyFilename => write!(f, "filename must not be empty"),
        }
    }
}

impl std::error::Error for BinHexError {}

// ---------------------------------------------------------------------------
// Result of a successful decode
// ---------------------------------------------------------------------------

/// The decoded contents of a BinHex 4.0 stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinHexDecoded {
    /// Original filename stored in the BinHex header.
    pub filename: String,
    /// Raw bytes of the data fork.
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// 6-to-8 decode table
// ---------------------------------------------------------------------------

/// BinHex character-to-6-bit-value lookup table.
/// Invalid characters map to `0xFF`.
const BH_DECODE: [u8; 256] = {
    let mut t = [0xFFu8; 256];
    // The valid BinHex charset:
    //  !"#$%&'()*+,-  012345678  9  @ABCDEFGHIJKLMNO
    //  PQRSTUVWXYZ[\]  `abcdefghijklmnopqr
    let chars: &[u8] = b"!\"#$%&'()*+,-012345689@ABCDEFGHIJKLMNPQRSTUVXYZ[`abcdefhijklmpqr";
    let mut i = 0;
    while i < chars.len() {
        t[chars[i] as usize] = i as u8;
        i += 1;
    }
    t
};

// ---------------------------------------------------------------------------
// 6-bit encode table
// ---------------------------------------------------------------------------

/// 6-bit-value-to-character encode table (same mapping as the classic
/// Eudora / StuffIt implementation).
const BH_ENCODE: [u8; 64] = [
    0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x30, 0x31, 0x32,
    0x33, 0x34, 0x35, 0x36, 0x38, 0x39, 0x40, 0x41,
    0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x50, 0x51, 0x52,
    0x53, 0x54, 0x55, 0x56, 0x58, 0x59, 0x5a, 0x5b,
    0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x68,
    0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x70, 0x71, 0x72,
];

// ---------------------------------------------------------------------------
// CRC-16 (BinHex variant)
// ---------------------------------------------------------------------------

/// Compute one step of the BinHex CRC-16.
///
/// The polynomial is 0x1021 with an extra high-bit trick: the accumulator
/// is 17 bits wide and the polynomial applied is `0x10211`.
fn bh_crc(crc: u16, byte: u8) -> u16 {
    let mut temp = crc as u32;
    let mut c = byte;
    for _ in 0..8 {
        temp <<= 1;
        if c & 0x80 != 0 {
            temp |= 1;
        }
        c <<= 1;
        if temp & 0x10000 != 0 {
            temp ^= 0x10211;
        }
    }
    (temp & 0xFFFF) as u16
}

/// Compute the BinHex CRC-16 over a byte slice.
fn bh_crc_bytes(init: u16, data: &[u8]) -> u16 {
    let mut crc = init;
    for &b in data {
        crc = bh_crc(crc, b);
    }
    crc
}

/// Finalize CRC by feeding two zero bytes (standard BinHex finalization).
fn bh_crc_finalize(crc: u16) -> u16 {
    let crc = bh_crc(crc, 0);
    bh_crc(crc, 0)
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// The canonical header that identifies BinHex 4.0 content.
const BINHEX_MAGIC: &[u8] = b"(This file must be converted with BinHex";

/// Returns `true` if `data` appears to contain BinHex 4.0 content.
pub fn detect(data: &[u8]) -> bool {
    if data.len() < 45 {
        return false;
    }
    data.windows(BINHEX_MAGIC.len())
        .any(|w| w == BINHEX_MAGIC)
}

// ---------------------------------------------------------------------------
// Decode internals
// ---------------------------------------------------------------------------

/// First pass: decode the 6-bit BinHex encoding to raw bytes.
fn decode_6to8(encoded: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(encoded.len() * 3 / 4 + 4);
    let mut accum: u32 = 0;
    let mut bits: u32 = 0;

    for &c in encoded {
        match c {
            b'\r' | b'\n' | b' ' | b'\t' => continue,
            b':' => break,
            _ => {
                let val = BH_DECODE[c as usize];
                if val == 0xFF {
                    continue; // invalid char — skip
                }
                accum = (accum << 6) | val as u32;
                bits += 6;
                while bits >= 8 {
                    bits -= 8;
                    out.push(((accum >> bits) & 0xFF) as u8);
                }
            }
        }
    }
    out
}

/// Second pass: expand RLE (run-length encoding).
///
/// `0x90 0x00` is a literal `0x90`.
/// `0x90 N` (N > 0) repeats the previous byte N-1 more times.
fn rle_expand(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() * 2);
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == 0x90 {
            if i + 1 < raw.len() && raw[i + 1] == 0x00 {
                // literal 0x90
                out.push(0x90);
                i += 2;
            } else if i + 1 < raw.len() {
                let count = raw[i + 1] as usize;
                let prev = if out.is_empty() { 0u8 } else { *out.last().unwrap() };
                // repeat prev (count-1) more times
                for _ in 1..count {
                    out.push(prev);
                }
                i += 2;
            } else {
                // trailing 0x90 with no count — just emit it
                out.push(0x90);
                i += 1;
            }
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Public decode
// ---------------------------------------------------------------------------

/// Decode a BinHex 4.0 encoded text, returning the embedded filename and
/// data-fork bytes.
///
/// The resource fork (Mac-specific) is ignored.
///
/// # Errors
///
/// Returns [`BinHexError`] if the input is not valid BinHex or is truncated.
pub fn decode(data: &[u8]) -> Result<BinHexDecoded, BinHexError> {
    if data.len() < 50 {
        return Err(BinHexError::InputTooShort);
    }

    // Find opening ':'
    let start_pos = data.iter().position(|&b| b == b':')
        .ok_or(BinHexError::NoStartMarker)?;
    let encoded = &data[start_pos + 1..];

    // Find closing ':'
    let end_pos = encoded.iter().position(|&b| b == b':')
        .ok_or(BinHexError::NoEndMarker)?;
    if end_pos == 0 {
        return Err(BinHexError::NoEndMarker);
    }
    let encoded = &encoded[..end_pos];

    // 6-to-8 decode, then RLE expand
    let raw6 = decode_6to8(encoded);
    let raw = rle_expand(&raw6);

    if raw.len() < 22 {
        return Err(BinHexError::HeaderTruncated);
    }

    // Parse header
    let name_len = raw[0] as usize;
    if name_len == 0 || name_len > 63 {
        return Err(BinHexError::BadFilenameLength);
    }
    if 1 + name_len + 20 > raw.len() {
        return Err(BinHexError::HeaderTruncated);
    }

    let filename = String::from_utf8_lossy(&raw[1..1 + name_len]).into_owned();

    // Skip: version(1) + type(4) + creator(4) + flags(2) = 11
    let mut off = 1 + name_len + 1 + 10;

    // Data fork length (4 bytes big-endian)
    let df_len = u32::from_be_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]]) as usize;
    off += 4;

    // Resource fork length (skip)
    off += 4;

    // Header CRC (skip)
    off += 2;

    // Data fork
    if off > raw.len() {
        return Err(BinHexError::DataForkMissing);
    }
    let available = raw.len() - off;
    let actual_len = df_len.min(available); // tolerate truncation
    if actual_len == 0 && df_len > 0 {
        return Err(BinHexError::DataForkMissing);
    }

    let fork_data = raw[off..off + actual_len].to_vec();

    Ok(BinHexDecoded {
        filename,
        data: fork_data,
    })
}

// ---------------------------------------------------------------------------
// Encode internals
// ---------------------------------------------------------------------------

/// Encoder state machine — mirrors the C `BHEnc` struct.
struct Encoder {
    out: Vec<u8>,
    state86: u8,      // 0, 1, or 2
    saved_bits: u8,
    line_len: usize,
    crc: u16,
}

impl Encoder {
    fn new(capacity: usize) -> Self {
        Self {
            out: Vec::with_capacity(capacity),
            state86: 0,
            saved_bits: 0,
            line_len: 0,
            crc: 0,
        }
    }

    /// Emit one encoded character, wrapping lines at 64 characters.
    fn emit_char(&mut self, encoded: u8) {
        self.out.push(encoded);
        self.line_len += 1;
        if self.line_len >= 64 {
            self.out.push(b'\r');
            self.out.push(b'\n');
            self.line_len = 0;
        }
    }

    /// Encode one raw byte into the 6-bit output stream.
    fn encode_byte(&mut self, c: u8) {
        match self.state86 {
            0 => {
                self.emit_char(BH_ENCODE[(c >> 2) as usize]);
                self.saved_bits = (c & 0x03) << 4;
                self.state86 = 1;
            }
            1 => {
                self.emit_char(BH_ENCODE[(self.saved_bits | (c >> 4)) as usize]);
                self.saved_bits = (c & 0x0F) << 2;
                self.state86 = 2;
            }
            2 => {
                self.emit_char(BH_ENCODE[(self.saved_bits | (c >> 6)) as usize]);
                self.emit_char(BH_ENCODE[(c & 0x3F) as usize]);
                self.state86 = 0;
            }
            _ => unreachable!(),
        }
    }

    /// Encode a byte with RLE escaping and CRC update.
    fn code(&mut self, c: u8) {
        self.encode_byte(c);
        if c == 0x90 {
            self.encode_byte(0x00); // escape literal 0x90
        }
        self.crc = bh_crc(self.crc, c);
    }

    /// Encode a big-endian u16.
    fn code_u16(&mut self, v: u16) {
        self.code((v >> 8) as u8);
        self.code((v & 0xFF) as u8);
    }

    /// Encode a big-endian u32.
    fn code_u32(&mut self, v: u32) {
        self.code(((v >> 24) & 0xFF) as u8);
        self.code(((v >> 16) & 0xFF) as u8);
        self.code(((v >> 8) & 0xFF) as u8);
        self.code((v & 0xFF) as u8);
    }

    /// Finalize the current CRC (feed two zero bytes), emit it, then
    /// reset CRC to zero.
    fn emit_crc(&mut self) {
        let finalized = bh_crc_finalize(self.crc);
        self.code_u16(finalized);
        self.crc = 0;
    }

    /// Flush any remaining bits and emit the closing `:` delimiter.
    fn finish(mut self) -> Vec<u8> {
        if self.state86 != 0 {
            self.encode_byte(0);
        }
        self.out.push(b':');
        self.out.push(b'\r');
        self.out.push(b'\n');
        self.out
    }
}

// ---------------------------------------------------------------------------
// Public encode
// ---------------------------------------------------------------------------

/// Encode binary data as BinHex 4.0.
///
/// The output includes the standard header line and colon delimiters.
/// Type and creator are set to `????` (generic/portable).
/// The resource fork is empty.
///
/// # Errors
///
/// Returns [`BinHexError::EmptyFilename`] if `filename` is empty.
pub fn encode(filename: &str, data: &[u8]) -> Result<Vec<u8>, BinHexError> {
    if filename.is_empty() {
        return Err(BinHexError::EmptyFilename);
    }

    let name_bytes = filename.as_bytes();
    let name_len = name_bytes.len().min(63);

    let mut enc = Encoder::new(data.len() * 2 + 1024);

    // Header line
    let hdr = b"(This file must be converted with BinHex 4.0)\r\n:";
    enc.out.extend_from_slice(hdr);
    enc.line_len = 1; // after the ':'

    // Filename (length-prefixed)
    enc.code(name_len as u8);
    for &b in &name_bytes[..name_len] {
        enc.code(b);
    }
    enc.code(0); // version

    // Type and creator — generic '????'
    enc.code_u32(0x3F3F3F3F);
    enc.code_u32(0x3F3F3F3F);
    enc.code_u16(0); // flags

    // Fork lengths
    enc.code_u32(data.len() as u32); // data fork
    enc.code_u32(0);                  // resource fork (none)

    // Header CRC
    enc.emit_crc();

    // Data fork
    for &b in data {
        enc.code(b);
    }

    // Data fork CRC
    enc.emit_crc();

    // Empty resource fork CRC (CRC of nothing, finalized)
    enc.emit_crc();

    Ok(enc.finish())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- CRC ----------------------------------------------------------------

    #[test]
    fn crc_single_byte() {
        assert_eq!(bh_crc(0, 0), 0);
        // Known value: CRC of 'A' starting from 0
        let c = bh_crc(0, b'A');
        assert_ne!(c, 0);
        // Running the same byte twice should differ from once
        let c2 = bh_crc(c, b'A');
        assert_ne!(c, c2);
    }

    #[test]
    fn crc_bytes_matches_stepwise() {
        let data = b"Hello, BinHex!";
        let step = data.iter().fold(0u16, |acc, &b| bh_crc(acc, b));
        let bulk = bh_crc_bytes(0, data);
        assert_eq!(step, bulk);
    }

    #[test]
    fn crc_finalize_feeds_two_zeros() {
        let crc = bh_crc_bytes(0, b"test");
        let finalized = bh_crc_finalize(crc);
        let manual = bh_crc(bh_crc(crc, 0), 0);
        assert_eq!(finalized, manual);
    }

    // -- Detect -------------------------------------------------------------

    #[test]
    fn detect_valid() {
        let input = b"(This file must be converted with BinHex 4.0)\r\n:stuff:";
        assert!(detect(input));
    }

    #[test]
    fn detect_too_short() {
        assert!(!detect(b"(This file"));
    }

    #[test]
    fn detect_no_header() {
        let input = b"This is just a normal text file with enough length to pass the minimum.";
        assert!(!detect(input));
    }

    // -- 6-to-8 decode table consistency ------------------------------------

    #[test]
    fn decode_encode_tables_consistent() {
        // Every value the encode table produces must decode back to its index.
        for (i, &enc_byte) in BH_ENCODE.iter().enumerate() {
            let decoded = BH_DECODE[enc_byte as usize];
            assert_eq!(
                decoded, i as u8,
                "BH_ENCODE[{}] = 0x{:02X}, but BH_DECODE[0x{:02X}] = {}",
                i, enc_byte, enc_byte, decoded
            );
        }
    }

    // -- RLE ----------------------------------------------------------------

    #[test]
    fn rle_literal_0x90() {
        // 0x90 0x00 → literal 0x90
        assert_eq!(rle_expand(&[0x41, 0x90, 0x00, 0x42]), vec![0x41, 0x90, 0x42]);
    }

    #[test]
    fn rle_repeat() {
        // 0x41 0x90 0x05 → 0x41 repeated 5 times total (4 more)
        assert_eq!(
            rle_expand(&[0x41, 0x90, 0x05]),
            vec![0x41, 0x41, 0x41, 0x41, 0x41]
        );
    }

    #[test]
    fn rle_no_run() {
        let data = vec![1, 2, 3, 4, 5];
        assert_eq!(rle_expand(&data), data);
    }

    // -- Round-trip ----------------------------------------------------------

    #[test]
    fn roundtrip_simple() {
        let original = b"Hello, world!";
        let encoded = encode("test.txt", original).unwrap();

        // Should be detectable
        assert!(detect(&encoded));

        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.filename, "test.txt");
        assert_eq!(decoded.data, original);
    }

    #[test]
    fn roundtrip_binary_data() {
        // Include bytes that stress RLE (0x90) and other edge cases
        let mut data = Vec::new();
        for i in 0..=255u8 {
            data.push(i);
        }
        // Add a run of 0x90 bytes
        for _ in 0..10 {
            data.push(0x90);
        }

        let encoded = encode("binary.dat", &data).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.filename, "binary.dat");
        assert_eq!(decoded.data, data);
    }

    #[test]
    fn roundtrip_empty_data() {
        let encoded = encode("empty.bin", b"").unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.filename, "empty.bin");
        assert!(decoded.data.is_empty());
    }

    #[test]
    fn roundtrip_long_filename() {
        let name = "a".repeat(100); // will be truncated to 63
        let encoded = encode(&name, b"data").unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.filename.len(), 63);
        assert_eq!(decoded.data, b"data");
    }

    // -- Error cases --------------------------------------------------------

    #[test]
    fn decode_too_short() {
        assert_eq!(decode(b"short").unwrap_err(), BinHexError::InputTooShort);
    }

    #[test]
    fn decode_no_colon() {
        let input = [b'A'; 100];
        assert_eq!(decode(&input).unwrap_err(), BinHexError::NoStartMarker);
    }

    #[test]
    fn encode_empty_filename() {
        assert_eq!(encode("", b"data").unwrap_err(), BinHexError::EmptyFilename);
    }

    // -- Encode output format -----------------------------------------------

    #[test]
    fn encode_starts_with_header() {
        let encoded = encode("f.txt", b"x").unwrap();
        let text = String::from_utf8_lossy(&encoded);
        assert!(text.starts_with("(This file must be converted with BinHex 4.0)\r\n:"));
        assert!(text.trim_end().ends_with(':'));
    }

    #[test]
    fn encode_lines_not_too_long() {
        let data = vec![0x42; 500];
        let encoded = encode("big.bin", &data).unwrap();
        let text = String::from_utf8_lossy(&encoded);
        for line in text.split("\r\n") {
            // Allow the header line and empty trailing line
            assert!(
                line.len() <= 80,
                "line too long ({} chars): {:?}",
                line.len(),
                &line[..line.len().min(80)]
            );
        }
    }
}
