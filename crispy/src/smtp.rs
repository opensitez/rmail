//! SMTP client library (RFC 5321).
//!
//! Standalone implementation — no Eudora/GTK dependency.
//! Transport is pluggable via [`crate::transport::Transport`].
//!
//! # Example
//! ```ignore
//! let mut session = SmtpSession::new(transport, "localhost");
//! session.connect("mail.example.com", 587, Security::StartTls)?;
//! session.auth_plain("user", "pass")?;
//! session.send_message("user@example.com", &["dest@example.com"], message)?;
//! session.close();
//! ```

use std::fmt;
use std::io;

use crate::auth;
use crate::transport::Transport;

// ── Error types ──────────────────────────────────────────────────────

/// SMTP-specific errors.
#[derive(Debug)]
pub enum Error {
    /// Transport/IO failure.
    Io(io::Error),
    /// Server returned an error reply code.
    Reply { code: u16, message: String },
    /// Protocol violation (unparseable reply, unexpected state).
    Protocol(String),
    /// Not connected.
    NotConnected,
    /// Authentication failed.
    AuthFailed { code: u16, message: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "SMTP I/O error: {}", e),
            Error::Reply { code, message } => write!(f, "SMTP {} {}", code, message),
            Error::Protocol(msg) => write!(f, "SMTP protocol error: {}", msg),
            Error::NotConnected => write!(f, "SMTP not connected"),
            Error::AuthFailed { code, message } => {
                write!(f, "SMTP auth failed: {} {}", code, message)
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

// ── Security mode ────────────────────────────────────────────────────

/// Connection security mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// No encryption.
    Plain,
    /// Upgrade via STARTTLS after connecting.
    StartTls,
    /// Implicit TLS (SMTPS, typically port 465).
    Ssl,
}

// ── SMTP command IDs ─────────────────────────────────────────────────

/// SMTP command identifiers (RFC 5321).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Helo,
    Ehlo,
    MailFrom,
    RcptTo,
    Data,
    Rset,
    Quit,
    Auth,
    StartTls,
    Noop,
    Vrfy,
    Expn,
    Help,
}

impl Command {
    /// Returns the SMTP command keyword.
    pub fn as_str(&self) -> &'static str {
        match self {
            Command::Helo => "HELO",
            Command::Ehlo => "EHLO",
            Command::MailFrom => "MAIL FROM:",
            Command::RcptTo => "RCPT TO:",
            Command::Data => "DATA",
            Command::Rset => "RSET",
            Command::Quit => "QUIT",
            Command::Auth => "AUTH",
            Command::StartTls => "STARTTLS",
            Command::Noop => "NOOP",
            Command::Vrfy => "VRFY",
            Command::Expn => "EXPN",
            Command::Help => "HELP",
        }
    }
}

// ── Reply parsing ────────────────────────────────────────────────────

/// A parsed SMTP reply (possibly multi-line).
#[derive(Debug, Clone)]
pub struct Reply {
    /// 3-digit reply code.
    pub code: u16,
    /// Text of the last line (after the code and separator).
    pub text: String,
    /// All lines of text (after code+separator), for multi-line replies.
    pub lines: Vec<String>,
}

impl Reply {
    /// Reply class (first digit): 2=OK, 3=intermediate, 4=temp error, 5=perm error.
    pub fn class(&self) -> u16 {
        self.code / 100
    }

    /// True if reply indicates success (2xx).
    pub fn is_ok(&self) -> bool {
        self.class() == 2
    }

    /// True if reply is a temporary error (4xx).
    pub fn is_temp_error(&self) -> bool {
        self.class() == 4
    }

    /// True if reply is a permanent error (5xx).
    pub fn is_perm_error(&self) -> bool {
        self.class() == 5
    }
}

/// Parse a single SMTP reply line.
///
/// Returns `(code, text_after_separator, has_continuation)`.
/// `has_continuation` is true when the separator is '-' (multi-line reply).
fn parse_reply_line(line: &[u8]) -> std::result::Result<(u16, String, bool), Error> {
    // Strip trailing CRLF/LF
    let line = strip_crlf(line);

    if line.len() < 3 {
        return Err(Error::Protocol("reply too short".into()));
    }

    // Parse 3-digit code
    let d0 = digit(line[0])?;
    let d1 = digit(line[1])?;
    let d2 = digit(line[2])?;
    let code = d0 as u16 * 100 + d1 as u16 * 10 + d2 as u16;

    let (text, more) = if line.len() > 3 {
        let sep = line[3];
        let more = sep == b'-';
        let text = if line.len() > 4 {
            String::from_utf8_lossy(&line[4..]).into_owned()
        } else {
            String::new()
        };
        (text, more)
    } else {
        (String::new(), false)
    };

    Ok((code, text, more))
}

fn digit(b: u8) -> std::result::Result<u8, Error> {
    if b.is_ascii_digit() {
        Ok(b - b'0')
    } else {
        Err(Error::Protocol(format!("expected digit, got {:?}", b as char)))
    }
}

fn strip_crlf(data: &[u8]) -> &[u8] {
    let mut end = data.len();
    while end > 0 && (data[end - 1] == b'\r' || data[end - 1] == b'\n') {
        end -= 1;
    }
    &data[..end]
}

// ── EHLO capabilities ───────────────────────────────────────────────

/// EHLO capabilities parsed from the server response.
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    /// Server supports STARTTLS.
    pub starttls: bool,
    /// Server supports 8BITMIME.
    pub eightbitmime: bool,
    /// Server supports PIPELINING.
    pub pipelining: bool,
    /// Server advertises AUTH with these mechanisms.
    pub auth_mechanisms: Vec<auth::Mechanism>,
    /// Raw AUTH mechanism string from EHLO.
    pub auth_mechs_raw: String,
    /// Maximum message size (0 = not advertised).
    pub max_size: u64,
}

