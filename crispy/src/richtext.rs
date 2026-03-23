//! Rich text format conversions.
//!
//! Conversions between text/enriched (RFC 1896), text/html, text/plain,
//! and format=flowed (RFC 3676). Pure text transforms with no UI dependency.
//!
//! # Supported formats
//!
//! - **Text/enriched** (RFC 1896): `<bold>`, `<italic>`, `<underline>`, `<fixed>`,
//!   `<bigger>`, `<smaller>`, `<color><param>RRGGBB</param></color>`,
//!   `<fontfamily><param>name</param>`, `<center>`, `<flushleft>`, `<flushright>`,
//!   `<excerpt>`, `<nofill>`, `<param>`, `<<` for literal `<`.
//!
//! - **Format=flowed** (RFC 3676): soft line breaks (trailing space), quote levels
//!   (leading `>`), `delsp=yes` support.
//!
//! - **HTML**: tag stripping, entity decoding, basic tag-to-enriched mapping.

// ---------------------------------------------------------------------------
// Text/enriched -> HTML
// ---------------------------------------------------------------------------

/// Parses an enriched tag starting at `<`. Returns `(tag_name_lowercase, is_close, rest)`.
/// If the input is `<<`, returns `None` (caller handles the literal `<`).
fn parse_enriched_tag(input: &str) -> Option<(String, bool, &str)> {
    let s = input.strip_prefix('<')?;

    // << means literal <
    if s.starts_with('<') {
        return None;
    }

    let (is_close, s) = if let Some(rest) = s.strip_prefix('/') {
        (true, rest)
    } else {
        (false, s)
    };

    let end = s.find('>').unwrap_or(s.len());
    let tag = s[..end].to_ascii_lowercase();
    let rest = if end < s.len() { &s[end + 1..] } else { &s[end..] };
    Some((tag, is_close, rest))
}

/// Convert text/enriched (RFC 1896) to HTML.
///
/// Maps enriched tags to their HTML equivalents:
/// - `<bold>` -> `<b>`, `<italic>` -> `<i>`, `<underline>` -> `<u>`
/// - `<fixed>` -> `<tt>`, `<bigger>` -> `<big>`, `<smaller>` -> `<small>`
/// - `<center>` -> `<center>`, `<flushleft>` / `<flushright>` -> `<div>`
/// - `<excerpt>` -> `<blockquote>`, `<nofill>` -> `<pre>`
/// - `<color><param>RRGGBB</param>` -> `<span style="color:#RRGGBB">`
/// - `<fontfamily><param>name</param>` -> `<span style="font-family:name">`
/// - `<<` -> literal `<` (rendered as `&lt;`)
///
/// Single newlines become spaces; double newlines become `<br>`.
/// Inside `<nofill>`, newlines are preserved literally.
pub fn enriched_to_html(enriched: &str) -> String {
    let mut out = String::with_capacity(enriched.len() + 128);
    out.push_str("<html><body>");

    let mut in_nofill = false;
    let mut in_param = false;
    let mut param_buf = String::new();
    let mut pending_tag = String::new();
    let mut p = enriched;

    while !p.is_empty() {
        if p.starts_with("<<") {
            if in_param {
                param_buf.push('<');
            } else {
                out.push_str("&lt;");
            }
            p = &p[2..];
            continue;
        }

        if p.starts_with('<') {
            if let Some((tag, is_close, rest)) = parse_enriched_tag(p) {
                p = rest;

                if tag == "param" {
                    if !is_close {
                        in_param = true;
                        param_buf.clear();
                    } else {
                        in_param = false;
                        // Apply param to pending tag
                        if pending_tag == "color" {
                            let color = parse_color_param(&param_buf);
                            out.push_str(&format!("<span style=\"color:{}\">", color));
                        } else if pending_tag == "fontfamily" {
                            out.push_str(&format!(
                                "<span style=\"font-family:{}\">",
                                param_buf
                            ));
                        }
                        pending_tag.clear();
                    }
                    continue;
                }

                if in_param {
                    continue;
                }

                match tag.as_str() {
                    "bold" => out.push_str(if is_close { "</b>" } else { "<b>" }),
                    "italic" => out.push_str(if is_close { "</i>" } else { "<i>" }),
                    "underline" => out.push_str(if is_close { "</u>" } else { "<u>" }),
                    "fixed" => out.push_str(if is_close { "</tt>" } else { "<tt>" }),
                    "bigger" => out.push_str(if is_close { "</big>" } else { "<big>" }),
                    "smaller" => out.push_str(if is_close { "</small>" } else { "<small>" }),
                    "center" => out.push_str(if is_close { "</center>" } else { "<center>" }),
                    "flushleft" => out.push_str(if is_close {
                        "</div>"
                    } else {
                        "<div style=\"text-align:left\">"
                    }),
                    "flushright" => out.push_str(if is_close {
                        "</div>"
                    } else {
                        "<div style=\"text-align:right\">"
                    }),
                    "flushboth" => out.push_str(if is_close {
                        "</div>"
                    } else {
                        "<div style=\"text-align:justify\">"
                    }),
                    "excerpt" => {
                        out.push_str(if is_close { "</blockquote>" } else { "<blockquote>" })
                    }
                    "nofill" => {
                        out.push_str(if is_close { "</pre>" } else { "<pre>" });
                        in_nofill = !is_close;
                    }
                    "color" => {
                        if is_close {
                            out.push_str("</span>");
                        } else {
                            pending_tag = "color".into();
                        }
                    }
                    "fontfamily" => {
                        if is_close {
                            out.push_str("</span>");
                        } else {
                            pending_tag = "fontfamily".into();
                        }
                    }
                    _ => {} // unknown tags: ignore
                }
                continue;
            } else {
                // Was << — already handled above, but if parse returned None
                // for some other reason, skip the '<'
                if p.starts_with("<<") {
                    if in_param {
                        param_buf.push('<');
                    } else {
                        out.push_str("&lt;");
                    }
                    p = &p[2..];
                } else {
                    p = &p[1..];
                }
                continue;
            }
        }

        if in_param {
            let ch = p.as_bytes()[0];
            param_buf.push(ch as char);
            p = &p[1..];
            continue;
        }

        // Regular text
        let ch = p.as_bytes()[0];
        match ch {
            b'&' => out.push_str("&amp;"),
            b'>' => out.push_str("&gt;"),
            b'\n' => {
                if in_nofill {
                    out.push('\n');
                } else {
                    // Single newline = space, double newline = <br>
                    if p.len() > 1 && p.as_bytes()[1] == b'\n' {
                        out.push_str("<br>\n");
                        p = &p[2..];
                        continue;
                    } else {
                        out.push(' ');
                    }
                }
            }
            b'\r' => {} // skip CR
            _ => out.push(ch as char),
        }
        p = &p[1..];
    }

    out.push_str("</body></html>");
    out
}

