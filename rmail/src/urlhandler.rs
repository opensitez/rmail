//! URL click handling — open links in the default browser.
//!
//! Port of url.c. Detects URLs in message text and opens them
//! via the system default handler.

/// Open a URL in the system default browser.
/// Uses `open` on macOS, `xdg-open` on Linux.
pub fn open_url(url: &str) {
    if url.is_empty() { return; }

    // Sanitize — only allow http, https, mailto, ftp schemes
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://")
        && !lower.starts_with("mailto:") && !lower.starts_with("ftp://")
    {
        eprintln!("urlhandler: refusing to open non-standard URL: {}", url);
        return;
    }

    #[cfg(target_os = "macos")]
    { let _ = std::process::Command::new("open").arg(url).spawn(); }

    #[cfg(target_os = "linux")]
    { let _ = std::process::Command::new("xdg-open").arg(url).spawn(); }
}

/// Extract URLs from plain text. Returns (start_byte, end_byte, url) tuples.
/// Detects http://, https://, ftp://, mailto:, and bare www. URLs
/// (port of url.c URLIsSelected scheme detection + PeteURLScan).
pub fn find_urls(text: &str) -> Vec<(usize, usize, String)> {
    let mut urls = Vec::new();
    let prefixes = ["http://", "https://", "ftp://", "mailto:", "www."];

    for prefix in &prefixes {
        let mut search_from = 0;
        while let Some(start) = text[search_from..].to_ascii_lowercase().find(prefix) {
            let abs_start = search_from + start;
            // Don't match www. if preceded by :// (already caught by http/https)
            if *prefix == "www." && abs_start >= 3 && &text[abs_start - 3..abs_start] == "://" {
                search_from = abs_start + prefix.len();
                continue;
            }
            // Find end of URL — whitespace, >, ", or end of string
            let rest = &text[abs_start..];
            let end = rest.find(|c: char| c.is_whitespace() || c == '>' || c == '"'
                || c == '\'' || c == ')' || c == ']' || c == '<')
                .unwrap_or(rest.len());
            let url = &text[abs_start..abs_start + end];
            // Strip trailing punctuation (port of URL_TRAIL_IGNORE)
            let url = url.trim_end_matches(|c: char| c == '.' || c == ',' || c == ';'
                || c == '!' || c == '?' || c == ':');
            if url.len() > prefix.len() {
                // For bare www., prepend http://
                let full_url = if *prefix == "www." {
                    format!("http://{}", url)
                } else {
                    url.to_string()
                };
                urls.push((abs_start, abs_start + url.len(), full_url));
            }
            search_from = abs_start + end;
        }
    }
    urls.sort_by_key(|(start, _, _)| *start);
    // Deduplicate overlapping ranges
    urls.dedup_by(|b, a| b.0 >= a.0 && b.0 < a.1);
    urls
}

/// Scan plain text for URLs and apply link styling in the editor.
/// Port of url.c PeteURLScan — turns plain text URLs into clickable links.
pub fn linkify(ctrl: &reditctrl::glue::EditorCtrl, text: &str, char_offset: i32) {
    let urls = find_urls(text);
    for (byte_start, byte_end, url) in &urls {
        // Convert byte offsets to char offsets
        let c_start = text[..*byte_start].chars().count() as i32 + char_offset;
        let c_len = text[*byte_start..*byte_end].chars().count() as i32;
        ctrl.doc.set_link(c_start, c_len, Some(url));
    }
}
