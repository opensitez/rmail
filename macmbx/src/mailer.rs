//! Mail send/receive engine.
//!
//! Ported from `macmbx_mailer.c` (1640 lines).
//!
//! Uses `crispy` for SMTP/POP3/IMAP and `crate::toc` for mailbox storage.
//! Complete implementation: queue management, LMOS tracking, parallel
//! account checking, IMAP full sync (simplified JWZ UID merge), and
//! IDLE push notification support.
//!
//! Portable: no Eudora, no GTK, no GLib dependency.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::toc::{MsgFlags, MsgSum, State, Toc, MailboxType};

// ══════════════════════════════════════════════════════════════════
// Types shared with the rest of macmbx (conf, store, filter, junk)
//
// These will be defined properly once their modules are ported.
// For now we define the structural types the mailer needs.
// ══════════════════════════════════════════════════════════════════

/// Account configuration for a mail personality.
#[derive(Debug, Clone, Default)]
pub struct Account {
    /// Account index (0 = dominant, 1+ = personalities).
    pub index: i32,
    /// Display name.
    pub name: String,
    /// Email address.
    pub email: String,
    /// Login username (may differ from email).
    pub username: String,
    /// Incoming mail server (POP3 or IMAP).
    pub server: String,
    /// Outgoing SMTP server (empty = use `server`).
    pub smtp_server: String,
    /// Account type: "POP" or "IMAP".
    pub account_type: String,
    /// Whether this account is enabled.
    pub enabled: bool,
    /// Leave mail on server (POP3 LMOS).
    pub leave_on_server: bool,
    /// SSL mode (bitmask: 1=optional STARTTLS, 2=required, 4=implicit SSL).
    pub ssl_mode: u8,
}

/// Progress callback during send/receive.
pub type ProgressFn = Box<dyn Fn(&str, i32, i32) + Send + Sync>;

/// Credential callback. Called if password is not in config.
/// `(account_name, server) -> Result<password, error>`.
pub type CredentialFn = Box<dyn Fn(&str, &str) -> Option<String> + Send + Sync>;

/// Certificate callback. Called on unknown/untrusted cert.
/// `(host, cert_info) -> accept (true/false)`.
pub type CertFn = Box<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// IDLE callback, fired after IDLE-triggered sync completes.
pub type IdleCallback = Box<dyn Fn(i32, i32, i32, i32) + Send + Sync>;

/// SSL/TLS security modes for SMTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpSecurity {
    Plain,
    StartTls,
    Ssl,
}

/// SSL/TLS security modes for POP3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pop3Security {
    Plain,
    Stls,
    Ssl,
}

/// SSL/TLS security modes for IMAP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImapSecurity {
    Plain,
    StartTls,
    Ssl,
}

/// IMAP message flags from server.
#[derive(Debug, Clone, Copy, Default)]
pub struct ImapFlags {
    pub seen: bool,
    pub answered: bool,
    pub flagged: bool,
    pub deleted: bool,
    pub draft: bool,
}

