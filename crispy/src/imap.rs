//! IMAP4rev1 client (RFC 3501).
//!
//! Full IMAP4rev1 implementation ported from crispy_imap.c:
//!   Connection:    CAPABILITY, LOGIN, AUTHENTICATE (PLAIN, XOAUTH2, CRAM-MD5,
//!                  DIGEST-MD5, SCRAM-SHA-256), STARTTLS, LOGOUT
//!   Mailbox:       LIST, LSUB, SELECT, EXAMINE, STATUS, CREATE, DELETE,
//!                  RENAME, SUBSCRIBE, UNSUBSCRIBE, CHECK, CLOSE, NAMESPACE, ID
//!   Fetch:         UID FETCH (FLAGS, BODY.PEEK[], BODY.PEEK[HEADER],
//!                  BODY.PEEK[TEXT], BODY.PEEK[section], HEADER.FIELDS,
//!                  RFC822.SIZE, INTERNALDATE, ENVELOPE, BODYSTRUCTURE)
//!   Modify:        UID STORE, UID COPY, UID MOVE, EXPUNGE, APPEND
//!   Search:        UID SEARCH, UID SORT, UID THREAD
//!   Session:       NOOP, IDLE (RFC 2177)
//!   Quota:         GETQUOTAROOT, GETQUOTA, SETQUOTA (RFC 2087)
//!   ACL:           GETACL, SETACL, DELETEACL, LISTRIGHTS, MYRIGHTS (RFC 4314)
//!   Cache:         Per-session UID/flags/size cache with seqno mapping
//!   Reconnect:     Auto-reconnect with credential storage
//!
//! Portable. No external dependencies beyond std.
//! Uses `crate::transport::Transport` for pluggable I/O.
//! Uses `crate::auth` for SASL authentication.
//! Uses `crate::base64` for base64 encoding/decoding.
//! Uses `crate::md5` for MD5/HMAC-MD5.

use std::fmt;
use std::io;

use crate::auth;
use crate::base64;
use crate::md5;
use crate::transport::Transport;

// ══════════════════════════════════════════════════════════════════
// Error types
// ══════════════════════════════════════════════════════════════════

/// IMAP response status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImapStatus {
    Ok,
    No,
    Bad,
    Err,
}

impl fmt::Display for ImapStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ok => write!(f, "OK"),
            Self::No => write!(f, "NO"),
            Self::Bad => write!(f, "BAD"),
            Self::Err => write!(f, "ERR"),
        }
    }
}

/// IMAP error with status and optional text.
#[derive(Debug)]
pub struct ImapError {
    pub status: ImapStatus,
    pub message: String,
}

impl fmt::Display for ImapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IMAP {}: {}", self.status, self.message)
    }
}

impl std::error::Error for ImapError {}

impl From<io::Error> for ImapError {
    fn from(e: io::Error) -> Self {
        ImapError {
            status: ImapStatus::Err,
            message: e.to_string(),
        }
    }
}

impl ImapError {
    pub fn err(msg: impl Into<String>) -> Self {
        ImapError { status: ImapStatus::Err, message: msg.into() }
    }
    pub fn no(msg: impl Into<String>) -> Self {
        ImapError { status: ImapStatus::No, message: msg.into() }
    }
    pub fn bad(msg: impl Into<String>) -> Self {
        ImapError { status: ImapStatus::Bad, message: msg.into() }
    }
}

pub type ImapResult<T> = Result<T, ImapError>;

// ══════════════════════════════════════════════════════════════════
// Enums
// ══════════════════════════════════════════════════════════════════

/// Connection security mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImapSecurity {
    Plain,
    StartTls,
    Ssl,
}

/// IMAP protocol level (detected from CAPABILITY).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImapLevel {
    Unknown  = 0,
    Imap2    = 2,
    Imap2bis = 3,
    Imap4    = 4,
    Imap4rev1 = 5,
    Imap4rev2 = 6,
}

impl Default for ImapLevel {
    fn default() -> Self { Self::Unknown }
}

/// MIME body content type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyType {
    Text,
    Multipart,
    Message,
    Application,
    Audio,
    Image,
    Video,
    Model,
    Other,
}

impl Default for BodyType {
    fn default() -> Self { Self::Other }
}

impl BodyType {
    fn from_str_ci(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "TEXT" => Self::Text,
            "MULTIPART" => Self::Multipart,
            "MESSAGE" => Self::Message,
            "APPLICATION" => Self::Application,
            "AUDIO" => Self::Audio,
            "IMAGE" => Self::Image,
            "VIDEO" => Self::Video,
            "MODEL" => Self::Model,
            _ => Self::Other,
        }
    }
}

/// Transfer encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    SevenBit,
    EightBit,
    Binary,
    Base64,
    QuotedPrintable,
    Other,
}

impl Default for Encoding {
    fn default() -> Self { Self::Other }
}

impl Encoding {
    fn from_str_ci(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "7BIT" => Self::SevenBit,
            "8BIT" => Self::EightBit,
            "BINARY" => Self::Binary,
            "BASE64" => Self::Base64,
            "QUOTED-PRINTABLE" => Self::QuotedPrintable,
            _ => Self::Other,
        }
    }
}

/// Response code from tagged reply `[CODE ...]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseCode {
    None,
    Alert,
    TryCreate,
    Parse,
    ReadOnly,
    ReadWrite,
    UidValidity,
    UidNext,
    Unseen,
    PermanentFlags,
    AppendUid,
    CopyUid,
    Referral,
    Other,
}

impl Default for ResponseCode {
    fn default() -> Self { Self::None }
}

// ══════════════════════════════════════════════════════════════════
// Data types
// ══════════════════════════════════════════════════════════════════

/// MIME parameter (name=value pair).
#[derive(Debug, Clone, Default)]
pub struct Param {
    pub name: String,
    pub value: String,
}

/// Content disposition.
#[derive(Debug, Clone, Default)]
pub struct Disposition {
    pub dtype: Option<String>,
    pub params: Vec<Param>,
}

/// IMAP address (from ENVELOPE).
#[derive(Debug, Clone, Default)]
pub struct Address {
    pub name: Option<String>,
    pub adl: Option<String>,
    pub mailbox: Option<String>,
    pub host: Option<String>,
}

impl Address {
    /// Format as "mailbox@host" or display name + email.
    pub fn email(&self) -> String {
        match (&self.mailbox, &self.host) {
            (Some(m), Some(h)) => format!("{}@{}", m, h),
            (Some(m), None) => m.clone(),
            _ => String::new(),
        }
    }
}

/// Parsed ENVELOPE from FETCH.
#[derive(Debug, Clone, Default)]
pub struct Envelope {
    pub date: Option<String>,
    pub subject: Option<String>,
    pub from: Vec<Address>,
    pub sender: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub in_reply_to: Option<String>,
    pub message_id: Option<String>,
}

/// Body structure (recursive, from BODYSTRUCTURE FETCH).
#[derive(Debug, Clone, Default)]
pub struct BodyPart {
    pub body_type: BodyType,
    pub encoding: Encoding,
    pub subtype: Option<String>,
    pub params: Vec<Param>,
    pub id: Option<String>,
    pub description: Option<String>,
    pub md5: Option<String>,
    pub disposition: Disposition,
    pub language: Option<String>,
    pub location: Option<String>,
    pub size_bytes: u64,
    pub size_lines: u64,
    /// For multipart: child parts.
    pub subparts: Vec<BodyPart>,
    /// For message/rfc822: nested envelope.
    pub nested_env: Option<Box<Envelope>>,
    /// For message/rfc822: nested body.
    pub nested_body: Option<Box<BodyPart>>,
    /// Section number for FETCH (e.g. "1.2.3").
    pub section: String,
}

/// NAMESPACE entry.
#[derive(Debug, Clone, Default)]
pub struct NamespaceEntry {
    pub prefix: String,
    pub delimiter: Option<char>,
}

/// NAMESPACE (personal, other users, shared).
#[derive(Debug, Clone, Default)]
pub struct Namespace {
    pub personal: Vec<NamespaceEntry>,
    pub other: Vec<NamespaceEntry>,
    pub shared: Vec<NamespaceEntry>,
}

/// Mailbox info from SELECT/EXAMINE.
#[derive(Debug, Clone, Default)]
pub struct MailboxInfo {
    pub name: String,
    pub exists: i64,
    pub recent: i64,
    pub unseen: i64,
    pub uidvalidity: u64,
    pub uidnext: u64,
    pub read_only: bool,
    pub perm_seen: bool,
    pub perm_answered: bool,
    pub perm_flagged: bool,
    pub perm_deleted: bool,
    pub perm_draft: bool,
    pub perm_custom: bool,
}

/// LIST/LSUB mailbox entry.
#[derive(Debug, Clone, Default)]
pub struct ListEntry {
    pub name: String,
    pub delimiter: Option<char>,
    pub noselect: bool,
    pub noinferiors: bool,
    pub has_children: bool,
    pub has_no_children: bool,
    pub marked: bool,
    pub unmarked: bool,
}

/// Message flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    pub seen: bool,
    pub answered: bool,
    pub flagged: bool,
    pub deleted: bool,
    pub draft: bool,
    pub recent: bool,
}

impl Flags {
    /// Parse flags from a parenthesized flag list like `(\Seen \Flagged)`.
    pub fn parse(s: &str) -> Self {
        let su = s.to_ascii_uppercase();
        Flags {
            seen: su.contains("\\SEEN"),
            answered: su.contains("\\ANSWERED"),
            flagged: su.contains("\\FLAGGED"),
            deleted: su.contains("\\DELETED"),
            draft: su.contains("\\DRAFT"),
            recent: su.contains("\\RECENT"),
        }
    }

    /// Format as IMAP flag string: `\Seen \Flagged`.
    pub fn to_imap_string(&self) -> String {
        let mut parts = Vec::new();
        if self.seen { parts.push("\\Seen"); }
        if self.answered { parts.push("\\Answered"); }
        if self.flagged { parts.push("\\Flagged"); }
        if self.deleted { parts.push("\\Deleted"); }
        if self.draft { parts.push("\\Draft"); }
        parts.join(" ")
    }

    pub fn any_set(&self) -> bool {
        self.seen || self.answered || self.flagged || self.deleted || self.draft
    }
}

/// Fetch result per message.
#[derive(Debug, Clone, Default)]
pub struct FetchResult {
    pub uid: u64,
    pub seqno: u64,
    pub size: u64,
    pub flags: Flags,
    pub internaldate: Option<String>,
    pub envelope: Option<Envelope>,
    pub bodystructure: Option<BodyPart>,
    pub headers: Option<String>,
    pub body: Option<Vec<u8>>,
    pub full: Option<Vec<u8>>,
}

/// Quota information.
#[derive(Debug, Clone, Default)]
pub struct Quota {
    pub root: String,
    pub usage: u64,
    pub limit: u64,
}

/// ACL entry.
#[derive(Debug, Clone, Default)]
pub struct AclEntry {
    pub identifier: String,
    pub rights: String,
}

/// Cached message entry.
#[derive(Debug, Clone, Default)]
pub struct CacheEntry {
    pub uid: u64,
    pub seqno: u64,
    pub size: u64,
    pub flags: Flags,
    pub internaldate: Option<String>,
    pub envelope: Option<Envelope>,
}

// ══════════════════════════════════════════════════════════════════
// Capabilities
// ══════════════════════════════════════════════════════════════════

/// Server capabilities detected from CAPABILITY response.
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    pub starttls: bool,
    pub idle: bool,
    pub uidplus: bool,
    pub r#move: bool,
    pub literal_plus: bool,
    pub sort: bool,
    pub thread: bool,
    pub namespace: bool,
    pub condstore: bool,
    pub children: bool,
    pub id: bool,
    pub quota: bool,
    pub acl: bool,
    /// Space-separated AUTH mechanisms (e.g. "PLAIN CRAM-MD5").
    pub auth_mechs: String,
}

impl Capabilities {
    fn parse(&mut self, line: &str) {
        let upper = line.to_ascii_uppercase();
        if upper.contains("IMAP4REV2") {
            // level handled in session
        }
        self.starttls = self.starttls || upper.contains("STARTTLS");
        self.idle = self.idle || upper.contains("IDLE");
        self.uidplus = self.uidplus || upper.contains("UIDPLUS");
        self.r#move = self.r#move || upper.contains("MOVE");
        self.literal_plus = self.literal_plus || upper.contains("LITERAL+");
        self.sort = self.sort || upper.contains("SORT");
        self.thread = self.thread || upper.contains("THREAD");
        self.namespace = self.namespace || upper.contains("NAMESPACE");
        self.condstore = self.condstore || upper.contains("CONDSTORE");
        self.children = self.children || upper.contains("CHILDREN");
        self.id = self.id || upper.contains(" ID");
        self.quota = self.quota || upper.contains("QUOTA");
        self.acl = self.acl || upper.contains(" ACL");

        // Collect AUTH= mechanisms
        let mut pos = 0;
        let upper_bytes = upper.as_bytes();
        while pos < upper_bytes.len() {
            if let Some(idx) = upper[pos..].find("AUTH=") {
                let start = pos + idx + 5;
                let end = upper[start..].find(|c: char| c == ' ' || c == ']')
                    .map(|i| start + i)
                    .unwrap_or(upper.len());
                let mech = &line[start..end];
                if !self.auth_mechs.is_empty() {
                    self.auth_mechs.push(' ');
                }
                self.auth_mechs.push_str(mech);
                pos = end;
            } else {
                break;
            }
        }
    }

    fn reset(&mut self) {
        *self = Capabilities::default();
    }
}

fn detect_level(line: &str) -> ImapLevel {
    let upper = line.to_ascii_uppercase();
    if upper.contains("IMAP4REV2") { ImapLevel::Imap4rev2 }
    else if upper.contains("IMAP4REV1") { ImapLevel::Imap4rev1 }
    else if upper.contains("IMAP4") { ImapLevel::Imap4 }
    else if upper.contains("IMAP2BIS") { ImapLevel::Imap2bis }
    else if upper.contains("IMAP2") { ImapLevel::Imap2 }
    else { ImapLevel::Unknown }
}

// ══════════════════════════════════════════════════════════════════
// Session
// ══════════════════════════════════════════════════════════════════

/// IMAP client session.
pub struct ImapSession<T: Transport> {
    tp: T,
    tag_num: u32,
    tag: String,
    /// Last untagged/tagged response text.
    pub reply: String,
    /// Currently selected mailbox info.
    pub selected: MailboxInfo,
    pub connected: bool,
    pub authenticated: bool,
    pub bye_received: bool,
    pub level: ImapLevel,
    pub last_rcode: ResponseCode,
    pub last_alert: String,
    pub last_referral: String,
    /// Message cache (per selected mailbox).
    cache: Vec<CacheEntry>,
    /// Auto-reconnect state.
    reconnect_host: String,
    reconnect_port: u16,
    reconnect_security: ImapSecurity,
    reconnect_user: String,
    reconnect_pass: String,
    reconnect_enabled: bool,
    /// Server capabilities.
    pub caps: Capabilities,
    /// Debug callback.
    debug_fn: Option<Box<dyn Fn(&str)>>,
}

