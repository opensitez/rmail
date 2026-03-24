//! POP3 client (RFC 1939).
//!
//! Standalone — no Eudora/GTK dependency.  Uses [`crate::transport::Transport`]
//! for all I/O and [`crate::auth`] for SASL mechanisms.
//!
//! # Supported commands
//!
//! `USER/PASS`, `APOP`, `AUTH CRAM-MD5`, `AUTH XOAUTH2`,
//! `STAT`, `LIST`, `UIDL`, `RETR`, `TOP`, `DELE`, `RSET`, `QUIT`, `STLS`.

use std::fmt;
use std::io;

use crate::auth;
use crate::md5;
use crate::transport::Transport;

// ── Error type ──────────────────────────────────────────────────────────────

/// POP3-specific errors.
#[derive(Debug)]
pub enum Pop3Error {
    /// Server returned `-ERR` with an optional message.
    Server(String),
    /// Transport / I/O failure.
    Io(io::Error),
    /// Protocol violation (unexpected data from server).
    Protocol(String),
    /// Not connected or not authenticated.
    State(String),
}

impl fmt::Display for Pop3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Server(msg) => write!(f, "POP3 server error: {}", msg),
            Self::Io(e) => write!(f, "POP3 I/O error: {}", e),
            Self::Protocol(msg) => write!(f, "POP3 protocol error: {}", msg),
            Self::State(msg) => write!(f, "POP3 state error: {}", msg),
        }
    }
}

impl std::error::Error for Pop3Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Pop3Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Pop3Error>;

// ── Security mode ───────────────────────────────────────────────────────────

/// Connection security mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pop3Security {
    /// No encryption (port 110).
    Plain,
    /// Upgrade via STLS after greeting (port 110).
    Stls,
    /// Implicit TLS / POP3S (port 995).
    Ssl,
}

// ── Message info ────────────────────────────────────────────────────────────

/// Information about a single message on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pop3Message {
    /// 1-based message number.
    pub number: u32,
    /// Message size in bytes (from LIST).
    pub size: u64,
    /// Unique ID from UIDL (empty if server does not support UIDL).
    pub uidl: String,
}

// ── Debug callback ──────────────────────────────────────────────────────────

/// Optional debug callback — receives `"C: ..."` for sent commands and
/// `"S: ..."` for server replies.
pub type DebugFn = Box<dyn FnMut(&str)>;

// ── Session ─────────────────────────────────────────────────────────────────

/// A POP3 client session.
///
/// Wraps a [`Transport`] and provides the full POP3 command set.
pub struct Pop3Session<T: Transport> {
    pub tp: T,
    connected: bool,
    authenticated: bool,
    /// Last single-line reply from the server (CRLF stripped).
    pub last_reply: String,
    /// Server greeting (saved for APOP timestamp extraction).
    pub greeting: String,
    /// Message count from the last `STAT`.
    pub msg_count: u32,
    /// Mailbox size in bytes from the last `STAT`.
    pub mailbox_size: u64,
    /// Optional debug hook.
    debug: Option<DebugFn>,
}

impl<T: Transport> Pop3Session<T> {
    // ── Construction ────────────────────────────────────────────────────

    /// Create a new session around a transport (not yet connected).
    pub fn new(tp: T) -> Self {
        Self {
            tp,
            connected: false,
            authenticated: false,
            last_reply: String::new(),
            greeting: String::new(),
            msg_count: 0,
            mailbox_size: 0,
            debug: None,
        }
    }

