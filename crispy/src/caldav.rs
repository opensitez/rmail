//! CalDAV engine — RFC 4791 calendar sync over HTTP/WebDAV.
//!
//! Port of CalDAVEngine.cpp. Uses the `http` module for HTTP and
//! simple string-based XML parsing (no expat dependency).
//!
//! Usage:
//! ```ignore
//! let mut tp = TcpTransport::new(...);
//! let mut eng = CalDAVEngine::new(&mut tp);
//! let home = eng.discover("https://caldav.example.com", "user", "pass")?;
//! let cals = eng.fetch_calendars(&home, "user", "pass")?;
//! let ics_list = eng.fetch_events(&cals[0].href, "user", "pass")?;
//! ```

use crate::http::{HttpClient, basic_auth};
use crate::transport::Transport;
use std::io;

/// Metadata for a discovered CalDAV calendar collection.
#[derive(Debug, Clone)]
pub struct CalDAVCalendarMeta {
    pub href: String,
    pub display_name: String,
    pub color: String,
    pub ctag: String, // getctag or sync-token
}

/// CalDAV engine. Wraps an HttpClient for all CalDAV operations.
pub struct CalDAVEngine<'a> {
    http: HttpClient<'a>,
}

impl<'a> CalDAVEngine<'a> {
    pub fn new(tp: &'a mut dyn Transport) -> Self {
        Self { http: HttpClient::new(tp) }
    }

    // ── Discovery ──────────────────────────────────────────────────────

    /// Resolve a server URL to the calendar-home-set URL.
    /// Follows RFC 6764: .well-known/caldav → principal → home-set.
    pub fn discover(
        &mut self,
        server_url: &str,
        username: &str,
        password: &str,
    ) -> io::Result<String> {
        let base = ensure_scheme(server_url);
        let origin = extract_origin(&base);
        let auth = basic_auth(username, password);

        // Step 1: .well-known/caldav
        let well_known = format!("{}/.well-known/caldav", origin);
        let mut principal_href = String::new();

        if let Ok(resp) = self.propfind(&well_known, &auth, &PROPFIND_PRINCIPAL, 0) {
            principal_href = parse_current_user_principal(&resp);
        }

        // Step 2: direct PROPFIND on supplied URL
        if principal_href.is_empty() {
            if let Ok(resp) = self.propfind(&base, &auth, &PROPFIND_PRINCIPAL, 0) {
                principal_href = parse_current_user_principal(&resp);
            }
        }

        // Step 3: PROPFIND on principal for calendar-home-set
        let mut home_url = String::new();
        if !principal_href.is_empty() {
            let principal_url = absolute_url(&base, &principal_href);
            if let Ok(resp) = self.propfind(&principal_url, &auth, &PROPFIND_HOME_SET, 0) {
                home_url = parse_calendar_home_set(&resp, &base);
            }
        }

        if home_url.is_empty() {
            home_url = base.clone();
        }

        Ok(home_url)
    }

    // ── Calendar listing ───────────────────────────────────────────────

    /// PROPFIND at depth 1 to enumerate calendar collections.
    pub fn fetch_calendars(
        &mut self,
        home_url: &str,
        username: &str,
        password: &str,
    ) -> io::Result<Vec<CalDAVCalendarMeta>> {
        let auth = basic_auth(username, password);
        let resp = self.propfind(home_url, &auth, &PROPFIND_CALENDARS, 1)?;
        Ok(parse_calendar_list(&resp))
    }

    // ── Event operations ───────────────────────────────────────────────

    /// Fetch all iCalendar strings from a calendar via REPORT calendar-query.
    pub fn fetch_events(
        &mut self,
        cal_url: &str,
        username: &str,
        password: &str,
    ) -> io::Result<Vec<String>> {
        let auth = basic_auth(username, password);
        let resp = self.report(cal_url, &auth, &REPORT_CALENDAR_QUERY)?;
        Ok(parse_calendar_data(&resp))
    }

    /// Incremental sync using ctag comparison.
    ///
    /// Fetches the server's current ctag for the given calendar URL.
    /// If `local_ctag` is non-empty and matches the server's ctag, returns
    /// `Ok(None)` (no changes). Otherwise returns `Ok(Some((events, new_ctag)))`.
    pub fn sync_collection(
        &mut self,
        cal_url: &str,
        username: &str,
        password: &str,
        local_ctag: &str,
    ) -> io::Result<Option<(Vec<String>, String)>> {
        let auth = basic_auth(username, password);

        // Fetch current ctag from server
        let resp = self.propfind(cal_url, &auth, &PROPFIND_CTAG, 0)?;
        let server_ctag = extract_tag(&resp, "getctag")
            .or_else(|| extract_tag(&resp, "sync-token"))
            .unwrap_or_default();

        // If local ctag matches server, no sync needed
        if !local_ctag.is_empty() && !server_ctag.is_empty() && local_ctag == server_ctag {
            return Ok(None);
        }

        // Full sync — fetch all events
        let events = self.fetch_events(cal_url, username, password)?;
        Ok(Some((events, server_ctag)))
    }

