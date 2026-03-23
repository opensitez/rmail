//! MD5 hash, HMAC-MD5, and hex formatting.
//! Pure Rust implementation — no OpenSSL dependency.

/// MD5 digest (16 bytes).
pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut state = Md5State::new();
    state.update(data);
    state.finalize()
}

/// MD5 digest as 32-char hex string.
pub fn md5_hex(data: &[u8]) -> String {
    hex(&md5(data))
}

/// HMAC-MD5 (RFC 2104).
pub fn hmac_md5(key: &[u8], data: &[u8]) -> [u8; 16] {
    let mut ikey = [0x36u8; 64];
    let mut okey = [0x5cu8; 64];

    let actual_key = if key.len() > 64 {
        let h = md5(key);
        for i in 0..16 {
            ikey[i] ^= h[i];
            okey[i] ^= h[i];
        }
        None
    } else {
        for (i, &b) in key.iter().enumerate() {
            ikey[i] ^= b;
            okey[i] ^= b;
        }
        Some(())
    };
    let _ = actual_key;

    // inner = MD5(ikey || data)
    let mut inner = Md5State::new();
    inner.update(&ikey);
    inner.update(data);
    let inner_hash = inner.finalize();

    // outer = MD5(okey || inner)
    let mut outer = Md5State::new();
    outer.update(&okey);
    outer.update(&inner_hash);
    outer.finalize()
}

/// HMAC-MD5 as hex string.
pub fn hmac_md5_hex(key: &[u8], data: &[u8]) -> String {
    hex(&hmac_md5(key, data))
}

/// Format bytes as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize]);
        s.push(HEX[(b & 0x0f) as usize]);
    }
    unsafe { String::from_utf8_unchecked(s) }
}

// ── MD5 implementation (RFC 1321) ──

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5,  9, 14, 20, 5,  9, 14, 20, 5,  9, 14, 20, 5,  9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee,
    0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
    0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa,
    0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
    0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
    0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05,
    0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039,
    0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
    0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

struct Md5State {
    a: u32, b: u32, c: u32, d: u32,
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Md5State {
    fn new() -> Self {
        Md5State {
            a: 0x67452301, b: 0xefcdab89, c: 0x98badcfe, d: 0x10325476,
            buf: [0; 64], buf_len: 0, total_len: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.total_len += data.len() as u64;
        let mut offset = 0;

        // Fill partial buffer
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = need.min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            offset = take;
            if self.buf_len == 64 {
                let block = self.buf;
                self.process_block(&block);
                self.buf_len = 0;
            }
        }

        // Process full blocks
        while offset + 64 <= data.len() {
            let block: [u8; 64] = data[offset..offset + 64].try_into().unwrap();
            self.process_block(&block);
            offset += 64;
        }

        // Buffer remainder
        if offset < data.len() {
            let rem = data.len() - offset;
            self.buf[..rem].copy_from_slice(&data[offset..]);
            self.buf_len = rem;
        }
    }

    fn finalize(mut self) -> [u8; 16] {
        let bit_len = self.total_len * 8;

        // Padding: 1 bit + zeros + 64-bit length
        let mut pad = [0u8; 72];
        pad[0] = 0x80;
        let pad_len = if self.buf_len < 56 { 56 - self.buf_len } else { 120 - self.buf_len };
        self.update(&pad[..pad_len]);
        self.update(&bit_len.to_le_bytes());

        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&self.a.to_le_bytes());
        out[4..8].copy_from_slice(&self.b.to_le_bytes());
        out[8..12].copy_from_slice(&self.c.to_le_bytes());
        out[12..16].copy_from_slice(&self.d.to_le_bytes());
        out
    }

    fn process_block(&mut self, block: &[u8; 64]) {
        let mut m = [0u32; 16];
        for i in 0..16 {
            m[i] = u32::from_le_bytes(block[i * 4..(i + 1) * 4].try_into().unwrap());
        }

        let (mut a, mut b, mut c, mut d) = (self.a, self.b, self.c, self.d);

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 =>  ((b & c) | ((!b) & d),           i),
                16..=31 => ((d & b) | ((!d) & c),           (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d,                      (3 * i + 5) % 16),
                _ =>       (c ^ (b | (!d)),                  (7 * i) % 16),
            };

            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                (a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]))
                    .rotate_left(S[i])
            );
            a = temp;
        }

        self.a = self.a.wrapping_add(a);
        self.b = self.b.wrapping_add(b);
        self.c = self.c.wrapping_add(c);
        self.d = self.d.wrapping_add(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_empty() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn md5_a() {
        assert_eq!(md5_hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
    }

    #[test]
    fn md5_abc() {
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn md5_message_digest() {
        assert_eq!(md5_hex(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
    }

    #[test]
    fn md5_alphabet() {
        assert_eq!(md5_hex(b"abcdefghijklmnopqrstuvwxyz"), "c3fcd3d76192e4007dfb496cca67e13b");
    }

    #[test]
    fn md5_long() {
        assert_eq!(
            md5_hex(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"),
            "d174ab98d277d9f5a5611c2c9f419d9f"
        );
    }

    #[test]
    fn md5_numeric() {
        assert_eq!(
            md5_hex(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn hmac_md5_rfc2104() {
        // RFC 2104 test vector 1
        let key = [0x0bu8; 16];
        assert_eq!(hmac_md5_hex(&key, b"Hi There"), "9294727a3638bb1c13f48ef8158bfc9d");
    }

    #[test]
    fn hmac_md5_jefe() {
        // RFC 2104 test vector 2
        assert_eq!(hmac_md5_hex(b"Jefe", b"what do ya want for nothing?"),
                   "750c783e6ab0b503eaa86e310a5db738");
    }

    #[test]
    fn hex_format() {
        assert_eq!(hex(&[0x00, 0xff, 0xab, 0x12]), "00ffab12");
    }
}