/// Parse a color parameter in either `RRRR,GGGG,BBBB` or `RRGGBB` format,
/// returning an HTML color string like `#rrggbb`.
fn parse_color_param(param: &str) -> String {
    if param.contains(',') {
        // RRRR,GGGG,BBBB format — take high byte of each 16-bit component
        let parts: Vec<&str> = param.split(',').collect();
        if parts.len() >= 3 {
            let r = u16::from_str_radix(parts[0].trim(), 16).unwrap_or(0);
            let g = u16::from_str_radix(parts[1].trim(), 16).unwrap_or(0);
            let b = u16::from_str_radix(parts[2].trim(), 16).unwrap_or(0);
            return format!("#{:02x}{:02x}{:02x}", r >> 8, g >> 8, b >> 8);
        }
    }
    if param.len() >= 6 {
        return format!("#{}", &param[..6]);
    }
    "#000000".into()
}

// ---------------------------------------------------------------------------
// Text/enriched -> plain text
// ---------------------------------------------------------------------------

/// Convert text/enriched (RFC 1896) to plain text.
///
/// Strips all formatting tags. `<<` becomes a literal `<`.
/// `<excerpt>` inserts a `> ` quote prefix. `<param>` content is discarded.
pub fn enriched_to_plain(enriched: &str) -> String {
    let mut out = String::with_capacity(enriched.len());
    let mut in_param = false;
    let mut p = enriched;

    while !p.is_empty() {
        if p.starts_with("<<") {
            if !in_param {
                out.push('<');
            }
            p = &p[2..];
            continue;
        }

        if p.starts_with('<') {
            if let Some((tag, is_close, rest)) = parse_enriched_tag(p) {
                p = rest;
                if tag == "param" {
                    in_param = !is_close;
                } else if tag == "excerpt" && !is_close {
                    out.push_str("> ");
                }
                continue;
            } else {
                // << case already handled
                p = &p[1..];
                continue;
            }
        }

        if in_param {
            p = &p[1..];
            continue;
        }

        if p.as_bytes()[0] == b'\r' {
            p = &p[1..];
            continue;
        }

        out.push(p.as_bytes()[0] as char);
        p = &p[1..];
    }
    out
}

// ---------------------------------------------------------------------------
// HTML -> text/enriched
// ---------------------------------------------------------------------------

