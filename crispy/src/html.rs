//! HTML tokenizer and parser.
//!
//! Port of `crispy_html.c` — full HTML tokenizer for email messages:
//! - Token-based parser (tags, text, entities, comments)
//! - Entity decoding (named + numeric)
//! - Attribute parsing
//! - Color parsing (named colors + #hex)
//! - Charset detection from `<meta>`
//! - Tag enumeration
//!
//! Standalone: no external dependencies.

// ================================================================
// Tag enumeration (common HTML tags in email)
// ================================================================

/// Known HTML tag types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tag {
    Unknown,
    Html,
    Head,
    Body,
    Title,
    P,
    Br,
    Hr,
    Div,
    Span,
    A,
    Img,
    B,
    I,
    U,
    Strong,
    Em,
    Font,
    Big,
    Small,
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Ul,
    Ol,
    Li,
    Dl,
    Dt,
    Dd,
    Table,
    Tr,
    Td,
    Th,
    Caption,
    Pre,
    Code,
    Blockquote,
    Style,
    Script,
    Meta,
    Link,
    Sup,
    Sub,
    S,
    Strike,
    Center,
    Form,
    Input,
    Textarea,
    Select,
    Option,
    Iframe,
    Object,
    Embed,
    Thead,
    Tbody,
    Tfoot,
    Col,
    Colgroup,
}

impl Tag {
    /// Parse a tag name (case-insensitive) into a `Tag` enum variant.
    pub fn from_name(name: &str) -> Tag {
        match name.to_ascii_lowercase().as_str() {
            "html" => Tag::Html,
            "head" => Tag::Head,
            "body" => Tag::Body,
            "title" => Tag::Title,
            "p" => Tag::P,
            "br" => Tag::Br,
            "hr" => Tag::Hr,
            "div" => Tag::Div,
            "span" => Tag::Span,
            "a" => Tag::A,
            "img" => Tag::Img,
            "b" => Tag::B,
            "i" => Tag::I,
            "u" => Tag::U,
            "strong" => Tag::Strong,
            "em" => Tag::Em,
            "font" => Tag::Font,
            "big" => Tag::Big,
            "small" => Tag::Small,
            "h1" => Tag::H1,
            "h2" => Tag::H2,
            "h3" => Tag::H3,
            "h4" => Tag::H4,
            "h5" => Tag::H5,
            "h6" => Tag::H6,
            "ul" => Tag::Ul,
            "ol" => Tag::Ol,
            "li" => Tag::Li,
            "dl" => Tag::Dl,
            "dt" => Tag::Dt,
            "dd" => Tag::Dd,
            "table" => Tag::Table,
            "tr" => Tag::Tr,
            "td" => Tag::Td,
            "th" => Tag::Th,
            "caption" => Tag::Caption,
            "pre" => Tag::Pre,
            "code" => Tag::Code,
            "blockquote" => Tag::Blockquote,
            "style" => Tag::Style,
            "script" => Tag::Script,
            "meta" => Tag::Meta,
            "link" => Tag::Link,
            "sup" => Tag::Sup,
            "sub" => Tag::Sub,
            "s" => Tag::S,
            "strike" => Tag::Strike,
            "center" => Tag::Center,
            "form" => Tag::Form,
            "input" => Tag::Input,
            "textarea" => Tag::Textarea,
            "select" => Tag::Select,
            "option" => Tag::Option,
            "iframe" => Tag::Iframe,
            "object" => Tag::Object,
            "embed" => Tag::Embed,
            "thead" => Tag::Thead,
            "tbody" => Tag::Tbody,
            "tfoot" => Tag::Tfoot,
            "col" => Tag::Col,
            "colgroup" => Tag::Colgroup,
            _ => Tag::Unknown,
        }
    }

    /// Returns `true` if this tag is a block-level element.
    pub fn is_block(self) -> bool {
        matches!(
            self,
            Tag::P
                | Tag::Div
                | Tag::Blockquote
                | Tag::H1
                | Tag::H2
                | Tag::H3
                | Tag::H4
                | Tag::H5
                | Tag::H6
                | Tag::Ul
                | Tag::Ol
                | Tag::Li
                | Tag::Table
                | Tag::Tr
                | Tag::Td
                | Tag::Th
                | Tag::Pre
                | Tag::Hr
                | Tag::Dl
                | Tag::Dt
                | Tag::Dd
                | Tag::Center
                | Tag::Form
        )
    }

