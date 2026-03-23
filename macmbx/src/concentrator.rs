//! Message structure analysis and content concentrator.
//!
//! Ported from `macmbx_concentrator.c` / `macmbx_concentrator.h`.
//!
//! Analyzes message body text paragraph-by-paragraph to identify:
//! - Body text, quoted text (with nesting depth)
//! - Attributions ("On ... wrote:")
//! - Forwarded sections ("--- Begin forwarded message ---")
//! - Signature blocks ("-- ")
//! - Attachment references
//! - Digest separators
//!
//! Also provides message summarization with configurable profiles
//! (show/hide/truncate each element type).

// ══════════════════════════════════════════════════════════════════
// Paragraph types
// ══════════════════════════════════════════════════════════════════

/// Classification of a single paragraph/line in a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParaType {
    /// Regular body text.
    Body,
    /// Quoted text (`>`-prefixed).
    Quoted,
    /// Attribution line ("On Mon, John wrote:").
    Attribution,
    /// Forward begin marker ("--- Begin forwarded message ---").
    ForwardBegin,
    /// Forward end marker ("--- End forwarded message ---").
    ForwardEnd,
    /// Signature content (text after "-- " separator).
    Signature,
    /// The "-- " signature separator line itself.
    SigSeparator,
    /// Attachment reference ("Attachment converted: ...").
    Attachment,
    /// Digest separator ("----...").
    DigestSep,
    /// Empty/blank line.
    Blank,
    /// Header line (in forwarded section).
    Header,
    /// Body text inside a forwarded section.
    ForwardBody,
}

/// A single analyzed paragraph.
#[derive(Debug, Clone)]
pub struct Paragraph {
    /// Classification of this paragraph.
    pub para_type: ParaType,
    /// Quote nesting depth (0 = not quoted, 1 = `>`, 2 = `>>`, etc.).
    pub quote_depth: i32,
    /// Byte offset in original text.
    pub offset: usize,
    /// Total bytes including newline.
    pub length: usize,
    /// The text content (without trailing CR/LF).
    pub text: String,
    /// Length of text content (without trailing newline).
    pub text_len: usize,
}

/// Analyzed message structure.
#[derive(Debug)]
pub struct MsgStructure {
    /// All analyzed paragraphs.
    pub paras: Vec<Paragraph>,
    /// Count of body paragraphs.
    pub body_paras: usize,
    /// Count of quoted paragraphs.
    pub quoted_paras: usize,
    /// Maximum quote nesting depth found.
    pub max_quote_depth: i32,
    /// Whether a signature block was found.
    pub has_signature: bool,
    /// Whether a forwarded section was found.
    pub has_forwarded: bool,
    /// Whether attachment references were found.
    pub has_attachments: bool,
}

// ══════════════════════════════════════════════════════════════════
// Paragraph detection utilities
// ══════════════════════════════════════════════════════════════════

/// Count leading `>` quote characters. Returns the quote depth.
pub fn quote_depth(line: &[u8]) -> i32 {
    let mut depth = 0;
    for &b in line {
        if b == b'>' {
            depth += 1;
        } else {
            break;
        }
    }
    depth
}

/// Check if a line is all whitespace/blank.
pub fn is_blank(line: &[u8]) -> bool {
    line.iter().all(|&b| b == b' ' || b == b'\t' || b == b'\r' || b == b'\n')
}

/// Check if a line is a signature separator ("-- ").
pub fn is_sig_separator(line: &[u8]) -> bool {
    if line.len() < 3 {
        return false;
    }
    if line[0] != b'-' || line[1] != b'-' || line[2] != b' ' {
        return false;
    }
    // Rest should be whitespace/newline only
    line[3..].iter().all(|&b| b == b' ' || b == b'\r' || b == b'\n')
}

