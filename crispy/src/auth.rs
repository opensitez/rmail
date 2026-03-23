//! SASL authentication mechanisms for mail protocols.
//!
//! Provides pure functions that build auth strings — no I/O, no sessions.
//! Protocol modules (smtp, pop3) use these to implement AUTH commands.

use crate::base64;
use crate::md5;

/// Authentication mechanism types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanism {
    Plain,
    Login,
    CramMd5,
    XOAuth2,
    DigestMd5,
    ScramSha256,
}

impl Mechanism {
    /// Parse from an EHLO/CAPA capability string.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_uppercase().as_str() {
            "PLAIN" => Some(Self::Plain),
            "LOGIN" => Some(Self::Login),
            "CRAM-MD5" => Some(Self::CramMd5),
            "XOAUTH2" => Some(Self::XOAuth2),
            "DIGEST-MD5" => Some(Self::DigestMd5),
            "SCRAM-SHA-256" => Some(Self::ScramSha256),
            _ => None,
        }
    }

    /// Mechanism name as used in SMTP/POP3 AUTH command.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Plain => "PLAIN",
            Self::Login => "LOGIN",
            Self::CramMd5 => "CRAM-MD5",
            Self::XOAuth2 => "XOAUTH2",
            Self::DigestMd5 => "DIGEST-MD5",
            Self::ScramSha256 => "SCRAM-SHA-256",
        }
    }
}

// ── PLAIN (RFC 4616) ──

/// Build PLAIN auth string: base64("\0user\0password").
pub fn plain_response(user: &str, pass: &str) -> String {
    let mut raw = Vec::with_capacity(1 + user.len() + 1 + pass.len());
    raw.push(0); // authzid (empty)
    raw.extend_from_slice(user.as_bytes());
    raw.push(0);
    raw.extend_from_slice(pass.as_bytes());
    base64::encode(&raw)
}

// ── LOGIN ──

/// Build LOGIN auth step 1: base64(user).
pub fn login_username(user: &str) -> String {
    base64::encode(user.as_bytes())
}

/// Build LOGIN auth step 2: base64(password).
pub fn login_password(pass: &str) -> String {
    base64::encode(pass.as_bytes())
}

// ── CRAM-MD5 (RFC 2195) ──

/// Build CRAM-MD5 response from a server challenge.
///
/// The challenge is the base64-encoded string from the server's 334 reply.
/// Returns base64("user hmac-md5-hex") ready to send.
pub fn cram_md5_response(user: &str, pass: &str, challenge_b64: &str) -> Option<String> {
    let challenge = base64::decode(challenge_b64).ok()?;
    let hex = md5::hmac_md5_hex(pass.as_bytes(), &challenge);
    let response = format!("{} {}", user, hex);
    Some(base64::encode(response.as_bytes()))
}

// ── XOAUTH2 (RFC 7628 / Google extension) ──

/// Build XOAUTH2 auth string from user and OAuth2 bearer token.
///
/// Format: base64("user=<user>\x01auth=Bearer <token>\x01\x01")
pub fn xoauth2_response(user: &str, token: &str) -> String {
    let raw = format!("user={}\x01auth=Bearer {}\x01\x01", user, token);
    base64::encode(raw.as_bytes())
}

// ── DIGEST-MD5 (RFC 2831) ──

/// Parsed DIGEST-MD5 server challenge.
#[derive(Debug, Default)]
pub struct DigestMd5Challenge {
    pub realm: String,
    pub nonce: String,
    pub qop: String,
}

/// Parse a DIGEST-MD5 challenge (decoded from base64).
pub fn digest_md5_parse_challenge(challenge: &str) -> DigestMd5Challenge {
    let mut c = DigestMd5Challenge::default();
    for part in challenge.split(',') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("realm=") {
            c.realm = unquote(val).to_string();
        } else if let Some(val) = part.strip_prefix("nonce=") {
            c.nonce = unquote(val).to_string();
        } else if let Some(val) = part.strip_prefix("qop=") {
            c.qop = unquote(val).to_string();
        }
    }
    if c.qop.is_empty() { c.qop = "auth".to_string(); }
    c
}

