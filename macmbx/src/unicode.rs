//! Unicode utilities: UTF-8 validation, encoding conversion tables, charset detection,
//! RFC 2047 MIME encoded-word, ISO-2022 cleaning, Content-Type charset extraction.
//!
//! Ported from macmbx_unicode.c — standalone, no external dependencies.

// ---------------------------------------------------------------------------
// Charset IDs
// ---------------------------------------------------------------------------

pub const CHARSET_UNKNOWN: i32 = 0;
pub const CHARSET_US_ASCII: i32 = 1;
pub const CHARSET_UTF8: i32 = 2;
pub const CHARSET_UTF16LE: i32 = 3;
pub const CHARSET_UTF16BE: i32 = 4;
pub const CHARSET_UTF32LE: i32 = 5;
pub const CHARSET_UTF32BE: i32 = 6;
pub const CHARSET_ISO_8859_1: i32 = 7;
pub const CHARSET_ISO_8859_2: i32 = 8;
pub const CHARSET_ISO_8859_3: i32 = 9;
pub const CHARSET_ISO_8859_4: i32 = 10;
pub const CHARSET_ISO_8859_5: i32 = 11;
pub const CHARSET_ISO_8859_6: i32 = 12;
pub const CHARSET_ISO_8859_7: i32 = 13;
pub const CHARSET_ISO_8859_8: i32 = 14;
pub const CHARSET_ISO_8859_9: i32 = 15;
pub const CHARSET_ISO_8859_10: i32 = 16;
pub const CHARSET_ISO_8859_11: i32 = 17;
pub const CHARSET_ISO_8859_13: i32 = 18;
pub const CHARSET_ISO_8859_14: i32 = 19;
pub const CHARSET_ISO_8859_15: i32 = 20;
pub const CHARSET_WINDOWS_1250: i32 = 21;
pub const CHARSET_WINDOWS_1251: i32 = 22;
pub const CHARSET_WINDOWS_1252: i32 = 23;
pub const CHARSET_WINDOWS_1253: i32 = 24;
pub const CHARSET_WINDOWS_1254: i32 = 25;
pub const CHARSET_WINDOWS_1255: i32 = 26;
pub const CHARSET_WINDOWS_1256: i32 = 27;
pub const CHARSET_WINDOWS_1257: i32 = 28;
pub const CHARSET_WINDOWS_1258: i32 = 29;
pub const CHARSET_MAC_ROMAN: i32 = 30;

// ---------------------------------------------------------------------------
// UTF-8 codec
// ---------------------------------------------------------------------------

/// Encode one codepoint to UTF-8. Returns number of bytes written, 0 on error.
pub fn utf8_encode(cp: u32, out: &mut [u8]) -> usize {
    if cp < 0x80 {
        if out.is_empty() { return 0; }
        out[0] = cp as u8;
        1
    } else if cp < 0x800 {
        if out.len() < 2 { return 0; }
        out[0] = 0xC0 | (cp >> 6) as u8;
        out[1] = 0x80 | (cp & 0x3F) as u8;
        2
    } else if cp < 0x10000 {
        if out.len() < 3 { return 0; }
        out[0] = 0xE0 | (cp >> 12) as u8;
        out[1] = 0x80 | ((cp >> 6) & 0x3F) as u8;
        out[2] = 0x80 | (cp & 0x3F) as u8;
        3
    } else if cp <= 0x10FFFF {
        if out.len() < 4 { return 0; }
        out[0] = 0xF0 | (cp >> 18) as u8;
        out[1] = 0x80 | ((cp >> 12) & 0x3F) as u8;
        out[2] = 0x80 | ((cp >> 6) & 0x3F) as u8;
        out[3] = 0x80 | (cp & 0x3F) as u8;
        4
    } else {
        0
    }
}

/// Encode a codepoint to a UTF-8 String (convenience).
pub fn utf8_encode_char(cp: u32) -> Option<String> {
    let mut buf = [0u8; 4];
    let n = utf8_encode(cp, &mut buf);
    if n == 0 { return None; }
    String::from_utf8(buf[..n].to_vec()).ok()
}

/// Decode one codepoint from UTF-8 bytes. Returns (codepoint, bytes_consumed).
/// Returns (0xFFFD, 1) on invalid sequence.
pub fn utf8_decode(data: &[u8]) -> (u32, usize) {
    if data.is_empty() { return (0xFFFD, 0); }
    let b0 = data[0];
    let (mut cp, trail): (u32, usize) = if b0 < 0x80 {
        (b0 as u32, 0)
    } else if b0 & 0xE0 == 0xC0 {
        ((b0 & 0x1F) as u32, 1)
    } else if b0 & 0xF0 == 0xE0 {
        ((b0 & 0x0F) as u32, 2)
    } else if b0 & 0xF8 == 0xF0 {
        ((b0 & 0x07) as u32, 3)
    } else {
        return (0xFFFD, 1);
    };

    if data.len() < 1 + trail { return (0xFFFD, 1); }

    for i in 0..trail {
        if data[1 + i] & 0xC0 != 0x80 {
            return (0xFFFD, 1);
        }
        cp = (cp << 6) | (data[1 + i] & 0x3F) as u32;
    }

    // Overlong and surrogate checks
    if (trail == 1 && cp < 0x80) || (trail == 2 && cp < 0x800)
        || (trail == 3 && cp < 0x10000) || (cp >= 0xD800 && cp <= 0xDFFF)
        || cp > 0x10FFFF
    {
        return (0xFFFD, 1);
    }

    (cp, 1 + trail)
}

/// Count codepoints in a UTF-8 byte slice.
pub fn utf8_len(data: &[u8]) -> usize {
    let mut count = 0;
    let mut pos = 0;
    while pos < data.len() {
        let (_, n) = utf8_decode(&data[pos..]);
        if n == 0 { break; }
        pos += n;
        count += 1;
    }
    count
}

/// Find the longest valid UTF-8 prefix length (no split chars at end).
pub fn utf8_valid_len(data: &[u8]) -> usize {
    let mut last_good = 0;
    let mut pos = 0;
    while pos < data.len() {
        let b = data[pos];
        let seq_len = if b < 0x80 { 1 }
            else if b & 0xE0 == 0xC0 { 2 }
            else if b & 0xF0 == 0xE0 { 3 }
            else if b & 0xF8 == 0xF0 { 4 }
            else { break };
        if pos + seq_len > data.len() { break; }
        let mut valid = true;
        for i in 1..seq_len {
            if data[pos + i] & 0xC0 != 0x80 { valid = false; break; }
        }
        if !valid { break; }
        pos += seq_len;
        last_good = pos;
    }
    last_good
}

/// Check if entire buffer is valid UTF-8.
pub fn utf8_validate(data: &[u8]) -> bool {
    utf8_valid_len(data) == data.len()
}

/// Check if data is pure 7-bit ASCII.
pub fn is_ascii(data: &[u8]) -> bool {
    data.iter().all(|&b| b < 0x80)
}

/// Alias for utf8_validate.
pub fn is_utf8(data: &[u8]) -> bool {
    utf8_validate(data)
}

// ---------------------------------------------------------------------------
// Charset conversion tables
// ---------------------------------------------------------------------------

static WIN1252_80_9F: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x017D, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

static MAC_ROMAN: [u16; 128] = [
    0x00C4, 0x00C5, 0x00C7, 0x00C9, 0x00D1, 0x00D6, 0x00DC, 0x00E1,
    0x00E0, 0x00E2, 0x00E4, 0x00E3, 0x00E5, 0x00E7, 0x00E9, 0x00E8,
    0x00EA, 0x00EB, 0x00ED, 0x00EC, 0x00EE, 0x00EF, 0x00F1, 0x00F3,
    0x00F2, 0x00F4, 0x00F6, 0x00F5, 0x00FA, 0x00F9, 0x00FB, 0x00FC,
    0x2020, 0x00B0, 0x00A2, 0x00A3, 0x00A7, 0x2022, 0x00B6, 0x00DF,
    0x00AE, 0x00A9, 0x2122, 0x00B4, 0x00A8, 0x2260, 0x00C6, 0x00D8,
    0x221E, 0x00B1, 0x2264, 0x2265, 0x00A5, 0x00B5, 0x2202, 0x2211,
    0x220F, 0x03C0, 0x222B, 0x00AA, 0x00BA, 0x03A9, 0x00E6, 0x00F8,
    0x00BF, 0x00A1, 0x00AC, 0x221A, 0x0192, 0x2248, 0x2206, 0x00AB,
    0x00BB, 0x2026, 0x00A0, 0x00C0, 0x00C3, 0x00D5, 0x0152, 0x0153,
    0x2013, 0x2014, 0x201C, 0x201D, 0x2018, 0x2019, 0x00F7, 0x25CA,
    0x00FF, 0x0178, 0x2044, 0x20AC, 0x2039, 0x203A, 0xFB01, 0xFB02,
    0x2021, 0x00B7, 0x201A, 0x201E, 0x2030, 0x00C2, 0x00CA, 0x00C1,
    0x00CB, 0x00C8, 0x00CD, 0x00CE, 0x00CF, 0x00CC, 0x00D3, 0x00D4,
    0xF8FF, 0x00D2, 0x00DA, 0x00DB, 0x00D9, 0x0131, 0x02C6, 0x02DC,
    0x00AF, 0x02D8, 0x02D9, 0x02DA, 0x00B8, 0x02DD, 0x02DB, 0x02C7,
];