impl Capabilities {
    /// Parse a single EHLO capability line (text after "250-" or "250 ").
    fn parse_line(&mut self, line: &str) {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("STARTTLS") {
            self.starttls = true;
        } else if upper.starts_with("8BITMIME") {
            self.eightbitmime = true;
        } else if upper.starts_with("PIPELINING") {
            self.pipelining = true;
        } else if upper.starts_with("SIZE") {
            let rest = line[4..].trim();
            if let Ok(sz) = rest.parse::<u64>() {
                self.max_size = sz;
            }
        } else if upper.starts_with("AUTH") {
            let rest = line[4..].trim_start_matches(|c| c == ' ' || c == '=');
            self.auth_mechs_raw = rest.to_string();
            self.auth_mechanisms.clear();
            for name in rest.split_whitespace() {
                if let Some(m) = auth::Mechanism::from_name(name) {
                    self.auth_mechanisms.push(m);
                }
            }
        }
    }

    /// Check if a specific auth mechanism is supported.
    pub fn has_auth(&self, mech: auth::Mechanism) -> bool {
        self.auth_mechanisms.contains(&mech)
    }
}

// ── Dot-stuffing ─────────────────────────────────────────────────────

/// Dot-stuff message data for DATA transmission (RFC 5321 Section 4.5.2).
///
/// Any line beginning with a period gets an extra period prepended.
/// `newline` is the line ending to detect (typically "\r\n").
/// `nl_state` tracks position within the newline sequence across calls;
/// initialize to `newline.len()` (as if a newline just occurred).
///
/// Returns the dot-stuffed output.
pub fn dot_stuff(input: &[u8], newline: &[u8], nl_state: &mut usize) -> Vec<u8> {
    let nl_len = newline.len();
    let mut out = Vec::with_capacity(input.len() + input.len() / 40);
    let mut state = *nl_state;

    for &byte in input {
        if byte == b'.' && state == nl_len {
            out.push(b'.'); // stuff extra period
            state = 0;
        } else {
            if state == nl_len {
                state = 0;
            }
            if state < nl_len && byte == newline[state] {
                state += 1;
            } else {
                state = 0;
            }
        }
        out.push(byte);
    }

    *nl_state = state;
    out
}

// ── Date formatting ──────────────────────────────────────────────────

