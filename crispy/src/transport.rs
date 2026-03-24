//! TLS-capable TCP transport layer.
//!
//! Ported from `crispy_transport.c` / `crispy_transport.h`.
//!
//! Provides:
//! - `Transport` trait: pluggable abstraction used by SMTP, POP3, IMAP modules
//! - `TcpTransport`: concrete implementation over `std::net::TcpStream`
//! - TLS upgrade (STARTTLS) and direct SSL via pluggable `TlsConnector` trait
//! - Buffered line-based I/O (`recv_line`, `send`)
//! - Timeout support
//! - Certificate verification callback
//! - File-based certificate store (`CertStore`)

use std::cell::RefCell;
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Transport trait (protocol-facing abstraction)
// ---------------------------------------------------------------------------

/// Pluggable transport for mail protocol sessions.
///
/// All methods use `&mut self` so the transport can hold mutable socket state.
/// Implementations must handle buffering; `recv_line` returns one CRLF-terminated
/// line at a time.
pub trait Transport {
    /// Open a TCP connection to `host:port`.
    fn connect(&mut self, host: &str, port: u16) -> io::Result<()>;

    /// Send `data` in its entirety.
    fn send(&mut self, data: &[u8]) -> io::Result<()>;

    /// Read one CRLF-terminated line (including the CRLF).
    /// Returns the line as bytes.  The caller is responsible for stripping CRLF.
    fn recv_line(&mut self) -> io::Result<Vec<u8>>;

    /// Upgrade the connection to TLS (STARTTLS).
    fn start_tls(&mut self) -> io::Result<()>;

    /// Close the connection (but don't destroy the transport).
    fn close(&mut self);
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during transport operations.
#[derive(Debug)]
pub enum TransportError {
    /// An I/O error from the underlying stream.
    Io(io::Error),
    /// DNS resolution failed or no addresses returned.
    DnsResolution(String),
    /// TLS handshake or upgrade failed.
    Tls(String),
    /// Certificate verification failed and was rejected.
    CertRejected,
    /// No TLS connector was provided but TLS was requested.
    NoTlsConnector,
    /// The transport is not connected.
    NotConnected,
    /// A line exceeded the maximum buffer size.
    LineTooLong,
    /// The read timed out.
    Timeout,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Io(e) => write!(f, "I/O error: {}", e),
            TransportError::DnsResolution(host) => {
                write!(f, "DNS resolution failed for: {}", host)
            }
            TransportError::Tls(msg) => write!(f, "TLS error: {}", msg),
            TransportError::CertRejected => write!(f, "certificate rejected"),
            TransportError::NoTlsConnector => write!(f, "no TLS connector configured"),
            TransportError::NotConnected => write!(f, "transport not connected"),
            TransportError::LineTooLong => write!(f, "line exceeded maximum buffer size"),
            TransportError::Timeout => write!(f, "operation timed out"),
        }
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TransportError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for TransportError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::TimedOut {
            TransportError::Timeout
        } else {
            TransportError::Io(e)
        }
    }
}

impl From<TransportError> for io::Error {
    fn from(e: TransportError) -> io::Error {
        match e {
            TransportError::Io(e) => e,
            TransportError::Timeout => io::Error::new(io::ErrorKind::TimedOut, "timed out"),
            TransportError::NotConnected => {
                io::Error::new(io::ErrorKind::NotConnected, "transport not connected")
            }
            other => io::Error::new(io::ErrorKind::Other, other.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, TransportError>;

// ---------------------------------------------------------------------------
// Proxy configuration
// ---------------------------------------------------------------------------

/// Proxy type for tunneling connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyType {
    /// HTTP CONNECT proxy (RFC 7231 §4.3.6).
    HttpConnect,
    /// SOCKS5 proxy (RFC 1928).
    Socks5,
}

/// Optional proxy configuration for tunneling mail connections.
///
/// When set on a `TcpTransport`, the transport connects to the proxy first,
/// establishes a tunnel to the target host:port, then proceeds with the
/// mail protocol handshake (including TLS if configured).
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub proxy_type: ProxyType,
    pub host: String,
    pub port: u16,
    /// Optional username for proxy authentication.
    pub username: Option<String>,
    /// Optional password for proxy authentication.
    pub password: Option<String>,
}

impl ProxyConfig {
    /// Create an HTTP CONNECT proxy config.
    pub fn http(host: &str, port: u16) -> Self {
        Self {
            proxy_type: ProxyType::HttpConnect,
            host: host.to_string(),
            port,
            username: None,
            password: None,
        }
    }

    /// Create a SOCKS5 proxy config.
    pub fn socks5(host: &str, port: u16) -> Self {
        Self {
            proxy_type: ProxyType::Socks5,
            host: host.to_string(),
            port,
            username: None,
            password: None,
        }
    }

    /// Set proxy authentication credentials.
    pub fn with_auth(mut self, username: &str, password: &str) -> Self {
        self.username = Some(username.to_string());
        self.password = Some(password.to_string());
        self
    }
}

// ---------------------------------------------------------------------------
// Security mode
// ---------------------------------------------------------------------------

/// How TLS should be established for the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityMode {
    /// Plain text, no encryption.
    None,
    /// Connect plain, then upgrade to TLS via STARTTLS command.
    StartTls,
    /// Connect directly over TLS (implicit TLS, e.g. port 465/993/995).
    DirectTls,
}

impl SecurityMode {
    /// Infer security mode from a port number, matching the C behaviour.
    ///
    /// Ports 465 (SMTPS), 993 (IMAPS), and 995 (POP3S) use direct TLS.
    /// All other ports default to `None` (caller can override to `StartTls`).
    pub fn from_port(port: u16) -> Self {
        match port {
            465 | 993 | 995 => SecurityMode::DirectTls,
            _ => SecurityMode::None,
        }
    }