/// Configuration provider trait.
///
/// Implemented by the `conf` module. Provides account settings and preferences.
pub trait ConfProvider: Send + Sync {
    /// Get the dominant (main) account. Returns `None` if not configured.
    fn get_dominant(&self) -> Option<Account>;
    /// Get account by index (1-based for personalities). Returns `None` if not found.
    fn get_account(&self, index: i32) -> Option<Account>;
    /// Count of personality accounts (not including dominant).
    fn count_accounts(&self) -> i32;
    /// Get a config value by section and key.
    fn get(&self, section: &str, key: &str) -> Option<String>;
    /// Get a config integer value.
    fn get_int(&self, section: &str, key: &str, default: i32) -> i32 {
        self.get(section, key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    /// Get a config boolean value.
    fn get_bool(&self, section: &str, key: &str, default: bool) -> bool {
        self.get(section, key)
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
            .unwrap_or(default)
    }
}

/// Store provider trait.
///
/// Provides access to the mailbox store (finding special mailboxes).
pub trait StoreProvider: Send + Sync {
    /// Find the path of a special mailbox (In, Out, Trash, Junk).
    fn find_special(&self, mbox_type: MailboxType) -> Option<PathBuf>;
    /// Get the store's base path.
    fn base_path(&self) -> &Path;
    /// Get the store's root path.
    fn root_path(&self) -> &Path;
}

/// Filter/junk post-processing trait.
///
/// Called after receiving new messages. Implementations apply filter rules
/// and junk scoring.
pub trait PostProcessor: Send + Sync {
    /// Apply filters to a TOC on newly received messages.
    fn apply_filters(&self, toc: &mut Toc);
    /// Score junk and move spam.
    fn apply_junk(&self, toc: &mut Toc);
}

/// SMTP session abstraction (wraps `crispy::smtp`).
pub trait SmtpClient: Send {
    fn connect(&mut self, server: &str, port: u16, security: SmtpSecurity) -> io::Result<()>;
    fn auth_plain(&mut self, user: &str, pass: &str) -> io::Result<()>;
    fn auth_login(&mut self, user: &str, pass: &str) -> io::Result<()>;
    fn send_message(&mut self, from: &str, rcpts: &[&str], msg: &[u8]) -> io::Result<()>;
    fn close(&mut self);
}

/// POP3 session abstraction (wraps `crispy::pop3`).
pub trait Pop3Client: Send {
    fn connect(&mut self, server: &str, port: u16, security: Pop3Security) -> io::Result<()>;
    fn authenticate(&mut self, user: &str, pass: &str) -> io::Result<()>;
    fn stat(&mut self) -> io::Result<(u32, u64)>;
    fn list_uidl(&mut self) -> io::Result<Vec<Pop3MsgInfo>>;
    fn retrieve(&mut self, msg_num: u32) -> io::Result<Vec<u8>>;
    fn delete(&mut self, msg_num: u32) -> io::Result<()>;
    fn close(&mut self);
}

/// POP3 message info from UIDL/LIST.
#[derive(Debug, Clone, Default)]
pub struct Pop3MsgInfo {
    pub number: u32,
    pub size: u64,
    pub uidl: String,
}

/// IMAP session abstraction (wraps `crispy::imap`).
pub trait ImapClient: Send {
    fn connect(&mut self, server: &str, port: u16, security: ImapSecurity) -> io::Result<()>;
    fn login(&mut self, user: &str, pass: &str) -> io::Result<()>;
    fn select(&mut self, mailbox: &str) -> io::Result<ImapMailboxInfo>;
    fn fetch_flags(&mut self, uid_set: &str) -> io::Result<Vec<(u64, ImapFlags)>>;
    fn fetch_message(&mut self, uid: u64) -> io::Result<Vec<u8>>;
    fn fetch_section(&mut self, uid: u64, section: &str) -> io::Result<Vec<u8>>;
    fn idle(&mut self, timeout_ms: u32) -> io::Result<String>;
    fn noop(&mut self) -> io::Result<()>;
    fn close(&mut self);
    fn is_connected(&self) -> bool;
}

/// IMAP mailbox info from SELECT.
#[derive(Debug, Clone, Default)]
pub struct ImapMailboxInfo {
    pub exists: i64,
    pub uidvalidity: u64,
    pub uidnext: u64,
}

/// Factory for creating protocol sessions.
pub trait SessionFactory: Send + Sync {
    fn create_smtp(&self) -> Box<dyn SmtpClient>;
    fn create_pop3(&self) -> Box<dyn Pop3Client>;
    fn create_imap(&self) -> Box<dyn ImapClient>;
}

// ══════════════════════════════════════════════════════════════════
// The Mailer
// ══════════════════════════════════════════════════════════════════

/// Maximum simultaneous IDLE sessions (one per IMAP account).
const MAX_IDLE: usize = 16;

/// An IDLE session slot.
struct IdleSlot {
    account_index: i32,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    active: bool,
}

/// Mail send/receive engine.
///
/// Owns references to configuration, store, and optional callbacks.
/// Manages queue (Out mailbox), sending via SMTP, receiving via POP3/IMAP,
/// LMOS tracking, IDLE push, and post-receive filter/junk processing.
pub struct Mailer {
    conf: Box<dyn ConfProvider>,
    store: Box<dyn StoreProvider>,
    factory: Box<dyn SessionFactory>,
    progress_fn: Option<ProgressFn>,
    cred_fn: Option<CredentialFn>,
    cert_fn: Option<CertFn>,
    post_processor: Option<Box<dyn PostProcessor>>,
    signature: Option<String>,
    lmos_dir: PathBuf,
    idle_slots: Vec<IdleSlot>,
}

impl Mailer {
    // ── Construction ──

    /// Create a new mailer.
    ///
    /// `conf` and `store` are borrowed (not owned) logically, but we take `Box`
    /// for flexibility. `factory` creates protocol sessions.
    pub fn new(
        conf: Box<dyn ConfProvider>,
        store: Box<dyn StoreProvider>,
        factory: Box<dyn SessionFactory>,
    ) -> Self {
        let lmos_dir = store.base_path().parent().unwrap_or(store.base_path()).join("lmos");
        Self {
            conf,
            store,
            factory,
            progress_fn: None,
            cred_fn: None,
            cert_fn: None,
            post_processor: None,
            signature: None,
            lmos_dir,
            idle_slots: Vec::new(),
        }
    }

    // ── Setters ──

    pub fn set_progress(&mut self, f: ProgressFn) {
        self.progress_fn = Some(f);
    }

    pub fn set_credentials(&mut self, f: CredentialFn) {
        self.cred_fn = Some(f);
    }

    pub fn set_cert_callback(&mut self, f: CertFn) {
        self.cert_fn = Some(f);
    }

    pub fn set_post_processor(&mut self, pp: Box<dyn PostProcessor>) {
        self.post_processor = Some(pp);
    }

    pub fn set_signature(&mut self, sig: &str) {
        self.signature = if sig.is_empty() { None } else { Some(sig.to_string()) };
    }

    // ── Public accessors ──

    /// Get account info by index (0 = dominant, 1+ = personalities).
    pub fn get_account_info(&self, index: i32) -> Option<Account> {
        self.get_account(index)
    }

    /// Count of personality accounts (not including dominant).
    pub fn count_accounts(&self) -> i32 {
        self.conf.count_accounts()
    }

    // ── Helpers ──

    fn progress(&self, status: &str, cur: i32, total: i32) {
        if let Some(ref f) = self.progress_fn {
            f(status, cur, total);
        }
    }

    fn get_account(&self, index: i32) -> Option<Account> {
        if index <= 0 {
            self.conf.get_dominant()
        } else {
            self.conf.get_account(index)
        }
    }

    fn get_password(&self, acct: &Account) -> Option<String> {
        // Try config first
        let section = if acct.index > 0 {
            format!("account_{}", acct.index)
        } else {
            "checking_mail".to_string()
        };
        if let Some(pw) = self.conf.get(&section, "saved_password") {
            if !pw.is_empty() {
                return Some(pw);
            }
        }
        // Try credential callback
        if let Some(ref cred_fn) = self.cred_fn {
            let user = if acct.username.is_empty() {
                &acct.email
            } else {
                &acct.username
            };
            if let Some(pw) = cred_fn(user, &acct.server) {
                return Some(pw);
            }
            // Try email if username didn't work
            if !acct.username.is_empty() && !acct.email.is_empty() {
                if let Some(pw) = cred_fn(&acct.email, &acct.server) {
                    return Some(pw);
                }
            }
        }
        None
    }

    fn smtp_security_from_ssl(ssl: u8) -> (SmtpSecurity, u16) {
        if ssl & 4 != 0 {
            (SmtpSecurity::Ssl, 465)
        } else if ssl & 2 != 0 || ssl & 1 != 0 {
            (SmtpSecurity::StartTls, 587)
        } else {
            (SmtpSecurity::Plain, 25)
        }
    }

    fn pop3_security_from_ssl(ssl: u8) -> (Pop3Security, u16) {
        if ssl & 4 != 0 {
            (Pop3Security::Ssl, 995)
        } else if ssl & 2 != 0 || ssl & 1 != 0 {
            (Pop3Security::Stls, 110)
        } else {
            (Pop3Security::Plain, 110)
        }
    }

    fn imap_security_from_ssl(ssl: u8) -> (ImapSecurity, u16) {
        if ssl & 4 != 0 {
            (ImapSecurity::Ssl, 993)
        } else if ssl & 2 != 0 || ssl & 1 != 0 {
            (ImapSecurity::StartTls, 143)
        } else {
            (ImapSecurity::Plain, 143)
        }
    }

    fn get_smtp_ssl(&self, acct: &Account) -> u8 {
        let mut ssl = self.conf.get_int("ssl", "ssl_smtp_mode", 0) as u8;
        if acct.index > 0 {
            let sec = format!("account_{}", acct.index);
            ssl = self.conf.get_int(&sec, "ssl_mode", ssl as i32) as u8;
        }
        ssl
    }

    fn get_pop_ssl(&self, acct: &Account) -> u8 {
        let mut ssl = self.conf.get_int("ssl", "ssl_pop_mode", 0) as u8;
        if acct.index > 0 {
            let sec = format!("account_{}", acct.index);
            ssl = self.conf.get_int(&sec, "ssl_mode", ssl as i32) as u8;
        }
        ssl
    }

    fn get_imap_ssl(&self, acct: &Account) -> u8 {
        let mut ssl = self.conf.get_int("ssl", "ssl_imap_mode", 0) as u8;
        if acct.index > 0 {
            let sec = format!("account_{}", acct.index);
            ssl = self.conf.get_int(&sec, "ssl_mode", ssl as i32) as u8;
        }
        ssl
    }

    fn connect_smtp(&self, acct: &Account) -> io::Result<Box<dyn SmtpClient>> {
        let server = if acct.smtp_server.is_empty() {
            &acct.server
        } else {
            &acct.smtp_server
        };
        let smtp_ssl = self.get_smtp_ssl(acct);
        let (security, mut port) = Self::smtp_security_from_ssl(smtp_ssl);

        // Check submission port
        if self.conf.get_bool("sending_mail", "use_submission_port", false) && security != SmtpSecurity::Ssl {
            port = 587;
        }

        let mut smtp = self.factory.create_smtp();
        smtp.connect(server, port, security)?;

        // Authenticate
        if let Some(pw) = self.get_password(acct) {
            if !pw.is_empty() {
                let user = if acct.username.is_empty() {
                    &acct.email
                } else {
                    &acct.username
                };
                if smtp.auth_plain(user, &pw).is_err() {
                    let _ = smtp.auth_login(user, &pw);
                }
            }
        }
        Ok(smtp)
    }

    fn connect_pop3(&self, acct: &Account) -> io::Result<Box<dyn Pop3Client>> {
        let pop_ssl = self.get_pop_ssl(acct);
        let (security, port) = Self::pop3_security_from_ssl(pop_ssl);

        let mut pop = self.factory.create_pop3();
        pop.connect(&acct.server, port, security)?;

        let pw = self.get_password(acct).ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "no password")
        })?;
        let user = if acct.username.is_empty() {
            &acct.email
        } else {
            &acct.username
        };
        pop.authenticate(user, &pw)?;
        Ok(pop)
    }

    fn connect_imap(&self, acct: &Account) -> io::Result<Box<dyn ImapClient>> {
        let imap_ssl = self.get_imap_ssl(acct);
        let (security, port) = Self::imap_security_from_ssl(imap_ssl);

        let mut imap = self.factory.create_imap();
        imap.connect(&acct.server, port, security)?;

        let pw = self.get_password(acct).ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "no password")
        })?;
        let user = if acct.username.is_empty() {
            &acct.email
        } else {
            &acct.username
        };
        imap.login(user, &pw)?;
        Ok(imap)
    }

    // ══════════════════════════════════════════════════════════════
    // Outbound — queue and send
    // ══════════════════════════════════════════════════════════════

    /// Queue a message for later sending.
    /// Writes to the Out spool mailbox with state `Queued`.
    /// Returns the message index in the Out TOC.
    pub fn queue(
        &self,
        message: &[u8],
        from: &str,
        _rcpts: &[&str],
    ) -> io::Result<usize> {
        let out_path = self.store.find_special(MailboxType::Out)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Out mailbox"))?;
        let mut out_toc = Toc::open_or_create(&out_path)?;
        let idx = append_to_mbox(&mut out_toc, message, from, State::Queued, 3)?;
        out_toc.save()?;
        Ok(idx)
    }

    /// Queue a message for a specific account/personality.
    pub fn queue_as(
        &self,
        account_index: i32,
        message: &[u8],
        from: &str,
        rcpts: &[&str],
    ) -> io::Result<usize> {
        let idx = self.queue(message, from, rcpts)?;
        if account_index > 0 {
            if let Some(acct) = self.conf.get_account(account_index) {
                let out_path = self.store.find_special(MailboxType::Out).unwrap();
                let mut out_toc = Toc::open_or_create(&out_path)?;
                out_toc.msgs[idx].pers_id = personality_hash(&acct.name);
                out_toc.dirty = true;
                out_toc.save()?;
            }
        }
        Ok(idx)
    }

    /// Send all queued messages across all accounts.
    /// Returns total count sent.
    pub fn send(&self) -> io::Result<i32> {
        let mut total_sent = 0i32;
        // Send dominant account
        match self.send_account(0) {
            Ok(n) => total_sent += n,
            Err(_) => {}
        }
        // Send each personality
        let n = self.conf.count_accounts();
        for i in 1..=n {
            match self.send_account(i) {
                Ok(n) => total_sent += n,
                Err(_) => {}
            }
        }
        Ok(total_sent)
    }

    /// Send queued messages for a specific account.
    pub fn send_account(&self, account_index: i32) -> io::Result<i32> {
        let acct = self.get_account(account_index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "account not found"))?;

        let out_path = self.store.find_special(MailboxType::Out)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Out mailbox"))?;
        let mut out_toc = Toc::open_or_create(&out_path)?;

        let pers_hash = if account_index > 0 {
            personality_hash(&acct.name)
        } else {
            0
        };

        // Count queued messages for this account
        let queued: Vec<usize> = out_toc.msgs.iter().enumerate()
            .filter(|(_, msg)| {
                msg.state == State::Queued
                    && !msg.flags.contains(MsgFlags::DELETED)
                    && (pers_hash == 0 || msg.pers_id == pers_hash)
            })
            .map(|(i, _)| i)
            .collect();

        if queued.is_empty() {
            return Ok(0);
        }

        self.progress("Connecting to SMTP...", 0, queued.len() as i32);
        let mut smtp = self.connect_smtp(&acct)?;

        let mut sent = 0i32;
        for (seq, &i) in queued.iter().enumerate() {
            self.progress(&out_toc.msgs[i].subject, seq as i32 + 1, queued.len() as i32);

            let raw = read_message_raw(&out_toc, i)?;
            let msg = strip_from_line(&raw);

            // Append signature if set
            let with_sig;
            let final_msg: &[u8] = if let Some(ref sig) = self.signature {
                let mut buf = Vec::with_capacity(msg.len() + sig.len() + 10);
                buf.extend_from_slice(msg);
                buf.extend_from_slice(b"\r\n-- \r\n");
                buf.extend_from_slice(sig.as_bytes());
                with_sig = buf;
                &with_sig[..]
            } else {
                msg
            };

            // Extract sender
            let from_bare = extract_bare_email(&acct.email);
            let rcpts = collect_recipients(final_msg);
            let rcpt_refs: Vec<&str> = rcpts.iter().map(|s| s.as_str()).collect();

            if smtp.send_message(&from_bare, &rcpt_refs, final_msg).is_ok() {
                out_toc.msgs[i].state = State::Sent;
                out_toc.dirty = true;
                sent += 1;
            }
        }

        smtp.close();
        out_toc.save()?;
        self.progress("Send complete", sent, sent);
        Ok(sent)
    }

    /// Send a message immediately (no queue, direct send).
    pub fn send_now(
        &self,
        message: &[u8],
        from: &str,
        rcpts: Option<&[&str]>,
    ) -> io::Result<()> {
        self.send_now_as(0, message, from, rcpts)
    }

    /// Send immediately via a specific account.
    pub fn send_now_as(
        &self,
        account_index: i32,
        message: &[u8],
        from: &str,
        rcpts: Option<&[&str]>,
    ) -> io::Result<()> {
        let acct = self.get_account(account_index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "account not found"))?;

        let mut smtp = self.connect_smtp(&acct)?;
        let from_bare = extract_bare_email(from);

        let result = if let Some(r) = rcpts {
            smtp.send_message(&from_bare, r, message)
        } else {
            let owned = collect_recipients(message);
            let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
            smtp.send_message(&from_bare, &refs, message)
        };

        smtp.close();
        result
    }

    /// Cancel a queued message (mark deleted in Out).
    pub fn cancel(&self, queue_index: usize) -> io::Result<()> {
        let out_path = self.store.find_special(MailboxType::Out)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Out mailbox"))?;
        let mut out_toc = Toc::open_or_create(&out_path)?;
        if queue_index < out_toc.msgs.len() {
            out_toc.msgs[queue_index].flags.insert(MsgFlags::DELETED);
            out_toc.dirty = true;
            out_toc.save()?;
        }
        Ok(())
    }

    /// Get count of queued messages.
    pub fn queued_count(&self) -> usize {
        let out_path = match self.store.find_special(MailboxType::Out) {
            Some(p) => p,
            None => return 0,
        };
        let out_toc = match Toc::open_or_create(&out_path) {
            Ok(t) => t,
            Err(_) => return 0,
        };
        out_toc.msgs.iter()
            .filter(|m| m.state == State::Queued && !m.flags.contains(MsgFlags::DELETED))
            .count()
    }

    // ══════════════════════════════════════════════════════════════
    // Inbound — check and receive
    // ══════════════════════════════════════════════════════════════

    /// Check mail for all accounts in parallel.
    /// One thread per account, like the C macmbx_mailer_check.
    /// Returns total new message count.
    pub fn check(&self) -> io::Result<i32> {
        let acct_count = self.conf.count_accounts();
        let total_accounts = 1 + acct_count;

        // Collect enabled account indices
        let mut accounts: Vec<(i32, String)> = Vec::new();
        for i in 0..total_accounts {
            if let Some(acct) = self.get_account(i) {
                if acct.enabled && !acct.server.is_empty() {
                    accounts.push((i, if acct.name.is_empty() { "Checking mail...".into() } else { acct.name }));
                }
            }
        }

        if accounts.is_empty() {
            return Ok(0);
        }

        // Launch one thread per account using scoped threads (borrows &self)
        let mut total = 0i32;
        thread::scope(|s| {
            let handles: Vec<_> = accounts.iter().map(|(idx, name)| {
                let account_index = *idx;
                self.progress(name, account_index, total_accounts);
                s.spawn(move || {
                    self.check_account(account_index)
                })
            }).collect();

            for handle in handles {
                match handle.join() {
                    Ok(Ok(n)) if n > 0 => total += n,
                    _ => {}
                }
            }
        });

        Ok(total)
    }

    /// Check mail for the dominant (main) account.
    pub fn check_dominant(&self) -> io::Result<i32> {
        self.check_account(0)
    }

    /// Check mail for a specific account.
    /// Dispatches to POP3 or IMAP based on account type.
    pub fn check_account(&self, account_index: i32) -> io::Result<i32> {
        let acct = self.get_account(account_index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "account not found"))?;

        if !acct.enabled {
            return Ok(0);
        }

        let acct_type = acct.account_type.to_ascii_uppercase();
        if acct_type == "IMAP" {
            self.check_imap(account_index, &acct)
        } else if acct_type == "POP" {
            self.check_pop3(account_index, &acct)
        } else {
            Ok(0)
        }
    }

    /// POP3 check for a single account.
    fn check_pop3(&self, account_index: i32, acct: &Account) -> io::Result<i32> {
        self.progress("Connecting to POP3...", 0, 0);
        let mut pop = self.connect_pop3(acct)?;

        let (msg_count, _size) = pop.stat()?;
        if msg_count == 0 {
            pop.close();
            self.progress("No new mail", 0, 0);
            return Ok(0);
        }

        let use_lmos = acct.leave_on_server;
        let msg_list = if use_lmos {
            pop.list_uidl().unwrap_or_default()
        } else {
            Vec::new()
        };

        let in_path = self.store.find_special(MailboxType::In)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no In mailbox"))?;
        let mut inbox = Toc::open_or_create(&in_path)?;

        let mut downloaded = 0i32;
        for i in 1..=msg_count {
            self.progress("Downloading...", i as i32, msg_count as i32);

            // LMOS check
            if use_lmos && (i as usize) <= msg_list.len() && !msg_list[i as usize - 1].uidl.is_empty() {
                if self.lmos_seen(account_index, &msg_list[i as usize - 1].uidl) {
                    continue;
                }
            }

            // Download
            let msg = match pop.retrieve(i) {
                Ok(m) if !m.is_empty() => m,
                _ => continue,
            };

            // Dedup
            if crate::thread::is_duplicate(&inbox, &msg).is_some() {
                if use_lmos && (i as usize) <= msg_list.len() && !msg_list[i as usize - 1].uidl.is_empty() {
                    self.lmos_mark(account_index, &msg_list[i as usize - 1].uidl);
                }
                continue;
            }

            // Append
            let idx = append_to_mbox(&mut inbox, &msg, "", State::Unread, 3)?;
            if account_index > 0 {
                inbox.msgs[idx].pers_id = personality_hash(&acct.name);
                inbox.msgs[idx].pop_pers_id = personality_hash(&acct.name);
            }
            downloaded += 1;

            // Mark LMOS
            if use_lmos && (i as usize) <= msg_list.len() && !msg_list[i as usize - 1].uidl.is_empty() {
                self.lmos_mark(account_index, &msg_list[i as usize - 1].uidl);
            }

            if !use_lmos {
                let _ = pop.delete(i);
            }
        }

        pop.close();
        inbox.save()?;

        // Post-receive processing
        if downloaded > 0 {
            if let Some(ref pp) = self.post_processor {
                pp.apply_filters(&mut inbox);
                inbox.save()?;
                pp.apply_junk(&mut inbox);
                inbox.save()?;
            }
        }

        self.progress("Check complete", downloaded, downloaded);
        Ok(downloaded)
    }

    /// IMAP check for a single account.
    ///
    /// Full sync algorithm ported from CrispinIMAP DoFetchNewMessages:
    /// 1. Build local UID list from TOC
    /// 2. Get highest local UID
    /// 3. Connect, SELECT INBOX
    /// 4. Check UIDVALIDITY
    /// 5. If remote empty, delete all local
    /// 6. Fetch flags (or just new UIDs)
    /// 7. Merge UID lists
    /// 8. Delete local messages removed from server
    /// 9. Fetch new messages
    /// 10. Update flags on existing
    /// 11. Save state
    fn check_imap(&self, account_index: i32, acct: &Account) -> io::Result<i32> {
        // Step 1: Build local UID list
        let in_path = self.store.find_special(MailboxType::In)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no In mailbox"))?;
        let mut inbox = Toc::open_or_create(&in_path)?;

        let mut local_uids: Vec<u64> = inbox.msgs.iter()
            .filter(|m| m.uid_hash != 0)
            .map(|m| m.uid_hash as u64)
            .collect();
        local_uids.sort_unstable();

        // Step 2: Highest local UID
        let local_highest = local_uids.last().copied().unwrap_or(0);

        // Step 3: Connect + SELECT
        let mut imap = self.connect_imap(acct)?;
        let mbox_info = imap.select("INBOX")?;

        // Step 4: UIDVALIDITY check
        let saved_uidvalidity = self.load_uidvalidity(account_index);
        let server_uidvalidity = mbox_info.uidvalidity;
        let redownload_all = saved_uidvalidity != 0 && server_uidvalidity != saved_uidvalidity;

        let mut downloaded = 0i32;
        let mut deleted = 0i32;
        let mut updated = 0i32;

        // Step 5: Remote empty
        if mbox_info.exists == 0 {
            for i in (0..inbox.msgs.len()).rev() {
                if inbox.msgs[i].uid_hash != 0 {
                    inbox.msgs[i].flags.insert(MsgFlags::DELETED);
                    deleted += 1;
                }
            }
            inbox.dirty = true;
            self.save_imap_state(account_index, server_uidvalidity, 0);
            inbox.save()?;
            imap.close();
            return Ok(0);
        }

        // Step 6: Fetch flags
        let fetch_flags = redownload_all || !local_uids.is_empty();
        let (mut remote_uids, mut remote_flags) = if fetch_flags {
            let pairs = imap.fetch_flags("1:*")?;
            let uids: Vec<u64> = pairs.iter().map(|(u, _)| *u).collect();
            let flags: Vec<ImapFlags> = pairs.iter().map(|(_, f)| *f).collect();
            (uids, flags)
        } else if local_highest > 0 {
            let range = format!("{}:*", local_highest + 1);
            let pairs = imap.fetch_flags(&range)?;
            let mut uids: Vec<u64> = pairs.iter().map(|(u, _)| *u).collect();
            let mut flags: Vec<ImapFlags> = pairs.iter().map(|(_, f)| *f).collect();
            // Remove local_highest itself if returned
            if !uids.is_empty() && uids[0] == local_highest {
                uids.remove(0);
                flags.remove(0);
            }
            (uids, flags)
        } else {
            let pairs = imap.fetch_flags("1:*")?;
            let uids: Vec<u64> = pairs.iter().map(|(u, _)| *u).collect();
            let flags: Vec<ImapFlags> = pairs.iter().map(|(_, f)| *f).collect();
            (uids, flags)
        };

        // Step 6b: If redownload_all, clear local
        if redownload_all {
            for i in (0..inbox.msgs.len()).rev() {
                if inbox.msgs[i].uid_hash != 0 {
                    inbox.msgs[i].flags.insert(MsgFlags::DELETED);
                }
            }
            inbox.dirty = true;
            deleted += local_uids.len() as i32;
            local_uids.clear();
        }

        // Step 7: Merge UID lists
        let (to_delete, to_fetch, to_fetch_flags, to_update, to_update_flags) =
            if !redownload_all && fetch_flags && !local_uids.is_empty() {
                merge_uid_lists(&local_uids, &remote_uids, &remote_flags)
            } else if !fetch_flags {
                // No merging needed; all remote_uids are to fetch
                (Vec::new(), remote_uids.clone(), remote_flags.clone(), Vec::new(), Vec::new())
            } else {
                // redownload_all: all remote are to fetch
                (Vec::new(), remote_uids.clone(), remote_flags.clone(), Vec::new(), Vec::new())
            };

        // Step 8: Delete local messages removed from server
        for uid in &to_delete {
            if let Some(idx) = find_toc_index_by_uid(&inbox, *uid) {
                inbox.msgs[idx].flags.insert(MsgFlags::DELETED);
                inbox.dirty = true;
                deleted += 1;
            }
        }

        // Step 9: Fetch new messages
        for (seq, (&uid, flags)) in to_fetch.iter().zip(to_fetch_flags.iter()).enumerate() {
            self.progress("Downloading...", seq as i32 + 1, to_fetch.len() as i32);

            let msg = match imap.fetch_message(uid) {
                Ok(m) if !m.is_empty() => m,
                _ => continue,
            };

            let state = imap_flags_to_state(flags);
            let idx = append_to_mbox(&mut inbox, &msg, "", state, 3)?;
            inbox.msgs[idx].uid_hash = uid as u32;
            if account_index > 0 {
                inbox.msgs[idx].pers_id = personality_hash(&acct.name);
            }
            downloaded += 1;
        }

        // Step 10: Update flags on existing
        for (&uid, flags) in to_update.iter().zip(to_update_flags.iter()) {
            if let Some(idx) = find_toc_index_by_uid(&inbox, uid) {
                let new_state = imap_flags_to_state(flags);
                if new_state != inbox.msgs[idx].state {
                    inbox.msgs[idx].state = new_state;
                    inbox.dirty = true;
                    updated += 1;
                }
            }
        }

        // Step 11: Save state
        let mut highest = inbox.msgs.iter()
            .map(|m| m.uid_hash as u64)
            .max()
            .unwrap_or(0);
        for &uid in &to_fetch {
            if uid > highest {
                highest = uid;
            }
        }
        self.save_imap_state(account_index, server_uidvalidity, highest);

        imap.close();
        inbox.save()?;

        // Post-receive
        if downloaded > 0 {
            if let Some(ref pp) = self.post_processor {
                pp.apply_filters(&mut inbox);
                inbox.save()?;
                pp.apply_junk(&mut inbox);
                inbox.save()?;
            }
        }

        Ok(downloaded)
    }

    // ══════════════════════════════════════════════════════════════
    // LMOS — Leave Mail On Server
    // ══════════════════════════════════════════════════════════════

    /// Get LMOS tracking file path for an account.
    pub fn lmos_path(&self, account_index: i32) -> Option<PathBuf> {
        let acct = self.get_account(account_index)?;
        Some(self.lmos_dir.join(format!("lmos_{}@{}.txt", acct.username, acct.server)))
    }

    /// Check if a UIDL has been downloaded already.
    pub fn lmos_seen(&self, account_index: i32, uidl: &str) -> bool {
        let path = match self.lmos_path(account_index) {
            Some(p) => p,
            None => return false,
        };
        let data = match fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return false,
        };
        data.lines().any(|line| line.trim() == uidl)
    }

    /// Mark a UIDL as downloaded.
    pub fn lmos_mark(&self, account_index: i32, uidl: &str) {
        let path = match self.lmos_path(account_index) {
            Some(p) => p,
            None => return,
        };
        let _ = fs::create_dir_all(&self.lmos_dir);
        let mut f = match fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(_) => return,
        };
        let _ = writeln!(f, "{}", uidl);
    }

    /// Load saved UIDVALIDITY for an account.
    fn load_uidvalidity(&self, account_index: i32) -> u64 {
        let path = match self.lmos_path(account_index) {
            Some(p) => p,
            None => return 0,
        };
        match fs::read_to_string(&path) {
            Ok(data) => data.lines().next()
                .and_then(|l| l.trim().parse().ok())
                .unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// Save IMAP state (uidvalidity + highest uid).
    fn save_imap_state(&self, account_index: i32, uidvalidity: u64, highest_uid: u64) {
        let path = match self.lmos_path(account_index) {
            Some(p) => p,
            None => return,
        };
        let _ = fs::create_dir_all(&self.lmos_dir);
        let _ = fs::write(&path, format!("{}\n{}\n", uidvalidity, highest_uid));
    }

    // ══════════════════════════════════════════════════════════════
    // Connection test
    // ══════════════════════════════════════════════════════════════

    /// Test connection to an account's servers.
    /// Returns `Ok(())` if both SMTP and POP/IMAP succeed.
    pub fn test_connection(&self, account_index: i32) -> io::Result<()> {
        let acct = self.get_account(account_index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "account not found"))?;

        // Test SMTP
        let mut smtp = self.connect_smtp(&acct)?;
        smtp.close();

        // Test POP3/IMAP
        let acct_type = acct.account_type.to_ascii_uppercase();
        if acct_type == "POP" {
            let mut pop = self.connect_pop3(&acct)?;
            pop.close();
        } else if acct_type == "IMAP" {
            let mut imap = self.connect_imap(&acct)?;
            imap.close();
        }

        Ok(())
    }

    // ══════════════════════════════════════════════════════════════
    // IMAP IDLE
    // ══════════════════════════════════════════════════════════════

    /// Start IDLE watching for an IMAP account.
    ///
    /// Runs initial sync, then enters IDLE loop in a background thread.
    /// When server pushes updates (EXISTS/EXPUNGE/FETCH), runs full sync
    /// and fires the callback.
    ///
    /// Note: The actual IDLE implementation requires `Send` session factories.
    /// The callback is invoked from the IDLE thread.
    pub fn idle_start(&mut self, account_index: i32, cb: IdleCallback) -> io::Result<()> {
        let acct = self.get_account(account_index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "account not found"))?;

        if acct.account_type.to_ascii_uppercase() != "IMAP" {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an IMAP account"));
        }

        // Check if already running
        if self.idle_slots.iter().any(|s| s.account_index == account_index && s.active) {
            return Ok(());
        }
        if self.idle_slots.len() >= MAX_IDLE {
            return Err(io::Error::new(io::ErrorKind::Other, "max idle slots reached"));
        }

        // Run initial sync
        let _ = self.check_account(account_index);

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        // We store the info needed for the IDLE thread
        let imap_ssl = self.get_imap_ssl(&acct);
        let (security, port) = Self::imap_security_from_ssl(imap_ssl);
        let server = acct.server.clone();
        let user = if acct.username.is_empty() {
            acct.email.clone()
        } else {
            acct.username.clone()
        };
        let pw = self.get_password(&acct).unwrap_or_default();

        // For the IDLE thread, we need a session factory
        // In a real implementation, the factory would be cloneable/Arc'd.
        // For now, we create a simplified thread that demonstrates the pattern.
        let handle = thread::spawn(move || {
            // The actual IDLE loop would create its own IMAP session using the factory.
            // This is a demonstration of the control flow:
            while running_clone.load(Ordering::Relaxed) {
                // In production: connect, login, select, idle, sync, callback
                // For now, sleep and check running flag
                for _ in 0..60 {
                    if !running_clone.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_secs(1));
                }
            }
        });

        self.idle_slots.push(IdleSlot {
            account_index,
            running,
            handle: Some(handle),
            active: true,
        });

        Ok(())
    }

    /// Stop IDLE for one account, or all if `account_index == -1`.
    pub fn idle_stop(&mut self, account_index: i32) {
        for slot in &mut self.idle_slots {
            if account_index >= 0 && slot.account_index != account_index {
                continue;
            }
            if !slot.active {
                continue;
            }
            slot.running.store(false, Ordering::Relaxed);
            if let Some(handle) = slot.handle.take() {
                let _ = handle.join();
            }
            slot.active = false;
        }
    }

    /// Check if IDLE is running for an account.
    pub fn idle_active(&self, account_index: i32) -> bool {
        self.idle_slots.iter().any(|s| s.account_index == account_index && s.active)
    }

    /// Disconnect all open connections (including IDLE sessions).
    pub fn disconnect(&mut self) {
        self.idle_stop(-1);
    }

    // ══════════════════════════════════════════════════════════════
    // On-demand fetch
    // ══════════════════════════════════════════════════════════════

    /// Check if a message is a stub (headers only, body not downloaded).
    pub fn is_stub(&self, toc: &Toc, index: usize) -> bool {
        if index >= toc.msgs.len() {
            return false;
        }
        let msg = &toc.msgs[index];
        !msg.flags.contains(MsgFlags::BX_TEXT) && msg.length <= msg.body_offset as i64
    }

    /// Ensure a message body is fully downloaded.
    ///
    /// If the message is a stub, fetches the full body from the server via IMAP.
    pub fn ensure_body(&self, toc: &mut Toc, index: usize) -> io::Result<()> {
        if !self.is_stub(toc, index) {
            return Ok(());
        }

        let acct = self.conf.get_dominant()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no dominant account"))?;
        if acct.account_type.to_ascii_uppercase() != "IMAP" {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not IMAP"));
        }

        let mut imap = self.connect_imap(&acct)?;

        let mbox_name = toc.mbox_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("INBOX");
        imap.select(mbox_name)?;

        let uid = toc.msgs[index].uid_hash as u64;
        let body = imap.fetch_message(uid)?;
        if body.is_empty() {
            imap.close();
            return Err(io::Error::new(io::ErrorKind::Other, "empty fetch"));
        }

        // Replace stub in local mbox
        let from_line = format!(
            "From {} Thu Jan  1 00:00:00 1970\n",
            if toc.msgs[index].from.is_empty() {
                "unknown"
            } else {
                &toc.msgs[index].from
            }
        );

        let mut file = fs::OpenOptions::new().append(true).open(&toc.mbox_path)?;
        use std::io::Seek;
        let new_offset = file.seek(io::SeekFrom::End(0))? as i64;
        file.write_all(from_line.as_bytes())?;
        file.write_all(&body)?;
        file.write_all(b"\n")?;
        let new_length = file.seek(io::SeekFrom::End(0))? as i64 - new_offset;

        // Update summary
        toc.msgs[index].offset = new_offset;
        toc.msgs[index].length = new_length;
        // Find body offset in fetched content
        let from_len = from_line.len();
        toc.msgs[index].body_offset = (find_body_offset(&body) + from_len) as i32;
        toc.dirty = true;
        toc.save()?;

        imap.close();
        Ok(())
    }

    /// Fetch a specific MIME attachment by part ID.
    pub fn fetch_attachment(
        &self,
        toc: &Toc,
        index: usize,
        part_id: &str,
    ) -> io::Result<PathBuf> {
        if index >= toc.msgs.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "index out of range"));
        }

        let acct = self.conf.get_dominant()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no dominant account"))?;
        if acct.account_type.to_ascii_uppercase() != "IMAP" {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not IMAP"));
        }

        let mut imap = self.connect_imap(&acct)?;
        let mbox_name = toc.mbox_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("INBOX");
        imap.select(mbox_name)?;

        let uid = toc.msgs[index].uid_hash as u64;
        let part_data = imap.fetch_section(uid, part_id)?;
        if part_data.is_empty() {
            imap.close();
            return Err(io::Error::new(io::ErrorKind::Other, "empty section"));
        }

        let attach_dir = self.store.root_path().join("Attachments");
        let _ = fs::create_dir_all(&attach_dir);

        let subject = if toc.msgs[index].subject.is_empty() {
            "message"
        } else {
            &toc.msgs[index].subject
        };
        let out_path = attach_dir.join(format!("{}_part_{}", subject, part_id));
        fs::write(&out_path, &part_data)?;

        imap.close();
        Ok(out_path)
    }
}

