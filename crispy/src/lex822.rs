//! # RFC 822 Header Lexical Analyzer
//!
//! Tokenizes RFC 822 header fields into atoms, quoted strings, comments,
//! domain literals, specials, and whitespace. Standalone — works on string
//! slices, no transport dependency.
//!
//! Also provides: RFC 822 quoting, pseudo-QP fixing for broken headers,
//! and inline base64 decoding (for RFC 2047 B-encoded fragments).
//!
//! Ported from `crispy_lex822.c`.

/// Token types produced by the lexer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    /// Linear whitespace (space, tab, folding).
    Lwsp,
    /// Unquoted word (alphanumeric + allowed chars).
    Atom,
    /// Quoted string (content between quotes, unescaped).
    QString,
    /// RFC 822 comment (content between parens, nested supported).
    Comment,
    /// Domain literal `[content]`.
    DomainLiteral,
    /// Single special character: `( ) < > @ , ; : \ " . [ ]`
    Special,
    /// Unstructured text (body of unstructured headers).
    Text,
}

/// A single token produced by the lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The type of this token.
    pub token_type: TokenType,
    /// Byte offset of the token start within the original input.
    pub offset: usize,
    /// Length of the raw token text in the original input (bytes).
    pub raw_len: usize,
    /// Processed value (unescaped/decoded content).
    pub value: String,
}

// ================================================================
// Character classification (RFC 822)
// ================================================================

/// Returns `true` if `c` is an RFC 822 special character.
pub fn is_special(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'@' | b',' | b';' | b':' | b'\\' | b'"' | b'.' | b'['
            | b']'
    )
}

/// Returns `true` if `c` is linear whitespace (space or tab).
fn is_lwsp(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// Returns `true` if `c` is a valid atom character (printable, non-special, non-space).
fn is_atom_char(c: u8) -> bool {
    c > 32 && c != 127 && !is_special(c)
}

/// Check if a string needs quoting for RFC 822.
///
/// Returns `true` if the string contains any special or whitespace character.
pub fn needs_quoting(s: &str) -> bool {
    s.bytes().any(|c| is_special(c) || is_lwsp(c))
}

// ================================================================
// Lexer
// ================================================================

/// RFC 822 header lexical analyzer.
///
/// Operates on a borrowed string slice. Supports two modes:
/// - **Structured** (`structured = true`): for address and content-type headers
///   where specials, atoms, quoted strings, comments, and domain literals are
///   individually recognized.
/// - **Unstructured** (`structured = false`): for Subject/Comments headers where
///   the entire content (minus folding) is returned as text.
///
/// # Example
/// ```
/// use crispy::lex822::{Lexer, TokenType};
///
/// let mut lexer = Lexer::new("user@example.com", true);
/// let tokens: Vec<_> = lexer.collect();
/// assert_eq!(tokens[0].token_type, TokenType::Atom);
/// assert_eq!(tokens[0].value, "user");
/// assert_eq!(tokens[1].token_type, TokenType::Special);
/// assert_eq!(tokens[1].value, "@");
/// ```
pub struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
    structured: bool,
}

impl<'a> Lexer<'a> {
    /// Create a new lexer over `header_value`.
    ///
    /// Set `structured` to `true` for address/content-type headers, `false`
    /// for unstructured headers like Subject.
    pub fn new(header_value: &'a str, structured: bool) -> Self {
        Lexer {
            input: header_value.as_bytes(),
            pos: 0,
            structured,
        }
    }

    /// Reset the lexer to the beginning of the input.
    pub fn reset(&mut self) {
        self.pos = 0;
    }

    /// Peek at the next token type without consuming it.
    pub fn peek_type(&mut self) -> Option<TokenType> {
        let saved = self.pos;
        let tt = self.next().map(|t| t.token_type);
        self.pos = saved;
        tt
    }

    /// Skip folding whitespace: CRLF (or bare LF) followed by space/tab.
    fn skip_fws(&mut self) {
        while self.pos < self.input.len() {
            if self.input[self.pos] == b'\r'
                && self.pos + 1 < self.input.len()
                && self.input[self.pos + 1] == b'\n'
                && self.pos + 2 < self.input.len()
                && is_lwsp(self.input[self.pos + 2])
            {
                self.pos += 2; // skip CRLF; space/tab consumed as LWSP
            } else if self.input[self.pos] == b'\n'
                && self.pos + 1 < self.input.len()
                && is_lwsp(self.input[self.pos + 1])
            {
                self.pos += 1; // skip LF; space/tab next
            } else {
                break;
            }
        }
    }