static DAY_NAMES: &[&str] = &["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
static MONTH_NAMES: &[&str] = &[
    "Jan", "Feb", "Mar", "Apr", "May", "Jun",
    "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Simple broken-down time (UTC-based, then shifted by tz_offset).
struct DateTime {
    year: i32,
    month: u32,  // 0-11
    mday: u32,   // 1-31
    hour: u32,
    min: u32,
    sec: u32,
    wday: u32,   // 0=Sun
}

/// Convert unix timestamp to broken-down time (UTC).
fn gmtime(ts: i64) -> DateTime {
    // Days from epoch, with proper handling of negative timestamps
    let secs_per_day: i64 = 86400;
    let mut days = ts.div_euclid(secs_per_day);
    let day_secs = ts.rem_euclid(secs_per_day);

    let hour = (day_secs / 3600) as u32;
    let min = ((day_secs % 3600) / 60) as u32;
    let sec = (day_secs % 60) as u32;

    // Day of week: Jan 1 1970 was Thursday (4)
    let wday = ((days % 7 + 4 + 7) % 7) as u32;

    // Year calculation
    days += 719468; // shift epoch to 0000-03-01
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = (days - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    DateTime {
        year: year as i32,
        month: m - 1, // 0-based
        mday: d,
        hour,
        min,
        sec,
        wday,
    }
}

/// Format an RFC 5322 date string.
///
/// `utc_seconds`: Unix timestamp.
/// `tz_offset_seconds`: Timezone offset in seconds (e.g. +7200 for UTC+2).
pub fn format_date(utc_seconds: i64, tz_offset_seconds: i64) -> String {
    let local_ts = utc_seconds + tz_offset_seconds;
    let dt = gmtime(local_ts);
    let tz = format_zone(tz_offset_seconds);

    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} {}",
        DAY_NAMES[dt.wday as usize],
        dt.mday,
        MONTH_NAMES[dt.month as usize],
        dt.year,
        dt.hour,
        dt.min,
        dt.sec,
        tz,
    )
}

/// Format timezone offset as "+HHMM" or "-HHMM".
pub fn format_zone(tz_offset_seconds: i64) -> String {
    let total_min = tz_offset_seconds / 60;
    let neg = total_min < 0;
    let abs_min = total_min.unsigned_abs();
    format!(
        "{}{:02}{:02}",
        if neg { '-' } else { '+' },
        abs_min / 60,
        abs_min % 60,
    )
}

// ── Debug callback ───────────────────────────────────────────────────

/// Optional debug callback for protocol tracing.
pub type DebugFn = Box<dyn Fn(&str) + Send>;

// ── SMTP Session ─────────────────────────────────────────────────────

/// An SMTP client session.
///
/// Wraps a [`Transport`] and provides the full SMTP command set:
/// connect, EHLO, STARTTLS, AUTH, MAIL FROM, RCPT TO, DATA, QUIT.
pub struct SmtpSession<T: Transport> {
    pub tp: T,
    /// Parsed EHLO capabilities.
    pub caps: Capabilities,
    /// Reply code from last server response.
    pub last_code: u16,
    /// Text from last server response.
    pub last_reply: String,
    /// Local hostname for EHLO.
    pub local_hostname: String,
    /// True if TCP connection is open.
    pub connected: bool,
    /// True if AUTH succeeded.
    pub authenticated: bool,
    /// Optional debug callback.
    debug: Option<DebugFn>,
}

impl<T: Transport> SmtpSession<T> {
    /// Create a new session. Does not connect yet.
    pub fn new(tp: T, local_hostname: &str) -> Self {
        SmtpSession {
            tp,
            caps: Capabilities::default(),
            last_code: 0,
            last_reply: String::new(),
            local_hostname: if local_hostname.is_empty() {
                "localhost".into()
            } else {
                local_hostname.into()
            },
            connected: false,
            authenticated: false,
            debug: None,
        }
    }

    /// Set a debug callback for protocol tracing.
    pub fn set_debug<F: Fn(&str) + Send + 'static>(&mut self, f: F) {
        self.debug = Some(Box::new(f));
    }

    /// Get a mutable reference to the underlying transport.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.tp
    }

    /// Get a reference to the underlying transport.
    pub fn transport(&self) -> &T {
        &self.tp
    }

    // ── Internal helpers ──

    fn dbg(&self, msg: &str) {
        if let Some(ref f) = self.debug {
            f(msg);
        }
    }

    fn send_bytes(&mut self, data: &[u8]) -> Result<()> {
        self.tp.send(data).map_err(Error::Io)
    }

    fn send_line(&mut self, line: &str) -> Result<()> {
        self.dbg(&format!("C: {}", line));
        self.send_bytes(line.as_bytes())?;
        self.send_bytes(b"\r\n")?;
        Ok(())
    }

    /// Read a complete (possibly multi-line) SMTP reply.
    pub fn read_reply(&mut self) -> Result<Reply> {
        let mut lines = Vec::new();
        let mut code = 0u16;

        loop {
            let raw = self.tp.recv_line().map_err(Error::Io)?;
            let (c, text, more) = parse_reply_line(&raw)?;

            if code == 0 {
                code = c;
            } else if c != code {
                return Err(Error::Protocol(format!(
                    "reply code changed mid-reply: {} vs {}", code, c
                )));
            }

            self.dbg(&format!("S: {}", String::from_utf8_lossy(strip_crlf(&raw))));
            lines.push(text);

            if !more {
                break;
            }
        }

        let text = lines.last().cloned().unwrap_or_default();
        self.last_code = code;
        self.last_reply = text.clone();

        Ok(Reply { code, text, lines })
    }

    /// Read the EHLO response, parsing capability lines.
    fn read_ehlo_reply(&mut self) -> Result<Reply> {
        self.caps = Capabilities::default();
        let mut lines = Vec::new();
        let mut code = 0u16;
        let mut first = true;

        loop {
            let raw = self.tp.recv_line().map_err(Error::Io)?;
            let (c, text, more) = parse_reply_line(&raw)?;

            if code == 0 {
                code = c;
            }

            self.dbg(&format!("S: {}", String::from_utf8_lossy(strip_crlf(&raw))));

            if first {
                first = false; // skip greeting line
            } else {
                self.caps.parse_line(&text);
            }

            lines.push(text);

            if !more {
                break;
            }
        }

        let text = lines.last().cloned().unwrap_or_default();
        self.last_code = code;
        self.last_reply = text.clone();

        Ok(Reply { code, text, lines })
    }

    /// Send a raw command line, read and return the reply.
    pub fn command(&mut self, cmd_line: &str) -> Result<Reply> {
        self.send_line(cmd_line)?;
        self.read_reply()
    }

    fn require_ok(&self, reply: &Reply) -> Result<()> {
        if reply.is_ok() {
            Ok(())
        } else {
            Err(Error::Reply {
                code: reply.code,
                message: reply.text.clone(),
            })
        }
    }

    // ── Session lifecycle ──

    /// Connect to an SMTP server, read greeting, do EHLO, optionally STARTTLS.
    pub fn connect(&mut self, host: &str, port: u16, security: Security) -> Result<()> {
        self.tp.connect(host, port).map_err(Error::Io)?;
        self.connected = true;

        // Read greeting
        let greeting = self.read_reply()?;
        if !greeting.is_ok() {
            return Err(Error::Reply {
                code: greeting.code,
                message: greeting.text,
            });
        }

        // EHLO (falls back to HELO)
        self.ehlo()?;

        // STARTTLS if requested and supported
        if security == Security::StartTls && self.caps.starttls {
            self.starttls()?;
            // Re-EHLO after TLS upgrade
            self.ehlo()?;
        }

        Ok(())
    }

    /// Send EHLO, falling back to HELO if EHLO fails.
    pub fn ehlo(&mut self) -> Result<Reply> {
        let cmd = format!("EHLO {}", self.local_hostname);
        self.send_line(&cmd)?;
        let reply = self.read_ehlo_reply()?;

        if reply.is_ok() {
            return Ok(reply);
        }

        // Fall back to HELO
        let cmd = format!("HELO {}", self.local_hostname);
        self.send_line(&cmd)?;
        let reply = self.read_reply()?;
        Ok(reply)
    }

    /// Upgrade to TLS via STARTTLS.
    pub fn starttls(&mut self) -> Result<()> {
        let reply = self.command("STARTTLS")?;
        if !reply.is_ok() {
            return Err(Error::Reply {
                code: reply.code,
                message: reply.text,
            });
        }
        self.tp.start_tls().map_err(Error::Io)?;
        Ok(())
    }

    /// Send QUIT and read the reply.
    pub fn quit(&mut self) -> Result<Reply> {
        self.command("QUIT")
    }

    /// Close the session gracefully (QUIT + transport close).
    pub fn close(&mut self) {
        if self.connected {
            let _ = self.quit();
            self.tp.close();
            self.connected = false;
        }
    }

    // ── Authentication ──

    /// Authenticate using PLAIN (RFC 4616).
    pub fn auth_plain(&mut self, user: &str, pass: &str) -> Result<()> {
        let encoded = auth::plain_response(user, pass);
        let cmd = format!("AUTH PLAIN {}", encoded);
        let reply = self.command(&cmd)?;

        if reply.code == 235 {
            self.authenticated = true;
            return Ok(());
        }

        Err(Error::AuthFailed {
            code: reply.code,
            message: reply.text,
        })
    }

    /// Authenticate using LOGIN.
    pub fn auth_login(&mut self, user: &str, pass: &str) -> Result<()> {
        let reply = self.command("AUTH LOGIN")?;
        if reply.code != 334 {
            return Err(Error::AuthFailed {
                code: reply.code,
                message: reply.text,
            });
        }

        // Send base64-encoded username
        let u_b64 = auth::login_username(user);
        let reply = self.command(&u_b64)?;
        if reply.code != 334 {
            return Err(Error::AuthFailed {
                code: reply.code,
                message: reply.text,
            });
        }

        // Send base64-encoded password
        let p_b64 = auth::login_password(pass);
        let reply = self.command(&p_b64)?;

        if reply.code == 235 {
            self.authenticated = true;
            return Ok(());
        }

        Err(Error::AuthFailed {
            code: reply.code,
            message: reply.text,
        })
    }

    /// Authenticate using CRAM-MD5 (RFC 2195).
    pub fn auth_cram_md5(&mut self, user: &str, pass: &str) -> Result<()> {
        // Step 1: initiate, get challenge
        let reply = self.command("AUTH CRAM-MD5")?;
        if reply.code != 334 {
            return Err(Error::AuthFailed {
                code: reply.code,
                message: reply.text,
            });
        }

        // Step 2: compute HMAC-MD5 response
        let response = auth::cram_md5_response(user, pass, &reply.text).ok_or_else(|| {
            Error::Protocol("failed to compute CRAM-MD5 response".into())
        })?;

        // Step 3: send response
        let reply = self.command(&response)?;

        if reply.code == 235 {
            self.authenticated = true;
            return Ok(());
        }

        Err(Error::AuthFailed {
            code: reply.code,
            message: reply.text,
        })
    }

    /// Authenticate using XOAUTH2 (Google/Outlook).
    pub fn auth_xoauth2(&mut self, user: &str, token: &str) -> Result<()> {
        let encoded = auth::xoauth2_response(user, token);
        let cmd = format!("AUTH XOAUTH2 {}", encoded);
        let reply = self.command(&cmd)?;

        if reply.code == 235 {
            self.authenticated = true;
            return Ok(());
        }

        Err(Error::AuthFailed {
            code: reply.code,
            message: reply.text,
        })
    }

    /// Authenticate using DIGEST-MD5 (RFC 2831).
    pub fn auth_digest_md5(&mut self, user: &str, pass: &str) -> Result<()> {
        // Step 1: initiate, get challenge
        let reply = self.command("AUTH DIGEST-MD5")?;
        if reply.code != 334 {
            return Err(Error::AuthFailed {
                code: reply.code,
                message: reply.text,
            });
        }

        // Step 2: compute response
        let response =
            auth::digest_md5_response(user, pass, &reply.text, "smtp").ok_or_else(|| {
                Error::Protocol("failed to compute DIGEST-MD5 response".into())
            })?;

        // Step 3: send response
        let reply = self.command(&response)?;

        // Server may send 334 with rspauth, then we send empty line
        if reply.code == 334 {
            let reply = self.command("")?;
            if reply.code == 235 {
                self.authenticated = true;
                return Ok(());
            }
            return Err(Error::AuthFailed {
                code: reply.code,
                message: reply.text,
            });
        }

        if reply.code == 235 {
            self.authenticated = true;
            return Ok(());
        }

        Err(Error::AuthFailed {
            code: reply.code,
            message: reply.text,
        })
    }

    /// Send AUTH with an arbitrary mechanism and optional initial response.
    /// Returns the server reply.
    pub fn auth_begin(&mut self, mech: &str, initial_response: Option<&str>) -> Result<Reply> {
        let cmd = match initial_response {
            Some(ir) => format!("AUTH {} {}", mech, ir),
            None => format!("AUTH {}", mech),
        };
        self.command(&cmd)
    }

    /// Send a continuation response during SASL exchange.
    pub fn auth_respond(&mut self, response: &str) -> Result<Reply> {
        self.command(response)
    }

    /// Cancel an in-progress SASL exchange.
    pub fn auth_cancel(&mut self) -> Result<Reply> {
        self.command("*")
    }

    // ── Message sending ──

    /// MAIL FROM command.
    pub fn mail_from(&mut self, addr: &str) -> Result<Reply> {
        let cmd = format!("MAIL FROM:<{}>", addr);
        self.command(&cmd)
    }

    /// RCPT TO command.
    pub fn rcpt_to(&mut self, addr: &str) -> Result<Reply> {
        let cmd = format!("RCPT TO:<{}>", addr);
        self.command(&cmd)
    }

    /// DATA command (initiates data transfer, expects 354).
    pub fn data_begin(&mut self) -> Result<Reply> {
        self.command("DATA")
    }

    /// Send raw data bytes during DATA phase (no framing added).
    pub fn data_send(&mut self, data: &[u8]) -> Result<()> {
        self.send_bytes(data)
    }

    /// End DATA phase: send CRLF.CRLF and read final reply.
    pub fn data_end(&mut self) -> Result<Reply> {
        self.send_bytes(b"\r\n.\r\n")?;
        self.read_reply()
    }

    /// RSET command.
    pub fn rset(&mut self) -> Result<Reply> {
        self.command("RSET")
    }

    /// NOOP command (keep-alive / connection test).
    pub fn noop(&mut self) -> Result<Reply> {
        self.command("NOOP")
    }

    /// VRFY command.
    pub fn vrfy(&mut self, addr: &str) -> Result<Reply> {
        let cmd = format!("VRFY {}", addr);
        self.command(&cmd)
    }

    /// High-level send: RSET, MAIL FROM, RCPT TO for each recipient, DATA with dot-stuffing.
    ///
    /// `from`: envelope sender address (e.g. "user@example.com").
    /// `rcpts`: slice of recipient addresses.
    /// `message`: raw RFC 5322 message (headers + body).
    pub fn send_message(&mut self, from: &str, rcpts: &[&str], message: &[u8]) -> Result<()> {
        // RSET to clear previous state
        let rset_reply = self.rset()?;
        // Ignore RSET errors (some servers return errors if no prior transaction)
        if !rset_reply.is_ok() && rset_reply.code != 0 {
            // non-fatal, continue
        }

        // MAIL FROM
        let reply = self.mail_from(from)?;
        self.require_ok(&reply)?;

        // RCPT TO for each recipient
        for rcpt in rcpts {
            let reply = self.rcpt_to(rcpt)?;
            self.require_ok(&reply)?;
        }

        // DATA
        let reply = self.data_begin()?;
        if reply.code != 354 && !reply.is_ok() {
            return Err(Error::Reply {
                code: reply.code,
                message: reply.text,
            });
        }

        // Send message body with dot-stuffing
        let mut nl_state = 2usize; // as if we just saw CRLF
        let stuffed = dot_stuff(message, b"\r\n", &mut nl_state);
        self.data_send(&stuffed)?;

        // End DATA
        let reply = self.data_end()?;
        self.require_ok(&reply)?;

        Ok(())
    }
}