    /// PUT a single event (create or update).
    pub fn put_event(
        &mut self,
        cal_url: &str,
        uid: &str,
        ics_data: &str,
        username: &str,
        password: &str,
    ) -> io::Result<()> {
        let auth = basic_auth(username, password);
        let url = format!("{}{}.ics", ensure_trailing_slash(cal_url), uid);
        let resp = self.http.request("PUT", &url, &[
            ("Authorization", &auth),
            ("Content-Type", "text/calendar; charset=utf-8"),
        ], ics_data.as_bytes())?;
        if !resp.is_ok() {
            return Err(io::Error::new(io::ErrorKind::Other,
                format!("PUT {} → {}", url, resp.status)));
        }
        Ok(())
    }

    /// DELETE a single event by UID.
    pub fn delete_event(
        &mut self,
        cal_url: &str,
        uid: &str,
        username: &str,
        password: &str,
    ) -> io::Result<()> {
        let auth = basic_auth(username, password);
        let url = format!("{}{}.ics", ensure_trailing_slash(cal_url), uid);
        let resp = self.http.request("DELETE", &url, &[
            ("Authorization", &auth),
        ], &[])?;
        if !resp.is_ok() {
            return Err(io::Error::new(io::ErrorKind::Other,
                format!("DELETE {} → {}", url, resp.status)));
        }
        Ok(())
    }

    // ── Internal HTTP helpers ──────────────────────────────────────────

    fn propfind(&mut self, url: &str, auth: &str, body: &str, depth: u8) -> io::Result<String> {
        let depth_str = match depth { 0 => "0", 1 => "1", _ => "infinity" };
        let resp = self.http.request("PROPFIND", url, &[
            ("Authorization", auth),
            ("Content-Type", "application/xml; charset=utf-8"),
            ("Depth", depth_str),
        ], body.as_bytes())?;
        if resp.status >= 400 {
            return Err(io::Error::new(io::ErrorKind::Other,
                format!("PROPFIND {} → {}", url, resp.status)));
        }
        Ok(resp.body_str().to_string())
    }

    fn report(&mut self, url: &str, auth: &str, body: &str) -> io::Result<String> {
        let resp = self.http.request("REPORT", url, &[
            ("Authorization", auth),
            ("Content-Type", "application/xml; charset=utf-8"),
            ("Depth", "1"),
        ], body.as_bytes())?;
        if resp.status >= 400 {
            return Err(io::Error::new(io::ErrorKind::Other,
                format!("REPORT {} → {}", url, resp.status)));
        }
        Ok(resp.body_str().to_string())
    }
}

// ── XML request bodies ─────────────────────────────────────────────────

const PROPFIND_PRINCIPAL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop><D:current-user-principal/></D:prop>
</D:propfind>"#;

const PROPFIND_HOME_SET: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><C:calendar-home-set/></D:prop>
</D:propfind>"#;

const PROPFIND_CALENDARS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:"
            xmlns:C="urn:ietf:params:xml:ns:caldav"
            xmlns:CS="http://calendarserver.org/ns/"
            xmlns:I="http://apple.com/ns/ical/">
  <D:prop>
    <D:displayname/>
    <D:resourcetype/>
    <CS:getctag/>
    <D:sync-token/>
    <I:calendar-color/>
  </D:prop>
</D:propfind>"#;

const PROPFIND_CTAG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:"
            xmlns:CS="http://calendarserver.org/ns/">
  <D:prop>
    <CS:getctag/>
    <D:sync-token/>
  </D:prop>
</D:propfind>"#;

const REPORT_CALENDAR_QUERY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<C:calendar-query xmlns:D="DAV:"
                  xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT"/>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;

// ── Minimal XML parsing (no expat — just string extraction) ────────────
//
// CalDAV XML responses are well-structured multistatus documents.
// Instead of a full XML parser, we extract values with simple tag matching.
// This is sufficient for the specific CalDAV responses we expect.