    /// Lex whitespace token.
    fn lex_lwsp(&mut self, start: usize) -> Token {
        while self.pos < self.input.len() && is_lwsp(self.input[self.pos]) {
            self.pos += 1;
            self.skip_fws();
        }
        Token {
            token_type: TokenType::Lwsp,
            offset: start,
            raw_len: self.pos - start,
            value: " ".to_string(), // normalized to single space
        }
    }

    /// Lex quoted string token (opening `"` already identified at `start`).
    fn lex_qstring(&mut self, start: usize) -> Token {
        self.pos += 1; // skip opening quote
        let mut value = String::new();
        while self.pos < self.input.len() && self.input[self.pos] != b'"' {
            if self.input[self.pos] == b'\\' && self.pos + 1 < self.input.len() {
                self.pos += 1; // skip backslash
                value.push(self.input[self.pos] as char);
            } else {
                value.push(self.input[self.pos] as char);
            }
            self.pos += 1;
        }
        if self.pos < self.input.len() {
            self.pos += 1; // skip closing quote
        }
        Token {
            token_type: TokenType::QString,
            offset: start,
            raw_len: self.pos - start,
            value,
        }
    }

    /// Lex comment token (opening `(` already identified at `start`).
    fn lex_comment(&mut self, start: usize) -> Token {
        self.pos += 1; // skip (
        let mut depth: usize = 1;
        let mut value = String::new();
        while self.pos < self.input.len() && depth > 0 {
            let c = self.input[self.pos];
            if c == b'(' {
                depth += 1;
                value.push('(');
                self.pos += 1;
            } else if c == b')' {
                depth -= 1;
                if depth > 0 {
                    value.push(')');
                }
                self.pos += 1;
            } else if c == b'\\' && self.pos + 1 < self.input.len() {
                self.pos += 1;
                value.push(self.input[self.pos] as char);
                self.pos += 1;
            } else {
                value.push(c as char);
                self.pos += 1;
            }
        }
        Token {
            token_type: TokenType::Comment,
            offset: start,
            raw_len: self.pos - start,
            value,
        }
    }

    /// Lex domain literal token (opening `[` already identified at `start`).
    fn lex_domain_literal(&mut self, start: usize) -> Token {
        self.pos += 1; // skip [
        let mut value = String::new();
        while self.pos < self.input.len() && self.input[self.pos] != b']' {
            if self.input[self.pos] == b'\\' && self.pos + 1 < self.input.len() {
                self.pos += 1;
                value.push(self.input[self.pos] as char);
            } else {
                value.push(self.input[self.pos] as char);
            }
            self.pos += 1;
        }
        if self.pos < self.input.len() {
            self.pos += 1; // skip ]
        }
        Token {
            token_type: TokenType::DomainLiteral,
            offset: start,
            raw_len: self.pos - start,
            value,
        }
    }

    /// Lex a special character token.
    fn lex_special(&mut self, start: usize) -> Token {
        let c = self.input[self.pos];
        self.pos += 1;
        Token {
            token_type: TokenType::Special,
            offset: start,
            raw_len: 1,
            value: String::from(c as char),
        }
    }

    /// Lex an atom token (structured mode).
    fn lex_atom(&mut self, start: usize) -> Token {
        let mut value = String::new();
        while self.pos < self.input.len() && is_atom_char(self.input[self.pos]) {
            value.push(self.input[self.pos] as char);
            self.pos += 1;
        }
        Token {
            token_type: TokenType::Atom,
            offset: start,
            raw_len: self.pos - start,
            value,
        }
    }