impl<T: Transport> Drop for SmtpSession<T> {
    fn drop(&mut self) {
        if self.connected {
            self.tp.close();
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base64;
    use std::collections::VecDeque;

    // ── Reply parsing tests ──

    #[test]
    fn parse_simple_reply() {
        let (code, text, more) = parse_reply_line(b"250 OK\r\n").unwrap();
        assert_eq!(code, 250);
        assert_eq!(text, "OK");
        assert!(!more);
    }

    #[test]
    fn parse_multiline_reply() {
        let (code, text, more) = parse_reply_line(b"250-mail.example.com\r\n").unwrap();
        assert_eq!(code, 250);
        assert_eq!(text, "mail.example.com");
        assert!(more);
    }

    #[test]
    fn parse_reply_no_text() {
        let (code, text, more) = parse_reply_line(b"250\r\n").unwrap();
        assert_eq!(code, 250);
        assert_eq!(text, "");
        assert!(!more);
    }

    #[test]
    fn parse_reply_354() {
        let (code, text, more) =
            parse_reply_line(b"354 Start mail input; end with <CRLF>.<CRLF>\r\n").unwrap();
        assert_eq!(code, 354);
        assert!(text.contains("Start mail input"));
        assert!(!more);
    }

    #[test]
    fn parse_reply_error_codes() {
        let (code, _, _) = parse_reply_line(b"421 Service not available\r\n").unwrap();
        assert_eq!(code, 421);

        let (code, _, _) = parse_reply_line(b"550 Mailbox not found\r\n").unwrap();
        assert_eq!(code, 550);
    }

    #[test]
    fn parse_reply_too_short() {
        assert!(parse_reply_line(b"25\r\n").is_err());
    }

    #[test]
    fn parse_reply_non_digit() {
        assert!(parse_reply_line(b"ABC OK\r\n").is_err());
    }

    #[test]
    fn reply_class() {
        let r = Reply { code: 250, text: "OK".into(), lines: vec!["OK".into()] };
        assert!(r.is_ok());
        assert!(!r.is_temp_error());
        assert!(!r.is_perm_error());

        let r = Reply { code: 451, text: "".into(), lines: vec![] };
        assert!(r.is_temp_error());

        let r = Reply { code: 550, text: "".into(), lines: vec![] };
        assert!(r.is_perm_error());
    }

    // ── Capability parsing tests ──

    #[test]
    fn caps_starttls() {
        let mut caps = Capabilities::default();
        caps.parse_line("STARTTLS");
        assert!(caps.starttls);
    }

    #[test]
    fn caps_8bitmime() {
        let mut caps = Capabilities::default();
        caps.parse_line("8BITMIME");
        assert!(caps.eightbitmime);
    }

    #[test]
    fn caps_pipelining() {
        let mut caps = Capabilities::default();
        caps.parse_line("PIPELINING");
        assert!(caps.pipelining);
    }

    #[test]
    fn caps_size() {
        let mut caps = Capabilities::default();
        caps.parse_line("SIZE 52428800");
        assert_eq!(caps.max_size, 52428800);
    }

    #[test]
    fn caps_size_no_value() {
        let mut caps = Capabilities::default();
        caps.parse_line("SIZE");
        assert_eq!(caps.max_size, 0);
    }

    #[test]
    fn caps_auth_mechanisms() {
        let mut caps = Capabilities::default();
        caps.parse_line("AUTH PLAIN LOGIN CRAM-MD5 XOAUTH2");
        assert!(caps.has_auth(auth::Mechanism::Plain));
        assert!(caps.has_auth(auth::Mechanism::Login));
        assert!(caps.has_auth(auth::Mechanism::CramMd5));
        assert!(caps.has_auth(auth::Mechanism::XOAuth2));
        assert!(!caps.has_auth(auth::Mechanism::DigestMd5));
    }

    #[test]
    fn caps_auth_with_equals() {
        // Some servers use AUTH=PLAIN LOGIN
        let mut caps = Capabilities::default();
        caps.parse_line("AUTH=PLAIN LOGIN");
        assert!(caps.has_auth(auth::Mechanism::Plain));
        assert!(caps.has_auth(auth::Mechanism::Login));
    }

    #[test]
    fn caps_case_insensitive() {
        let mut caps = Capabilities::default();
        caps.parse_line("starttls");
        assert!(caps.starttls);

        let mut caps = Capabilities::default();
        caps.parse_line("Pipelining");
        assert!(caps.pipelining);
    }

    // ── Command formatting tests ──

    #[test]
    fn command_strings() {
        assert_eq!(Command::Ehlo.as_str(), "EHLO");
        assert_eq!(Command::MailFrom.as_str(), "MAIL FROM:");
        assert_eq!(Command::RcptTo.as_str(), "RCPT TO:");
        assert_eq!(Command::Data.as_str(), "DATA");
        assert_eq!(Command::Quit.as_str(), "QUIT");
        assert_eq!(Command::StartTls.as_str(), "STARTTLS");
        assert_eq!(Command::Auth.as_str(), "AUTH");
        assert_eq!(Command::Noop.as_str(), "NOOP");
        assert_eq!(Command::Rset.as_str(), "RSET");
    }

    // ── Dot-stuffing tests ──

    #[test]
    fn dot_stuff_no_dots() {
        let mut state = 2;
        let result = dot_stuff(b"Hello\r\nWorld\r\n", b"\r\n", &mut state);
        assert_eq!(result, b"Hello\r\nWorld\r\n");
    }

    #[test]
    fn dot_stuff_leading_dot() {
        let mut state = 2; // as if just saw newline
        let result = dot_stuff(b".hidden\r\n", b"\r\n", &mut state);
        assert_eq!(result, b"..hidden\r\n");
    }

    #[test]
    fn dot_stuff_dot_after_newline() {
        let mut state = 0;
        let result = dot_stuff(b"line1\r\n.line2\r\n", b"\r\n", &mut state);
        assert_eq!(result, b"line1\r\n..line2\r\n");
    }

    #[test]
    fn dot_stuff_lone_dot() {
        let mut state = 2;
        let result = dot_stuff(b".\r\n", b"\r\n", &mut state);
        assert_eq!(result, b"..\r\n");
    }

    #[test]
    fn dot_stuff_dot_mid_line() {
        let mut state = 0;
        let result = dot_stuff(b"no.stuff.here\r\n", b"\r\n", &mut state);
        assert_eq!(result, b"no.stuff.here\r\n");
    }

    #[test]
    fn dot_stuff_multiple_dots() {
        let mut state = 2;
        let result = dot_stuff(b"..already stuffed\r\n", b"\r\n", &mut state);
        // Leading ".." at start of line becomes "..."
        assert_eq!(result, b"...already stuffed\r\n");
    }

    #[test]
    fn dot_stuff_state_across_calls() {
        let mut state = 0;
        let r1 = dot_stuff(b"line\r", b"\r\n", &mut state);
        assert_eq!(state, 1); // saw \r, waiting for \n
        let r2 = dot_stuff(b"\n.dot\r\n", b"\r\n", &mut state);
        assert_eq!(r1, b"line\r");
        assert_eq!(r2, b"\n..dot\r\n");
    }

    // ── Date formatting tests ──

    #[test]
    fn format_zone_positive() {
        assert_eq!(format_zone(7200), "+0200");
    }

    #[test]
    fn format_zone_negative() {
        assert_eq!(format_zone(-18000), "-0500");
    }

    #[test]
    fn format_zone_utc() {
        assert_eq!(format_zone(0), "+0000");
    }

    #[test]
    fn format_zone_half_hour() {
        assert_eq!(format_zone(19800), "+0530"); // IST
    }

    #[test]
    fn format_date_epoch() {
        let d = format_date(0, 0);
        assert!(d.contains("Thu"));
        assert!(d.contains("01 Jan 1970"));
        assert!(d.contains("00:00:00"));
        assert!(d.contains("+0000"));
    }

    #[test]
    fn format_date_with_tz() {
        // 2024-01-15 12:00:00 UTC, +0200
        let d = format_date(1705320000, 7200);
        assert!(d.contains("+0200"));
        assert!(d.contains("14:00:00")); // 12 UTC + 2 = 14
    }

    // ── Mock transport for session tests ──

    /// A mock transport that records sent data and plays back scripted replies.
    struct MockTransport {
        replies: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
        tls_upgraded: bool,
    }

    impl MockTransport {
        fn new(replies: Vec<&str>) -> Self {
            MockTransport {
                replies: replies.into_iter().map(|s| s.as_bytes().to_vec()).collect(),
                sent: Vec::new(),
                tls_upgraded: false,
            }
        }

        fn sent_lines(&self) -> Vec<String> {
            let mut all = Vec::new();
            let mut buf = Vec::new();
            for chunk in &self.sent {
                buf.extend_from_slice(chunk);
            }
            // Split on CRLF
            let s = String::from_utf8_lossy(&buf);
            for line in s.split("\r\n") {
                if !line.is_empty() {
                    all.push(line.to_string());
                }
            }
            all
        }
    }

    impl Transport for MockTransport {
        fn connect(&mut self, _host: &str, _port: u16) -> io::Result<()> {
            Ok(())
        }

        fn send(&mut self, data: &[u8]) -> io::Result<()> {
            self.sent.push(data.to_vec());
            Ok(())
        }

        fn recv_line(&mut self) -> io::Result<Vec<u8>> {
            self.replies.pop_front().ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "no more scripted replies")
            })
        }