static ISO8859_2: [u16; 96] = [
    0x00A0, 0x0104, 0x02D8, 0x0141, 0x00A4, 0x013D, 0x015A, 0x00A7,
    0x00A8, 0x0160, 0x015E, 0x0164, 0x0179, 0x00AD, 0x017D, 0x017B,
    0x00B0, 0x0105, 0x02DB, 0x0142, 0x00B4, 0x013E, 0x015B, 0x02C7,
    0x00B8, 0x0161, 0x015F, 0x0165, 0x017A, 0x02DD, 0x017E, 0x017C,
    0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E,
    0x0110, 0x0143, 0x0147, 0x00D3, 0x00D4, 0x0150, 0x00D6, 0x00D7,
    0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF,
    0x0155, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7,
    0x010D, 0x00E9, 0x0119, 0x00EB, 0x011B, 0x00ED, 0x00EE, 0x010F,
    0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7,
    0x0159, 0x016F, 0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];

static ISO8859_3: [u16; 96] = [
    0x00A0, 0x0126, 0x02D8, 0x00A3, 0x00A4, 0x0000, 0x0124, 0x00A7,
    0x00A8, 0x0130, 0x015E, 0x011E, 0x0134, 0x00AD, 0x0000, 0x017B,
    0x00B0, 0x0127, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x0125, 0x00B7,
    0x00B8, 0x0131, 0x015F, 0x011F, 0x0135, 0x00BD, 0x0000, 0x017C,
    0x00C0, 0x00C1, 0x00C2, 0x0000, 0x00C4, 0x010A, 0x0108, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x0000, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x0120, 0x00D6, 0x00D7,
    0x011C, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x016C, 0x015C, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x0000, 0x00E4, 0x010B, 0x0109, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x0000, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x0121, 0x00F6, 0x00F7,
    0x011D, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x016D, 0x015D, 0x02D9,
];

static ISO8859_4: [u16; 96] = [
    0x00A0, 0x0104, 0x0138, 0x0156, 0x00A4, 0x0128, 0x013B, 0x00A7,
    0x00A8, 0x0160, 0x0112, 0x0122, 0x0166, 0x00AD, 0x017D, 0x00AF,
    0x00B0, 0x0105, 0x02DB, 0x0157, 0x00B4, 0x0129, 0x013C, 0x02C7,
    0x00B8, 0x0161, 0x0113, 0x0123, 0x0167, 0x014A, 0x017E, 0x014B,
    0x0100, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x012E,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x0116, 0x00CD, 0x00CE, 0x012A,
    0x0110, 0x0145, 0x014C, 0x0136, 0x00D4, 0x00D5, 0x00D6, 0x00D7,
    0x00D8, 0x0172, 0x00DA, 0x00DB, 0x00DC, 0x0168, 0x016A, 0x00DF,
    0x0101, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x012F,
    0x010D, 0x00E9, 0x0119, 0x00EB, 0x0117, 0x00ED, 0x00EE, 0x012B,
    0x0111, 0x0146, 0x014D, 0x0137, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x0173, 0x00FA, 0x00FB, 0x00FC, 0x0169, 0x016B, 0x02D9,
];

static ISO8859_5: [u16; 96] = [
    0x00A0, 0x0401, 0x0402, 0x0403, 0x0404, 0x0405, 0x0406, 0x0407,
    0x0408, 0x0409, 0x040A, 0x040B, 0x040C, 0x00AD, 0x040E, 0x040F,
    0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417,
    0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F,
    0x0420, 0x0421, 0x0422, 0x0423, 0x0424, 0x0425, 0x0426, 0x0427,
    0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F,
    0x0430, 0x0431, 0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437,
    0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F,
    0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447,
    0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D, 0x044E, 0x044F,
    0x2116, 0x0451, 0x0452, 0x0453, 0x0454, 0x0455, 0x0456, 0x0457,
    0x0458, 0x0459, 0x045A, 0x045B, 0x045C, 0x00A7, 0x045E, 0x045F,
];

static ISO8859_6: [u16; 96] = [
    0x00A0, 0x0000, 0x0000, 0x0000, 0x00A4, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x060C, 0x00AD, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x061B, 0x0000, 0x0000, 0x0000, 0x061F,
    0x0000, 0x0621, 0x0622, 0x0623, 0x0624, 0x0625, 0x0626, 0x0627,
    0x0628, 0x0629, 0x062A, 0x062B, 0x062C, 0x062D, 0x062E, 0x062F,
    0x0630, 0x0631, 0x0632, 0x0633, 0x0634, 0x0635, 0x0636, 0x0637,
    0x0638, 0x0639, 0x063A, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0640, 0x0641, 0x0642, 0x0643, 0x0644, 0x0645, 0x0646, 0x0647,
    0x0648, 0x0649, 0x064A, 0x064B, 0x064C, 0x064D, 0x064E, 0x064F,
    0x0650, 0x0651, 0x0652, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
];

static ISO8859_7: [u16; 96] = [
    0x00A0, 0x2018, 0x2019, 0x00A3, 0x20AC, 0x20AF, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x037A, 0x00AB, 0x00AC, 0x00AD, 0x0000, 0x2015,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x0384, 0x0385, 0x0386, 0x00B7,
    0x0388, 0x0389, 0x038A, 0x00BB, 0x038C, 0x00BD, 0x038E, 0x038F,
    0x0390, 0x0391, 0x0392, 0x0393, 0x0394, 0x0395, 0x0396, 0x0397,
    0x0398, 0x0399, 0x039A, 0x039B, 0x039C, 0x039D, 0x039E, 0x039F,
    0x03A0, 0x03A1, 0x0000, 0x03A3, 0x03A4, 0x03A5, 0x03A6, 0x03A7,
    0x03A8, 0x03A9, 0x03AA, 0x03AB, 0x03AC, 0x03AD, 0x03AE, 0x03AF,
    0x03B0, 0x03B1, 0x03B2, 0x03B3, 0x03B4, 0x03B5, 0x03B6, 0x03B7,
    0x03B8, 0x03B9, 0x03BA, 0x03BB, 0x03BC, 0x03BD, 0x03BE, 0x03BF,
    0x03C0, 0x03C1, 0x03C2, 0x03C3, 0x03C4, 0x03C5, 0x03C6, 0x03C7,
    0x03C8, 0x03C9, 0x03CA, 0x03CB, 0x03CC, 0x03CD, 0x03CE, 0x0000,
];

static ISO8859_8: [u16; 96] = [
    0x00A0, 0x0000, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00D7, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00F7, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x2017,
    0x05D0, 0x05D1, 0x05D2, 0x05D3, 0x05D4, 0x05D5, 0x05D6, 0x05D7,
    0x05D8, 0x05D9, 0x05DA, 0x05DB, 0x05DC, 0x05DD, 0x05DE, 0x05DF,
    0x05E0, 0x05E1, 0x05E2, 0x05E3, 0x05E4, 0x05E5, 0x05E6, 0x05E7,
    0x05E8, 0x05E9, 0x05EA, 0x0000, 0x0000, 0x200E, 0x200F, 0x0000,
];

static ISO8859_9: [u16; 96] = [
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x011E, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x0130, 0x015E, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x011F, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x0131, 0x015F, 0x00FF,
];