/// Convert HTML to text/enriched.
///
/// Maps common HTML tags to enriched equivalents:
/// - `<b>`, `<strong>` -> `<bold>`
/// - `<i>`, `<em>` -> `<italic>`
/// - `<u>` -> `<underline>`
/// - `<tt>`, `<code>` -> `<fixed>`
/// - `<blockquote>` -> `<excerpt>`
/// - `<pre>` -> `<nofill>`
/// - `<center>` -> `<center>`
/// - `<br>` -> double newline, `<p>` -> double newline
///
/// Decodes HTML entities (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&nbsp;`, `&#NNN;`).
/// Literal `<` in text is escaped as `<<`.
pub fn html_to_enriched(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut p = html;

    while !p.is_empty() {
        if p.starts_with('<') {
            p = &p[1..];
            let is_close;
            if p.starts_with('/') {
                is_close = true;
                p = &p[1..];
            } else {
                is_close = false;
            }

            // Read tag name
            let tag_end = p
                .find(|c: char| c == '>' || c == ' ')
                .unwrap_or(p.len());
            let tag = p[..tag_end].to_ascii_lowercase();

            // Skip past attributes and '>'
            if let Some(gt) = p.find('>') {
                p = &p[gt + 1..];
            } else {
                p = &p[tag_end..];
            }

            match tag.as_str() {
                "b" | "strong" => {
                    out.push_str(if is_close { "</bold>" } else { "<bold>" })
                }
                "i" | "em" => {
                    out.push_str(if is_close { "</italic>" } else { "<italic>" })
                }
                "u" => out.push_str(if is_close { "</underline>" } else { "<underline>" }),
                "tt" | "code" => {
                    out.push_str(if is_close { "</fixed>" } else { "<fixed>" })
                }
                "blockquote" => {
                    out.push_str(if is_close { "</excerpt>" } else { "<excerpt>" })
                }
                "pre" => out.push_str(if is_close { "</nofill>" } else { "<nofill>" }),
                "center" => {
                    out.push_str(if is_close { "</center>" } else { "<center>" })
                }
                "br" => out.push_str("\n\n"),
                "p" if !is_close => out.push_str("\n\n"),
                _ => {} // skip other tags
            }
            continue;
        }

        if p.starts_with('&') {
            if let Some(decoded) = decode_html_entity_for_enriched(p) {
                out.push_str(&decoded.0);
                p = decoded.1;
                continue;
            }
            out.push('&');
            p = &p[1..];
            continue;
        }

        out.push(p.as_bytes()[0] as char);
        p = &p[1..];
    }
    out
}

/// Decode an HTML entity at the current position, returning the replacement
/// text and remaining input. For enriched output, `<` must be escaped as `<<`.
fn decode_html_entity_for_enriched(p: &str) -> Option<(String, &str)> {
    if let Some(rest) = p.strip_prefix("&amp;") {
        return Some(("&".into(), rest));
    }
    if let Some(rest) = p.strip_prefix("&lt;") {
        return Some(("<<".into(), rest)); // escape < for enriched
    }
    if let Some(rest) = p.strip_prefix("&gt;") {
        return Some((">".into(), rest));
    }
    if let Some(rest) = p.strip_prefix("&quot;") {
        return Some(("\"".into(), rest));
    }
    if let Some(rest) = p.strip_prefix("&nbsp;") {
        return Some((" ".into(), rest));
    }
    if let Some(rest) = p.strip_prefix("&#") {
        return Some(decode_numeric_entity_ascii(rest));
    }
    None
}

/// Decode a numeric character reference (`&#NNN;` or `&#xHHH;`).
/// Only emits ASCII codepoints; skips others. Returns (output, rest).
fn decode_numeric_entity_ascii(s: &str) -> (String, &str) {
    let (cp, rest) = if s.starts_with('x') || s.starts_with('X') {
        let s = &s[1..];
        let end = s.find(';').unwrap_or(s.len());
        let cp = u32::from_str_radix(&s[..end], 16).unwrap_or(0);
        let rest = if end < s.len() { &s[end + 1..] } else { &s[end..] };
        (cp, rest)
    } else {
        let end = s.find(';').unwrap_or(s.len());
        let cp = s[..end].parse::<u32>().unwrap_or(0);
        let rest = if end < s.len() { &s[end + 1..] } else { &s[end..] };
        (cp, rest)
    };
    if cp < 128 {
        (String::from(cp as u8 as char), rest)
    } else {
        (String::new(), rest)
    }
}

// ---------------------------------------------------------------------------
// Plain text -> text/enriched
// ---------------------------------------------------------------------------

/// Convert plain text to text/enriched.
///
/// Wraps content in `<nofill>...</nofill>` and escapes `<` as `<<`.
pub fn plain_to_enriched(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 20);
    out.push_str("<nofill>");
    for ch in text.chars() {
        if ch == '<' {
            out.push_str("<<");
        } else {
            out.push(ch);
        }
    }
    out.push_str("</nofill>");
    out
}

// ---------------------------------------------------------------------------
// Format=flowed (RFC 3676) -> plain text
// ---------------------------------------------------------------------------

/// Count leading `>` characters (quote depth) and strip the optional
/// space-stuffing character. Returns `(depth, content)`.
fn count_quote_depth(line: &str) -> (usize, &str) {
    let depth = line.bytes().take_while(|&b| b == b'>').count();
    let rest = &line[depth..];
    // Strip optional space-stuffing
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    (depth, rest)
}

