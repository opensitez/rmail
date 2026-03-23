//! Simple XML parser and generator.
//!
//! Ported from macmbx_xml.c — tokenizer + recursive descent parser.
//! NOT a full standards-compliant parser: practical utility for config/filter files.

// ---------------------------------------------------------------------------
// Token types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    Done,
    Element,
    Content,
    EndTag,
    EmptyTag,
    Comment,
    Pi,
    Cdata,
}

// ---------------------------------------------------------------------------
// HTML named entity table (beyond the 5 XML entities)
// ---------------------------------------------------------------------------

fn html_entity_codepoint(name: &str) -> Option<u32> {
    match name {
        "nbsp"    => Some(0x00A0), "iexcl"   => Some(0x00A1), "cent"    => Some(0x00A2),
        "pound"   => Some(0x00A3), "yen"     => Some(0x00A5), "sect"    => Some(0x00A7),
        "copy"    => Some(0x00A9), "laquo"   => Some(0x00AB), "reg"     => Some(0x00AE),
        "deg"     => Some(0x00B0), "micro"   => Some(0x00B5), "para"    => Some(0x00B6),
        "middot"  => Some(0x00B7), "raquo"   => Some(0x00BB), "frac14"  => Some(0x00BC),
        "frac12"  => Some(0x00BD), "frac34"  => Some(0x00BE), "iquest"  => Some(0x00BF),
        "times"   => Some(0x00D7), "divide"  => Some(0x00F7), "ndash"   => Some(0x2013),
        "mdash"   => Some(0x2014), "lsquo"   => Some(0x2018), "rsquo"   => Some(0x2019),
        "ldquo"   => Some(0x201C), "rdquo"   => Some(0x201D), "bull"    => Some(0x2022),
        "hellip"  => Some(0x2026), "euro"    => Some(0x20AC), "trade"   => Some(0x2122),
        _ => None,
    }
}