        fn start_tls(&mut self) -> io::Result<()> {
            self.tls_upgraded = true;
            Ok(())
        }

        fn close(&mut self) {}
    }

    // ── Session tests ──

    #[test]
    fn session_read_reply_simple() {
        let tp = MockTransport::new(vec!["250 OK\r\n"]);
        let mut s = SmtpSession::new(tp, "test.local");
        let reply = s.read_reply().unwrap();
        assert_eq!(reply.code, 250);
        assert_eq!(reply.text, "OK");
        assert!(reply.is_ok());
    }

    #[test]
    fn session_read_reply_multiline() {
        let tp = MockTransport::new(vec![
            "250-mail.example.com\r\n",
            "250-STARTTLS\r\n",
            "250-AUTH PLAIN LOGIN\r\n",
            "250 SIZE 52428800\r\n",
        ]);
        let mut s = SmtpSession::new(tp, "test.local");
        let reply = s.read_reply().unwrap();
        assert_eq!(reply.code, 250);
        assert_eq!(reply.lines.len(), 4);
    }

    #[test]
    fn session_ehlo_parses_caps() {
        let tp = MockTransport::new(vec![
            // EHLO response
            "250-mail.example.com Hello\r\n",
            "250-STARTTLS\r\n",
            "250-8BITMIME\r\n",
            "250-PIPELINING\r\n",
            "250-SIZE 10485760\r\n",
            "250 AUTH PLAIN LOGIN CRAM-MD5\r\n",
        ]);
        let mut s = SmtpSession::new(tp, "test.local");
        let reply = s.read_ehlo_reply().unwrap();
        assert!(reply.is_ok());
        assert!(s.caps.starttls);
        assert!(s.caps.eightbitmime);
        assert!(s.caps.pipelining);
        assert_eq!(s.caps.max_size, 10485760);
        assert!(s.caps.has_auth(auth::Mechanism::Plain));
        assert!(s.caps.has_auth(auth::Mechanism::Login));
        assert!(s.caps.has_auth(auth::Mechanism::CramMd5));
    }

    #[test]
    fn session_command_roundtrip() {
        let tp = MockTransport::new(vec!["250 2.1.0 Ok\r\n"]);
        let mut s = SmtpSession::new(tp, "test.local");
        let reply = s.command("MAIL FROM:<user@example.com>").unwrap();
        assert_eq!(reply.code, 250);

        let sent = s.tp.sent_lines();
        assert_eq!(sent[0], "MAIL FROM:<user@example.com>");
    }

    #[test]
    fn session_mail_from_format() {
        let tp = MockTransport::new(vec!["250 Ok\r\n"]);
        let mut s = SmtpSession::new(tp, "test.local");
        s.mail_from("sender@example.com").unwrap();

        let sent = s.tp.sent_lines();
        assert_eq!(sent[0], "MAIL FROM:<sender@example.com>");
    }

    #[test]
    fn session_rcpt_to_format() {
        let tp = MockTransport::new(vec!["250 Ok\r\n"]);
        let mut s = SmtpSession::new(tp, "test.local");
        s.rcpt_to("dest@example.com").unwrap();

        let sent = s.tp.sent_lines();
        assert_eq!(sent[0], "RCPT TO:<dest@example.com>");
    }

    #[test]
    fn session_auth_plain() {
        let tp = MockTransport::new(vec!["235 2.7.0 Authentication successful\r\n"]);
        let mut s = SmtpSession::new(tp, "test.local");
        s.auth_plain("user", "pass").unwrap();
        assert!(s.authenticated);

        let sent = s.tp.sent_lines();
        assert!(sent[0].starts_with("AUTH PLAIN "));
        // Verify the base64 payload decodes correctly
        let b64_part = &sent[0]["AUTH PLAIN ".len()..];
        let decoded = base64::decode(b64_part).unwrap();
        assert_eq!(decoded, b"\0user\0pass");
    }

    #[test]
    fn session_auth_login() {
        let tp = MockTransport::new(vec![
            "334 VXNlcm5hbWU6\r\n", // Username:
            "334 UGFzc3dvcmQ6\r\n", // Password:
            "235 Authentication successful\r\n",
        ]);
        let mut s = SmtpSession::new(tp, "test.local");
        s.auth_login("user", "pass").unwrap();
        assert!(s.authenticated);

        let sent = s.tp.sent_lines();
        assert_eq!(sent[0], "AUTH LOGIN");
        // sent[1] is base64(user), sent[2] is base64(pass)
        assert_eq!(base64::decode(&sent[1]).unwrap(), b"user");
        assert_eq!(base64::decode(&sent[2]).unwrap(), b"pass");
    }

    #[test]
    fn session_auth_cram_md5() {
        let challenge = base64::encode(b"<12345@server>");
        let reply_line = format!("334 {}\r\n", challenge);
        let tp = MockTransport::new(vec![
            &reply_line,
            "235 Authentication successful\r\n",
        ]);
        let mut s = SmtpSession::new(tp, "test.local");
        s.auth_cram_md5("user", "pass").unwrap();
        assert!(s.authenticated);

        let sent = s.tp.sent_lines();
        assert_eq!(sent[0], "AUTH CRAM-MD5");
        // Verify response decodes to "user <hex>"
        let decoded = base64::decode(&sent[1]).unwrap();
        let resp = String::from_utf8(decoded).unwrap();
        assert!(resp.starts_with("user "));
        assert_eq!(resp.len(), 5 + 32); // "user " + 32 hex chars
    }

    #[test]
    fn session_auth_xoauth2() {
        let tp = MockTransport::new(vec!["235 Accepted\r\n"]);
        let mut s = SmtpSession::new(tp, "test.local");
        s.auth_xoauth2("user@gmail.com", "ya29.token123").unwrap();
        assert!(s.authenticated);

        let sent = s.tp.sent_lines();
        assert!(sent[0].starts_with("AUTH XOAUTH2 "));
        let b64_part = &sent[0]["AUTH XOAUTH2 ".len()..];
        let decoded = String::from_utf8(base64::decode(b64_part).unwrap()).unwrap();
        assert_eq!(decoded, "user=user@gmail.com\x01auth=Bearer ya29.token123\x01\x01");
    }

    #[test]
    fn session_auth_plain_failure() {
        let tp = MockTransport::new(vec!["535 5.7.8 Authentication credentials invalid\r\n"]);
        let mut s = SmtpSession::new(tp, "test.local");
        let result = s.auth_plain("user", "wrongpass");
        assert!(result.is_err());
        assert!(!s.authenticated);
        match result.unwrap_err() {
            Error::AuthFailed { code, .. } => assert_eq!(code, 535),
            other => panic!("expected AuthFailed, got {:?}", other),
        }
    }

    #[test]
    fn session_connect_greeting_ehlo() {
        let tp = MockTransport::new(vec![
            // Greeting
            "220 mail.example.com ESMTP\r\n",
            // EHLO response
            "250-mail.example.com\r\n",
            "250-STARTTLS\r\n",
            "250 AUTH PLAIN\r\n",
        ]);
        let mut s = SmtpSession::new(tp, "test.local");
        s.connect("mail.example.com", 587, Security::Plain).unwrap();
        assert!(s.connected);
        assert!(s.caps.starttls);
        assert!(s.caps.has_auth(auth::Mechanism::Plain));
    }

    #[test]
    fn session_connect_starttls() {
        let tp = MockTransport::new(vec![
            // Greeting
            "220 mail.example.com ESMTP\r\n",
            // First EHLO
            "250-mail.example.com\r\n",
            "250 STARTTLS\r\n",
            // STARTTLS reply
            "220 2.0.0 Ready to start TLS\r\n",
            // Second EHLO (post-TLS)
            "250-mail.example.com\r\n",
            "250-8BITMIME\r\n",
            "250 AUTH PLAIN LOGIN\r\n",
        ]);
        let mut s = SmtpSession::new(tp, "test.local");
        s.connect("mail.example.com", 587, Security::StartTls).unwrap();
        assert!(s.connected);
        assert!(s.tp.tls_upgraded);
        assert!(s.caps.has_auth(auth::Mechanism::Plain));
        assert!(s.caps.has_auth(auth::Mechanism::Login));
    }

    #[test]
    fn session_send_message() {
        let tp = MockTransport::new(vec![
            "250 Ok\r\n",        // RSET
            "250 Ok\r\n",        // MAIL FROM
            "250 Ok\r\n",        // RCPT TO 1
            "250 Ok\r\n",        // RCPT TO 2
            "354 Go ahead\r\n",  // DATA
            "250 Ok: queued\r\n", // . (end of data)
        ]);
        let mut s = SmtpSession::new(tp, "test.local");
        s.send_message(
            "sender@example.com",
            &["alice@example.com", "bob@example.com"],
            b"Subject: Test\r\n\r\nHello world\r\n",
        )
        .unwrap();

        let sent = s.tp.sent_lines();
        assert_eq!(sent[0], "RSET");
        assert_eq!(sent[1], "MAIL FROM:<sender@example.com>");
        assert_eq!(sent[2], "RCPT TO:<alice@example.com>");
        assert_eq!(sent[3], "RCPT TO:<bob@example.com>");
        assert_eq!(sent[4], "DATA");
        // After DATA, the raw message body + ".\r\n" are sent as bytes
    }

    #[test]
    fn session_send_message_dot_stuffing() {
        let tp = MockTransport::new(vec![
            "250 Ok\r\n",        // RSET
            "250 Ok\r\n",        // MAIL FROM
            "250 Ok\r\n",        // RCPT TO
            "354 Go ahead\r\n",  // DATA
            "250 Ok\r\n",        // .
        ]);
        let mut s = SmtpSession::new(tp, "test.local");
        // Message has a line starting with "."
        s.send_message(
            "sender@example.com",
            &["dest@example.com"],
            b"Subject: Test\r\n\r\n.hidden line\r\n",
        )
        .unwrap();

        // Verify dot-stuffing happened in the sent data
        let mut all_sent = Vec::new();
        for chunk in &s.tp.sent {
            all_sent.extend_from_slice(chunk);
        }
        let sent_str = String::from_utf8_lossy(&all_sent);
        // The ".hidden" should become "..hidden" in the sent data
        assert!(sent_str.contains("..hidden line"));
    }

    #[test]
    fn session_noop() {
        let tp = MockTransport::new(vec!["250 Ok\r\n"]);
        let mut s = SmtpSession::new(tp, "test.local");
        let reply = s.noop().unwrap();
        assert_eq!(reply.code, 250);
    }

    #[test]
    fn session_vrfy() {
        let tp = MockTransport::new(vec!["252 Cannot VRFY user\r\n"]);
        let mut s = SmtpSession::new(tp, "test.local");
        let reply = s.vrfy("user@example.com").unwrap();
        assert_eq!(reply.code, 252);

        let sent = s.tp.sent_lines();
        assert_eq!(sent[0], "VRFY user@example.com");
    }

    #[test]
    fn session_quit() {
        let tp = MockTransport::new(vec!["221 Bye\r\n"]);
        let mut s = SmtpSession::new(tp, "test.local");
        let reply = s.quit().unwrap();
        assert_eq!(reply.code, 221);
    }

    #[test]
    fn session_ehlo_fallback_to_helo() {
        let tp = MockTransport::new(vec![
            // EHLO rejected
            "502 Command not implemented\r\n",
            // HELO succeeds
            "250 Hello test.local\r\n",
        ]);
        let mut s = SmtpSession::new(tp, "test.local");
        let reply = s.ehlo().unwrap();
        assert_eq!(reply.code, 250);

        let sent = s.tp.sent_lines();
        assert_eq!(sent[0], "EHLO test.local");
        assert_eq!(sent[1], "HELO test.local");
    }

    #[test]
    fn session_default_hostname() {
        let tp = MockTransport::new(vec![]);
        let s = SmtpSession::new(tp, "");
        assert_eq!(s.local_hostname, "localhost");
    }

    #[test]
    fn error_display() {
        let e = Error::Reply { code: 550, message: "Mailbox not found".into() };
        assert_eq!(format!("{}", e), "SMTP 550 Mailbox not found");

        let e = Error::NotConnected;
        assert_eq!(format!("{}", e), "SMTP not connected");

        let e = Error::AuthFailed { code: 535, message: "Bad credentials".into() };
        assert_eq!(format!("{}", e), "SMTP auth failed: 535 Bad credentials");
    }
}