    /// Parse from a string representation.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "none" | "plain" => Some(SecurityMode::None),
            "starttls" | "start_tls" | "start-tls" => Some(SecurityMode::StartTls),
            "tls" | "ssl" | "directtls" | "direct_tls" | "direct-tls" => {
                Some(SecurityMode::DirectTls)
            }
            _ => Option::None,
        }
    }
}

impl fmt::Display for SecurityMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecurityMode::None => write!(f, "none"),
            SecurityMode::StartTls => write!(f, "starttls"),
            SecurityMode::DirectTls => write!(f, "directtls"),
        }
    }
}

// ---------------------------------------------------------------------------
// TLS abstraction traits
// ---------------------------------------------------------------------------

/// A TLS stream that supports both reading and writing.
///
/// Implementors wrap a `TcpStream` with a TLS layer (e.g. rustls, native-tls).
pub trait TlsStream: Read + Write + Send {
    /// Return a reference to the underlying TCP stream, if accessible.
    fn tcp_stream(&self) -> Option<&TcpStream> {
        Option::None
    }
}

/// Factory for creating TLS connections over an existing TCP stream.
///
/// Plug in rustls or native-tls by implementing this trait.
pub trait TlsConnector: Send {
    /// Perform a TLS handshake over `stream`, verifying against `domain`.
    ///
    /// If `trusted_pem` is `Some`, the connector should trust that certificate
    /// in addition to system roots.
    fn connect(
        &self,
        domain: &str,
        stream: TcpStream,
        trusted_pem: Option<&str>,
    ) -> std::result::Result<Box<dyn TlsStream>, TlsError>;
}

/// Errors from the TLS connector.
#[derive(Debug)]
pub struct TlsError {
    /// Human-readable description (e.g. "self-signed certificate").
    pub message: String,
    /// The server certificate in PEM format, if available.
    pub cert_pem: Option<String>,
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for TlsError {}

// ---------------------------------------------------------------------------
// Certificate callback
// ---------------------------------------------------------------------------

/// Called when a server certificate fails validation.
///
/// Parameters:
/// - `host`: the server hostname
/// - `error`: human-readable error string
/// - `cert_pem`: the certificate in PEM format
///
/// Return `true` to accept the certificate, `false` to reject.
pub type CertCallback = Box<dyn Fn(&str, &str, &str) -> bool + Send>;

// ---------------------------------------------------------------------------
// Internal stream enum
// ---------------------------------------------------------------------------

/// The active stream, either plain TCP or TLS-wrapped.
enum Stream {
    Plain(TcpStream),
    Tls(Box<dyn TlsStream>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

impl Stream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.set_read_timeout(timeout),
            Stream::Tls(s) => {
                if let Some(tcp) = s.tcp_stream() {
                    tcp.set_read_timeout(timeout)
                } else {
                    Ok(())
                }
            }
        }
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.set_write_timeout(timeout),
            Stream::Tls(s) => {
                if let Some(tcp) = s.tcp_stream() {
                    tcp.set_write_timeout(timeout)
                } else {
                    Ok(())
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared TLS stream wrappers (for split read/write)
// ---------------------------------------------------------------------------

struct SharedTlsRead(Rc<RefCell<Box<dyn TlsStream>>>);
struct SharedTlsWrite(Rc<RefCell<Box<dyn TlsStream>>>);

impl Read for SharedTlsRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.borrow_mut().read(buf)
    }
}

impl Write for SharedTlsRead {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.borrow_mut().flush()
    }
}

impl Read for SharedTlsWrite {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.borrow_mut().read(buf)
    }
}

impl Write for SharedTlsWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.borrow_mut().flush()
    }
}

// Safety: TcpTransport is used single-threaded (matching the C code).
// The Rc<RefCell<>> is never shared across threads.
unsafe impl Send for SharedTlsRead {}
unsafe impl Send for SharedTlsWrite {}

impl TlsStream for SharedTlsRead {}
impl TlsStream for SharedTlsWrite {}

// ---------------------------------------------------------------------------
// TcpTransport — concrete implementation
// ---------------------------------------------------------------------------

/// Maximum line length for `recv_line` (matches C readbuf of 4096).
const MAX_LINE_LEN: usize = 4096;

