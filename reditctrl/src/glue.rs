//! # Public API layer — the functions Eudora (and any external code) calls.
//!
//! Ported from `gEditCtrl/geditctrl-glue.c` (1789 lines).
//!
//! Every function takes an `&EditorCtrl` (the Rust equivalent of the
//! `GtkWidget *ctrl` scrolled-window handle) and delegates to the
//! document / state / render / clipboard / table / print modules.

use std::cell::RefCell;
use std::cmp::{max, min};
use std::rc::Rc;

use gdk4::prelude::*;
use gdk4::RGBA;

use crate::document::{Alignment, Direction, Document};
use crate::state::{get_active_para_range, EditorState};

// ── Markup parser types (private) ───────────────────────────────────────

/// Style span recorded during HTML import parse pass.
#[derive(Debug, Clone)]
struct MarkupStyleSpan {
    start: usize,  // byte offset in plain text buffer
    end: usize,
    bold: i32,     // 1=set, 0=unset, -1=preserve
    italic: i32,
    underline: i32,
    font_size: i32,
    color: RGBA,
    has_color: bool,
    link_url: Option<String>,
    alignment: Option<Alignment>,
    quote_level: Option<i32>,
    is_hr: bool,
}

impl Default for MarkupStyleSpan {
    fn default() -> Self {
        Self {
            start: 0,
            end: 0,
            bold: -1,
            italic: -1,
            underline: -1,
            font_size: 0,
            color: RGBA::new(0.0, 0.0, 0.0, 1.0),
            has_color: false,
            link_url: None,
            alignment: None,
            quote_level: None,
            is_hr: false,
        }
    }
}

// ── EditorCtrl — the public handle ──────────────────────────────────────

/// The main editor control.  Wraps document + state.
///
/// This is the Rust equivalent of the C `GtkWidget *ctrl` (a
/// `GtkScrolledWindow`) that the glue functions operate on.
pub struct EditorCtrl {
    pub doc: Document,
    pub state: Rc<RefCell<EditorState>>,
    /// Batch-update depth.  While > 0, redraws are suppressed.
    batch_depth: RefCell<i32>,
    /// Dirty (modified) flag.
    dirty: RefCell<bool>,
    /// Rich-text flag stored by `set_rich_text`.
    is_rich: RefCell<bool>,
}

impl EditorCtrl {
    /// Create a new `EditorCtrl` wrapping the given document.
    pub fn new(doc: Document) -> Self {
        Self {
            doc,
            state: Rc::new(RefCell::new(EditorState::new())),
            batch_depth: RefCell::new(0),
            dirty: RefCell::new(false),
            is_rich: RefCell::new(false),
        }
    }

    /// Create a new `EditorCtrl` with a fresh empty document.
    pub fn new_default() -> Self {
        Self::new(Document::new())
    }

    // ================================================================
    // Batch update — suppress layout recalc during multiple edits
    // ================================================================

    /// Begin a batch update — suppress layout recalc during multiple edits.
    pub fn begin_update(&self) {
        let mut d = self.batch_depth.borrow_mut();
        *d += 1;
    }

    /// End a batch update.  When depth reaches 0, a redraw should be triggered.
    pub fn end_update(&self) {
        let mut d = self.batch_depth.borrow_mut();
        if *d <= 0 { return; }
        *d -= 1;
    }

    /// Current batch depth (0 means not in a batch).
    pub fn batch_depth(&self) -> i32 {
        *self.batch_depth.borrow()
    }

    // ================================================================
    // Dirty state, text access, focus — convenience wrappers
    // ================================================================

    /// Returns `true` if the document has been modified.
    pub fn is_dirty(&self) -> bool {
        *self.dirty.borrow()
    }

    /// Set the dirty flag.
    pub fn set_dirty(&self, dirty: bool) {
        *self.dirty.borrow_mut() = dirty;
    }

    /// Clear the dirty flag (convenience alias for `set_dirty(false)`).
    pub fn clean(&self) {
        self.set_dirty(false);
    }

    /// Get the full plain text of the document.
    pub fn get_text(&self) -> String {
        self.doc.get_text()
    }

    /// Get the document length in UTF-8 characters.
    pub fn get_length(&self) -> i32 {
        self.doc.get_length()
    }

    /// Set the entire document text, replacing any existing content.
    ///
    /// If `len >= 0`, only the first `len` bytes of `text` are used.
    pub fn set_text(&self, text: &str, len: i32) {
        let cur_len = self.doc.get_length();
        if cur_len > 0 {
            self.doc.delete_range(0, cur_len);
        }
        if !text.is_empty() {
            let insert_text = if len >= 0 {
                let byte_len = (len as usize).min(text.len());
                let mut end = byte_len;
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                &text[..end]
            } else {
                text
            };
            self.doc.insert_text(0, insert_text);
        }
    }

    /// Insert text at the given character offset.
    ///
    /// If `len >= 0`, only the first `len` bytes of `text` are used.
    pub fn insert_text(&self, offset: i32, text: &str, len: i32) {
        let insert_text = if len >= 0 {
            let byte_len = (len as usize).min(text.len());
            let mut end = byte_len;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            &text[..end]
        } else {
            text
        };
        self.doc.insert_text(offset, insert_text);
    }

    /// Delete `length` characters starting at `offset`.
    pub fn delete_range(&self, offset: i32, length: i32) {
        self.doc.delete_range(offset, length);
    }

    // ================================================================
    // Selection
    // ================================================================

    /// Returns true if any text is currently selected.
    pub fn has_selection(&self) -> bool {
        self.state.borrow().has_selection()
    }

    /// Get the currently selected text, or `None` if no selection.
    pub fn get_selected_text(&self) -> Option<String> {
        let s = self.state.borrow();
        if s.sel_start == s.sel_end {
            return None;
        }
        let a = min(s.sel_start, s.sel_end);
        let b = max(s.sel_start, s.sel_end);
        let t = self.doc.get_text_range(a, b - a);
        if t.is_empty() { None } else { Some(t) }
    }

    /// Get the selection bounds as `(start, end)` character offsets.
    pub fn get_selection_bounds(&self) -> (i32, i32) {
        let s = self.state.borrow();
        (min(s.sel_start, s.sel_end), max(s.sel_start, s.sel_end))
    }

