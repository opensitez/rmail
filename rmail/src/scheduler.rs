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
use std::io::{self, Write as _};
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

// ── native-tls connector for crispy ──

struct NativeTlsConnector;

impl crispy::transport::TlsConnector for NativeTlsConnector {
    fn connect(
        &self,
        domain: &str,
        stream: std::net::TcpStream,
        trusted_pem: Option<&str>,
    ) -> Result<Box<dyn crispy::transport::TlsStream>, crispy::transport::TlsError> {
        let mut builder = native_tls::TlsConnector::builder();

        // If trusted_pem set OR host was previously accepted, skip verification
        if trusted_pem.is_some() || is_host_accepted(domain) {
            builder.danger_accept_invalid_certs(true);
        }

        let connector = builder.build()
            .map_err(|e| crispy::transport::TlsError {
                message: e.to_string(),
                cert_pem: None,
            })?;

        match connector.connect(domain, stream) {
            Ok(tls_stream) => Ok(Box::new(NativeTlsStream(tls_stream))),
            Err(e) => {
                Err(crispy::transport::TlsError {
                    message: e.to_string(),
                    // Return a placeholder so crispy's cert_callback fires
                    cert_pem: Some("ACCEPTED".to_string()),
                })
            }
        }
    }
}

struct NativeTlsStream(native_tls::TlsStream<std::net::TcpStream>);
unsafe impl Send for NativeTlsStream {}

impl std::io::Read for NativeTlsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> { self.0.read(buf) }
}
impl std::io::Write for NativeTlsStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> { self.0.write(buf) }
    fn flush(&mut self) -> io::Result<()> { self.0.flush() }
}
impl crispy::transport::TlsStream for NativeTlsStream {
    fn tcp_stream(&self) -> Option<&std::net::TcpStream> {
        self.0.get_ref().into()
    }
}

/// Cert prompt result — matches the C enum.
#[derive(Clone, Copy, PartialEq)]
enum CertResponse { Reject, AcceptOnce, AcceptAlways }

/// Block background thread, show cert dialog on main thread, return user choice.
fn prompt_cert(host: &str, error: &str) -> CertResponse {
    let response = Arc::new((Mutex::new(None::<CertResponse>),
                              std::sync::Condvar::new()));
    let r = response.clone();
    let host_s = host.to_string();
    let error_s = error.to_string();

    glib::idle_add_once(move || {
        let dlg = gtk4::Window::new();
        dlg.set_title(Some("SSL Certificate Warning"));
        dlg.set_modal(true);
        dlg.set_default_size(480, -1);
        dlg.set_resizable(false);

        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        vbox.set_margin_start(20);
        vbox.set_margin_end(20);
        vbox.set_margin_top(16);
        vbox.set_margin_bottom(16);
        dlg.set_child(Some(&vbox));

        let title = gtk4::Label::new(None);
        title.set_markup("<b><span size='large'>Untrusted SSL Certificate</span></b>");
        title.set_halign(gtk4::Align::Start);
        vbox.append(&title);

        let info_text = format!(
            "The server <b>{}</b> presented a certificate that could not be verified.\n\n\
             <b>Error:</b> {}", host_s, error_s);
        let info = gtk4::Label::new(None);
        info.set_markup(&info_text);
        info.set_halign(gtk4::Align::Start);
        info.set_wrap(true);
        info.set_selectable(true);
        vbox.append(&info);

        vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

        let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        btn_box.set_halign(gtk4::Align::End);
        btn_box.set_margin_top(8);

        let reject_btn = gtk4::Button::with_label("Reject");
        let once_btn = gtk4::Button::with_label("Accept Once");
        let always_btn = gtk4::Button::with_label("Accept Always");
        // Only highlight the suggested action, keep others plain
        always_btn.add_css_class("suggested-action");

        btn_box.append(&reject_btn);
        btn_box.append(&once_btn);
        btn_box.append(&always_btn);
        vbox.append(&btn_box);

        let respond = {
            let r = r.clone();
            let dlg = dlg.clone();
            move |resp: CertResponse| {
                let (lock, cvar) = &*r;
                let mut guard = lock.lock().unwrap();
                // Only set if not already responded (button click before close_request)
                if guard.is_none() {
                    *guard = Some(resp);
                    cvar.notify_one();
                }
                drop(guard);
                dlg.close();
            }
        };

        let resp_reject = respond.clone();
        reject_btn.connect_clicked(move |_| resp_reject(CertResponse::Reject));
        let resp_once = respond.clone();
        once_btn.connect_clicked(move |_| resp_once(CertResponse::AcceptOnce));
        let resp_always = respond.clone();
        always_btn.connect_clicked(move |_| resp_always(CertResponse::AcceptAlways));

        let r_close = r.clone();
        dlg.connect_close_request(move |_| {
            let (lock, cvar) = &*r_close;
            let mut guard = lock.lock().unwrap();
            if guard.is_none() {
                *guard = Some(CertResponse::Reject);
                cvar.notify_one();
            }
            glib::Propagation::Proceed
        });

        dlg.present();
    });

    // Block background thread until user responds
    let (lock, cvar) = &*response;
    let mut guard = lock.lock().unwrap();
    while guard.is_none() {
        guard = cvar.wait(guard).unwrap();
    }
    guard.unwrap()
}