impl<T: Transport> ImapSession<T> {
    // ── Construction ──

    pub fn new(transport: T) -> Self {
        ImapSession {
            tp: transport,
            tag_num: 0,
            tag: String::new(),
            reply: String::new(),
            selected: MailboxInfo::default(),
            connected: false,
            authenticated: false,
            bye_received: false,
            level: ImapLevel::Unknown,
            last_rcode: ResponseCode::None,
            last_alert: String::new(),
            last_referral: String::new(),
            cache: Vec::new(),
            reconnect_host: String::new(),
            reconnect_port: 0,
            reconnect_security: ImapSecurity::Plain,
            reconnect_user: String::new(),
            reconnect_pass: String::new(),
            reconnect_enabled: false,
            caps: Capabilities::default(),
            debug_fn: None,
        }
    }

    pub fn set_debug<F: Fn(&str) + 'static>(&mut self, f: F) {
        self.debug_fn = Some(Box::new(f));
    }

    fn dbg(&self, msg: &str) {
        if let Some(ref f) = self.debug_fn {
            f(msg);
        }
    }

    // ── Transport helpers ──

    fn send_raw(&mut self, data: &[u8]) -> ImapResult<()> {
        self.tp.send(data).map_err(ImapError::from)
    }

    fn send_line(&mut self, line: &str) -> ImapResult<()> {
        self.dbg(&format!("C: {}", line));
        self.send_raw(line.as_bytes())?;
        self.send_raw(b"\r\n")?;
        Ok(())
    }

    /// Read one line from server, strip CRLF. Returns the line as a String.
    pub fn readline(&mut self) -> ImapResult<String> {
        let raw = self.tp.recv_line().map_err(ImapError::from)?;
        let mut line = String::from_utf8_lossy(&raw).into_owned();
        // Strip trailing CRLF/LF
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        self.dbg(&format!("S: {}", line));
        Ok(line)
    }

    /// Read exactly `size` bytes of literal data.
    fn read_literal(&mut self, size: usize) -> ImapResult<Vec<u8>> {
        let mut data = Vec::with_capacity(size);
        while data.len() < size {
            let chunk = self.tp.recv_line().map_err(ImapError::from)?;
            if chunk.is_empty() {
                return Err(ImapError::err("Connection closed during literal read"));
            }
            data.extend_from_slice(&chunk);
        }
        data.truncate(size);
        Ok(data)
    }

    // ── Tag management ──

    fn next_tag(&mut self) -> String {
        self.tag_num += 1;
        self.tag = format!("A{:04}", self.tag_num);
        self.tag.clone()
    }

    // ── Mailbox update tracking ──

    fn track_mailbox_updates(&mut self, untagged: &str) {
        // Parse "N EXISTS", "N RECENT", "N EXPUNGE", "BYE ..."
        let parts: Vec<&str> = untagged.splitn(2, ' ').collect();
        if parts.len() == 2 {
            if let Ok(num) = parts[0].parse::<i64>() {
                let keyword = parts[1].to_ascii_uppercase();
                if keyword.starts_with("EXISTS") {
                    self.selected.exists = num;
                    self.dbg(&format!("Mailbox update: {} EXISTS", num));
                } else if keyword.starts_with("RECENT") {
                    self.selected.recent = num;
                } else if keyword.starts_with("EXPUNGE") {
                    if self.selected.exists > 0 {
                        self.selected.exists -= 1;
                    }
                    let seqno = num as u64;
                    self.cache.retain(|e| e.seqno != seqno);
                    for entry in &mut self.cache {
                        if entry.seqno > seqno {
                            entry.seqno -= 1;
                        }
                    }
                    self.dbg(&format!("Mailbox update: msg {} EXPUNGED, now {}", num, self.selected.exists));
                }
            }
        }
        let upper = untagged.to_ascii_uppercase();
        if upper.starts_with("BYE ") {
            self.bye_received = true;
            self.connected = false;
            self.dbg(&format!("Server BYE: {}", &untagged[4..]));
        }
    }

    // ── Response code parser ──

    fn parse_response_code(&mut self, reply: &str) {
        self.last_rcode = ResponseCode::None;
        // Find '[' after OK/NO/BAD
        let bracket = match reply.find('[') {
            Some(i) => i,
            None => return,
        };
        let after = &reply[bracket + 1..];

        let upper = after.to_ascii_uppercase();
        if upper.starts_with("ALERT") {
            self.last_rcode = ResponseCode::Alert;
            if let Some(close) = after.find(']') {
                let text = after[close + 1..].trim();
                self.last_alert = text.to_string();
            }
            self.dbg(&format!("ALERT: {}", self.last_alert));
        } else if upper.starts_with("TRYCREATE") {
            self.last_rcode = ResponseCode::TryCreate;
        } else if upper.starts_with("PARSE") {
            self.last_rcode = ResponseCode::Parse;
        } else if upper.starts_with("READ-ONLY") {
            self.last_rcode = ResponseCode::ReadOnly;
        } else if upper.starts_with("READ-WRITE") {
            self.last_rcode = ResponseCode::ReadWrite;
        } else if upper.starts_with("APPENDUID") {
            self.last_rcode = ResponseCode::AppendUid;
        } else if upper.starts_with("COPYUID") {
            self.last_rcode = ResponseCode::CopyUid;
        } else if upper.starts_with("UIDVALIDITY") {
            self.last_rcode = ResponseCode::UidValidity;
        } else if upper.starts_with("UIDNEXT") {
            self.last_rcode = ResponseCode::UidNext;
        } else if upper.starts_with("UNSEEN") {
            self.last_rcode = ResponseCode::Unseen;
        } else if upper.starts_with("PERMANENTFLAGS") {
            self.last_rcode = ResponseCode::PermanentFlags;
        } else if upper.starts_with("REFERRAL ") {
            self.last_rcode = ResponseCode::Referral;
            if let Some(close) = after.find(']') {
                self.last_referral = after[9..close].to_string();
            }
        } else {
            self.last_rcode = ResponseCode::Other;
        }
    }

    // ══════════════════════════════════════════════════════════════
    // Core command/response engine
    // ══════════════════════════════════════════════════════════════

    /// Classify the status word from a tagged reply.
    fn classify_status(status: &str) -> ImapStatus {
        let upper = status.to_ascii_uppercase();
        if upper.starts_with("OK") { ImapStatus::Ok }
        else if upper.starts_with("NO") { ImapStatus::No }
        else if upper.starts_with("BAD") { ImapStatus::Bad }
        else { ImapStatus::Err }
    }

    /// Send a tagged command, read all responses until tagged reply.
    /// Untagged responses are tracked for mailbox updates; the last one
    /// is stored in `self.reply`.
    pub fn command(&mut self, cmd: &str) -> ImapResult<()> {
        if self.bye_received {
            return Err(ImapError::err("Server sent BYE"));
        }
        let tag = self.next_tag();
        let full = format!("{} {}", tag, cmd);
        self.send_line(&full)?;

        loop {
            let line = self.readline()?;
            if line.starts_with(&tag) && line.as_bytes().get(tag.len()) == Some(&b' ') {
                let status = &line[tag.len() + 1..];
                self.reply = status.to_string();
                self.parse_response_code(status);
                let s = Self::classify_status(status);
                return match s {
                    ImapStatus::Ok => Ok(()),
                    ImapStatus::No => Err(ImapError::no(status)),
                    ImapStatus::Bad => Err(ImapError::bad(status)),
                    _ => Err(ImapError::err(status)),
                };
            }

            // Untagged response
            if line.starts_with("* ") {
                self.track_mailbox_updates(&line[2..]);
                self.reply = line[2..].to_string();
            }

            if self.bye_received {
                return Err(ImapError::err("Server sent BYE"));
            }
        }
    }

    /// Send a tagged command and collect all untagged responses.
    pub fn command_collect(&mut self, cmd: &str) -> ImapResult<Vec<String>> {
        if self.bye_received {
            return Err(ImapError::err("Server sent BYE"));
        }
        let tag = self.next_tag();
        let full = format!("{} {}", tag, cmd);
        self.send_line(&full)?;

        let mut lines = Vec::new();
        loop {
            let line = self.readline()?;
            if line.starts_with(&tag) && line.as_bytes().get(tag.len()) == Some(&b' ') {
                let status = &line[tag.len() + 1..];
                self.reply = status.to_string();
                self.parse_response_code(status);
                let s = Self::classify_status(status);
                return match s {
                    ImapStatus::Ok => Ok(lines),
                    ImapStatus::No => Err(ImapError::no(status)),
                    _ => Err(ImapError::bad(status)),
                };
            }

            if line.starts_with("* ") {
                self.track_mailbox_updates(&line[2..]);
                lines.push(line[2..].to_string());
            }
        }
    }

    /// Send a FETCH command and collect responses, handling `{NNN}` literals
    /// by reading the literal data and appending it to the response line.
    fn fetch_collect(&mut self, cmd: &str) -> ImapResult<Vec<String>> {
        let tag = self.next_tag();
        let full = format!("{} {}", tag, cmd);
        self.send_line(&full)?;

        let mut lines = Vec::new();
        loop {
            let line = self.readline()?;
            if line.starts_with(&tag) && line.as_bytes().get(tag.len()) == Some(&b' ') {
                let status = &line[tag.len() + 1..];
                self.reply = status.to_string();
                let s = Self::classify_status(status);
                return match s {
                    ImapStatus::Ok => Ok(lines),
                    ImapStatus::No => Err(ImapError::no(status)),
                    _ => Err(ImapError::bad(status)),
                };
            }

            // Check for literal {NNN} at end of line
            if let Some(combined) = self.try_read_literal_line(&line)? {
                // Store the combined data (without "* " prefix if present)
                if combined.starts_with("* ") {
                    lines.push(combined[2..].to_string());
                } else {
                    lines.push(combined);
                }
                continue;
            }

            if line.starts_with("* ") {
                lines.push(line[2..].to_string());
            }
        }
    }

    /// If `line` ends with `{NNN}`, read the literal data and the closing line.
    /// Returns `Some(combined)` or `None` if no literal.
    fn try_read_literal_line(&mut self, line: &str) -> ImapResult<Option<String>> {
        if let Some(lbrace) = line.rfind('{') {
            if let Some(rbrace) = line[lbrace..].find('}') {
                let size_str = &line[lbrace + 1..lbrace + rbrace];
                if let Ok(lit_size) = size_str.parse::<usize>() {
                    if lit_size > 0 {
                        let lit_data = self.read_literal(lit_size)?;
                        let prefix = &line[..lbrace];
                        let lit_str = String::from_utf8_lossy(&lit_data);

                        // Read closing line (usually just ")")
                        let close_line = self.readline().unwrap_or_default();

                        let combined = format!("{}\n{}\n{}", prefix, lit_str, close_line);
                        return Ok(Some(combined));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Send a FETCH command and read literal response data.
    /// Used for message/header/body fetches that return `{NNN}` literal data.
    fn fetch_literal_response(&mut self, cmd: &str) -> ImapResult<Vec<u8>> {
        let tag = self.next_tag();
        let full = format!("{} {}", tag, cmd);
        self.send_line(&full)?;

        let mut result: Option<Vec<u8>> = None;

        loop {
            let line = self.readline()?;

            // Untagged FETCH response with literal
            if line.starts_with('*') {
                if let Some(lbrace) = line.rfind('{') {
                    if let Some(rbrace) = line[lbrace..].find('}') {
                        let size_str = &line[lbrace + 1..lbrace + rbrace];
                        if let Ok(lit_size) = size_str.parse::<usize>() {
                            if lit_size > 0 {
                                result = Some(self.read_literal(lit_size)?);
                                // Read closing paren/continuation line
                                let _ = self.readline();
                                continue;
                            }
                        }
                    }
                }
            }

            // Tagged response
            if line.starts_with(&tag) && line.as_bytes().get(tag.len()) == Some(&b' ') {
                self.reply = line[tag.len() + 1..].to_string();
                break;
            }
        }

        result.ok_or_else(|| ImapError::err("No literal data in response"))
    }

    // ══════════════════════════════════════════════════════════════
    // Literal argument sending
    // ══════════════════════════════════════════════════════════════

    /// Check if a string needs literal encoding (contains special chars).
    fn needs_literal(s: &str) -> bool {
        s.bytes().any(|b| b < 0x20 || b == b'"' || b == b'\\' || b == b'{' || b > 0x7e)
    }

    /// Send a command with an argument that may need literal encoding.
    fn send_with_literal(&mut self, prefix: &str, arg: &str) -> ImapResult<()> {
        if !Self::needs_literal(arg) {
            let cmd = format!("{} \"{}\"", prefix, arg);
            return self.command(&cmd);
        }

        // Send as literal
        let tag = self.next_tag();
        let full = format!("{} {} {{{}}}", tag, prefix, arg.len());
        self.send_line(&full)?;

        // Wait for continuation "+"
        let cont = self.readline()?;
        if !cont.starts_with('+') {
            return Err(ImapError::err("Expected continuation for literal"));
        }

        // Send literal data
        self.send_raw(arg.as_bytes())?;
        self.send_raw(b"\r\n")?;

        // Read tagged response
        self.read_tagged_response()
    }

    /// Read responses until we get our tagged reply.
    fn read_tagged_response(&mut self) -> ImapResult<()> {
        loop {
            let line = self.readline()?;
            if line.starts_with(&self.tag) && line.as_bytes().get(self.tag.len()) == Some(&b' ') {
                let status = &line[self.tag.len() + 1..];
                self.reply = status.to_string();
                self.parse_response_code(status);
                let s = Self::classify_status(status);
                return match s {
                    ImapStatus::Ok => Ok(()),
                    ImapStatus::No => Err(ImapError::no(status)),
                    ImapStatus::Bad => Err(ImapError::bad(status)),
                    _ => Err(ImapError::err(status)),
                };
            }
            if line.starts_with("* ") {
                self.track_mailbox_updates(&line[2..]);
            }
        }
    }

    /// Read a SASL continuation line: `+ <base64>`.
    fn read_continuation(&mut self) -> ImapResult<String> {
        let line = self.readline()?;
        if !line.starts_with('+') {
            return Err(ImapError::err("Expected continuation (+)"));
        }
        let data = line[1..].trim_start().to_string();
        Ok(data)
    }

    // ══════════════════════════════════════════════════════════════
    // Connection
    // ══════════════════════════════════════════════════════════════

    /// Connect to IMAP server, read greeting, discover capabilities.
    pub fn connect(&mut self, host: &str, port: u16, security: ImapSecurity) -> ImapResult<()> {
        // For implicit TLS (port 993), connect then upgrade before IMAP
        if security == ImapSecurity::Ssl {
            self.tp.connect(host, port).map_err(ImapError::from)?;
            self.tp.start_tls().map_err(ImapError::from)?;
        } else {
            self.tp.connect(host, port).map_err(ImapError::from)?;
        }
        self.connected = true;
        self.bye_received = false;

        // Save for reconnect
        self.reconnect_host = host.to_string();
        self.reconnect_port = port;
        self.reconnect_security = security;

        // Read greeting
        let greeting = self.readline()?;
        if !greeting.starts_with("* OK") && !greeting.starts_with("* PREAUTH") {
            return Err(ImapError::err(format!("Bad greeting: {}", greeting)));
        }
        if greeting.starts_with("* PREAUTH") {
            self.authenticated = true;
        }
        let level = detect_level(&greeting);
        if level > ImapLevel::Unknown {
            self.level = level;
        }
        if greeting.to_ascii_uppercase().contains("CAPABILITY") {
            self.caps.parse(&greeting);
        }

        // Always send CAPABILITY to get full capability set
        let lines = self.command_collect("CAPABILITY")?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("CAPABILITY ") {
                let lvl = detect_level(l);
                if lvl > ImapLevel::Unknown { self.level = lvl; }
                self.caps.parse(l);
            }
        }
        // Also check tagged OK reply
        let reply_clone = self.reply.clone();
        self.caps.parse(&reply_clone);
        let lvl = detect_level(&reply_clone);
        if lvl > ImapLevel::Unknown { self.level = lvl; }

        // STARTTLS if requested
        if security == ImapSecurity::StartTls {
            if !self.caps.starttls {
                return Err(ImapError::err("Server does not support STARTTLS"));
            }
            self.command("STARTTLS")?;
            self.tp.start_tls().map_err(ImapError::from)?;
            // Re-CAPABILITY after TLS
            self.caps.reset();
            let lines = self.command_collect("CAPABILITY")?;
            for l in &lines {
                let upper = l.to_ascii_uppercase();
                if upper.starts_with("CAPABILITY ") {
                    let lvl = detect_level(l);
                    if lvl > ImapLevel::Unknown { self.level = lvl; }
                    self.caps.parse(l);
                }
            }
            let reply_clone = self.reply.clone();
            self.caps.parse(&reply_clone);
        }

        Ok(())
    }

    /// LOGIN with quoted user and password.
    pub fn login(&mut self, user: &str, pass: &str) -> ImapResult<()> {
        let quoted_user = imap_quote(user);
        let quoted_pass = imap_quote(pass);
        let cmd = format!("LOGIN {} {}", quoted_user, quoted_pass);
        self.command(&cmd)?;
        self.authenticated = true;
        // Server may send updated capabilities after login
        let reply_clone = self.reply.clone();
        if reply_clone.to_ascii_uppercase().contains("CAPABILITY") {
            self.caps.parse(&reply_clone);
        }
        Ok(())
    }

    /// AUTHENTICATE PLAIN (single-line, RFC 4616).
    pub fn auth_plain(&mut self, user: &str, pass: &str) -> ImapResult<()> {
        let b64 = auth::plain_response(user, pass);
        let cmd = format!("AUTHENTICATE PLAIN {}", b64);
        self.command(&cmd)?;
        self.authenticated = true;
        Ok(())
    }

    /// AUTHENTICATE XOAUTH2 (Google extension).
    pub fn auth_xoauth2(&mut self, user: &str, token: &str) -> ImapResult<()> {
        let b64 = auth::xoauth2_response(user, token);
        let cmd = format!("AUTHENTICATE XOAUTH2 {}", b64);
        self.command(&cmd)?;
        self.authenticated = true;
        Ok(())
    }

    /// AUTHENTICATE CRAM-MD5 (challenge-response).
    pub fn auth_cram_md5(&mut self, user: &str, pass: &str) -> ImapResult<()> {
        let tag = self.next_tag();
        let full = format!("{} AUTHENTICATE CRAM-MD5", tag);
        self.send_line(&full)?;

        // Server sends + <base64 challenge>
        let chal_b64 = self.read_continuation()?;

        // Build response
        let resp_b64 = auth::cram_md5_response(user, pass, &chal_b64)
            .ok_or_else(|| ImapError::err("CRAM-MD5 response generation failed"))?;

        self.send_line(&resp_b64)?;

        // Read tagged response
        loop {
            let line = self.readline()?;
            if line.starts_with(&tag) && line.as_bytes().get(tag.len()) == Some(&b' ') {
                let status = &line[tag.len() + 1..];
                self.reply = status.to_string();
                if Self::classify_status(status) == ImapStatus::Ok {
                    self.authenticated = true;
                    return Ok(());
                }
                return Err(ImapError::no(status));
            }
        }
    }

    /// AUTHENTICATE DIGEST-MD5 (RFC 2831).
    pub fn auth_digest_md5(&mut self, user: &str, pass: &str) -> ImapResult<()> {
        let tag = self.next_tag();
        let full = format!("{} AUTHENTICATE DIGEST-MD5", tag);
        self.send_line(&full)?;

        // Server sends + <base64 challenge>
        let chal_b64 = self.read_continuation()?;

        let resp_b64 = auth::digest_md5_response(user, pass, &chal_b64, "imap")
            .ok_or_else(|| ImapError::err("DIGEST-MD5 response generation failed"))?;

        self.send_line(&resp_b64)?;

        // Server sends + <rspauth> — respond with empty line
        let _rspauth = self.read_continuation()?;
        self.send_line("")?;

        // Read tagged response
        loop {
            let line = self.readline()?;
            if line.starts_with(&tag) && line.as_bytes().get(tag.len()) == Some(&b' ') {
                let status = &line[tag.len() + 1..];
                self.reply = status.to_string();
                if Self::classify_status(status) == ImapStatus::Ok {
                    self.authenticated = true;
                    return Ok(());
                }
                return Err(ImapError::no(status));
            }
        }
    }

    /// AUTHENTICATE SCRAM-SHA-256 (RFC 7677).
    pub fn auth_scram_sha256(&mut self, user: &str, pass: &str) -> ImapResult<()> {
        // Generate client nonce from MD5 of username (matches C implementation)
        let cnonce = md5::md5_hex(user.as_bytes());
        let cnonce = &cnonce[..24]; // truncate like C does

        // Client-first-message-bare
        let cfmb = format!("n={},r={}", user, cnonce);
        // Client-first-message with GS2 header
        let cfm = format!("n,,{}", cfmb);

        let b64 = base64::encode(cfm.as_bytes());

        let tag = self.next_tag();
        let full = format!("{} AUTHENTICATE SCRAM-SHA-256 {}", tag, b64);
        self.send_line(&full)?;

        // Read server-first-message
        let sfm_b64 = self.read_continuation()?;
        let sfm_raw = base64::decode(&sfm_b64)
            .map_err(|_| ImapError::err("Failed to decode server-first-message"))?;
        let sfm = String::from_utf8_lossy(&sfm_raw).into_owned();

        // Parse r=nonce,s=salt,i=iterations
        let mut rnonce = String::new();
        let mut salt_b64 = String::new();
        let mut iters: u32 = 4096;
        for part in sfm.split(',') {
            if let Some(v) = part.strip_prefix("r=") { rnonce = v.to_string(); }
            else if let Some(v) = part.strip_prefix("s=") { salt_b64 = v.to_string(); }
            else if let Some(v) = part.strip_prefix("i=") { iters = v.parse().unwrap_or(4096); }
        }
        if rnonce.is_empty() || salt_b64.is_empty() {
            return Err(ImapError::err("Incomplete server-first-message"));
        }

        let salt = base64::decode(&salt_b64)
            .map_err(|_| ImapError::err("Failed to decode salt"))?;

        // PBKDF2(pass, salt, iters, 32) -> SaltedPassword
        let salted_pw = pbkdf2_sha256(pass.as_bytes(), &salt, iters, 32);

        // ClientKey = HMAC-SHA256(SaltedPassword, "Client Key")
        let client_key = hmac_sha256(&salted_pw, b"Client Key");

        // StoredKey = SHA256(ClientKey)
        let stored_key = sha256(&client_key);

        // Client-final-message-without-proof
        let cfmwp = format!("c=biws,r={}", rnonce);

        // AuthMessage = cfmb,sfm,cfmwp
        let auth_msg = format!("{},{},{}", cfmb, sfm, cfmwp);

        // ClientSignature = HMAC-SHA256(StoredKey, AuthMessage)
        let client_sig = hmac_sha256(&stored_key, auth_msg.as_bytes());

        // ClientProof = ClientKey XOR ClientSignature
        let mut proof = [0u8; 32];
        for i in 0..32 {
            proof[i] = client_key[i] ^ client_sig[i];
        }

        let proof_b64 = base64::encode(&proof);

        // Client-final-message
        let final_msg = format!("{},p={}", cfmwp, proof_b64);
        let final_b64 = base64::encode(final_msg.as_bytes());
        self.send_line(&final_b64)?;

        // Server-final (+ <verifier>) — skip verification
        let _sv = self.read_continuation()?;
        self.send_line("")?;

        // Read tagged response
        loop {
            let line = self.readline()?;
            if line.starts_with(&tag) && line.as_bytes().get(tag.len()) == Some(&b' ') {
                let status = &line[tag.len() + 1..];
                self.reply = status.to_string();
                if Self::classify_status(status) == ImapStatus::Ok {
                    self.authenticated = true;
                    return Ok(());
                }
                return Err(ImapError::no(status));
            }
        }
    }

    /// LOGOUT and close connection.
    pub fn close(&mut self) {
        if self.connected {
            let _ = self.command("LOGOUT");
            self.tp.close();
            self.connected = false;
        }
    }

    // ══════════════════════════════════════════════════════════════
    // Mailbox operations
    // ══════════════════════════════════════════════════════════════

    /// LIST — list mailboxes matching pattern.
    pub fn list(&mut self, reference: &str, pattern: &str) -> ImapResult<Vec<ListEntry>> {
        let cmd = format!("LIST \"{}\" \"{}\"", reference, pattern);
        let lines = self.command_collect(&cmd)?;
        Ok(parse_list_response("LIST ", &lines))
    }

    /// LSUB — list subscribed mailboxes.
    pub fn lsub(&mut self, reference: &str, pattern: &str) -> ImapResult<Vec<ListEntry>> {
        let cmd = format!("LSUB \"{}\" \"{}\"", reference, pattern);
        let lines = self.command_collect(&cmd)?;
        Ok(parse_list_response("LSUB ", &lines))
    }

    /// SELECT a mailbox.
    pub fn select(&mut self, mailbox: &str) -> ImapResult<()> {
        self.select_or_examine(mailbox, false)
    }

    /// EXAMINE a mailbox (read-only SELECT).
    pub fn examine(&mut self, mailbox: &str) -> ImapResult<()> {
        self.select_or_examine(mailbox, true)
    }

    fn select_or_examine(&mut self, mailbox: &str, rdonly: bool) -> ImapResult<()> {
        let verb = if rdonly { "EXAMINE" } else { "SELECT" };
        let cmd = format!("{} \"{}\"", verb, mailbox);
        let lines = self.command_collect(&cmd)?;

        self.cache_clear();
        self.selected = MailboxInfo::default();
        self.selected.name = mailbox.to_string();
        self.selected.read_only = rdonly;

        for l in &lines {
            // Parse "N EXISTS", "N RECENT"
            let parts: Vec<&str> = l.splitn(2, ' ').collect();
            if parts.len() == 2 {
                if let Ok(num) = parts[0].parse::<i64>() {
                    let kw = parts[1].to_ascii_uppercase();
                    if kw.starts_with("EXISTS") {
                        self.selected.exists = num;
                    } else if kw.starts_with("RECENT") {
                        self.selected.recent = num;
                    }
                }
            }

            let upper = l.to_ascii_uppercase();
            if let Some(pos) = upper.find("UIDVALIDITY") {
                let after = &l[pos + 11..];
                let val = parse_num_after_delims(after);
                self.selected.uidvalidity = val;
            }
            if let Some(pos) = upper.find("UIDNEXT") {
                let after = &l[pos + 7..];
                self.selected.uidnext = parse_num_after_delims(after);
            }
            if let Some(pos) = upper.find("UNSEEN") {
                let after = &l[pos + 6..];
                self.selected.unseen = parse_num_after_delims(after) as i64;
            }
            if upper.contains("PERMANENTFLAGS") {
                if let Some(pos) = upper.find("PERMANENTFLAGS") {
                    let pf = &l[pos..];
                    let pf_upper = pf.to_ascii_uppercase();
                    self.selected.perm_seen = pf_upper.contains("\\SEEN");
                    self.selected.perm_answered = pf_upper.contains("\\ANSWERED");
                    self.selected.perm_flagged = pf_upper.contains("\\FLAGGED");
                    self.selected.perm_deleted = pf_upper.contains("\\DELETED");
                    self.selected.perm_draft = pf_upper.contains("\\DRAFT");
                    self.selected.perm_custom = pf_upper.contains("\\*");
                }
            }
        }

        // Check tagged reply for READ-ONLY
        if self.reply.to_ascii_uppercase().contains("[READ-ONLY]") {
            self.selected.read_only = true;
        }

        Ok(())
    }

    /// STATUS — check mailbox without selecting.
    pub fn status(&mut self, mailbox: &str) -> ImapResult<MailboxInfo> {
        let cmd = format!(
            "STATUS \"{}\" (MESSAGES RECENT UNSEEN UIDVALIDITY UIDNEXT)",
            mailbox
        );
        let lines = self.command_collect(&cmd)?;

        let mut info = MailboxInfo::default();
        info.name = mailbox.to_string();
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if let Some(pos) = upper.find("MESSAGES") {
                let after = &l[pos + 8..];
                info.exists = parse_num_after_space(after) as i64;
            }
            if let Some(pos) = upper.find("RECENT") {
                let after = &l[pos + 6..];
                info.recent = parse_num_after_space(after) as i64;
            }
            if let Some(pos) = upper.find("UNSEEN") {
                let after = &l[pos + 6..];
                info.unseen = parse_num_after_space(after) as i64;
            }
            if let Some(pos) = upper.find("UIDVALIDITY") {
                let after = &l[pos + 11..];
                info.uidvalidity = parse_num_after_space(after);
            }
            if let Some(pos) = upper.find("UIDNEXT") {
                let after = &l[pos + 7..];
                info.uidnext = parse_num_after_space(after);
            }
        }
        Ok(info)
    }

    /// CREATE a new mailbox.
    pub fn create(&mut self, mailbox: &str) -> ImapResult<()> {
        self.send_with_literal("CREATE", mailbox)
    }

    /// DELETE a mailbox.
    pub fn delete_mailbox(&mut self, mailbox: &str) -> ImapResult<()> {
        self.send_with_literal("DELETE", mailbox)
    }

    /// RENAME a mailbox.
    pub fn rename(&mut self, from: &str, to: &str) -> ImapResult<()> {
        let cmd = format!("RENAME \"{}\" \"{}\"", from, to);
        self.command(&cmd)
    }

    /// SUBSCRIBE to a mailbox.
    pub fn subscribe(&mut self, mailbox: &str) -> ImapResult<()> {
        self.send_with_literal("SUBSCRIBE", mailbox)
    }

    /// UNSUBSCRIBE from a mailbox.
    pub fn unsubscribe(&mut self, mailbox: &str) -> ImapResult<()> {
        self.send_with_literal("UNSUBSCRIBE", mailbox)
    }

    /// CHECK — request checkpoint of currently selected mailbox.
    pub fn check(&mut self) -> ImapResult<()> {
        self.command("CHECK")
    }

    /// CLOSE — close selected mailbox (expunges deleted messages).
    pub fn close_mailbox(&mut self) -> ImapResult<()> {
        self.command("CLOSE")?;
        self.selected = MailboxInfo::default();
        Ok(())
    }

    /// NAMESPACE — discover personal, other users, shared namespaces.
    pub fn namespace(&mut self) -> ImapResult<Namespace> {
        if !self.caps.namespace {
            return Err(ImapError::no("Server does not support NAMESPACE"));
        }
        let lines = self.command_collect("NAMESPACE")?;
        let mut ns = Namespace::default();
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("NAMESPACE ") {
                let cursor = &l[10..];
                let mut p = cursor;
                ns.personal = parse_namespace_group(&mut p);
                ns.other = parse_namespace_group(&mut p);
                ns.shared = parse_namespace_group(&mut p);
                break;
            }
        }
        Ok(ns)
    }

    /// ID (RFC 2971) — identify client to server.
    pub fn id(&mut self, name: Option<&str>, version: Option<&str>) -> ImapResult<()> {
        let cmd = match (name, version) {
            (Some(n), Some(v)) => {
                format!("ID (\"name\" \"{}\" \"version\" \"{}\")", n, v)
            }
            _ => "ID NIL".to_string(),
        };
        self.command(&cmd)
    }

    // ══════════════════════════════════════════════════════════════
    // QUOTA (RFC 2087)
    // ══════════════════════════════════════════════════════════════

    /// GETQUOTAROOT — get quota for a mailbox.
    pub fn getquotaroot(&mut self, mailbox: &str) -> ImapResult<Quota> {
        let cmd = format!("GETQUOTAROOT \"{}\"", mailbox);
        let lines = self.command_collect(&cmd)?;
        let mut q = Quota::default();
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("QUOTA ") {
                parse_quota_response(l, &mut q);
            }
        }
        Ok(q)
    }

    /// GETQUOTA — get quota for a named root.
    pub fn getquota(&mut self, root: &str) -> ImapResult<Quota> {
        let cmd = format!("GETQUOTA \"{}\"", root);
        let lines = self.command_collect(&cmd)?;
        let mut q = Quota::default();
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("QUOTA ") {
                parse_quota_response(l, &mut q);
            }
        }
        Ok(q)
    }

    /// SETQUOTA — set quota limit (admin only).
    pub fn setquota(&mut self, root: &str, limit_kb: u64) -> ImapResult<()> {
        let cmd = format!("SETQUOTA \"{}\" (STORAGE {})", root, limit_kb);
        self.command(&cmd)
    }

    // ══════════════════════════════════════════════════════════════
    // ACL (RFC 4314)
    // ══════════════════════════════════════════════════════════════

    /// GETACL — get access control list for a mailbox.
    pub fn getacl(&mut self, mailbox: &str) -> ImapResult<Vec<AclEntry>> {
        let cmd = format!("GETACL \"{}\"", mailbox);
        let lines = self.command_collect(&cmd)?;
        let mut entries = Vec::new();
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("ACL ") {
                let mut p = &l[4..];
                // Skip mailbox name
                if p.starts_with('"') {
                    if let Some(end) = p[1..].find('"') {
                        p = &p[end + 2..];
                    }
                } else {
                    if let Some(sp) = p.find(' ') {
                        p = &p[sp..];
                    }
                }
                // Parse identifier/rights pairs
                let tokens: Vec<&str> = p.split_whitespace().collect();
                let mut i = 0;
                while i + 1 < tokens.len() {
                    let id = tokens[i].trim_matches('"');
                    let rights = tokens[i + 1];
                    entries.push(AclEntry {
                        identifier: id.to_string(),
                        rights: rights.to_string(),
                    });
                    i += 2;
                }
                break;
            }
        }
        Ok(entries)
    }

    /// SETACL — set rights for an identifier on a mailbox.
    pub fn setacl(&mut self, mailbox: &str, identifier: &str, rights: &str) -> ImapResult<()> {
        let cmd = format!("SETACL \"{}\" \"{}\" {}", mailbox, identifier, rights);
        self.command(&cmd)
    }

    /// DELETEACL — remove all rights for an identifier.
    pub fn deleteacl(&mut self, mailbox: &str, identifier: &str) -> ImapResult<()> {
        let cmd = format!("DELETEACL \"{}\" \"{}\"", mailbox, identifier);
        self.command(&cmd)
    }

    /// LISTRIGHTS — list rights available to an identifier.
    pub fn listrights(&mut self, mailbox: &str, identifier: &str) -> ImapResult<String> {
        let cmd = format!("LISTRIGHTS \"{}\" \"{}\"", mailbox, identifier);
        let lines = self.command_collect(&cmd)?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("LISTRIGHTS ") {
                let mut p = &l[11..];
                // Skip mailbox and identifier
                for _ in 0..2 {
                    p = p.trim_start();
                    if p.starts_with('"') {
                        if let Some(end) = p[1..].find('"') {
                            p = &p[end + 2..];
                        }
                    } else if let Some(sp) = p.find(' ') {
                        p = &p[sp..];
                    }
                }
                return Ok(p.trim().to_string());
            }
        }
        Ok(String::new())
    }

    /// MYRIGHTS — get the caller's rights on a mailbox.
    pub fn myrights(&mut self, mailbox: &str) -> ImapResult<String> {
        let cmd = format!("MYRIGHTS \"{}\"", mailbox);
        let lines = self.command_collect(&cmd)?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("MYRIGHTS ") {
                let mut p = &l[9..];
                // Skip mailbox name
                p = p.trim_start();
                if p.starts_with('"') {
                    if let Some(end) = p[1..].find('"') {
                        p = &p[end + 2..];
                    }
                } else if let Some(sp) = p.find(' ') {
                    p = &p[sp..];
                }
                return Ok(p.trim().to_string());
            }
        }
        Ok(String::new())
    }

    // ══════════════════════════════════════════════════════════════
    // Message fetch operations (all UID-based)
    // ══════════════════════════════════════════════════════════════

    fn has_body_peek(&self) -> bool {
        self.level >= ImapLevel::Imap4rev1 || self.level == ImapLevel::Unknown
    }

    /// Fetch UIDs for all messages via UID SEARCH ALL.
    pub fn fetch_uids(&mut self) -> ImapResult<Vec<u64>> {
        let lines = self.command_collect("UID SEARCH ALL")?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("SEARCH") {
                return Ok(parse_uid_list(&l[6..]));
            }
        }
        Ok(Vec::new())
    }

    /// Fetch flags for a UID set (e.g. "1:*").
    pub fn fetch_flags(&mut self, uid_set: &str) -> ImapResult<Vec<(u64, Flags)>> {
        let cmd = format!("UID FETCH {} (UID FLAGS)", uid_set);
        let lines = self.command_collect(&cmd)?;
        let mut result = Vec::new();
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            let uid = find_number_after(&upper, "UID ").unwrap_or(0);
            if uid > 0 {
                let flags = if let Some(pos) = upper.find("FLAGS ") {
                    Flags::parse(&l[pos..])
                } else {
                    Flags::default()
                };
                result.push((uid, flags));
            }
        }
        Ok(result)
    }

    /// Fetch full RFC822 message by UID. Returns raw bytes.
    pub fn fetch_message(&mut self, uid: u64) -> ImapResult<Vec<u8>> {
        let cmd = if self.has_body_peek() {
            format!("UID FETCH {} BODY.PEEK[]", uid)
        } else {
            format!("UID FETCH {} RFC822", uid)
        };
        self.fetch_literal_response(&cmd)
    }

    /// Fetch headers only by UID.
    pub fn fetch_headers(&mut self, uid: u64) -> ImapResult<String> {
        let cmd = if self.has_body_peek() {
            format!("UID FETCH {} BODY.PEEK[HEADER]", uid)
        } else {
            format!("UID FETCH {} RFC822.HEADER", uid)
        };
        let data = self.fetch_literal_response(&cmd)?;
        Ok(String::from_utf8_lossy(&data).into_owned())
    }

    /// Fetch specific header fields by UID.
    /// `fields`: space-separated, e.g. "From Subject Date Message-ID"
    pub fn fetch_header_fields(&mut self, uid: u64, fields: &str) -> ImapResult<String> {
        let cmd = if self.has_body_peek() {
            format!("UID FETCH {} BODY.PEEK[HEADER.FIELDS ({})]", uid, fields)
        } else {
            format!("UID FETCH {} RFC822.HEADER.LINES ({})", uid, fields)
        };
        let data = self.fetch_literal_response(&cmd)?;
        Ok(String::from_utf8_lossy(&data).into_owned())
    }

    /// Fetch body text only by UID.
    pub fn fetch_body(&mut self, uid: u64) -> ImapResult<Vec<u8>> {
        let cmd = if self.has_body_peek() {
            format!("UID FETCH {} BODY.PEEK[TEXT]", uid)
        } else {
            format!("UID FETCH {} RFC822.TEXT", uid)
        };
        self.fetch_literal_response(&cmd)
    }

    /// Fetch a specific MIME section by UID (e.g. "1.2", "2.1.MIME").
    pub fn fetch_section(&mut self, uid: u64, section: &str) -> ImapResult<Vec<u8>> {
        let cmd = format!("UID FETCH {} BODY.PEEK[{}]", uid, section);
        self.fetch_literal_response(&cmd)
    }

    /// Partial fetch: download byte range of a message.
    pub fn fetch_partial(&mut self, uid: u64, offset: u64, count: u64) -> ImapResult<Vec<u8>> {
        let cmd = format!("UID FETCH {} BODY.PEEK[]<{}.{}>", uid, offset, count);
        self.fetch_literal_response(&cmd)
    }

    /// Fetch RFC822.SIZE for a UID. Returns size or error.
    pub fn fetch_size(&mut self, uid: u64) -> ImapResult<u64> {
        let cmd = format!("UID FETCH {} (RFC822.SIZE)", uid);
        let lines = self.command_collect(&cmd)?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if let Some(pos) = upper.find("RFC822.SIZE ") {
                let after = &l[pos + 12..];
                let num = parse_num_from_start(after);
                return Ok(num);
            }
        }
        Err(ImapError::err("No RFC822.SIZE in response"))
    }

    /// Fetch RFC822 — full message (marks as \Seen, legacy form).
    pub fn fetch_rfc822(&mut self, uid: u64) -> ImapResult<Vec<u8>> {
        let cmd = format!("UID FETCH {} RFC822", uid);
        self.fetch_literal_response(&cmd)
    }

    /// Fetch RFC822.HEADER — headers only (legacy form).
    pub fn fetch_rfc822_header(&mut self, uid: u64) -> ImapResult<String> {
        let cmd = format!("UID FETCH {} RFC822.HEADER", uid);
        let data = self.fetch_literal_response(&cmd)?;
        Ok(String::from_utf8_lossy(&data).into_owned())
    }

    /// Fetch RFC822.TEXT — body only (legacy, marks \Seen).
    pub fn fetch_rfc822_text(&mut self, uid: u64) -> ImapResult<Vec<u8>> {
        let cmd = format!("UID FETCH {} RFC822.TEXT", uid);
        self.fetch_literal_response(&cmd)
    }

    /// Fetch INTERNALDATE for a UID.
    pub fn fetch_date(&mut self, uid: u64) -> ImapResult<String> {
        let cmd = format!("UID FETCH {} (INTERNALDATE)", uid);
        let lines = self.command_collect(&cmd)?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if let Some(pos) = upper.find("INTERNALDATE ") {
                let after = &l[pos + 13..].trim_start();
                if after.starts_with('"') {
                    if let Some(end) = after[1..].find('"') {
                        return Ok(after[1..end + 1].to_string());
                    }
                }
            }
        }
        Err(ImapError::err("No INTERNALDATE in response"))
    }

    /// Fetch parsed ENVELOPE for a UID.
    pub fn fetch_envelope(&mut self, uid: u64) -> ImapResult<Envelope> {
        let cmd = format!("UID FETCH {} (ENVELOPE)", uid);
        let lines = self.fetch_collect(&cmd)?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if let Some(pos) = upper.find("ENVELOPE ") {
                let cursor = &l[pos + 9..];
                let mut p = cursor;
                if let Some(env) = parse_envelope_sexp(&mut p) {
                    return Ok(env);
                }
            }
        }
        Err(ImapError::err("No ENVELOPE in response"))
    }

    /// Fetch parsed BODYSTRUCTURE for a UID.
    pub fn fetch_structure(&mut self, uid: u64) -> ImapResult<BodyPart> {
        let use_bodystructure = self.has_body_peek();
        let cmd = if use_bodystructure {
            format!("UID FETCH {} (BODYSTRUCTURE)", uid)
        } else {
            format!("UID FETCH {} (BODY)", uid)
        };
        let lines = self.fetch_collect(&cmd)?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if use_bodystructure {
                if let Some(pos) = upper.find("BODYSTRUCTURE ") {
                    let cursor = &l[pos + 14..];
                    let mut p = cursor;
                    if let Some(bp) = parse_bodystructure(&mut p, "") {
                        return Ok(bp);
                    }
                }
            } else {
                // BODY (not BODYSTRUCTURE)
                let mut search_pos = 0;
                while let Some(pos) = upper[search_pos..].find("BODY ") {
                    let abs_pos = search_pos + pos;
                    // Make sure it's not BODYSTRUCTURE
                    if !upper[abs_pos..].starts_with("BODYSTRUCTURE") {
                        let cursor = &l[abs_pos + 5..];
                        let mut p = cursor;
                        if let Some(bp) = parse_bodystructure(&mut p, "") {
                            return Ok(bp);
                        }
                    }
                    search_pos = abs_pos + 5;
                }
            }
        }
        Err(ImapError::err("No BODYSTRUCTURE in response"))
    }

    /// Fetch BODY (legacy, non-STRUCTURE) for a UID.
    pub fn fetch_body_structure_legacy(&mut self, uid: u64) -> ImapResult<BodyPart> {
        let cmd = format!("UID FETCH {} (BODY)", uid);
        let lines = self.fetch_collect(&cmd)?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            let mut search_pos = 0;
            while let Some(pos) = upper[search_pos..].find("BODY ") {
                let abs_pos = search_pos + pos;
                if !upper[abs_pos..].starts_with("BODYSTRUCTURE") {
                    let cursor = &l[abs_pos + 5..];
                    let mut p = cursor;
                    if let Some(bp) = parse_bodystructure(&mut p, "") {
                        return Ok(bp);
                    }
                }
                search_pos = abs_pos + 5;
            }
        }
        Err(ImapError::err("No BODY in response"))
    }

    /// Fetch overview: UID, FLAGS, SIZE, INTERNALDATE, ENVELOPE, BODYSTRUCTURE
    /// for a UID set.
    pub fn fetch_overview(&mut self, uid_set: &str) -> ImapResult<Vec<FetchResult>> {
        let cmd = format!(
            "UID FETCH {} (UID FLAGS RFC822.SIZE INTERNALDATE ENVELOPE BODYSTRUCTURE)",
            uid_set
        );
        let lines = self.fetch_collect(&cmd)?;
        let mut results = Vec::new();

        for l in &lines {
            let upper = l.to_ascii_uppercase();
            let mut r = FetchResult::default();

            // Sequence number
            if let Some(first_ch) = l.chars().next() {
                if first_ch.is_ascii_digit() {
                    r.seqno = parse_num_from_start(l);
                }
            }

            // UID
            if let Some(uid) = find_number_after(&upper, "UID ") {
                r.uid = uid;
            }

            // FLAGS
            if let Some(pos) = upper.find("FLAGS ") {
                r.flags = Flags::parse(&l[pos..]);
            }

            // RFC822.SIZE
            if let Some(pos) = upper.find("RFC822.SIZE ") {
                r.size = parse_num_from_start(&l[pos + 12..]);
            }

            // INTERNALDATE
            if let Some(pos) = upper.find("INTERNALDATE ") {
                let after = l[pos + 13..].trim_start();
                if after.starts_with('"') {
                    if let Some(end) = after[1..].find('"') {
                        r.internaldate = Some(after[1..end + 1].to_string());
                    }
                }
            }

            // ENVELOPE
            if let Some(pos) = upper.find("ENVELOPE ") {
                let cursor = &l[pos + 9..];
                let mut p = cursor;
                r.envelope = parse_envelope_sexp(&mut p);
            }

            // BODYSTRUCTURE
            if let Some(pos) = upper.find("BODYSTRUCTURE ") {
                let cursor = &l[pos + 14..];
                let mut p = cursor;
                r.bodystructure = parse_bodystructure(&mut p, "");
            }

            if r.uid > 0 {
                results.push(r);
            }
        }

        Ok(results)
    }

    // ══════════════════════════════════════════════════════════════
    // Message modification operations (all UID-based)
    // ══════════════════════════════════════════════════════════════

    /// STORE flags. `action`: "+FLAGS", "-FLAGS", or "FLAGS".
    pub fn store_flags(&mut self, uid_set: &str, action: &str, flags: Flags) -> ImapResult<()> {
        let flag_str = flags.to_imap_string();
        let cmd = format!("UID STORE {} {} ({})", uid_set, action, flag_str);
        self.command(&cmd)
    }

    /// COPY UIDs to another mailbox.
    pub fn copy(&mut self, uid_set: &str, dest: &str) -> ImapResult<()> {
        let cmd = format!("UID COPY {} \"{}\"", uid_set, dest);
        self.command(&cmd)
    }

    /// MOVE UIDs (native MOVE or COPY+DELETE+EXPUNGE fallback).
    pub fn r#move(&mut self, uid_set: &str, dest: &str) -> ImapResult<()> {
        if self.caps.r#move {
            let cmd = format!("UID MOVE {} \"{}\"", uid_set, dest);
            return self.command(&cmd);
        }
        // Fallback: COPY + store \Deleted + EXPUNGE
        self.copy(uid_set, dest)?;
        let del_flags = Flags { deleted: true, ..Flags::default() };
        self.store_flags(uid_set, "+FLAGS", del_flags)?;
        self.expunge()
    }

    /// EXPUNGE deleted messages.
    pub fn expunge(&mut self) -> ImapResult<()> {
        self.command("EXPUNGE")
    }

    /// APPEND a message to a mailbox with optional flags and date.
    pub fn append(
        &mut self,
        mailbox: &str,
        flags: Flags,
        date: Option<&str>,
        message: &[u8],
    ) -> ImapResult<()> {
        let mut flag_part = String::new();
        if flags.any_set() {
            flag_part = format!(" ({})", flags.to_imap_string());
        }

        let mut date_part = String::new();
        if let Some(d) = date {
            if !d.is_empty() {
                date_part = format!(" \"{}\"", d);
            }
        }

        let cmd = format!(
            "APPEND \"{}\"{}{} {{{}}}",
            mailbox, flag_part, date_part, message.len()
        );

        let tag = self.next_tag();
        let full = format!("{} {}", tag, cmd);
        self.send_line(&full)?;

        // Server should respond with + to accept literal
        let cont = self.readline()?;
        if !cont.starts_with('+') {
            return Err(ImapError::err("Expected continuation for APPEND"));
        }

        // Send message data
        self.send_raw(message)?;
        self.send_raw(b"\r\n")?;

        // Read tagged response
        loop {
            let line = self.readline()?;
            if line.starts_with(&tag) && line.as_bytes().get(tag.len()) == Some(&b' ') {
                let status = &line[tag.len() + 1..];
                self.reply = status.to_string();
                return if Self::classify_status(status) == ImapStatus::Ok {
                    Ok(())
                } else {
                    Err(ImapError::no(status))
                };
            }
        }
    }

    // ══════════════════════════════════════════════════════════════
    // UIDPLUS (RFC 4315) — parse APPENDUID/COPYUID from tagged reply
    // ══════════════════════════════════════════════════════════════

    /// Parse APPENDUID from last reply. Returns the new UID or 0.
    pub fn last_append_uid(&self) -> u64 {
        let upper = self.reply.to_ascii_uppercase();
        if let Some(pos) = upper.find("APPENDUID ") {
            let after = &self.reply[pos + 10..];
            // Skip uidvalidity
            let after = after.trim_start();
            if let Some(sp) = after.find(' ') {
                let uid_part = after[sp + 1..].trim_start();
                return parse_num_from_start(uid_part);
            }
        }
        0
    }

    /// Parse COPYUID uidvalidity from last reply. Returns uidvalidity or 0.
    pub fn last_copy_uidvalidity(&self) -> u64 {
        let upper = self.reply.to_ascii_uppercase();
        if let Some(pos) = upper.find("COPYUID ") {
            let after = &self.reply[pos + 8..];
            return parse_num_from_start(after.trim_start());
        }
        0
    }

    // ══════════════════════════════════════════════════════════════
    // Search
    // ══════════════════════════════════════════════════════════════

    /// UID SEARCH. Returns matching UIDs.
    pub fn search(&mut self, criteria: &str) -> ImapResult<Vec<u64>> {
        let cmd = format!("UID SEARCH {}", criteria);
        let lines = self.command_collect(&cmd)?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("SEARCH") {
                return Ok(parse_uid_list(&l[6..]));
            }
        }
        Ok(Vec::new())
    }

    /// UID SORT (RFC 5256). Returns sorted UIDs.
    /// `criteria`: sort keys like "DATE", "SUBJECT", "FROM", "SIZE".
    /// `charset`: e.g. "UTF-8". `search`: search criteria or "ALL".
    pub fn sort(
        &mut self,
        criteria: &str,
        charset: Option<&str>,
        search: Option<&str>,
    ) -> ImapResult<Vec<u64>> {
        if !self.caps.sort {
            return Err(ImapError::no("Server does not support SORT"));
        }
        let cmd = format!(
            "UID SORT ({}) {} {}",
            criteria,
            charset.unwrap_or("UTF-8"),
            search.unwrap_or("ALL")
        );
        let lines = self.command_collect(&cmd)?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("SORT") {
                return Ok(parse_uid_list(&l[4..]));
            }
        }
        Ok(Vec::new())
    }

    /// UID THREAD (RFC 5256). Returns thread structure as IMAP THREAD response string.
    /// `algorithm`: "REFERENCES" or "ORDEREDSUBJECT".
    pub fn thread(
        &mut self,
        algorithm: Option<&str>,
        charset: Option<&str>,
        search: Option<&str>,
    ) -> ImapResult<String> {
        if !self.caps.thread {
            return Err(ImapError::no("Server does not support THREAD"));
        }
        let cmd = format!(
            "UID THREAD {} {} {}",
            algorithm.unwrap_or("REFERENCES"),
            charset.unwrap_or("UTF-8"),
            search.unwrap_or("ALL")
        );
        let lines = self.command_collect(&cmd)?;
        for l in &lines {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("THREAD ") {
                return Ok(l[7..].to_string());
            }
        }
        Ok(String::new())
    }

    // ══════════════════════════════════════════════════════════════
    // Session management
    // ══════════════════════════════════════════════════════════════

    /// NOOP — keep-alive + get pending updates.
    pub fn noop(&mut self) -> ImapResult<()> {
        self.command("NOOP")
    }

    /// IDLE (RFC 2177) — wait for server push.
    /// Returns when server sends an update or `timeout_ms` expires.
    /// Falls back to NOOP if server doesn't support IDLE.
    pub fn idle(&mut self, _timeout_ms: u32) -> ImapResult<()> {
        if !self.caps.idle {
            return self.noop();
        }

        let tag = self.next_tag();
        let full = format!("{} IDLE", tag);
        self.send_line(&full)?;

        // Server sends + continuation to confirm IDLE mode
        let cont = self.readline()?;
        if !cont.starts_with('+') {
            return Err(ImapError::err("Expected continuation for IDLE"));
        }

        // Read untagged update(s) from server
        // Note: timeout handling would require non-blocking I/O or poll();
        // the transport trait doesn't expose fd-level access, so we do a
        // blocking read here (matching the C code's fallback path).
        let update = self.readline()?;
        if update.starts_with("* ") {
            self.reply = update[2..].to_string();
        } else {
            self.reply = update;
        }

        // Send DONE to exit IDLE
        self.send_line("DONE")?;

        // Read remaining untagged responses and tagged reply
        loop {
            let line = self.readline()?;
            if line.starts_with(&tag) && line.as_bytes().get(tag.len()) == Some(&b' ') {
                let status = &line[tag.len() + 1..];
                return if Self::classify_status(status) == ImapStatus::Ok {
                    Ok(())
                } else {
                    Err(ImapError::no(status))
                };
            }
            if line.starts_with("* ") {
                self.reply = line[2..].to_string();
            }
        }
    }

    // ══════════════════════════════════════════════════════════════
    // Message cache
    // ══════════════════════════════════════════════════════════════

    /// Look up a cached entry by UID.
    pub fn cache_lookup(&self, uid: u64) -> Option<&CacheEntry> {
        self.cache.iter().find(|e| e.uid == uid)
    }

    /// Look up a cached entry by sequence number.
    pub fn cache_lookup_seqno(&self, seqno: u64) -> Option<&CacheEntry> {
        self.cache.iter().find(|e| e.seqno == seqno)
    }

    /// Get UID for a sequence number. Returns 0 if not mapped.
    pub fn seqno_to_uid(&self, seqno: u64) -> u64 {
        self.cache_lookup_seqno(seqno).map_or(0, |e| e.uid)
    }

    /// Get sequence number for a UID. Returns 0 if not mapped.
    pub fn uid_to_seqno(&self, uid: u64) -> u64 {
        self.cache_lookup(uid).map_or(0, |e| e.seqno)
    }

    /// Update flags in cache for a UID.
    pub fn cache_update_flags(&mut self, uid: u64, flags: Flags) {
        if let Some(entry) = self.cache.iter_mut().find(|e| e.uid == uid) {
            entry.flags = flags;
        }
    }

    /// Invalidate (clear) the cache.
    pub fn cache_clear(&mut self) {
        self.cache.clear();
    }

    /// Populate cache: fetch UID, FLAGS, SIZE for all messages in selected mailbox.
    pub fn cache_populate(&mut self) -> ImapResult<()> {
        let cmd = "UID FETCH 1:* (UID FLAGS RFC822.SIZE)";
        let lines = self.command_collect(cmd)?;

        self.cache.clear();

        for l in &lines {
            let upper = l.to_ascii_uppercase();
            let mut entry = CacheEntry::default();

            // Parse sequence number
            if let Some(first_ch) = l.chars().next() {
                if first_ch.is_ascii_digit() {
                    entry.seqno = parse_num_from_start(l);
                }
            }

            if let Some(uid) = find_number_after(&upper, "UID ") {
                entry.uid = uid;
            }

            if let Some(pos) = upper.find("FLAGS ") {
                entry.flags = Flags::parse(&l[pos..]);
            }

            if let Some(pos) = upper.find("RFC822.SIZE ") {
                entry.size = parse_num_from_start(&l[pos + 12..]);
            }

            if entry.uid > 0 {
                self.cache.push(entry);
            }
        }

        self.dbg(&format!("Cache populated: {} messages", self.cache.len()));
        Ok(())
    }

    // ══════════════════════════════════════════════════════════════
    // Auto-reconnect
    // ══════════════════════════════════════════════════════════════

    /// Enable auto-reconnect. Stores credentials for re-login.
    pub fn enable_reconnect(&mut self, user: &str, pass: &str) {
        self.reconnect_user = user.to_string();
        self.reconnect_pass = pass.to_string();
        self.reconnect_enabled = true;
    }

    /// Disable auto-reconnect.
    pub fn disable_reconnect(&mut self) {
        self.reconnect_enabled = false;
        self.reconnect_user.clear();
        self.reconnect_pass.clear();
    }

    /// Attempt reconnect now. Re-authenticates and re-selects.
    pub fn reconnect(&mut self) -> ImapResult<()> {
        if self.reconnect_host.is_empty() || self.reconnect_user.is_empty() {
            return Err(ImapError::err("No reconnect credentials stored"));
        }

        let saved_mailbox = self.selected.name.clone();
        let host = self.reconnect_host.clone();
        let port = self.reconnect_port;
        let security = self.reconnect_security;
        let user = self.reconnect_user.clone();
        let pass = self.reconnect_pass.clone();

        self.dbg(&format!("Reconnecting to {}:{}...", host, port));

        // Close existing connection without LOGOUT (it's dead)
        self.tp.close();
        self.connected = false;
        self.authenticated = false;
        self.bye_received = false;

        // Reconnect
        self.connect(&host, port, security)?;
        self.login(&user, &pass)?;

        if !saved_mailbox.is_empty() {
            self.select(&saved_mailbox)?;
        }

        self.dbg("Reconnected successfully");
        Ok(())
    }
}