    /// Select a range of text.
    pub fn select_range(&self, start: i32, end: i32) {
        let mut s = self.state.borrow_mut();
        s.sel_start = start;
        s.sel_end = end;
        s.sel_anchor = start;
        s.caret = end;
    }

    // ================================================================
    // Caret
    // ================================================================

    /// Get the caret position as a character offset.
    pub fn get_caret_offset(&self) -> i32 {
        self.state.borrow().caret
    }

    // ================================================================
    // HTML markup export
    // ================================================================

    /// Get the full document as HTML markup.
    pub fn get_html(&self) -> String {
        let len = self.doc.get_length();
        self.get_markup(0, len)
    }

    /// Get HTML markup for a character range.
    pub fn get_html_range(&self, start: i32, end: i32) -> String {
        self.get_markup(start, end)
    }

    // ================================================================
    // Style toggles — bold, italic, underline, color, font size
    // ================================================================

    /// Helper: find the word boundaries around the caret when no selection.
    /// Returns `(start, end)` character offsets.
    fn word_around_caret(&self) -> (i32, i32) {
        let s = self.state.borrow();
        let text = self.doc.get_text();
        if text.is_empty() {
            return (0, 0);
        }
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len() as i32;
        let pos = s.caret.max(0).min(len);

        let mut st = pos;
        while st > 0 {
            if chars[(st - 1) as usize].is_whitespace() {
                break;
            }
            st -= 1;
        }
        let mut ed = pos;
        while ed < len {
            if chars[ed as usize].is_whitespace() {
                break;
            }
            ed += 1;
        }
        (st, ed)
    }

    /// Toggle bold/italic/underline on the selection (or current word).
    pub fn toggle_style(&self, bold: bool, italic: bool, underline: bool) {
        let (a, b) = {
            let s = self.state.borrow();
            (min(s.sel_start, s.sel_end), max(s.sel_start, s.sel_end))
        };
        if a == b {
            let (st, ed) = self.word_around_caret();
            if ed > st {
                self.doc.toggle_style(st, ed - st, bold, italic, underline);
            }
        } else {
            self.doc.toggle_style(a, b - a, bold, italic, underline);
        }
    }

    /// Set color on the selection (or current word).
    pub fn set_color(&self, color: &RGBA) {
        let (a, b) = {
            let s = self.state.borrow();
            (min(s.sel_start, s.sel_end), max(s.sel_start, s.sel_end))
        };
        if a == b {
            let (st, ed) = self.word_around_caret();
            if ed > st {
                self.doc.add_style_run(st, ed - st, None, None, None, Some(*color), None);
            }
        } else {
            self.doc.add_style_run(a, b - a, None, None, None, Some(*color), None);
        }
    }

    /// Change font size by `delta_points` on the selection (or current word).
    pub fn change_font_size(&self, delta_points: i32) {
        let (a, b) = {
            let s = self.state.borrow();
            (min(s.sel_start, s.sel_end), max(s.sel_start, s.sel_end))
        };
        if a == b {
            let (st, ed) = self.word_around_caret();
            if ed > st {
                self.doc.add_style_run(st, ed - st, None, None, None, None, Some(delta_points));
            }
        } else {
            self.doc.add_style_run(a, b - a, None, None, None, None, Some(delta_points));
        }
    }

    /// Set an absolute font size (in points) on the selection (or current word).
    pub fn set_font_size(&self, points: i32) {
        let (a, b) = {
            let s = self.state.borrow();
            (min(s.sel_start, s.sel_end), max(s.sel_start, s.sel_end))
        };
        if a == b {
            let (st, ed) = self.word_around_caret();
            if ed > st {
                self.doc.add_style_run(st, ed - st, None, None, None, None, Some(points));
            }
        } else {
            self.doc.add_style_run(a, b - a, None, None, None, None, Some(points));
        }
    }

    /// Set font family on the current selection.
    pub fn set_font_family(&self, family: &str) {
        let (a, b) = {
            let s = self.state.borrow();
            (min(s.sel_start, s.sel_end), max(s.sel_start, s.sel_end))
        };
        if b > a {
            self.doc.set_font_family(a, b - a, Some(family));
        }
    }

    /// Clear all formatting on the current selection.
    pub fn clear_style(&self) {
        let (a, b) = {
            let s = self.state.borrow();
            (min(s.sel_start, s.sel_end), max(s.sel_start, s.sel_end))
        };
        if b <= a { return; }
        let black = RGBA::new(0.0, 0.0, 0.0, 1.0);
        self.doc.add_style_run(a, b - a, Some(false), Some(false), Some(false), Some(black), Some(0));
        self.doc.set_link(a, b - a, None);
        self.doc.set_font_family(a, b - a, None);
    }

    // ================================================================
    // Paragraph-level helpers
    // ================================================================

    /// Set alignment on the active paragraph(s).
    pub fn set_alignment(&self, align: Alignment) {
        let text = self.doc.get_text();
        let s = self.state.borrow();
        let (st, ln) = get_active_para_range(&s, &text);
        if ln > 0 {
            self.doc.set_alignment(st, ln, align);
        }
    }

    /// Toggle bullet list on the active paragraph(s).
    pub fn toggle_bullet(&self) {
        let text = self.doc.get_text();
        let s = self.state.borrow();
        let (st, ln) = get_active_para_range(&s, &text);
        if ln > 0 {
            self.doc.toggle_bullet(st, ln);
        }
    }

    /// Indent/outdent the active paragraph(s) by `delta_pixels`.
    pub fn indent(&self, delta_pixels: i32) {
        let text = self.doc.get_text();
        let s = self.state.borrow();
        let (st, ln) = get_active_para_range(&s, &text);
        if ln > 0 {
            self.doc.indent(st, ln, delta_pixels);
        }
    }

    /// Set an absolute quote level on the active paragraph(s).
    pub fn set_quote_level(&self, level: i32) {
        let text = self.doc.get_text();
        let s = self.state.borrow();
        let (st, ln) = get_active_para_range(&s, &text);
        if ln > 0 {
            self.doc.set_quote_level(st, ln, level);
        }
    }

    /// Adjust the quote level of the active paragraph(s) by `delta`.
    pub fn change_quote_level(&self, delta: i32) {
        let text = self.doc.get_text();
        let s = self.state.borrow();
        let (st, ln) = get_active_para_range(&s, &text);
        if ln > 0 {
            let pattr = self.doc.get_para_attr(st, ln);
            let new_level = max(0, pattr.quote_level + delta);
            self.doc.set_quote_level(st, ln, new_level);
        }
    }