static ISO8859_10: [u16; 96] = [
    0x00A0, 0x0104, 0x0112, 0x0122, 0x012A, 0x0128, 0x0136, 0x00A7,
    0x013B, 0x0110, 0x0160, 0x0166, 0x017D, 0x00AD, 0x016A, 0x014A,
    0x00B0, 0x0105, 0x0113, 0x0123, 0x012B, 0x0129, 0x0137, 0x00B7,
    0x013C, 0x0111, 0x0161, 0x0167, 0x017E, 0x2015, 0x016B, 0x014B,
    0x0100, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x012E,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x0116, 0x00CD, 0x00CE, 0x00CF,
    0x00D0, 0x0145, 0x014C, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x0168,
    0x00D8, 0x0172, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x0101, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x012F,
    0x010D, 0x00E9, 0x0119, 0x00EB, 0x0117, 0x00ED, 0x00EE, 0x00EF,
    0x00F0, 0x0146, 0x014D, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x0169,
    0x00F8, 0x0173, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x0138,
];

static ISO8859_11: [u16; 96] = [
    0x00A0, 0x0E01, 0x0E02, 0x0E03, 0x0E04, 0x0E05, 0x0E06, 0x0E07,
    0x0E08, 0x0E09, 0x0E0A, 0x0E0B, 0x0E0C, 0x0E0D, 0x0E0E, 0x0E0F,
    0x0E10, 0x0E11, 0x0E12, 0x0E13, 0x0E14, 0x0E15, 0x0E16, 0x0E17,
    0x0E18, 0x0E19, 0x0E1A, 0x0E1B, 0x0E1C, 0x0E1D, 0x0E1E, 0x0E1F,
    0x0E20, 0x0E21, 0x0E22, 0x0E23, 0x0E24, 0x0E25, 0x0E26, 0x0E27,
    0x0E28, 0x0E29, 0x0E2A, 0x0E2B, 0x0E2C, 0x0E2D, 0x0E2E, 0x0E2F,
    0x0E30, 0x0E31, 0x0E32, 0x0E33, 0x0E34, 0x0E35, 0x0E36, 0x0E37,
    0x0E38, 0x0E39, 0x0E3A, 0x0000, 0x0000, 0x0000, 0x0000, 0x0E3F,
    0x0E40, 0x0E41, 0x0E42, 0x0E43, 0x0E44, 0x0E45, 0x0E46, 0x0E47,
    0x0E48, 0x0E49, 0x0E4A, 0x0E4B, 0x0E4C, 0x0E4D, 0x0E4E, 0x0E4F,
    0x0E50, 0x0E51, 0x0E52, 0x0E53, 0x0E54, 0x0E55, 0x0E56, 0x0E57,
    0x0E58, 0x0E59, 0x0E5A, 0x0E5B, 0x0000, 0x0000, 0x0000, 0x0000,
];

static ISO8859_13: [u16; 96] = [
    0x00A0, 0x201D, 0x00A2, 0x00A3, 0x00A4, 0x201E, 0x00A6, 0x00A7,
    0x00D8, 0x00A9, 0x0156, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00C6,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x201C, 0x00B5, 0x00B6, 0x00B7,
    0x00F8, 0x00B9, 0x0157, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00E6,
    0x0104, 0x012E, 0x0100, 0x0106, 0x00C4, 0x00C5, 0x0118, 0x0112,
    0x010C, 0x00C9, 0x0179, 0x0116, 0x0122, 0x0136, 0x012A, 0x013B,
    0x0160, 0x0143, 0x0145, 0x00D3, 0x014C, 0x00D5, 0x00D6, 0x00D7,
    0x0172, 0x0141, 0x015A, 0x016A, 0x00DC, 0x017B, 0x017D, 0x00DF,
    0x0105, 0x012F, 0x0101, 0x0107, 0x00E4, 0x00E5, 0x0119, 0x0113,
    0x010D, 0x00E9, 0x017A, 0x0117, 0x0123, 0x0137, 0x012B, 0x013C,
    0x0161, 0x0144, 0x0146, 0x00F3, 0x014D, 0x00F5, 0x00F6, 0x00F7,
    0x0173, 0x0142, 0x015B, 0x016B, 0x00FC, 0x017C, 0x017E, 0x2019,
];

static ISO8859_14: [u16; 96] = [
    0x00A0, 0x1E02, 0x1E03, 0x00A3, 0x010A, 0x010B, 0x1E0A, 0x00A7,
    0x1E80, 0x00A9, 0x1E82, 0x1E0B, 0x1EF2, 0x00AD, 0x00AE, 0x0178,
    0x1E1E, 0x1E1F, 0x0120, 0x0121, 0x1E40, 0x1E41, 0x00B6, 0x1E56,
    0x1E81, 0x1E57, 0x1E83, 0x1E60, 0x1EF3, 0x1E84, 0x1E85, 0x1E61,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x0174, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x1E6A,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x0176, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x0175, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x1E6B,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x0177, 0x00FF,
];

static ISO8859_15: [u16; 96] = [
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x20AC, 0x00A5, 0x0160, 0x00A7,
    0x0161, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x017D, 0x00B5, 0x00B6, 0x00B7,
    0x017E, 0x00B9, 0x00BA, 0x00BB, 0x0152, 0x0153, 0x0178, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x00D0, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x00FF,
];

static WIN1250: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0083, 0x201E, 0x2026, 0x2020, 0x2021,
    0x0088, 0x2030, 0x0160, 0x2039, 0x015A, 0x0164, 0x017D, 0x0179,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0098, 0x2122, 0x0161, 0x203A, 0x015B, 0x0165, 0x017E, 0x017A,
    0x00A0, 0x02C7, 0x02D8, 0x0141, 0x00A4, 0x0104, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x015E, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x017B,
    0x00B0, 0x00B1, 0x02DB, 0x0142, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x0105, 0x015F, 0x00BB, 0x013D, 0x02DD, 0x013E, 0x017C,
    0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E,
    0x0110, 0x0143, 0x0147, 0x00D3, 0x00D4, 0x0150, 0x00D6, 0x00D7,
    0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF,
    0x0155, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7,
    0x010D, 0x00E9, 0x0119, 0x00EB, 0x011B, 0x00ED, 0x00EE, 0x010F,
    0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7,
    0x0159, 0x016F, 0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];

static WIN1251: [u16; 128] = [
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021,
    0x20AC, 0x2030, 0x0409, 0x2039, 0x040A, 0x040C, 0x040B, 0x040F,
    0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0098, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F,
    0x00A0, 0x040E, 0x045E, 0x0408, 0x00A4, 0x0490, 0x00A6, 0x00A7,
    0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407,
    0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7,
    0x0451, 0x2116, 0x0454, 0x00BB, 0x0458, 0x0405, 0x0455, 0x0457,
    0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417,
    0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F,
    0x0420, 0x0421, 0x0422, 0x0423, 0x0424, 0x0425, 0x0426, 0x0427,
    0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F,
    0x0430, 0x0431, 0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437,
    0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F,
    0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447,
    0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D, 0x044E, 0x044F,
];

static WIN1253: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x0088, 0x2030, 0x008A, 0x2039, 0x008C, 0x008D, 0x008E, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0098, 0x2122, 0x009A, 0x203A, 0x009C, 0x009D, 0x009E, 0x009F,
    0x00A0, 0x0385, 0x0386, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x0000, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x2015,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x0384, 0x00B5, 0x00B6, 0x00B7,
    0x0388, 0x0389, 0x038A, 0x00BB, 0x038C, 0x00BD, 0x038E, 0x038F,
    0x0390, 0x0391, 0x0392, 0x0393, 0x0394, 0x0395, 0x0396, 0x0397,
    0x0398, 0x0399, 0x039A, 0x039B, 0x039C, 0x039D, 0x039E, 0x039F,
    0x03A0, 0x03A1, 0x0000, 0x03A3, 0x03A4, 0x03A5, 0x03A6, 0x03A7,
    0x03A8, 0x03A9, 0x03AA, 0x03AB, 0x03AC, 0x03AD, 0x03AE, 0x03AF,
    0x03B0, 0x03B1, 0x03B2, 0x03B3, 0x03B4, 0x03B5, 0x03B6, 0x03B7,
    0x03B8, 0x03B9, 0x03BA, 0x03BB, 0x03BC, 0x03BD, 0x03BE, 0x03BF,
    0x03C0, 0x03C1, 0x03C2, 0x03C3, 0x03C4, 0x03C5, 0x03C6, 0x03C7,
    0x03C8, 0x03C9, 0x03CA, 0x03CB, 0x03CC, 0x03CD, 0x03CE, 0x0000,
];

static WIN1254: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x008E, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x009E, 0x0178,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x011E, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x0130, 0x015E, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x011F, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x0131, 0x015F, 0x00FF,
];