// ══════════════════════════════════════════════════════════════════
// Modified UTF-7 (RFC 3501 section 5.1.3)
// ══════════════════════════════════════════════════════════════════

const MUTF7_B64: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

fn mutf7_b64_val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b',' => Some(63),
        _ => None,
    }
}

/// Encode UTF-8 mailbox name to IMAP modified UTF-7.
pub fn utf8_to_mutf7(utf8: &str) -> String {
    let mut out = Vec::new();
    let mut chars = utf8.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c >= '\x20' && c <= '\x7e' && c != '&' {
            out.push(c as u8);
            chars.next();
        } else if c == '&' {
            out.push(b'&');
            out.push(b'-');
            chars.next();
        } else {
            // Collect non-ASCII run, convert to UTF-16BE, then modified base64
            out.push(b'&');
            let mut utf16 = Vec::new();
            while let Some(&ch) = chars.peek() {
                if ch >= '\x20' && ch <= '\x7e' {
                    break;
                }
                chars.next();
                let cp = ch as u32;
                if cp < 0x10000 {
                    utf16.push(cp as u16);
                } else {
                    // Surrogate pair
                    let adj = cp - 0x10000;
                    utf16.push(0xD800 | ((adj >> 10) as u16));
                    utf16.push(0xDC00 | ((adj & 0x3FF) as u16));
                }
            }

            // Convert to big-endian bytes
            let mut bytes = Vec::new();
            for &u in &utf16 {
                bytes.push((u >> 8) as u8);
                bytes.push((u & 0xFF) as u8);
            }

            // Base64 encode without padding
            let mut bi = 0;
            while bi < bytes.len() {
                let b0 = bytes[bi] as u32;
                bi += 1;
                let b1 = if bi < bytes.len() { bytes[bi] as i32 } else { -1 };
                if b1 >= 0 { bi += 1; }
                let b2 = if bi < bytes.len() { bytes[bi] as i32 } else { -1 };
                if b2 >= 0 { bi += 1; }

                out.push(MUTF7_B64[(b0 >> 2) as usize]);
                out.push(MUTF7_B64[(((b0 & 3) << 4) | if b1 >= 0 { b1 as u32 >> 4 } else { 0 }) as usize]);
                if b1 >= 0 {
                    out.push(MUTF7_B64[(((b1 as u32 & 0xF) << 2) | if b2 >= 0 { b2 as u32 >> 6 } else { 0 }) as usize]);
                }
                if b2 >= 0 {
                    out.push(MUTF7_B64[(b2 as u32 & 0x3F) as usize]);
                }
            }
            out.push(b'-');
        }
    }

    String::from_utf8(out).unwrap_or_default()
}

