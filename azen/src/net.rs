//! Network engine wiring for Azen: SMTP, POP3, IMAP protocol sessions via Crispy,
//! native TLS connector with certificate handling, credential lookup (macOS Keychain + Conf),
//! and Mailer creation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use macmbx::conf::Conf;
use macmbx::mailer::*;
use macmbx::toc::{MailboxType, Toc};

static ACCEPTED_HOSTS: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

pub fn is_host_accepted(host: &str) -> bool {
    let lock = ACCEPTED_HOSTS.lock().unwrap();
    lock.as_ref().map_or(false, |s| s.contains(host))
}

pub fn accept_host(host: &str) {
    let mut lock = ACCEPTED_HOSTS.lock().unwrap();
    if lock.is_none() {
        *lock = Some(std::collections::HashSet::new());
    }
    lock.as_mut().unwrap().insert(host.to_string());
}

pub fn load_trusted_hosts(data_dir: &Path) {
    let mut hosts = std::collections::HashSet::new();
    let p = data_dir.join("trusted_certs.txt");
    if let Ok(data) = fs::read_to_string(&p) {
        for line in data.lines() {
            let h = line.trim();
            if !h.is_empty() {
                hosts.insert(h.to_string());
            }
        }
    }
    let dir = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}/.local/share", home)
    });
    let p2 = PathBuf::from(dir).join("geudora").join("trusted_certs.txt");
    if let Ok(data) = fs::read_to_string(&p2) {
        for line in data.lines() {
            let h = line.trim();
            if !h.is_empty() {
                hosts.insert(h.to_string());
            }
        }
    }
    let mut lock = ACCEPTED_HOSTS.lock().unwrap();
    *lock = Some(hosts);
}

// ── Native TLS Connector ──

pub struct NativeTlsConnector;

impl crispy::transport::TlsConnector for NativeTlsConnector {
    fn connect(
        &self,
        domain: &str,
        stream: std::net::TcpStream,
        trusted_pem: Option<&str>,
    ) -> Result<Box<dyn crispy::transport::TlsStream>, crispy::transport::TlsError> {
        let mut builder = native_tls::TlsConnector::builder();

        if trusted_pem.is_some() || is_host_accepted(domain) {
            builder.danger_accept_invalid_certs(true);
        }

        let connector = builder.build().map_err(|e| crispy::transport::TlsError {
            message: e.to_string(),
            cert_pem: None,
        })?;

        match connector.connect(domain, stream) {
            Ok(tls_stream) => Ok(Box::new(NativeTlsStream(tls_stream))),
            Err(e) => {
                accept_host(domain);
                Err(crispy::transport::TlsError {
                    message: e.to_string(),
                    cert_pem: Some("ACCEPTED".to_string()),
                })
            }
        }
    }
}

pub struct NativeTlsStream(pub native_tls::TlsStream<std::net::TcpStream>);
unsafe impl Send for NativeTlsStream {}

impl std::io::Read for NativeTlsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}
impl std::io::Write for NativeTlsStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
impl crispy::transport::TlsStream for NativeTlsStream {
    fn tcp_stream(&self) -> Option<&std::net::TcpStream> {
        self.0.get_ref().into()
    }
}

pub fn new_transport() -> crispy::transport::TcpTransport {
    let mut t = crispy::transport::TcpTransport::new();
    t.set_tls_connector(Box::new(NativeTlsConnector));
    t.set_cert_callback(Box::new(|host: &str, _err: &str, _pem: &str| -> bool {
        accept_host(host);
        true
    }));
    t
}

// ── Crispy Protocol Wrappers ──

struct CrispySmtp(crispy::smtp::SmtpSession<crispy::transport::TcpTransport>);
unsafe impl Send for CrispySmtp {}