static WIN1255: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x008A, 0x2039, 0x008C, 0x008D, 0x008E, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x009A, 0x203A, 0x009C, 0x009D, 0x009E, 0x009F,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x20AA, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00D7, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00F7, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x05B0, 0x05B1, 0x05B2, 0x05B3, 0x05B4, 0x05B5, 0x05B6, 0x05B7,
    0x05B8, 0x05B9, 0x0000, 0x05BB, 0x05BC, 0x05BD, 0x05BE, 0x05BF,
    0x05C0, 0x05C1, 0x05C2, 0x05C3, 0x05F0, 0x05F1, 0x05F2, 0x05F3,
    0x05F4, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x05D0, 0x05D1, 0x05D2, 0x05D3, 0x05D4, 0x05D5, 0x05D6, 0x05D7,
    0x05D8, 0x05D9, 0x05DA, 0x05DB, 0x05DC, 0x05DD, 0x05DE, 0x05DF,
    0x05E0, 0x05E1, 0x05E2, 0x05E3, 0x05E4, 0x05E5, 0x05E6, 0x05E7,
    0x05E8, 0x05E9, 0x05EA, 0x0000, 0x0000, 0x200E, 0x200F, 0x0000,
];

static WIN1256: [u16; 128] = [
    0x20AC, 0x067E, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x0679, 0x2039, 0x0152, 0x0686, 0x0698, 0x0688,
    0x06AF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x06A9, 0x2122, 0x0691, 0x203A, 0x0153, 0x200C, 0x200D, 0x06BA,
    0x00A0, 0x060C, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x06BE, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x061B, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x061F,
    0x06C1, 0x0621, 0x0622, 0x0623, 0x0624, 0x0625, 0x0626, 0x0627,
    0x0628, 0x0629, 0x062A, 0x062B, 0x062C, 0x062D, 0x062E, 0x062F,
    0x0630, 0x0631, 0x0632, 0x0633, 0x0634, 0x0635, 0x0636, 0x00D7,
    0x0637, 0x0638, 0x0639, 0x063A, 0x0640, 0x0641, 0x0642, 0x0643,
    0x00E0, 0x0644, 0x00E2, 0x0645, 0x0646, 0x0647, 0x0648, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x0649, 0x064A, 0x00EE, 0x00EF,
    0x064B, 0x064C, 0x064D, 0x064E, 0x00F4, 0x064F, 0x0650, 0x00F7,
    0x0651, 0x00F9, 0x0652, 0x00FB, 0x00FC, 0x200E, 0x200F, 0x06D2,
];

static WIN1257: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0083, 0x201E, 0x2026, 0x2020, 0x2021,
    0x0088, 0x2030, 0x008A, 0x2039, 0x008C, 0x00A8, 0x02C7, 0x00B8,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0098, 0x2122, 0x009A, 0x203A, 0x009C, 0x00AF, 0x02DB, 0x009F,
    0x00A0, 0x0000, 0x00A2, 0x00A3, 0x00A4, 0x0000, 0x00A6, 0x00A7,
    0x00D8, 0x00A9, 0x0156, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00C6,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00F8, 0x00B9, 0x0157, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00E6,
    0x0104, 0x012E, 0x0100, 0x0106, 0x00C4, 0x00C5, 0x0118, 0x0112,
    0x010C, 0x00C9, 0x0179, 0x0116, 0x0122, 0x0136, 0x012A, 0x013B,
    0x0160, 0x0143, 0x0145, 0x00D3, 0x014C, 0x00D5, 0x00D6, 0x00D7,
    0x0172, 0x0141, 0x015A, 0x016A, 0x00DC, 0x017B, 0x017D, 0x00DF,
    0x0105, 0x012F, 0x0101, 0x0107, 0x00E4, 0x00E5, 0x0119, 0x0113,
    0x010D, 0x00E9, 0x017A, 0x0117, 0x0123, 0x0137, 0x012B, 0x013C,
    0x0161, 0x0144, 0x0146, 0x00F3, 0x014D, 0x00F5, 0x00F6, 0x00F7,
    0x0173, 0x0142, 0x015B, 0x016B, 0x00FC, 0x017C, 0x017E, 0x02D9,
];

static WIN1258: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x008A, 0x2039, 0x0152, 0x008D, 0x008E, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x009A, 0x203A, 0x0153, 0x009D, 0x009E, 0x0178,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x0300, 0x00CD, 0x00CE, 0x00CF,
    0x0110, 0x00D1, 0x0309, 0x00D3, 0x00D4, 0x01A0, 0x00D6, 0x00D7,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x01AF, 0x0303, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x0301, 0x00ED, 0x00EE, 0x00EF,
    0x0111, 0x00F1, 0x0323, 0x00F3, 0x00F4, 0x01A1, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x01B0, 0x20AB, 0x00FF,
];

/// Internal: charset table descriptor
struct CharsetTable {
    table: &'static [u16],
    base: u8, // starting byte (0x80 or 0xA0)
}

fn get_charset_table(id: i32) -> Option<CharsetTable> {
    match id {
        CHARSET_WINDOWS_1252 => Some(CharsetTable { table: &WIN1252_80_9F, base: 0x80 }),
        CHARSET_MAC_ROMAN    => Some(CharsetTable { table: &MAC_ROMAN, base: 0x80 }),
        CHARSET_ISO_8859_2   => Some(CharsetTable { table: &ISO8859_2, base: 0xA0 }),
        CHARSET_ISO_8859_3   => Some(CharsetTable { table: &ISO8859_3, base: 0xA0 }),
        CHARSET_ISO_8859_4   => Some(CharsetTable { table: &ISO8859_4, base: 0xA0 }),
        CHARSET_ISO_8859_5   => Some(CharsetTable { table: &ISO8859_5, base: 0xA0 }),
        CHARSET_ISO_8859_6   => Some(CharsetTable { table: &ISO8859_6, base: 0xA0 }),
        CHARSET_ISO_8859_7   => Some(CharsetTable { table: &ISO8859_7, base: 0xA0 }),
        CHARSET_ISO_8859_8   => Some(CharsetTable { table: &ISO8859_8, base: 0xA0 }),
        CHARSET_ISO_8859_9   => Some(CharsetTable { table: &ISO8859_9, base: 0xA0 }),
        CHARSET_ISO_8859_10  => Some(CharsetTable { table: &ISO8859_10, base: 0xA0 }),
        CHARSET_ISO_8859_11  => Some(CharsetTable { table: &ISO8859_11, base: 0xA0 }),
        CHARSET_ISO_8859_13  => Some(CharsetTable { table: &ISO8859_13, base: 0xA0 }),
        CHARSET_ISO_8859_14  => Some(CharsetTable { table: &ISO8859_14, base: 0xA0 }),
        CHARSET_ISO_8859_15  => Some(CharsetTable { table: &ISO8859_15, base: 0xA0 }),
        CHARSET_WINDOWS_1250 => Some(CharsetTable { table: &WIN1250, base: 0x80 }),
        CHARSET_WINDOWS_1251 => Some(CharsetTable { table: &WIN1251, base: 0x80 }),
        CHARSET_WINDOWS_1253 => Some(CharsetTable { table: &WIN1253, base: 0x80 }),
        CHARSET_WINDOWS_1254 => Some(CharsetTable { table: &WIN1254, base: 0x80 }),
        CHARSET_WINDOWS_1255 => Some(CharsetTable { table: &WIN1255, base: 0x80 }),
        CHARSET_WINDOWS_1256 => Some(CharsetTable { table: &WIN1256, base: 0x80 }),
        CHARSET_WINDOWS_1257 => Some(CharsetTable { table: &WIN1257, base: 0x80 }),
        CHARSET_WINDOWS_1258 => Some(CharsetTable { table: &WIN1258, base: 0x80 }),
        _ => None,
    }
}

/// Convert a single byte from a charset to Unicode codepoint.
fn byte_to_unicode(charset_id: i32, byte: u8) -> u32 {
    if byte < 0x80 { return byte as u32; }

    if charset_id == CHARSET_WINDOWS_1252 {
        if byte >= 0x80 && byte <= 0x9F {
            return WIN1252_80_9F[(byte - 0x80) as usize] as u32;
        }
        return byte as u32; // 0xA0-0xFF = identity
    }

    if charset_id == CHARSET_ISO_8859_1 || charset_id == CHARSET_US_ASCII {
        return byte as u32;
    }

    if let Some(ct) = get_charset_table(charset_id) {
        let idx = byte as usize;
        let base = ct.base as usize;
        if idx >= base && idx < base + ct.table.len() {
            return ct.table[idx - base] as u32;
        }
    }

    byte as u32 // fallback identity
}