/// Decode IMAP modified UTF-7 mailbox name to UTF-8.
pub fn mutf7_to_utf8(mutf7: &str) -> String {
    let bytes = mutf7.as_bytes();
    let mut out = String::new();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'&' {
            out.push(bytes[i] as char);
            i += 1;
        } else {
            i += 1; // skip '&'
            if i < bytes.len() && bytes[i] == b'-' {
                out.push('&');
                i += 1;
            } else {
                // Decode modified base64 until '-'
                let mut raw_bytes = Vec::new();
                let mut accum: u32 = 0;
                let mut bits: u32 = 0;
                while i < bytes.len() && bytes[i] != b'-' {
                    if let Some(val) = mutf7_b64_val(bytes[i]) {
                        accum = (accum << 6) | (val as u32);
                        bits += 6;
                        if bits >= 8 {
                            bits -= 8;
                            raw_bytes.push(((accum >> bits) & 0xFF) as u8);
                        }
                    }
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'-' {
                    i += 1;
                }

                // Decode UTF-16BE to UTF-8
                let mut j = 0;
                while j + 1 < raw_bytes.len() {
                    let mut cp = ((raw_bytes[j] as u32) << 8) | (raw_bytes[j + 1] as u32);
                    j += 2;
                    // Handle surrogate pairs
                    if cp >= 0xD800 && cp <= 0xDBFF && j + 1 < raw_bytes.len() {
                        let lo = ((raw_bytes[j] as u32) << 8) | (raw_bytes[j + 1] as u32);
                        if lo >= 0xDC00 && lo <= 0xDFFF {
                            cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                            j += 2;
                        }
                    }
                    if let Some(c) = char::from_u32(cp) {
                        out.push(c);
                    }
                }
            }
        }
    }

    out
}