/// Check if a line is an attribution ("On Mon, John wrote:").
///
/// Criteria:
/// - Starts with alphanumeric character
/// - Ends with `:` (before trailing whitespace)
/// - Length 10-200
/// - Contains "wrote", "said", "writes", "schrieb", "a ecrit", "escribi",
///   or starts with "On " and ends with `:`
pub fn is_attribution(line: &[u8]) -> bool {
    if line.len() < 10 || line.len() > 200 {
        return false;
    }
    if !line[0].is_ascii_alphanumeric() {
        return false;
    }
    // Find end (strip trailing whitespace/newline)
    let mut end = line.len().saturating_sub(1);
    while end > 0 && (line[end] == b'\r' || line[end] == b'\n' || line[end] == b' ') {
        end -= 1;
    }
    if end < 5 || line[end] != b':' {
        return false;
    }

    let lower = String::from_utf8_lossy(line).to_lowercase();
    let keywords = ["wrote", "said", "writes", "schrieb", "a ecrit", "escribi"];
    for kw in &keywords {
        if lower.contains(kw) {
            return true;
        }
    }

    // "On <date>, <name>:" pattern
    if line.len() > 3 && line[..3].eq_ignore_ascii_case(b"On ") && line[end] == b':' {
        return true;
    }

    false
}

/// Check if a line is a forward begin marker.
pub fn is_forward_begin(line: &[u8]) -> bool {
    if line.len() < 10 || line.len() > 60 {
        return false;
    }
    if line[0] != b'-' || line[1] != b'-' || line[2] != b'-' {
        return false;
    }
    let lower = String::from_utf8_lossy(line).to_lowercase();
    // Exclude "end forwarded" / "end of forwarded" lines
    if lower.contains("end forwarded") || lower.contains("end of forwarded") {
        return false;
    }
    let patterns = [
        "begin forwarded",
        "original message",
        "forwarded message",
        "forwarded by",
        "start of forwarded",
    ];
    patterns.iter().any(|p| lower.contains(p))
}

/// Check if a line is a forward end marker.
pub fn is_forward_end(line: &[u8]) -> bool {
    if line.len() < 10 || line.len() > 60 {
        return false;
    }
    if line[0] != b'-' || line[1] != b'-' || line[2] != b'-' {
        return false;
    }
    let lower = String::from_utf8_lossy(line).to_lowercase();
    let patterns = [
        "end forwarded",
        "end of forwarded",
        "end original",
    ];
    patterns.iter().any(|p| lower.contains(p))
}

/// Check if a line is a digest separator (4+ dashes).
pub fn is_digest_separator(line: &[u8]) -> bool {
    if line.len() < 4 {
        return false;
    }
    let mut dashes = 0;
    for &b in line {
        if b == b'-' {
            dashes += 1;
        } else {
            break;
        }
    }
    if dashes < 4 {
        return false;
    }
    line[dashes..].iter().all(|&b| {
        b == b'-' || b == b' ' || b == b'\r' || b == b'\n'
    })
}

/// Check if a line is an attachment reference.
pub fn is_attachment(line: &[u8]) -> bool {
    if line.len() < 6 {
        return false;
    }
    let lower = String::from_utf8_lossy(line).to_lowercase();
    if lower.starts_with("attachment converted:") {
        return true;
    }
    if lower.starts_with("attachment ") {
        return true;
    }
    // [image: ...] inline markers
    if line[0] == b'[' && line.len() > 7 && line[1..7].eq_ignore_ascii_case(b"image:") {
        return true;
    }
    // <<filename.pdf>> MIME markers
    if line.len() > 4 && line[0] == b'<' && line[1] == b'<' && line.len() < 80 {
        for i in 2..line.len().saturating_sub(1) {
            if line[i] == b'>' && i + 1 < line.len() && line[i + 1] == b'>' {
                return true;
            }
        }
    }
    false
}

// ══════════════════════════════════════════════════════════════════
// Analyze message structure
// ══════════════════════════════════════════════════════════════════