// ---------------------------------------------------------------------------
// Charset name resolution
// ---------------------------------------------------------------------------

/// Normalize charset name: strip hyphens/underscores, lowercase.
fn normalize_charset(name: &str) -> String {
    name.chars()
        .filter(|&c| c != '-' && c != '_' && c != ' ')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Resolve charset name to ID.
pub fn charset_name_to_id(name: &str) -> i32 {
    if name.is_empty() { return CHARSET_UNKNOWN; }
    let norm = normalize_charset(name);
    match norm.as_str() {
        "usascii" | "ascii" => CHARSET_US_ASCII,
        "utf8" => CHARSET_UTF8,
        "utf16le" => CHARSET_UTF16LE,
        "utf16be" | "utf16" => CHARSET_UTF16BE,
        "utf32le" => CHARSET_UTF32LE,
        "utf32be" | "utf32" => CHARSET_UTF32BE,
        "iso88591" | "latin1" => CHARSET_ISO_8859_1,
        "iso88592" | "latin2" => CHARSET_ISO_8859_2,
        "iso88593" => CHARSET_ISO_8859_3,
        "iso88594" => CHARSET_ISO_8859_4,
        "iso88595" => CHARSET_ISO_8859_5,
        "iso88596" => CHARSET_ISO_8859_6,
        "iso88597" => CHARSET_ISO_8859_7,
        "iso88598" => CHARSET_ISO_8859_8,
        "iso88599" | "latin5" => CHARSET_ISO_8859_9,
        "iso885910" => CHARSET_ISO_8859_10,
        "iso885911" => CHARSET_ISO_8859_11,
        "iso885913" => CHARSET_ISO_8859_13,
        "iso885914" => CHARSET_ISO_8859_14,
        "iso885915" | "latin9" => CHARSET_ISO_8859_15,
        "windows1250" | "cp1250" => CHARSET_WINDOWS_1250,
        "windows1251" | "cp1251" => CHARSET_WINDOWS_1251,
        "windows1252" | "cp1252" => CHARSET_WINDOWS_1252,
        "windows1253" | "cp1253" => CHARSET_WINDOWS_1253,
        "windows1254" | "cp1254" => CHARSET_WINDOWS_1254,
        "windows1255" | "cp1255" => CHARSET_WINDOWS_1255,
        "windows1256" | "cp1256" => CHARSET_WINDOWS_1256,
        "windows1257" | "cp1257" => CHARSET_WINDOWS_1257,
        "windows1258" | "cp1258" => CHARSET_WINDOWS_1258,
        "macroman" | "macintosh" | "xmacroman" => CHARSET_MAC_ROMAN,
        _ => CHARSET_UNKNOWN,
    }
}

/// Get canonical charset name from ID.
pub fn charset_name(id: i32) -> &'static str {
    match id {
        CHARSET_US_ASCII     => "US-ASCII",
        CHARSET_UTF8         => "UTF-8",
        CHARSET_UTF16LE      => "UTF-16LE",
        CHARSET_UTF16BE      => "UTF-16BE",
        CHARSET_UTF32LE      => "UTF-32LE",
        CHARSET_UTF32BE      => "UTF-32BE",
        CHARSET_ISO_8859_1   => "ISO-8859-1",
        CHARSET_ISO_8859_2   => "ISO-8859-2",
        CHARSET_ISO_8859_3   => "ISO-8859-3",
        CHARSET_ISO_8859_4   => "ISO-8859-4",
        CHARSET_ISO_8859_5   => "ISO-8859-5",
        CHARSET_ISO_8859_6   => "ISO-8859-6",
        CHARSET_ISO_8859_7   => "ISO-8859-7",
        CHARSET_ISO_8859_8   => "ISO-8859-8",
        CHARSET_ISO_8859_9   => "ISO-8859-9",
        CHARSET_ISO_8859_10  => "ISO-8859-10",
        CHARSET_ISO_8859_11  => "ISO-8859-11",
        CHARSET_ISO_8859_13  => "ISO-8859-13",
        CHARSET_ISO_8859_14  => "ISO-8859-14",
        CHARSET_ISO_8859_15  => "ISO-8859-15",
        CHARSET_WINDOWS_1250 => "Windows-1250",
        CHARSET_WINDOWS_1251 => "Windows-1251",
        CHARSET_WINDOWS_1252 => "Windows-1252",
        CHARSET_WINDOWS_1253 => "Windows-1253",
        CHARSET_WINDOWS_1254 => "Windows-1254",
        CHARSET_WINDOWS_1255 => "Windows-1255",
        CHARSET_WINDOWS_1256 => "Windows-1256",
        CHARSET_WINDOWS_1257 => "Windows-1257",
        CHARSET_WINDOWS_1258 => "Windows-1258",
        CHARSET_MAC_ROMAN    => "macintosh",
        _ => "unknown",
    }
}

// ---------------------------------------------------------------------------
// Charset conversion: to UTF-8
// ---------------------------------------------------------------------------

/// Convert from a named charset to UTF-8. Returns Ok(String) or Err.
pub fn charset_to_utf8(charset: &str, data: &[u8]) -> Result<String, &'static str> {
    let id = charset_name_to_id(charset);

    // UTF-8 passthrough
    if id == CHARSET_UTF8 {
        return Ok(String::from_utf8_lossy(data).into_owned());
    }

    // UTF-16
    if id == CHARSET_UTF16LE || id == CHARSET_UTF16BE {
        let le = id == CHARSET_UTF16LE;
        let mut out = String::new();
        let mut start = 0;
        // Strip BOM
        if data.len() >= 2 {
            let bom = if le {
                data[0] as u16 | ((data[1] as u16) << 8)
            } else {
                ((data[0] as u16) << 8) | data[1] as u16
            };
            if bom == 0xFEFF { start = 2; }
        }
        let mut i = start;
        while i + 1 < data.len() {
            let w = if le {
                data[i] as u16 | ((data[i + 1] as u16) << 8)
            } else {
                ((data[i] as u16) << 8) | data[i + 1] as u16
            };
            let cp;
            if w >= 0xD800 && w <= 0xDBFF && i + 3 < data.len() {
                let w2 = if le {
                    data[i + 2] as u16 | ((data[i + 3] as u16) << 8)
                } else {
                    ((data[i + 2] as u16) << 8) | data[i + 3] as u16
                };
                if w2 >= 0xDC00 && w2 <= 0xDFFF {
                    cp = 0x10000 + ((w as u32 - 0xD800) << 10) + (w2 as u32 - 0xDC00);
                    i += 4;
                } else {
                    cp = w as u32;
                    i += 2;
                }
            } else {
                cp = w as u32;
                i += 2;
            }
            if let Some(c) = char::from_u32(cp) {
                out.push(c);
            }
        }
        return Ok(out);
    }

    // UTF-32
    if id == CHARSET_UTF32LE || id == CHARSET_UTF32BE {
        let le = id == CHARSET_UTF32LE;
        let mut out = String::new();
        let mut start = 0;
        if data.len() >= 4 {
            let bom = if le {
                data[0] as u32 | ((data[1] as u32) << 8) | ((data[2] as u32) << 16) | ((data[3] as u32) << 24)
            } else {
                ((data[0] as u32) << 24) | ((data[1] as u32) << 16) | ((data[2] as u32) << 8) | data[3] as u32
            };
            if bom == 0xFEFF { start = 4; }
        }
        let mut i = start;
        while i + 3 < data.len() {
            let cp = if le {
                data[i] as u32 | ((data[i+1] as u32) << 8) | ((data[i+2] as u32) << 16) | ((data[i+3] as u32) << 24)
            } else {
                ((data[i] as u32) << 24) | ((data[i+1] as u32) << 16) | ((data[i+2] as u32) << 8) | data[i+3] as u32
            };
            let cp = if cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF) { 0xFFFD } else { cp };
            if let Some(c) = char::from_u32(cp) {
                out.push(c);
            }
            i += 4;
        }
        return Ok(out);
    }

    // Single-byte charsets
    let mut out = String::new();
    for &byte in data {
        let cp = byte_to_unicode(id, byte);
        if let Some(c) = char::from_u32(cp) {
            out.push(c);
        }
    }
    Ok(out)
}