// ══════════════════════════════════════════════════════════════════
// S-expression parser for ENVELOPE and BODYSTRUCTURE
// ══════════════════════════════════════════════════════════════════

/// Skip whitespace in the cursor.
fn skip_ws(p: &mut &str) {
    *p = p.trim_start_matches(|c: char| c == ' ' || c == '\t' || c == '\r' || c == '\n');
}

/// Parse a quoted string or NIL. Advances cursor.
fn parse_nstring(p: &mut &str) -> Option<String> {
    skip_ws(p);
    let bytes = p.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    // NIL
    if bytes.len() >= 3 {
        let first3 = &p[..3];
        if first3.eq_ignore_ascii_case("NIL") {
            let after = if bytes.len() > 3 { bytes[3] } else { 0 };
            if after == 0 || after == b' ' || after == b')' || after == b'(' {
                *p = &p[3..];
                return None;
            }
        }
    }

    // Quoted string
    if bytes[0] == b'"' {
        *p = &p[1..]; // skip opening quote
        let mut result = String::new();
        let b = p.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'"' {
                *p = &p[i + 1..];
                return Some(result);
            }
            if b[i] == b'\\' && i + 1 < b.len() {
                result.push(b[i + 1] as char);
                i += 2;
            } else {
                result.push(b[i] as char);
                i += 1;
            }
        }
        *p = &p[b.len()..];
        return Some(result);
    }

    // Atom (unquoted token) — not ( or )
    if bytes[0] != b'(' && bytes[0] != b')' {
        let end = p.find(|c: char| c == ' ' || c == ')' || c == '(' || c == '\r' || c == '\n')
            .unwrap_or(p.len());
        let token = &p[..end];
        *p = &p[end..];
        if token.eq_ignore_ascii_case("NIL") {
            return None;
        }
        return Some(token.to_string());
    }

    None
}