    /// Attach a debug callback.
    pub fn set_debug<F: FnMut(&str) + 'static>(&mut self, f: F) {
        self.debug = Some(Box::new(f));
    }

    /// Return a reference to the underlying transport.
    pub fn transport(&self) -> &T {
        &self.tp
    }

    /// Return a mutable reference to the underlying transport.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.tp
    }

    // ── Internal helpers ────────────────────────────────────────────────

    fn dbg(&mut self, msg: &str) {
        if let Some(ref mut f) = self.debug {
            f(msg);
        }
    }

    /// Send a line (appends CRLF).
    fn send_line(&mut self, line: &str) -> Result<()> {
        let dbg_msg = format!("C: {}", line);
        self.dbg(&dbg_msg);
        self.tp.send(line.as_bytes())?;
        self.tp.send(b"\r\n")?;
        Ok(())
    }

    /// Read one response line.  Returns `Ok(true)` for `+OK`, `Err` for
    /// `-ERR` or I/O failure.  Stores the full line in `self.last_reply`.
    fn read_response(&mut self) -> Result<()> {
        let raw = self.tp.recv_line()?;
        let line = strip_crlf(&raw);
        self.last_reply = line.clone();

        let dbg_msg = format!("S: {}", line);
        self.dbg(&dbg_msg);

        if line.starts_with("+OK") {
            Ok(())
        } else if line.starts_with("-ERR") {
            let msg = line.get(4..).unwrap_or("").trim().to_string();
            Err(Pop3Error::Server(msg))
        } else {
            Err(Pop3Error::Protocol(format!("unexpected reply: {}", line)))
        }
    }

    // ── Low-level API ───────────────────────────────────────────────────

    /// Send a POP3 command and read the single-line response.
    ///
    /// Returns `Ok(())` on `+OK`, `Err` on `-ERR` or I/O failure.
    /// The full reply text is available in [`Self::last_reply`].
    pub fn command(&mut self, cmd: &str) -> Result<()> {
        self.send_line(cmd)?;
        self.read_response()
    }

    /// Read a multi-line (dot-terminated) response body.
    ///
    /// Call this immediately after a successful `command()` that triggers
    /// a multi-line response (LIST, UIDL, RETR, TOP).
    /// Performs dot-unstuffing per RFC 1939 section 3.
    pub fn read_multiline(&mut self) -> Result<Vec<u8>> {
        let mut buf: Vec<u8> = Vec::with_capacity(4096);

        loop {
            let raw = self.tp.recv_line()?;
            let line = strip_crlf_bytes(&raw);

            // Dot-only line terminates the response.
            if line == b"." {
                break;
            }

            // Dot-unstuffing: leading ".." becomes ".".
            let data = if line.starts_with(b"..") {
                &line[1..]
            } else {
                &line[..]
            };

            buf.extend_from_slice(data);
            buf.extend_from_slice(b"\r\n");
        }

        Ok(buf)
    }

    // ── Connection ──────────────────────────────────────────────────────

    /// Connect to a POP3 server, read the greeting, and optionally upgrade
    /// to TLS via STLS.
    pub fn connect(&mut self, host: &str, port: u16, security: Pop3Security) -> Result<()> {
        self.tp.connect(host, port)?;
        self.connected = true;

        // Read server greeting.
        self.read_response()?;
        self.greeting = self.last_reply.clone();

        // STLS upgrade if requested.
        if security == Pop3Security::Stls {
            self.command("STLS")?;
            self.tp.start_tls().map_err(|e| {
                Pop3Error::Io(e)
            })?;
        }

        Ok(())
    }

    // ── Authentication ──────────────────────────────────────────────────

    /// Authenticate with USER/PASS (cleartext).
    pub fn authenticate(&mut self, user: &str, pass: &str) -> Result<()> {
        self.command(&format!("USER {}", user))?;
        self.command(&format!("PASS {}", pass))?;
        self.authenticated = true;
        Ok(())
    }

    /// Authenticate with APOP (MD5 of greeting timestamp + password).
    pub fn auth_apop(&mut self, user: &str, pass: &str) -> Result<()> {
        let timestamp = extract_apop_timestamp(&self.greeting)
            .ok_or_else(|| Pop3Error::Protocol(
                "no APOP timestamp in greeting".to_string()
            ))?;

        let mut concat = Vec::with_capacity(timestamp.len() + pass.len());
        concat.extend_from_slice(timestamp.as_bytes());
        concat.extend_from_slice(pass.as_bytes());
        let hex = md5::md5_hex(&concat);

        self.command(&format!("APOP {} {}", user, hex))?;
        self.authenticated = true;
        Ok(())
    }

    /// Authenticate with AUTH CRAM-MD5 (challenge-response).
    pub fn auth_cram_md5(&mut self, user: &str, pass: &str) -> Result<()> {
        self.send_line("AUTH CRAM-MD5")?;

        // Server responds with "+ <base64-challenge>".
        let raw = self.tp.recv_line()?;
        let line = strip_crlf(&raw);
        let dbg_msg = format!("S: {}", line);
        self.dbg(&dbg_msg);

        let challenge_b64 = line.strip_prefix("+ ")
            .ok_or_else(|| Pop3Error::Protocol(
                format!("expected challenge, got: {}", line)
            ))?;

        let response = auth::cram_md5_response(user, pass, challenge_b64)
            .ok_or_else(|| Pop3Error::Protocol(
                "CRAM-MD5 response generation failed".to_string()
            ))?;

        self.send_line(&response)?;
        self.read_response()?;
        self.authenticated = true;
        Ok(())
    }

    /// Authenticate with AUTH XOAUTH2 (OAuth2 bearer token).
    pub fn auth_xoauth2(&mut self, user: &str, token: &str) -> Result<()> {
        let payload = auth::xoauth2_response(user, token);
        self.command(&format!("AUTH XOAUTH2 {}", payload))?;
        self.authenticated = true;
        Ok(())
    }

    // ── Message operations ──────────────────────────────────────────────

    /// Issue STAT.  Updates [`Self::msg_count`] and [`Self::mailbox_size`].
    pub fn stat(&mut self) -> Result<()> {
        self.command("STAT")?;
        // Parse "+OK count size"
        let rest = self.last_reply.get(3..).unwrap_or("").trim();
        let mut parts = rest.split_whitespace();
        if let Some(count_str) = parts.next() {
            self.msg_count = count_str.parse().unwrap_or(0);
        }
        if let Some(size_str) = parts.next() {
            self.mailbox_size = size_str.parse().unwrap_or(0);
        }
        Ok(())
    }

    /// Issue LIST + UIDL and return information for every message.
    ///
    /// Calls STAT first to get the message count, then LIST for sizes,
    /// then UIDL for unique IDs (silently skipped if unsupported).
    pub fn list(&mut self) -> Result<Vec<Pop3Message>> {
        self.stat()?;
        if self.msg_count == 0 {
            return Ok(Vec::new());
        }

        // LIST — get message numbers and sizes.
        self.command("LIST")?;
        let list_body = self.read_multiline()?;
        let list_text = String::from_utf8_lossy(&list_body);

        let mut msgs: Vec<Pop3Message> = Vec::with_capacity(self.msg_count as usize);

        for line in list_text.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            let mut parts = line.split_whitespace();
            let number: u32 = match parts.next().and_then(|s| s.parse().ok()) {
                Some(n) => n,
                None => continue,
            };
            let size: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            msgs.push(Pop3Message { number, size, uidl: String::new() });
        }

        // UIDL — get unique IDs (optional, server may reject).
        if self.command("UIDL").is_ok() {
            let uidl_body = self.read_multiline()?;
            let uidl_text = String::from_utf8_lossy(&uidl_body);

            for line in uidl_text.lines() {
                let line = line.trim();
                if line.is_empty() { continue; }
                let mut parts = line.splitn(2, char::is_whitespace);
                let number: u32 = match parts.next().and_then(|s| s.parse().ok()) {
                    Some(n) => n,
                    None => continue,
                };
                let uidl = parts.next().unwrap_or("").trim();
                // Match by message number.
                if let Some(msg) = msgs.iter_mut().find(|m| m.number == number) {
                    msg.uidl = uidl.to_string();
                }
            }
        }

        Ok(msgs)
    }

    /// Retrieve a full message by number (1-based).  Returns the raw RFC 5322
    /// message bytes.
    pub fn retrieve(&mut self, msg_num: u32) -> Result<Vec<u8>> {
        self.command(&format!("RETR {}", msg_num))?;
        self.read_multiline()
    }

    /// Retrieve just the headers and first `lines` body lines (TOP command).
    pub fn top(&mut self, msg_num: u32, lines: u32) -> Result<Vec<u8>> {
        self.command(&format!("TOP {} {}", msg_num, lines))?;
        self.read_multiline()
    }

    /// Mark a message for deletion (DELE).  Deletion is committed on QUIT.
    pub fn delete(&mut self, msg_num: u32) -> Result<()> {
        self.command(&format!("DELE {}", msg_num))
    }

    /// Unmark all messages marked for deletion (RSET).
    pub fn reset(&mut self) -> Result<()> {
        self.command("RSET")
    }

    /// Close the session (sends QUIT, which commits deletions).
    pub fn quit(&mut self) {
        if self.connected {
            let _ = self.command("QUIT");
            self.tp.close();
            self.connected = false;
            self.authenticated = false;
        }
    }

    /// Whether the session is connected.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Whether the session is authenticated.
    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }
}

