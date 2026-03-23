//! Mail scheduler — check mail, send queued, offline toggle.
//!
//! Port of idle_scheduler.c.
//! Wires macmbx::mailer::Mailer to crispy protocol sessions.
//! Background threads for check/send, results via glib::idle_add.

use gtk4::prelude::*;
use gtk4::glib;
use macmbx::conf::Conf;
use macmbx::mailer::*;
use macmbx::toc::MailboxType;
use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

// ── ConfProvider ──

struct RealConf(Arc<Mutex<Conf>>);
impl ConfProvider for RealConf {
    fn get_dominant(&self) -> Option<Account> {
        let c = self.0.lock().unwrap();
        let a = c.get_dominant();
        Some(Account {
            index: 0, name: a.name, email: a.email, username: a.username,
            server: a.server, smtp_server: a.smtp_server,
            account_type: a.account_type, enabled: a.enabled,
            leave_on_server: a.leave_on_server, ssl_mode: a.ssl_mode as u8,
        })
    }
    fn get_account(&self, i: i32) -> Option<Account> {
        let c = self.0.lock().unwrap();
        let a = if i == 0 { Some(c.get_dominant()) } else { c.get_account(i)? ; c.get_account(i) }?;
        Some(Account {
            index: a.index, name: a.name, email: a.email, username: a.username,
            server: a.server, smtp_server: a.smtp_server,
            account_type: a.account_type, enabled: a.enabled,
            leave_on_server: a.leave_on_server, ssl_mode: a.ssl_mode as u8,
        })
    }
    fn count_accounts(&self) -> i32 { self.0.lock().unwrap().count_accounts() as i32 }
    fn get(&self, s: &str, k: &str) -> Option<String> { self.0.lock().unwrap().get(s, k).map(|s| s.to_string()) }
}

// ── StoreProvider ──

struct RealStore(PathBuf);
impl StoreProvider for RealStore {
    fn find_special(&self, t: MailboxType) -> Option<PathBuf> {
        let n = match t { MailboxType::In => "In", MailboxType::Out => "Out",
                          MailboxType::Trash => "Trash", MailboxType::Junk => "Junk", _ => return None };
        let p = self.0.join(n); if p.exists() { Some(p) } else { None }
    }
    fn base_path(&self) -> &Path { &self.0 }
    fn root_path(&self) -> &Path { &self.0 }
}

// ── Crispy wrappers ──
// SAFETY: sessions are created per-thread and never shared.
// The non-Send debug callback is always None in our usage.

struct CrispySmtp(crispy::smtp::SmtpSession<crispy::transport::TcpTransport>);
unsafe impl Send for CrispySmtp {}

impl SmtpClient for CrispySmtp {
    fn connect(&mut self, server: &str, port: u16, sec: SmtpSecurity) -> io::Result<()> {
        let s = match sec { SmtpSecurity::Plain => crispy::smtp::Security::Plain,
                           SmtpSecurity::StartTls => crispy::smtp::Security::StartTls,
                           SmtpSecurity::Ssl => crispy::smtp::Security::Ssl };
        self.0.connect(server, port, s).map_err(io_err)
    }
    fn auth_plain(&mut self, u: &str, p: &str) -> io::Result<()> { self.0.auth_plain(u, p).map_err(io_err) }
    fn auth_login(&mut self, u: &str, p: &str) -> io::Result<()> { self.0.auth_login(u, p).map_err(io_err) }
    fn send_message(&mut self, from: &str, rcpts: &[&str], msg: &[u8]) -> io::Result<()> {
        self.0.send_message(from, rcpts, msg).map_err(io_err)
    }
    fn close(&mut self) { let _ = self.0.quit(); }
}

struct CrispyPop3(crispy::pop3::Pop3Session<crispy::transport::TcpTransport>);
unsafe impl Send for CrispyPop3 {}