impl SmtpClient for CrispySmtp {
    fn set_proxy(&mut self, proxy: &ProxyConfig) {
        apply_crispy_proxy(&mut self.0.tp, proxy);
    }
    fn connect(&mut self, server: &str, port: u16, sec: SmtpSecurity) -> io::Result<()> {
        let s = match sec {
            SmtpSecurity::Plain => crispy::smtp::Security::Plain,
            SmtpSecurity::StartTls => crispy::smtp::Security::StartTls,
            SmtpSecurity::Ssl => crispy::smtp::Security::Ssl,
        };
        self.0.connect(server, port, s).map_err(io_err)
    }
    fn auth_plain(&mut self, u: &str, p: &str) -> io::Result<()> {
        self.0.auth_plain(u, p).map_err(io_err)
    }
    fn auth_login(&mut self, u: &str, p: &str) -> io::Result<()> {
        self.0.auth_login(u, p).map_err(io_err)
    }
    fn send_message(&mut self, from: &str, rcpts: &[&str], msg: &[u8]) -> io::Result<()> {
        self.0.send_message(from, rcpts, msg).map_err(io_err)
    }
    fn close(&mut self) {
        let _ = self.0.quit();
    }
}

struct CrispyPop3(crispy::pop3::Pop3Session<crispy::transport::TcpTransport>);
unsafe impl Send for CrispyPop3 {}

impl Pop3Client for CrispyPop3 {
    fn set_proxy(&mut self, proxy: &ProxyConfig) {
        apply_crispy_proxy(&mut self.0.tp, proxy);
    }
    fn connect(&mut self, server: &str, port: u16, sec: Pop3Security) -> io::Result<()> {
        let s = match sec {
            Pop3Security::Plain => crispy::pop3::Pop3Security::Plain,
            Pop3Security::Stls => crispy::pop3::Pop3Security::Stls,
            Pop3Security::Ssl => crispy::pop3::Pop3Security::Ssl,
        };
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
        Ok(msgs
            .into_iter()
            .map(|m| Pop3MsgInfo {
                number: m.number,
                size: m.size,
                uidl: m.uidl,
            })
            .collect())
    }
    fn retrieve(&mut self, n: u32) -> io::Result<Vec<u8>> {
        self.0.retrieve(n).map_err(io_err)
    }
    fn delete(&mut self, n: u32) -> io::Result<()> {
        self.0.delete(n).map_err(io_err)
    }
    fn close(&mut self) {
        self.0.quit();
    }
}

struct CrispyImap(crispy::imap::ImapSession<crispy::transport::TcpTransport>);
unsafe impl Send for CrispyImap {}

impl ImapClient for CrispyImap {
    fn set_proxy(&mut self, proxy: &ProxyConfig) {
        apply_crispy_proxy(&mut self.0.tp, proxy);
    }
    fn connect(&mut self, server: &str, port: u16, sec: ImapSecurity) -> io::Result<()> {
        let s = match sec {
            ImapSecurity::Plain => crispy::imap::ImapSecurity::Plain,
            ImapSecurity::StartTls => crispy::imap::ImapSecurity::StartTls,
            ImapSecurity::Ssl => crispy::imap::ImapSecurity::Ssl,
        };
        self.0.connect(server, port, s).map_err(io_err)
    }
    fn login(&mut self, u: &str, p: &str) -> io::Result<()> {
        self.0.login(u, p).map_err(io_err)
    }
    fn select(&mut self, mb: &str) -> io::Result<ImapMailboxInfo> {
        self.0.select(mb).map_err(io_err)?;
        let s = &self.0.selected;
        Ok(ImapMailboxInfo {
            exists: s.exists,
            uidvalidity: s.uidvalidity,
            uidnext: s.uidnext,
        })
    }
    fn fetch_flags(&mut self, uid_set: &str) -> io::Result<Vec<(u64, ImapFlags)>> {
        let r = self.0.fetch_flags(uid_set).map_err(io_err)?;
        Ok(r.into_iter()
            .map(|(uid, f)| {
                (
                    uid,
                    ImapFlags {
                        seen: f.seen,
                        answered: f.answered,
                        flagged: f.flagged,
                        deleted: f.deleted,
                        draft: f.draft,
                    },
                )
            })
            .collect())
    }
    fn fetch_message(&mut self, uid: u64) -> io::Result<Vec<u8>> {
        self.0.fetch_body(uid).map_err(io_err)
    }
    fn fetch_section(&mut self, uid: u64, sec: &str) -> io::Result<Vec<u8>> {
        self.0.fetch_section(uid, sec).map_err(io_err)
    }
    fn idle(&mut self, timeout_ms: u32) -> io::Result<String> {
        self.0.idle(timeout_ms).map_err(io_err)?;
        Ok(String::new())
    }
    fn noop(&mut self) -> io::Result<()> {
        self.0.noop().map_err(io_err)
    }
    fn close(&mut self) {
        self.0.close();
    }
    fn is_connected(&self) -> bool {
        self.0.connected
    }
}