/// Convert format=flowed text (RFC 3676) to reflowed plain text.
///
/// - Lines ending with a trailing space are "soft" — joined with the next line.
/// - Lines without a trailing space are "hard" — line break is preserved.
/// - Lines starting with `>` are quoted; nesting depth is tracked.
/// - `"-- "` is a signature separator (always a hard break).
/// - If `delsp` is true, the trailing space on soft lines is deleted.
pub fn flowed_to_plain(flowed: &str, delsp: bool) -> String {
    let mut out = String::with_capacity(flowed.len());
    let mut prev_depth: usize = 0;

    for raw_line in LineIter::new(flowed) {
        // Strip trailing CR
        let line = raw_line.trim_end_matches('\r');
        let (depth, content) = count_quote_depth(line);

        // Quote depth changed? Insert newline and new prefix
        if depth != prev_depth {
            out.push('\n');
            for _ in 0..depth {
                out.push('>');
            }
            if depth > 0 {
                out.push(' ');
            }
            prev_depth = depth;
        }

        // Signature separator
        if content == "-- " {
            out.push_str("-- \n");
            prev_depth = depth;
            continue;
        }

        let soft = content.ends_with(' ') && !content.is_empty();

        if soft && delsp {
            out.push_str(&content[..content.len() - 1]);
        } else if soft {
            out.push_str(content);
        } else {
            out.push_str(content);
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Format=flowed -> HTML
// ---------------------------------------------------------------------------

/// Convert format=flowed text (RFC 3676) to HTML.
///
/// Quote levels are rendered as nested `<blockquote>` elements.
/// Hard line breaks become `<br>`. Text is HTML-escaped.
/// If `delsp` is true, trailing spaces on soft-wrapped lines are removed.
pub fn flowed_to_html(flowed: &str, delsp: bool) -> String {
    let mut out = String::with_capacity(flowed.len() + 128);
    out.push_str("<html><body>");
    let mut cur_depth: usize = 0;

    for raw_line in LineIter::new(flowed) {
        let line = raw_line.trim_end_matches('\r');
        let (depth, content) = count_quote_depth(line);

        // Adjust blockquote nesting
        while cur_depth < depth {
            out.push_str("<blockquote>");
            cur_depth += 1;
        }
        while cur_depth > depth {
            out.push_str("</blockquote>");
            cur_depth -= 1;
        }

        let soft = content.ends_with(' ') && !content.is_empty();
        let emit_len = if soft && delsp {
            content.len() - 1
        } else {
            content.len()
        };

        for ch in content[..emit_len].chars() {
            match ch {
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '&' => out.push_str("&amp;"),
                _ => out.push(ch),
            }
        }

        if !soft {
            out.push_str("<br>\n");
        }
    }

    while cur_depth > 0 {
        out.push_str("</blockquote>");
        cur_depth -= 1;
    }
    out.push_str("</body></html>");
    out
}

// ---------------------------------------------------------------------------
// Plain text -> format=flowed
// ---------------------------------------------------------------------------

/// Convert plain text to format=flowed (RFC 3676).
///
/// Lines are wrapped at `max_col` columns (default 72 if 0 or negative).
/// Quote prefixes (`>`) are preserved and accounted for in the wrap width.
/// Soft breaks are indicated by a trailing space before CRLF.
pub fn plain_to_flowed(text: &str, max_col: usize) -> String {
    let max_col = if max_col == 0 { 72 } else { max_col };
    let mut out = String::with_capacity(text.len() + text.len() / 4);

    for raw_line in text.split('\n') {
        // Count quote prefix
        let depth = raw_line.bytes().take_while(|&b| b == b'>').count();
        let rest = &raw_line[depth..];
        let rest = rest.strip_prefix(' ').unwrap_or(rest);

        let prefix_len = depth + if depth > 0 { 1 } else { 0 }; // "> "
        let wrap_at = max_col.saturating_sub(prefix_len).max(20);
        let content = rest;

        if content.len() <= wrap_at {
            // Short line — output with hard break
            write_quote_prefix(&mut out, depth);
            out.push_str(content);
            out.push_str("\r\n");
        } else {
            // Long line — wrap with soft breaks
            let mut remaining = content;
            while !remaining.is_empty() {
                let chunk_len = if remaining.len() > wrap_at {
                    // Find last space within chunk for word wrap
                    let search = &remaining[..wrap_at];
                    if let Some(pos) = search.rfind(' ') {
                        if pos > 0 {
                            pos + 1 // include the space
                        } else {
                            wrap_at
                        }
                    } else {
                        wrap_at
                    }
                } else {
                    remaining.len()
                };

                write_quote_prefix(&mut out, depth);
                out.push_str(&remaining[..chunk_len]);
                remaining = &remaining[chunk_len..];
                out.push_str("\r\n");
            }
        }
    }
    out
}

fn write_quote_prefix(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push('>');
    }
    if depth > 0 {
        out.push(' ');
    }
}

// ---------------------------------------------------------------------------
// HTML -> plain text
// ---------------------------------------------------------------------------

/// Convert HTML to plain text.
///
/// Strips all tags, converting structural elements to appropriate whitespace:
/// - `<br>` -> newline, `<p>` -> double newline, `<div>` -> newline
/// - `<li>` -> `\n- `, `<hr>` -> `\n---\n`
/// - `<h1>`..`<h3>` -> surrounding newlines
/// - `<pre>` content preserves whitespace
///
/// Decodes standard HTML entities and numeric character references (including
/// non-ASCII via UTF-8).
pub fn html_to_plain(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_pre = false;
    let mut p = html;

    while !p.is_empty() {
        if p.starts_with('<') {
            p = &p[1..];
            let is_close;
            if p.starts_with('/') {
                is_close = true;
                p = &p[1..];
            } else {
                is_close = false;
            }

            let tag_end = p
                .find(|c: char| c == '>' || c == ' ')
                .unwrap_or(p.len());
            let tag = p[..tag_end].to_ascii_lowercase();

            // Skip past '>'
            if let Some(gt) = p.find('>') {
                p = &p[gt + 1..];
            } else {
                p = &p[tag_end..];
            }

            match tag.as_str() {
                "br" => out.push('\n'),
                "p" if !is_close => out.push_str("\n\n"),
                "div" if !is_close => out.push('\n'),
                "li" if !is_close => out.push_str("\n- "),
                "hr" => out.push_str("\n---\n"),
                "pre" => in_pre = !is_close,
                "h1" | "h2" | "h3" => {
                    if !is_close {
                        out.push_str("\n\n");
                    } else {
                        out.push('\n');
                    }
                }
                _ => {}
            }
            continue;
        }

        if p.starts_with('&') {
            if let Some((s, rest)) = decode_html_entity_full(p) {
                out.push_str(&s);
                p = rest;
                continue;
            }
            // Unknown entity — skip the '&'
            p = &p[1..];
            continue;
        }

        if !in_pre && p.as_bytes()[0].is_ascii_whitespace() {
            // Collapse whitespace outside <pre>
            let skip = p
                .bytes()
                .take_while(|b| matches!(b, b'\r' | b'\n' | b' ' | b'\t'))
                .count();
            p = &p[skip..];
            out.push(' ');
            continue;
        }

        // Safe for ASCII and multi-byte UTF-8: take one char
        let ch = p.chars().next().unwrap();
        out.push(ch);
        p = &p[ch.len_utf8()..];
    }
    out
}

/// Decode an HTML entity for plain-text output (supports UTF-8 for non-ASCII).
fn decode_html_entity_full(p: &str) -> Option<(String, &str)> {
    if let Some(rest) = p.strip_prefix("&amp;") {
        return Some(("&".into(), rest));
    }
    if let Some(rest) = p.strip_prefix("&lt;") {
        return Some(("<".into(), rest));
    }
    if let Some(rest) = p.strip_prefix("&gt;") {
        return Some((">".into(), rest));
    }
    if let Some(rest) = p.strip_prefix("&quot;") {
        return Some(("\"".into(), rest));
    }
    if let Some(rest) = p.strip_prefix("&apos;") {
        return Some(("'".into(), rest));
    }
    if let Some(rest) = p.strip_prefix("&nbsp;") {
        return Some((" ".into(), rest));
    }
    if let Some(rest) = p.strip_prefix("&#") {
        let (s, r) = decode_numeric_entity_utf8(rest);
        return Some((s, r));
    }
    None
}

/// Decode a numeric character reference, emitting UTF-8 for any valid codepoint.
fn decode_numeric_entity_utf8(s: &str) -> (String, &str) {
    let (cp, rest) = if s.starts_with('x') || s.starts_with('X') {
        let s = &s[1..];
        let end = s.find(';').unwrap_or(s.len());
        let cp = u32::from_str_radix(&s[..end], 16).unwrap_or(0);
        let rest = if end < s.len() { &s[end + 1..] } else { &s[end..] };
        (cp, rest)
    } else {
        let end = s.find(';').unwrap_or(s.len());
        let cp = s[..end].parse::<u32>().unwrap_or(0);
        let rest = if end < s.len() { &s[end + 1..] } else { &s[end..] };
        (cp, rest)
    };

    if let Some(ch) = char::from_u32(cp) {
        (ch.to_string(), rest)
    } else {
        (String::new(), rest)
    }
}

// ---------------------------------------------------------------------------
// Plain text -> HTML
// ---------------------------------------------------------------------------

/// Convert plain text to minimal HTML.
///
/// Wraps content in `<pre>` and escapes `<`, `>`, `&`. Strips carriage returns.
pub fn plain_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    out.push_str("<html><body><pre>");
    for ch in text.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '\r' => {}
            _ => out.push(ch),
        }
    }
    out.push_str("</pre></body></html>");
    out
}