    /// Returns `true` if this tag is a void (self-closing) element.
    pub fn is_void(self) -> bool {
        matches!(
            self,
            Tag::Br
                | Tag::Hr
                | Tag::Img
                | Tag::Input
                | Tag::Meta
                | Tag::Link
                | Tag::Col
                | Tag::Embed
        )
    }
}

// ================================================================
// Token types
// ================================================================

/// An HTML token produced by the tokenizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token<'a> {
    /// Plain text between tags.
    Text(&'a str),
    /// Opening tag: `<tag ...>`.
    StartTag {
        tag: Tag,
        tag_name: String,
        raw: &'a str,
    },
    /// Closing tag: `</tag>`.
    EndTag {
        tag: Tag,
        tag_name: String,
        raw: &'a str,
    },
    /// Self-closing tag: `<tag .../>` or void element like `<br>`.
    SelfClosingTag {
        tag: Tag,
        tag_name: String,
        raw: &'a str,
    },
    /// HTML comment: `<!-- ... -->`.
    Comment(&'a str),
    /// DOCTYPE declaration: `<!DOCTYPE ...>`.
    DocType(&'a str),
    /// HTML entity: `&entity;` or `&#123;`.
    Entity(&'a str),
}

// ================================================================
// Tokenizer
// ================================================================

/// An iterator-based HTML tokenizer.
///
/// Borrows the source HTML and yields [`Token`] values.
pub struct Tokenizer<'a> {
    html: &'a str,
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    /// Create a new tokenizer over the given HTML string.
    pub fn new(html: &'a str) -> Self {
        Tokenizer { html, pos: 0 }
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let bytes = self.html.as_bytes();
        if self.pos >= bytes.len() {
            return None;
        }

        let start = self.pos;

        if bytes[self.pos] == b'<' {
            self.pos += 1;

            // Comment: <!--
            if self.pos + 2 < bytes.len()
                && bytes[self.pos] == b'!'
                && bytes[self.pos + 1] == b'-'
                && bytes[self.pos + 2] == b'-'
            {
                self.pos += 3;
                while self.pos + 2 < bytes.len()
                    && !(bytes[self.pos] == b'-'
                        && bytes[self.pos + 1] == b'-'
                        && bytes[self.pos + 2] == b'>')
                {
                    self.pos += 1;
                }
                if self.pos + 2 < bytes.len() {
                    self.pos += 3;
                }
                return Some(Token::Comment(&self.html[start..self.pos]));
            }

            // DOCTYPE: <!
            if self.pos < bytes.len() && bytes[self.pos] == b'!' {
                while self.pos < bytes.len() && bytes[self.pos] != b'>' {
                    self.pos += 1;
                }
                if self.pos < bytes.len() {
                    self.pos += 1;
                }
                return Some(Token::DocType(&self.html[start..self.pos]));
            }

            // Close tag?
            let is_close = if self.pos < bytes.len() && bytes[self.pos] == b'/' {
                self.pos += 1;
                true
            } else {
                false
            };

            // Skip whitespace before tag name
            while self.pos < bytes.len() && bytes[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }

            // Tag name
            let name_start = self.pos;
            while self.pos < bytes.len()
                && bytes[self.pos] != b'>'
                && bytes[self.pos] != b' '
                && bytes[self.pos] != b'/'
                && bytes[self.pos] != b'\t'
                && bytes[self.pos] != b'\n'
                && bytes[self.pos] != b'\r'
            {
                self.pos += 1;
            }
            let tag_name = self.html[name_start..self.pos].to_ascii_lowercase();
            let tag = Tag::from_name(&tag_name);

            // Skip to end of tag, handling quoted attributes
            let mut self_close = false;
            while self.pos < bytes.len() && bytes[self.pos] != b'>' {
                match bytes[self.pos] {
                    b'"' => {
                        self.pos += 1;
                        while self.pos < bytes.len() && bytes[self.pos] != b'"' {
                            self.pos += 1;
                        }
                        if self.pos < bytes.len() {
                            self.pos += 1;
                        }
                    }
                    b'\'' => {
                        self.pos += 1;
                        while self.pos < bytes.len() && bytes[self.pos] != b'\'' {
                            self.pos += 1;
                        }
                        if self.pos < bytes.len() {
                            self.pos += 1;
                        }
                    }
                    b'/' => {
                        self_close = true;
                        self.pos += 1;
                    }
                    _ => {
                        self.pos += 1;
                    }
                }
            }
            if self.pos < bytes.len() {
                self.pos += 1; // skip '>'
            }

            let raw = &self.html[start..self.pos];

            if is_close {
                Some(Token::EndTag {
                    tag,
                    tag_name,
                    raw,
                })
            } else if self_close || tag.is_void() {
                Some(Token::SelfClosingTag {
                    tag,
                    tag_name,
                    raw,
                })
            } else {
                Some(Token::StartTag {
                    tag,
                    tag_name,
                    raw,
                })
            }
        } else if bytes[self.pos] == b'&' {
            // Entity
            let ent_start = self.pos;
            self.pos += 1;
            while self.pos < bytes.len()
                && bytes[self.pos] != b';'
                && bytes[self.pos] != b'<'
                && bytes[self.pos] != b' '
                && (self.pos - ent_start) < 12
            {
                self.pos += 1;
            }
            if self.pos < bytes.len() && bytes[self.pos] == b';' {
                self.pos += 1;
            }
            Some(Token::Entity(&self.html[ent_start..self.pos]))
        } else {
            // Plain text
            while self.pos < bytes.len() && bytes[self.pos] != b'<' && bytes[self.pos] != b'&' {
                self.pos += 1;
            }
            Some(Token::Text(&self.html[start..self.pos]))
        }
    }
}

// ================================================================
// Entity decoding
// ================================================================

/// Named HTML entity table.
static NAMED_ENTITIES: &[(&str, char)] = &[
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("nbsp", '\u{A0}'),
    ("copy", '\u{A9}'),
    ("reg", '\u{AE}'),
    ("trade", '\u{2122}'),
    ("ndash", '\u{2013}'),
    ("mdash", '\u{2014}'),
    ("lsquo", '\u{2018}'),
    ("rsquo", '\u{2019}'),
    ("ldquo", '\u{201C}'),
    ("rdquo", '\u{201D}'),
    ("bull", '\u{2022}'),
    ("hellip", '\u{2026}'),
    ("euro", '\u{20AC}'),
    ("pound", '\u{A3}'),
    ("yen", '\u{A5}'),
    ("cent", '\u{A2}'),
    ("laquo", '\u{AB}'),
    ("raquo", '\u{BB}'),
    ("deg", '\u{B0}'),
    ("micro", '\u{B5}'),
    ("para", '\u{B6}'),
    ("middot", '\u{B7}'),
    ("frac14", '\u{BC}'),
    ("frac12", '\u{BD}'),
    ("frac34", '\u{BE}'),
    ("times", '\u{D7}'),
    ("divide", '\u{F7}'),
];

/// Decode a single HTML entity string (e.g. `"&amp;"`, `"&#65;"`, `"&#x41;"`).
///
/// Returns the decoded character, or `None` if the entity is unrecognized.
pub fn decode_entity(entity: &str) -> Option<char> {
    let s = entity.strip_prefix('&')?;
    let s = s.strip_suffix(';').unwrap_or(s);

    if let Some(rest) = s.strip_prefix('#') {
        // Numeric entity
        let codepoint = if let Some(hex) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            rest.parse::<u32>().ok()?
        };
        if codepoint == 0 {
            return None;
        }
        char::from_u32(codepoint)
    } else {
        // Named entity (case-insensitive)
        let lower = s.to_ascii_lowercase();
        NAMED_ENTITIES
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&lower))
            .map(|(_, ch)| *ch)
    }
}