/// A TCP transport with optional TLS, buffered line-based I/O, and timeouts.
///
/// This is the Rust equivalent of the C `SmtpTransport` + `PosixCtx`.
/// It implements the `Transport` trait so it can be used by SMTP, POP3,
/// and IMAP protocol modules.
pub struct TcpTransport {
    hostname: String,
    port: u16,
    security: SecurityMode,
    reader: Option<BufReader<Stream>>,
    writer: Option<Stream>,
    tls_connector: Option<Box<dyn TlsConnector>>,
    cert_callback: Option<CertCallback>,
    trusted_pem: Option<String>,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
    proxy: Option<ProxyConfig>,
}

impl TcpTransport {
    /// Create a new, unconnected transport.
    pub fn new() -> Self {
        TcpTransport {
            hostname: String::new(),
            port: 0,
            security: SecurityMode::None,
            reader: Option::None,
            writer: Option::None,
            tls_connector: Option::None,
            cert_callback: Option::None,
            trusted_pem: Option::None,
            read_timeout: Option::None,
            write_timeout: Option::None,
            proxy: None,
        }
    }

    /// Set the security mode. Must be called before `connect`.
    pub fn set_security(&mut self, mode: SecurityMode) {
        self.security = mode;
    }

    /// Set the TLS connector implementation (e.g. rustls or native-tls wrapper).
    pub fn set_tls_connector(&mut self, connector: Box<dyn TlsConnector>) {
        self.tls_connector = Some(connector);
    }

    /// Set the certificate verification callback.
    ///
    /// Called when a certificate fails validation. If the callback returns
    /// `true`, the certificate is accepted.
    pub fn set_cert_callback(&mut self, cb: CertCallback) {
        self.cert_callback = Some(cb);
    }

    /// Load a previously trusted certificate PEM for this connection.
    /// Must be called before `connect`.
    pub fn load_trusted(&mut self, pem: String) {
        self.trusted_pem = Some(pem);
    }

    /// Set the read timeout. `None` means block indefinitely.
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) {
        self.read_timeout = timeout;
        if let Some(ref reader) = self.reader {
            let _ = reader.get_ref().set_read_timeout(timeout);
        }
    }

    /// Set an optional proxy. When set, `connect()` tunnels through the proxy.
    pub fn set_proxy(&mut self, proxy: ProxyConfig) {
        self.proxy = Some(proxy);
    }

    /// Set the write timeout. `None` means block indefinitely.
    pub fn set_write_timeout(&mut self, timeout: Option<Duration>) {
        self.write_timeout = timeout;
        if let Some(ref writer) = self.writer {
            let _ = writer.set_write_timeout(timeout);
        }
    }

    /// Check if the transport is currently connected.
    pub fn is_connected(&self) -> bool {
        self.reader.is_some()
    }

    /// Check if the transport is using TLS.
    pub fn is_tls(&self) -> bool {
        if let Some(ref writer) = self.writer {
            matches!(writer, Stream::Tls(_))
        } else {
            false
        }
    }

    /// Read a line from the server into a String (including trailing CRLF/LF).
    pub fn read_line(&mut self, buf: &mut String) -> Result<usize> {
        let reader = self.reader.as_mut().ok_or(TransportError::NotConnected)?;
        buf.clear();
        let n = reader.read_line(buf).map_err(TransportError::from)?;
        if n == 0 {
            return Err(TransportError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed",
            )));
        }
        if n > MAX_LINE_LEN {
            return Err(TransportError::LineTooLong);
        }
        Ok(n)
    }

    /// Write a line to the server, appending `\r\n` if not already present.
    pub fn write_line(&mut self, line: &str) -> Result<()> {
        let formatted = format_line(line);
        self.write_all(formatted.as_bytes())
    }

    /// Write raw data to the server, looping until all bytes are sent.
    pub fn write_all(&mut self, data: &[u8]) -> Result<()> {
        let writer = self.writer.as_mut().ok_or(TransportError::NotConnected)?;
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    /// Return the hostname of the current connection.
    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    // -- private helpers --

    /// Set up a plain TCP connection with read/write split.
    fn setup_plain(&mut self, tcp: TcpStream) -> Result<()> {
        let writer_tcp = tcp.try_clone()?;
        self.reader = Some(BufReader::with_capacity(MAX_LINE_LEN, Stream::Plain(tcp)));
        self.writer = Some(Stream::Plain(writer_tcp));
        Ok(())
    }

    /// Perform TLS handshake over a raw TCP stream and install the TLS
    /// stream as both reader and writer.
    fn setup_tls_from_tcp(&mut self, tcp: TcpStream) -> Result<()> {
        let connector = self
            .tls_connector
            .as_ref()
            .ok_or(TransportError::NoTlsConnector)?;

        let trusted = self.trusted_pem.as_deref();

        match connector.connect(&self.hostname, tcp, trusted) {
            Ok(tls_stream) => {
                self.install_tls_stream(tls_stream);
                Ok(())
            }
            Err(tls_err) => {
                // Certificate failed verification -- try the callback
                if let (Some(ref cb), Some(ref pem)) =
                    (&self.cert_callback, &tls_err.cert_pem)
                {
                    if cb(&self.hostname, &tls_err.message, pem) {
                        // User accepted the cert. Store it as trusted so a
                        // retry will succeed.
                        self.trusted_pem = Some(pem.clone());
                        return Err(TransportError::Tls(format!(
                            "certificate accepted by user, retry connection ({})",
                            tls_err.message
                        )));
                    }
                }
                Err(TransportError::CertRejected)
            }
        }
    }

    /// Install a TLS stream using Rc<RefCell<>> shared ownership so both
    /// the reader and writer can access the same underlying TLS stream.
    fn install_tls_stream(&mut self, stream: Box<dyn TlsStream>) {
        let shared = Rc::new(RefCell::new(stream));
        let read_half = SharedTlsRead(Rc::clone(&shared));
        let write_half = SharedTlsWrite(Rc::clone(&shared));
        self.reader = Some(BufReader::with_capacity(
            MAX_LINE_LEN,
            Stream::Tls(Box::new(read_half)),
        ));
        self.writer = Some(Stream::Tls(Box::new(write_half)));
    }
}