// ---------------------------------------------------------------------------
// Line iterator (splits on \n, like the C code line-by-line processing)
// ---------------------------------------------------------------------------

/// Iterates over lines splitting on `\n`. Unlike `str::lines()`, this
/// preserves trailing empty lines and does not strip `\r`.
struct LineIter<'a> {
    remaining: &'a str,
    done: bool,
}

impl<'a> LineIter<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            remaining: s,
            done: false,
        }
    }
}

impl<'a> Iterator for LineIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.done {
            return None;
        }
        if let Some(pos) = self.remaining.find('\n') {
            let line = &self.remaining[..pos];
            self.remaining = &self.remaining[pos + 1..];
            Some(line)
        } else if !self.remaining.is_empty() {
            let line = self.remaining;
            self.remaining = "";
            self.done = true;
            Some(line)
        } else {
            self.done = true;
            None
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- enriched_to_html ---------------------------------------------------

    #[test]
    fn enriched_to_html_basic_tags() {
        let input = "<bold>Hello</bold> <italic>world</italic>";
        let html = enriched_to_html(input);
        assert!(html.contains("<b>Hello</b> <i>world</i>"));
    }

    #[test]
    fn enriched_to_html_literal_lt() {
        let input = "a << b";
        let html = enriched_to_html(input);
        assert!(html.contains("a &lt; b"));
    }

    #[test]
    fn enriched_to_html_newline_handling() {
        // Single newline -> space, double newline -> <br>
        let input = "line1\nline2\n\nline3";
        let html = enriched_to_html(input);
        assert!(html.contains("line1 line2"));
        assert!(html.contains("<br>\nline3"));
    }

    #[test]
    fn enriched_to_html_nofill() {
        let input = "<nofill>line1\nline2</nofill>";
        let html = enriched_to_html(input);
        assert!(html.contains("<pre>line1\nline2</pre>"));
    }

    #[test]
    fn enriched_to_html_color_rrggbb() {
        let input = "<color><param>ff0000</param>red text</color>";
        let html = enriched_to_html(input);
        assert!(html.contains("<span style=\"color:#ff0000\">red text</span>"));
    }

    #[test]
    fn enriched_to_html_color_comma_format() {
        // RRRR,GGGG,BBBB -> take high byte
        let input = "<color><param>ffff,0000,8000</param>text</color>";
        let html = enriched_to_html(input);
        assert!(html.contains("<span style=\"color:#ff0080\">text</span>"));
    }

    #[test]
    fn enriched_to_html_fontfamily() {
        let input = "<fontfamily><param>Helvetica</param>text</fontfamily>";
        let html = enriched_to_html(input);
        assert!(html.contains("<span style=\"font-family:Helvetica\">text</span>"));
    }

    #[test]
    fn enriched_to_html_all_formatting_tags() {
        let input = concat!(
            "<underline>u</underline>",
            "<fixed>f</fixed>",
            "<bigger>big</bigger>",
            "<smaller>sm</smaller>",
            "<center>c</center>",
            "<flushleft>l</flushleft>",
            "<flushright>r</flushright>",
            "<flushboth>j</flushboth>",
            "<excerpt>q</excerpt>",
        );
        let html = enriched_to_html(input);
        assert!(html.contains("<u>u</u>"));
        assert!(html.contains("<tt>f</tt>"));
        assert!(html.contains("<big>big</big>"));
        assert!(html.contains("<small>sm</small>"));
        assert!(html.contains("<center>c</center>"));
        assert!(html.contains("<div style=\"text-align:left\">l</div>"));
        assert!(html.contains("<div style=\"text-align:right\">r</div>"));
        assert!(html.contains("<div style=\"text-align:justify\">j</div>"));
        assert!(html.contains("<blockquote>q</blockquote>"));
    }

    #[test]
    fn enriched_to_html_unknown_tags_ignored() {
        let input = "<unknown>hello</unknown>";
        let html = enriched_to_html(input);
        assert!(html.contains("hello"));
        assert!(!html.contains("<unknown>"));
    }

    #[test]
    fn enriched_to_html_amp_gt_escaped() {
        let input = "a & b > c";
        let html = enriched_to_html(input);
        assert!(html.contains("a &amp; b &gt; c"));
    }

    #[test]
    fn enriched_to_html_empty() {
        assert_eq!(enriched_to_html(""), "<html><body></body></html>");
    }

    #[test]
    fn enriched_to_html_cr_stripped() {
        let input = "hello\r\nworld";
        let html = enriched_to_html(input);
        // \r should be stripped; single \n -> space
        assert!(html.contains("hello world"));
    }

    // -- enriched_to_plain --------------------------------------------------

    #[test]
    fn enriched_to_plain_strips_tags() {
        let input = "<bold>Hello</bold> <italic>world</italic>";
        assert_eq!(enriched_to_plain(input), "Hello world");
    }

    #[test]
    fn enriched_to_plain_literal_lt() {
        assert_eq!(enriched_to_plain("a << b"), "a < b");
    }

    #[test]
    fn enriched_to_plain_excerpt() {
        let input = "<excerpt>quoted text</excerpt>";
        assert_eq!(enriched_to_plain(input), "> quoted text");
    }

    #[test]
    fn enriched_to_plain_param_stripped() {
        let input = "<color><param>ff0000</param>red</color>";
        assert_eq!(enriched_to_plain(input), "red");
    }

    #[test]
    fn enriched_to_plain_empty() {
        assert_eq!(enriched_to_plain(""), "");
    }

    #[test]
    fn enriched_to_plain_preserves_newlines() {
        assert_eq!(enriched_to_plain("line1\nline2"), "line1\nline2");
    }

    // -- html_to_enriched ---------------------------------------------------

    #[test]
    fn html_to_enriched_basic_tags() {
        let html = "<b>bold</b> <i>italic</i> <u>underline</u>";
        let enriched = html_to_enriched(html);
        assert!(enriched.contains("<bold>bold</bold>"));
        assert!(enriched.contains("<italic>italic</italic>"));
        assert!(enriched.contains("<underline>underline</underline>"));
    }

    #[test]
    fn html_to_enriched_strong_em() {
        let html = "<strong>s</strong><em>e</em>";
        let enriched = html_to_enriched(html);
        assert!(enriched.contains("<bold>s</bold>"));
        assert!(enriched.contains("<italic>e</italic>"));
    }

    #[test]
    fn html_to_enriched_br_p() {
        let enriched = html_to_enriched("a<br>b<p>c");
        assert!(enriched.contains("a\n\nb\n\nc"));
    }

    #[test]
    fn html_to_enriched_entities() {
        let enriched = html_to_enriched("&amp; &lt; &gt; &quot; &nbsp;");
        assert!(enriched.contains("& << > \" "));
    }

    #[test]
    fn html_to_enriched_numeric_entity() {
        let enriched = html_to_enriched("&#65;&#x42;");
        assert!(enriched.contains("AB"));
    }

    #[test]
    fn html_to_enriched_blockquote_pre() {
        let enriched = html_to_enriched("<blockquote>q</blockquote><pre>p</pre>");
        assert!(enriched.contains("<excerpt>q</excerpt>"));
        assert!(enriched.contains("<nofill>p</nofill>"));
    }

    // -- plain_to_enriched --------------------------------------------------

    #[test]
    fn plain_to_enriched_basic() {
        let enriched = plain_to_enriched("hello");
        assert_eq!(enriched, "<nofill>hello</nofill>");
    }

    #[test]
    fn plain_to_enriched_escapes_lt() {
        let enriched = plain_to_enriched("a < b");
        assert_eq!(enriched, "<nofill>a << b</nofill>");
    }

    #[test]
    fn plain_to_enriched_preserves_newlines() {
        let enriched = plain_to_enriched("line1\nline2");
        assert_eq!(enriched, "<nofill>line1\nline2</nofill>");
    }

    // -- flowed_to_plain ----------------------------------------------------

    #[test]
    fn flowed_to_plain_soft_wrap() {
        // Trailing space = soft break, lines should be joined
        let input = "Hello \nworld";
        let result = flowed_to_plain(input, false);
        assert!(result.contains("Hello world"));
    }

    #[test]
    fn flowed_to_plain_hard_break() {
        let input = "Hello\nworld";
        let result = flowed_to_plain(input, false);
        assert!(result.contains("Hello\nworld\n"));
    }

    #[test]
    fn flowed_to_plain_delsp() {
        let input = "Hello \nworld";
        let result = flowed_to_plain(input, true);
        // delsp removes trailing space
        assert!(result.contains("Helloworld"));
    }

    #[test]
    fn flowed_to_plain_quoted() {
        let input = "> quoted line\n> more";
        let result = flowed_to_plain(input, false);
        assert!(result.contains("> "));
        assert!(result.contains("quoted line"));
    }

    #[test]
    fn flowed_to_plain_sig_separator() {
        let input = "text\n-- \nSig";
        let result = flowed_to_plain(input, false);
        assert!(result.contains("-- \n"));
    }

    #[test]
    fn flowed_to_plain_empty() {
        assert_eq!(flowed_to_plain("", false), "");
    }

    // -- flowed_to_html -----------------------------------------------------

    #[test]
    fn flowed_to_html_basic() {
        let input = "Hello\nworld";
        let html = flowed_to_html(input, false);
        assert!(html.contains("Hello<br>\n"));
        assert!(html.contains("world<br>\n"));
    }

    #[test]
    fn flowed_to_html_soft_wrap() {
        let input = "Hello \nworld";
        let html = flowed_to_html(input, false);
        // Soft wrap: no <br> after first line
        assert!(html.contains("Hello "));
        assert!(!html.contains("Hello <br>"));
    }

    #[test]
    fn flowed_to_html_quoted() {
        let input = "> quoted";
        let html = flowed_to_html(input, false);
        assert!(html.contains("<blockquote>"));
        assert!(html.contains("quoted"));
        assert!(html.contains("</blockquote>"));
    }

    #[test]
    fn flowed_to_html_escapes_special() {
        let input = "<b> & stuff";
        let html = flowed_to_html(input, false);
        assert!(html.contains("&lt;b&gt; &amp; stuff"));
    }

    #[test]
    fn flowed_to_html_nested_quotes() {
        let input = "> level1\n>> level2";
        let html = flowed_to_html(input, false);
        // Should have nested blockquotes
        let bq_count = html.matches("<blockquote>").count();
        assert!(bq_count >= 2);
    }

    // -- plain_to_flowed ----------------------------------------------------

    #[test]
    fn plain_to_flowed_short_line() {
        let result = plain_to_flowed("Hello world\n", 72);
        assert_eq!(result, "Hello world\r\n\r\n");
    }

    #[test]
    fn plain_to_flowed_wraps_long_line() {
        let long = "a ".repeat(50); // 100 chars
        let result = plain_to_flowed(&long, 72);
        // Should have multiple CRLF-terminated lines
        let line_count = result.matches("\r\n").count();
        assert!(line_count > 1);
    }

    #[test]
    fn plain_to_flowed_preserves_quotes() {
        let input = "> quoted text\n";
        let result = plain_to_flowed(input, 72);
        assert!(result.contains("> quoted text\r\n"));
    }

    #[test]
    fn plain_to_flowed_default_col() {
        // max_col=0 should default to 72
        let result = plain_to_flowed("short\n", 0);
        assert!(result.contains("short\r\n"));
    }

    // -- html_to_plain ------------------------------------------------------

    #[test]
    fn html_to_plain_strips_tags() {
        let html = "<p>Hello <b>world</b></p>";
        let plain = html_to_plain(html);
        assert!(plain.contains("Hello world"));
    }

    #[test]
    fn html_to_plain_br_newline() {
        let html = "line1<br>line2";
        let plain = html_to_plain(html);
        assert!(plain.contains("line1\nline2"));
    }

    #[test]
    fn html_to_plain_entities() {
        let html = "&amp; &lt; &gt; &quot; &apos; &nbsp;";
        let plain = html_to_plain(html);
        assert!(plain.contains("& < > \" ' "));
    }

    #[test]
    fn html_to_plain_numeric_entity_ascii() {
        let html = "&#65;&#x42;";
        let plain = html_to_plain(html);
        assert!(plain.contains("AB"));
    }

    #[test]
    fn html_to_plain_numeric_entity_utf8() {
        // U+00E9 = e-acute
        let html = "caf&#xe9;";
        let plain = html_to_plain(html);
        assert!(plain.contains("caf\u{e9}"));
    }

    #[test]
    fn html_to_plain_collapses_whitespace() {
        let html = "hello   \n\n   world";
        let plain = html_to_plain(html);
        assert!(plain.contains("hello world"));
    }

    #[test]
    fn html_to_plain_pre_preserves_whitespace() {
        let html = "<pre>  two\n  spaces</pre>";
        let plain = html_to_plain(html);
        assert!(plain.contains("  two\n  spaces"));
    }

    #[test]
    fn html_to_plain_hr() {
        let html = "above<hr>below";
        let plain = html_to_plain(html);
        assert!(plain.contains("---"));
    }

    #[test]
    fn html_to_plain_list_items() {
        let html = "<ul><li>one<li>two</ul>";
        let plain = html_to_plain(html);
        assert!(plain.contains("- one"));
        assert!(plain.contains("- two"));
    }

    #[test]
    fn html_to_plain_heading() {
        let html = "<h1>Title</h1>text";
        let plain = html_to_plain(html);
        assert!(plain.contains("Title"));
    }

    // -- plain_to_html ------------------------------------------------------

    #[test]
    fn plain_to_html_basic() {
        let html = plain_to_html("Hello");
        assert_eq!(html, "<html><body><pre>Hello</pre></body></html>");
    }

    #[test]
    fn plain_to_html_escapes() {
        let html = plain_to_html("<b>&</b>");
        assert!(html.contains("&lt;b&gt;&amp;&lt;/b&gt;"));
    }

    #[test]
    fn plain_to_html_strips_cr() {
        let html = plain_to_html("a\r\nb");
        assert!(html.contains("a\nb"));
        assert!(!html.contains('\r'));
    }

    // -- round-trip tests ---------------------------------------------------

    #[test]
    fn roundtrip_plain_enriched_plain() {
        let original = "Hello <world> & friends\nLine 2";
        let enriched = plain_to_enriched(original);
        let recovered = enriched_to_plain(&enriched);
        assert_eq!(recovered, original);
    }

    #[test]
    fn roundtrip_enriched_html_enriched() {
        // Basic formatting should survive enriched->html->enriched
        let original = "<bold>hello</bold>";
        let html = enriched_to_html(original);
        let back = html_to_enriched(&html);
        assert!(back.contains("<bold>hello</bold>"));
    }

    // -- parse_color_param --------------------------------------------------

    #[test]
    fn color_param_rrggbb() {
        assert_eq!(parse_color_param("ff8800"), "#ff8800");
    }

    #[test]
    fn color_param_comma_format() {
        assert_eq!(parse_color_param("ff00,0000,8000"), "#ff0080");
    }

    #[test]
    fn color_param_short_fallback() {
        assert_eq!(parse_color_param("abc"), "#000000");
    }

    // -- edge cases ---------------------------------------------------------

    #[test]
    fn enriched_case_insensitive_tags() {
        let input = "<BOLD>text</BOLD>";
        let html = enriched_to_html(input);
        assert!(html.contains("<b>text</b>"));
    }

    #[test]
    fn enriched_nested_formatting() {
        let input = "<bold><italic>both</italic></bold>";
        let html = enriched_to_html(input);
        assert!(html.contains("<b><i>both</i></b>"));
    }

    #[test]
    fn flowed_crlf_handling() {
        let input = "Hello\r\nworld\r\n";
        let result = flowed_to_plain(input, false);
        assert!(result.contains("Hello\nworld\n"));
    }

    #[test]
    fn html_to_plain_empty() {
        assert_eq!(html_to_plain(""), "");
    }

    #[test]
    fn enriched_to_html_complex_email() {
        // Simulate a real enriched-text email body
        let input = concat!(
            "<bold>Subject: Meeting</bold>\n\n",
            "<italic>Date: Monday</italic>\n",
            "Please confirm.\n\n",
            "<excerpt>Previous message:\n",
            "Are you available?</excerpt>\n\n",
            "<color><param>0000,0000,ffff</param>Blue text</color>\n\n",
            "-- \n",
            "Signature",
        );
        let html = enriched_to_html(input);
        assert!(html.contains("<b>Subject: Meeting</b>"));
        assert!(html.contains("<i>Date: Monday</i>"));
        assert!(html.contains("<br>"));
        assert!(html.contains("<blockquote>"));
        assert!(html.contains("<span style=\"color:#0000ff\">Blue text</span>"));
    }
}