/// Decode all HTML entities in a string, returning a new `String`.
///
/// Unrecognized entities are passed through verbatim.
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'&' {
            // Find the extent of the entity
            let ent_start = i;
            i += 1;
            while i < bytes.len()
                && bytes[i] != b';'
                && bytes[i] != b'<'
                && bytes[i] != b' '
                && (i - ent_start) < 12
            {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b';' {
                i += 1;
            }
            let entity_str = &text[ent_start..i];
            if let Some(ch) = decode_entity(entity_str) {
                out.push(ch);
            } else {
                out.push_str(entity_str);
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }

    out
}

// ================================================================
// Attribute parsing
// ================================================================

/// Extract the value of an attribute from raw tag text.
///
/// Performs case-insensitive matching on the attribute name.
/// Returns `None` if the attribute is not found.
///
/// # Example
/// ```
/// use crispy::html::get_attr;
/// let tag = r#"<a href="http://example.com" title="Example">"#;
/// assert_eq!(get_attr(tag, "href"), Some("http://example.com".to_string()));
/// assert_eq!(get_attr(tag, "TITLE"), Some("Example".to_string()));
/// assert_eq!(get_attr(tag, "class"), None);
/// ```
pub fn get_attr(tag_text: &str, attr_name: &str) -> Option<String> {
    let bytes = tag_text.as_bytes();
    let mut p = 0;

    // Skip past '<' and optional '/'
    if p < bytes.len() && bytes[p] == b'<' {
        p += 1;
    }
    if p < bytes.len() && bytes[p] == b'/' {
        p += 1;
    }

    // Skip tag name
    while p < bytes.len() && !bytes[p].is_ascii_whitespace() && bytes[p] != b'>' {
        p += 1;
    }

    loop {
        // Skip whitespace
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        if p >= bytes.len() || bytes[p] == b'>' || bytes[p] == b'/' {
            break;
        }

        // Attribute name
        let astart = p;
        while p < bytes.len()
            && bytes[p] != b'='
            && !bytes[p].is_ascii_whitespace()
            && bytes[p] != b'>'
        {
            p += 1;
        }
        let aname = &tag_text[astart..p];

        // Skip whitespace
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        if p >= bytes.len() || bytes[p] != b'=' {
            // Attribute without value — skip it
            continue;
        }
        p += 1; // skip '='

        // Skip whitespace
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }

        // Value
        let (vstart, vend);
        if p < bytes.len() && (bytes[p] == b'"' || bytes[p] == b'\'') {
            let q = bytes[p];
            p += 1;
            vstart = p;
            while p < bytes.len() && bytes[p] != q {
                p += 1;
            }
            vend = p;
            if p < bytes.len() {
                p += 1;
            }
        } else {
            vstart = p;
            while p < bytes.len() && !bytes[p].is_ascii_whitespace() && bytes[p] != b'>' {
                p += 1;
            }
            vend = p;
        }

        if aname.eq_ignore_ascii_case(attr_name) {
            return Some(tag_text[vstart..vend].to_string());
        }
    }

    None
}