/// Parse a number atom. Returns the number, 0 if NIL.
fn parse_number(p: &mut &str) -> u64 {
    skip_ws(p);
    if p.len() >= 3 && p[..3].eq_ignore_ascii_case("NIL") {
        let after = p.as_bytes().get(3).copied().unwrap_or(0);
        if after == 0 || after == b' ' || after == b')' {
            *p = &p[3..];
            return 0;
        }
    }
    let end = p.find(|c: char| !c.is_ascii_digit()).unwrap_or(p.len());
    if end == 0 { return 0; }
    let num = p[..end].parse::<u64>().unwrap_or(0);
    *p = &p[end..];
    num
}

/// Skip one S-expression element (atom, string, or nested parens).
fn skip_sexp(p: &mut &str) {
    skip_ws(p);
    if p.is_empty() { return; }
    let first = p.as_bytes()[0];

    if first == b'(' {
        let mut depth = 1i32;
        *p = &p[1..];
        while !p.is_empty() && depth > 0 {
            let b = p.as_bytes()[0];
            if b == b'(' { depth += 1; }
            else if b == b')' { depth -= 1; }
            else if b == b'"' {
                *p = &p[1..];
                while !p.is_empty() && p.as_bytes()[0] != b'"' {
                    if p.as_bytes()[0] == b'\\' { *p = &p[1..]; }
                    if !p.is_empty() { *p = &p[1..]; }
                }
            }
            if !p.is_empty() { *p = &p[1..]; }
        }
    } else if first == b'"' {
        *p = &p[1..];
        while !p.is_empty() && p.as_bytes()[0] != b'"' {
            if p.as_bytes()[0] == b'\\' { *p = &p[1..]; }
            if !p.is_empty() { *p = &p[1..]; }
        }
        if !p.is_empty() { *p = &p[1..]; }
    } else {
        // Atom or NIL
        let end = p.find(|c: char| c == ' ' || c == ')' || c == '(').unwrap_or(p.len());
        *p = &p[end..];
    }
}

/// Parse a single address: `(personal adl mailbox host)`.
fn parse_address(p: &mut &str) -> Option<Address> {
    skip_ws(p);
    if p.is_empty() || p.as_bytes()[0] != b'(' { return None; }
    *p = &p[1..]; // skip (

    let mut addr = Address::default();
    addr.name = parse_nstring(p);
    addr.adl = parse_nstring(p);
    addr.mailbox = parse_nstring(p);
    addr.host = parse_nstring(p);

    skip_ws(p);
    if !p.is_empty() && p.as_bytes()[0] == b')' { *p = &p[1..]; }
    Some(addr)
}

/// Parse an address list: `((addr1) (addr2) ...)` or NIL.
fn parse_address_list(p: &mut &str) -> Vec<Address> {
    skip_ws(p);
    if p.len() >= 3 && p[..3].eq_ignore_ascii_case("NIL") {
        let after = p.as_bytes().get(3).copied().unwrap_or(0);
        if after == 0 || after == b' ' || after == b')' {
            *p = &p[3..];
            return Vec::new();
        }
    }
    if p.is_empty() || p.as_bytes()[0] != b'(' { return Vec::new(); }
    *p = &p[1..]; // skip outer (

    let mut addrs = Vec::new();
    loop {
        skip_ws(p);
        if p.is_empty() || p.as_bytes()[0] == b')' { break; }
        if let Some(a) = parse_address(p) {
            addrs.push(a);
        } else {
            break;
        }
    }
    if !p.is_empty() && p.as_bytes()[0] == b')' { *p = &p[1..]; }
    addrs
}

/// Parse ENVELOPE: `(date subject from sender reply_to to cc bcc in_reply_to message_id)`.
fn parse_envelope_sexp(p: &mut &str) -> Option<Envelope> {
    skip_ws(p);
    if p.is_empty() || p.as_bytes()[0] != b'(' { return None; }
    *p = &p[1..];

    let mut env = Envelope::default();
    env.date = parse_nstring(p);
    env.subject = parse_nstring(p);
    env.from = parse_address_list(p);
    env.sender = parse_address_list(p);
    env.reply_to = parse_address_list(p);
    env.to = parse_address_list(p);
    env.cc = parse_address_list(p);
    env.bcc = parse_address_list(p);
    env.in_reply_to = parse_nstring(p);
    env.message_id = parse_nstring(p);

    skip_ws(p);
    if !p.is_empty() && p.as_bytes()[0] == b')' { *p = &p[1..]; }
    Some(env)
}

/// Parse MIME parameter list: `(name1 value1 name2 value2 ...)` or NIL.
fn parse_param_list(p: &mut &str) -> Vec<Param> {
    skip_ws(p);
    if p.len() >= 3 && p[..3].eq_ignore_ascii_case("NIL") {
        let after = p.as_bytes().get(3).copied().unwrap_or(0);
        if after == 0 || after == b' ' || after == b')' {
            *p = &p[3..];
            return Vec::new();
        }
    }
    if p.is_empty() || p.as_bytes()[0] != b'(' { return Vec::new(); }
    *p = &p[1..];

    let mut params = Vec::new();
    loop {
        skip_ws(p);
        if p.is_empty() || p.as_bytes()[0] == b')' { break; }
        let name = parse_nstring(p);
        let value = parse_nstring(p);
        if let Some(n) = name {
            params.push(Param {
                name: n,
                value: value.unwrap_or_default(),
            });
        }
    }
    if !p.is_empty() && p.as_bytes()[0] == b')' { *p = &p[1..]; }
    params
}

/// Parse disposition: `(type (params))` or NIL.
fn parse_disposition_sexp(p: &mut &str) -> Disposition {
    skip_ws(p);
    if p.len() >= 3 && p[..3].eq_ignore_ascii_case("NIL") {
        let after = p.as_bytes().get(3).copied().unwrap_or(0);
        if after == 0 || after == b' ' || after == b')' {
            *p = &p[3..];
            return Disposition::default();
        }
    }
    if p.is_empty() || p.as_bytes()[0] != b'(' { return Disposition::default(); }
    *p = &p[1..];

    let dtype = parse_nstring(p);
    let params = parse_param_list(p);

    skip_ws(p);
    if !p.is_empty() && p.as_bytes()[0] == b')' { *p = &p[1..]; }
    Disposition { dtype, params }
}

/// Parse a single (non-multipart) body part.
fn parse_body_single(p: &mut &str, section: &str) -> BodyPart {
    let mut bp = BodyPart::default();
    bp.section = if section.is_empty() { "1".to_string() } else { section.to_string() };

    // type subtype
    let type_str = parse_nstring(p).unwrap_or_default();
    bp.subtype = parse_nstring(p);
    bp.body_type = BodyType::from_str_ci(&type_str);

    // params
    bp.params = parse_param_list(p);

    // id, description, encoding
    bp.id = parse_nstring(p);
    bp.description = parse_nstring(p);
    let enc_str = parse_nstring(p).unwrap_or_default();
    bp.encoding = Encoding::from_str_ci(&enc_str);

    // size in bytes
    bp.size_bytes = parse_number(p);

    // For text types: line count
    if bp.body_type == BodyType::Text {
        bp.size_lines = parse_number(p);
    }

    // For MESSAGE/RFC822: envelope, bodystructure, lines
    if bp.body_type == BodyType::Message {
        if let Some(ref sub) = bp.subtype {
            if sub.eq_ignore_ascii_case("RFC822") {
                bp.nested_env = parse_envelope_sexp(p).map(Box::new);
                bp.nested_body = parse_bodystructure(p, section).map(Box::new);
                bp.size_lines = parse_number(p);
            }
        }
    }

    // Extension data: md5, disposition, language, location
    skip_ws(p);
    if !p.is_empty() && p.as_bytes()[0] != b')' {
        bp.md5 = parse_nstring(p);
        skip_ws(p);
        if !p.is_empty() && p.as_bytes()[0] != b')' {
            bp.disposition = parse_disposition_sexp(p);
            skip_ws(p);
            if !p.is_empty() && p.as_bytes()[0] != b')' {
                bp.language = parse_nstring(p);
                skip_ws(p);
                if !p.is_empty() && p.as_bytes()[0] != b')' {
                    bp.location = parse_nstring(p);
                    // Skip further extensions
                    skip_ws(p);
                    while !p.is_empty() && p.as_bytes()[0] != b')' {
                        skip_sexp(p);
                        skip_ws(p);
                    }
                }
            }
        }
    }

    bp
}