fn apply_crispy_proxy(tp: &mut crispy::transport::TcpTransport, proxy: &ProxyConfig) {
    let cp = if proxy.proxy_type == "socks5" {
        crispy::transport::ProxyConfig::socks5(&proxy.host, proxy.port)
    } else {
        crispy::transport::ProxyConfig::http(&proxy.host, proxy.port)
    };
    let cp = if let (Some(u), Some(p)) = (&proxy.username, &proxy.password) {
        cp.with_auth(u, p)
    } else {
        cp
    };
    tp.set_proxy(cp);
}

fn io_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

pub struct CrispyFactory;

impl SessionFactory for CrispyFactory {
    fn create_smtp(&self) -> Box<dyn SmtpClient> {
        Box::new(CrispySmtp(crispy::smtp::SmtpSession::new(
            new_transport(),
            "localhost",
        )))
    }
    fn create_pop3(&self) -> Box<dyn Pop3Client> {
        Box::new(CrispyPop3(crispy::pop3::Pop3Session::new(new_transport())))
    }
    fn create_imap(&self) -> Box<dyn ImapClient> {
        Box::new(CrispyImap(crispy::imap::ImapSession::new(new_transport())))
    }
}

// ── Providers ──

pub struct RealConf(pub Conf);

impl ConfProvider for RealConf {
    fn get_dominant(&self) -> Option<Account> {
        let mut a = self.0.get_dominant();
        if a.server.is_empty() {
            if let Some(srv) = self.0.get("Settings", "MailServer") {
                a.server = srv.to_string();
            }
        }
        if a.smtp_server.is_empty() {
            if let Some(srv) = self.0.get("Settings", "SMTPServer") {
                a.smtp_server = srv.to_string();
            }
        }
        if a.username.is_empty() {
            if let Some(u) = self.0.get("Settings", "LoginName") {
                a.username = u.to_string();
            }
        }
        if a.email.is_empty() {
            if let Some(e) = self.0.get("Settings", "ReturnAddress") {
                a.email = e.to_string();
            }
        }
        Some(Account {
            index: 0,
            name: if a.name.is_empty() {
                a.email.clone()
            } else {
                a.name
            },
            email: a.email,
            username: a.username,
            server: a.server,
            smtp_server: a.smtp_server,
            account_type: a.account_type,
            enabled: a.enabled,
            leave_on_server: a.leave_on_server,
            ssl_mode: a.ssl_mode as u8,
        })
    }
    fn get_account(&self, i: i32) -> Option<Account> {
        let a = if i == 0 {
            let mut dom = self.0.get_dominant();
            if dom.server.is_empty() {
                if let Some(srv) = self.0.get("Settings", "MailServer") {
                    dom.server = srv.to_string();
                }
            }
            if dom.smtp_server.is_empty() {
                if let Some(srv) = self.0.get("Settings", "SMTPServer") {
                    dom.smtp_server = srv.to_string();
                }
            }
            if dom.username.is_empty() {
                if let Some(u) = self.0.get("Settings", "LoginName") {
                    dom.username = u.to_string();
                }
            }
            if dom.email.is_empty() {
                if let Some(e) = self.0.get("Settings", "ReturnAddress") {
                    dom.email = e.to_string();
                }
            }
            Some(dom)
        } else {
            self.0.get_account(i)
        }?;
        Some(Account {
            index: a.index,
            name: if a.name.is_empty() {
                a.email.clone()
            } else {
                a.name
            },
            email: a.email,
            username: a.username,
            server: a.server,
            smtp_server: a.smtp_server,
            account_type: a.account_type,
            enabled: a.enabled,
            leave_on_server: a.leave_on_server,
            ssl_mode: a.ssl_mode as u8,
        })
    }
    fn count_accounts(&self) -> i32 {
        self.0.count_accounts() as i32
    }
    fn get(&self, s: &str, k: &str) -> Option<String> {
        self.0.get(s, k).map(|s| s.to_string())
    }
}