/// Extract text content between `<tag>` and `</tag>` (first occurrence).
fn extract_tag(xml: &str, local_name: &str) -> Option<String> {
    // Match both prefixed (D:href, C:calendar-home-set) and bare tags
    for pattern in &[
        format!("<{}>", local_name),
        format!("<{}/>", local_name), // self-closing
    ] {
        if let Some(start) = xml.find(pattern.as_str()) {
            if pattern.ends_with("/>") { return Some(String::new()); }
            let after = start + pattern.len();
            let end_tag = format!("</{}>", local_name);
            if let Some(end) = xml[after..].find(&end_tag) {
                return Some(xml[after..after + end].trim().to_string());
            }
        }
    }
    // Try with namespace prefix (any single-char prefix like D:, C:, etc.)
    for prefix_len in 1..4 {
        let prefixed = format!(":{}",  local_name);
        if let Some(pos) = xml.find(&prefixed) {
            // Walk back to find '<'
            let tag_start = xml[..pos].rfind('<')?;
            let full_tag = &xml[tag_start + 1..pos + prefixed.len()];
            let open = format!("<{}>", full_tag);
            let close = format!("</{}>", full_tag);
            if let Some(s) = xml.find(&open) {
                let after = s + open.len();
                if let Some(e) = xml[after..].find(&close) {
                    return Some(xml[after..after + e].trim().to_string());
                }
            }
            break;
        }
    }
    None
}

/// Extract all occurrences of content between opening and closing tags of `local_name`.
fn extract_all_tags<'a>(xml: &'a str, local_name: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut search = xml;
    // Build patterns to search for (with common namespace prefixes)
    let patterns: Vec<String> = vec![
        format!("<{}>", local_name),
        format!("<D:{}>", local_name),
        format!("<C:{}>", local_name),
        format!("<CS:{}>", local_name),
        format!("<I:{}>", local_name),
    ];
    let close_patterns: Vec<String> = vec![
        format!("</{}>", local_name),
        format!("</D:{}>", local_name),
        format!("</C:{}>", local_name),
        format!("</CS:{}>", local_name),
        format!("</I:{}>", local_name),
    ];

    loop {
        let mut found = false;
        for (i, pat) in patterns.iter().enumerate() {
            if let Some(start) = search.find(pat.as_str()) {
                let after = start + pat.len();
                if let Some(end) = search[after..].find(close_patterns[i].as_str()) {
                    results.push(search[after..after + end].to_string());
                    search = &search[after + end + close_patterns[i].len()..];
                    found = true;
                    break;
                }
            }
        }
        if !found { break; }
    }
    results
}

fn parse_current_user_principal(xml: &str) -> String {
    // Look for <current-user-principal><href>...</href></current-user-principal>
    if let Some(block) = extract_tag(xml, "current-user-principal") {
        if let Some(href) = extract_tag(&block, "href") {
            return href;
        }
    }
    String::new()
}

fn parse_calendar_home_set(xml: &str, base: &str) -> String {
    if let Some(block) = extract_tag(xml, "calendar-home-set") {
        if let Some(href) = extract_tag(&block, "href") {
            return absolute_url(base, &href);
        }
    }
    String::new()
}

fn parse_calendar_list(xml: &str) -> Vec<CalDAVCalendarMeta> {
    let mut results = Vec::new();
    // Split by <response> blocks
    for block in extract_all_tags(xml, "response") {
        // Check if resourcetype contains "calendar"
        let is_calendar = block.contains(":calendar") || block.contains("<calendar");
        if !is_calendar { continue; }

        let href = extract_tag(&block, "href").unwrap_or_default();
        if href.is_empty() { continue; }

        let display_name = extract_tag(&block, "displayname")
            .unwrap_or_else(|| "Calendar".to_string());
        let color = extract_tag(&block, "calendar-color")
            .map(|c| {
                let c = c.trim().to_string();
                if c.len() == 9 && c.starts_with('#') { c[..7].to_string() } else { c }
            })
            .unwrap_or_else(|| "#4A90D9".to_string());
        let ctag = extract_tag(&block, "getctag")
            .or_else(|| extract_tag(&block, "sync-token"))
            .unwrap_or_default();

        results.push(CalDAVCalendarMeta { href, display_name, color, ctag });
    }
    results
}

fn parse_calendar_data(xml: &str) -> Vec<String> {
    extract_all_tags(xml, "calendar-data")
}

// ── URL helpers ────────────────────────────────────────────────────────

fn ensure_scheme(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("https://{}", url)
    }
}

fn extract_origin(url: &str) -> String {
    if let Some(pos) = url.find("://") {
        if let Some(slash) = url[pos + 3..].find('/') {
            return url[..pos + 3 + slash].to_string();
        }
    }
    url.to_string()
}

fn ensure_trailing_slash(url: &str) -> String {
    if url.ends_with('/') { url.to_string() } else { format!("{}/", url) }
}

fn absolute_url(base: &str, href: &str) -> String {
    if href.is_empty() { return base.to_string(); }
    if href.starts_with("http") { return href.to_string(); }
    if href.starts_with("//") {
        let scheme = base.split("://").next().unwrap_or("https");
        return format!("{}:{}", scheme, href);
    }
    if href.starts_with('/') {
        let origin = extract_origin(base);
        return format!("{}{}", origin, href);
    }
    format!("{}/{}", ensure_trailing_slash(base).trim_end_matches('/'), href)
}