impl<T: Transport> Drop for Pop3Session<T> {
    fn drop(&mut self) {
        if self.connected {
            // Best-effort QUIT.
            let _ = self.command("QUIT");
            self.tp.close();
        }
    }
}

// ── Free functions ──────────────────────────────────────────────────────────

/// Extract the APOP timestamp `<...>` from a POP3 greeting line.
fn extract_apop_timestamp(greeting: &str) -> Option<String> {
    let start = greeting.find('<')?;
    let end = greeting[start..].find('>')? + start;
    Some(greeting[start..=end].to_string())
}

/// Strip trailing CR/LF from a byte slice, return as String.
fn strip_crlf(data: &[u8]) -> String {
    let s = String::from_utf8_lossy(data);
    s.trim_end_matches(|c| c == '\r' || c == '\n').to_string()
}

/// Strip trailing CR/LF from a byte slice, return as bytes.
fn strip_crlf_bytes(data: &[u8]) -> &[u8] {
    let mut end = data.len();
    while end > 0 && (data[end - 1] == b'\r' || data[end - 1] == b'\n') {
        end -= 1;
    }
    &data[..end]
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    // ── Mock transport ──────────────────────────────────────────────────

    /// A simple mock transport that records sent data and plays back
    /// pre-loaded server lines.
    struct MockTransport {
        /// Lines the "server" will send (each should include CRLF).
        responses: VecDeque<Vec<u8>>,
        /// Bytes sent by the client.
        sent: Vec<u8>,
        /// Whether start_tls was called.
        tls_started: bool,
        /// Whether close was called.
        closed: bool,
    }

    impl MockTransport {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: responses.into_iter()
                    .map(|s| s.as_bytes().to_vec())
                    .collect(),
                sent: Vec::new(),
                tls_started: false,
                closed: false,
            }
        }

        /// Return everything the client sent as a string.
        fn sent_str(&self) -> String {
            String::from_utf8_lossy(&self.sent).to_string()
        }
    }

    impl Transport for MockTransport {
        fn connect(&mut self, _host: &str, _port: u16) -> io::Result<()> {
            Ok(())
        }
        fn send(&mut self, data: &[u8]) -> io::Result<()> {
            self.sent.extend_from_slice(data);
            Ok(())
        }
        fn recv_line(&mut self) -> io::Result<Vec<u8>> {
            self.responses.pop_front().ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "no more responses")
            })
        }
        fn start_tls(&mut self) -> io::Result<()> {
            self.tls_started = true;
            Ok(())
        }
        fn close(&mut self) {
            self.closed = true;
        }
    }

    // ── Helper to build a session that is already "connected" ───────────

    /// Build a Pop3Session with greeting already consumed.
    fn session_with(greeting: &str, responses: Vec<&str>) -> Pop3Session<MockTransport> {
        let mut all = vec![&*format_leak(format!("+OK {}\r\n", greeting))];
        all.extend(responses);
        let tp = MockTransport::new(all);
        let mut s = Pop3Session::new(tp);
        s.connect("localhost", 110, Pop3Security::Plain).unwrap();
        s
    }

    /// Leak a String to get a &'static str (only for tests).
    fn format_leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    // ── Reply parsing tests ─────────────────────────────────────────────

    #[test]
    fn parse_ok_reply() {
        let tp = MockTransport::new(vec![
            "+OK POP3 server ready\r\n",
        ]);
        let mut s = Pop3Session::new(tp);
        assert!(s.read_response().is_ok());
        assert_eq!(s.last_reply, "+OK POP3 server ready");
    }

    #[test]
    fn parse_err_reply() {
        let tp = MockTransport::new(vec![
            "-ERR invalid command\r\n",
        ]);
        let mut s = Pop3Session::new(tp);
        let err = s.read_response().unwrap_err();
        match err {
            Pop3Error::Server(msg) => assert_eq!(msg, "invalid command"),
            other => panic!("expected Server error, got: {:?}", other),
        }
    }

    #[test]
    fn parse_unexpected_reply() {
        let tp = MockTransport::new(vec![
            "GARBAGE LINE\r\n",
        ]);
        let mut s = Pop3Session::new(tp);
        let err = s.read_response().unwrap_err();
        assert!(matches!(err, Pop3Error::Protocol(_)));
    }

    // ── STAT parsing ────────────────────────────────────────────────────

    #[test]
    fn stat_parses_count_and_size() {
        let mut s = session_with("ready", vec![
            "+OK 5 12345\r\n",
        ]);
        s.stat().unwrap();
        assert_eq!(s.msg_count, 5);
        assert_eq!(s.mailbox_size, 12345);
    }

    #[test]
    fn stat_handles_count_only() {
        let mut s = session_with("ready", vec![
            "+OK 3\r\n",
        ]);
        s.stat().unwrap();
        assert_eq!(s.msg_count, 3);
        assert_eq!(s.mailbox_size, 0);
    }

    // ── Command formatting ──────────────────────────────────────────────

    #[test]
    fn user_pass_commands() {
        let mut s = session_with("ready", vec![
            "+OK\r\n", // USER
            "+OK\r\n", // PASS
        ]);
        s.authenticate("alice", "secret").unwrap();
        assert!(s.is_authenticated());
        let sent = s.transport().sent_str();
        assert!(sent.contains("USER alice\r\n"));
        assert!(sent.contains("PASS secret\r\n"));
    }

    #[test]
    fn dele_command_format() {
        let mut s = session_with("ready", vec![
            "+OK\r\n", // DELE
        ]);
        s.delete(3).unwrap();
        let sent = s.transport().sent_str();
        assert!(sent.contains("DELE 3\r\n"));
    }

    #[test]
    fn retr_command_format() {
        let mut s = session_with("ready", vec![
            "+OK\r\n",                // RETR reply
            "Subject: hi\r\n",        // body line
            "\r\n",                    // blank line
            "body text\r\n",          // body line
            ".\r\n",                   // terminator
        ]);
        let msg = s.retrieve(1).unwrap();
        let text = String::from_utf8_lossy(&msg);
        assert!(text.contains("Subject: hi"));
        assert!(text.contains("body text"));
        let sent = s.transport().sent_str();
        assert!(sent.contains("RETR 1\r\n"));
    }

    #[test]
    fn top_command_format() {
        let mut s = session_with("ready", vec![
            "+OK\r\n",
            "Subject: hi\r\n",
            ".\r\n",
        ]);
        let msg = s.top(2, 5).unwrap();
        assert!(!msg.is_empty());
        let sent = s.transport().sent_str();
        assert!(sent.contains("TOP 2 5\r\n"));
    }

    #[test]
    fn rset_command() {
        let mut s = session_with("ready", vec![
            "+OK\r\n",
        ]);
        s.reset().unwrap();
        let sent = s.transport().sent_str();
        assert!(sent.contains("RSET\r\n"));
    }

    // ── UIDL parsing ────────────────────────────────────────────────────

    #[test]
    fn list_parses_messages_and_uidls() {
        let mut s = session_with("ready", vec![
            // STAT
            "+OK 2 5000\r\n",
            // LIST
            "+OK\r\n",
            "1 2000\r\n",
            "2 3000\r\n",
            ".\r\n",
            // UIDL
            "+OK\r\n",
            "1 abc123\r\n",
            "2 def456\r\n",
            ".\r\n",
        ]);
        let msgs = s.list().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].number, 1);
        assert_eq!(msgs[0].size, 2000);
        assert_eq!(msgs[0].uidl, "abc123");
        assert_eq!(msgs[1].number, 2);
        assert_eq!(msgs[1].size, 3000);
        assert_eq!(msgs[1].uidl, "def456");
    }

    #[test]
    fn list_empty_mailbox() {
        let mut s = session_with("ready", vec![
            "+OK 0 0\r\n", // STAT
        ]);
        let msgs = s.list().unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn list_uidl_unsupported() {
        let mut s = session_with("ready", vec![
            // STAT
            "+OK 1 1000\r\n",
            // LIST
            "+OK\r\n",
            "1 1000\r\n",
            ".\r\n",
            // UIDL — server rejects
            "-ERR not supported\r\n",
        ]);
        let msgs = s.list().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].number, 1);
        assert_eq!(msgs[0].uidl, ""); // no UIDL available
    }

    // ── Multiline dot-unstuffing ────────────────────────────────────────

    #[test]
    fn multiline_dot_unstuffing() {
        let tp = MockTransport::new(vec![
            "line one\r\n",
            "..dot-stuffed\r\n",
            "..\r\n",               // just a dot after unstuffing
            "normal\r\n",
            ".\r\n",                // terminator
        ]);
        let mut s = Pop3Session::new(tp);
        let body = s.read_multiline().unwrap();
        let text = String::from_utf8_lossy(&body);
        assert_eq!(text, "line one\r\n.dot-stuffed\r\n.\r\nnormal\r\n");
    }

    #[test]
    fn multiline_empty() {
        let tp = MockTransport::new(vec![
            ".\r\n",
        ]);
        let mut s = Pop3Session::new(tp);
        let body = s.read_multiline().unwrap();
        assert!(body.is_empty());
    }

    // ── APOP timestamp extraction ───────────────────────────────────────

    #[test]
    fn extract_timestamp_basic() {
        let ts = extract_apop_timestamp("+OK POP3 <1234.5678@mail.example.com>");
        assert_eq!(ts.unwrap(), "<1234.5678@mail.example.com>");
    }

    #[test]
    fn extract_timestamp_none() {
        let ts = extract_apop_timestamp("+OK POP3 server ready");
        assert!(ts.is_none());
    }

    #[test]
    fn extract_timestamp_no_close() {
        let ts = extract_apop_timestamp("+OK POP3 <broken");
        assert!(ts.is_none());
    }

    // ── APOP authentication ─────────────────────────────────────────────

    #[test]
    fn auth_apop_sends_correct_command() {
        // Greeting with a timestamp.
        let tp = MockTransport::new(vec![
            "+OK POP3 <1234@server>\r\n",  // greeting
            "+OK welcome\r\n",              // APOP reply
        ]);
        let mut s = Pop3Session::new(tp);
        s.connect("localhost", 110, Pop3Security::Plain).unwrap();
        s.auth_apop("user", "pass").unwrap();
        assert!(s.is_authenticated());
        let sent = s.transport().sent_str();
        assert!(sent.contains("APOP user "));
        // The hex digest should be 32 hex chars.
        let apop_line = sent.lines()
            .find(|l| l.starts_with("APOP"))
            .unwrap();
        let digest = apop_line.split_whitespace().nth(2).unwrap();
        assert_eq!(digest.len(), 32);
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn auth_apop_no_timestamp() {
        let tp = MockTransport::new(vec![
            "+OK no timestamp here\r\n",
        ]);
        let mut s = Pop3Session::new(tp);
        s.connect("localhost", 110, Pop3Security::Plain).unwrap();
        let err = s.auth_apop("user", "pass").unwrap_err();
        assert!(matches!(err, Pop3Error::Protocol(_)));
    }

    // ── STLS ────────────────────────────────────────────────────────────

    #[test]
    fn stls_upgrade() {
        let tp = MockTransport::new(vec![
            "+OK ready\r\n",  // greeting
            "+OK begin TLS\r\n",  // STLS reply
        ]);
        let mut s = Pop3Session::new(tp);
        s.connect("localhost", 110, Pop3Security::Stls).unwrap();
        assert!(s.transport().tls_started);
    }

    // ── XOAUTH2 ────────────────────────────────────────────────────────

    #[test]
    fn auth_xoauth2_command() {
        let mut s = session_with("ready", vec![
            "+OK\r\n",
        ]);
        s.auth_xoauth2("user@example.com", "ya29.token").unwrap();
        assert!(s.is_authenticated());
        let sent = s.transport().sent_str();
        assert!(sent.contains("AUTH XOAUTH2 "));
    }

    // ── CRAM-MD5 ────────────────────────────────────────────────────────

    #[test]
    fn auth_cram_md5_challenge_response() {
        let challenge_b64 = crate::base64::encode(b"<challenge@server>");
        let lines: Vec<String> = vec![
            "+OK ready\r\n".to_string(),
            format!("+ {}\r\n", challenge_b64),
            "+OK authenticated\r\n".to_string(),
        ];

        // Build transport with owned strings leaked for the test.
        let line_refs: Vec<&str> = lines.iter().map(|s| format_leak(s.clone())).collect();
        let tp = MockTransport::new(line_refs);
        let mut s = Pop3Session::new(tp);
        s.connect("localhost", 110, Pop3Security::Plain).unwrap();
        s.auth_cram_md5("user", "pass").unwrap();
        assert!(s.is_authenticated());
    }

    // ── Quit ────────────────────────────────────────────────────────────

    #[test]
    fn quit_sends_quit_and_closes() {
        let mut s = session_with("ready", vec![
            "+OK bye\r\n",
        ]);
        s.quit();
        assert!(!s.is_connected());
        let sent = s.transport().sent_str();
        assert!(sent.contains("QUIT\r\n"));
        assert!(s.transport().closed);
    }

    // ── strip_crlf ─────────────────────────────────────────────────────

    #[test]
    fn strip_crlf_variants() {
        assert_eq!(strip_crlf(b"+OK hello\r\n"), "+OK hello");
        assert_eq!(strip_crlf(b"+OK hello\n"), "+OK hello");
        assert_eq!(strip_crlf(b"+OK hello"), "+OK hello");
        assert_eq!(strip_crlf(b"\r\n"), "");
    }

    #[test]
    fn strip_crlf_bytes_variants() {
        assert_eq!(strip_crlf_bytes(b"hello\r\n"), b"hello");
        assert_eq!(strip_crlf_bytes(b"hello\n"), b"hello");
        assert_eq!(strip_crlf_bytes(b"hello"), b"hello");
        assert_eq!(strip_crlf_bytes(b"\r\n"), b"");
        assert_eq!(strip_crlf_bytes(b""), b"");
    }

    // ── UIDL parsing edge cases ─────────────────────────────────────────

    #[test]
    fn uidl_with_spaces_in_id() {
        // Some servers include spaces in UIDL values — splitn(2, ...) handles this.
        let mut s = session_with("ready", vec![
            "+OK 1 500\r\n",   // STAT
            "+OK\r\n",          // LIST
            "1 500\r\n",
            ".\r\n",
            "+OK\r\n",          // UIDL
            "1 abc 123 xyz\r\n",
            ".\r\n",
        ]);
        let msgs = s.list().unwrap();
        assert_eq!(msgs[0].uidl, "abc 123 xyz");
    }

    #[test]
    fn uidl_non_sequential_numbers() {
        // After deletion, message numbers may have gaps.
        let mut s = session_with("ready", vec![
            "+OK 2 3000\r\n",  // STAT
            "+OK\r\n",          // LIST
            "1 1000\r\n",
            "3 2000\r\n",
            ".\r\n",
            "+OK\r\n",          // UIDL
            "1 uid-a\r\n",
            "3 uid-c\r\n",
            ".\r\n",
        ]);
        let msgs = s.list().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].number, 1);
        assert_eq!(msgs[0].uidl, "uid-a");
        assert_eq!(msgs[1].number, 3);
        assert_eq!(msgs[1].uidl, "uid-c");
    }
}