impl Pop3Client for CrispyPop3 {
    fn connect(&mut self, server: &str, port: u16, sec: Pop3Security) -> io::Result<()> {
        let s = match sec { Pop3Security::Plain => crispy::pop3::Pop3Security::Plain,
                           Pop3Security::Stls => crispy::pop3::Pop3Security::Stls,
                           Pop3Security::Ssl => crispy::pop3::Pop3Security::Ssl };
        self.0.connect(server, port, s).map_err(io_err)
    }
    fn authenticate(&mut self, u: &str, p: &str) -> io::Result<()> {
        self.0.authenticate(u, p).map_err(io_err)
    }
    fn stat(&mut self) -> io::Result<(u32, u64)> {
        self.0.stat().map_err(io_err)?;
        Ok((self.0.msg_count, self.0.mailbox_size))
    }
    fn list_uidl(&mut self) -> io::Result<Vec<Pop3MsgInfo>> {
        let msgs = self.0.list().map_err(io_err)?;
        Ok(msgs.into_iter().map(|m| Pop3MsgInfo {
            number: m.number, size: m.size, uidl: m.uidl,
        }).collect())
    }
    fn retrieve(&mut self, n: u32) -> io::Result<Vec<u8>> { self.0.retrieve(n).map_err(io_err) }
    fn delete(&mut self, n: u32) -> io::Result<()> { self.0.delete(n).map_err(io_err) }
    fn close(&mut self) { self.0.quit(); }
}

struct CrispyImap(crispy::imap::ImapSession<crispy::transport::TcpTransport>);
unsafe impl Send for CrispyImap {}

impl ImapClient for CrispyImap {
    fn connect(&mut self, server: &str, port: u16, sec: ImapSecurity) -> io::Result<()> {
        let s = match sec { ImapSecurity::Plain => crispy::imap::ImapSecurity::Plain,
                           ImapSecurity::StartTls => crispy::imap::ImapSecurity::StartTls,
                           ImapSecurity::Ssl => crispy::imap::ImapSecurity::Ssl };
        self.0.connect(server, port, s).map_err(io_err)
    }
    fn login(&mut self, u: &str, p: &str) -> io::Result<()> { self.0.login(u, p).map_err(io_err) }
    fn select(&mut self, mb: &str) -> io::Result<ImapMailboxInfo> {
        self.0.select(mb).map_err(io_err)?;
        let s = &self.0.selected;
        Ok(ImapMailboxInfo {
            exists: s.exists, uidvalidity: s.uidvalidity, uidnext: s.uidnext,
        })
    }
    fn fetch_flags(&mut self, uid_set: &str) -> io::Result<Vec<(u64, ImapFlags)>> {
        let r = self.0.fetch_flags(uid_set).map_err(io_err)?;
        Ok(r.into_iter().map(|(uid, f)| (uid, ImapFlags {
            seen: f.seen, answered: f.answered, flagged: f.flagged,
            deleted: f.deleted, draft: f.draft,
        })).collect())
    }
    fn fetch_message(&mut self, uid: u64) -> io::Result<Vec<u8>> { self.0.fetch_body(uid).map_err(io_err) }
    fn fetch_section(&mut self, uid: u64, sec: &str) -> io::Result<Vec<u8>> { self.0.fetch_section(uid, sec).map_err(io_err) }
    fn idle(&mut self, timeout_ms: u32) -> io::Result<String> { self.0.idle(timeout_ms).map_err(io_err)?; Ok(String::new()) }
    fn noop(&mut self) -> io::Result<()> { self.0.noop().map_err(io_err) }
    fn close(&mut self) { self.0.close(); }
    fn is_connected(&self) -> bool { self.0.connected }
}

fn io_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

struct CrispyFactory;
impl SessionFactory for CrispyFactory {
    fn create_smtp(&self) -> Box<dyn SmtpClient> {
        Box::new(CrispySmtp(crispy::smtp::SmtpSession::new(
            crispy::transport::TcpTransport::new(), "localhost")))
    }
    fn create_pop3(&self) -> Box<dyn Pop3Client> {
        Box::new(CrispyPop3(crispy::pop3::Pop3Session::new(
            crispy::transport::TcpTransport::new())))
    }
    fn create_imap(&self) -> Box<dyn ImapClient> {
        Box::new(CrispyImap(crispy::imap::ImapSession::new(
            crispy::transport::TcpTransport::new())))
    }
}