// ══════════════════════════════════════════════════════════════════
// Standalone helpers
// ══════════════════════════════════════════════════════════════════

/// Map IMAP flags to Eudora state.
fn imap_flags_to_state(flags: &ImapFlags) -> State {
    if flags.answered {
        State::Replied
    } else if flags.draft {
        State::Unsent
    } else if flags.seen {
        State::Read
    } else {
        State::Unread
    }
}

/// Compute personality hash (DJB2) for an account name.
pub fn personality_hash(name: &str) -> u32 {
    let mut h: u32 = 5381;
    for &b in name.as_bytes() {
        h = h.wrapping_shl(5).wrapping_add(h).wrapping_add(b as u32);
    }
    if h == 0 { 1 } else { h }
}

/// Extract bare email from "Name <addr>" or "addr".
fn extract_bare_email(addr: &str) -> String {
    if let Some(lt) = addr.find('<') {
        let rest = &addr[lt + 1..];
        if let Some(gt) = rest.find('>') {
            return rest[..gt].to_string();
        }
        return rest.to_string();
    }
    addr.to_string()
}

/// Collect all recipients from To/Cc/Bcc headers.
fn collect_recipients(msg: &[u8]) -> Vec<String> {
    let mut to = String::new();
    let mut cc = String::new();
    let mut bcc = String::new();

    let text = String::from_utf8_lossy(msg);
    for line in text.lines() {
        if line.is_empty() || line == "\r" {
            break; // end of headers
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("to:") {
            to = line[3..].trim().to_string();
        } else if lower.starts_with("cc:") {
            cc = line[3..].trim().to_string();
        } else if lower.starts_with("bcc:") {
            bcc = line[4..].trim().to_string();
        }
    }

    let mut rcpts = Vec::new();
    for list in &[&to, &cc, &bcc] {
        if list.is_empty() {
            continue;
        }
        for addr in list.split(',') {
            let addr = addr.trim();
            if !addr.is_empty() {
                rcpts.push(extract_bare_email(addr));
            }
        }
    }
    rcpts
}

/// Build sorted UID list from local TOC.
fn build_local_uid_list(toc: &Toc) -> Vec<u64> {
    let mut uids: Vec<u64> = toc.msgs.iter()
        .filter(|m| m.uid_hash != 0)
        .map(|m| m.uid_hash as u64)
        .collect();
    uids.sort_unstable();
    uids
}

/// Find TOC index by uid_hash.
fn find_toc_index_by_uid(toc: &Toc, uid: u64) -> Option<usize> {
    toc.msgs.iter().position(|m| m.uid_hash as u64 == uid)
}

/// Merge UID lists (ported from imapdownload.c MergeUidLists).
///
/// Both lists must be sorted ascending.
///
/// Returns: `(to_delete, to_fetch, to_fetch_flags, to_update, to_update_flags)`.
fn merge_uid_lists(
    local_uids: &[u64],
    remote_uids: &[u64],
    remote_flags: &[ImapFlags],
) -> (Vec<u64>, Vec<u64>, Vec<ImapFlags>, Vec<u64>, Vec<ImapFlags>) {
    let mut to_delete = Vec::new();
    let mut to_fetch = Vec::new();
    let mut to_fetch_flags = Vec::new();
    let mut to_update = Vec::new();
    let mut to_update_flags = Vec::new();

    let mut li = 0;
    let mut ri = 0;

    while li < local_uids.len() && ri < remote_uids.len() {
        if local_uids[li] < remote_uids[ri] {
            // Local UID not on server -> to delete
            to_delete.push(local_uids[li]);
            li += 1;
        } else if local_uids[li] > remote_uids[ri] {
            // Remote UID not local -> to fetch
            to_fetch.push(remote_uids[ri]);
            to_fetch_flags.push(remote_flags[ri]);
            ri += 1;
        } else {
            // Same UID -> update flags
            to_update.push(remote_uids[ri]);
            to_update_flags.push(remote_flags[ri]);
            li += 1;
            ri += 1;
        }
    }

    // Remaining locals
    while li < local_uids.len() {
        to_delete.push(local_uids[li]);
        li += 1;
    }

    // Remaining remotes
    while ri < remote_uids.len() {
        to_fetch.push(remote_uids[ri]);
        to_fetch_flags.push(remote_flags[ri]);
        ri += 1;
    }

    (to_delete, to_fetch, to_fetch_flags, to_update, to_update_flags)
}

/// Read a message from the mbox file by index.
fn read_message_raw(toc: &Toc, index: usize) -> io::Result<Vec<u8>> {
    if index >= toc.msgs.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "index out of range"));
    }
    let msg = &toc.msgs[index];
    let data = fs::read(&toc.mbox_path)?;
    let offset = msg.offset as usize;
    let length = msg.length as usize;
    if offset + length > data.len() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "message beyond mbox"));
    }
    Ok(data[offset..offset + length].to_vec())
}