// ================================================================
// Color parsing
// ================================================================

/// An RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Named HTML color table.
static NAMED_COLORS: &[(&str, Color)] = &[
    ("black", Color { r: 0, g: 0, b: 0 }),
    (
        "white",
        Color {
            r: 255,
            g: 255,
            b: 255,
        },
    ),
    ("red", Color { r: 255, g: 0, b: 0 }),
    ("green", Color { r: 0, g: 128, b: 0 }),
    ("blue", Color { r: 0, g: 0, b: 255 }),
    (
        "yellow",
        Color {
            r: 255,
            g: 255,
            b: 0,
        },
    ),
    (
        "cyan",
        Color {
            r: 0,
            g: 255,
            b: 255,
        },
    ),
    (
        "magenta",
        Color {
            r: 255,
            g: 0,
            b: 255,
        },
    ),
    (
        "gray",
        Color {
            r: 128,
            g: 128,
            b: 128,
        },
    ),
    (
        "grey",
        Color {
            r: 128,
            g: 128,
            b: 128,
        },
    ),
    (
        "silver",
        Color {
            r: 192,
            g: 192,
            b: 192,
        },
    ),
    ("maroon", Color { r: 128, g: 0, b: 0 }),
    (
        "olive",
        Color {
            r: 128,
            g: 128,
            b: 0,
        },
    ),
    ("lime", Color { r: 0, g: 255, b: 0 }),
    (
        "aqua",
        Color {
            r: 0,
            g: 255,
            b: 255,
        },
    ),
    (
        "teal",
        Color {
            r: 0,
            g: 128,
            b: 128,
        },
    ),
    ("navy", Color { r: 0, g: 0, b: 128 }),
    (
        "fuchsia",
        Color {
            r: 255,
            g: 0,
            b: 255,
        },
    ),
    (
        "purple",
        Color {
            r: 128,
            g: 0,
            b: 128,
        },
    ),
    (
        "orange",
        Color {
            r: 255,
            g: 165,
            b: 0,
        },
    ),
];

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse an HTML color string.
///
/// Supports:
/// - `#rrggbb` (6-digit hex)
/// - `#rgb` (3-digit shorthand hex)
/// - Named colors (case-insensitive): black, white, red, green, blue, etc.
///
/// # Example
/// ```
/// use crispy::html::{parse_color, Color};
/// assert_eq!(parse_color("#ff8800"), Some(Color { r: 255, g: 136, b: 0 }));
/// assert_eq!(parse_color("#f80"), Some(Color { r: 255, g: 136, b: 0 }));
/// assert_eq!(parse_color("red"), Some(Color { r: 255, g: 0, b: 0 }));
/// ```
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();

    if let Some(hex) = s.strip_prefix('#') {
        let hex = hex.as_bytes();
        if hex.len() == 6 {
            Some(Color {
                r: hex_digit(hex[0])? * 16 + hex_digit(hex[1])?,
                g: hex_digit(hex[2])? * 16 + hex_digit(hex[3])?,
                b: hex_digit(hex[4])? * 16 + hex_digit(hex[5])?,
            })
        } else if hex.len() == 3 {
            Some(Color {
                r: hex_digit(hex[0])? * 17,
                g: hex_digit(hex[1])? * 17,
                b: hex_digit(hex[2])? * 17,
            })
        } else {
            None
        }
    } else {
        NAMED_COLORS
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(s))
            .map(|(_, c)| *c)
    }
}