/// Analyze a message body text into structured paragraphs.
///
/// `text` should be the plain text message body (after headers).
/// Returns an [`MsgStructure`] with classified paragraphs and summary stats.
pub fn analyze(text: &[u8]) -> MsgStructure {
    let mut ms = MsgStructure {
        paras: Vec::new(),
        body_paras: 0,
        quoted_paras: 0,
        max_quote_depth: 0,
        has_signature: false,
        has_forwarded: false,
        has_attachments: false,
    };

    let mut pos = 0;
    let mut in_signature = false;
    let mut in_forward = false;

    while pos < text.len() {
        let line_start = pos;
        // Find end of line
        while pos < text.len() && text[pos] != b'\n' {
            pos += 1;
        }
        let line_len = pos - line_start;
        if pos < text.len() {
            pos += 1; // skip \n
        }
        let total_len = pos - line_start;

        // Strip trailing CR
        let mut content_len = line_len;
        while content_len > 0 && text[line_start + content_len - 1] == b'\r' {
            content_len -= 1;
        }

        let line_bytes = &text[line_start..line_start + content_len];
        let text_str = String::from_utf8_lossy(line_bytes).into_owned();

        let para_type;

        if is_blank(line_bytes) {
            para_type = ParaType::Blank;
        } else if in_signature {
            para_type = ParaType::Signature;
        } else if is_sig_separator(line_bytes) {
            para_type = ParaType::SigSeparator;
            in_signature = true;
            ms.has_signature = true;
        } else if is_forward_begin(line_bytes) {
            para_type = ParaType::ForwardBegin;
            in_forward = true;
            ms.has_forwarded = true;
        } else if is_forward_end(line_bytes) {
            para_type = ParaType::ForwardEnd;
            in_forward = false;
        } else if is_attachment(line_bytes) {
            para_type = ParaType::Attachment;
            ms.has_attachments = true;
        } else if is_digest_separator(line_bytes) {
            para_type = ParaType::DigestSep;
        } else if in_forward
            && content_len > 0
            && line_bytes.contains(&b':')
            && !line_bytes[0].is_ascii_whitespace()
            && !ms.paras.is_empty()
            && ms.paras.last().unwrap().para_type == ParaType::ForwardBegin
        {
            para_type = ParaType::Header;
        } else if in_forward {
            para_type = ParaType::ForwardBody;
        } else {
            let depth = quote_depth(line_bytes);
            if depth > 0 {
                para_type = ParaType::Quoted;
                if depth > ms.max_quote_depth {
                    ms.max_quote_depth = depth;
                }
                ms.quoted_paras += 1;
            } else if is_attribution(line_bytes) {
                para_type = ParaType::Attribution;
            } else {
                para_type = ParaType::Body;
                ms.body_paras += 1;
            }
        }

        let para = Paragraph {
            para_type,
            quote_depth: if para_type == ParaType::Quoted {
                quote_depth(line_bytes)
            } else {
                0
            },
            offset: line_start,
            length: total_len,
            text: text_str,
            text_len: content_len,
        };

        ms.paras.push(para);
    }

    ms
}

/// Free-standing convenience: same as [`analyze`].
pub fn concentrator_analyze(text: &str) -> MsgStructure {
    analyze(text.as_bytes())
}

// ══════════════════════════════════════════════════════════════════
// Concentration profiles
// ══════════════════════════════════════════════════════════════════

/// Summary profile controlling what to include in concentrated output.
#[derive(Debug, Clone)]
pub struct Profile {
    /// Show body text.
    pub show_body: bool,
    /// Show quoted text.
    pub show_quoted: bool,
    /// Hide quotes deeper than this (0 = show all).
    pub max_quote_depth: i32,
    /// Show attribution lines.
    pub show_attribution: bool,
    /// Show signature block.
    pub show_signature: bool,
    /// Show forwarded sections.
    pub show_forwarded: bool,
    /// Show attachment references.
    pub show_attachments: bool,
    /// Truncate body at N chars (0 = no limit).
    pub max_body_chars: usize,
    /// Max body paragraphs (0 = no limit).
    pub max_body_paras: usize,
    /// Truncate quoted at N chars (0 = no limit).
    pub max_quoted_chars: usize,
    /// Add "..." when truncating.
    pub ellipsis: bool,
}

impl Profile {
    /// Default profile: show everything.
    pub fn default_profile() -> Self {
        Self {
            show_body: true,
            show_quoted: true,
            max_quote_depth: 0,
            show_attribution: true,
            show_signature: true,
            show_forwarded: true,
            show_attachments: true,
            max_body_chars: 0,
            max_body_paras: 0,
            max_quoted_chars: 0,
            ellipsis: true,
        }
    }

    /// Preview profile: body only, truncated.
    pub fn preview(max_chars: usize) -> Self {
        let max_chars = if max_chars > 0 { max_chars } else { 200 };
        Self {
            show_body: true,
            show_quoted: false,
            max_quote_depth: 0,
            show_attribution: false,
            show_signature: false,
            show_forwarded: false,
            show_attachments: false,
            max_body_chars: max_chars,
            max_body_paras: 3,
            max_quoted_chars: 0,
            ellipsis: true,
        }
    }