    /// Lex unstructured text (unstructured mode).
    fn lex_text(&mut self, start: usize) -> Token {
        let mut value = String::new();
        while self.pos < self.input.len() {
            let c = self.input[self.pos];
            if c == b'\r' || c == b'\n' {
                let saved = self.pos;
                self.skip_fws();
                if self.pos > saved
                    && self.pos < self.input.len()
                    && is_lwsp(self.input[self.pos])
                {
                    value.push(' ');
                    self.pos += 1;
                    continue;
                }
                if self.pos == saved {
                    break; // real end of header
                }
                continue;
            }
            value.push(c as char);
            self.pos += 1;
        }
        Token {
            token_type: TokenType::Text,
            offset: start,
            raw_len: self.pos - start,
            value,
        }
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        if self.pos >= self.input.len() {
            return None;
        }

        self.skip_fws();
        if self.pos >= self.input.len() {
            return None;
        }

        let c = self.input[self.pos];
        let start = self.pos;

        // Whitespace
        if is_lwsp(c) {
            return Some(self.lex_lwsp(start));
        }

        // Quoted string
        if c == b'"' {
            return Some(self.lex_qstring(start));
        }

        // Comment (structured mode only)
        if c == b'(' && self.structured {
            return Some(self.lex_comment(start));
        }

        // Domain literal (structured mode only)
        if c == b'[' && self.structured {
            return Some(self.lex_domain_literal(start));
        }

        // Special characters (structured mode, excluding already handled delimiters)
        if self.structured && is_special(c) && c != b'"' && c != b'(' && c != b'[' {
            return Some(self.lex_special(start));
        }

        // Atom (structured) or text (unstructured)
        if self.structured {
            return Some(self.lex_atom(start));
        }

        Some(self.lex_text(start))
    }
}

// ================================================================
// Convenience: tokenize entire header
// ================================================================

/// Tokenize a header value into a `Vec` of tokens.
///
/// `structured` controls whether the header is parsed as structured
/// (addresses, content-type) or unstructured (subject, comments).
pub fn tokenize(header_value: &str, structured: bool) -> Vec<Token> {
    Lexer::new(header_value, structured).collect()
}

// ================================================================
// RFC 822 quoting
// ================================================================

/// Quote a string for RFC 822.
///
/// Wraps `src` in double quotes, escaping internal `"` and `\` characters.
/// If `escape_spaces` is true, spaces are also backslash-escaped.
///
/// # Example
/// ```
/// use crispy::lex822::quote;
/// assert_eq!(quote("hello world", false), r#""hello world""#);
/// assert_eq!(quote(r#"say "hi""#, false), r#""say \"hi\"""#);
/// ```
pub fn quote(src: &str, escape_spaces: bool) -> String {
    let mut out = String::with_capacity(src.len() + 4);
    out.push('"');
    for c in src.bytes() {
        if c == b'"' || c == b'\\' {
            out.push('\\');
            out.push(c as char);
        } else if escape_spaces && c == b' ' {
            out.push('\\');
            out.push(' ');
        } else {
            out.push(c as char);
        }
    }
    out.push('"');
    out
}

// ================================================================
// PseudoQP — fix broken QP in headers
// ================================================================

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// Fix broken quoted-printable in headers.
///
/// Some spam has raw `=XX` sequences in unencoded headers. This decodes
/// them in place, returning a new string with the decoded bytes.
/// Only decodes to printable characters (>= 32) or tab.
///
/// # Example
/// ```
/// use crispy::lex822::fix_pseudo_qp;
/// assert_eq!(fix_pseudo_qp("hello=20world"), "hello world");
/// ```
pub fn fix_pseudo_qp(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                let decoded = (h << 4) | l;
                if decoded >= 32 || decoded == b'\t' {
                    out.push(decoded);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // Safety: we only produce bytes that were already in the input or printable ASCII
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

// ================================================================
// Inline base64 decode
// ================================================================

/// Base64 character to 6-bit value.
fn b64_val(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => 0,
    }
}

fn is_b64_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'='
}