impl Default for TcpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TcpTransport {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------------
// Transport trait implementation for TcpTransport
// ---------------------------------------------------------------------------

impl Transport for TcpTransport {
    fn connect(&mut self, host: &str, port: u16) -> io::Result<()> {
        self.hostname = host.to_string();
        self.port = port;

        // Connect: direct or through proxy
        let tcp = if let Some(ref proxy) = self.proxy {
            let proxy_addr = format!("{}:{}", proxy.host, proxy.port);
            let mut tcp = TcpStream::connect(&proxy_addr)?;
            match proxy.proxy_type {
                ProxyType::HttpConnect => {
                    proxy_http_connect(&mut tcp, host, port,
                        proxy.username.as_deref(), proxy.password.as_deref())?;
                }
                ProxyType::Socks5 => {
                    proxy_socks5(&mut tcp, host, port,
                        proxy.username.as_deref(), proxy.password.as_deref())?;
                }
            }
            tcp
        } else {
            let addr = format!("{}:{}", host, port);
            TcpStream::connect(&addr)?
        };

        // Apply timeouts
        if let Some(t) = self.read_timeout {
            tcp.set_read_timeout(Some(t))?;
        }
        if let Some(t) = self.write_timeout {
            tcp.set_write_timeout(Some(t))?;
        }

        // Determine effective security: if caller set a mode use that,
        // otherwise infer from port (matching C: ports 465/993/995 do
        // immediate TLS).
        let security = if self.security != SecurityMode::None {
            self.security
        } else {
            SecurityMode::from_port(port)
        };

        if security == SecurityMode::DirectTls {
            match self.setup_tls_from_tcp(tcp) {
                Ok(()) => {}
                Err(TransportError::Tls(msg)) if msg.contains("retry connection") => {
                    // User accepted cert — trusted_pem is now set. Reconnect and retry.
                    let tcp2 = if let Some(ref proxy) = self.proxy {
                        let pa = format!("{}:{}", proxy.host, proxy.port);
                        let mut t = TcpStream::connect(&pa)?;
                        match proxy.proxy_type {
                            ProxyType::HttpConnect => proxy_http_connect(&mut t, host, port,
                                proxy.username.as_deref(), proxy.password.as_deref())?,
                            ProxyType::Socks5 => proxy_socks5(&mut t, host, port,
                                proxy.username.as_deref(), proxy.password.as_deref())?,
                        }
                        t
                    } else {
                        TcpStream::connect(format!("{}:{}", host, port))?
                    };
                    if let Some(t) = self.read_timeout { tcp2.set_read_timeout(Some(t))?; }
                    if let Some(t) = self.write_timeout { tcp2.set_write_timeout(Some(t))?; }
                    self.setup_tls_from_tcp(tcp2).map_err(io::Error::from)?;
                }
                Err(e) => return Err(io::Error::from(e)),
            }
        } else {
            self.setup_plain(tcp).map_err(io::Error::from)?;
        }

        Ok(())
    }

    fn send(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_all(data).map_err(io::Error::from)
    }

    fn recv_line(&mut self) -> io::Result<Vec<u8>> {
        let reader = self
            .reader
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "not connected"))?;
        let mut buf = Vec::with_capacity(256);
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed",
            ));
        }
        if n > MAX_LINE_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "line exceeded maximum buffer size",
            ));
        }
        Ok(buf)
    }

    fn start_tls(&mut self) -> io::Result<()> {
        if self.reader.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "not connected",
            ));
        }

        // Take ownership of the reader's inner stream
        let reader = self.reader.take().unwrap();
        let stream = reader.into_inner();

        let tcp = match stream {
            Stream::Plain(s) => s,
            Stream::Tls(_) => {
                // Already TLS — matches C: "if (c->ssl) return 0"
                return Ok(());
            }
        };

        // Drop the old writer
        self.writer.take();

        match self.setup_tls_from_tcp(tcp) {
            Ok(()) => Ok(()),
            Err(TransportError::Tls(msg)) if msg.contains("retry connection") => {
                // User accepted cert — trusted_pem is now set.
                // For STARTTLS, we must reconnect TCP and redo TLS directly
                // (the protocol layer will need to reconnect too).
                // Signal the caller to retry the entire connection.
                Err(io::Error::new(io::ErrorKind::ConnectionReset,
                    "certificate accepted, please reconnect"))
            }
            Err(e) => Err(io::Error::from(e)),
        }
    }

    fn close(&mut self) {
        // Drop writer first (for TLS this may send close_notify)
        self.writer.take();
        self.reader.take();
        self.hostname.clear();
    }
}