// ================================================================
// Charset detection
// ================================================================

/// Scan HTML for a charset declaration in `<meta>` tags.
///
/// Looks for:
/// - `<meta charset="utf-8">`
/// - `<meta http-equiv="Content-Type" content="text/html; charset=utf-8">`
///
/// Stops scanning at `</head>` or `<body>`.
/// Returns `None` if no charset is found.
pub fn detect_charset(html: &str) -> Option<String> {
    let tokenizer = Tokenizer::new(html);

    for token in tokenizer {
        match &token {
            Token::StartTag { tag, raw, .. } | Token::SelfClosingTag { tag, raw, .. } => {
                if *tag == Tag::Meta {
                    // <meta charset="utf-8">
                    if let Some(cs) = get_attr(raw, "charset") {
                        return Some(cs);
                    }

                    // <meta http-equiv="Content-Type" content="text/html; charset=utf-8">
                    if let Some(he) = get_attr(raw, "http-equiv") {
                        if he.eq_ignore_ascii_case("Content-Type") {
                            if let Some(content) = get_attr(raw, "content") {
                                let lower = content.to_ascii_lowercase();
                                if let Some(idx) = lower.find("charset=") {
                                    let rest = &content[idx + 8..];
                                    let end = rest
                                        .find(|c: char| c == ';' || c == ' ' || c == '"')
                                        .unwrap_or(rest.len());
                                    return Some(rest[..end].to_string());
                                }
                            }
                        }
                    }
                }

                // Stop after <body>
                if *tag == Tag::Body {
                    break;
                }
            }
            Token::EndTag { tag, .. } => {
                // Stop after </head>
                if *tag == Tag::Head {
                    break;
                }
            }
            _ => {}
        }
    }

    None
}