/// Decode a base64-encoded string inline (for RFC 2047 B-encoded fragments
/// in headers).
///
/// Returns a `Vec<u8>` of decoded bytes.
///
/// # Example
/// ```
/// use crispy::lex822::decode_b64_inline;
/// assert_eq!(decode_b64_inline("SGVsbG8="), b"Hello");
/// ```
pub fn decode_b64_inline(s: &str) -> Vec<u8> {
    let src = s.as_bytes();
    let mut out = Vec::with_capacity(src.len() * 3 / 4 + 4);
    let mut i = 0;
    let len = src.len();

    while i < len {
        // Skip non-base64 chars
        while i < len && !is_b64_char(src[i]) {
            i += 1;
        }
        if i >= len {
            break;
        }

        let a = if i < len {
            let v = b64_val(src[i]);
            i += 1;
            v
        } else {
            0
        };
        let b = if i < len {
            let v = b64_val(src[i]);
            i += 1;
            v
        } else {
            0
        };
        let c_char = if i < len { src[i] } else { b'=' };
        let c = if i < len {
            let v = b64_val(src[i]);
            i += 1;
            v
        } else {
            0
        };
        let d_char = if i < len { src[i] } else { b'=' };
        let _d_val = if i < len {
            let v = b64_val(src[i]);
            i += 1;
            v
        } else {
            0
        };

        out.push((a << 2) | (b >> 4));
        if c_char != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if d_char != b'=' {
            out.push((c << 6) | _d_val);
        }
    }
    out
}