/// Passwords that were tried and failed (cleared after successful check).
static BAD_PASSWORDS: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

fn is_password_bad(key: &str) -> bool {
    let lock = BAD_PASSWORDS.lock().unwrap();
    lock.as_ref().map_or(false, |s| s.contains(key))
}

fn mark_password_bad(key: &str) {
    let mut lock = BAD_PASSWORDS.lock().unwrap();
    if lock.is_none() { *lock = Some(std::collections::HashSet::new()); }
    lock.as_mut().unwrap().insert(key.to_string());
}

fn clear_bad_passwords() {
    let mut lock = BAD_PASSWORDS.lock().unwrap();
    if let Some(ref mut s) = *lock { s.clear(); }
}

/// Thread-local task ID for progress reporting from check_account threads.
thread_local! {
    static CURRENT_TASK_ID: std::cell::Cell<i32> = const { std::cell::Cell::new(-1) };
}

/// Hosts the user has accepted certs for this session only (Accept Once).
static ACCEPTED_ONCE: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);
/// Hosts the user has permanently accepted certs for (Accept Always — persisted).
static ACCEPTED_ALWAYS: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

fn is_host_accepted(host: &str) -> bool {
    let once = ACCEPTED_ONCE.lock().unwrap();
    if once.as_ref().map_or(false, |s| s.contains(host)) { return true; }
    let always = ACCEPTED_ALWAYS.lock().unwrap();
    always.as_ref().map_or(false, |s| s.contains(host))
}

fn accept_host_once(host: &str) {
    let mut lock = ACCEPTED_ONCE.lock().unwrap();
    if lock.is_none() { *lock = Some(std::collections::HashSet::new()); }
    lock.as_mut().unwrap().insert(host.to_string());
}

fn accept_host_always(host: &str) {
    let mut lock = ACCEPTED_ALWAYS.lock().unwrap();
    if lock.is_none() { *lock = Some(std::collections::HashSet::new()); }
    lock.as_mut().unwrap().insert(host.to_string());
    // Persist to disk
    let dir = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{}/.local/share", home)
        });
    let path = PathBuf::from(dir).join("geudora").join("trusted_certs.txt");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", host);
    }
}

/// Load permanently accepted hosts from disk on startup.
pub fn load_trusted_hosts() {
    let dir = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{}/.local/share", home)
        });
    let path = PathBuf::from(dir).join("geudora").join("trusted_certs.txt");
    if let Ok(data) = std::fs::read_to_string(&path) {
        let mut lock = ACCEPTED_ALWAYS.lock().unwrap();
        if lock.is_none() { *lock = Some(std::collections::HashSet::new()); }
        let set = lock.as_mut().unwrap();
        for line in data.lines() {
            let h = line.trim();
            if !h.is_empty() { set.insert(h.to_string()); }
        }
    }
}