pub struct RealStore(pub PathBuf);

impl StoreProvider for RealStore {
    fn find_special(&self, t: MailboxType) -> Option<PathBuf> {
        let n = match t {
            MailboxType::In => "In",
            MailboxType::Out => "Out",
            MailboxType::Trash => "Trash",
            MailboxType::Junk => "Junk",
            _ => return None,
        };
        let p = self.0.join(n);
        Some(p)
    }
    fn base_path(&self) -> &Path {
        &self.0
    }
    fn root_path(&self) -> &Path {
        &self.0
    }
}

pub struct RealPostProcessor {
    pub filter_path: PathBuf,
    pub mailboxes_dir: PathBuf,
}

impl PostProcessor for RealPostProcessor {
    fn apply_filters(&self, toc: &mut Toc) {
        if self.filter_path.exists() {
            if let Ok(mut fs) = macmbx::filter::FilterSet::load(&self.filter_path) {
                fs.compile();
                let reader = macmbx::filter::SummaryOnlyReader;
                fs.apply_all(toc, macmbx::filter::WHEN_INCOMING, &reader, None);
            }
        }
    }
    fn apply_junk(&self, toc: &mut Toc) {
        let cfg = macmbx::junk::JunkConfig::default();
        let read_headers = |t: &Toc, i: usize| -> Option<String> {
            macmbx::ops::read_headers(t, i).ok()
        };
        let read_body = |t: &Toc, i: usize| -> Option<String> {
            macmbx::ops::read_body(t, i)
                .ok()
                .map(|b| String::from_utf8_lossy(&b).to_string())
        };
        macmbx::junk::score_box(&cfg, toc, read_headers, read_body, None);
        if let Ok(store) = macmbx::store::Store::open(&self.mailboxes_dir) {
            let _ = macmbx::junk::move_spam(&cfg, toc, &store);
        }
    }
}

/// Create a configured Mailer instance backed by crispy protocols,
/// local mailbox store, post-processor filters, and keychain/conf credentials.
pub fn create_mailer(conf: &Conf, data_dir: &Path) -> Mailer {
    load_trusted_hosts(data_dir);
    let mb_dir = data_dir.join("mailboxes");
    let mut mailer = Mailer::new(
        Box::new(RealConf(conf.clone())),
        Box::new(RealStore(mb_dir.clone())),
        Box::new(CrispyFactory),
    );

    // Keychain and config credential lookup
    let conf_clone = conf.clone();
    mailer.set_credentials(Box::new(move |user: &str, server: &str| -> Option<String> {
        let key = format!("{}@{}", user, server);

        #[cfg(target_os = "macos")]
        {
            use security_framework::passwords::get_generic_password;
            if let Ok(pw_bytes) = get_generic_password("gEudora", &key) {
                if let Ok(pw) = String::from_utf8(pw_bytes) {
                    if !pw.is_empty() {
                        return Some(pw);
                    }
                }
            }
            if let Ok(pw_bytes) = get_generic_password("Eudora", &key) {
                if let Ok(pw) = String::from_utf8(pw_bytes) {
                    if !pw.is_empty() {
                        return Some(pw);
                    }
                }
            }
            if let Ok(pw_bytes) = get_generic_password("gEudora", user) {
                if let Ok(pw) = String::from_utf8(pw_bytes) {
                    if !pw.is_empty() {
                        return Some(pw);
                    }
                }
            }
        }

        if let Some(pw) = conf_clone
            .get("checking_mail", "saved_password")
            .or_else(|| conf_clone.get("checking_mail", "password"))
            .or_else(|| conf_clone.get("Settings", "Password"))
            .or_else(|| conf_clone.get("Settings", "SavePasswordText"))
        {
            if !pw.is_empty() {
                return Some(pw.to_string());
            }
        }

        None
    }));

    let filter_path = data_dir.join("Filters");
    mailer.set_post_processor(Box::new(RealPostProcessor {
        filter_path,
        mailboxes_dir: mb_dir,
    }));

    mailer
}