/// Convert from UTF-8 to a named charset. Returns Ok(Vec<u8>) or Err.
pub fn utf8_to_charset(charset: &str, data: &str) -> Result<Vec<u8>, &'static str> {
    let id = charset_name_to_id(charset);

    // UTF-8 passthrough
    if id == CHARSET_UTF8 {
        return Ok(data.as_bytes().to_vec());
    }

    // UTF-16
    if id == CHARSET_UTF16LE || id == CHARSET_UTF16BE {
        let le = id == CHARSET_UTF16LE;
        let mut out = Vec::new();
        for cp in data.chars().map(|c| c as u32) {
            if cp < 0x10000 {
                if le {
                    out.push((cp & 0xFF) as u8);
                    out.push(((cp >> 8) & 0xFF) as u8);
                } else {
                    out.push(((cp >> 8) & 0xFF) as u8);
                    out.push((cp & 0xFF) as u8);
                }
            } else {
                let cp = cp - 0x10000;
                let hi = 0xD800 + (cp >> 10) as u16;
                let lo = 0xDC00 + (cp & 0x3FF) as u16;
                if le {
                    out.push((hi & 0xFF) as u8);
                    out.push(((hi >> 8) & 0xFF) as u8);
                    out.push((lo & 0xFF) as u8);
                    out.push(((lo >> 8) & 0xFF) as u8);
                } else {
                    out.push(((hi >> 8) & 0xFF) as u8);
                    out.push((hi & 0xFF) as u8);
                    out.push(((lo >> 8) & 0xFF) as u8);
                    out.push((lo & 0xFF) as u8);
                }
            }
        }
        return Ok(out);
    }

    // UTF-32
    if id == CHARSET_UTF32LE || id == CHARSET_UTF32BE {
        let le = id == CHARSET_UTF32LE;
        let mut out = Vec::new();
        for cp in data.chars().map(|c| c as u32) {
            if le {
                out.push((cp & 0xFF) as u8);
                out.push(((cp >> 8) & 0xFF) as u8);
                out.push(((cp >> 16) & 0xFF) as u8);
                out.push(((cp >> 24) & 0xFF) as u8);
            } else {
                out.push(((cp >> 24) & 0xFF) as u8);
                out.push(((cp >> 16) & 0xFF) as u8);
                out.push(((cp >> 8) & 0xFF) as u8);
                out.push((cp & 0xFF) as u8);
            }
        }
        return Ok(out);
    }

    // Single-byte charsets: reverse lookup
    let mut out = Vec::new();
    for cp in data.chars().map(|c| c as u32) {
        if cp < 0x80 {
            out.push(cp as u8);
        } else {
            let mut found = false;

            if id == CHARSET_WINDOWS_1252 {
                for (i, &v) in WIN1252_80_9F.iter().enumerate() {
                    if v as u32 == cp {
                        out.push((0x80 + i) as u8);
                        found = true;
                        break;
                    }
                }
                if !found && cp >= 0xA0 && cp <= 0xFF {
                    out.push(cp as u8);
                    found = true;
                }
            } else if id == CHARSET_ISO_8859_1 {
                if cp <= 0xFF {
                    out.push(cp as u8);
                    found = true;
                }
            } else if let Some(ct) = get_charset_table(id) {
                for (i, &v) in ct.table.iter().enumerate() {
                    if v as u32 == cp {
                        out.push((ct.base as usize + i) as u8);
                        found = true;
                        break;
                    }
                }
            }

            if !found {
                out.push(b'?');
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Charset detection
// ---------------------------------------------------------------------------

/// Sniff encoding from BOM or byte patterns. Returns charset ID.
pub fn charset_detect(data: &[u8]) -> i32 {
    if data.is_empty() { return CHARSET_UNKNOWN; }

    // BOM detection
    if data.len() >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF {
        return CHARSET_UTF8;
    }
    if data.len() >= 4 && data[0] == 0xFF && data[1] == 0xFE && data[2] == 0 && data[3] == 0 {
        return CHARSET_UTF32LE;
    }
    if data.len() >= 4 && data[0] == 0 && data[1] == 0 && data[2] == 0xFE && data[3] == 0xFF {
        return CHARSET_UTF32BE;
    }
    if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xFE {
        return CHARSET_UTF16LE;
    }
    if data.len() >= 2 && data[0] == 0xFE && data[1] == 0xFF {
        return CHARSET_UTF16BE;
    }

    // Try UTF-8 validation
    if utf8_validate(data) {
        let has_high = data.iter().any(|&b| b >= 0x80);
        return if has_high { CHARSET_UTF8 } else { CHARSET_US_ASCII };
    }

    // Not valid UTF-8 with high bytes -> likely Windows-1252
    CHARSET_WINDOWS_1252
}

// ---------------------------------------------------------------------------
// RFC 2047 MIME encoded-word
// ---------------------------------------------------------------------------

fn b64_decode(input: &[u8]) -> Vec<u8> {
    const B64: [i8; 256] = {
        let mut t = [-1i8; 256];
        let mut i = 0u8;
        while i < 26 { t[(b'A' + i) as usize] = i as i8; i += 1; }
        i = 0;
        while i < 26 { t[(b'a' + i) as usize] = (26 + i) as i8; i += 1; }
        i = 0;
        while i < 10 { t[(b'0' + i) as usize] = (52 + i) as i8; i += 1; }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &b in input {
        if b == b'=' { break; }
        let v = B64[b as usize];
        if v < 0 { continue; }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    out
}

fn q_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'_' {
            out.push(b' ');
            i += 1;
        } else if input[i] == b'=' && i + 2 < input.len() {
            let hi = hex_val(input[i + 1]);
            let lo = hex_val(input[i + 2]);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h << 4) | l);
                i += 3;
            } else {
                out.push(input[i]);
                i += 1;
            }
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Decode =?charset?B?...?= and =?charset?Q?...?= encoded words in a header.
/// Returns UTF-8 string.
pub fn mime_decode_header(header: &str) -> String {
    let bytes = header.as_bytes();
    let mut result = String::new();
    let mut i = 0;
    let mut last_was_encoded = false;

    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'=' && bytes[i + 1] == b'?' {
            // Find charset
            let cs_start = i + 2;
            if let Some(rel) = bytes[cs_start..].iter().position(|&b| b == b'?') {
                let cs_end = cs_start + rel;
                if cs_end + 2 < bytes.len() && bytes[cs_end + 2] == b'?' {
                    let encoding = bytes[cs_end + 1].to_ascii_uppercase();
                    let text_start = cs_end + 3;
                    // Find ?=
                    if let Some(te_rel) = find_subseq(&bytes[text_start..], b"?=") {
                        let text_end = text_start + te_rel;
                        if encoding == b'B' || encoding == b'Q' {
                            // Skip whitespace between consecutive encoded words
                            // (already handled by last_was_encoded flag below)

                            let cs = &bytes[cs_start..cs_end];
                            let cs_str = String::from_utf8_lossy(cs);
                            let encoded_text = &bytes[text_start..text_end];

                            let decoded = if encoding == b'B' {
                                b64_decode(encoded_text)
                            } else {
                                q_decode(encoded_text)
                            };

                            if let Ok(utf8) = charset_to_utf8(&cs_str, &decoded) {
                                result.push_str(&utf8);
                            }

                            i = text_end + 2;
                            last_was_encoded = true;

                            // Skip whitespace between consecutive encoded words (RFC 2047 6.2)
                            let mut ws = i;
                            while ws < bytes.len() && (bytes[ws] == b' ' || bytes[ws] == b'\t'
                                || bytes[ws] == b'\r' || bytes[ws] == b'\n') {
                                ws += 1;
                            }
                            if ws + 1 < bytes.len() && bytes[ws] == b'=' && bytes[ws + 1] == b'?' {
                                i = ws;
                            }
                            continue;
                        }
                    }
                }
            }
        }
        last_was_encoded = false;
        if i < bytes.len() {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    let _ = last_was_encoded; // suppress warning
    result
}

fn find_subseq(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Encode UTF-8 string as RFC 2047 for use in email headers. Returns encoded string.
pub fn mime_encode_header(utf8: &str, charset: Option<&str>) -> String {
    let cs = charset.unwrap_or("UTF-8");
    if is_ascii(utf8.as_bytes()) {
        return utf8.to_string();
    }

    static B64CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let data = utf8.as_bytes();
    let mut result = format!("=?{}?B?", cs);

    let mut i = 0;
    while i + 2 < data.len() {
        let v = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        result.push(B64CHARS[((v >> 18) & 0x3F) as usize] as char);
        result.push(B64CHARS[((v >> 12) & 0x3F) as usize] as char);
        result.push(B64CHARS[((v >> 6) & 0x3F) as usize] as char);
        result.push(B64CHARS[(v & 0x3F) as usize] as char);
        i += 3;
    }
    if i < data.len() {
        let mut v = (data[i] as u32) << 16;
        if i + 1 < data.len() { v |= (data[i + 1] as u32) << 8; }
        result.push(B64CHARS[((v >> 18) & 0x3F) as usize] as char);
        result.push(B64CHARS[((v >> 12) & 0x3F) as usize] as char);
        if i + 1 < data.len() {
            result.push(B64CHARS[((v >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        result.push('=');
    }

    result.push_str("?=");
    result
}

// ---------------------------------------------------------------------------
// Convenience
// ---------------------------------------------------------------------------

/// One-shot convert to UTF-8.
pub fn to_utf8(data: &[u8], charset: &str) -> Option<String> {
    charset_to_utf8(charset, data).ok()
}

// ---------------------------------------------------------------------------
// ISO-2022 escape sequence stripping
// ---------------------------------------------------------------------------

/// Strip ISO-2022 escape sequences (ESC $ B, ESC ( B, SI, SO, etc.).
pub fn clean_iso2022(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0x1B && i + 1 < data.len() {
            i += 1; // skip ESC
            if data[i] == b'$' || data[i] == b'(' {
                i += 1; // skip $ or (
                if i < data.len() { i += 1; } // skip final byte
                continue;
            }
            if data[i] == b'.' || data[i] == b'-' || data[i] == b')' {
                i += 1;
                if i < data.len() { i += 1; }
                continue;
            }
            // Unknown ESC sequence, skip just the ESC
            continue;
        }
        // SI (0x0F) and SO (0x0E) shift codes -- strip them
        if data[i] == 0x0E || data[i] == 0x0F {
            i += 1;
            continue;
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Content-Type charset extraction
// ---------------------------------------------------------------------------

/// Extract charset from a Content-Type header value or HTML meta tag.
pub fn parse_charset(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();

    // Try "charset=" in Content-Type header
    if let Some(pos) = lower.find("charset") {
        let rest = &content_type[pos + 7..];
        let rest = rest.trim_start();
        if rest.starts_with('=') {
            let rest = rest[1..].trim_start();
            let (value, _) = if rest.starts_with('"') {
                let inner = &rest[1..];
                if let Some(end) = inner.find('"') {
                    (&inner[..end], &inner[end..])
                } else {
                    (inner, "")
                }
            } else if rest.starts_with('\'') {
                let inner = &rest[1..];
                if let Some(end) = inner.find('\'') {
                    (&inner[..end], &inner[end..])
                } else {
                    (inner, "")
                }
            } else {
                let end = rest.find(|c: char| c == ';' || c == ' ' || c == '\t' || c == '"' || c == '>');
                match end {
                    Some(e) => (&rest[..e], &rest[e..]),
                    None => (rest, ""),
                }
            };
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }

    // Try HTML <meta charset="...">
    let mut search = &lower[..];
    while let Some(pos) = search.find("<meta") {
        let tag_start = pos;
        let rest = &search[pos + 5..];
        if let Some(end) = rest.find('>') {
            let tag_content = &content_type[tag_start + 5..tag_start + 5 + end];
            let tag_lower = tag_content.to_ascii_lowercase();
            if let Some(cs_pos) = tag_lower.find("charset") {
                let cs_rest = &tag_content[cs_pos + 7..];
                let cs_rest = cs_rest.trim_start();
                if cs_rest.starts_with('=') {
                    let cs_rest = cs_rest[1..].trim_start();
                    let value = if cs_rest.starts_with('"') {
                        let inner = &cs_rest[1..];
                        inner.split('"').next().unwrap_or("")
                    } else if cs_rest.starts_with('\'') {
                        let inner = &cs_rest[1..];
                        inner.split('\'').next().unwrap_or("")
                    } else {
                        let end = cs_rest.find(|c: char| c == ';' || c == ' ' || c == '\t' || c == '"' || c == '>');
                        match end {
                            Some(e) => &cs_rest[..e],
                            None => cs_rest,
                        }
                    };
                    if !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }
            search = &search[tag_start + 5 + end..];
        } else {
            break;
        }
    }

    None
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utf8_encode_ascii() {
        let mut buf = [0u8; 4];
        assert_eq!(utf8_encode(0x41, &mut buf), 1);
        assert_eq!(buf[0], b'A');
    }

    #[test]
    fn test_utf8_encode_2byte() {
        let mut buf = [0u8; 4];
        assert_eq!(utf8_encode(0xE9, &mut buf), 2); // e-acute
        assert_eq!(buf[0], 0xC3);
        assert_eq!(buf[1], 0xA9);
    }

    #[test]
    fn test_utf8_encode_3byte() {
        let mut buf = [0u8; 4];
        assert_eq!(utf8_encode(0x20AC, &mut buf), 3); // Euro sign
        assert_eq!(buf[0], 0xE2);
        assert_eq!(buf[1], 0x82);
        assert_eq!(buf[2], 0xAC);
    }

    #[test]
    fn test_utf8_encode_4byte() {
        let mut buf = [0u8; 4];
        assert_eq!(utf8_encode(0x1F600, &mut buf), 4); // grinning face
        assert_eq!(buf[0], 0xF0);
        assert_eq!(buf[1], 0x9F);
        assert_eq!(buf[2], 0x98);
        assert_eq!(buf[3], 0x80);
    }

    #[test]
    fn test_utf8_decode_ascii() {
        let (cp, n) = utf8_decode(b"A");
        assert_eq!(cp, 0x41);
        assert_eq!(n, 1);
    }

    #[test]
    fn test_utf8_decode_multibyte() {
        let data = "\u{00E9}".as_bytes(); // e-acute
        let (cp, n) = utf8_decode(data);
        assert_eq!(cp, 0xE9);
        assert_eq!(n, 2);
    }

    #[test]
    fn test_utf8_decode_invalid() {
        let (cp, n) = utf8_decode(&[0xFF]);
        assert_eq!(cp, 0xFFFD);
        assert_eq!(n, 1);
    }

    #[test]
    fn test_utf8_decode_overlong() {
        // Overlong encoding of NUL: C0 80
        let (cp, _) = utf8_decode(&[0xC0, 0x80]);
        assert_eq!(cp, 0xFFFD);
    }

    #[test]
    fn test_utf8_len() {
        assert_eq!(utf8_len("Hello".as_bytes()), 5);
        assert_eq!(utf8_len("\u{00E9}".as_bytes()), 1);
        assert_eq!(utf8_len("H\u{00E9}llo".as_bytes()), 5);
    }

    #[test]
    fn test_utf8_valid_len() {
        let valid = "Hello\u{00E9}".as_bytes();
        assert_eq!(utf8_valid_len(valid), valid.len());

        // Truncated 2-byte sequence at end
        let mut trunc = vec![0x41, 0xC3]; // A + start of 2-byte
        assert_eq!(utf8_valid_len(&trunc), 1);
        trunc.push(0xA9); // complete
        assert_eq!(utf8_valid_len(&trunc), 3);
    }

    #[test]
    fn test_utf8_validate() {
        assert!(utf8_validate("Hello world".as_bytes()));
        assert!(utf8_validate("\u{20AC}".as_bytes())); // Euro
        assert!(!utf8_validate(&[0xFF, 0xFE]));
    }

    #[test]
    fn test_is_ascii() {
        assert!(is_ascii(b"Hello"));
        assert!(!is_ascii(&[0x80]));
        assert!(is_ascii(b""));
    }

    #[test]
    fn test_charset_name_to_id() {
        assert_eq!(charset_name_to_id("UTF-8"), CHARSET_UTF8);
        assert_eq!(charset_name_to_id("utf-8"), CHARSET_UTF8);
        assert_eq!(charset_name_to_id("utf8"), CHARSET_UTF8);
        assert_eq!(charset_name_to_id("ISO-8859-1"), CHARSET_ISO_8859_1);
        assert_eq!(charset_name_to_id("latin1"), CHARSET_ISO_8859_1);
        assert_eq!(charset_name_to_id("Windows-1252"), CHARSET_WINDOWS_1252);
        assert_eq!(charset_name_to_id("cp1252"), CHARSET_WINDOWS_1252);
        assert_eq!(charset_name_to_id("macintosh"), CHARSET_MAC_ROMAN);
        assert_eq!(charset_name_to_id("bogus"), CHARSET_UNKNOWN);
    }

    #[test]
    fn test_charset_name() {
        assert_eq!(charset_name(CHARSET_UTF8), "UTF-8");
        assert_eq!(charset_name(CHARSET_WINDOWS_1252), "Windows-1252");
        assert_eq!(charset_name(CHARSET_MAC_ROMAN), "macintosh");
        assert_eq!(charset_name(999), "unknown");
    }

    #[test]
    fn test_charset_to_utf8_latin1() {
        let data = [0xE9u8]; // e-acute in Latin-1
        let result = charset_to_utf8("ISO-8859-1", &data).unwrap();
        assert_eq!(result, "\u{00E9}");
    }

    #[test]
    fn test_charset_to_utf8_win1252() {
        // 0x80 = Euro sign in Windows-1252
        let data = [0x80u8];
        let result = charset_to_utf8("Windows-1252", &data).unwrap();
        assert_eq!(result, "\u{20AC}");
    }

    #[test]
    fn test_charset_to_utf8_mac_roman() {
        // 0x80 = A-dieresis in Mac Roman
        let data = [0x80u8];
        let result = charset_to_utf8("macintosh", &data).unwrap();
        assert_eq!(result, "\u{00C4}");
    }

    #[test]
    fn test_charset_to_utf8_utf16le() {
        // "AB" in UTF-16LE
        let data = [0x41, 0x00, 0x42, 0x00];
        let result = charset_to_utf8("UTF-16LE", &data).unwrap();
        assert_eq!(result, "AB");
    }

    #[test]
    fn test_charset_to_utf8_utf16be_with_bom() {
        // BOM + "A"
        let data = [0xFE, 0xFF, 0x00, 0x41];
        let result = charset_to_utf8("UTF-16BE", &data).unwrap();
        assert_eq!(result, "A");
    }

    #[test]
    fn test_charset_to_utf8_utf16_surrogate() {
        // U+1F600 in UTF-16LE: D83D DE00
        let data = [0x3D, 0xD8, 0x00, 0xDE];
        let result = charset_to_utf8("UTF-16LE", &data).unwrap();
        assert_eq!(result, "\u{1F600}");
    }

    #[test]
    fn test_utf8_to_charset_latin1() {
        let result = utf8_to_charset("ISO-8859-1", "\u{00E9}").unwrap();
        assert_eq!(result, vec![0xE9]);
    }

    #[test]
    fn test_utf8_to_charset_win1252_euro() {
        let result = utf8_to_charset("Windows-1252", "\u{20AC}").unwrap();
        assert_eq!(result, vec![0x80]);
    }

    #[test]
    fn test_utf8_to_charset_unmappable() {
        // CJK character not in Latin-1
        let result = utf8_to_charset("ISO-8859-1", "\u{4E2D}").unwrap();
        assert_eq!(result, vec![b'?']);
    }

    #[test]
    fn test_charset_detect_ascii() {
        assert_eq!(charset_detect(b"Hello"), CHARSET_US_ASCII);
    }

    #[test]
    fn test_charset_detect_utf8_bom() {
        assert_eq!(charset_detect(&[0xEF, 0xBB, 0xBF, 0x41]), CHARSET_UTF8);
    }

    #[test]
    fn test_charset_detect_utf8_no_bom() {
        let data = "\u{00E9}".as_bytes(); // valid UTF-8, high bytes
        assert_eq!(charset_detect(data), CHARSET_UTF8);
    }

    #[test]
    fn test_charset_detect_win1252() {
        // 0x80 alone is not valid UTF-8 -> Windows-1252
        assert_eq!(charset_detect(&[0x80]), CHARSET_WINDOWS_1252);
    }

    #[test]
    fn test_charset_detect_utf16_bom() {
        assert_eq!(charset_detect(&[0xFF, 0xFE, 0x41, 0x00]), CHARSET_UTF16LE);
        assert_eq!(charset_detect(&[0xFE, 0xFF, 0x00, 0x41]), CHARSET_UTF16BE);
    }

    #[test]
    fn test_mime_decode_header_plain() {
        assert_eq!(mime_decode_header("Hello World"), "Hello World");
    }

    #[test]
    fn test_mime_decode_header_b_encoding() {
        // =?UTF-8?B?SGVsbG8=?=  -> "Hello"
        let result = mime_decode_header("=?UTF-8?B?SGVsbG8=?=");
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_mime_decode_header_q_encoding() {
        // =?UTF-8?Q?Hello_World?=  -> "Hello World"
        let result = mime_decode_header("=?UTF-8?Q?Hello_World?=");
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_mime_decode_header_q_hex() {
        // =?ISO-8859-1?Q?=E9?= -> e-acute
        let result = mime_decode_header("=?ISO-8859-1?Q?=E9?=");
        assert_eq!(result, "\u{00E9}");
    }

    #[test]
    fn test_mime_decode_header_consecutive() {
        // Two consecutive encoded words: whitespace between them should be consumed
        let result = mime_decode_header("=?UTF-8?B?SGVs?= =?UTF-8?B?bG8=?=");
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_mime_encode_header_ascii() {
        assert_eq!(mime_encode_header("Hello", None), "Hello");
    }

    #[test]
    fn test_mime_encode_header_non_ascii() {
        let encoded = mime_encode_header("\u{00E9}", None);
        assert!(encoded.starts_with("=?UTF-8?B?"));
        assert!(encoded.ends_with("?="));
        // Decode back
        let decoded = mime_decode_header(&encoded);
        assert_eq!(decoded, "\u{00E9}");
    }

    #[test]
    fn test_clean_iso2022() {
        // ESC $ B ... ESC ( B
        let input = [0x1B, b'$', b'B', b'A', b'B', 0x1B, b'(', b'B'];
        let result = clean_iso2022(&input);
        assert_eq!(result, vec![b'A', b'B']);
    }

    #[test]
    fn test_clean_iso2022_shift_codes() {
        let input = [b'A', 0x0E, b'B', 0x0F, b'C'];
        let result = clean_iso2022(&input);
        assert_eq!(result, vec![b'A', b'B', b'C']);
    }

    #[test]
    fn test_parse_charset_content_type() {
        let ct = "text/html; charset=UTF-8";
        assert_eq!(parse_charset(ct), Some("UTF-8".to_string()));
    }

    #[test]
    fn test_parse_charset_quoted() {
        let ct = "text/html; charset=\"ISO-8859-1\"";
        assert_eq!(parse_charset(ct), Some("ISO-8859-1".to_string()));
    }

    #[test]
    fn test_parse_charset_meta_tag() {
        let html = "<meta charset=\"UTF-8\">";
        assert_eq!(parse_charset(html), Some("UTF-8".to_string()));
    }

    #[test]
    fn test_parse_charset_none() {
        assert_eq!(parse_charset("text/plain"), None);
    }

    #[test]
    fn test_roundtrip_iso8859_2() {
        // Check a few specific mappings
        let data = [0xA1u8]; // LATIN CAPITAL LETTER A WITH OGONEK
        let utf8 = charset_to_utf8("ISO-8859-2", &data).unwrap();
        assert_eq!(utf8, "\u{0104}");
        let back = utf8_to_charset("ISO-8859-2", &utf8).unwrap();
        assert_eq!(back, vec![0xA1]);
    }

    #[test]
    fn test_roundtrip_win1251_cyrillic() {
        let data = [0xC0u8]; // Cyrillic A
        let utf8 = charset_to_utf8("Windows-1251", &data).unwrap();
        assert_eq!(utf8, "\u{0410}");
        let back = utf8_to_charset("Windows-1251", &utf8).unwrap();
        assert_eq!(back, vec![0xC0]);
    }

    #[test]
    fn test_utf8_encode_char_convenience() {
        assert_eq!(utf8_encode_char(0x41), Some("A".to_string()));
        assert_eq!(utf8_encode_char(0x20AC), Some("\u{20AC}".to_string()));
        assert_eq!(utf8_encode_char(0x110000), None); // out of range
    }

    #[test]
    fn test_to_utf8_convenience() {
        assert_eq!(to_utf8(b"Hello", "US-ASCII"), Some("Hello".to_string()));
        assert_eq!(to_utf8(&[0xE9], "ISO-8859-1"), Some("\u{00E9}".to_string()));
    }

    #[test]
    fn test_utf32_roundtrip() {
        let result = charset_to_utf8("UTF-32LE", &[0x41, 0x00, 0x00, 0x00]).unwrap();
        assert_eq!(result, "A");

        let back = utf8_to_charset("UTF-32LE", "A").unwrap();
        assert_eq!(back, vec![0x41, 0x00, 0x00, 0x00]);
    }
}