/// Block background thread, show password dialog on main thread, return password.
fn prompt_password(user: &str, server: &str) -> Option<String> {
    let response: Arc<(Mutex<Option<Option<String>>>, std::sync::Condvar)> =
        Arc::new((Mutex::new(None), std::sync::Condvar::new()));
    let r = response.clone();
    let user_s = user.to_string();
    let server_s = server.to_string();

    glib::idle_add_once(move || {
        let dlg = gtk4::Window::new();
        dlg.set_title(Some("Password Required"));
        dlg.set_modal(true);
        dlg.set_default_size(400, -1);
        dlg.set_resizable(false);

        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        vbox.set_margin_start(20);
        vbox.set_margin_end(20);
        vbox.set_margin_top(16);
        vbox.set_margin_bottom(16);
        dlg.set_child(Some(&vbox));

        let label = gtk4::Label::new(None);
        label.set_markup(&format!(
            "Enter password for <b>{}</b> on <b>{}</b>:", user_s, server_s));
        label.set_halign(gtk4::Align::Start);
        label.set_wrap(true);
        vbox.append(&label);

        let entry = gtk4::PasswordEntry::new();
        entry.set_show_peek_icon(true);
        vbox.append(&entry);

        let save_check = gtk4::CheckButton::with_label("Save to Keychain");
        save_check.set_active(true);
        vbox.append(&save_check);

        let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        btn_box.set_halign(gtk4::Align::End);
        btn_box.set_margin_top(8);
        let cancel_btn = gtk4::Button::with_label("Cancel");
        let ok_btn = gtk4::Button::with_label("OK");
        ok_btn.add_css_class("suggested-action");
        btn_box.append(&cancel_btn);
        btn_box.append(&ok_btn);
        vbox.append(&btn_box);

        let respond = {
            let r = r.clone();
            let dlg = dlg.clone();
            move |pw: Option<String>| {
                let (lock, cvar) = &*r;
                let mut guard = lock.lock().unwrap();
                if guard.is_none() {
                    *guard = Some(pw);
                    cvar.notify_one();
                }
                drop(guard);
                dlg.close();
            }
        };

        let resp_ok = respond.clone();
        let entry_ok = entry.clone();
        let save_ok = save_check.clone();
        let user_ok = user_s.clone();
        let server_ok = server_s.clone();
        ok_btn.connect_clicked(move |_| {
            let pw = entry_ok.text().to_string();
            if !pw.is_empty() {
                // Save to keychain if checked
                #[cfg(target_os = "macos")]
                if save_ok.is_active() {
                    use security_framework::passwords::set_generic_password;
                    let key = format!("{}@{}", user_ok, server_ok);
                    let _ = set_generic_password("gEudora", &key, pw.as_bytes());
                }
                resp_ok(Some(pw));
            } else {
                resp_ok(None);
            }
        });

        let resp_cancel = respond.clone();
        cancel_btn.connect_clicked(move |_| resp_cancel(None));

        let resp_enter = respond.clone();
        let entry_enter = entry.clone();
        entry_enter.connect_activate(move |e| {
            let pw = e.text().to_string();
            if !pw.is_empty() {
                resp_enter(Some(pw));
            }
        });

        let r_close = r.clone();
        dlg.connect_close_request(move |_| {
            let (lock, cvar) = &*r_close;
            let mut guard = lock.lock().unwrap();
            if guard.is_none() {
                *guard = Some(None);
                cvar.notify_one();
            }
            glib::Propagation::Proceed
        });

        dlg.present();
        entry.grab_focus();
    });

    // Block background thread until user responds
    let (lock, cvar) = &*response;
    let mut guard = lock.lock().unwrap();
    while guard.is_none() {
        guard = cvar.wait(guard).unwrap();
    }
    guard.clone().flatten()
}

fn new_transport() -> crispy::transport::TcpTransport {
    let mut t = crispy::transport::TcpTransport::new();
    t.set_tls_connector(Box::new(NativeTlsConnector));
    // Prompt user on cert errors — blocks thread until dialog is answered
    t.set_cert_callback(Box::new(|host: &str, error: &str, _pem: &str| -> bool {
        eprintln!("cert_callback FIRED for host={} error={}", host, error);
        // Already accepted?
        if is_host_accepted(host) {
            eprintln!("cert_callback: host already accepted");
            return true;
        }
        let response = prompt_cert(host, error);
        eprintln!("cert_callback: user response = {:?}", response as i32);
        match response {
            CertResponse::AcceptOnce => { accept_host_once(host); true }
            CertResponse::AcceptAlways => { accept_host_always(host); true }
            CertResponse::Reject => false,
        }
    }));
    // If host was previously accepted, pre-set trusted_pem
    t
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
            new_transport(), "localhost")))
    }
    fn create_pop3(&self) -> Box<dyn Pop3Client> {
        Box::new(CrispyPop3(crispy::pop3::Pop3Session::new(
            new_transport())))
    }
    fn create_imap(&self) -> Box<dyn ImapClient> {
        Box::new(CrispyImap(crispy::imap::ImapSession::new(
            new_transport())))
    }
}

// ── Scheduler state ──

static OFFLINE: AtomicBool = AtomicBool::new(false);
static NEED_CHECK: AtomicBool = AtomicBool::new(false);
static NEED_SEND: AtomicBool = AtomicBool::new(false);
static CHECK_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static SEND_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
/// Set after check/send completes with changes — UI should refresh mailbox views.
static NEED_REFRESH: AtomicBool = AtomicBool::new(false);