// ================================================================
// Tests
// ================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Character classification ----

    #[test]
    fn test_is_special() {
        let specials = b"()<>@,;:\\\".[]";
        for &c in specials {
            assert!(is_special(c), "expected special: {}", c as char);
        }
        assert!(!is_special(b'a'));
        assert!(!is_special(b'0'));
        assert!(!is_special(b' '));
    }

    #[test]
    fn test_needs_quoting() {
        assert!(needs_quoting("hello world"));
        assert!(needs_quoting("user@host"));
        assert!(needs_quoting("(comment)"));
        assert!(!needs_quoting("simple"));
        assert!(!needs_quoting("no-special-chars"));
    }

    // ---- Simple structured tokens ----

    #[test]
    fn test_atom() {
        let tokens = tokenize("hello", true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Atom);
        assert_eq!(tokens[0].value, "hello");
    }

    #[test]
    fn test_email_address() {
        let tokens = tokenize("user@example.com", true);
        assert_eq!(tokens.len(), 5);
        assert_eq!(tokens[0].token_type, TokenType::Atom);
        assert_eq!(tokens[0].value, "user");
        assert_eq!(tokens[1].token_type, TokenType::Special);
        assert_eq!(tokens[1].value, "@");
        assert_eq!(tokens[2].token_type, TokenType::Atom);
        assert_eq!(tokens[2].value, "example");
        assert_eq!(tokens[3].token_type, TokenType::Special);
        assert_eq!(tokens[3].value, ".");
        assert_eq!(tokens[4].token_type, TokenType::Atom);
        assert_eq!(tokens[4].value, "com");
    }

    #[test]
    fn test_quoted_string() {
        let tokens = tokenize(r#""hello world""#, true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::QString);
        assert_eq!(tokens[0].value, "hello world");
    }

    #[test]
    fn test_quoted_string_with_escapes() {
        let tokens = tokenize(r#""say \"hi\"""#, true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::QString);
        assert_eq!(tokens[0].value, "say \"hi\"");
    }

    #[test]
    fn test_comment() {
        let tokens = tokenize("(this is a comment)", true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Comment);
        assert_eq!(tokens[0].value, "this is a comment");
    }

    #[test]
    fn test_nested_comment() {
        let tokens = tokenize("(outer (inner) end)", true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Comment);
        assert_eq!(tokens[0].value, "outer (inner) end");
    }

    #[test]
    fn test_comment_with_escape() {
        let tokens = tokenize(r"(escaped \) paren)", true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Comment);
        assert_eq!(tokens[0].value, "escaped ) paren");
    }

    #[test]
    fn test_domain_literal() {
        let tokens = tokenize("[127.0.0.1]", true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::DomainLiteral);
        assert_eq!(tokens[0].value, "127.0.0.1");
    }

    #[test]
    fn test_domain_literal_with_escape() {
        let tokens = tokenize(r"[escaped\]]", true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::DomainLiteral);
        assert_eq!(tokens[0].value, "escaped]");
    }

    #[test]
    fn test_specials() {
        let tokens = tokenize("<@,;:>", true);
        assert_eq!(tokens.len(), 6);
        assert!(tokens.iter().all(|t| t.token_type == TokenType::Special));
        let chars: String = tokens.iter().map(|t| t.value.as_str()).collect();
        assert_eq!(chars, "<@,;:>");
    }

    #[test]
    fn test_whitespace_normalized() {
        let tokens = tokenize("a   \t  b", true);
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].token_type, TokenType::Atom);
        assert_eq!(tokens[1].token_type, TokenType::Lwsp);
        assert_eq!(tokens[1].value, " ");
        assert_eq!(tokens[2].token_type, TokenType::Atom);
    }

    // ---- Full address header ----

    #[test]
    fn test_full_address() {
        let tokens = tokenize(r#""John Doe" <john@example.com>"#, true);
        // "John Doe" LWSP < john @ example . com >
        assert_eq!(tokens[0].token_type, TokenType::QString);
        assert_eq!(tokens[0].value, "John Doe");
        assert_eq!(tokens[1].token_type, TokenType::Lwsp);
        assert_eq!(tokens[2].token_type, TokenType::Special);
        assert_eq!(tokens[2].value, "<");
        assert_eq!(tokens[3].token_type, TokenType::Atom);
        assert_eq!(tokens[3].value, "john");
        assert_eq!(tokens[4].token_type, TokenType::Special);
        assert_eq!(tokens[4].value, "@");
    }

    // ---- Unstructured mode ----

    #[test]
    fn test_unstructured_text() {
        let tokens = tokenize("Hello, World! <greeting>", false);
        // In unstructured mode, whitespace is separate, but text runs without specials
        assert!(tokens.len() >= 1);
        // First token should be text (starts with non-whitespace)
        assert_eq!(tokens[0].token_type, TokenType::Text);
    }

    #[test]
    fn test_unstructured_preserves_specials() {
        // In unstructured mode, specials like @ and < are part of text
        let tokens = tokenize("user@host.com", false);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Text);
        assert_eq!(tokens[0].value, "user@host.com");
    }

    // ---- Folding whitespace ----

    #[test]
    fn test_fws_crlf() {
        // CRLF + space = folding, should be consumed
        let tokens = tokenize("hello\r\n world", true);
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].token_type, TokenType::Atom);
        assert_eq!(tokens[0].value, "hello");
        assert_eq!(tokens[1].token_type, TokenType::Lwsp);
        assert_eq!(tokens[2].token_type, TokenType::Atom);
        assert_eq!(tokens[2].value, "world");
    }

    #[test]
    fn test_fws_lf() {
        // LF + space = folding
        let tokens = tokenize("hello\n world", true);
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].value, "hello");
        assert_eq!(tokens[2].value, "world");
    }

    // ---- Peek ----

    #[test]
    fn test_peek() {
        let mut lex = Lexer::new("atom (comment)", true);
        assert_eq!(lex.peek_type(), Some(TokenType::Atom));
        // peek should not consume
        assert_eq!(lex.peek_type(), Some(TokenType::Atom));
        let tok = lex.next().unwrap();
        assert_eq!(tok.value, "atom");
        assert_eq!(lex.peek_type(), Some(TokenType::Lwsp));
    }

    // ---- Reset ----

    #[test]
    fn test_reset() {
        let mut lex = Lexer::new("abc", true);
        let _ = lex.next();
        assert!(lex.next().is_none());
        lex.reset();
        let tok = lex.next().unwrap();
        assert_eq!(tok.value, "abc");
    }

    // ---- Offset tracking ----

    #[test]
    fn test_offsets() {
        let input = "abc @def";
        let tokens = tokenize(input, true);
        assert_eq!(tokens[0].offset, 0); // abc
        assert_eq!(tokens[0].raw_len, 3);
        assert_eq!(tokens[1].offset, 3); // space
        assert_eq!(tokens[2].offset, 4); // @
        assert_eq!(tokens[2].raw_len, 1);
        assert_eq!(tokens[3].offset, 5); // def
    }

    // ---- Empty / edge cases ----

    #[test]
    fn test_empty_input() {
        let tokens = tokenize("", true);
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_only_whitespace() {
        let tokens = tokenize("   ", true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Lwsp);
    }

    #[test]
    fn test_unterminated_qstring() {
        // Unterminated quoted string — lexer should handle gracefully
        let tokens = tokenize("\"unterminated", true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::QString);
        assert_eq!(tokens[0].value, "unterminated");
    }

    #[test]
    fn test_unterminated_comment() {
        let tokens = tokenize("(unterminated", true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Comment);
        assert_eq!(tokens[0].value, "unterminated");
    }

    #[test]
    fn test_unterminated_domain_literal() {
        let tokens = tokenize("[192.168.1.1", true);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::DomainLiteral);
        assert_eq!(tokens[0].value, "192.168.1.1");
    }

    // ---- Quote function ----

    #[test]
    fn test_quote_simple() {
        assert_eq!(quote("hello", false), "\"hello\"");
    }

    #[test]
    fn test_quote_with_special_chars() {
        assert_eq!(quote(r#"say "hi""#, false), r#""say \"hi\"""#);
        assert_eq!(quote(r"back\slash", false), r#""back\\slash""#);
    }

    #[test]
    fn test_quote_escape_spaces() {
        assert_eq!(quote("hello world", true), r#""hello\ world""#);
    }

    // ---- Pseudo-QP ----

    #[test]
    fn test_fix_pseudo_qp_basic() {
        assert_eq!(fix_pseudo_qp("hello=20world"), "hello world");
    }

    #[test]
    fn test_fix_pseudo_qp_no_change() {
        assert_eq!(fix_pseudo_qp("nothing to decode"), "nothing to decode");
    }

    #[test]
    fn test_fix_pseudo_qp_mixed() {
        assert_eq!(fix_pseudo_qp("a=20b=3Dc=2E"), "a b=c.");
    }

    #[test]
    fn test_fix_pseudo_qp_invalid_hex() {
        // =ZZ is not valid hex, should pass through
        assert_eq!(fix_pseudo_qp("hello=ZZworld"), "hello=ZZworld");
    }

    #[test]
    fn test_fix_pseudo_qp_control_char() {
        // =01 is a control char (< 32 and not tab), should NOT be decoded
        assert_eq!(fix_pseudo_qp("hello=01world"), "hello=01world");
    }

    #[test]
    fn test_fix_pseudo_qp_tab() {
        // =09 is tab, should be decoded
        assert_eq!(fix_pseudo_qp("hello=09world"), "hello\tworld");
    }

    // ---- Base64 decode ----

    #[test]
    fn test_decode_b64_hello() {
        assert_eq!(decode_b64_inline("SGVsbG8="), b"Hello");
    }

    #[test]
    fn test_decode_b64_empty() {
        assert_eq!(decode_b64_inline(""), b"");
    }

    #[test]
    fn test_decode_b64_no_padding() {
        // "Hi" = "SGk="
        assert_eq!(decode_b64_inline("SGk="), b"Hi");
    }

    #[test]
    fn test_decode_b64_double_padding() {
        // "H" = "SA=="
        assert_eq!(decode_b64_inline("SA=="), b"H");
    }

    #[test]
    fn test_decode_b64_longer() {
        // "Hello, World!" base64 = "SGVsbG8sIFdvcmxkIQ=="
        assert_eq!(
            decode_b64_inline("SGVsbG8sIFdvcmxkIQ=="),
            b"Hello, World!"
        );
    }

    // ---- Iterator / collect ----

    #[test]
    fn test_iterator_count() {
        let count = Lexer::new("a, b, c", true).count();
        // a , LWSP b , LWSP c = 7 tokens
        assert_eq!(count, 7);
    }

    #[test]
    fn test_multiple_addresses() {
        let input = "alice@a.com, bob@b.com";
        let tokens = tokenize(input, true);
        // alice @ a . com , LWSP bob @ b . com
        let specials: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == TokenType::Special && t.value == ",")
            .collect();
        assert_eq!(specials.len(), 1);
    }

    // ---- Comment in unstructured mode ----

    #[test]
    fn test_paren_not_comment_in_unstructured() {
        // In unstructured mode, parentheses are just text
        let tokens = tokenize("Hello (world)", false);
        // Should get text tokens, NOT a comment
        assert!(tokens.iter().all(|t| t.token_type != TokenType::Comment));
    }

    // ---- Bracket not domain literal in unstructured mode ----

    #[test]
    fn test_bracket_not_domain_literal_in_unstructured() {
        let tokens = tokenize("Hello [world]", false);
        assert!(tokens
            .iter()
            .all(|t| t.token_type != TokenType::DomainLiteral));
    }
}