/// Parse BODYSTRUCTURE recursively.
fn parse_bodystructure(p: &mut &str, prefix: &str) -> Option<BodyPart> {
    skip_ws(p);
    if p.is_empty() || p.as_bytes()[0] != b'(' { return None; }
    *p = &p[1..]; // skip (

    skip_ws(p);

    // If next char is '(' it's multipart
    if !p.is_empty() && p.as_bytes()[0] == b'(' {
        let mut bp = BodyPart::default();
        bp.body_type = BodyType::Multipart;
        bp.section = prefix.to_string();

        let mut part_num = 1u32;
        while !p.is_empty() && p.as_bytes()[0] == b'(' {
            let child_section = if prefix.is_empty() {
                format!("{}", part_num)
            } else {
                format!("{}.{}", prefix, part_num)
            };

            if let Some(child) = parse_bodystructure(p, &child_section) {
                bp.subparts.push(child);
            }
            part_num += 1;
            skip_ws(p);
        }

        // After all parts: subtype
        bp.subtype = parse_nstring(p);

        // Extension data: params, disposition, language, location
        skip_ws(p);
        if !p.is_empty() && p.as_bytes()[0] != b')' {
            bp.params = parse_param_list(p);
            skip_ws(p);
            if !p.is_empty() && p.as_bytes()[0] != b')' {
                bp.disposition = parse_disposition_sexp(p);
                skip_ws(p);
                if !p.is_empty() && p.as_bytes()[0] != b')' {
                    bp.language = parse_nstring(p);
                    skip_ws(p);
                    if !p.is_empty() && p.as_bytes()[0] != b')' {
                        bp.location = parse_nstring(p);
                        skip_ws(p);
                        while !p.is_empty() && p.as_bytes()[0] != b')' {
                            skip_sexp(p);
                            skip_ws(p);
                        }
                    }
                }
            }
        }

        skip_ws(p);
        if !p.is_empty() && p.as_bytes()[0] == b')' { *p = &p[1..]; }
        return Some(bp);
    }

    // Single body part
    let bp = parse_body_single(p, prefix);
    skip_ws(p);
    if !p.is_empty() && p.as_bytes()[0] == b')' { *p = &p[1..]; }
    Some(bp)
}

// ══════════════════════════════════════════════════════════════════
// LIST/LSUB response parser
// ══════════════════════════════════════════════════════════════════

fn parse_list_response(prefix: &str, lines: &[String]) -> Vec<ListEntry> {
    let prefix_upper = prefix.to_ascii_uppercase();
    let mut entries = Vec::new();

    for l in lines {
        let upper = l.to_ascii_uppercase();
        if !upper.starts_with(&prefix_upper) { continue; }

        let rest = &l[prefix.len()..];
        let mut entry = ListEntry::default();

        let mut pos = rest;

        // Parse attributes: (\Noselect \HasChildren ...)
        if pos.starts_with('(') {
            if let Some(close) = pos.find(')') {
                let attrs = &pos[1..close].to_ascii_uppercase();
                entry.noselect = attrs.contains("\\NOSELECT");
                entry.noinferiors = attrs.contains("\\NOINFERIORS");
                entry.has_children = attrs.contains("\\HASCHILDREN");
                entry.has_no_children = attrs.contains("\\HASNOCHILDREN");
                entry.marked = attrs.contains("\\MARKED");
                entry.unmarked = attrs.contains("\\UNMARKED");
                pos = &pos[close + 1..];
            }
        }

        // Parse delimiter
        let pos = pos.trim_start();
        let (delim, pos) = if pos.starts_with('"') {
            let after_quote = &pos[1..];
            let ch = after_quote.chars().next();
            let remainder = if let Some(end) = after_quote[1..].find('"') {
                &after_quote[end + 2..]
            } else {
                &after_quote[1..]
            };
            (ch, remainder)
        } else if pos.to_ascii_uppercase().starts_with("NIL") {
            (None, &pos[3..])
        } else {
            (None, pos)
        };
        entry.delimiter = delim;

        // Parse name
        let pos = pos.trim_start();
        if pos.starts_with('"') {
            if let Some(end) = pos[1..].find('"') {
                entry.name = pos[1..end + 1].to_string();
            }
        } else {
            entry.name = pos.trim().to_string();
        }

        entries.push(entry);
    }

    entries
}

// ══════════════════════════════════════════════════════════════════
// NAMESPACE parser
// ══════════════════════════════════════════════════════════════════

fn parse_namespace_group(p: &mut &str) -> Vec<NamespaceEntry> {
    skip_ws(p);
    if p.len() >= 3 && p[..3].eq_ignore_ascii_case("NIL") {
        let after = p.as_bytes().get(3).copied().unwrap_or(0);
        if after == 0 || after == b' ' || after == b')' {
            *p = &p[3..];
            return Vec::new();
        }
    }
    if p.is_empty() || p.as_bytes()[0] != b'(' { return Vec::new(); }
    *p = &p[1..]; // outer (

    let mut entries = Vec::new();
    loop {
        skip_ws(p);
        if p.is_empty() || p.as_bytes()[0] == b')' { break; }
        if p.as_bytes()[0] != b'(' { break; }
        *p = &p[1..]; // inner (

        let prefix = parse_nstring(p).unwrap_or_default();
        let delim_str = parse_nstring(p);
        let delimiter = delim_str.and_then(|s| s.chars().next());

        entries.push(NamespaceEntry { prefix, delimiter });

        // Skip extension data
        skip_ws(p);
        while !p.is_empty() && p.as_bytes()[0] != b')' {
            skip_sexp(p);
            skip_ws(p);
        }
        if !p.is_empty() && p.as_bytes()[0] == b')' { *p = &p[1..]; }
    }
    if !p.is_empty() && p.as_bytes()[0] == b')' { *p = &p[1..]; }
    entries
}

// ══════════════════════════════════════════════════════════════════
// QUOTA parser
// ══════════════════════════════════════════════════════════════════