// ---------------------------------------------------------------------------
// Certificate store
// ---------------------------------------------------------------------------

/// File-based certificate store: one PEM file per host in a directory.
///
/// Port of `crispy_cert_store_*` functions from C.
pub struct CertStore {
    dir: PathBuf,
}

impl CertStore {
    /// Create a new certificate store rooted at `dir`.
    ///
    /// The directory is created if it does not exist.
    pub fn new(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(CertStore { dir })
    }

    /// Open an existing store directory (does not create it).
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        CertStore { dir: dir.into() }
    }

    /// Path to the PEM file for a given host.
    fn cert_path(&self, host: &str) -> PathBuf {
        self.dir.join(format!("{}.pem", host))
    }

    /// Save a trusted certificate PEM for a host.
    pub fn save(&self, host: &str, cert_pem: &str) -> io::Result<()> {
        fs::write(self.cert_path(host), cert_pem)
    }

    /// Load a trusted certificate for a host. Returns `None` if not found.
    pub fn load(&self, host: &str) -> io::Result<Option<String>> {
        let path = self.cert_path(host);
        if path.exists() {
            Ok(Some(fs::read_to_string(path)?))
        } else {
            Ok(Option::None)
        }
    }

    /// Delete a trusted certificate for a host.
    pub fn delete(&self, host: &str) -> io::Result<()> {
        let path = self.cert_path(host);
        if path.exists() {
            fs::remove_file(path)
        } else {
            Ok(())
        }
    }

    /// List all trusted hostnames (derived from `.pem` filenames).
    pub fn list(&self) -> io::Result<Vec<String>> {
        let mut hosts = Vec::new();
        if !self.dir.exists() {
            return Ok(hosts);
        }
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(host) = name.strip_suffix(".pem") {
                hosts.push(host.to_string());
            }
        }
        hosts.sort();
        Ok(hosts)
    }

    /// Return the store directory path.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Format a protocol line with `\r\n` termination.
///
/// - If the input already ends with `\r\n`, returns it unchanged.
/// - If it ends with bare `\n`, replaces with `\r\n`.
/// - Otherwise appends `\r\n`.
pub fn format_line(line: &str) -> String {
    if line.ends_with("\r\n") {
        line.to_string()
    } else if line.ends_with('\n') {
        format!("{}\r\n", &line[..line.len() - 1])
    } else {
        format!("{}\r\n", line)
    }
}

/// Strip `\r\n` or `\n` from the end of a server response line.
pub fn strip_crlf(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

// ---------------------------------------------------------------------------
// Proxy tunneling — HTTP CONNECT (RFC 7231 §4.3.6)
// ---------------------------------------------------------------------------

/// Establish an HTTP CONNECT tunnel through the proxy to target host:port.
fn proxy_http_connect(
    tcp: &mut TcpStream,
    target_host: &str,
    target_port: u16,
    username: Option<&str>,
    password: Option<&str>,
) -> io::Result<()> {
    let mut req = format!(
        "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n",
        target_host, target_port, target_host, target_port
    );

    // Proxy-Authorization: Basic base64(user:pass)
    if let (Some(user), Some(pass)) = (username, password) {
        let creds = crate::base64::encode(format!("{}:{}", user, pass).as_bytes());
        req.push_str(&format!("Proxy-Authorization: Basic {}\r\n", creds));
    }

    req.push_str("\r\n");
    tcp.write_all(req.as_bytes())?;
    tcp.flush()?;

    // Read the HTTP response status line
    let mut response = Vec::with_capacity(256);
    let mut buf = [0u8; 1];
    loop {
        let n = tcp.read(&mut buf)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "proxy closed connection"));
        }
        response.push(buf[0]);
        // End of headers: \r\n\r\n
        if response.len() >= 4 && &response[response.len() - 4..] == b"\r\n\r\n" {
            break;
        }
        if response.len() > 4096 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "proxy response too large"));
        }
    }

    let status = String::from_utf8_lossy(&response);
    // Expect "HTTP/1.x 200 ..." or "HTTP/1.x 2xx ..."
    if let Some(first_line) = status.lines().next() {
        let parts: Vec<&str> = first_line.splitn(3, ' ').collect();
        if parts.len() >= 2 {
            if let Ok(code) = parts[1].parse::<u16>() {
                if (200..300).contains(&code) {
                    return Ok(()); // Tunnel established
                }
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    format!("proxy CONNECT failed: {}", first_line),
                ));
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::InvalidData, "invalid proxy response"))
}

// ---------------------------------------------------------------------------
// Proxy tunneling — SOCKS5 (RFC 1928)
// ---------------------------------------------------------------------------