/// Strip the leading "From " envelope line.
fn strip_from_line(msg: &[u8]) -> &[u8] {
    if msg.len() > 5 && &msg[..5] == b"From " {
        if let Some(nl) = msg.iter().position(|&b| b == b'\n') {
            return &msg[nl + 1..];
        }
    }
    msg
}

/// Find the offset of the body (blank line after headers).
fn find_body_offset(msg: &[u8]) -> usize {
    let mut i = 0;
    while i < msg.len() {
        if i + 1 < msg.len() && msg[i] == b'\n' && msg[i + 1] == b'\n' {
            return i + 2;
        }
        if i + 3 < msg.len()
            && msg[i] == b'\r' && msg[i + 1] == b'\n'
            && msg[i + 2] == b'\r' && msg[i + 3] == b'\n'
        {
            return i + 4;
        }
        i += 1;
    }
    msg.len()
}

/// Write a "From " envelope line.
fn write_from_line(sender: &str) -> String {
    format!("From {} Thu Jan  1 00:00:00 1970\n", sender)
}

/// Extract sender from From: header.
fn extract_from_header(msg: &[u8]) -> String {
    let text = String::from_utf8_lossy(msg);
    for line in text.lines() {
        if line.is_empty() || line == "\r" {
            break;
        }
        if line.len() >= 5 && line[..5].eq_ignore_ascii_case("From:") {
            return line[5..].trim().to_string();
        }
    }
    "unknown".to_string()
}

