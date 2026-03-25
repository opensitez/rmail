//! Minimal HTTP/1.1 client built on the `Transport` trait.
//!
//! Supports GET, PUT, DELETE, and WebDAV methods (PROPFIND, REPORT).
//! Handles Basic auth, Content-Length bodies, chunked transfer-encoding,
//! and redirect following (up to 5 hops).

use crate::transport::Transport;
use std::io;
use std::collections::HashMap;

/// An HTTP response.
#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn body_str(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap_or("")
    }

    pub fn is_ok(&self) -> bool { self.status >= 200 && self.status < 300 }
    pub fn is_redirect(&self) -> bool { self.status == 301 || self.status == 302 || self.status == 307 || self.status == 308 }
}

/// An HTTP client session that wraps a `Transport`.
pub struct HttpClient<'a> {
    tp: &'a mut dyn Transport,
    connected_host: String,
    connected_port: u16,
}

impl<'a> HttpClient<'a> {
    pub fn new(tp: &'a mut dyn Transport) -> Self {
        Self { tp, connected_host: String::new(), connected_port: 0 }
    }

    /// Perform an HTTP request. Handles connection setup, TLS, and redirect following.
    pub fn request(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> io::Result<HttpResponse> {
        self.request_inner(method, url, headers, body, 0)
    }

    fn request_inner(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        redirect_count: u8,
    ) -> io::Result<HttpResponse> {
        let parsed = ParsedUrl::parse(url)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad URL"))?;

        // Connect if needed (different host or not connected)
        if self.connected_host != parsed.host || self.connected_port != parsed.port {
            self.tp.close();
            self.tp.connect(&parsed.host, parsed.port)?;
            if parsed.tls {
                self.tp.start_tls()?;
            }
            self.connected_host = parsed.host.clone();
            self.connected_port = parsed.port;
        }

        // Build request
        let path = if parsed.path.is_empty() { "/" } else { &parsed.path };
        let mut req = format!("{} {} HTTP/1.1\r\n", method, path);
        req.push_str(&format!("Host: {}\r\n", parsed.host));
        req.push_str("User-Agent: Phoenix-Eudora/1.0 CalDAV\r\n");
        req.push_str("Connection: keep-alive\r\n");

        // Add caller headers
        for (k, v) in headers {
            req.push_str(&format!("{}: {}\r\n", k, v));
        }

        if !body.is_empty() {
            req.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }

        req.push_str("\r\n");

        // Send request line + headers
        self.tp.send(req.as_bytes())?;
        // Send body
        if !body.is_empty() {
            self.tp.send(body)?;
        }

        // Read response status line
        let status_line = self.read_line_str()?;
        let (status, reason) = parse_status_line(&status_line);

        // Read headers
        let mut resp_headers = HashMap::new();
        loop {
            let line = self.read_line_str()?;
            if line.is_empty() || line == "\r\n" { break; }
            let line = line.trim_end_matches("\r\n").trim_end_matches('\n');
            if let Some((k, v)) = line.split_once(':') {
                resp_headers.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
        }

        // Read body
        let resp_body = self.read_body(&resp_headers)?;

        let resp = HttpResponse {
            status,
            reason,
            headers: resp_headers,
            body: resp_body,
        };

        // Handle redirects
        if resp.is_redirect() && redirect_count < 5 {
            if let Some(loc) = resp.headers.get("location") {
                let new_url = resolve_url(url, loc);
                // Reconnect may be needed for different host
                self.connected_host.clear(); // force reconnect
                return self.request_inner(method, &new_url, headers, body, redirect_count + 1);
            }
        }

        Ok(resp)
    }

    fn read_line_str(&mut self) -> io::Result<String> {
        let bytes = self.tp.recv_line()?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    fn read_body(&mut self, headers: &HashMap<String, String>) -> io::Result<Vec<u8>> {
        // Check for chunked transfer-encoding
        if let Some(te) = headers.get("transfer-encoding") {
            if te.to_lowercase().contains("chunked") {
                return self.read_chunked();
            }
        }

        // Content-Length
        if let Some(cl) = headers.get("content-length") {
            if let Ok(len) = cl.parse::<usize>() {
                return self.read_exact_bytes(len);
            }
        }

        // No content-length, no chunked — read until connection closes or empty line
        // For keep-alive this is tricky; assume 0-length body if no indicator
        Ok(Vec::new())
    }

    fn read_exact_bytes(&mut self, total: usize) -> io::Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(total);
        while buf.len() < total {
            let line = self.tp.recv_line()?;
            if line.is_empty() {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"));
            }
            buf.extend_from_slice(&line);
        }
        buf.truncate(total);
        Ok(buf)
    }

    fn read_chunked(&mut self) -> io::Result<Vec<u8>> {
        let mut body = Vec::new();
        loop {
            let size_line = self.read_line_str()?;
            let size_str = size_line.trim();
            let chunk_size = usize::from_str_radix(size_str.split(';').next().unwrap_or("0").trim(), 16)
                .unwrap_or(0);
            if chunk_size == 0 {
                // Read trailing CRLF
                let _ = self.tp.recv_line();
                break;
            }
            let chunk = self.read_exact_bytes(chunk_size)?;
            body.extend_from_slice(&chunk);
            // Read trailing CRLF after chunk data
            let _ = self.tp.recv_line();
        }
        Ok(body)
    }
}

// ── URL parsing ────────────────────────────────────────────────────────

struct ParsedUrl {
    tls: bool,
    host: String,
    port: u16,
    path: String,
}

impl ParsedUrl {
    fn parse(url: &str) -> Option<Self> {
        let (tls, rest) = if url.starts_with("https://") {
            (true, &url[8..])
        } else if url.starts_with("http://") {
            (false, &url[7..])
        } else {
            return None;
        };

        let (host_port, path) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].to_string()),
            None => (rest, "/".to_string()),
        };

        let (host, port) = match host_port.rfind(':') {
            Some(i) if !host_port[i+1..].contains(']') => {
                let p = host_port[i+1..].parse::<u16>().ok()?;
                (host_port[..i].to_string(), p)
            }
            _ => (host_port.to_string(), if tls { 443 } else { 80 }),
        };

        Some(Self { tls, host, port, path })
    }
}

fn parse_status_line(line: &str) -> (u16, String) {
    // "HTTP/1.1 207 Multi-Status\r\n"
    let parts: Vec<&str> = line.trim().splitn(3, ' ').collect();
    let status = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let reason = parts.get(2).map(|s| s.to_string()).unwrap_or_default();
    (status, reason)
}

fn resolve_url(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    // Absolute path
    if href.starts_with('/') {
        if let Some(pos) = base.find("://") {
            if let Some(slash) = base[pos + 3..].find('/') {
                return format!("{}{}", &base[..pos + 3 + slash], href);
            }
        }
        return format!("{}{}", base, href);
    }
    // Relative
    if let Some(pos) = base.rfind('/') {
        return format!("{}/{}", &base[..pos], href);
    }
    format!("{}/{}", base, href)
}

/// Build a Basic auth header value.
pub fn basic_auth(username: &str, password: &str) -> String {
    let encoded = crate::base64::encode(format!("{}:{}", username, password).as_bytes());
    format!("Basic {}", encoded)
}