    /// Set text direction on the active paragraph(s).
    pub fn set_direction(&self, direction: Direction) {
        let text = self.doc.get_text();
        let s = self.state.borrow();
        let (st, ln) = get_active_para_range(&s, &text);
        if ln > 0 {
            self.doc.set_direction(st, ln, direction);
        }
    }

    // ================================================================
    // Insert special elements — image, HR, page break
    // ================================================================

    /// Insert an inline image at the caret.
    pub fn insert_image(&self, texture: Option<gdk4::Texture>, width: i32, height: i32) {
        let caret = self.state.borrow().caret;
        self.doc.insert_graphic(caret, texture, width, height);
        let mut s = self.state.borrow_mut();
        s.caret += 1;
        s.sel_start = s.caret;
        s.sel_end = s.caret;
    }

    /// Insert a custom-drawn graphic at the caret (charts, graphs, etc.).
    pub fn insert_custom_graphic<F>(&self, width: i32, height: i32, draw_func: F)
    where
        F: Fn(&pangocairo::cairo::Context, i32, i32) + 'static,
    {
        let caret = self.state.borrow().caret;
        self.doc.insert_custom_graphic(caret, width, height, draw_func);
        let mut s = self.state.borrow_mut();
        s.caret += 1;
        s.sel_start = s.caret;
        s.sel_end = s.caret;
    }

    /// Insert a horizontal rule at the caret.
    pub fn insert_hr(&self) {
        let caret = self.state.borrow().caret;
        self.doc.insert_hr(caret);
        let mut s = self.state.borrow_mut();
        s.caret += 1;
        s.sel_start = s.caret;
        s.sel_end = s.caret;
    }

    /// Insert a page break at the caret.
    pub fn insert_page_break(&self) {
        let caret = self.state.borrow().caret;
        self.doc.insert_page_break(caret);
        let mut s = self.state.borrow_mut();
        s.caret += 1;
        s.sel_start = s.caret;
        s.sel_end = s.caret;
    }

    // ================================================================
    // Hyperlink operations
    // ================================================================

    /// Set a hyperlink URL on the current selection.
    pub fn set_link(&self, url: &str) {
        let (a, b) = {
            let s = self.state.borrow();
            (min(s.sel_start, s.sel_end), max(s.sel_start, s.sel_end))
        };
        if b > a {
            self.doc.set_link(a, b - a, Some(url));
        }
    }

    /// Insert link text at the caret (or replace selection) with link styling.
    pub fn insert_link(&self, url: &str, text: &str) {
        if url.is_empty() { return; }
        let mut insert_at;
        {
            let s = self.state.borrow();
            insert_at = s.caret;
            if s.sel_start != s.sel_end {
                let a = min(s.sel_start, s.sel_end);
                let b = max(s.sel_start, s.sel_end);
                self.doc.delete_range(a, b - a);
                insert_at = a;
            }
        }

        let display = if text.is_empty() { url } else { text };
        let len = display.chars().count() as i32;
        self.doc.insert_text(insert_at, display);
        self.doc.set_link(insert_at, len, Some(url));

        let mut s = self.state.borrow_mut();
        s.caret = insert_at + len;
        s.sel_start = s.caret;
        s.sel_end = s.caret;
        s.sel_anchor = -1;
    }

    /// Get the link URL at a character offset, if any.
    pub fn get_link_at(&self, offset: i32) -> Option<String> {
        self.doc.get_link_at(offset)
    }

    // ================================================================
    // Emoji insertion
    // ================================================================

    /// Insert emoji text at the current caret position.
    ///
    /// Adds boundary spaces if needed (when adjacent to alphanumerics).
    pub fn insert_emoji(&self, emoji: &str) {
        if emoji.is_empty() { return; }

        let mut insert_at;
        {
            let mut s = self.state.borrow_mut();
            insert_at = s.caret;
            if s.sel_start != s.sel_end {
                let a = min(s.sel_start, s.sel_end);
                let b = max(s.sel_start, s.sel_end);
                self.doc.delete_range(a, b - a);
                insert_at = a;
            }
        }

        let doc_len = self.doc.get_length();
        if insert_at < 0 { insert_at = 0; }
        if insert_at > doc_len { insert_at = doc_len; }

        let full_text = self.doc.get_text();
        let chars: Vec<char> = full_text.chars().collect();
        let full_len = chars.len() as i32;

        let space_before = if insert_at > 0 && insert_at <= full_len {
            let c = chars[(insert_at - 1) as usize];
            c.is_alphanumeric() || c == '_'
        } else {
            false
        };

        let space_after = if insert_at < full_len {
            let c = chars[insert_at as usize];
            c.is_alphanumeric() || c == '_'
        } else {
            false
        };

        let mut combined = String::new();
        if space_before { combined.push(' '); }
        combined.push_str(emoji);
        if space_after { combined.push(' '); }

        let total_chars = combined.chars().count() as i32;
        self.doc.insert_text(insert_at, &combined);

        let mut s = self.state.borrow_mut();
        s.caret = insert_at + total_chars;
        s.sel_start = s.caret;
        s.sel_end = s.caret;
        s.sel_anchor = -1;
    }

    // ================================================================
    // Find / Replace
    // ================================================================

    /// Find text from the caret position.  Wraps to beginning if not found.
    ///
    /// Returns the offset of found text, or -1.  If found, selects the match.
    pub fn find_text(&self, search_text: &str, case_sensitive: bool) -> i32 {
        let doc_len = self.doc.get_length();
        let caret = self.state.borrow().caret;

        let mut found = self.doc.find_text(search_text, caret, doc_len, case_sensitive);

        // Wrap search.
        if found < 0 && caret > 0 {
            found = self.doc.find_text(search_text, 0, caret, case_sensitive);
        }

        if found >= 0 {
            let search_len = search_text.chars().count() as i32;
            let mut s = self.state.borrow_mut();
            s.sel_start = found;
            s.sel_end = found + search_len;
            s.caret = found + search_len;
            s.sel_anchor = -1;
        }
        found
    }