/// Expand XML character/entity references.
/// Handles: &lt; &gt; &amp; &quot; &apos; &#123; &#x1F; + common HTML named entities.
pub fn expand_entities(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'&' {
            if let Some(semi_rel) = bytes[i..].iter().position(|&b| b == b';') {
                let entity_name = &input[i + 1..i + semi_rel];
                let mut resolved = false;

                // Standard XML entities
                match entity_name {
                    "lt" => { out.push('<'); resolved = true; }
                    "gt" => { out.push('>'); resolved = true; }
                    "amp" => { out.push('&'); resolved = true; }
                    "quot" => { out.push('"'); resolved = true; }
                    "apos" => { out.push('\''); resolved = true; }
                    _ => {}
                }

                // Numeric reference
                if !resolved && entity_name.starts_with('#') {
                    let num_str = &entity_name[1..];
                    let cp = if num_str.starts_with('x') || num_str.starts_with('X') {
                        u32::from_str_radix(&num_str[1..], 16).ok()
                    } else {
                        num_str.parse::<u32>().ok()
                    };
                    if let Some(cp) = cp {
                        if let Some(c) = char::from_u32(cp) {
                            out.push(c);
                            resolved = true;
                        }
                    }
                }

                // HTML named entities
                if !resolved {
                    if let Some(cp) = html_entity_codepoint(entity_name) {
                        if let Some(c) = char::from_u32(cp) {
                            out.push(c);
                            resolved = true;
                        }
                    }
                }

                if resolved {
                    i += semi_rel + 1;
                    continue;
                }
            }
            // Unknown or malformed entity, keep the &
            out.push('&');
            i += 1;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// XML-escape text for output.
pub fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// XML tokenizer state. Borrows input text.
pub struct Tokenizer<'a> {
    text: &'a [u8],
    offset: usize,
    // Last token info
    pub token_type: TokenType,
    pub token_start: usize,
    pub token_len: usize,
    pub attr_start: usize,
    pub attr_len: usize,
    attr_parse_pos: usize,
}

impl<'a> Tokenizer<'a> {
    pub fn new(text: &'a str) -> Self {
        Self::from_bytes(text.as_bytes())
    }

    pub fn from_bytes(text: &'a [u8]) -> Self {
        Tokenizer {
            text,
            offset: 0,
            token_type: TokenType::Done,
            token_start: 0,
            token_len: 0,
            attr_start: 0,
            attr_len: 0,
            attr_parse_pos: 0,
        }
    }

    fn peek(&self) -> u8 {
        if self.offset < self.text.len() { self.text[self.offset] } else { 0 }
    }

    fn advance(&mut self) -> u8 {
        if self.offset < self.text.len() {
            let b = self.text[self.offset];
            self.offset += 1;
            b
        } else { 0 }
    }

    fn skip_ws(&mut self) {
        while self.offset < self.text.len() {
            match self.text[self.offset] {
                b' ' | b'\t' | b'\r' | b'\n' => self.offset += 1,
                _ => break,
            }
        }
    }

    /// Get next token. Returns token type.
    pub fn next_token(&mut self) -> TokenType {
        if self.offset >= self.text.len() {
            self.token_type = TokenType::Done;
            return TokenType::Done;
        }

        if self.peek() == b'<' {
            let start = self.offset;
            self.advance(); // skip <

            // Comment: <!-- ... -->
            if self.offset + 2 < self.text.len()
                && self.text[self.offset] == b'!'
                && self.text[self.offset + 1] == b'-'
                && self.text[self.offset + 2] == b'-'
            {
                self.offset += 3;
                while self.offset + 2 < self.text.len() {
                    if self.text[self.offset] == b'-'
                        && self.text[self.offset + 1] == b'-'
                        && self.text[self.offset + 2] == b'>'
                    {
                        self.offset += 3;
                        break;
                    }
                    self.offset += 1;
                }
                self.token_type = TokenType::Comment;
                self.token_start = start;
                self.token_len = self.offset - start;
                return TokenType::Comment;
            }

            // CDATA: <![CDATA[ ... ]]>
            if self.offset + 7 < self.text.len()
                && &self.text[self.offset..self.offset + 8] == b"![CDATA["
            {
                self.offset += 8;
                let cdata_start = self.offset;
                while self.offset + 2 < self.text.len() {
                    if self.text[self.offset] == b']'
                        && self.text[self.offset + 1] == b']'
                        && self.text[self.offset + 2] == b'>'
                    {
                        self.token_type = TokenType::Cdata;
                        self.token_start = cdata_start;
                        self.token_len = self.offset - cdata_start;
                        self.offset += 3;
                        return TokenType::Cdata;
                    }
                    self.offset += 1;
                }
            }

            // PI: <? ... ?>
            if self.peek() == b'?' {
                self.offset += 1;
                while self.offset + 1 < self.text.len() {
                    if self.text[self.offset] == b'?' && self.text[self.offset + 1] == b'>' {
                        self.offset += 2;
                        break;
                    }
                    self.offset += 1;
                }
                self.token_type = TokenType::Pi;
                self.token_start = start;
                self.token_len = self.offset - start;
                return TokenType::Pi;
            }

            // End tag: </name>
            if self.peek() == b'/' {
                self.advance();
                let name_start = self.offset;
                while self.offset < self.text.len() && self.text[self.offset] != b'>' {
                    self.offset += 1;
                }
                self.token_type = TokenType::EndTag;
                self.token_start = name_start;
                self.token_len = self.offset - name_start;
                // Trim trailing whitespace from name
                while self.token_len > 0 {
                    let last = self.text[self.token_start + self.token_len - 1];
                    if last == b' ' || last == b'\t' { self.token_len -= 1; }
                    else { break; }
                }
                if self.offset < self.text.len() { self.offset += 1; } // skip >
                self.attr_start = 0;
                self.attr_len = 0;
                return TokenType::EndTag;
            }

            // Element tag: <name attrs> or <name attrs />
            let name_start = self.offset;
            while self.offset < self.text.len()
                && self.text[self.offset] != b'>'
                && self.text[self.offset] != b' '
                && self.text[self.offset] != b'\t'
                && self.text[self.offset] != b'\r'
                && self.text[self.offset] != b'\n'
                && self.text[self.offset] != b'/'
            {
                self.offset += 1;
            }
            self.token_start = name_start;
            self.token_len = self.offset - name_start;

            // Attributes area
            self.skip_ws();
            self.attr_start = self.offset;

            // Scan to end of tag
            while self.offset < self.text.len() && self.text[self.offset] != b'>' {
                if self.text[self.offset] == b'/'
                    && self.offset + 1 < self.text.len()
                    && self.text[self.offset + 1] == b'>'
                {
                    self.attr_len = self.offset - self.attr_start;
                    self.offset += 2; // skip />
                    self.token_type = TokenType::EmptyTag;
                    return TokenType::EmptyTag;
                }
                // Skip quoted attribute values
                if self.text[self.offset] == b'"' || self.text[self.offset] == b'\'' {
                    let q = self.text[self.offset];
                    self.offset += 1;
                    while self.offset < self.text.len() && self.text[self.offset] != q {
                        self.offset += 1;
                    }
                    if self.offset < self.text.len() { self.offset += 1; }
                } else {
                    self.offset += 1;
                }
            }
            self.attr_len = self.offset - self.attr_start;
            if self.offset < self.text.len() { self.offset += 1; } // skip >
            self.token_type = TokenType::Element;
            return TokenType::Element;
        }

        // Text content
        let start = self.offset;
        while self.offset < self.text.len() && self.text[self.offset] != b'<' {
            self.offset += 1;
        }
        self.token_type = TokenType::Content;
        self.token_start = start;
        self.token_len = self.offset - start;
        self.attr_start = 0;
        self.attr_len = 0;
        TokenType::Content
    }

    /// Get current token text with entity expansion.
    pub fn token_text(&self) -> String {
        if self.token_len == 0 { return String::new(); }
        let raw = &self.text[self.token_start..self.token_start + self.token_len];
        let raw_str = String::from_utf8_lossy(raw);
        expand_entities(&raw_str)
    }

    /// Get tag name from current element/end tag.
    pub fn tag_name(&self) -> String {
        if self.token_len == 0 { return String::new(); }
        let raw = &self.text[self.token_start..self.token_start + self.token_len];
        String::from_utf8_lossy(raw).into_owned()
    }

    /// Reset attribute parsing to beginning of current tag's attributes.
    pub fn reset_attrs(&mut self) {
        self.attr_parse_pos = self.attr_start;
    }

    /// Parse next attribute from current element tag.
    /// Returns Some((name, value)) or None if no more.
    pub fn next_attr(&mut self) -> Option<(String, String)> {
        let end = self.attr_start + self.attr_len;

        // Skip whitespace
        while self.attr_parse_pos < end {
            match self.text[self.attr_parse_pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.attr_parse_pos += 1,
                _ => break,
            }
        }
        if self.attr_parse_pos >= end { return None; }

        // Read name
        let ns = self.attr_parse_pos;
        while self.attr_parse_pos < end {
            match self.text[self.attr_parse_pos] {
                b'=' | b' ' | b'\t' | b'/' | b'>' => break,
                _ => self.attr_parse_pos += 1,
            }
        }
        let nlen = self.attr_parse_pos - ns;
        if nlen == 0 { return None; }
        let name = String::from_utf8_lossy(&self.text[ns..ns + nlen]).into_owned();

        // Skip whitespace around =
        while self.attr_parse_pos < end && (self.text[self.attr_parse_pos] == b' '
            || self.text[self.attr_parse_pos] == b'\t') {
            self.attr_parse_pos += 1;
        }

        if self.attr_parse_pos < end && self.text[self.attr_parse_pos] == b'=' {
            self.attr_parse_pos += 1;
            while self.attr_parse_pos < end && (self.text[self.attr_parse_pos] == b' '
                || self.text[self.attr_parse_pos] == b'\t') {
                self.attr_parse_pos += 1;
            }
        } else {
            // Boolean attribute (no value)
            return Some((name, String::new()));
        }

        // Read value
        let value;
        if self.attr_parse_pos < end
            && (self.text[self.attr_parse_pos] == b'"' || self.text[self.attr_parse_pos] == b'\'')
        {
            let q = self.text[self.attr_parse_pos];
            self.attr_parse_pos += 1;
            let vs = self.attr_parse_pos;
            while self.attr_parse_pos < end && self.text[self.attr_parse_pos] != q {
                self.attr_parse_pos += 1;
            }
            let raw = String::from_utf8_lossy(&self.text[vs..self.attr_parse_pos]).into_owned();
            value = expand_entities(&raw);
            if self.attr_parse_pos < end { self.attr_parse_pos += 1; } // skip closing quote
        } else {
            // Unquoted value
            let vs = self.attr_parse_pos;
            while self.attr_parse_pos < end {
                match self.text[self.attr_parse_pos] {
                    b' ' | b'\t' | b'>' | b'/' => break,
                    _ => self.attr_parse_pos += 1,
                }
            }
            let raw = String::from_utf8_lossy(&self.text[vs..self.attr_parse_pos]).into_owned();
            value = expand_entities(&raw);
        }

        Some((name, value))
    }

    /// Get a specific attribute value by name (case-insensitive). Returns None if not found.
    pub fn get_attr(&mut self, name: &str) -> Option<String> {
        self.reset_attrs();
        while let Some((n, v)) = self.next_attr() {
            if n.eq_ignore_ascii_case(name) {
                return Some(v);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// DOM-like tree parser
// ---------------------------------------------------------------------------

/// An XML node (element or text).
#[derive(Debug, Clone)]
pub struct XmlNode {
    /// Tag name (None for text nodes).
    pub name: Option<String>,
    /// Text content (None for element nodes).
    pub text: Option<String>,
    /// Attributes (name -> value).
    pub attrs: Vec<(String, String)>,
    /// Child nodes.
    pub children: Vec<XmlNode>,
}

impl XmlNode {
    fn new_element(name: &str) -> Self {
        XmlNode {
            name: Some(name.to_string()),
            text: None,
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }

    fn new_text(text: &str) -> Self {
        XmlNode {
            name: None,
            text: Some(text.to_string()),
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Find first child element by name (case-insensitive).
    pub fn find(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|c| {
            c.name.as_ref().map_or(false, |n| n.eq_ignore_ascii_case(name))
        })
    }

    /// Get text content of first non-whitespace child text node.
    pub fn text_content(&self) -> &str {
        // First non-whitespace text child
        for child in &self.children {
            if let Some(ref t) = child.text {
                if !t.trim().is_empty() {
                    return t;
                }
            }
        }
        // Fallback: first text child even if whitespace
        for child in &self.children {
            if let Some(ref t) = child.text {
                return t;
            }
        }
        if let Some(ref t) = self.text { t } else { "" }
    }

    /// Get ALL text content concatenated from node and descendants (mixed content).
    pub fn all_text(&self) -> String {
        let mut buf = String::new();
        if let Some(ref t) = self.text {
            buf.push_str(t);
        }
        for child in &self.children {
            buf.push_str(&child.all_text());
        }
        buf
    }

    /// Get attribute value by name (case-insensitive).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Get local name from a possibly-prefixed tag name.
    /// "soap:Envelope" -> "Envelope", "div" -> "div".
    pub fn local_name(&self) -> &str {
        match &self.name {
            Some(n) => xml_local_name(n),
            None => "",
        }
    }

    /// Find child element by local name and optional namespace URI.
    /// Namespace declarations are resolved from the child's own attrs first,
    /// then from this (parent) node's attrs (walks up one level).
    pub fn find_ns(&self, local_name: &str, ns_uri: Option<&str>) -> Option<&XmlNode> {
        self.children.iter().find(|child| {
            let child_name = match &child.name {
                Some(n) => n,
                None => return false,
            };
            let c_local = xml_local_name(child_name);
            if !c_local.eq_ignore_ascii_case(local_name) { return false; }
            if let Some(uri) = ns_uri {
                let pfx = xml_prefix(child_name);
                // Try child's own attrs first, then parent's attrs
                let child_uri = resolve_ns_uri(child, &pfx)
                    .or_else(|| resolve_ns_uri(self, &pfx));
                match child_uri {
                    Some(u) => u == uri,
                    None => false,
                }
            } else {
                true
            }
        })
    }
}

/// Get local name from a possibly-prefixed tag name.
pub fn xml_local_name(qname: &str) -> &str {
    match qname.find(':') {
        Some(pos) => &qname[pos + 1..],
        None => qname,
    }
}

/// Get prefix from a possibly-prefixed tag name.
pub fn xml_prefix(qname: &str) -> String {
    match qname.find(':') {
        Some(pos) => qname[..pos].to_string(),
        None => String::new(),
    }
}

/// Walk up the tree looking for namespace URI. Since we don't store parent
/// pointers in the Rust tree (ownership), this works on a single node's attrs.
fn resolve_ns_uri(node: &XmlNode, prefix: &str) -> Option<String> {
    for (name, value) in &node.attrs {
        if prefix.is_empty() {
            if name.eq_ignore_ascii_case("xmlns") {
                return Some(value.clone());
            }
        } else if name.starts_with("xmlns:") {
            if name[6..].eq_ignore_ascii_case(prefix) {
                return Some(value.clone());
            }
        }
    }
    None
}

/// Parse element from tokenizer (called after ELEMENT or EMPTY_TAG token).
fn parse_element(tok: &mut Tokenizer) -> XmlNode {
    let name = tok.tag_name();
    let mut node = XmlNode::new_element(&name);

    // Parse attributes
    tok.reset_attrs();
    while let Some((n, v)) = tok.next_attr() {
        node.attrs.push((n, v));
    }

    // If empty element, done
    if tok.token_type == TokenType::EmptyTag {
        return node;
    }

    // Parse children until matching end tag
    loop {
        let tt = tok.next_token();
        match tt {
            TokenType::Done => break,
            TokenType::EndTag => {
                let ename = tok.tag_name();
                if ename.eq_ignore_ascii_case(&name) { break; }
                // Mismatched end tag -- keep going
                continue;
            }
            TokenType::Comment | TokenType::Pi => continue,
            TokenType::Content => {
                let text = tok.token_text();
                node.children.push(XmlNode::new_text(&text));
            }
            TokenType::Cdata => {
                // CDATA: no entity expansion
                let raw = &tok.text[tok.token_start..tok.token_start + tok.token_len];
                let text = String::from_utf8_lossy(raw).into_owned();
                node.children.push(XmlNode::new_text(&text));
            }
            TokenType::Element | TokenType::EmptyTag => {
                let child = parse_element(tok);
                node.children.push(child);
            }
        }
    }

    node
}

/// Parse XML string into a tree. Returns root node with name "_root".
pub fn parse(xml: &str) -> Option<XmlNode> {
    if xml.is_empty() { return None; }
    let mut tok = Tokenizer::new(xml);
    let mut root = XmlNode::new_element("_root");

    loop {
        let tt = tok.next_token();
        match tt {
            TokenType::Done => break,
            TokenType::Comment | TokenType::Pi => continue,
            TokenType::Content => {
                let text = tok.token_text();
                root.children.push(XmlNode::new_text(&text));
            }
            TokenType::Cdata => {
                let raw = &tok.text[tok.token_start..tok.token_start + tok.token_len];
                let text = String::from_utf8_lossy(raw).into_owned();
                root.children.push(XmlNode::new_text(&text));
            }
            TokenType::Element | TokenType::EmptyTag => {
                let child = parse_element(&mut tok);
                root.children.push(child);
            }
            _ => {}
        }
    }

    Some(root)
}

// ---------------------------------------------------------------------------
// XML generation (writer)
// ---------------------------------------------------------------------------

/// XML writer for generating XML output.
pub struct XmlWriter {
    buf: String,
    indent: usize,
}

impl XmlWriter {
    pub fn new() -> Self {
        XmlWriter { buf: String::with_capacity(256), indent: 0 }
    }

    /// Get result string.
    pub fn as_str(&self) -> &str {
        &self.buf
    }

    /// Take ownership of the result string.
    pub fn take(self) -> String {
        self.buf
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.buf.push('\t');
        }
    }

    /// Write opening tag.
    pub fn open_tag(&mut self, name: &str) {
        self.write_indent();
        self.buf.push('<');
        self.buf.push_str(name);
        self.buf.push_str(">\n");
        self.indent += 1;
    }

    /// Write opening tag with attributes.
    pub fn open_tag_attrs(&mut self, name: &str, attrs: &[(&str, &str)]) {
        self.write_indent();
        self.buf.push('<');
        self.buf.push_str(name);
        for (aname, aval) in attrs {
            self.buf.push(' ');
            self.buf.push_str(aname);
            self.buf.push_str("=\"");
            self.buf.push_str(&escape_xml(aval));
            self.buf.push('"');
        }
        self.buf.push_str(">\n");
        self.indent += 1;
    }

    /// Write closing tag.
    pub fn close_tag(&mut self, name: &str) {
        if self.indent > 0 { self.indent -= 1; }
        self.write_indent();
        self.buf.push_str("</");
        self.buf.push_str(name);
        self.buf.push_str(">\n");
    }

    /// Write empty element <name />.
    pub fn empty_tag(&mut self, name: &str) {
        self.write_indent();
        self.buf.push('<');
        self.buf.push_str(name);
        self.buf.push_str(" />\n");
    }

    /// Write text content (escaped).
    pub fn write_text(&mut self, text: &str) {
        self.buf.push_str(&escape_xml(text));
    }

    /// Write <name>value</name> on one line.
    pub fn write_element(&mut self, name: &str, value: &str) {
        self.write_indent();
        self.buf.push('<');
        self.buf.push_str(name);
        self.buf.push('>');
        self.buf.push_str(&escape_xml(value));
        self.buf.push_str("</");
        self.buf.push_str(name);
        self.buf.push_str(">\n");
    }

    /// Write <name>intvalue</name>.
    pub fn write_int(&mut self, name: &str, value: i64) {
        self.write_element(name, &value.to_string());
    }

    /// Write raw string (no escaping).
    pub fn write_raw(&mut self, text: &str) {
        self.buf.push_str(text);
    }

    /// Write CRLF.
    pub fn write_crlf(&mut self) {
        self.buf.push_str("\r\n");
    }

    /// Increase indent.
    pub fn indent_inc(&mut self) { self.indent += 1; }

    /// Decrease indent.
    pub fn indent_dec(&mut self) { if self.indent > 0 { self.indent -= 1; } }
}

impl Default for XmlWriter {
    fn default() -> Self { Self::new() }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Entity expansion ---

    #[test]
    fn test_expand_xml_entities() {
        assert_eq!(expand_entities("&lt;div&gt;"), "<div>");
        assert_eq!(expand_entities("&amp;"), "&");
        assert_eq!(expand_entities("&quot;hi&quot;"), "\"hi\"");
        assert_eq!(expand_entities("&apos;"), "'");
    }

    #[test]
    fn test_expand_numeric_entities() {
        assert_eq!(expand_entities("&#65;"), "A");
        assert_eq!(expand_entities("&#x41;"), "A");
        assert_eq!(expand_entities("&#x20AC;"), "\u{20AC}"); // Euro sign
    }

    #[test]
    fn test_expand_html_entities() {
        assert_eq!(expand_entities("&nbsp;"), "\u{00A0}");
        assert_eq!(expand_entities("&euro;"), "\u{20AC}");
        assert_eq!(expand_entities("&mdash;"), "\u{2014}");
    }

    #[test]
    fn test_expand_unknown_entity() {
        // Unknown entity: keep the &
        assert_eq!(expand_entities("&bogus;"), "&bogus;");
    }

    #[test]
    fn test_expand_bare_ampersand() {
        assert_eq!(expand_entities("a & b"), "a & b");
    }

    // --- Escape ---

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("<div>"), "&lt;div&gt;");
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("\"hi\""), "&quot;hi&quot;");
        assert_eq!(escape_xml("plain"), "plain");
    }

    // --- Tokenizer ---

    #[test]
    fn test_tokenize_simple() {
        let xml = "<root>Hello</root>";
        let mut tok = Tokenizer::new(xml);
        assert_eq!(tok.next_token(), TokenType::Element);
        assert_eq!(tok.tag_name(), "root");
        assert_eq!(tok.next_token(), TokenType::Content);
        assert_eq!(tok.token_text(), "Hello");
        assert_eq!(tok.next_token(), TokenType::EndTag);
        assert_eq!(tok.tag_name(), "root");
        assert_eq!(tok.next_token(), TokenType::Done);
    }

    #[test]
    fn test_tokenize_empty_tag() {
        let xml = "<br />";
        let mut tok = Tokenizer::new(xml);
        assert_eq!(tok.next_token(), TokenType::EmptyTag);
        assert_eq!(tok.tag_name(), "br");
        assert_eq!(tok.next_token(), TokenType::Done);
    }

    #[test]
    fn test_tokenize_attributes() {
        let xml = r#"<div class="main" id="top">"#;
        let mut tok = Tokenizer::new(xml);
        assert_eq!(tok.next_token(), TokenType::Element);
        assert_eq!(tok.tag_name(), "div");

        tok.reset_attrs();
        let (n1, v1) = tok.next_attr().unwrap();
        assert_eq!(n1, "class");
        assert_eq!(v1, "main");
        let (n2, v2) = tok.next_attr().unwrap();
        assert_eq!(n2, "id");
        assert_eq!(v2, "top");
        assert!(tok.next_attr().is_none());
    }

    #[test]
    fn test_tokenize_get_attr() {
        let xml = r#"<img src="pic.jpg" alt="A &amp; B" />"#;
        let mut tok = Tokenizer::new(xml);
        assert_eq!(tok.next_token(), TokenType::EmptyTag);
        assert_eq!(tok.get_attr("src"), Some("pic.jpg".to_string()));
        assert_eq!(tok.get_attr("alt"), Some("A & B".to_string()));
        assert_eq!(tok.get_attr("missing"), None);
    }

    #[test]
    fn test_tokenize_comment() {
        let xml = "<!-- comment --><root/>";
        let mut tok = Tokenizer::new(xml);
        assert_eq!(tok.next_token(), TokenType::Comment);
        assert_eq!(tok.next_token(), TokenType::EmptyTag);
        assert_eq!(tok.tag_name(), "root");
    }

    #[test]
    fn test_tokenize_cdata() {
        let xml = "<root><![CDATA[Hello <world>]]></root>";
        let mut tok = Tokenizer::new(xml);
        assert_eq!(tok.next_token(), TokenType::Element);
        assert_eq!(tok.next_token(), TokenType::Cdata);
        let raw = &tok.text[tok.token_start..tok.token_start + tok.token_len];
        assert_eq!(std::str::from_utf8(raw).unwrap(), "Hello <world>");
        assert_eq!(tok.next_token(), TokenType::EndTag);
    }

    #[test]
    fn test_tokenize_pi() {
        let xml = "<?xml version=\"1.0\"?><root/>";
        let mut tok = Tokenizer::new(xml);
        assert_eq!(tok.next_token(), TokenType::Pi);
        assert_eq!(tok.next_token(), TokenType::EmptyTag);
    }

    #[test]
    fn test_tokenize_entity_in_content() {
        let xml = "<p>a &amp; b</p>";
        let mut tok = Tokenizer::new(xml);
        tok.next_token(); // <p>
        assert_eq!(tok.next_token(), TokenType::Content);
        assert_eq!(tok.token_text(), "a & b");
    }

    // --- DOM tree parser ---

    #[test]
    fn test_parse_simple() {
        let root = parse("<root><child>Hello</child></root>").unwrap();
        assert_eq!(root.name.as_deref(), Some("_root"));
        let root_elem = root.find("root").unwrap();
        let child = root_elem.find("child").unwrap();
        assert_eq!(child.text_content(), "Hello");
    }

    #[test]
    fn test_parse_attributes() {
        let root = parse(r#"<item id="42" name="test">val</item>"#).unwrap();
        let item = root.find("item").unwrap();
        assert_eq!(item.attr("id"), Some("42"));
        assert_eq!(item.attr("name"), Some("test"));
        assert_eq!(item.text_content(), "val");
    }

    #[test]
    fn test_parse_empty_element() {
        let root = parse("<root><br/></root>").unwrap();
        let root_elem = root.find("root").unwrap();
        let br = root_elem.find("br").unwrap();
        assert!(br.children.is_empty());
    }

    #[test]
    fn test_parse_nested() {
        let xml = "<a><b><c>deep</c></b></a>";
        let root = parse(xml).unwrap();
        let a = root.find("a").unwrap();
        let b = a.find("b").unwrap();
        let c = b.find("c").unwrap();
        assert_eq!(c.text_content(), "deep");
    }

    #[test]
    fn test_parse_mixed_content() {
        let xml = "<p>Hello <b>world</b> foo</p>";
        let root = parse(xml).unwrap();
        let p = root.find("p").unwrap();
        assert_eq!(p.all_text(), "Hello world foo");
    }

    #[test]
    fn test_parse_cdata() {
        let xml = "<data><![CDATA[<not>xml</not>]]></data>";
        let root = parse(xml).unwrap();
        let data = root.find("data").unwrap();
        assert_eq!(data.text_content(), "<not>xml</not>");
    }

    #[test]
    fn test_parse_entities_in_attr() {
        let xml = r#"<link url="a&amp;b" />"#;
        let root = parse(xml).unwrap();
        let link = root.find("link").unwrap();
        assert_eq!(link.attr("url"), Some("a&b"));
    }

    #[test]
    fn test_parse_skip_comments_and_pi() {
        let xml = "<?xml version=\"1.0\"?><!-- comment --><root>ok</root>";
        let root = parse(xml).unwrap();
        let root_elem = root.find("root").unwrap();
        assert_eq!(root_elem.text_content(), "ok");
    }

    // --- Namespace support ---

    #[test]
    fn test_local_name() {
        assert_eq!(xml_local_name("soap:Envelope"), "Envelope");
        assert_eq!(xml_local_name("div"), "div");
    }

    #[test]
    fn test_prefix() {
        assert_eq!(xml_prefix("soap:Envelope"), "soap");
        assert_eq!(xml_prefix("div"), "");
    }

    #[test]
    fn test_find_ns() {
        let xml = r#"<root xmlns:ns="http://example.com"><ns:item>hi</ns:item></root>"#;
        let root = parse(xml).unwrap();
        let root_elem = root.find("root").unwrap();
        let item = root_elem.find_ns("item", Some("http://example.com"));
        assert!(item.is_some());
        assert_eq!(item.unwrap().text_content(), "hi");
    }

    // --- XML writer ---

    #[test]
    fn test_writer_simple() {
        let mut w = XmlWriter::new();
        w.open_tag("root");
        w.write_element("name", "test");
        w.close_tag("root");
        let result = w.take();
        assert!(result.contains("<root>"));
        assert!(result.contains("<name>test</name>"));
        assert!(result.contains("</root>"));
    }

    #[test]
    fn test_writer_attrs() {
        let mut w = XmlWriter::new();
        w.open_tag_attrs("div", &[("class", "main"), ("id", "top")]);
        w.close_tag("div");
        let result = w.take();
        assert!(result.contains(r#"class="main""#));
        assert!(result.contains(r#"id="top""#));
    }

    #[test]
    fn test_writer_empty_tag() {
        let mut w = XmlWriter::new();
        w.empty_tag("br");
        assert_eq!(w.as_str(), "<br />\n");
    }

    #[test]
    fn test_writer_escaping() {
        let mut w = XmlWriter::new();
        w.write_element("msg", "a < b & c > d");
        let result = w.take();
        assert!(result.contains("a &lt; b &amp; c &gt; d"));
    }

    #[test]
    fn test_writer_int() {
        let mut w = XmlWriter::new();
        w.write_int("count", 42);
        assert!(w.as_str().contains("<count>42</count>"));
    }

    #[test]
    fn test_writer_raw() {
        let mut w = XmlWriter::new();
        w.write_raw("<raw>text</raw>");
        assert_eq!(w.as_str(), "<raw>text</raw>");
    }

    #[test]
    fn test_writer_crlf() {
        let mut w = XmlWriter::new();
        w.write_crlf();
        assert_eq!(w.as_str(), "\r\n");
    }

    #[test]
    fn test_writer_indent() {
        let mut w = XmlWriter::new();
        w.open_tag("a");
        w.open_tag("b");
        w.write_element("c", "val");
        w.close_tag("b");
        w.close_tag("a");
        let result = w.take();
        // Check indentation: b should be indented once, c twice
        assert!(result.contains("\t<b>"));
        assert!(result.contains("\t\t<c>val</c>"));
    }

    #[test]
    fn test_writer_indent_inc_dec() {
        let mut w = XmlWriter::new();
        w.indent_inc();
        w.write_element("x", "y");
        w.indent_dec();
        w.write_element("z", "w");
        let result = w.take();
        assert!(result.starts_with("\t<x>y</x>"));
        assert!(result.contains("<z>w</z>"));
    }

    // --- Roundtrip: parse then write ---

    #[test]
    fn test_roundtrip_simple() {
        let xml = "<config><setting>value</setting></config>";
        let root = parse(xml).unwrap();
        let config = root.find("config").unwrap();
        let setting = config.find("setting").unwrap();
        assert_eq!(setting.text_content(), "value");
    }

    #[test]
    fn test_parse_multiple_children() {
        let xml = "<list><item>a</item><item>b</item><item>c</item></list>";
        let root = parse(xml).unwrap();
        let list = root.find("list").unwrap();
        let items: Vec<&str> = list.children.iter()
            .filter(|c| c.name.as_deref() == Some("item"))
            .map(|c| c.text_content())
            .collect();
        assert_eq!(items, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_case_insensitive_find() {
        let xml = "<Root><Child>ok</Child></Root>";
        let root = parse(xml).unwrap();
        let r = root.find("root").unwrap();
        let c = r.find("child").unwrap();
        assert_eq!(c.text_content(), "ok");
    }

    #[test]
    fn test_boolean_attribute() {
        let xml = "<input disabled readonly />";
        let mut tok = Tokenizer::new(xml);
        assert_eq!(tok.next_token(), TokenType::EmptyTag);
        tok.reset_attrs();
        let (n1, v1) = tok.next_attr().unwrap();
        assert_eq!(n1, "disabled");
        assert_eq!(v1, "");
        let (n2, v2) = tok.next_attr().unwrap();
        assert_eq!(n2, "readonly");
        assert_eq!(v2, "");
    }

    #[test]
    fn test_writer_attr_escaping() {
        let mut w = XmlWriter::new();
        w.open_tag_attrs("a", &[("href", "x&y<z")]);
        w.close_tag("a");
        let result = w.take();
        assert!(result.contains(r#"href="x&amp;y&lt;z""#));
    }
}