// ── Scheduler state ──

static OFFLINE: AtomicBool = AtomicBool::new(false);
static NEED_CHECK: AtomicBool = AtomicBool::new(false);
static NEED_SEND: AtomicBool = AtomicBool::new(false);
static CHECK_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static SEND_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

pub fn is_offline() -> bool { OFFLINE.load(Ordering::Relaxed) }
pub fn set_offline(offline: bool) { OFFLINE.store(offline, Ordering::Relaxed); }
pub fn request_check() { if !is_offline() { NEED_CHECK.store(true, Ordering::Relaxed); } }
pub fn request_send() { if !is_offline() { NEED_SEND.store(true, Ordering::Relaxed); } }

/// Create a Mailer — public so AppState can hold it.
/// Sets up the keychain credential callback so the Mailer can
/// get passwords without the GUI needing to know about them.
pub fn create_mailer_from_conf(conf: &Conf, mb_dir: &Path) -> Mailer {
    let mut mailer = Mailer::new(
        Box::new(RealConf(Arc::new(Mutex::new(conf.clone())))),
        Box::new(RealStore(mb_dir.to_path_buf())),
        Box::new(CrispyFactory),
    );

    // Credential callback — macOS keychain lookup.
    // The Mailer calls this when it needs a password and doesn't have
    // one cached in conf. Passwords stay in macmbx, never in the GUI.
    #[cfg(target_os = "macos")]
    mailer.set_credentials(Box::new(|user: &str, server: &str| -> Option<String> {
        use security_framework::passwords::get_generic_password;
        let key = format!("{}@{}", user, server);
        match get_generic_password("gEudora", &key) {
            Ok(pw_bytes) => String::from_utf8(pw_bytes).ok(),
            Err(_) => None,
        }
    }));

    // PostProcessor — apply filters + junk scoring + spam move on incoming mail
    let filter_path = mb_dir.parent().unwrap_or(mb_dir).join("Filters");
    mailer.set_post_processor(Box::new(RealPostProcessor {
        filter_path,
        mailboxes_dir: mb_dir.to_path_buf(),
    }));

    mailer
}

/// PostProcessor — applies filters + junk scoring + spam move after check mail.
/// Port of the C mailer's post-check pipeline: filter → score → move_spam.
struct RealPostProcessor {
    filter_path: PathBuf,
    mailboxes_dir: PathBuf,
}

impl PostProcessor for RealPostProcessor {
    fn apply_filters(&self, toc: &mut macmbx::toc::Toc) {
        if self.filter_path.exists() {
            if let Ok(mut fs) = macmbx::filter::FilterSet::load(&self.filter_path) {
                fs.compile();
                let reader = macmbx::filter::SummaryOnlyReader;
                fs.apply_all(toc, macmbx::filter::WHEN_INCOMING, &reader, None);
            }
        }
    }
    fn apply_junk(&self, toc: &mut macmbx::toc::Toc) {
        let cfg = macmbx::junk::JunkConfig::default();

        // Score all new messages
        let read_headers = |t: &macmbx::toc::Toc, i: usize| -> Option<String> {
            macmbx::ops::read_headers(t, i).ok()
        };
        let read_body = |t: &macmbx::toc::Toc, i: usize| -> Option<String> {
            macmbx::ops::read_body(t, i).ok().map(|b| String::from_utf8_lossy(&b).to_string())
        };
        macmbx::junk::score_box(&cfg, toc, read_headers, read_body);

        // Move scored spam to Junk mailbox (port of macmbx_junk_move_spam)
        if let Ok(store) = macmbx::store::Store::open(&self.mailboxes_dir) {
            let _ = macmbx::junk::move_spam(&cfg, toc, &store);
        }
    }
}