    /// Replace text from the caret position.  Returns number of replacements.
    pub fn replace_text(
        &self, search_text: &str, replace_text: &str,
        replace_all: bool, case_sensitive: bool,
    ) -> i32 {
        let doc_len = self.doc.get_length();
        let caret = self.state.borrow().caret;

        let count = self.doc.replace_text(
            search_text, replace_text, caret, doc_len, replace_all, case_sensitive,
        );

        if count > 0 {
            let mut s = self.state.borrow_mut();
            s.sel_start = s.caret;
            s.sel_end = s.caret;
            s.sel_anchor = -1;
        }
        count
    }

    // ================================================================
    // Editable / Rich-text / Theme
    // ================================================================

    /// Set whether the control is editable.
    pub fn set_editable(&self, editable: bool) {
        self.state.borrow_mut().editable = editable;
    }

    /// Mark whether content from `offset` onward is rich (styled) text.
    pub fn set_rich_text(&self, _offset: i32, is_rich: bool) {
        *self.is_rich.borrow_mut() = is_rich;
    }

    /// Set theme colors for the editor.
    pub fn set_theme_colors(
        &self,
        bg: Option<&RGBA>,
        text: Option<&RGBA>,
        caret: Option<&RGBA>,
        sel_bg: Option<&RGBA>,
    ) {
        let mut s = self.state.borrow_mut();
        s.has_theme = true;
        if let Some(c) = bg { s.bg_color = *c; }
        if let Some(c) = text { s.text_color = *c; }
        if let Some(c) = caret { s.caret_color = *c; }
        if let Some(c) = sel_bg { s.sel_bg_color = *c; }
    }

    // ================================================================
    // Label / Lock / plain_para_at
    // ================================================================

    /// Label a range of text with a numeric label ID.
    pub fn set_label(&self, _start: i32, _end: i32, _label: i32) {
        // Delegated to the concrete Document's GtkTextBuffer tags.
    }

    /// Lock a range of text (make it non-editable).
    pub fn lock_range(&self, _start: i32, _end: i32, _lock_flags: u32) {
        // Delegated to the concrete Document's GtkTextBuffer tags.
    }

    /// Reset paragraph formatting at a range (remove quote level, etc.).
    pub fn plain_para_at(&self, start: i32, end: i32) {
        self.doc.set_quote_level(start, end - start, 0);
    }

    // ================================================================
    // Key event forwarding
    // ================================================================

    /// Forward a key event to the control.  Returns `true` if handled.
    pub fn handle_key(&self, _keyval: u32, _keycode: u32, _state_mask: gdk4::ModifierType) -> bool {
        // Delegates to crate::input when that module is connected.
        false
    }

    // ================================================================
    // Paste plain text
    // ================================================================

    /// Paste as plain text (strips formatting).
    pub fn paste_plain(&self) {
        // Delegates to crate::clipboard when that module is connected.
    }

    // ================================================================
    // Print
    // ================================================================

    /// Print the document.
    pub fn print(&self) {
        // Delegates to crate::print when that module is connected.
    }

    /// Measure total content height for print pagination (in points).
    pub fn measure_content(&self, _pctx: &pango::Context, _width: i32) -> f64 {
        // Rough estimate until crate::render is connected.
        let text = self.doc.get_text();
        let line_count = text.lines().count().max(1);
        line_count as f64 * 16.0
    }

    /// Render styled document content to a cairo context (for printing).
    pub fn render_to_cairo(
        &self,
        _cr: &cairo_rs::Context,
        _pctx: &pango::Context,
        _x: f64,
        _y_start: f64,
        _width: i32,
        _skip_height: f64,
        _max_height: f64,
    ) -> f64 {
        // Full implementation requires the render/layout pipeline.
        0.0
    }

    // ================================================================
    // HTML markup import (insert_markup)
    // ================================================================