    /// Notification profile: first paragraph only.
    pub fn notify(max_chars: usize) -> Self {
        let max_chars = if max_chars > 0 { max_chars } else { 100 };
        Self {
            show_body: true,
            show_quoted: false,
            max_quote_depth: 0,
            show_attribution: false,
            show_signature: false,
            show_forwarded: false,
            show_attachments: false,
            max_body_chars: max_chars,
            max_body_paras: 1,
            max_quoted_chars: 0,
            ellipsis: true,
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// Concentrate: apply profile to produce summary
// ══════════════════════════════════════════════════════════════════

/// Apply a [`Profile`] to an analyzed [`MsgStructure`] to produce a summary string.
///
/// Walks through all paragraphs, includes/excludes based on the profile,
/// respects truncation limits, and optionally appends "..." on truncation.
pub fn concentrate(ms: &MsgStructure, profile: &Profile) -> String {
    let mut out = String::with_capacity(2048);
    let mut body_chars: usize = 0;
    let mut body_paras: usize = 0;
    let mut quoted_chars: usize = 0;
    let mut truncated = false;

    for para in &ms.paras {
        if truncated {
            break;
        }
        let include = match para.para_type {
            ParaType::Body => {
                if !profile.show_body {
                    false
                } else if profile.max_body_paras > 0 && body_paras >= profile.max_body_paras {
                    truncated = true;
                    false
                } else if profile.max_body_chars > 0 && body_chars >= profile.max_body_chars {
                    truncated = true;
                    false
                } else {
                    body_paras += 1;
                    true
                }
            }
            ParaType::Quoted => {
                if !profile.show_quoted {
                    false
                } else if profile.max_quote_depth > 0
                    && para.quote_depth > profile.max_quote_depth
                {
                    false
                } else if profile.max_quoted_chars > 0
                    && quoted_chars >= profile.max_quoted_chars
                {
                    false
                } else {
                    true
                }
            }
            ParaType::Attribution => profile.show_attribution,
            ParaType::ForwardBegin | ParaType::ForwardEnd | ParaType::ForwardBody => profile.show_forwarded,
            ParaType::SigSeparator | ParaType::Signature => profile.show_signature,
            ParaType::Attachment => profile.show_attachments,
            ParaType::Blank => {
                // Include blanks only between included content
                !out.is_empty() && !out.ends_with('\n')
            }
            ParaType::DigestSep | ParaType::Header => profile.show_forwarded,
        };

        if include {
            let mut add_len = para.text_len;

            // Check truncation limits for body
            if para.para_type == ParaType::Body && profile.max_body_chars > 0 {
                let remaining = profile.max_body_chars.saturating_sub(body_chars);
                if add_len > remaining {
                    add_len = remaining;
                    // Truncate at word boundary
                    let text_bytes = para.text.as_bytes();
                    let mut trunc = add_len;
                    while trunc > 0 && text_bytes.get(trunc - 1) != Some(&b' ') {
                        trunc -= 1;
                    }
                    if trunc > 0 {
                        add_len = trunc;
                    }
                    // else keep original add_len
                    truncated = true;
                }
                body_chars += add_len;
            }

            // Check truncation limits for quoted
            if para.para_type == ParaType::Quoted && profile.max_quoted_chars > 0 {
                let remaining = profile.max_quoted_chars.saturating_sub(quoted_chars);
                if add_len > remaining {
                    add_len = remaining;
                }
                quoted_chars += add_len;
            }

            // Add text (up to add_len), ensuring we don't split UTF-8
            let safe_len = if add_len < para.text.len() {
                // Walk back to a char boundary
                let mut l = add_len;
                while l > 0 && !para.text.is_char_boundary(l) {
                    l -= 1;
                }
                l
            } else {
                para.text.len()
            };
            let text_to_add = &para.text[..safe_len];
            out.push_str(text_to_add);
            out.push('\n');
        }
    }

    // Add ellipsis if truncated
    if truncated && profile.ellipsis {
        out.push_str("...");
    }

    // Trim trailing blank lines
    while out.ends_with("\n\n") {
        out.pop();
    }

    out
}

/// Shortcut: analyze + concentrate in one call.
pub fn summarize(text: &str, profile: &Profile) -> String {
    let ms = analyze(text.as_bytes());
    concentrate(&ms, profile)
}

// ══════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_depth() {
        assert_eq!(quote_depth(b"Hello"), 0);
        assert_eq!(quote_depth(b"> Hello"), 1);
        assert_eq!(quote_depth(b">> Hello"), 2);
        assert_eq!(quote_depth(b">>> Hello"), 3);
        assert_eq!(quote_depth(b""), 0);
    }

    #[test]
    fn test_is_blank() {
        assert!(is_blank(b""));
        assert!(is_blank(b"   "));
        assert!(is_blank(b"\t  \r\n"));
        assert!(!is_blank(b"Hello"));
        assert!(!is_blank(b" x "));
    }

    #[test]
    fn test_is_sig_separator() {
        assert!(is_sig_separator(b"-- "));
        assert!(is_sig_separator(b"-- \n"));
        assert!(is_sig_separator(b"-- \r\n"));
        assert!(!is_sig_separator(b"--"));
        assert!(!is_sig_separator(b"--- "));
        assert!(!is_sig_separator(b"-- x"));
    }

    #[test]
    fn test_is_attribution() {
        assert!(is_attribution(b"On Mon, Jan 1, John wrote:"));
        assert!(is_attribution(b"Alice said:        \n"));
        assert!(is_attribution(b"Bob writes something:"));
        assert!(is_attribution(b"Hans schrieb am Montag:"));
        assert!(is_attribution(b"On 2024-01-01, someone@example.com:"));
        assert!(!is_attribution(b"Short:"));
        assert!(!is_attribution(b""));
        assert!(!is_attribution(b" On Mon, Jan 1, wrote:")); // starts with space
    }

    #[test]
    fn test_is_forward_begin() {
        assert!(is_forward_begin(b"--- Begin forwarded message ---"));
        assert!(is_forward_begin(b"--- Original Message ---"));
        assert!(is_forward_begin(b"--- Forwarded message ---"));
        assert!(!is_forward_begin(b"Hello world"));
        assert!(!is_forward_begin(b"--")); // too short
    }

    #[test]
    fn test_is_forward_end() {
        assert!(is_forward_end(b"--- End forwarded message ---"));
        assert!(is_forward_end(b"--- End of forwarded message ---"));
        assert!(!is_forward_end(b"Hello"));
        assert!(!is_forward_end(b"--- Begin forwarded message ---"));
    }

    #[test]
    fn test_is_digest_separator() {
        assert!(is_digest_separator(b"----"));
        assert!(is_digest_separator(b"----------"));
        assert!(is_digest_separator(b"---- -----"));
        assert!(!is_digest_separator(b"---")); // only 3 dashes
        assert!(!is_digest_separator(b"----abc"));
    }

    #[test]
    fn test_is_attachment() {
        assert!(is_attachment(b"Attachment converted: /path/to/file.pdf"));
        assert!(is_attachment(b"Attachment name: document.docx"));
        assert!(is_attachment(b"[image: photo.jpg]"));
        assert!(is_attachment(b"<<report.pdf>>"));
        assert!(!is_attachment(b"Hello world"));
        assert!(!is_attachment(b"Short")); // too short
    }

    #[test]
    fn test_analyze_basic() {
        let text = b"Hello world\n\n> quoted text\n-- \nSig line\n";
        let ms = analyze(text);
        assert_eq!(ms.paras.len(), 5);
        assert_eq!(ms.paras[0].para_type, ParaType::Body);
        assert_eq!(ms.paras[1].para_type, ParaType::Blank);
        assert_eq!(ms.paras[2].para_type, ParaType::Quoted);
        assert_eq!(ms.paras[2].quote_depth, 1);
        assert_eq!(ms.paras[3].para_type, ParaType::SigSeparator);
        assert_eq!(ms.paras[4].para_type, ParaType::Signature);
        assert!(ms.has_signature);
        assert_eq!(ms.body_paras, 1);
        assert_eq!(ms.quoted_paras, 1);
        assert_eq!(ms.max_quote_depth, 1);
    }

    #[test]
    fn test_analyze_forwarded() {
        let text = b"Hello\n--- Begin forwarded message ---\nFrom: someone\nBody here\n--- End forwarded message ---\n";
        let ms = analyze(text);
        assert!(ms.has_forwarded);
        assert!(ms.paras.iter().any(|p| p.para_type == ParaType::ForwardBegin));
        assert!(ms.paras.iter().any(|p| p.para_type == ParaType::ForwardEnd));
        assert!(ms.paras.iter().any(|p| p.para_type == ParaType::Header));
    }

    #[test]
    fn test_analyze_deep_quotes() {
        let text = b"> level 1\n>> level 2\n>>> level 3\n";
        let ms = analyze(text);
        assert_eq!(ms.max_quote_depth, 3);
        assert_eq!(ms.quoted_paras, 3);
    }

    #[test]
    fn test_analyze_attachment() {
        let text = b"Hello\nAttachment converted: /path/to/file.pdf\nBye\n";
        let ms = analyze(text);
        assert!(ms.has_attachments);
        assert!(ms.paras.iter().any(|p| p.para_type == ParaType::Attachment));
    }

    #[test]
    fn test_profile_default() {
        let profile = Profile::default_profile();
        assert!(profile.show_body);
        assert!(profile.show_quoted);
        assert!(profile.show_attribution);
        assert!(profile.show_signature);
        assert!(profile.show_forwarded);
        assert!(profile.show_attachments);
        assert!(profile.ellipsis);
    }

    #[test]
    fn test_profile_preview() {
        let profile = Profile::preview(0);
        assert!(profile.show_body);
        assert!(!profile.show_quoted);
        assert!(!profile.show_signature);
        assert_eq!(profile.max_body_chars, 200);
        assert_eq!(profile.max_body_paras, 3);
    }

    #[test]
    fn test_profile_notify() {
        let profile = Profile::notify(50);
        assert!(profile.show_body);
        assert!(!profile.show_quoted);
        assert_eq!(profile.max_body_chars, 50);
        assert_eq!(profile.max_body_paras, 1);
    }

    #[test]
    fn test_concentrate_default() {
        let text = b"Hello world\n> quoted\n-- \nSig\n";
        let ms = analyze(text);
        let profile = Profile::default_profile();
        let result = concentrate(&ms, &profile);
        assert!(result.contains("Hello world"));
        assert!(result.contains("> quoted"));
        assert!(result.contains("-- "));
        assert!(result.contains("Sig"));
    }

    #[test]
    fn test_concentrate_preview() {
        let text = b"First paragraph body.\nSecond paragraph body.\n> Some quote\n-- \nSignature\n";
        let ms = analyze(text);
        let profile = Profile::preview(200);
        let result = concentrate(&ms, &profile);
        assert!(result.contains("First paragraph"));
        assert!(result.contains("Second paragraph"));
        assert!(!result.contains("> Some quote"));
        assert!(!result.contains("Signature"));
    }

    #[test]
    fn test_concentrate_notify() {
        let text = b"First line of body.\nSecond line of body.\nThird.\n";
        let ms = analyze(text);
        let profile = Profile::notify(100);
        let result = concentrate(&ms, &profile);
        assert!(result.contains("First line"));
        // Notify allows only 1 para
        assert!(!result.contains("Second line"));
        assert!(result.contains("..."));
    }

    #[test]
    fn test_concentrate_truncation() {
        let text = b"This is a longer body text that should be truncated at a word boundary.\n";
        let ms = analyze(text);
        let profile = Profile::notify(20);
        let result = concentrate(&ms, &profile);
        // Should be truncated and have ellipsis
        assert!(result.len() < 80);
        assert!(result.contains("..."));
    }

    #[test]
    fn test_concentrate_no_ellipsis() {
        let text = b"Short body.\nAnother para.\n";
        let ms = analyze(text);
        let mut profile = Profile::notify(100);
        profile.ellipsis = false;
        let result = concentrate(&ms, &profile);
        assert!(!result.contains("..."));
    }

    #[test]
    fn test_concentrate_hide_quoted() {
        let text = b"Body text.\n> Quoted line 1.\n>> Deeply quoted.\nMore body.\n";
        let ms = analyze(text);
        let mut profile = Profile::default_profile();
        profile.show_quoted = false;
        let result = concentrate(&ms, &profile);
        assert!(result.contains("Body text"));
        assert!(result.contains("More body"));
        assert!(!result.contains("> Quoted"));
        assert!(!result.contains(">> Deeply"));
    }

    #[test]
    fn test_concentrate_max_quote_depth() {
        let text = b"> Level 1\n>> Level 2\n>>> Level 3\n";
        let ms = analyze(text);
        let mut profile = Profile::default_profile();
        profile.max_quote_depth = 2;
        let result = concentrate(&ms, &profile);
        assert!(result.contains("> Level 1"));
        assert!(result.contains(">> Level 2"));
        assert!(!result.contains(">>> Level 3"));
    }

    #[test]
    fn test_concentrate_hide_signature() {
        let text = b"Body.\n-- \nMy Signature\n";
        let ms = analyze(text);
        let mut profile = Profile::default_profile();
        profile.show_signature = false;
        let result = concentrate(&ms, &profile);
        assert!(result.contains("Body."));
        assert!(!result.contains("-- "));
        assert!(!result.contains("My Signature"));
    }

    #[test]
    fn test_concentrate_hide_forwarded() {
        let text = b"Body.\n--- Begin forwarded message ---\nForwarded content.\n--- End forwarded message ---\n";
        let ms = analyze(text);
        let mut profile = Profile::default_profile();
        profile.show_forwarded = false;
        let result = concentrate(&ms, &profile);
        assert!(result.contains("Body."));
        assert!(!result.contains("Begin forwarded"));
        assert!(!result.contains("Forwarded content"));
    }

    #[test]
    fn test_concentrate_hide_attachments() {
        let text = b"Body.\nAttachment converted: /path/to/file.pdf\nMore body.\n";
        let ms = analyze(text);
        let mut profile = Profile::default_profile();
        profile.show_attachments = false;
        let result = concentrate(&ms, &profile);
        assert!(result.contains("Body."));
        assert!(result.contains("More body"));
        assert!(!result.contains("Attachment converted"));
    }

    #[test]
    fn test_summarize_shortcut() {
        let text = "Hello world.\n> quote\n-- \nSig\n";
        let profile = Profile::preview(200);
        let result = summarize(text, &profile);
        assert!(result.contains("Hello world"));
        assert!(!result.contains("> quote"));
        assert!(!result.contains("Sig"));
    }

    #[test]
    fn test_analyze_empty() {
        let ms = analyze(b"");
        assert_eq!(ms.paras.len(), 0);
        assert_eq!(ms.body_paras, 0);
    }

    #[test]
    fn test_analyze_only_blank_lines() {
        let ms = analyze(b"\n\n\n");
        assert_eq!(ms.paras.len(), 3);
        assert!(ms.paras.iter().all(|p| p.para_type == ParaType::Blank));
    }

    #[test]
    fn test_concentrate_empty() {
        let ms = analyze(b"");
        let profile = Profile::default_profile();
        let result = concentrate(&ms, &profile);
        assert!(result.is_empty());
    }

    #[test]
    fn test_analyze_crlf() {
        let text = b"Hello\r\n> Quote\r\n-- \r\nSig\r\n";
        let ms = analyze(text);
        assert_eq!(ms.paras.len(), 4);
        assert_eq!(ms.paras[0].para_type, ParaType::Body);
        assert_eq!(ms.paras[0].text, "Hello");
        assert_eq!(ms.paras[1].para_type, ParaType::Quoted);
        assert_eq!(ms.paras[2].para_type, ParaType::SigSeparator);
        assert_eq!(ms.paras[3].para_type, ParaType::Signature);
    }

    #[test]
    fn test_concentrate_max_quoted_chars() {
        let text = b"> This is a very long quoted line that should be truncated.\n> Another long quote.\n";
        let ms = analyze(text);
        let mut profile = Profile::default_profile();
        profile.max_quoted_chars = 20;
        let result = concentrate(&ms, &profile);
        // The quoted content should be limited
        let quoted_content: String = result
            .lines()
            .filter(|l| l.starts_with('>'))
            .collect::<Vec<_>>()
            .join("");
        // Should have something but limited
        assert!(!quoted_content.is_empty());
    }

    #[test]
    fn test_digest_separator() {
        let text = b"Message 1 body.\n--------\nMessage 2 body.\n";
        let ms = analyze(text);
        assert!(ms.paras.iter().any(|p| p.para_type == ParaType::DigestSep));
    }

    #[test]
    fn test_attribution_various_languages() {
        // German
        assert!(is_attribution(b"Hans Mueller schrieb am Montag:"));
        // "On ... wrote:" in English
        assert!(is_attribution(b"On Mon, Jan 1, 2024, Alice wrote:"));
    }
}