// ================================================================
// Tests
// ================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Tag classification ---

    #[test]
    fn tag_from_name_known() {
        assert_eq!(Tag::from_name("div"), Tag::Div);
        assert_eq!(Tag::from_name("DIV"), Tag::Div);
        assert_eq!(Tag::from_name("Div"), Tag::Div);
        assert_eq!(Tag::from_name("blockquote"), Tag::Blockquote);
    }

    #[test]
    fn tag_from_name_unknown() {
        assert_eq!(Tag::from_name("foobar"), Tag::Unknown);
        assert_eq!(Tag::from_name(""), Tag::Unknown);
    }

    #[test]
    fn tag_is_block() {
        assert!(Tag::P.is_block());
        assert!(Tag::Div.is_block());
        assert!(Tag::Table.is_block());
        assert!(Tag::Hr.is_block());
        assert!(!Tag::Span.is_block());
        assert!(!Tag::A.is_block());
        assert!(!Tag::B.is_block());
    }

    #[test]
    fn tag_is_void() {
        assert!(Tag::Br.is_void());
        assert!(Tag::Hr.is_void());
        assert!(Tag::Img.is_void());
        assert!(Tag::Meta.is_void());
        assert!(!Tag::Div.is_void());
        assert!(!Tag::P.is_void());
    }

    // --- Tokenizer ---

    #[test]
    fn tokenize_simple_html() {
        let html = "<p>Hello</p>";
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        assert_eq!(tokens.len(), 3);
        assert!(matches!(&tokens[0], Token::StartTag { tag: Tag::P, .. }));
        assert!(matches!(&tokens[1], Token::Text("Hello")));
        assert!(matches!(&tokens[2], Token::EndTag { tag: Tag::P, .. }));
    }

    #[test]
    fn tokenize_self_closing() {
        let html = "<br/>";
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(
            &tokens[0],
            Token::SelfClosingTag { tag: Tag::Br, .. }
        ));
    }

    #[test]
    fn tokenize_void_element_without_slash() {
        let html = "<br>";
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(
            &tokens[0],
            Token::SelfClosingTag { tag: Tag::Br, .. }
        ));
    }

    #[test]
    fn tokenize_comment() {
        let html = "<!-- a comment -->";
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::Comment("<!-- a comment -->")));
    }

    #[test]
    fn tokenize_doctype() {
        let html = "<!DOCTYPE html>";
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::DocType("<!DOCTYPE html>")));
    }

    #[test]
    fn tokenize_entity() {
        let html = "&amp;";
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::Entity("&amp;")));
    }

    #[test]
    fn tokenize_mixed() {
        let html = "<div class=\"x\">Hello &amp; world</div>";
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        assert_eq!(tokens.len(), 5);
        assert!(matches!(
            &tokens[0],
            Token::StartTag { tag: Tag::Div, .. }
        ));
        assert!(matches!(&tokens[1], Token::Text("Hello ")));
        assert!(matches!(&tokens[2], Token::Entity("&amp;")));
        assert!(matches!(&tokens[3], Token::Text(" world")));
        assert!(matches!(
            &tokens[4],
            Token::EndTag { tag: Tag::Div, .. }
        ));
    }

    #[test]
    fn tokenize_attributes_with_quotes() {
        let html = r#"<a href="http://example.com" title='test'>link</a>"#;
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        assert_eq!(tokens.len(), 3);
        assert!(matches!(&tokens[0], Token::StartTag { tag: Tag::A, .. }));
        assert!(matches!(&tokens[1], Token::Text("link")));
        assert!(matches!(&tokens[2], Token::EndTag { tag: Tag::A, .. }));
    }

    #[test]
    fn tokenize_empty_input() {
        let tokens: Vec<_> = Tokenizer::new("").collect();
        assert!(tokens.is_empty());
    }

    #[test]
    fn tokenize_text_only() {
        let tokens: Vec<_> = Tokenizer::new("just text").collect();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::Text("just text")));
    }

    #[test]
    fn tokenize_img_is_void() {
        let html = r#"<img src="pic.jpg">"#;
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(
            &tokens[0],
            Token::SelfClosingTag { tag: Tag::Img, .. }
        ));
    }

    // --- Entity decoding ---

    #[test]
    fn decode_named_entities() {
        assert_eq!(decode_entity("&amp;"), Some('&'));
        assert_eq!(decode_entity("&lt;"), Some('<'));
        assert_eq!(decode_entity("&gt;"), Some('>'));
        assert_eq!(decode_entity("&quot;"), Some('"'));
        assert_eq!(decode_entity("&nbsp;"), Some('\u{A0}'));
        assert_eq!(decode_entity("&euro;"), Some('\u{20AC}'));
    }

    #[test]
    fn decode_named_case_insensitive() {
        assert_eq!(decode_entity("&AMP;"), Some('&'));
        assert_eq!(decode_entity("&Amp;"), Some('&'));
    }

    #[test]
    fn decode_numeric_decimal() {
        assert_eq!(decode_entity("&#65;"), Some('A'));
        assert_eq!(decode_entity("&#97;"), Some('a'));
        assert_eq!(decode_entity("&#8364;"), Some('\u{20AC}')); // euro sign
    }

    #[test]
    fn decode_numeric_hex() {
        assert_eq!(decode_entity("&#x41;"), Some('A'));
        assert_eq!(decode_entity("&#X41;"), Some('A'));
        assert_eq!(decode_entity("&#x20AC;"), Some('\u{20AC}'));
    }

    #[test]
    fn decode_unknown_entity() {
        assert_eq!(decode_entity("&bogus;"), None);
    }

    #[test]
    fn decode_zero_codepoint() {
        assert_eq!(decode_entity("&#0;"), None);
    }

    #[test]
    fn decode_entities_string() {
        assert_eq!(decode_entities("Hello &amp; world"), "Hello & world");
        assert_eq!(
            decode_entities("&lt;div&gt;"),
            "<div>"
        );
        assert_eq!(
            decode_entities("no entities here"),
            "no entities here"
        );
        assert_eq!(decode_entities("&#65;&#66;&#67;"), "ABC");
    }

    #[test]
    fn decode_entities_passthrough_unknown() {
        assert_eq!(decode_entities("&bogus;text"), "&bogus;text");
    }

    // --- Attribute parsing ---

    #[test]
    fn get_attr_basic() {
        let tag = r#"<a href="http://example.com">"#;
        assert_eq!(
            get_attr(tag, "href"),
            Some("http://example.com".to_string())
        );
    }

    #[test]
    fn get_attr_single_quotes() {
        let tag = "<div class='main'>";
        assert_eq!(get_attr(tag, "class"), Some("main".to_string()));
    }

    #[test]
    fn get_attr_unquoted() {
        let tag = "<div class=main>";
        assert_eq!(get_attr(tag, "class"), Some("main".to_string()));
    }

    #[test]
    fn get_attr_case_insensitive() {
        let tag = r#"<meta HTTP-EQUIV="Content-Type">"#;
        assert_eq!(
            get_attr(tag, "http-equiv"),
            Some("Content-Type".to_string())
        );
    }

    #[test]
    fn get_attr_not_found() {
        let tag = "<div class=\"main\">";
        assert_eq!(get_attr(tag, "id"), None);
    }

    #[test]
    fn get_attr_multiple() {
        let tag = r#"<a href="url" title="tip" class="link">"#;
        assert_eq!(get_attr(tag, "href"), Some("url".to_string()));
        assert_eq!(get_attr(tag, "title"), Some("tip".to_string()));
        assert_eq!(get_attr(tag, "class"), Some("link".to_string()));
    }

    // --- Color parsing ---

    #[test]
    fn parse_color_hex6() {
        assert_eq!(
            parse_color("#ff8800"),
            Some(Color {
                r: 255,
                g: 136,
                b: 0
            })
        );
        assert_eq!(
            parse_color("#000000"),
            Some(Color { r: 0, g: 0, b: 0 })
        );
        assert_eq!(
            parse_color("#FFFFFF"),
            Some(Color {
                r: 255,
                g: 255,
                b: 255
            })
        );
    }

    #[test]
    fn parse_color_hex3() {
        assert_eq!(
            parse_color("#f80"),
            Some(Color {
                r: 255,
                g: 136,
                b: 0
            })
        );
        assert_eq!(
            parse_color("#000"),
            Some(Color { r: 0, g: 0, b: 0 })
        );
        assert_eq!(
            parse_color("#fff"),
            Some(Color {
                r: 255,
                g: 255,
                b: 255
            })
        );
    }

    #[test]
    fn parse_color_named() {
        assert_eq!(
            parse_color("red"),
            Some(Color { r: 255, g: 0, b: 0 })
        );
        assert_eq!(
            parse_color("RED"),
            Some(Color { r: 255, g: 0, b: 0 })
        );
        assert_eq!(
            parse_color("Navy"),
            Some(Color { r: 0, g: 0, b: 128 })
        );
    }

    #[test]
    fn parse_color_with_whitespace() {
        assert_eq!(
            parse_color("  #ff0000  "),
            Some(Color { r: 255, g: 0, b: 0 })
        );
    }

    #[test]
    fn parse_color_invalid() {
        assert_eq!(parse_color("#gg0000"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("notacolor"), None);
    }

    // --- Charset detection ---

    #[test]
    fn detect_charset_meta_charset() {
        let html = r#"<html><head><meta charset="utf-8"></head><body></body></html>"#;
        assert_eq!(detect_charset(html), Some("utf-8".to_string()));
    }

    #[test]
    fn detect_charset_meta_http_equiv() {
        let html = r#"<html><head><meta http-equiv="Content-Type" content="text/html; charset=iso-8859-1"></head><body></body></html>"#;
        assert_eq!(detect_charset(html), Some("iso-8859-1".to_string()));
    }

    #[test]
    fn detect_charset_none() {
        let html = "<html><head><title>Test</title></head><body>Hello</body></html>";
        assert_eq!(detect_charset(html), None);
    }

    #[test]
    fn detect_charset_stops_at_body() {
        let html = r#"<html><body><meta charset="utf-8"></body></html>"#;
        assert_eq!(detect_charset(html), None);
    }

    // --- Integration / round-trip ---

    #[test]
    fn tokenize_and_decode_full_document() {
        let html = r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Test</title></head><body><p>Hello &amp; <b>world</b>!</p></body></html>"#;

        let tokens: Vec<_> = Tokenizer::new(html).collect();

        // Should have many tokens but not panic
        assert!(tokens.len() > 10);

        // Collect decoded text
        let mut text = String::new();
        for token in &tokens {
            match token {
                Token::Text(t) => text.push_str(t),
                Token::Entity(e) => {
                    if let Some(ch) = decode_entity(e) {
                        text.push(ch);
                    }
                }
                _ => {}
            }
        }
        assert_eq!(text, "TestHello & world!");

        // Check charset
        assert_eq!(detect_charset(html), Some("utf-8".to_string()));
    }

    #[test]
    fn tokenize_and_decode_entities_in_text() {
        let html = "5 &gt; 3 &amp;&amp; 2 &lt; 4";
        let decoded = decode_entities(html);
        assert_eq!(decoded, "5 > 3 && 2 < 4");
    }

    #[test]
    fn tokenize_nested_tags() {
        let html = "<div><ul><li>one</li><li>two</li></ul></div>";
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        // div(open) ul(open) li(open) "one" li(close) li(open) "two" li(close) ul(close) div(close)
        assert_eq!(tokens.len(), 10);
    }

    #[test]
    fn tag_name_preserved() {
        let html = "<DIV class=\"x\">text</DIV>";
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        match &tokens[0] {
            Token::StartTag { tag_name, tag, .. } => {
                assert_eq!(tag_name, "div");
                assert_eq!(*tag, Tag::Div);
            }
            _ => panic!("expected StartTag"),
        }
    }

    #[test]
    fn entity_without_semicolon() {
        // The tokenizer should handle entities that terminate without ';'
        // (e.g. followed by space or '<')
        let html = "&amp text";
        let tokens: Vec<_> = Tokenizer::new(html).collect();
        // "&amp" as entity, " text" as text — or similar split
        assert!(tokens.len() >= 1);
    }

    #[test]
    fn color_all_named() {
        // Verify all named colors parse
        let names = [
            "black", "white", "red", "green", "blue", "yellow", "cyan", "magenta", "gray",
            "grey", "silver", "maroon", "olive", "lime", "aqua", "teal", "navy", "fuchsia",
            "purple", "orange",
        ];
        for name in &names {
            assert!(
                parse_color(name).is_some(),
                "failed to parse color: {}",
                name
            );
        }
    }

    #[test]
    fn get_attr_from_self_closing() {
        let tag = r#"<img src="photo.jpg" alt="A photo" />"#;
        assert_eq!(get_attr(tag, "src"), Some("photo.jpg".to_string()));
        assert_eq!(get_attr(tag, "alt"), Some("A photo".to_string()));
    }
}