/// Establish a SOCKS5 tunnel through the proxy to target host:port.
fn proxy_socks5(
    tcp: &mut TcpStream,
    target_host: &str,
    target_port: u16,
    username: Option<&str>,
    password: Option<&str>,
) -> io::Result<()> {
    let has_auth = username.is_some() && password.is_some();

    // Step 1: Greeting — supported auth methods
    if has_auth {
        tcp.write_all(&[0x05, 0x02, 0x00, 0x02])?; // NO AUTH + USER/PASS
    } else {
        tcp.write_all(&[0x05, 0x01, 0x00])?; // NO AUTH only
    }
    tcp.flush()?;

    let mut resp = [0u8; 2];
    tcp.read_exact(&mut resp)?;
    if resp[0] != 0x05 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a SOCKS5 proxy"));
    }

    // Step 2: Authenticate if required
    if resp[1] == 0x02 {
        // Username/password auth (RFC 1929)
        let user = username.unwrap_or("");
        let pass = password.unwrap_or("");
        let mut auth = vec![0x01, user.len() as u8];
        auth.extend_from_slice(user.as_bytes());
        auth.push(pass.len() as u8);
        auth.extend_from_slice(pass.as_bytes());
        tcp.write_all(&auth)?;
        tcp.flush()?;

        let mut auth_resp = [0u8; 2];
        tcp.read_exact(&mut auth_resp)?;
        if auth_resp[1] != 0x00 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "SOCKS5 auth failed"));
        }
    } else if resp[1] != 0x00 {
        return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "SOCKS5: no acceptable auth"));
    }

    // Step 3: Connect request — DOMAINNAME address type
    let host_bytes = target_host.as_bytes();
    if host_bytes.len() > 255 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "hostname too long for SOCKS5"));
    }
    let mut connect = vec![
        0x05, // version
        0x01, // CONNECT
        0x00, // reserved
        0x03, // DOMAINNAME
        host_bytes.len() as u8,
    ];
    connect.extend_from_slice(host_bytes);
    connect.push((target_port >> 8) as u8);
    connect.push((target_port & 0xff) as u8);
    tcp.write_all(&connect)?;
    tcp.flush()?;

    // Step 4: Read response
    let mut hdr = [0u8; 4];
    tcp.read_exact(&mut hdr)?;
    if hdr[0] != 0x05 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad SOCKS5 response"));
    }
    if hdr[1] != 0x00 {
        let msg = match hdr[1] {
            0x01 => "general failure",
            0x02 => "connection not allowed by ruleset",
            0x03 => "network unreachable",
            0x04 => "host unreachable",
            0x05 => "connection refused by destination",
            0x06 => "TTL expired",
            0x07 => "command not supported",
            0x08 => "address type not supported",
            _ => "unknown SOCKS5 error",
        };
        return Err(io::Error::new(io::ErrorKind::ConnectionRefused,
            format!("SOCKS5 connect failed: {}", msg)));
    }

    // Skip bound address in response
    match hdr[3] {
        0x01 => { let mut skip = [0u8; 6]; tcp.read_exact(&mut skip)?; } // IPv4 + port
        0x03 => {
            let mut len = [0u8; 1];
            tcp.read_exact(&mut len)?;
            let mut skip = vec![0u8; len[0] as usize + 2]; // domain + port
            tcp.read_exact(&mut skip)?;
        }
        0x04 => { let mut skip = [0u8; 18]; tcp.read_exact(&mut skip)?; } // IPv6 + port
        _ => {}
    }

    Ok(()) // Tunnel established
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // -- SecurityMode tests --

    #[test]
    fn security_mode_from_port() {
        assert_eq!(SecurityMode::from_port(465), SecurityMode::DirectTls);
        assert_eq!(SecurityMode::from_port(993), SecurityMode::DirectTls);
        assert_eq!(SecurityMode::from_port(995), SecurityMode::DirectTls);
        assert_eq!(SecurityMode::from_port(25), SecurityMode::None);
        assert_eq!(SecurityMode::from_port(143), SecurityMode::None);
        assert_eq!(SecurityMode::from_port(587), SecurityMode::None);
        assert_eq!(SecurityMode::from_port(110), SecurityMode::None);
    }

    #[test]
    fn security_mode_parse() {
        assert_eq!(SecurityMode::parse("none"), Some(SecurityMode::None));
        assert_eq!(SecurityMode::parse("plain"), Some(SecurityMode::None));
        assert_eq!(SecurityMode::parse("PLAIN"), Some(SecurityMode::None));
        assert_eq!(SecurityMode::parse("starttls"), Some(SecurityMode::StartTls));
        assert_eq!(SecurityMode::parse("STARTTLS"), Some(SecurityMode::StartTls));
        assert_eq!(SecurityMode::parse("start_tls"), Some(SecurityMode::StartTls));
        assert_eq!(SecurityMode::parse("start-tls"), Some(SecurityMode::StartTls));
        assert_eq!(SecurityMode::parse("tls"), Some(SecurityMode::DirectTls));
        assert_eq!(SecurityMode::parse("ssl"), Some(SecurityMode::DirectTls));
        assert_eq!(SecurityMode::parse("SSL"), Some(SecurityMode::DirectTls));
        assert_eq!(SecurityMode::parse("directtls"), Some(SecurityMode::DirectTls));
        assert_eq!(SecurityMode::parse("direct_tls"), Some(SecurityMode::DirectTls));
        assert_eq!(SecurityMode::parse("direct-tls"), Some(SecurityMode::DirectTls));
        assert_eq!(SecurityMode::parse("bogus"), Option::None);
        assert_eq!(SecurityMode::parse(""), Option::None);
    }

    #[test]
    fn security_mode_display() {
        assert_eq!(SecurityMode::None.to_string(), "none");
        assert_eq!(SecurityMode::StartTls.to_string(), "starttls");
        assert_eq!(SecurityMode::DirectTls.to_string(), "directtls");
    }

    #[test]
    fn security_mode_roundtrip() {
        for mode in &[SecurityMode::None, SecurityMode::StartTls, SecurityMode::DirectTls] {
            let s = mode.to_string();
            assert_eq!(SecurityMode::parse(&s), Some(*mode));
        }
    }

    // -- format_line tests --

    #[test]
    fn format_line_bare() {
        assert_eq!(format_line("EHLO example.com"), "EHLO example.com\r\n");
    }

    #[test]
    fn format_line_already_crlf() {
        assert_eq!(format_line("EHLO example.com\r\n"), "EHLO example.com\r\n");
    }

    #[test]
    fn format_line_bare_lf() {
        assert_eq!(format_line("EHLO example.com\n"), "EHLO example.com\r\n");
    }

    #[test]
    fn format_line_empty() {
        assert_eq!(format_line(""), "\r\n");
    }

    #[test]
    fn format_line_dot_stuffing() {
        assert_eq!(format_line("."), ".\r\n");
    }

    // -- strip_crlf tests --

    #[test]
    fn strip_crlf_both() {
        assert_eq!(strip_crlf("250 OK\r\n"), "250 OK");
    }

    #[test]
    fn strip_crlf_lf_only() {
        assert_eq!(strip_crlf("250 OK\n"), "250 OK");
    }

    #[test]
    fn strip_crlf_none() {
        assert_eq!(strip_crlf("250 OK"), "250 OK");
    }

    #[test]
    fn strip_crlf_empty() {
        assert_eq!(strip_crlf(""), "");
    }

    #[test]
    fn strip_crlf_only_crlf() {
        assert_eq!(strip_crlf("\r\n"), "");
    }

    #[test]
    fn strip_crlf_only_lf() {
        assert_eq!(strip_crlf("\n"), "");
    }

    // -- BufReader line reading tests (simulated I/O) --

    #[test]
    fn buffered_read_line_single() {
        let data = b"220 mail.example.com ESMTP\r\n";
        let cursor = Cursor::new(data.to_vec());
        let mut reader = BufReader::new(cursor);
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap();
        assert_eq!(n, 28);
        assert_eq!(line, "220 mail.example.com ESMTP\r\n");
        assert_eq!(strip_crlf(&line), "220 mail.example.com ESMTP");
    }

    #[test]
    fn buffered_read_line_multiple() {
        let data = b"220 Hello\r\n250-SIZE\r\n250 OK\r\n";
        let cursor = Cursor::new(data.to_vec());
        let mut reader = BufReader::new(cursor);

        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(strip_crlf(&line), "220 Hello");

        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(strip_crlf(&line), "250-SIZE");

        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(strip_crlf(&line), "250 OK");
    }

    #[test]
    fn buffered_read_line_bare_lf() {
        let data = b"220 Hello\n250 OK\n";
        let cursor = Cursor::new(data.to_vec());
        let mut reader = BufReader::new(cursor);

        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(strip_crlf(&line), "220 Hello");

        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(strip_crlf(&line), "250 OK");
    }

    #[test]
    fn buffered_read_eof() {
        let data = b"";
        let cursor = Cursor::new(data.to_vec());
        let mut reader = BufReader::new(cursor);
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn buffered_read_line_bytes() {
        let data = b"220 Hello\r\n";
        let cursor = Cursor::new(data.to_vec());
        let mut reader = BufReader::new(cursor);
        let mut buf = Vec::new();
        let n = reader.read_until(b'\n', &mut buf).unwrap();
        assert_eq!(n, 11);
        assert_eq!(&buf, b"220 Hello\r\n");
    }

    // -- CertStore tests --

    #[test]
    fn cert_store_save_load_delete() {
        let dir = std::env::temp_dir().join("crispy_cert_test_sld");
        let _ = fs::remove_dir_all(&dir);

        let store = CertStore::new(&dir).unwrap();

        let pem = "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n";
        store.save("mail.example.com", pem).unwrap();

        let loaded = store.load("mail.example.com").unwrap();
        assert_eq!(loaded, Some(pem.to_string()));

        let missing = store.load("nonexistent.com").unwrap();
        assert_eq!(missing, Option::None);

        let hosts = store.list().unwrap();
        assert_eq!(hosts, vec!["mail.example.com".to_string()]);

        store.delete("mail.example.com").unwrap();
        let after = store.load("mail.example.com").unwrap();
        assert_eq!(after, Option::None);

        store.delete("nonexistent.com").unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cert_store_list_multiple() {
        let dir = std::env::temp_dir().join("crispy_cert_test_lm");
        let _ = fs::remove_dir_all(&dir);

        let store = CertStore::new(&dir).unwrap();
        store.save("alpha.com", "pem1").unwrap();
        store.save("beta.com", "pem2").unwrap();
        store.save("gamma.com", "pem3").unwrap();

        let hosts = store.list().unwrap();
        assert_eq!(hosts, vec!["alpha.com", "beta.com", "gamma.com"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cert_store_list_empty_dir() {
        let dir = std::env::temp_dir().join("crispy_cert_test_empty");
        let _ = fs::remove_dir_all(&dir);

        let store = CertStore::new(&dir).unwrap();
        let hosts = store.list().unwrap();
        assert!(hosts.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cert_store_list_nonexistent() {
        let store = CertStore::open("/tmp/crispy_cert_test_nonexistent_12345");
        let hosts = store.list().unwrap();
        assert!(hosts.is_empty());
    }

    #[test]
    fn cert_store_overwrite() {
        let dir = std::env::temp_dir().join("crispy_cert_test_overwrite");
        let _ = fs::remove_dir_all(&dir);

        let store = CertStore::new(&dir).unwrap();
        store.save("host.com", "old-pem").unwrap();
        store.save("host.com", "new-pem").unwrap();

        let loaded = store.load("host.com").unwrap();
        assert_eq!(loaded, Some("new-pem".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    // -- TcpTransport unit tests (no network) --

    #[test]
    fn transport_not_connected() {
        let mut tp = TcpTransport::new();
        assert!(!tp.is_connected());
        assert!(!tp.is_tls());

        let mut buf = String::new();
        match tp.read_line(&mut buf) {
            Err(TransportError::NotConnected) => {}
            other => panic!("expected NotConnected, got {:?}", other),
        }

        match tp.write_line("test") {
            Err(TransportError::NotConnected) => {}
            other => panic!("expected NotConnected, got {:?}", other),
        }
    }

    #[test]
    fn transport_close_when_not_connected() {
        let mut tp = TcpTransport::new();
        tp.close();
        assert!(!tp.is_connected());
    }

    #[test]
    fn transport_trait_start_tls_not_connected() {
        let mut tp = TcpTransport::new();
        let result = Transport::start_tls(&mut tp);
        assert!(result.is_err());
    }

    #[test]
    fn transport_default() {
        let tp = TcpTransport::default();
        assert!(!tp.is_connected());
        assert_eq!(tp.hostname(), "");
    }

    #[test]
    fn transport_set_timeouts_before_connect() {
        let mut tp = TcpTransport::new();
        tp.set_read_timeout(Some(Duration::from_secs(30)));
        tp.set_write_timeout(Some(Duration::from_secs(30)));
        // Should not panic
        assert!(!tp.is_connected());
    }

    #[test]
    fn transport_load_trusted() {
        let mut tp = TcpTransport::new();
        tp.load_trusted("some-pem-data".to_string());
        assert_eq!(tp.trusted_pem.as_deref(), Some("some-pem-data"));
    }

    #[test]
    fn transport_recv_line_trait_not_connected() {
        let mut tp = TcpTransport::new();
        let result = Transport::recv_line(&mut tp);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotConnected);
    }

    #[test]
    fn transport_send_trait_not_connected() {
        let mut tp = TcpTransport::new();
        let result = Transport::send(&mut tp, b"test");
        assert!(result.is_err());
    }

    // -- Error display tests --

    #[test]
    fn error_display() {
        assert_eq!(TransportError::NotConnected.to_string(), "transport not connected");
        assert_eq!(TransportError::CertRejected.to_string(), "certificate rejected");
        assert_eq!(
            TransportError::NoTlsConnector.to_string(),
            "no TLS connector configured"
        );
        assert!(TransportError::DnsResolution("example.com".into())
            .to_string()
            .contains("example.com"));
        assert!(TransportError::Tls("handshake failed".into())
            .to_string()
            .contains("handshake failed"));
        assert!(TransportError::LineTooLong.to_string().contains("buffer size"));
        assert_eq!(TransportError::Timeout.to_string(), "operation timed out");
    }

    #[test]
    fn error_from_io_timeout() {
        let io_err = io::Error::new(io::ErrorKind::TimedOut, "timed out");
        let e: TransportError = io_err.into();
        assert!(matches!(e, TransportError::Timeout));
    }

    #[test]
    fn error_from_io_other() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        let e: TransportError = io_err.into();
        assert!(matches!(e, TransportError::Io(_)));
    }

    #[test]
    fn error_into_io() {
        let e: io::Error = TransportError::NotConnected.into();
        assert_eq!(e.kind(), io::ErrorKind::NotConnected);

        let e: io::Error = TransportError::Timeout.into();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);

        let e: io::Error = TransportError::CertRejected.into();
        assert_eq!(e.kind(), io::ErrorKind::Other);
    }

    #[test]
    fn tls_error_display() {
        let e = TlsError {
            message: "self-signed certificate".to_string(),
            cert_pem: Some("pem-data".to_string()),
        };
        assert_eq!(e.to_string(), "self-signed certificate");
    }
}