pub fn is_offline() -> bool { OFFLINE.load(Ordering::Relaxed) }
pub fn set_offline(offline: bool) { OFFLINE.store(offline, Ordering::Relaxed); }
pub fn request_check() { if !is_offline() { NEED_CHECK.store(true, Ordering::Relaxed); } }
pub fn request_send() { if !is_offline() { NEED_SEND.store(true, Ordering::Relaxed); } }
/// Returns true (once) if mailbox views need refresh after check/send.
pub fn take_need_refresh() -> bool { NEED_REFRESH.swap(false, Ordering::Relaxed) }

/// Create a Mailer — public so AppState can hold it.
/// Sets up the keychain credential callback so the Mailer can
/// get passwords without the GUI needing to know about them.
/// Takes a shared conf so settings changes are visible to the mailer.
pub fn create_mailer_from_conf(conf: Arc<Mutex<Conf>>, mb_dir: &Path) -> Mailer {
    load_trusted_hosts();
    let mut mailer = Mailer::new(
        Box::new(RealConf(conf)),
        Box::new(RealStore(mb_dir.to_path_buf())),
        Box::new(CrispyFactory),
    );

    // Credential callback — macOS keychain lookup.
    // The Mailer calls this when it needs a password and doesn't have
    // one cached in conf. Passwords stay in macmbx, never in the GUI.
    mailer.set_credentials(Box::new(|user: &str, server: &str| -> Option<String> {
        let key = format!("{}@{}", user, server);

        // If this key was already tried (auth failed), skip keychain and prompt
        if is_password_bad(&key) {
            #[cfg(target_os = "macos")]
            {
                use security_framework::passwords::delete_generic_password;
                let _ = delete_generic_password("gEudora", &key);
            }
            return prompt_password(user, server);
        }

        // Try keychain first (macOS)
        #[cfg(target_os = "macos")]
        {
            use security_framework::passwords::get_generic_password;
            if let Ok(pw_bytes) = get_generic_password("gEudora", &key) {
                if let Ok(pw) = String::from_utf8(pw_bytes) {
                    // Mark as tried — if auth fails and cred_fn is called again,
                    // we'll know this password was bad
                    mark_password_bad(&key);
                    return Some(pw);
                }
            }
        }

        // Keychain failed or not on macOS — prompt user
        prompt_password(user, server)
    }));

    // Progress callback — routes per-message progress to the task panel
    mailer.set_progress(Box::new(|status: &str, current: i32, total: i32| {
        let task_id = CURRENT_TASK_ID.with(|c| c.get());
        if task_id >= 0 {
            let progress = if total > 0 { current as f64 / total as f64 } else { -1.0 };
            crate::tasks::update_detailed_task(task_id, status, progress);
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
pub fn start(
    mailer: std::sync::Arc<Mailer>,
    status_label: gtk4::Label,
    check_interval_mins: i32,
    on_refresh: Option<Rc<dyn Fn()>>,
) {
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

        // Refresh mailbox views if background thread signaled
        if NEED_REFRESH.swap(false, Ordering::Relaxed) {
            if let Some(ref cb) = on_refresh {
                cb();
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

        if NEED_CHECK.load(Ordering::Relaxed) && !CHECK_IN_PROGRESS.load(Ordering::Relaxed) {
            NEED_CHECK.store(false, Ordering::Relaxed);
            CHECK_IN_PROGRESS.store(true, Ordering::Relaxed);
            status_label.set_text("Checking mail...");

            // Create per-account task lines (on main thread, like the C version)
            let m = mailer.clone();
            let acct_count = 1 + m.count_accounts();
            let mut task_ids: Vec<(i32, i32)> = Vec::new(); // (account_index, task_id)
            for i in 0..acct_count {
                if let Some(acct) = m.get_account_info(i) {
                    if acct.enabled && !acct.server.is_empty() {
                        let name = if acct.name.is_empty() { acct.email.clone() } else { acct.name.clone() };
                        let tid = crate::tasks::add_detailed_task(
                            "mail-inbox-symbolic",
                            &name,
                            &format!("Connecting to {}...", acct.server),
                        );
                        task_ids.push((i, tid));
                    }
                }
            }
            // Fallback if no per-account tasks (no accounts configured)
            let fallback_tid = if task_ids.is_empty() {
                Some(crate::tasks::add_task("mail-inbox-symbolic", "Checking mail..."))
            } else {
                None
            };

            std::thread::spawn(move || {
                // One thread per account, parallel — like macmbx_mailer_check
                let task_map = Arc::new(task_ids);
                let mut total = 0i32;

                // Track which tasks succeeded vs failed
                let failed_ids: Arc<Mutex<Vec<(i32, String)>>> = Arc::new(Mutex::new(Vec::new()));

                if task_map.is_empty() {
                    // No accounts — just run check() which handles it
                    if let Ok(n) = m.check() {
                        total = n;
                    }
                } else {
                    std::thread::scope(|s| {
                        let handles: Vec<_> = task_map.iter().map(|&(acct_idx, task_id)| {
                            let mailer_ref = &m;
                            let failed = failed_ids.clone();
                            s.spawn(move || {
                                // Set thread-local so progress callback routes here
                                CURRENT_TASK_ID.with(|c| c.set(task_id));
                                let result = mailer_ref.check_account(acct_idx);
                                match &result {
                                    Ok(n) if *n > 0 => {
                                        crate::tasks::update_detailed_task(task_id,
                                            &format!("Done — {} new message(s)", n), 1.0);
                                    }
                                    Ok(_) => {
                                        crate::tasks::update_detailed_task(task_id,
                                            "No new mail", 1.0);
                                    }
                                    Err(e) => {
                                        let msg = format!("Failed: {}", e);
                                        crate::tasks::update_detailed_task(task_id, &msg, 1.0);
                                        failed.lock().unwrap().push((task_id, msg));
                                    }
                                }
                                result
                            })
                        }).collect();

                        for handle in handles {
                            match handle.join() {
                                Ok(Ok(n)) if n > 0 => total += n,
                                _ => {}
                            }
                        }
                    });
                }

                // Let results stay visible briefly before removing successful ones
                std::thread::sleep(std::time::Duration::from_secs(2));

                // Remove successful per-account tasks; keep failed as errors
                let failed = failed_ids.lock().unwrap();
                let failed_task_ids: Vec<i32> = failed.iter().map(|(id, _)| *id).collect();
                for &(_, task_id) in task_map.iter() {
                    if failed_task_ids.contains(&task_id) {
                        // Convert to sticky error — remove task, add error
                        crate::tasks::remove_task(task_id);
                        if let Some((_, ref msg)) = failed.iter().find(|(id, _)| *id == task_id) {
                            crate::tasks::add_error("dialog-error-symbolic", msg);
                        }
                    } else {
                        crate::tasks::remove_task(task_id);
                    }
                }
                if let Some(tid) = fallback_tid {
                    crate::tasks::remove_task(tid);
                }

                CHECK_IN_PROGRESS.store(false, Ordering::Relaxed);
                clear_bad_passwords();
                let msg = if total > 0 {
                    *STICKY_ERROR.lock().unwrap() = None;
                    NEED_REFRESH.store(true, Ordering::Relaxed);
                    let summary_id = crate::tasks::add_task("mail-unread-symbolic",
                        &format!("{} new message(s)", total));
                    // Auto-remove summary after 5 seconds
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    crate::tasks::remove_task(summary_id);
                    format!("{} new message(s)", total)
                } else {
                    *STICKY_ERROR.lock().unwrap() = None;
                    "No new mail".to_string()
                };
                *PENDING_STATUS.lock().unwrap() = Some(msg);
            });
        }

        if NEED_SEND.load(Ordering::Relaxed) && !SEND_IN_PROGRESS.load(Ordering::Relaxed) {
            NEED_SEND.store(false, Ordering::Relaxed);
            SEND_IN_PROGRESS.store(true, Ordering::Relaxed);
            status_label.set_text("Sending mail...");
            let task_id = crate::tasks::add_task("mail-send-symbolic", "Sending queued mail...");
            let m = mailer.clone();
            std::thread::spawn(move || {
                let result = m.send();
                match &result {
                    Ok(n) if *n > 0 => {
                        crate::tasks::update_task(task_id, &format!("{} message(s) sent", n));
                    }
                    Ok(_) => {
                        crate::tasks::update_task(task_id, "No queued messages");
                    }
                    Err(e) => {
                        crate::tasks::update_task(task_id, &format!("Send failed: {}", e));
                    }
                }

                // Keep visible briefly
                std::thread::sleep(std::time::Duration::from_secs(2));

                SEND_IN_PROGRESS.store(false, Ordering::Relaxed);
                let msg = match &result {
                    Ok(n) if *n > 0 => {
                        *STICKY_ERROR.lock().unwrap() = None;
                        crate::tasks::remove_task(task_id);
                        format!("{} message(s) sent", n)
                    }
                    Ok(_) => {
                        *STICKY_ERROR.lock().unwrap() = None;
                        crate::tasks::remove_task(task_id);
                        "Send complete".to_string()
                    }
                    Err(e) => {
                        // Convert to sticky error
                        let err = format!("Send failed: {}", e);
                        *STICKY_ERROR.lock().unwrap() = Some(err.clone());
                        crate::tasks::remove_task(task_id);
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