/// Build DIGEST-MD5 response.
///
/// Returns base64-encoded response string ready to send.
pub fn digest_md5_response(
    user: &str, pass: &str, challenge_b64: &str, service: &str,
) -> Option<String> {
    let challenge_raw = base64::decode(challenge_b64).ok()?;
    let challenge_str = String::from_utf8_lossy(&challenge_raw);
    let c = digest_md5_parse_challenge(&challenge_str);
    if c.nonce.is_empty() { return None; }

    let cnonce = md5::md5_hex(user.as_bytes());

    // HA1 = MD5("user:realm:pass")
    let a1 = format!("{}:{}:{}", user, c.realm, pass);
    let ha1 = md5::md5_hex(a1.as_bytes());

    let digest_uri = format!("{}/{}", service, c.realm);

    // HA2 = MD5("AUTHENTICATE:digest-uri")
    let a2 = format!("AUTHENTICATE:{}", digest_uri);
    let ha2 = md5::md5_hex(a2.as_bytes());

    // response = MD5("HA1:nonce:00000001:cnonce:auth:HA2")
    let resp_input = format!("{}:{}:00000001:{}:auth:{}", ha1, c.nonce, cnonce, ha2);
    let resp_hash = md5::md5_hex(resp_input.as_bytes());

    let resp = format!(
        "username=\"{}\",realm=\"{}\",nonce=\"{}\",cnonce=\"{}\",\
         nc=00000001,qop=auth,digest-uri=\"{}\",response={}",
        user, c.realm, c.nonce, cnonce, digest_uri, resp_hash
    );

    Some(base64::encode(resp.as_bytes()))
}

fn unquote(s: &str) -> &str {
    s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_basic() {
        let resp = plain_response("user", "pass");
        let decoded = base64::decode(&resp).unwrap();
        assert_eq!(decoded, b"\0user\0pass");
    }

    #[test]
    fn plain_empty_password() {
        let resp = plain_response("user", "");
        let decoded = base64::decode(&resp).unwrap();
        assert_eq!(decoded, b"\0user\0");
    }

    #[test]
    fn login_steps() {
        let u = login_username("user");
        let p = login_password("pass");
        assert_eq!(base64::decode(&u).unwrap(), b"user");
        assert_eq!(base64::decode(&p).unwrap(), b"pass");
    }

    #[test]
    fn cram_md5_basic() {
        let challenge_b64 = base64::encode(b"<1234@server>");
        let resp = cram_md5_response("user", "pass", &challenge_b64).unwrap();
        let decoded = base64::decode(&resp).unwrap();
        let decoded_str = String::from_utf8(decoded).unwrap();
        assert!(decoded_str.starts_with("user "));
        assert_eq!(decoded_str.len(), 5 + 32);
    }

    #[test]
    fn cram_md5_deterministic() {
        let challenge_b64 = base64::encode(b"test-challenge");
        let r1 = cram_md5_response("alice", "secret", &challenge_b64);
        let r2 = cram_md5_response("alice", "secret", &challenge_b64);
        assert_eq!(r1, r2);
    }

    #[test]
    fn cram_md5_different_passwords() {
        let challenge_b64 = base64::encode(b"challenge");
        let r1 = cram_md5_response("user", "pass1", &challenge_b64);
        let r2 = cram_md5_response("user", "pass2", &challenge_b64);
        assert_ne!(r1, r2);
    }

    #[test]
    fn xoauth2_format() {
        let resp = xoauth2_response("user@example.com", "ya29.token");
        let decoded = base64::decode(&resp).unwrap();
        let s = String::from_utf8(decoded).unwrap();
        assert_eq!(s, "user=user@example.com\x01auth=Bearer ya29.token\x01\x01");
    }

    #[test]
    fn digest_md5_parse() {
        let c = digest_md5_parse_challenge(
            "realm=\"example.com\",nonce=\"abc123\",qop=\"auth\""
        );
        assert_eq!(c.realm, "example.com");
        assert_eq!(c.nonce, "abc123");
        assert_eq!(c.qop, "auth");
    }

    #[test]
    fn digest_md5_response_basic() {
        let challenge = base64::encode(
            b"realm=\"mail.example.com\",nonce=\"testnonce\",qop=\"auth\""
        );
        let resp = digest_md5_response("user", "pass", &challenge, "smtp");
        assert!(resp.is_some());
        let decoded = base64::decode(&resp.unwrap()).unwrap();
        let s = String::from_utf8(decoded).unwrap();
        assert!(s.contains("username=\"user\""));
        assert!(s.contains("realm=\"mail.example.com\""));
        assert!(s.contains("response="));
    }

    #[test]
    fn mechanism_from_name() {
        assert_eq!(Mechanism::from_name("PLAIN"), Some(Mechanism::Plain));
        assert_eq!(Mechanism::from_name("cram-md5"), Some(Mechanism::CramMd5));
        assert_eq!(Mechanism::from_name("XOAUTH2"), Some(Mechanism::XOAuth2));
        assert_eq!(Mechanism::from_name("UNKNOWN"), None);
    }

    #[test]
    fn mechanism_name_roundtrip() {
        for m in [Mechanism::Plain, Mechanism::Login, Mechanism::CramMd5,
                  Mechanism::XOAuth2, Mechanism::DigestMd5, Mechanism::ScramSha256] {
            assert_eq!(Mechanism::from_name(m.name()), Some(m));
        }
    }
}