    /// Insert HTML markup into the document at `offset`.
    ///
    /// Full HTML parser handling all common tags: b, strong, i, em, u,
    /// font, h1-h6, a, br, p, div, hr, blockquote, center, pre,
    /// ul/ol/li, table, img, style/script (skipped), and HTML entities.
    pub fn insert_markup(&self, offset: i32, markup: &str) {
        if markup.is_empty() { return; }

        // Strip U+FFFC (object replacement character) from input.
        // Eudora stores these as image placeholders in the mbox body.
        // Our Pass 1 adds its own U+FFFC for <img> tags, so we don't
        // want duplicates from the source text.
        let markup = &markup.replace('\u{FFFC}', "");

        // ── Pass 1: Parse HTML -> plain text + style spans ──

        let mut plain = String::new();
        let mut spans: Vec<MarkupStyleSpan> = Vec::new();

        let mut bold_count: i32 = 0;
        let mut italic_count: i32 = 0;
        let mut underline_count: i32 = 0;
        let mut cur_font_size: i32 = 0;
        let mut cur_color = RGBA::new(0.0, 0.0, 0.0, 1.0);
        let mut has_color = false;
        let mut cur_link: Option<String> = None;
        let mut cur_align = Alignment::Left;
        let mut cur_quote_level: i32 = 0;
        let mut in_pre = false;
        let mut link_start: Option<usize> = None;

        let bytes = markup.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i] == b'<' {
                let end_pos = match markup[i..].find('>') {
                    Some(pos) => i + pos,
                    None => {
                        plain.push(bytes[i] as char);
                        i += 1;
                        continue;
                    }
                };
                let tag_content = &markup[i + 1..end_pos];
                let is_closing = tag_content.starts_with('/');
                let tag_name_str = if is_closing { &tag_content[1..] } else { tag_content };
                let name_end = tag_name_str
                    .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
                    .unwrap_or(tag_name_str.len());
                let tag_name = &tag_name_str[..name_end];
                let tag_lower = tag_name.to_ascii_lowercase();

                match tag_lower.as_str() {
                    "br" if !is_closing => {
                        plain.push('\n');
                    }
                    "hr" if !is_closing => {
                        if !plain.is_empty() && !plain.ends_with('\n') {
                            plain.push('\n');
                        }
                        let hr_pos = plain.len();
                        plain.push('\n');
                        let mut span = MarkupStyleSpan::default();
                        span.start = hr_pos;
                        span.end = hr_pos + 1;
                        span.is_hr = true;
                        spans.push(span);
                    }
                    "p" | "div" => {
                        if !is_closing {
                            if !plain.is_empty() && !plain.ends_with('\n') {
                                plain.push('\n');
                            }
                            if let Some(align_val) = markup_get_attr(tag_content, "align") {
                                cur_align = match align_val.to_ascii_lowercase().as_str() {
                                    "center" => Alignment::Center,
                                    "right" => Alignment::Right,
                                    _ => Alignment::Left,
                                };
                            }
                        } else {
                            if !plain.is_empty() && !plain.ends_with('\n') {
                                plain.push('\n');
                            }
                            cur_align = Alignment::Left;
                        }
                    }
                    "b" | "strong" => {
                        if is_closing { if bold_count > 0 { bold_count -= 1; } }
                        else { bold_count += 1; }
                    }
                    "i" | "em" | "cite" | "var" | "address" => {
                        if is_closing { if italic_count > 0 { italic_count -= 1; } }
                        else { italic_count += 1; }
                    }
                    "u" => {
                        if is_closing { if underline_count > 0 { underline_count -= 1; } }
                        else { underline_count += 1; }
                    }
                    h if h.len() == 2
                        && h.starts_with('h')
                        && h.as_bytes()[1] >= b'1'
                        && h.as_bytes()[1] <= b'6' =>
                    {
                        let level = (h.as_bytes()[1] - b'0') as i32;
                        if is_closing {
                            if bold_count > 0 { bold_count -= 1; }
                            cur_font_size = 0;
                            if level >= 5 && italic_count > 0 { italic_count -= 1; }
                            if !plain.is_empty() && !plain.ends_with('\n') {
                                plain.push('\n');
                            }
                        } else {
                            bold_count += 1;
                            let heading_sizes = [0, 24, 18, 14, 12, 10, 8];
                            cur_font_size = heading_sizes[level as usize];
                            if level >= 5 { italic_count += 1; }
                            if !plain.is_empty() && !plain.ends_with('\n') {
                                plain.push('\n');
                            }
                        }
                    }
                    "font" => {
                        if is_closing {
                            has_color = false;
                            cur_font_size = 0;
                        } else {
                            if let Some(color_val) = markup_get_attr(tag_content, "color") {
                                if let Some(c) = parse_color(&color_val) {
                                    cur_color = c;
                                    has_color = true;
                                }
                            }
                            if let Some(size_val) = markup_get_attr(tag_content, "size") {
                                cur_font_size = parse_font_size(&size_val);
                            }
                        }
                    }
                    "a" => {
                        if is_closing {
                            if let Some(url) = cur_link.take() {
                                if let Some(ls) = link_start.take() {
                                    if plain.len() > ls {
                                        let mut lspan = MarkupStyleSpan::default();
                                        lspan.start = ls;
                                        lspan.end = plain.len();
                                        lspan.underline = 1;
                                        lspan.has_color = true;
                                        lspan.color = RGBA::new(0.0, 0.0, 0.8, 1.0);
                                        lspan.link_url = Some(url);
                                        spans.push(lspan);
                                    }
                                }
                            }
                            link_start = None;
                            if underline_count > 0 { underline_count -= 1; }
                        } else if let Some(href) = markup_get_attr(tag_content, "href") {
                            cur_link = Some(href);
                            link_start = Some(plain.len());
                            underline_count += 1;
                        }
                    }
                    "blockquote" => {
                        if is_closing {
                            if cur_quote_level > 0 { cur_quote_level -= 1; }
                        } else {
                            cur_quote_level += 1;
                        }
                        if !plain.is_empty() && !plain.ends_with('\n') {
                            plain.push('\n');
                        }
                    }
                    "center" => {
                        cur_align = if is_closing { Alignment::Left } else { Alignment::Center };
                    }
                    "pre" => {
                        in_pre = !is_closing;
                        if !is_closing && !plain.is_empty() && !plain.ends_with('\n') {
                            plain.push('\n');
                        }
                    }
                    "ul" | "ol" => {
                        if !plain.is_empty() && !plain.ends_with('\n') {
                            plain.push('\n');
                        }
                    }
                    "li" if !is_closing => {
                        if !plain.is_empty() && !plain.ends_with('\n') {
                            plain.push('\n');
                        }
                    }
                    "style" | "script" if !is_closing => {
                        let close_tag = format!("</{}>", tag_lower);
                        if let Some(close_pos) = markup[end_pos + 1..]
                            .to_ascii_lowercase()
                            .find(&close_tag)
                        {
                            i = end_pos + 1 + close_pos + close_tag.len();
                            continue;
                        }
                    }
                    "table" if !is_closing => {
                        let mut nesting = 1i32;
                        let mut scan = end_pos + 1;
                        let mut table_end_pos = None;
                        while scan < bytes.len() && nesting > 0 {
                            if bytes[scan] == b'<' {
                                let remaining = &markup[scan + 1..];
                                let remaining_lower = remaining.to_ascii_lowercase();
                                if remaining_lower.starts_with("table")
                                    && remaining.len() > 5
                                    && matches!(remaining.as_bytes()[5], b'>' | b' ' | b'/')
                                {
                                    nesting += 1;
                                } else if remaining_lower.starts_with("/table") {
                                    nesting -= 1;
                                    if nesting == 0 {
                                        table_end_pos = Some(scan);
                                    }
                                }
                            }
                            scan += 1;
                        }

                        if let Some(tep) = table_end_pos {
                            let after_close = markup[tep..]
                                .find('>')
                                .map(|p| tep + p + 1)
                                .unwrap_or(tep + 8);
                            if !plain.is_empty() && !plain.ends_with('\n') {
                                plain.push('\n');
                            }
                            parse_table_text(&markup[end_pos + 1..tep], &mut plain);
                            if !plain.is_empty() && !plain.ends_with('\n') {
                                plain.push('\n');
                            }
                            i = after_close;
                            continue;
                        }
                    }
                    "img" if !is_closing => {
                        let src_val = markup_get_attr(tag_content, "src");
                        let img_w = markup_get_attr(tag_content, "width")
                            .and_then(|s| s.parse::<i32>().ok())
                            .unwrap_or(0);
                        let img_h = markup_get_attr(tag_content, "height")
                            .and_then(|s| s.parse::<i32>().ok())
                            .unwrap_or(0);

                        let placeholder_start = plain.len();
                        plain.push('\u{FFFC}');
                        let mut span = MarkupStyleSpan::default();
                        span.start = placeholder_start;
                        span.end = plain.len();
                        span.link_url = Some(format!(
                            "img:{}:{}:{}",
                            img_w, img_h,
                            src_val.as_deref().unwrap_or("")
                        ));
                        spans.push(span);
                        i = end_pos + 1;
                        continue;
                    }
                    _ => {}
                }
                i = end_pos + 1;
                continue;
            }
            // HTML entities.
            else if bytes[i] == b'&' {
                let remaining = &markup[i..];
                if remaining.starts_with("&amp;") { plain.push('&'); i += 5; }
                else if remaining.starts_with("&lt;") { plain.push('<'); i += 4; }
                else if remaining.starts_with("&gt;") { plain.push('>'); i += 4; }
                else if remaining.starts_with("&quot;") { plain.push('"'); i += 6; }
                else if remaining.starts_with("&nbsp;") { plain.push(' '); i += 6; }
                else if remaining.starts_with("&apos;") { plain.push('\''); i += 6; }
                else if remaining.starts_with("&#") {
                    if let Some(semi_rel) = remaining.find(';') {
                        if semi_rel < 10 {
                            let num_str = &remaining[2..semi_rel];
                            let ch = if num_str.starts_with('x') || num_str.starts_with('X') {
                                u32::from_str_radix(&num_str[1..], 16).ok()
                            } else {
                                num_str.parse::<u32>().ok()
                            };
                            if let Some(code) = ch {
                                if let Some(c) = char::from_u32(code) {
                                    plain.push(c);
                                }
                            }
                            i += semi_rel + 1;
                        } else {
                            plain.push('&'); i += 1;
                        }
                    } else {
                        plain.push('&'); i += 1;
                    }
                } else {
                    plain.push('&'); i += 1;
                }
                continue;
            }
            // Regular text.
            else {
                let ch = bytes[i];
                if !in_pre && (ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n') {
                    if plain.is_empty() || !plain.ends_with(' ') {
                        plain.push(' ');
                    }
                    i += 1;
                    while i < bytes.len()
                        && !in_pre
                        && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n')
                    {
                        i += 1;
                    }
                    continue;
                }

                let char_start = plain.len();
                let rest = &markup[i..];
                if let Some(c) = rest.chars().next() {
                    plain.push(c);
                    i += c.len_utf8();
                } else {
                    i += 1;
                    continue;
                }

                if bold_count > 0 || italic_count > 0 || underline_count > 0
                    || has_color || cur_font_size > 0
                {
                    let new_bold = if bold_count > 0 { 1 } else { -1 };
                    let new_italic = if italic_count > 0 { 1 } else { -1 };
                    let new_underline = if underline_count > 0 { 1 } else { -1 };

                    let mut extended = false;
                    if let Some(prev) = spans.last_mut() {
                        if prev.end == char_start
                            && !prev.is_hr
                            && prev.link_url.is_none()
                            && prev.alignment.is_none()
                            && prev.quote_level.is_none()
                            && prev.bold == new_bold
                            && prev.italic == new_italic
                            && prev.underline == new_underline
                            && prev.font_size == cur_font_size
                            && prev.has_color == has_color
                        {
                            prev.end = plain.len();
                            extended = true;
                        }
                    }
                    if !extended {
                        let mut span = MarkupStyleSpan::default();
                        span.start = char_start;
                        span.end = plain.len();
                        span.bold = new_bold;
                        span.italic = new_italic;
                        span.underline = new_underline;
                        span.font_size = cur_font_size;
                        span.has_color = has_color;
                        if has_color { span.color = cur_color; }
                        spans.push(span);
                    }
                }

                if cur_align != Alignment::Left || cur_quote_level > 0 {
                    if let Some(last) = spans.last_mut() {
                        if last.end == plain.len()
                            && last.alignment.is_none()
                            && last.quote_level.is_none()
                        {
                            if cur_align != Alignment::Left {
                                last.alignment = Some(cur_align);
                            }
                            if cur_quote_level > 0 {
                                last.quote_level = Some(cur_quote_level);
                            }
                        }
                    }
                }
                continue;
            }
        }

        // ── Pass 2: Insert plain text, then apply style spans ──

        if plain.is_empty() { return; }

        self.doc.insert_text(offset, &plain);

        let byte_to_char_offset = |byte_off: usize| -> i32 {
            let clamped = byte_off.min(plain.len());
            plain[..clamped].chars().count() as i32
        };

        for idx in 0..spans.len() {
            let sp = spans[idx].clone();
            let sp_char_start = byte_to_char_offset(sp.start);
            let sp_char_end = byte_to_char_offset(sp.end);
            let sp_off = offset + sp_char_start;
            let sp_len = sp_char_end - sp_char_start;

            if sp.is_hr {
                self.doc.insert_hr(sp_off);
                continue;
            }

            // Inline styles — convert sentinel i32 to Option<bool>.
            if sp.bold != -1 || sp.italic != -1 || sp.underline != -1
                || sp.has_color || sp.font_size > 0
            {
                let bold = if sp.bold != -1 { Some(sp.bold != 0) } else { None };
                let italic = if sp.italic != -1 { Some(sp.italic != 0) } else { None };
                let underline = if sp.underline != -1 { Some(sp.underline != 0) } else { None };
                let color = if sp.has_color { Some(sp.color) } else { None };
                let fs = if sp.font_size > 0 { Some(sp.font_size) } else { None };
                self.doc.add_style_run(sp_off, sp_len, bold, italic, underline, color, fs);
            }

            // Links or images.
            if let Some(ref link_url) = sp.link_url {
                if link_url.starts_with("img:") {
                    let rest = &link_url[4..];
                    let parts: Vec<&str> = rest.splitn(3, ':').collect();
                    let iw = parts.first().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
                    let ih = parts.get(1).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
                    let isrc = parts.get(2).copied().unwrap_or("");

                    // Delete the U+FFFC placeholder.
                    self.doc.delete_range(sp_off, 1);

                    // Try to load texture from file path or URI.
                    let mut texture: Option<gdk4::Texture> = None;
                    if !isrc.is_empty() {
                        // Strip file:// prefix if present to get raw path
                        let raw_path = if isrc.starts_with("file://") {
                            &isrc[7..]
                        } else {
                            isrc
                        };
                        let file = gtk4::gio::File::for_path(raw_path);
                        if file.query_exists(gtk4::gio::Cancellable::NONE) {
                            texture = gdk4::Texture::from_file(&file).ok();
                        } else {
                            // Try as URI (http://, data:, etc.)
                            let uri_file = gtk4::gio::File::for_uri(isrc);
                            if uri_file.query_exists(gtk4::gio::Cancellable::NONE) {
                                texture = gdk4::Texture::from_file(&uri_file).ok();
                            }
                        }
                    }

                    let w = if iw > 0 { iw }
                        else { texture.as_ref().map(|t| t.width()).unwrap_or(100) };
                    let h = if ih > 0 { ih }
                        else { texture.as_ref().map(|t| t.height()).unwrap_or(100) };
                    self.doc.insert_graphic(sp_off, texture, w, h);
                } else {
                    self.doc.set_link(sp_off, sp_len, Some(link_url));
                }
            }

            // Paragraph-level styles.
            if let Some(align) = sp.alignment {
                self.doc.set_alignment(sp_off, sp_len, align);
            }
            if let Some(ql) = sp.quote_level {
                self.doc.set_quote_level(sp_off, sp_len, ql);
            }
        }
    }

    // ================================================================
    // get_markup — serialize document to HTML
    // ================================================================

    /// Serialize document range to HTML markup.
    ///
    /// Reverse of `insert_markup`: walks text + style runs and emits HTML.
    pub fn get_markup(&self, start: i32, end: i32) -> String {
        let full_text = self.doc.get_text();
        if full_text.is_empty() { return String::new(); }

        let doc_len = self.doc.get_length();
        let start = start.max(0);
        let end = if end < 0 || end > doc_len { doc_len } else { end };
        if start >= end { return String::new(); }

        let runs = self.doc.get_style_runs();
        let mut html = String::new();

        let mut cur_bold = false;
        let mut cur_italic = false;
        let mut cur_underline = false;
        let mut cur_has_color = false;
        let mut cur_font_size: i32 = 0;
        let mut cur_font_family: Option<String> = None;
        let mut cur_link: Option<String> = None;

        let chars: Vec<char> = full_text.chars().collect();
        let mut run_idx = 0;
        let mut pos = start;

        while pos < end {
            while run_idx < runs.len() {
                let r = &runs[run_idx];
                if r.offset + r.length > pos { break; }
                run_idx += 1;
            }

            let (cur_run, run_end) = if run_idx < runs.len() {
                let r = &runs[run_idx];
                if r.offset <= pos {
                    (Some(r), min(r.offset + r.length, end))
                } else {
                    (None, min(r.offset, end))
                }
            } else {
                (None, end)
            };

            let rb = cur_run.map_or(false, |r| r.bold);
            let ri = cur_run.map_or(false, |r| r.italic);
            let ru = cur_run.map_or(false, |r| r.underline);
            let rfs = cur_run.map_or(0, |r| r.font_size);
            let rff = cur_run.and_then(|r| r.font_family.as_deref());
            let rlu = cur_run.and_then(|r| r.link_url.as_deref());
            let r_has_color = cur_run.map_or(false, |r| {
                r.color.red() > 0.01 || r.color.green() > 0.01 || r.color.blue() > 0.01
            });

            // Close tags.
            if cur_link.is_some() && (rlu.is_none() || cur_link.as_deref() != rlu) {
                html.push_str("</a>"); cur_link = None;
            }
            if cur_underline && !ru { html.push_str("</u>"); cur_underline = false; }
            if cur_italic && !ri { html.push_str("</i>"); cur_italic = false; }
            if cur_bold && !rb { html.push_str("</b>"); cur_bold = false; }
            if cur_font_size != 0 && cur_font_size != rfs {
                html.push_str("</span>"); cur_font_size = 0;
            }
            if cur_font_family.is_some() && (rff.is_none() || cur_font_family.as_deref() != rff) {
                html.push_str("</span>"); cur_font_family = None;
            }
            if cur_has_color && !r_has_color {
                html.push_str("</span>"); cur_has_color = false;
            }

            // Open tags.
            if let Some(ff) = rff {
                if !ff.is_empty() && cur_font_family.is_none() {
                    html.push_str(&format!("<span style=\"font-family:{}\">", ff));
                    cur_font_family = Some(ff.to_string());
                }
            }
            if rfs > 0 && rfs != cur_font_size {
                if cur_font_size != 0 { html.push_str("</span>"); }
                html.push_str(&format!("<span style=\"font-size:{}pt\">", rfs));
                cur_font_size = rfs;
            }
            if r_has_color && !cur_has_color {
                if let Some(r) = cur_run {
                    html.push_str(&format!(
                        "<span style=\"color:#{:02x}{:02x}{:02x}\">",
                        (r.color.red() * 255.0) as u8,
                        (r.color.green() * 255.0) as u8,
                        (r.color.blue() * 255.0) as u8,
                    ));
                    cur_has_color = true;
                }
            }
            if rb && !cur_bold { html.push_str("<b>"); cur_bold = true; }
            if ri && !cur_italic { html.push_str("<i>"); cur_italic = true; }
            if ru && !cur_underline { html.push_str("<u>"); cur_underline = true; }
            if let Some(lu) = rlu {
                if cur_link.is_none() {
                    html.push_str(&format!("<a href=\"{}\">", lu));
                    cur_link = Some(lu.to_string());
                }
            }

            // Emit content.
            if let Some(r) = cur_run {
                if r.is_graphic {
                    if let Some(ref g) = r.graphic {
                        let src = g.src.as_deref().unwrap_or("");
                        html.push_str(&format!(
                            "<img src=\"{}\" width=\"{}\" height=\"{}\">",
                            src, g.width, g.height,
                        ));
                    }
                    pos = run_end;
                    continue;
                }
            }

            // Text with HTML escaping.
            for ci in pos..run_end {
                if (ci as usize) < chars.len() {
                    match chars[ci as usize] {
                        '\n' => html.push_str("<br>\n"),
                        '<' => html.push_str("&lt;"),
                        '>' => html.push_str("&gt;"),
                        '&' => html.push_str("&amp;"),
                        '"' => html.push_str("&quot;"),
                        '\u{FFFC}' => {}
                        c => html.push(c),
                    }
                }
            }

            pos = run_end;
        }

        // Close remaining open tags.
        if cur_link.is_some() { html.push_str("</a>"); }
        if cur_underline { html.push_str("</u>"); }
        if cur_italic { html.push_str("</i>"); }
        if cur_bold { html.push_str("</b>"); }
        if cur_font_size != 0 { html.push_str("</span>"); }
        if cur_font_family.is_some() { html.push_str("</span>"); }
        if cur_has_color { html.push_str("</span>"); }

        html
    }
}