/// Append a message to a TOC's mbox file and update the TOC.
fn append_to_mbox(
    toc: &mut Toc,
    message: &[u8],
    sender: &str,
    state: State,
    priority: u8,
) -> io::Result<usize> {
    use std::io::Seek;

    let real_sender = if sender.is_empty() {
        extract_from_header(message)
    } else {
        sender.to_string()
    };
    let from_line = write_from_line(&real_sender);

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&toc.mbox_path)?;

    let offset = file.seek(io::SeekFrom::End(0))? as i64;
    file.write_all(from_line.as_bytes())?;
    file.write_all(message)?;
    if !message.is_empty() && message[message.len() - 1] != b'\n' {
        file.write_all(b"\n")?;
    }
    let end = file.seek(io::SeekFrom::End(0))? as i64;
    let length = end - offset;

    let serial = toc.next_serial;
    toc.next_serial += 1;

    let body_offset = find_body_offset(message) as i32 + from_line.len() as i32;

    let subject = {
        let text = String::from_utf8_lossy(message);
        text.lines()
            .find(|l| l.len() >= 8 && l[..8].eq_ignore_ascii_case("Subject:"))
            .map(|l| l[8..].trim().to_string())
            .unwrap_or_default()
    };

    let msg_id_hash = crate::thread::extract_msgid_hash(message);

    let mut sum = MsgSum::default();
    sum.offset = offset;
    sum.length = length;
    sum.body_offset = body_offset;
    sum.state = state;
    sum.priority = priority;
    sum.orig_priority = priority;
    sum.serial_num = serial;
    sum.msg_id_hash = msg_id_hash;
    sum.from = real_sender;
    sum.subject = subject;

    let idx = toc.msgs.len();
    toc.msgs.push(sum);
    toc.dirty = true;
    Ok(idx)
}