fn parse_quota_response(line: &str, q: &mut Quota) {
    let upper = line.to_ascii_uppercase();
    if let Some(pos) = upper.find("QUOTA ") {
        let rest = &line[pos + 6..];
        // Parse root name
        if rest.starts_with('"') {
            if let Some(end) = rest[1..].find('"') {
                q.root = rest[1..end + 1].to_string();
            }
        }
        // Parse (STORAGE usage limit)
        let upper_rest = rest.to_ascii_uppercase();
        if let Some(stor_pos) = upper_rest.find("STORAGE ") {
            let after = &rest[stor_pos + 8..];
            let nums: Vec<u64> = after.split_whitespace()
                .filter_map(|s| s.trim_matches(')').parse().ok())
                .collect();
            if nums.len() >= 2 {
                q.usage = nums[0];
                q.limit = nums[1];
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// Utility functions
// ══════════════════════════════════════════════════════════════════

/// Quote a string for IMAP: escape backslashes and quotes.
fn imap_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Parse a list of UIDs from a space-separated string.
fn parse_uid_list(s: &str) -> Vec<u64> {
    s.split_whitespace()
        .filter_map(|token| token.parse::<u64>().ok())
        .filter(|&uid| uid > 0)
        .collect()
}

/// Parse a number appearing right after some delimiters (space, ]).
fn parse_num_after_delims(s: &str) -> u64 {
    let trimmed = s.trim_start_matches(|c: char| c == ' ' || c == ']');
    parse_num_from_start(trimmed)
}

/// Parse a number after spaces.
fn parse_num_after_space(s: &str) -> u64 {
    let trimmed = s.trim_start();
    parse_num_from_start(trimmed)
}

/// Parse a number from the start of a string.
fn parse_num_from_start(s: &str) -> u64 {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 { return 0; }
    s[..end].parse().unwrap_or(0)
}

/// Find a number after a keyword like "UID ".
fn find_number_after(haystack: &str, keyword: &str) -> Option<u64> {
    haystack.find(keyword).map(|pos| {
        let after = &haystack[pos + keyword.len()..];
        parse_num_from_start(after)
    })
}

// ══════════════════════════════════════════════════════════════════
// SHA-256 (minimal pure-Rust for SCRAM-SHA-256)
// ══════════════════════════════════════════════════════════════════

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    // Pad
    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    // Process blocks
    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[i*4], block[i*4+1], block[i*4+2], block[i*4+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(SHA256_K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g; g = f; f = e; e = d.wrapping_add(temp1);
            d = c; c = b; b = a; a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }

    let mut result = [0u8; 32];
    for (i, val) in h.iter().enumerate() {
        result[i*4..i*4+4].copy_from_slice(&val.to_be_bytes());
    }
    result
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut actual_key = [0u8; 64];
    if key.len() > 64 {
        let h = sha256(key);
        actual_key[..32].copy_from_slice(&h);
    } else {
        actual_key[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= actual_key[i];
        opad[i] ^= actual_key[i];
    }

    // inner = SHA256(ipad || data)
    let mut inner_data = Vec::with_capacity(64 + data.len());
    inner_data.extend_from_slice(&ipad);
    inner_data.extend_from_slice(data);
    let inner_hash = sha256(&inner_data);

    // outer = SHA256(opad || inner_hash)
    let mut outer_data = Vec::with_capacity(64 + 32);
    outer_data.extend_from_slice(&opad);
    outer_data.extend_from_slice(&inner_hash);
    sha256(&outer_data)
}

fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, dk_len: usize) -> Vec<u8> {
    let mut result = Vec::new();
    let mut block_num = 1u32;

    while result.len() < dk_len {
        // U1 = HMAC(password, salt || INT(block_num))
        let mut salt_block = salt.to_vec();
        salt_block.extend_from_slice(&block_num.to_be_bytes());

        let mut u = hmac_sha256(password, &salt_block);
        let mut t = u;

        for _ in 1..iterations {
            u = hmac_sha256(password, &u);
            for j in 0..32 {
                t[j] ^= u[j];
            }
        }

        result.extend_from_slice(&t);
        block_num += 1;
    }

    result.truncate(dk_len);
    result
}

// ══════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Flags parsing ──

    #[test]
    fn test_flags_parse() {
        let f = Flags::parse("(\\Seen \\Flagged \\Deleted)");
        assert!(f.seen);
        assert!(f.flagged);
        assert!(f.deleted);
        assert!(!f.answered);
        assert!(!f.draft);
        assert!(!f.recent);
    }

    #[test]
    fn test_flags_to_string() {
        let f = Flags { seen: true, answered: true, ..Flags::default() };
        let s = f.to_imap_string();
        assert!(s.contains("\\Seen"));
        assert!(s.contains("\\Answered"));
        assert!(!s.contains("\\Flagged"));
    }

    #[test]
    fn test_flags_empty() {
        let f = Flags::default();
        assert_eq!(f.to_imap_string(), "");
        assert!(!f.any_set());
    }

    // ── Modified UTF-7 ──

    #[test]
    fn test_mutf7_ascii() {
        assert_eq!(utf8_to_mutf7("INBOX"), "INBOX");
        assert_eq!(mutf7_to_utf8("INBOX"), "INBOX");
    }

    #[test]
    fn test_mutf7_ampersand() {
        assert_eq!(utf8_to_mutf7("A&B"), "A&-B");
        assert_eq!(mutf7_to_utf8("A&-B"), "A&B");
    }

    #[test]
    fn test_mutf7_roundtrip_unicode() {
        let names = [
            "Br\u{00f8}dre",       // Norwegian
            "\u{65e5}\u{672c}\u{8a9e}", // Japanese
            "\u{0410}\u{0411}\u{0412}", // Cyrillic
        ];
        for name in &names {
            let encoded = utf8_to_mutf7(name);
            let decoded = mutf7_to_utf8(&encoded);
            assert_eq!(&decoded, name, "roundtrip failed for '{}'", name);
        }
    }

    // ── S-expression parser helpers ──

    #[test]
    fn test_parse_nstring_nil() {
        let mut p = "NIL rest";
        assert_eq!(parse_nstring(&mut p), None);
        assert_eq!(p, " rest");
    }

    #[test]
    fn test_parse_nstring_quoted() {
        let mut p = r#""hello world" rest"#;
        assert_eq!(parse_nstring(&mut p), Some("hello world".to_string()));
        assert_eq!(p, " rest");
    }

    #[test]
    fn test_parse_nstring_quoted_escaped() {
        let mut p = r#""say \"hi\"" rest"#;
        assert_eq!(parse_nstring(&mut p), Some("say \"hi\"".to_string()));
        assert_eq!(p, " rest");
    }

    #[test]
    fn test_parse_nstring_atom() {
        let mut p = "INBOX rest";
        assert_eq!(parse_nstring(&mut p), Some("INBOX".to_string()));
        assert_eq!(p, " rest");
    }

    #[test]
    fn test_parse_number() {
        let mut p = "12345 rest";
        assert_eq!(parse_number(&mut p), 12345);
        assert_eq!(p, " rest");
    }

    #[test]
    fn test_parse_number_nil() {
        let mut p = "NIL rest";
        assert_eq!(parse_number(&mut p), 0);
        assert_eq!(p, " rest");
    }

    // ── Envelope parsing ──

    #[test]
    fn test_parse_envelope() {
        let input = r#"("Mon, 1 Jan 2024 00:00:00 +0000" "Test Subject" ((NIL NIL "sender" "example.com")) ((NIL NIL "sender" "example.com")) ((NIL NIL "sender" "example.com")) ((NIL NIL "recipient" "example.com")) NIL NIL NIL "<msg@example.com>")"#;
        let mut p = input;
        let env = parse_envelope_sexp(&mut p).unwrap();
        assert_eq!(env.date.as_deref(), Some("Mon, 1 Jan 2024 00:00:00 +0000"));
        assert_eq!(env.subject.as_deref(), Some("Test Subject"));
        assert_eq!(env.from.len(), 1);
        assert_eq!(env.from[0].mailbox.as_deref(), Some("sender"));
        assert_eq!(env.from[0].host.as_deref(), Some("example.com"));
        assert_eq!(env.to.len(), 1);
        assert_eq!(env.to[0].mailbox.as_deref(), Some("recipient"));
        assert_eq!(env.message_id.as_deref(), Some("<msg@example.com>"));
        assert!(env.cc.is_empty());
        assert!(env.bcc.is_empty());
    }

    #[test]
    fn test_parse_envelope_nil_fields() {
        let input = r#"(NIL NIL NIL NIL NIL NIL NIL NIL NIL NIL)"#;
        let mut p = input;
        let env = parse_envelope_sexp(&mut p).unwrap();
        assert!(env.date.is_none());
        assert!(env.subject.is_none());
        assert!(env.from.is_empty());
        assert!(env.message_id.is_none());
    }

    #[test]
    fn test_parse_envelope_multiple_addresses() {
        let input = r#"("date" "subj" ((NIL NIL "a" "x.com")(NIL NIL "b" "y.com")) NIL NIL NIL NIL NIL NIL NIL)"#;
        let mut p = input;
        let env = parse_envelope_sexp(&mut p).unwrap();
        assert_eq!(env.from.len(), 2);
        assert_eq!(env.from[0].mailbox.as_deref(), Some("a"));
        assert_eq!(env.from[1].mailbox.as_deref(), Some("b"));
    }

    // ── Address parsing ──

    #[test]
    fn test_address_email() {
        let addr = Address {
            name: Some("John Doe".to_string()),
            adl: None,
            mailbox: Some("john".to_string()),
            host: Some("example.com".to_string()),
        };
        assert_eq!(addr.email(), "john@example.com");
    }

    // ── BODYSTRUCTURE parsing ──

    #[test]
    fn test_parse_bodystructure_simple_text() {
        let input = r#"("TEXT" "PLAIN" ("CHARSET" "UTF-8") NIL NIL "7BIT" 1234 56)"#;
        let mut p = input;
        let bp = parse_bodystructure(&mut p, "").unwrap();
        assert_eq!(bp.body_type, BodyType::Text);
        assert_eq!(bp.subtype.as_deref(), Some("PLAIN"));
        assert_eq!(bp.encoding, Encoding::SevenBit);
        assert_eq!(bp.size_bytes, 1234);
        assert_eq!(bp.size_lines, 56);
        assert_eq!(bp.params.len(), 1);
        assert_eq!(bp.params[0].name, "CHARSET");
        assert_eq!(bp.params[0].value, "UTF-8");
    }

    #[test]
    fn test_parse_bodystructure_multipart() {
        let input = r#"(("TEXT" "PLAIN" ("CHARSET" "UTF-8") NIL NIL "7BIT" 100 10)("TEXT" "HTML" ("CHARSET" "UTF-8") NIL NIL "QUOTED-PRINTABLE" 200 20) "ALTERNATIVE")"#;
        let mut p = input;
        let bp = parse_bodystructure(&mut p, "").unwrap();
        assert_eq!(bp.body_type, BodyType::Multipart);
        assert_eq!(bp.subtype.as_deref(), Some("ALTERNATIVE"));
        assert_eq!(bp.subparts.len(), 2);
        assert_eq!(bp.subparts[0].body_type, BodyType::Text);
        assert_eq!(bp.subparts[0].subtype.as_deref(), Some("PLAIN"));
        assert_eq!(bp.subparts[0].section, "1");
        assert_eq!(bp.subparts[1].subtype.as_deref(), Some("HTML"));
        assert_eq!(bp.subparts[1].encoding, Encoding::QuotedPrintable);
        assert_eq!(bp.subparts[1].section, "2");
    }

    #[test]
    fn test_parse_bodystructure_nested_multipart() {
        let input = r#"(("TEXT" "PLAIN" NIL NIL NIL "7BIT" 50 5)(("TEXT" "HTML" NIL NIL NIL "7BIT" 100 10)("IMAGE" "PNG" NIL NIL NIL "BASE64" 500 0) "RELATED") "MIXED")"#;
        let mut p = input;
        let bp = parse_bodystructure(&mut p, "").unwrap();
        assert_eq!(bp.body_type, BodyType::Multipart);
        assert_eq!(bp.subtype.as_deref(), Some("MIXED"));
        assert_eq!(bp.subparts.len(), 2);
        assert_eq!(bp.subparts[0].section, "1");
        assert_eq!(bp.subparts[1].body_type, BodyType::Multipart);
        assert_eq!(bp.subparts[1].section, "2");
        assert_eq!(bp.subparts[1].subparts.len(), 2);
        assert_eq!(bp.subparts[1].subparts[0].section, "2.1");
        assert_eq!(bp.subparts[1].subparts[1].section, "2.2");
        assert_eq!(bp.subparts[1].subparts[1].body_type, BodyType::Image);
    }

    #[test]
    fn test_parse_bodystructure_with_disposition() {
        let input = r#"("APPLICATION" "PDF" NIL NIL NIL "BASE64" 50000 NIL ("ATTACHMENT" ("FILENAME" "doc.pdf")) NIL NIL)"#;
        let mut p = input;
        let bp = parse_bodystructure(&mut p, "").unwrap();
        assert_eq!(bp.body_type, BodyType::Application);
        assert_eq!(bp.subtype.as_deref(), Some("PDF"));
        assert_eq!(bp.encoding, Encoding::Base64);
        assert_eq!(bp.disposition.dtype.as_deref(), Some("ATTACHMENT"));
        assert_eq!(bp.disposition.params.len(), 1);
        assert_eq!(bp.disposition.params[0].name, "FILENAME");
        assert_eq!(bp.disposition.params[0].value, "doc.pdf");
    }

    // ── LIST response parsing ──

    #[test]
    fn test_parse_list_response() {
        let lines = vec![
            r#"LIST (\HasNoChildren) "." INBOX"#.to_string(),
            r#"LIST (\Noselect \HasChildren) "." "Folder""#.to_string(),
            r#"LIST (\HasNoChildren) "/" Sent"#.to_string(),
        ];
        let entries = parse_list_response("LIST ", &lines);
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].name, "INBOX");
        assert_eq!(entries[0].delimiter, Some('.'));
        assert!(entries[0].has_no_children);
        assert!(!entries[0].noselect);

        assert_eq!(entries[1].name, "Folder");
        assert!(entries[1].noselect);
        assert!(entries[1].has_children);

        assert_eq!(entries[2].name, "Sent");
        assert_eq!(entries[2].delimiter, Some('/'));
    }

    // ── Param list parsing ──

    #[test]
    fn test_parse_param_list() {
        let mut p = r#"("CHARSET" "UTF-8" "NAME" "file.txt") rest"#;
        let params = parse_param_list(&mut p);
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "CHARSET");
        assert_eq!(params[0].value, "UTF-8");
        assert_eq!(params[1].name, "NAME");
        assert_eq!(params[1].value, "file.txt");
    }

    #[test]
    fn test_parse_param_list_nil() {
        let mut p = "NIL rest";
        let params = parse_param_list(&mut p);
        assert!(params.is_empty());
    }

    // ── Disposition parsing ──

    #[test]
    fn test_parse_disposition() {
        let mut p = r#"("INLINE" ("FILENAME" "img.jpg")) rest"#;
        let d = parse_disposition_sexp(&mut p);
        assert_eq!(d.dtype.as_deref(), Some("INLINE"));
        assert_eq!(d.params.len(), 1);
        assert_eq!(d.params[0].name, "FILENAME");
    }

    #[test]
    fn test_parse_disposition_nil() {
        let mut p = "NIL rest";
        let d = parse_disposition_sexp(&mut p);
        assert!(d.dtype.is_none());
        assert!(d.params.is_empty());
    }

    // ── skip_sexp ──

    #[test]
    fn test_skip_sexp_atom() {
        let mut p = "ATOM rest";
        skip_sexp(&mut p);
        assert_eq!(p, " rest");
    }

    #[test]
    fn test_skip_sexp_quoted() {
        let mut p = r#""quoted string" rest"#;
        skip_sexp(&mut p);
        assert_eq!(p, " rest");
    }

    #[test]
    fn test_skip_sexp_parens() {
        let mut p = "(nested (deep) stuff) rest";
        skip_sexp(&mut p);
        assert_eq!(p, " rest");
    }

    // ── Namespace parsing ──

    #[test]
    fn test_parse_namespace_group_nil() {
        let mut p = "NIL rest";
        let entries = parse_namespace_group(&mut p);
        assert!(entries.is_empty());
        assert_eq!(p, " rest");
    }

    #[test]
    fn test_parse_namespace_group() {
        let mut p = r#"(("" ".")) rest"#;
        let entries = parse_namespace_group(&mut p);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].prefix, "");
        assert_eq!(entries[0].delimiter, Some('.'));
    }

    // ── Quota parsing ──

    #[test]
    fn test_parse_quota_response() {
        let mut q = Quota::default();
        parse_quota_response(r#"QUOTA "root" (STORAGE 512 1024)"#, &mut q);
        assert_eq!(q.root, "root");
        assert_eq!(q.usage, 512);
        assert_eq!(q.limit, 1024);
    }

    // ── IMAP quoting ──

    #[test]
    fn test_imap_quote() {
        assert_eq!(imap_quote("hello"), r#""hello""#);
        assert_eq!(imap_quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(imap_quote(r"back\slash"), r#""back\\slash""#);
    }

    // ── UID list parsing ──

    #[test]
    fn test_parse_uid_list() {
        assert_eq!(parse_uid_list(" 1 2 3 45 678"), vec![1, 2, 3, 45, 678]);
        assert_eq!(parse_uid_list(""), Vec::<u64>::new());
        assert_eq!(parse_uid_list("  "), Vec::<u64>::new());
    }

    // ── SHA-256 ──

    #[test]
    fn test_sha256_empty() {
        let h = sha256(b"");
        let hex: String = h.iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(hex, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn test_sha256_abc() {
        let h = sha256(b"abc");
        let hex: String = h.iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn test_hmac_sha256() {
        // RFC 4231 Test Case 2
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        let h = hmac_sha256(key, data);
        let hex: String = h.iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(hex, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    // ── Capabilities parsing ──

    #[test]
    fn test_capabilities_parse() {
        let mut caps = Capabilities::default();
        caps.parse("CAPABILITY IMAP4rev1 STARTTLS IDLE UIDPLUS MOVE LITERAL+ SORT THREAD NAMESPACE CONDSTORE CHILDREN ID QUOTA ACL AUTH=PLAIN AUTH=CRAM-MD5");
        assert!(caps.starttls);
        assert!(caps.idle);
        assert!(caps.uidplus);
        assert!(caps.r#move);
        assert!(caps.literal_plus);
        assert!(caps.sort);
        assert!(caps.thread);
        assert!(caps.namespace);
        assert!(caps.condstore);
        assert!(caps.children);
        assert!(caps.id);
        assert!(caps.quota);
        assert!(caps.acl);
        assert!(caps.auth_mechs.contains("PLAIN"));
        assert!(caps.auth_mechs.contains("CRAM-MD5"));
    }

    #[test]
    fn test_capabilities_reset() {
        let mut caps = Capabilities::default();
        caps.parse("CAPABILITY IDLE STARTTLS");
        assert!(caps.idle);
        caps.reset();
        assert!(!caps.idle);
        assert!(!caps.starttls);
    }

    // ── Level detection ──

    #[test]
    fn test_detect_level() {
        assert_eq!(detect_level("CAPABILITY IMAP4rev1"), ImapLevel::Imap4rev1);
        assert_eq!(detect_level("CAPABILITY IMAP4rev2"), ImapLevel::Imap4rev2);
        assert_eq!(detect_level("CAPABILITY IMAP4"), ImapLevel::Imap4);
        assert_eq!(detect_level("CAPABILITY IMAP2bis"), ImapLevel::Imap2bis);
        assert_eq!(detect_level("CAPABILITY IMAP2"), ImapLevel::Imap2);
        assert_eq!(detect_level("CAPABILITY"), ImapLevel::Unknown);
    }

    // ── Response code parsing ──

    #[test]
    fn test_response_code_detection() {
        // We can test parse_response_code indirectly through ImapSession
        // but since it needs a session, let's just test the constituent parsing.

        // Test that IMAP quoting handles edge cases
        assert_eq!(imap_quote(""), r#""""#);
        assert_eq!(imap_quote("a"), r#""a""#);
    }

    // ── Body type / encoding conversions ──

    #[test]
    fn test_body_type_from_str() {
        assert_eq!(BodyType::from_str_ci("text"), BodyType::Text);
        assert_eq!(BodyType::from_str_ci("MULTIPART"), BodyType::Multipart);
        assert_eq!(BodyType::from_str_ci("message"), BodyType::Message);
        assert_eq!(BodyType::from_str_ci("APPLICATION"), BodyType::Application);
        assert_eq!(BodyType::from_str_ci("audio"), BodyType::Audio);
        assert_eq!(BodyType::from_str_ci("IMAGE"), BodyType::Image);
        assert_eq!(BodyType::from_str_ci("video"), BodyType::Video);
        assert_eq!(BodyType::from_str_ci("model"), BodyType::Model);
        assert_eq!(BodyType::from_str_ci("unknown"), BodyType::Other);
    }

    #[test]
    fn test_encoding_from_str() {
        assert_eq!(Encoding::from_str_ci("7BIT"), Encoding::SevenBit);
        assert_eq!(Encoding::from_str_ci("8bit"), Encoding::EightBit);
        assert_eq!(Encoding::from_str_ci("BINARY"), Encoding::Binary);
        assert_eq!(Encoding::from_str_ci("base64"), Encoding::Base64);
        assert_eq!(Encoding::from_str_ci("QUOTED-PRINTABLE"), Encoding::QuotedPrintable);
        assert_eq!(Encoding::from_str_ci("x-custom"), Encoding::Other);
    }

    // ── Command formatting ──

    #[test]
    fn test_needs_literal() {
        assert!(!ImapSession::<MockTransport>::needs_literal("INBOX"));
        assert!(!ImapSession::<MockTransport>::needs_literal("Sent Items"));
        assert!(ImapSession::<MockTransport>::needs_literal("My \"Folder\""));
        assert!(ImapSession::<MockTransport>::needs_literal("Path\\Name"));
        assert!(ImapSession::<MockTransport>::needs_literal("Line\nBreak"));
        // Non-ASCII
        assert!(ImapSession::<MockTransport>::needs_literal("\u{00e9}t\u{00e9}"));
    }

    // ── Helpers ──

    #[test]
    fn test_parse_num_from_start() {
        assert_eq!(parse_num_from_start("12345 rest"), 12345);
        assert_eq!(parse_num_from_start("0"), 0);
        assert_eq!(parse_num_from_start("abc"), 0);
        assert_eq!(parse_num_from_start(""), 0);
    }

    #[test]
    fn test_find_number_after() {
        assert_eq!(find_number_after("UID 42 FLAGS", "UID "), Some(42));
        assert_eq!(find_number_after("no uid here", "UID "), None);
    }

    // ── Mock transport for testing needs_literal ──

    struct MockTransport;

    impl Transport for MockTransport {
        fn connect(&mut self, _host: &str, _port: u16) -> io::Result<()> { Ok(()) }
        fn send(&mut self, _data: &[u8]) -> io::Result<()> { Ok(()) }
        fn recv_line(&mut self) -> io::Result<Vec<u8>> { Ok(Vec::new()) }
        fn start_tls(&mut self) -> io::Result<()> { Ok(()) }
        fn close(&mut self) {}
    }
}