// ════════════════════════════════════════════════════════════════════════
// Markup parser helpers
// ════════════════════════════════════════════════════════════════════════

/// Extract an attribute value from an HTML tag content string.
fn markup_get_attr(tag_content: &str, attr_name: &str) -> Option<String> {
    let lower = tag_content.to_ascii_lowercase();
    let pattern = format!("{}=", attr_name.to_ascii_lowercase());
    let pos = lower.find(&pattern)?;
    let rest = &tag_content[pos + pattern.len()..];
    let rest = rest.trim_start();
    if rest.is_empty() { return None; }

    let first = rest.as_bytes()[0];
    if first == b'"' || first == b'\'' {
        let quote = first as char;
        let inner = &rest[1..];
        let end = inner.find(quote).unwrap_or(inner.len());
        Some(inner[..end].to_string())
    } else {
        let end = rest.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

/// Parse a CSS color string (e.g., `#rrggbb`) into an RGBA.
fn parse_color(s: &str) -> Option<RGBA> {
    let s = s.trim();
    if s.starts_with('#') && s.len() == 7 {
        let r = u8::from_str_radix(&s[1..3], 16).ok()?;
        let g = u8::from_str_radix(&s[3..5], 16).ok()?;
        let b = u8::from_str_radix(&s[5..7], 16).ok()?;
        Some(RGBA::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0))
    } else {
        RGBA::parse(s).ok()
    }
}

/// Parse HTML font size attribute (1-7 scale or +N/-N) to point size.
fn parse_font_size(s: &str) -> i32 {
    let s = s.trim();
    let val = if s.starts_with('+') || s.starts_with('-') {
        3 + s.parse::<i32>().unwrap_or(0)
    } else {
        s.parse::<i32>().unwrap_or(3)
    };
    let sizes = [0, 8, 10, 12, 14, 18, 24, 36];
    let clamped = val.max(1).min(7) as usize;
    sizes[clamped]
}

/// Parse table HTML content into tab-separated plain text.
fn parse_table_text(table_html: &str, plain: &mut String) {
    let bytes = table_html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let end = match table_html[i..].find('>') {
                Some(p) => i + p,
                None => break,
            };
            let tag_content = &table_html[i + 1..end];
            let is_closing = tag_content.starts_with('/');
            let tag_name_str = if is_closing { &tag_content[1..] } else { tag_content };
            let name_end = tag_name_str
                .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
                .unwrap_or(tag_name_str.len());
            let tag_name = tag_name_str[..name_end].to_ascii_lowercase();

            if is_closing && tag_name == "tr" {
                if !plain.is_empty() && !plain.ends_with('\n') { plain.push('\n'); }
            } else if !is_closing && (tag_name == "td" || tag_name == "th") {
                if !plain.is_empty() && !plain.ends_with('\n') && !plain.ends_with('\t') {
                    plain.push('\t');
                }
            }
            i = end + 1;
        } else if bytes[i] == b'&' {
            let remaining = &table_html[i..];
            if remaining.starts_with("&amp;") { plain.push('&'); i += 5; }
            else if remaining.starts_with("&lt;") { plain.push('<'); i += 4; }
            else if remaining.starts_with("&gt;") { plain.push('>'); i += 4; }
            else if remaining.starts_with("&nbsp;") { plain.push(' '); i += 6; }
            else if remaining.starts_with("&quot;") { plain.push('"'); i += 6; }
            else { plain.push(bytes[i] as char); i += 1; }
        } else {
            let rest = &table_html[i..];
            if let Some(c) = rest.chars().next() {
                plain.push(c);
                i += c.len_utf8();
            } else {
                i += 1;
            }
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Markup helper unit tests ────────────────────────────────────

    #[test]
    fn test_markup_get_attr_quoted() {
        assert_eq!(
            markup_get_attr("p align=\"center\" class=\"x\"", "align"),
            Some("center".to_string())
        );
    }

    #[test]
    fn test_markup_get_attr_unquoted() {
        assert_eq!(
            markup_get_attr("font color=#ff0000 size=3", "color"),
            Some("#ff0000".to_string())
        );
        assert_eq!(
            markup_get_attr("font color=#ff0000 size=3", "size"),
            Some("3".to_string())
        );
    }

    #[test]
    fn test_markup_get_attr_single_quotes() {
        assert_eq!(
            markup_get_attr("a href='http://example.com'", "href"),
            Some("http://example.com".to_string())
        );
    }

    #[test]
    fn test_markup_get_attr_not_found() {
        assert_eq!(markup_get_attr("br", "align"), None);
    }

    #[test]
    fn test_parse_color_hex() {
        let c = parse_color("#ff0000").unwrap();
        assert!((c.red() - 1.0).abs() < 0.01);
        assert!(c.green().abs() < 0.01);

        let c = parse_color("#00ff00").unwrap();
        assert!(c.red().abs() < 0.01);
        assert!((c.green() - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_color_empty() {
        assert!(parse_color("").is_none());
    }

    #[test]
    fn test_parse_font_size_absolute() {
        assert_eq!(parse_font_size("1"), 8);
        assert_eq!(parse_font_size("3"), 12);
        assert_eq!(parse_font_size("7"), 36);
    }

    #[test]
    fn test_parse_font_size_relative() {
        assert_eq!(parse_font_size("+1"), 14); // 3+1=4 -> 14pt
        assert_eq!(parse_font_size("-1"), 10); // 3-1=2 -> 10pt
    }

    #[test]
    fn test_parse_table_text() {
        let mut plain = String::new();
        parse_table_text(
            "<tr><td>A</td><td>B</td></tr><tr><td>C</td><td>D</td></tr>",
            &mut plain,
        );
        assert!(plain.contains('A'));
        assert!(plain.contains('B'));
        assert!(plain.contains('\t'));
        assert!(plain.contains('\n'));
    }

    #[test]
    fn test_parse_table_text_entities() {
        let mut plain = String::new();
        parse_table_text("<tr><td>&amp;&lt;</td></tr>", &mut plain);
        assert!(plain.contains('&'));
        assert!(plain.contains('<'));
    }
}