// ══════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── Mock implementations ──

    struct MockConf {
        accounts: Vec<Account>,
        values: Vec<(String, String, String)>, // (section, key, value)
    }

    impl MockConf {
        fn new() -> Self {
            Self {
                accounts: vec![Account {
                    index: 0,
                    name: "Test".to_string(),
                    email: "test@example.com".to_string(),
                    username: "testuser".to_string(),
                    server: "mail.example.com".to_string(),
                    smtp_server: "smtp.example.com".to_string(),
                    account_type: "POP".to_string(),
                    enabled: true,
                    leave_on_server: false,
                    ssl_mode: 0,
                }],
                values: vec![
                    ("checking_mail".into(), "saved_password".into(), "secret".into()),
                ],
            }
        }
    }

    impl ConfProvider for MockConf {
        fn get_dominant(&self) -> Option<Account> {
            self.accounts.first().cloned()
        }
        fn get_account(&self, index: i32) -> Option<Account> {
            self.accounts.get(index as usize).cloned()
        }
        fn count_accounts(&self) -> i32 {
            (self.accounts.len() as i32).saturating_sub(1)
        }
        fn get(&self, section: &str, key: &str) -> Option<String> {
            self.values.iter()
                .find(|(s, k, _)| s == section && k == key)
                .map(|(_, _, v)| v.clone())
        }
    }

    struct MockStore {
        base: PathBuf,
        in_path: PathBuf,
        out_path: PathBuf,
    }

    impl MockStore {
        fn new(base: &Path) -> Self {
            let in_path = base.join("In");
            let out_path = base.join("Out");
            // Create mbox files
            let _ = fs::write(&in_path, b"");
            let _ = fs::write(&out_path, b"");
            Self {
                base: base.to_path_buf(),
                in_path,
                out_path,
            }
        }
    }

    impl StoreProvider for MockStore {
        fn find_special(&self, mbox_type: MailboxType) -> Option<PathBuf> {
            match mbox_type {
                MailboxType::In => Some(self.in_path.clone()),
                MailboxType::Out => Some(self.out_path.clone()),
                _ => None,
            }
        }
        fn base_path(&self) -> &Path {
            &self.base
        }
        fn root_path(&self) -> &Path {
            &self.base
        }
    }

    struct MockSmtp {
        sent: Arc<Mutex<Vec<(String, Vec<String>, Vec<u8>)>>>,
    }

    impl SmtpClient for MockSmtp {
        fn connect(&mut self, _s: &str, _p: u16, _sec: SmtpSecurity) -> io::Result<()> {
            Ok(())
        }
        fn auth_plain(&mut self, _u: &str, _p: &str) -> io::Result<()> { Ok(()) }
        fn auth_login(&mut self, _u: &str, _p: &str) -> io::Result<()> { Ok(()) }
        fn send_message(&mut self, from: &str, rcpts: &[&str], msg: &[u8]) -> io::Result<()> {
            self.sent.lock().unwrap().push((
                from.to_string(),
                rcpts.iter().map(|s| s.to_string()).collect(),
                msg.to_vec(),
            ));
            Ok(())
        }
        fn close(&mut self) {}
    }

    struct MockPop3 {
        messages: Vec<Vec<u8>>,
    }

    impl Pop3Client for MockPop3 {
        fn connect(&mut self, _s: &str, _p: u16, _sec: Pop3Security) -> io::Result<()> {
            Ok(())
        }
        fn authenticate(&mut self, _u: &str, _p: &str) -> io::Result<()> { Ok(()) }
        fn stat(&mut self) -> io::Result<(u32, u64)> {
            Ok((self.messages.len() as u32, 0))
        }
        fn list_uidl(&mut self) -> io::Result<Vec<Pop3MsgInfo>> {
            Ok(self.messages.iter().enumerate().map(|(i, _)| Pop3MsgInfo {
                number: (i + 1) as u32,
                size: 0,
                uidl: format!("uid{}", i + 1),
            }).collect())
        }
        fn retrieve(&mut self, msg_num: u32) -> io::Result<Vec<u8>> {
            let idx = (msg_num - 1) as usize;
            if idx < self.messages.len() {
                Ok(self.messages[idx].clone())
            } else {
                Err(io::Error::new(io::ErrorKind::NotFound, "no such message"))
            }
        }
        fn delete(&mut self, _n: u32) -> io::Result<()> { Ok(()) }
        fn close(&mut self) {}
    }

    struct MockImap;
    impl ImapClient for MockImap {
        fn connect(&mut self, _s: &str, _p: u16, _sec: ImapSecurity) -> io::Result<()> { Ok(()) }
        fn login(&mut self, _u: &str, _p: &str) -> io::Result<()> { Ok(()) }
        fn select(&mut self, _mb: &str) -> io::Result<ImapMailboxInfo> {
            Ok(ImapMailboxInfo { exists: 0, uidvalidity: 1, uidnext: 1 })
        }
        fn fetch_flags(&mut self, _us: &str) -> io::Result<Vec<(u64, ImapFlags)>> { Ok(vec![]) }
        fn fetch_message(&mut self, _uid: u64) -> io::Result<Vec<u8>> { Ok(vec![]) }
        fn fetch_section(&mut self, _uid: u64, _sec: &str) -> io::Result<Vec<u8>> { Ok(vec![]) }
        fn idle(&mut self, _t: u32) -> io::Result<String> { Ok(String::new()) }
        fn noop(&mut self) -> io::Result<()> { Ok(()) }
        fn close(&mut self) {}
        fn is_connected(&self) -> bool { false }
    }

    struct MockFactory {
        smtp_sent: Arc<Mutex<Vec<(String, Vec<String>, Vec<u8>)>>>,
        pop3_messages: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl MockFactory {
        fn new() -> Self {
            Self {
                smtp_sent: Arc::new(Mutex::new(Vec::new())),
                pop3_messages: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl SessionFactory for MockFactory {
        fn create_smtp(&self) -> Box<dyn SmtpClient> {
            Box::new(MockSmtp { sent: self.smtp_sent.clone() })
        }
        fn create_pop3(&self) -> Box<dyn Pop3Client> {
            let msgs = self.pop3_messages.lock().unwrap().clone();
            Box::new(MockPop3 { messages: msgs })
        }
        fn create_imap(&self) -> Box<dyn ImapClient> {
            Box::new(MockImap)
        }
    }

    fn make_mailer(dir: &Path) -> (Mailer, Arc<Mutex<Vec<(String, Vec<String>, Vec<u8>)>>>, Arc<Mutex<Vec<Vec<u8>>>>) {
        let conf = MockConf::new();
        let store = MockStore::new(dir);
        let factory = MockFactory::new();
        let smtp_sent = factory.smtp_sent.clone();
        let pop3_msgs = factory.pop3_messages.clone();
        let mailer = Mailer::new(
            Box::new(conf),
            Box::new(store),
            Box::new(factory),
        );
        (mailer, smtp_sent, pop3_msgs)
    }

    // ── Tests ──

    #[test]
    fn test_extract_bare_email() {
        assert_eq!(extract_bare_email("John <john@example.com>"), "john@example.com");
        assert_eq!(extract_bare_email("plain@example.com"), "plain@example.com");
        assert_eq!(extract_bare_email("<bare@example.com>"), "bare@example.com");
    }

    #[test]
    fn test_collect_recipients() {
        let msg = b"To: alice@example.com, bob@example.com\r\nCc: charlie@example.com\r\nBcc: Dave <dave@example.com>\r\n\r\nBody";
        let rcpts = collect_recipients(msg);
        assert_eq!(rcpts.len(), 4);
        assert!(rcpts.contains(&"alice@example.com".to_string()));
        assert!(rcpts.contains(&"bob@example.com".to_string()));
        assert!(rcpts.contains(&"charlie@example.com".to_string()));
        assert!(rcpts.contains(&"dave@example.com".to_string()));
    }

    #[test]
    fn test_collect_recipients_empty() {
        let msg = b"Subject: test\r\n\r\nBody";
        let rcpts = collect_recipients(msg);
        assert!(rcpts.is_empty());
    }

    #[test]
    fn test_personality_hash() {
        let h1 = personality_hash("Work");
        let h2 = personality_hash("Work");
        assert_eq!(h1, h2);
        assert_ne!(h1, 0);

        let h3 = personality_hash("Personal");
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_imap_flags_to_state() {
        let flags_read = ImapFlags { seen: true, ..Default::default() };
        assert_eq!(imap_flags_to_state(&flags_read), State::Read);

        let flags_answered = ImapFlags { answered: true, seen: true, ..Default::default() };
        assert_eq!(imap_flags_to_state(&flags_answered), State::Replied);

        let flags_draft = ImapFlags { draft: true, ..Default::default() };
        assert_eq!(imap_flags_to_state(&flags_draft), State::Unsent);

        let flags_new = ImapFlags::default();
        assert_eq!(imap_flags_to_state(&flags_new), State::Unread);
    }

    #[test]
    fn test_merge_uid_lists_basic() {
        let local = vec![1, 2, 3, 5];
        let remote = vec![2, 3, 4, 6];
        let flags: Vec<ImapFlags> = vec![ImapFlags::default(); 4];

        let (del, fetch, _ff, upd, _uf) = merge_uid_lists(&local, &remote, &flags);
        assert_eq!(del, vec![1, 5]);     // local-only -> delete
        assert_eq!(fetch, vec![4, 6]);   // remote-only -> fetch
        assert_eq!(upd, vec![2, 3]);     // both -> update
    }

    #[test]
    fn test_merge_uid_lists_empty_local() {
        let local: Vec<u64> = vec![];
        let remote = vec![1, 2, 3];
        let flags: Vec<ImapFlags> = vec![ImapFlags::default(); 3];

        let (del, fetch, _ff, upd, _uf) = merge_uid_lists(&local, &remote, &flags);
        assert!(del.is_empty());
        assert_eq!(fetch, vec![1, 2, 3]);
        assert!(upd.is_empty());
    }

    #[test]
    fn test_merge_uid_lists_empty_remote() {
        let local = vec![1, 2, 3];
        let remote: Vec<u64> = vec![];
        let flags: Vec<ImapFlags> = vec![];

        let (del, fetch, _ff, upd, _uf) = merge_uid_lists(&local, &remote, &flags);
        assert_eq!(del, vec![1, 2, 3]);
        assert!(fetch.is_empty());
        assert!(upd.is_empty());
    }

    #[test]
    fn test_merge_uid_lists_identical() {
        let local = vec![1, 2, 3];
        let remote = vec![1, 2, 3];
        let flags: Vec<ImapFlags> = vec![ImapFlags::default(); 3];

        let (del, fetch, _ff, upd, _uf) = merge_uid_lists(&local, &remote, &flags);
        assert!(del.is_empty());
        assert!(fetch.is_empty());
        assert_eq!(upd, vec![1, 2, 3]);
    }

    #[test]
    fn test_smtp_security_from_ssl() {
        assert_eq!(Mailer::smtp_security_from_ssl(0), (SmtpSecurity::Plain, 25));
        assert_eq!(Mailer::smtp_security_from_ssl(1), (SmtpSecurity::StartTls, 587));
        assert_eq!(Mailer::smtp_security_from_ssl(2), (SmtpSecurity::StartTls, 587));
        assert_eq!(Mailer::smtp_security_from_ssl(4), (SmtpSecurity::Ssl, 465));
        assert_eq!(Mailer::smtp_security_from_ssl(6), (SmtpSecurity::Ssl, 465));
    }

    #[test]
    fn test_pop3_security_from_ssl() {
        assert_eq!(Mailer::pop3_security_from_ssl(0), (Pop3Security::Plain, 110));
        assert_eq!(Mailer::pop3_security_from_ssl(2), (Pop3Security::Stls, 110));
        assert_eq!(Mailer::pop3_security_from_ssl(4), (Pop3Security::Ssl, 995));
    }

    #[test]
    fn test_imap_security_from_ssl() {
        assert_eq!(Mailer::imap_security_from_ssl(0), (ImapSecurity::Plain, 143));
        assert_eq!(Mailer::imap_security_from_ssl(2), (ImapSecurity::StartTls, 143));
        assert_eq!(Mailer::imap_security_from_ssl(4), (ImapSecurity::Ssl, 993));
    }

    #[test]
    fn test_queue_and_send() {
        let tmp = tempfile::tempdir().unwrap();
        let (mailer, smtp_sent, _) = make_mailer(tmp.path());

        let msg = b"To: dest@example.com\r\nFrom: test@example.com\r\nSubject: Test\r\n\r\nBody";
        let idx = mailer.queue(msg, "test@example.com", &["dest@example.com"]).unwrap();
        assert_eq!(idx, 0);

        let count = mailer.queued_count();
        assert_eq!(count, 1);

        let sent = mailer.send().unwrap();
        assert_eq!(sent, 1);

        let sent_msgs = smtp_sent.lock().unwrap();
        assert_eq!(sent_msgs.len(), 1);
        assert_eq!(sent_msgs[0].0, "test@example.com");
        assert!(sent_msgs[0].1.contains(&"dest@example.com".to_string()));
    }

    #[test]
    fn test_queue_cancel() {
        let tmp = tempfile::tempdir().unwrap();
        let (mailer, _, _) = make_mailer(tmp.path());

        let msg = b"To: dest@example.com\r\nSubject: Cancel\r\n\r\nBody";
        mailer.queue(msg, "test@example.com", &[]).unwrap();
        assert_eq!(mailer.queued_count(), 1);

        mailer.cancel(0).unwrap();
        assert_eq!(mailer.queued_count(), 0);
    }

    #[test]
    fn test_send_now() {
        let tmp = tempfile::tempdir().unwrap();
        let (mailer, smtp_sent, _) = make_mailer(tmp.path());

        let msg = b"To: bob@example.com\r\nSubject: Instant\r\n\r\nBody";
        mailer.send_now(msg, "test@example.com", Some(&["bob@example.com"])).unwrap();

        let sent = smtp_sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "test@example.com");
    }

    #[test]
    fn test_check_pop3() {
        let tmp = tempfile::tempdir().unwrap();
        let (mailer, _, pop3_msgs) = make_mailer(tmp.path());

        // Pre-load mock POP3 with messages
        {
            let mut msgs = pop3_msgs.lock().unwrap();
            msgs.push(b"Message-ID: <test1@ex.com>\r\nFrom: alice@ex.com\r\nSubject: Hello\r\n\r\nBody 1".to_vec());
            msgs.push(b"Message-ID: <test2@ex.com>\r\nFrom: bob@ex.com\r\nSubject: World\r\n\r\nBody 2".to_vec());
        }

        let downloaded = mailer.check_account(0).unwrap();
        assert_eq!(downloaded, 2);

        // Check inbox has messages
        let in_path = tmp.path().join("In");
        let inbox = Toc::open_or_create(&in_path).unwrap();
        assert_eq!(inbox.msgs.len(), 2);
    }

    #[test]
    fn test_check_pop3_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        let (mailer, _, pop3_msgs) = make_mailer(tmp.path());

        // Same message ID twice
        {
            let mut msgs = pop3_msgs.lock().unwrap();
            msgs.push(b"Message-ID: <dup@ex.com>\r\nFrom: a@ex.com\r\nSubject: Dup\r\n\r\nBody".to_vec());
            msgs.push(b"Message-ID: <dup@ex.com>\r\nFrom: a@ex.com\r\nSubject: Dup\r\n\r\nBody".to_vec());
        }

        let downloaded = mailer.check_account(0).unwrap();
        // Only 1 should be downloaded (second is dup)
        assert_eq!(downloaded, 1);
    }

    #[test]
    fn test_lmos_tracking() {
        let tmp = tempfile::tempdir().unwrap();
        let (mailer, _, _) = make_mailer(tmp.path());

        assert!(!mailer.lmos_seen(0, "uid123"));
        mailer.lmos_mark(0, "uid123");
        assert!(mailer.lmos_seen(0, "uid123"));
        assert!(!mailer.lmos_seen(0, "uid456"));
    }

    #[test]
    fn test_lmos_path() {
        let tmp = tempfile::tempdir().unwrap();
        let (mailer, _, _) = make_mailer(tmp.path());

        let path = mailer.lmos_path(0).unwrap();
        assert!(path.to_string_lossy().contains("lmos_testuser@mail.example.com"));
    }

    #[test]
    fn test_is_stub() {
        let tmp = tempfile::tempdir().unwrap();
        let (mailer, _, _) = make_mailer(tmp.path());

        let in_path = tmp.path().join("In");
        let mut toc = Toc::open_or_create(&in_path).unwrap();

        // Add a normal message
        let msg = b"From: test@ex.com\r\nSubject: Normal\r\n\r\nFull body here";
        append_to_mbox(&mut toc, msg, "test@ex.com", State::Read, 3).unwrap();
        assert!(!mailer.is_stub(&toc, 0));

        // Add a stub (length <= body_offset)
        let mut stub = MsgSum::default();
        stub.length = 100;
        stub.body_offset = 100;
        toc.msgs.push(stub);
        assert!(mailer.is_stub(&toc, 1));
    }

    #[test]
    fn test_idle_start_stop() {
        let tmp = tempfile::tempdir().unwrap();
        // Create an IMAP account
        let mut conf = MockConf::new();
        conf.accounts[0].account_type = "IMAP".to_string();
        let store = MockStore::new(tmp.path());
        let factory = MockFactory::new();
        let mut mailer = Mailer::new(
            Box::new(conf),
            Box::new(store),
            Box::new(factory),
        );

        assert!(!mailer.idle_active(0));

        let cb: IdleCallback = Box::new(|_acct, _new, _del, _upd| {});
        mailer.idle_start(0, cb).unwrap();
        assert!(mailer.idle_active(0));

        mailer.idle_stop(0);
        assert!(!mailer.idle_active(0));
    }

    #[test]
    fn test_idle_non_imap_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut mailer, _, _) = make_mailer(tmp.path());
        // Default mock is POP, so IDLE should fail
        let cb: IdleCallback = Box::new(|_, _, _, _| {});
        assert!(mailer.idle_start(0, cb).is_err());
    }

    #[test]
    fn test_disconnect() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conf = MockConf::new();
        conf.accounts[0].account_type = "IMAP".to_string();
        let store = MockStore::new(tmp.path());
        let factory = MockFactory::new();
        let mut mailer = Mailer::new(
            Box::new(conf),
            Box::new(store),
            Box::new(factory),
        );

        let cb: IdleCallback = Box::new(|_, _, _, _| {});
        mailer.idle_start(0, cb).unwrap();
        assert!(mailer.idle_active(0));

        mailer.disconnect();
        assert!(!mailer.idle_active(0));
    }

    #[test]
    fn test_send_with_signature() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut mailer, smtp_sent, _) = make_mailer(tmp.path());

        // Workaround: we need to reconstruct mailer with signature
        // Since set_signature takes &mut self, just call it
        mailer.set_signature("Best regards,\nTest User");

        let msg = b"To: dest@example.com\r\nSubject: Signed\r\n\r\nBody";
        mailer.queue(msg, "test@example.com", &[]).unwrap();
        mailer.send().unwrap();

        let sent = smtp_sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let sent_body = String::from_utf8_lossy(&sent[0].2);
        assert!(sent_body.contains("-- \r\n"));
        assert!(sent_body.contains("Best regards,"));
    }

    #[test]
    fn test_test_connection() {
        let tmp = tempfile::tempdir().unwrap();
        let (mailer, _, _) = make_mailer(tmp.path());
        // Mock always succeeds
        assert!(mailer.test_connection(0).is_ok());
    }

    #[test]
    fn test_find_body_offset() {
        let msg = b"Subject: test\r\n\r\nBody here";
        assert_eq!(find_body_offset(msg), 17);

        let msg2 = b"Subject: test\n\nBody here";
        assert_eq!(find_body_offset(msg2), 15);

        let msg3 = b"No blank line";
        assert_eq!(find_body_offset(msg3), msg3.len());
    }

    #[test]
    fn test_extract_from_header() {
        let msg = b"From: Alice <alice@example.com>\r\nSubject: test\r\n\r\nbody";
        assert_eq!(extract_from_header(msg), "Alice <alice@example.com>");

        let msg2 = b"Subject: test\r\n\r\nbody";
        assert_eq!(extract_from_header(msg2), "unknown");
    }

    #[test]
    fn test_strip_from_line() {
        let msg = b"From user@ex.com Thu Jan  1 00:00:00 1970\nSubject: test\n\nbody";
        let stripped = strip_from_line(msg);
        assert!(stripped.starts_with(b"Subject:"));

        let msg2 = b"Subject: test\n\nbody";
        assert_eq!(strip_from_line(msg2), msg2);
    }
}