// Thread-safe status/error message holders
static PENDING_STATUS: Mutex<Option<String>> = Mutex::new(None);
// Sticky error — stays visible until next successful operation
static STICKY_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Start the scheduler tick (port of IdleSchedulerStart).
/// Takes the shared Mailer from AppState — same instance used by compose.
/// `check_interval_mins` is from conf checking_mail.check_interval (0 = no auto-check).
pub fn start(mailer: std::sync::Arc<Mailer>, status_label: gtk4::Label, check_interval_mins: i32) {
    // Auto-check counter — counts 500ms ticks
    let auto_check_ticks = if check_interval_mins > 0 {
        (check_interval_mins as u64) * 60 * 2 // ticks per interval (500ms each)
    } else { 0 };
    let tick_counter = std::cell::Cell::new(0u64);
    glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        // Check for pending status from background threads
        if let Ok(mut lock) = PENDING_STATUS.try_lock() {
            if let Some(msg) = lock.take() {
                status_label.set_text(&msg);
            } else if let Ok(err_lock) = STICKY_ERROR.try_lock() {
                // Show sticky error if no new status and there's an error
                if let Some(ref err) = *err_lock {
                    let current = status_label.text();
                    if !current.contains("failed") && !current.contains("error") {
                        status_label.set_text(err);
                    }
                }
            }
        }

        // Auto-check timer
        if auto_check_ticks > 0 && !OFFLINE.load(Ordering::Relaxed) {
            let c = tick_counter.get() + 1;
            tick_counter.set(c);
            if c >= auto_check_ticks {
                tick_counter.set(0);
                NEED_CHECK.store(true, Ordering::Relaxed);
            }
        }

        if NEED_CHECK.swap(false, Ordering::Relaxed) && !CHECK_IN_PROGRESS.load(Ordering::Relaxed) {
            CHECK_IN_PROGRESS.store(true, Ordering::Relaxed);
            status_label.set_text("Checking mail...");
            let task_id = crate::tasks::add_task("mail-inbox-symbolic", "Checking mail...");
            let m = mailer.clone();
            std::thread::spawn(move || {
                let result = m.check();
                CHECK_IN_PROGRESS.store(false, Ordering::Relaxed);
                crate::tasks::remove_task(task_id);
                let msg = match &result {
                    Ok(n) if *n > 0 => {
                        *STICKY_ERROR.lock().unwrap() = None;
                        crate::tasks::add_task("mail-unread-symbolic", &format!("{} new message(s)", n));
                        // Auto-remove the "new messages" line after 5s
                        let done_id = crate::tasks::add_task("", "");
                        crate::tasks::remove_task(done_id); // placeholder
                        format!("{} new message(s)", n)
                    }
                    Ok(_) => {
                        *STICKY_ERROR.lock().unwrap() = None;
                        "No new mail".to_string()
                    }
                    Err(e) => {
                        let err = format!("Check failed: {}", e);
                        *STICKY_ERROR.lock().unwrap() = Some(err.clone());
                        crate::tasks::add_error("dialog-error-symbolic", &err);
                        err
                    }
                };
                *PENDING_STATUS.lock().unwrap() = Some(msg);
            });
        }

        if NEED_SEND.swap(false, Ordering::Relaxed) && !SEND_IN_PROGRESS.load(Ordering::Relaxed) {
            SEND_IN_PROGRESS.store(true, Ordering::Relaxed);
            status_label.set_text("Sending mail...");
            let task_id = crate::tasks::add_task("mail-send-symbolic", "Sending mail...");
            let m = mailer.clone();
            std::thread::spawn(move || {
                let result = m.send();
                SEND_IN_PROGRESS.store(false, Ordering::Relaxed);
                crate::tasks::remove_task(task_id);
                let msg = match &result {
                    Ok(n) if *n > 0 => {
                        *STICKY_ERROR.lock().unwrap() = None;
                        format!("{} message(s) sent", n)
                    }
                    Ok(_) => {
                        *STICKY_ERROR.lock().unwrap() = None;
                        "Send complete".to_string()
                    }
                    Err(e) => {
                        let err = format!("Send failed: {}", e);
                        *STICKY_ERROR.lock().unwrap() = Some(err.clone());
                        crate::tasks::add_error("dialog-error-symbolic", &err);
                        err
                    }
                };
                *PENDING_STATUS.lock().unwrap() = Some(msg);
            });
        }

        glib::ControlFlow::Continue
    });
}
