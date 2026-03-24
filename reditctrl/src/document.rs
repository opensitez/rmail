//! Core document model for rEditCtrl.
//!
//! Port of `gedit-document.c` / `gedit-document.h`.
//! Provides the rich-text document with:
//! - Style runs (bold, italic, underline, color, font size, font family, link URL)
//! - Graphics (inline images with GdkTexture, width, height, src path)
//! - Paragraph attributes (alignment, indent, quote level, bullet type, direction, HR, page break)
//! - Undo/redo via snapshots
//! - Text insert/delete with style run offset adjustment
//! - Attribute list generation for Pango

use gdk4 as gdk;
use std::cell::RefCell;

// Re-export RGBA from gdk4 for convenience across the crate.
pub use gdk4::RGBA;

// Re-export pango for the AttrList return type.
pub use pango;

// ---------------------------------------------------------------------------
// Public enums
// ---------------------------------------------------------------------------

/// Text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Alignment {
    #[default]
    Left = 0,
    Center = 1,
    Right = 2,
}

/// Text direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Direction {
    #[default]
    Ltr = 0,
    Rtl = 1,
}

/// Bullet type for list items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum BulletType {
    #[default]
    None = 0,
    Circle = 1,
    Square = 2,
    Disk = 3,
}

impl BulletType {
    /// Cycle: None -> Circle -> Square -> Disk -> None
    pub fn next(self) -> Self {
        match self {
            BulletType::None => BulletType::Circle,
            BulletType::Circle => BulletType::Square,
            BulletType::Square => BulletType::Disk,
            BulletType::Disk => BulletType::None,
        }
    }
}

// ---------------------------------------------------------------------------
// ParaAttrFlags  (bitmask — plain u32 constants to avoid external deps)
// ---------------------------------------------------------------------------

pub const PARA_ATTR_ALIGNMENT: u32 = 1 << 0;
pub const PARA_ATTR_INDENT: u32 = 1 << 1;
pub const PARA_ATTR_BULLET: u32 = 1 << 2;
pub const PARA_ATTR_QUOTE_LEVEL: u32 = 1 << 3;
pub const PARA_ATTR_HR: u32 = 1 << 4;
pub const PARA_ATTR_TAB_STOPS: u32 = 1 << 5;
pub const PARA_ATTR_DIRECTION: u32 = 1 << 6;
pub const PARA_ATTR_PAGE_BREAK: u32 = 1 << 7;

// ---------------------------------------------------------------------------
// TabStop
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabStop {
    pub position: i32, // pixels from left
}

// ---------------------------------------------------------------------------
// Graphic
// ---------------------------------------------------------------------------

/// Custom draw callback for host-drawn graphics (charts, graphs, etc.).
/// Called during rendering with cairo translated to the graphic's position.
pub type GraphicDrawFn = Box<dyn Fn(&pangocairo::cairo::Context, i32, i32) + 'static>;

/// Inline graphic embedded in the document.
pub struct Graphic {
    pub offset: i32,
    pub width: i32,
    pub height: i32,
    pub texture: Option<gdk::Texture>,
    pub src: Option<String>,
    /// Custom draw callback (charts, graphs, etc.). When set, used instead of texture.
    pub draw_func: Option<std::rc::Rc<dyn Fn(&pangocairo::cairo::Context, i32, i32)>>,
}

impl std::fmt::Debug for Graphic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Graphic")
            .field("offset", &self.offset)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("texture", &self.texture)
            .field("src", &self.src)
            .field("has_draw_func", &self.draw_func.is_some())
            .finish()
    }
}

impl Clone for Graphic {
    fn clone(&self) -> Self {
        Self {
            offset: self.offset,
            width: self.width,
            height: self.height,
            texture: self.texture.clone(),
            src: self.src.clone(),
            draw_func: self.draw_func.clone(),
        }
    }
}

impl Graphic {
    pub fn new(offset: i32, width: i32, height: i32, texture: Option<gdk::Texture>) -> Self {
        Self {
            offset,
            width,
            height,
            texture,
            src: None,
            draw_func: None,
        }
    }

    /// Create a graphic with a custom draw callback (no texture needed).
    pub fn with_draw_func<F>(offset: i32, width: i32, height: i32, draw_func: F) -> Self
    where
        F: Fn(&pangocairo::cairo::Context, i32, i32) + 'static,
    {
        Self {
            offset,
            width,
            height,
            texture: None,
            src: None,
            draw_func: Some(std::rc::Rc::new(draw_func)),
        }
    }
}

// ---------------------------------------------------------------------------
// StyleRun
// ---------------------------------------------------------------------------

/// A contiguous run of identically-styled characters.
#[derive(Debug, Clone)]
pub struct StyleRun {
    pub offset: i32,
    pub length: i32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: RGBA,
    pub font_size: i32,      // points, 0 == default
    pub font_family: Option<String>,
    pub is_graphic: bool,
    pub graphic: Option<Graphic>,
    pub link_url: Option<String>,
}

impl Default for StyleRun {
    fn default() -> Self {
        Self {
            offset: 0,
            length: 0,
            bold: false,
            italic: false,
            underline: false,
            color: RGBA::new(0.0, 0.0, 0.0, 1.0),
            font_size: 0,
            font_family: None,
            is_graphic: false,
            graphic: None,
            link_url: None,
        }
    }
}

impl StyleRun {
    /// Returns true when two runs have identical formatting (ignoring offset/length).
    pub fn format_eq(&self, other: &StyleRun) -> bool {
        self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.font_size == other.font_size
            && self.is_graphic == other.is_graphic
            && self.color.red() == other.color.red()
            && self.color.green() == other.color.green()
            && self.color.blue() == other.color.blue()
            && self.color.alpha() == other.color.alpha()
    }

    /// Returns true if two runs are contiguous and have identical formatting
    /// (excluding graphic runs which never merge).
    fn can_merge_with(&self, other: &StyleRun) -> bool {
        if self.is_graphic || other.is_graphic {
            return false;
        }
        self.offset + self.length == other.offset
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.color == other.color
            && self.font_size == other.font_size
            && self.font_family == other.font_family
            && self.link_url == other.link_url
    }
}

// ---------------------------------------------------------------------------
// ParaAttr
// ---------------------------------------------------------------------------

/// Paragraph-level attributes.
#[derive(Debug, Clone)]
pub struct ParaAttr {
    pub offset: i32,
    pub length: i32,
    pub alignment: Alignment,
    pub indent: i32,
    pub bullet: BulletType,
    pub quote_level: i32,
    pub is_hr: bool,
    pub page_break: bool,
    pub tab_stops: Vec<TabStop>,
    pub direction: Direction,
    pub mask: u32,
}

impl Default for ParaAttr {
    fn default() -> Self {
        Self {
            offset: 0,
            length: 0,
            alignment: Alignment::Left,
            indent: 0,
            bullet: BulletType::None,
            quote_level: 0,
            is_hr: false,
            page_break: false,
            tab_stops: Vec::new(),
            direction: Direction::Ltr,
            mask: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot (for undo/redo)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Snapshot {
    text: String,
    style_runs: Vec<StyleRun>,
    para_attrs: Vec<ParaAttr>,
}

// ---------------------------------------------------------------------------
// DocumentInner  (mutable state behind RefCell)
// ---------------------------------------------------------------------------

struct DocumentInner {
    text: String,
    style_runs: Vec<StyleRun>,
    para_attrs: Vec<ParaAttr>,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    on_changed: Option<Box<dyn Fn()>>,
    on_selection_changed: Option<Box<dyn Fn()>>,
    /// Spell error ranges: (char_offset, char_length). Rendered as wavy red underline.
    spell_errors: Vec<(i32, i32)>,
}

impl std::fmt::Debug for DocumentInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentInner")
            .field("text", &self.text)
            .field("style_runs", &self.style_runs)
            .field("para_attrs", &self.para_attrs)
            .field("undo_stack", &self.undo_stack.len())
            .field("redo_stack", &self.redo_stack.len())
            .field("on_changed", &self.on_changed.is_some())
            .field("on_selection_changed", &self.on_selection_changed.is_some())
            .finish()
    }
}

impl DocumentInner {
    fn new() -> Self {
        Self {
            text: String::new(),
            style_runs: Vec::new(),
            para_attrs: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            on_changed: None,
            on_selection_changed: None,
            spell_errors: Vec::new(),
        }
    }

    fn get_text(&self) -> String {
        self.text.clone()
    }

    fn get_length(&self) -> i32 {
        self.text.chars().count() as i32
    }

    fn take_snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.get_text(),
            style_runs: self.style_runs.clone(),
            para_attrs: self.para_attrs.clone(),
        }
    }

    fn push_undo(&mut self) {
        let snap = self.take_snapshot();
        self.undo_stack.push(snap);
        self.redo_stack.clear();
    }

    fn restore_snapshot(&mut self, snap: &Snapshot) {
        self.text = snap.text.clone();
        self.style_runs = snap.style_runs.clone();
        self.para_attrs = snap.para_attrs.clone();
    }

    /// Normalize line endings: \r\n -> \n, \r -> \n
    fn normalize_line_endings(text: &str) -> String {
        if !text.contains('\r') {
            return text.to_string();
        }
        let mut result = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\r' {
                result.push('\n');
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            } else {
                result.push(ch);
            }
        }
        result
    }

    fn insert_text(&mut self, offset: i32, text: &str) {
        let text = Self::normalize_line_endings(text);
        let byte_offset = char_offset_to_byte(&self.text, offset);
        self.text.insert_str(byte_offset, &text);

        let inserted_chars = text.chars().count() as i32;
        if inserted_chars > 0 {
            for run in &mut self.style_runs {
                if run.offset >= offset {
                    run.offset += inserted_chars;
                }
            }
        }
    }

    fn delete_range(&mut self, offset: i32, length: i32) {
        if length <= 0 {
            return;
        }

        let byte_start = char_offset_to_byte(&self.text, offset);
        let byte_end = char_offset_to_byte(&self.text, offset + length);
        self.text.drain(byte_start..byte_end);

        let del_end = offset + length;

        // Adjust or remove style runs overlapping the deleted range
        self.style_runs.retain_mut(|run| {
            let run_end = run.offset + run.length;
            if run_end <= offset {
                true
            } else if run.offset >= del_end {
                run.offset -= length;
                true
            } else {
                // Overlapping — remove
                false
            }
        });

        // Adjust or remove para attrs overlapping the deleted range
        self.para_attrs.retain_mut(|p| {
            let a = p.offset;
            let b = p.offset + p.length;
            if b <= offset || a >= del_end {
                if a >= del_end {
                    p.offset -= length;
                }
                true
            } else {
                false
            }
        });
    }

    /// Resolve effective style at a character offset by walking all runs.
    fn get_style_at(&self, offset: i32) -> StyleRun {
        let mut out = StyleRun {
            offset,
            ..Default::default()
        };
        for r in &self.style_runs {
            if offset >= r.offset && offset < r.offset + r.length {
                if r.bold {
                    out.bold = true;
                }
                if r.italic {
                    out.italic = true;
                }
                if r.underline {
                    out.underline = true;
                }
                if r.font_size > 0 {
                    out.font_size = r.font_size;
                }
                out.color = r.color;
                if r.is_graphic {
                    out.is_graphic = true;
                    out.graphic = r.graphic.clone();
                }
                if r.link_url.is_some() {
                    out.link_url = r.link_url.clone();
                }
                if r.font_family.is_some() {
                    out.font_family = r.font_family.clone();
                }
            }
        }
        out
    }

    /// Merge a style change into existing runs, preserving attributes not being changed.
    /// bold/italic/underline: None = preserve existing, Some(val) = set
    /// color: None = preserve existing per-run colors
    /// font_size: None = preserve existing, Some(val) = set absolute value
    fn add_style_run(
        &mut self,
        offset: i32,
        length: i32,
        bold: Option<bool>,
        italic: Option<bool>,
        underline: Option<bool>,
        color: Option<RGBA>,
        font_size: Option<i32>,
    ) {
        if length <= 0 {
            return;
        }
        let range_end = offset + length;
        let mut new_runs: Vec<StyleRun> = Vec::new();
        let mut mid_runs: Vec<StyleRun> = Vec::new();

        for run in &self.style_runs {
            let rs = run.offset;
            let re = rs + run.length;

            if re <= offset || rs >= range_end {
                new_runs.push(run.clone());
                continue;
            }

            // Prefix before the target range
            if rs < offset {
                let mut pre = run.clone();
                pre.length = offset - rs;
                new_runs.push(pre);
            }

            // Overlapping middle — copy then merge new attributes
            let ms = rs.max(offset);
            let me = re.min(range_end);
            let mut mid = run.clone();
            mid.offset = ms;
            mid.length = me - ms;
            if let Some(b) = bold {
                mid.bold = b;
            }
            if let Some(i) = italic {
                mid.italic = i;
            }
            if let Some(u) = underline {
                mid.underline = u;
            }
            if let Some(c) = color {
                mid.color = c;
            }
            if let Some(fs) = font_size {
                mid.font_size = fs;
            }
            mid_runs.push(mid);

            // Suffix after the target range
            if re > range_end {
                let mut suf = run.clone();
                suf.offset = range_end;
                suf.length = re - range_end;
                new_runs.push(suf);
            }
        }

        // Sort mid_runs by offset
        mid_runs.sort_by_key(|r| r.offset);

        // Walk the target range, inserting mid_runs and filling gaps
        let mut cursor = offset;
        for mr in mid_runs {
            if mr.offset > cursor {
                let eff = self.get_style_at(cursor);
                let gap = StyleRun {
                    offset: cursor,
                    length: mr.offset - cursor,
                    bold: bold.unwrap_or(eff.bold),
                    italic: italic.unwrap_or(eff.italic),
                    underline: underline.unwrap_or(eff.underline),
                    color: color.unwrap_or(eff.color),
                    font_size: font_size.unwrap_or(eff.font_size),
                    font_family: eff.font_family,
                    link_url: eff.link_url,
                    ..Default::default()
                };
                new_runs.push(gap);
            }
            cursor = mr.offset + mr.length;
            new_runs.push(mr);
        }

        // Trailing gap
        if cursor < range_end {
            let eff = self.get_style_at(cursor);
            let gap = StyleRun {
                offset: cursor,
                length: range_end - cursor,
                bold: bold.unwrap_or(eff.bold),
                italic: italic.unwrap_or(eff.italic),
                underline: underline.unwrap_or(eff.underline),
                color: color.unwrap_or(eff.color),
                font_size: font_size.unwrap_or(eff.font_size),
                font_family: eff.font_family,
                link_url: eff.link_url,
                ..Default::default()
            };
            new_runs.push(gap);
        }

        self.style_runs = new_runs;
    }

    /// Check whether all positions in [offset, offset+length) have the requested
    /// style flag(s) present.
    fn style_active_across_range(
        &self,
        offset: i32,
        length: i32,
        bold: bool,
        italic: bool,
        underline: bool,
    ) -> bool {
        if length <= 0 {
            return false;
        }
        for i in 0..length {
            let pos = offset + i;
            let mut has_flag = false;
            for run in &self.style_runs {
                if pos >= run.offset && pos < run.offset + run.length {
                    if (bold && run.bold) || (italic && run.italic) || (underline && run.underline)
                    {
                        has_flag = true;
                        break;
                    }
                }
            }
            if !has_flag {
                return false;
            }
        }
        true
    }

    /// Remove the specified style flags from any style runs that intersect the
    /// given range, splitting runs as needed.
    fn remove_style_flags_in_range(
        &mut self,
        offset: i32,
        length: i32,
        rem_bold: bool,
        rem_italic: bool,
        rem_underline: bool,
    ) {
        if length <= 0 {
            return;
        }
        let range_end = offset + length;
        let mut new_runs: Vec<StyleRun> = Vec::new();

        for run in &self.style_runs {
            let rs = run.offset;
            let re = rs + run.length;

            if re <= offset || rs >= range_end {
                new_runs.push(run.clone());
                continue;
            }

            if rs < offset {
                let mut pre = run.clone();
                pre.length = offset - rs;
                new_runs.push(pre);
            }

            let mid_s = rs.max(offset);
            let mid_e = re.min(range_end);
            let mut mid = run.clone();
            mid.offset = mid_s;
            mid.length = mid_e - mid_s;
            if rem_bold {
                mid.bold = false;
            }
            if rem_italic {
                mid.italic = false;
            }
            if rem_underline {
                mid.underline = false;
            }
            new_runs.push(mid);

            if re > range_end {
                let mut suf = run.clone();
                suf.offset = range_end;
                suf.length = re - range_end;
                new_runs.push(suf);
            }
        }

        // Merge adjacent runs with identical formatting
        let mut merged: Vec<StyleRun> = Vec::new();
        for r in new_runs {
            if let Some(last) = merged.last_mut() {
                if last.can_merge_with(&r) {
                    last.length += r.length;
                    continue;
                }
            }
            merged.push(r);
        }

        self.style_runs = merged;
    }

    /// Get merged paragraph attributes for a given char range.
    fn get_para_attr(&self, offset: i32, length: i32) -> ParaAttr {
        let mut out = ParaAttr {
            offset,
            length,
            ..Default::default()
        };

        for p in &self.para_attrs {
            let a = p.offset;
            let b = p.offset + p.length;
            let o = offset;
            let e = offset + length;
            if b <= o || a >= e {
                continue;
            }
            if p.mask & PARA_ATTR_ALIGNMENT != 0 {
                out.alignment = p.alignment;
            }
            if p.mask & PARA_ATTR_INDENT != 0 {
                out.indent = p.indent;
            }
            if p.mask & PARA_ATTR_BULLET != 0 {
                out.bullet = p.bullet;
            }
            if p.mask & PARA_ATTR_QUOTE_LEVEL != 0 {
                out.quote_level = p.quote_level;
            }
            if p.mask & PARA_ATTR_HR != 0 {
                out.is_hr = p.is_hr;
            }
            if p.mask & PARA_ATTR_TAB_STOPS != 0 {
                out.tab_stops = p.tab_stops.clone();
            }
            if p.mask & PARA_ATTR_DIRECTION != 0 {
                out.direction = p.direction;
            }
            if p.mask & PARA_ATTR_PAGE_BREAK != 0 {
                out.page_break = p.page_break;
            }
        }
        out
    }

    fn get_text_range(&self, offset: i32, length: i32) -> String {
        if length <= 0 {
            return String::new();
        }
        let chars: Vec<char> = self.text.chars().collect();
        let start = (offset as usize).min(chars.len());
        let end = ((offset + length) as usize).min(chars.len());
        chars[start..end].iter().collect()
    }

    /// Build a PangoAttrList representing all style runs.
    fn get_attr_list(&self) -> pango::AttrList {
        let text = self.get_text();
        let list = pango::AttrList::new();
        if text.is_empty() {
            return list;
        }
        for run in &self.style_runs {
            let start_byte = char_offset_to_byte(&text, run.offset) as u32;
            let end_byte = char_offset_to_byte(&text, run.offset + run.length) as u32;
            add_run_attrs_to_list(&list, run, start_byte, end_byte);
        }
        list
    }

    /// Build a PangoAttrList for a specific character range (byte-relative indices).
    fn get_attr_list_for_range(&self, range_offset: i32, range_length: i32) -> pango::AttrList {
        let text = self.get_text();
        let list = pango::AttrList::new();
        if text.is_empty() {
            return list;
        }

        let range_start_byte = char_offset_to_byte(&text, range_offset);
        let range_end_byte = char_offset_to_byte(&text, range_offset + range_length);

        for run in &self.style_runs {
            let run_start_byte = char_offset_to_byte(&text, run.offset);
            let run_end_byte = char_offset_to_byte(&text, run.offset + run.length);

            let inter_start = run_start_byte.max(range_start_byte);
            let inter_end = run_end_byte.min(range_end_byte);
            if inter_end <= inter_start {
                continue;
            }
            let rel_start = (inter_start - range_start_byte) as u32;
            let rel_end = (inter_end - range_start_byte) as u32;

            add_run_attrs_to_list(&list, run, rel_start, rel_end);
        }

        // Spell error wavy red underlines
        for &(err_offset, err_length) in &self.spell_errors {
            let err_start_byte = char_offset_to_byte(&text, err_offset);
            let err_end_byte = char_offset_to_byte(&text, err_offset + err_length);
            let inter_start = err_start_byte.max(range_start_byte);
            let inter_end = err_end_byte.min(range_end_byte);
            if inter_end <= inter_start { continue; }
            let rel_start = (inter_start - range_start_byte) as u32;
            let rel_end = (inter_end - range_start_byte) as u32;

            let mut a = pango::AttrInt::new_underline(pango::Underline::Error);
            a.set_start_index(rel_start);
            a.set_end_index(rel_end);
            list.insert(a);

            let mut c = pango::AttrColor::new_underline_color(65535, 0, 0); // red
            c.set_start_index(rel_start);
            c.set_end_index(rel_end);
            list.insert(c);
        }

        list
    }

    // -----------------------------------------------------------------------
    // Navigation helpers
    // -----------------------------------------------------------------------

    fn find_word_boundary_left(&self, offset: i32) -> i32 {
        let text = self.get_text();
        if text.is_empty() || offset <= 0 {
            return 0;
        }
        let chars: Vec<char> = text.chars().collect();
        let mut pos = (offset - 1).min(chars.len() as i32 - 1).max(0) as usize;

        // Skip whitespace backwards
        while pos > 0 && chars[pos].is_whitespace() {
            pos -= 1;
        }
        // Skip word characters backwards
        while pos > 0 {
            let prev = chars[pos - 1];
            if !prev.is_alphanumeric() && prev != '_' {
                break;
            }
            pos -= 1;
        }
        pos as i32
    }

    fn find_word_boundary_right(&self, offset: i32) -> i32 {
        let text = self.get_text();
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len() as i32;
        if offset >= len {
            return len;
        }
        let mut pos = offset as usize;

        // Skip word characters forward
        while (pos as i32) < len && (chars[pos].is_alphanumeric() || chars[pos] == '_') {
            pos += 1;
        }
        // Skip whitespace forward
        while (pos as i32) < len && chars[pos].is_whitespace() {
            pos += 1;
        }
        pos as i32
    }

    fn find_line_start(&self, offset: i32) -> i32 {
        let text = self.get_text();
        if text.is_empty() || offset <= 0 {
            return 0;
        }
        let chars: Vec<char> = text.chars().collect();
        let mut pos = offset as usize;
        while pos > 0 {
            if chars[pos - 1] == '\n' {
                break;
            }
            pos -= 1;
        }
        pos as i32
    }

    fn find_line_end(&self, offset: i32) -> i32 {
        let text = self.get_text();
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len() as i32;
        let mut pos = offset as usize;
        while (pos as i32) < len {
            if chars[pos] == '\n' {
                break;
            }
            pos += 1;
        }
        pos as i32
    }

    fn get_word_bounds(&self, offset: i32) -> (i32, i32) {
        let text = self.get_text();
        if text.is_empty() {
            return (offset, offset);
        }
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len() as i32;
        let mut start = offset;
        let mut end = offset;

        while start > 0 {
            let ch = chars[(start - 1) as usize];
            if ch.is_whitespace() || ch == '\n' {
                break;
            }
            start -= 1;
        }
        while end < len {
            let ch = chars[end as usize];
            if ch.is_whitespace() || ch == '\n' {
                break;
            }
            end += 1;
        }
        (start, end)
    }

    fn is_word_boundary_char(&self, offset: i32) -> bool {
        let text = self.get_text();
        let chars: Vec<char> = text.chars().collect();
        if offset < 0 || offset as usize >= chars.len() {
            return true;
        }
        let ch = chars[offset as usize];
        ch.is_whitespace() || ch == '\n' || ch.is_ascii_punctuation()
    }

    // -----------------------------------------------------------------------
    // Find / Replace
    // -----------------------------------------------------------------------

    fn find_text(
        &self,
        search_text: &str,
        start_offset: i32,
        end_offset: i32,
        case_sensitive: bool,
    ) -> i32 {
        if search_text.is_empty() {
            return -1;
        }
        let full_text = self.get_text();
        let doc_len = full_text.chars().count() as i32;
        let end_offset = if end_offset < 0 || end_offset > doc_len {
            doc_len
        } else {
            end_offset
        };
        let start_offset = start_offset.max(0);
        let search_len = search_text.chars().count() as i32;

        let full_chars: Vec<char> = full_text.chars().collect();
        let search_chars: Vec<char> = if case_sensitive {
            search_text.chars().collect()
        } else {
            search_text
                .chars()
                .flat_map(|c| c.to_lowercase())
                .collect()
        };
        let search_len_actual = search_chars.len() as i32;

        for i in start_offset..=(end_offset - search_len) {
            let mut matched = true;
            for j in 0..search_len_actual {
                let idx = (i + j) as usize;
                if idx >= full_chars.len() {
                    matched = false;
                    break;
                }
                let doc_ch = if case_sensitive {
                    full_chars[idx]
                } else {
                    full_chars[idx]
                        .to_lowercase()
                        .next()
                        .unwrap_or(full_chars[idx])
                };
                if doc_ch != search_chars[j as usize] {
                    matched = false;
                    break;
                }
            }
            if matched {
                return i;
            }
        }
        -1
    }

    /// Set the pixel dimensions of the graphic at the given character offset.
    fn set_graphic_size(&mut self, graphic_offset: i32, width: i32, height: i32) {
        for run in &mut self.style_runs {
            if run.is_graphic && run.offset == graphic_offset {
                if let Some(ref mut gr) = run.graphic {
                    gr.width = width;
                    gr.height = height;
                }
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Free function: add pango attributes for a single style run
// ---------------------------------------------------------------------------

fn add_run_attrs_to_list(
    list: &pango::AttrList,
    run: &StyleRun,
    start_index: u32,
    end_index: u32,
) {
    if run.bold {
        let mut a = pango::AttrInt::new_weight(pango::Weight::Bold);
        a.set_start_index(start_index);
        a.set_end_index(end_index);
        list.insert(a);
    }
    if run.italic {
        let mut a = pango::AttrInt::new_style(pango::Style::Italic);
        a.set_start_index(start_index);
        a.set_end_index(end_index);
        list.insert(a);
    }
    if run.underline {
        let mut a = pango::AttrInt::new_underline(pango::Underline::Single);
        a.set_start_index(start_index);
        a.set_end_index(end_index);
        list.insert(a);
    }
    if run.font_size > 0 {
        let mut a = pango::AttrSize::new(run.font_size * pango::SCALE);
        a.set_start_index(start_index);
        a.set_end_index(end_index);
        list.insert(a);
    }
    if let Some(ref family) = run.font_family {
        if !family.is_empty() {
            let mut a = pango::AttrString::new_family(family);
            a.set_start_index(start_index);
            a.set_end_index(end_index);
            list.insert(a);
        }
    }
    if run.is_graphic {
        if let Some(ref gr) = run.graphic {
            let ink = pango::Rectangle::new(
                0,
                -(gr.height * pango::SCALE),
                gr.width * pango::SCALE,
                gr.height * pango::SCALE,
            );
            let mut a = pango::AttrShape::new(&ink, &ink);
            a.set_start_index(start_index);
            a.set_end_index(end_index);
            list.insert(a);
        }
    }
    // Foreground color (16-bit)
    {
        let mut col = pango::AttrColor::new_foreground(
            (run.color.red() * 65535.0) as u16,
            (run.color.green() * 65535.0) as u16,
            (run.color.blue() * 65535.0) as u16,
        );
        col.set_start_index(start_index);
        col.set_end_index(end_index);
        list.insert(col);
    }
}

// ---------------------------------------------------------------------------
// Utility: char offset to byte offset in a UTF-8 string
// ---------------------------------------------------------------------------

fn char_offset_to_byte(text: &str, char_offset: i32) -> usize {
    text.char_indices()
        .nth(char_offset as usize)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(text.len())
}

// ---------------------------------------------------------------------------
// Document — plain Rust struct with interior mutability
// ---------------------------------------------------------------------------

/// Rich-text document backed by a plain `String` for text storage with parallel
/// vectors of `StyleRun` and `ParaAttr` for formatting, plus snapshot-based
/// undo/redo.
#[derive(Debug)]
pub struct Document {
    inner: RefCell<DocumentInner>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait abstracting over the document so clipboard, table, and print modules
/// can be tested with mock implementations.
pub trait DocumentOps {
    fn get_text_opt(&self) -> Option<String>;
    fn get_length(&self) -> i32;
    fn get_style_runs(&self) -> Vec<StyleRun>;
    fn delete_range(&self, offset: i32, length: i32);
    fn paste_formatted(&self, offset: i32, text: &str, style_runs: &[StyleRun]);
    fn find_word_boundary_left(&self, offset: i32) -> i32;
    fn find_word_boundary_right(&self, offset: i32) -> i32;
}

/// Trait abstracting over the full document interface so input, render,
/// and test modules can work with mock or real implementations.
///
/// All methods use `&self` because the `Document` uses interior mutability.
pub trait DocumentApi {
    fn get_text(&self) -> Option<String>;
    fn get_length(&self) -> i32;
    fn get_style_runs(&self) -> Vec<StyleRun>;
    fn delete_range(&self, offset: i32, length: i32);
    fn insert_text(&self, offset: i32, text: &str);
    fn undo(&self);
    fn redo(&self);
    fn get_link_at(&self, offset: i32) -> Option<String>;
    fn find_word_boundary_left(&self, offset: i32) -> i32;
    fn find_word_boundary_right(&self, offset: i32) -> i32;
    fn find_line_start(&self, offset: i32) -> i32;
    fn find_line_end(&self, offset: i32) -> i32;
    fn get_attr_list_for_range(&self, offset: i32, length: i32) -> pango::AttrList;
    fn set_graphic_size(&self, graphic_offset: i32, width: i32, height: i32);
    fn get_text_range(&self, offset: i32, length: i32) -> String;
}

impl DocumentApi for Document {
    fn get_text(&self) -> Option<String> {
        Document::get_text_opt(self)
    }
    fn get_length(&self) -> i32 {
        Document::get_length(self)
    }
    fn get_style_runs(&self) -> Vec<StyleRun> {
        Document::get_style_runs(self)
    }
    fn delete_range(&self, offset: i32, length: i32) {
        Document::delete_range(self, offset, length);
    }
    fn insert_text(&self, offset: i32, text: &str) {
        Document::insert_text(self, offset, text);
    }
    fn undo(&self) {
        Document::undo(self);
    }
    fn redo(&self) {
        Document::redo(self);
    }
    fn get_link_at(&self, offset: i32) -> Option<String> {
        Document::get_link_at(self, offset)
    }
    fn find_word_boundary_left(&self, offset: i32) -> i32 {
        Document::find_word_boundary_left(self, offset)
    }
    fn find_word_boundary_right(&self, offset: i32) -> i32 {
        Document::find_word_boundary_right(self, offset)
    }
    fn find_line_start(&self, offset: i32) -> i32 {
        Document::find_line_start(self, offset)
    }
    fn find_line_end(&self, offset: i32) -> i32 {
        Document::find_line_end(self, offset)
    }
    fn get_attr_list_for_range(&self, offset: i32, length: i32) -> pango::AttrList {
        Document::get_attr_list_for_range(self, offset, length)
    }
    fn set_graphic_size(&self, graphic_offset: i32, width: i32, height: i32) {
        Document::set_graphic_size(self, graphic_offset, width, height);
    }
    fn get_text_range(&self, offset: i32, length: i32) -> String {
        Document::get_text_range(self, offset, length)
    }
}

impl DocumentOps for Document {
    fn get_text_opt(&self) -> Option<String> {
        Document::get_text_opt(self)
    }
    fn get_length(&self) -> i32 {
        Document::get_length(self)
    }
    fn get_style_runs(&self) -> Vec<StyleRun> {
        Document::get_style_runs(self)
    }
    fn delete_range(&self, offset: i32, length: i32) {
        Document::delete_range(self, offset, length);
    }
    fn paste_formatted(&self, offset: i32, text: &str, style_runs: &[StyleRun]) {
        Document::paste_formatted(self, offset, text, style_runs);
    }
    fn find_word_boundary_left(&self, offset: i32) -> i32 {
        Document::find_word_boundary_left(self, offset)
    }
    fn find_word_boundary_right(&self, offset: i32) -> i32 {
        Document::find_word_boundary_right(self, offset)
    }
}

impl Document {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(DocumentInner::new()),
        }
    }

    fn emit_changed(&self) {
        let inner = self.inner.borrow();
        if let Some(ref cb) = inner.on_changed {
            cb();
        }
    }

    #[allow(dead_code)]
    fn emit_selection_changed(&self) {
        let inner = self.inner.borrow();
        if let Some(ref cb) = inner.on_selection_changed {
            cb();
        }
    }

    /// Register a callback for document-changed notifications.
    pub fn connect_changed<F: Fn() + 'static>(&self, f: F) {
        self.inner.borrow_mut().on_changed = Some(Box::new(f));
    }

    /// Register a callback for selection-changed notifications.
    #[allow(dead_code)]
    pub fn connect_selection_changed<F: Fn() + 'static>(&self, f: F) {
        self.inner.borrow_mut().on_selection_changed = Some(Box::new(f));
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Insert `text` at character `offset`. Creates an undo snapshot.
    pub fn insert_text(&self, offset: i32, text: &str) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();
            inner.insert_text(offset, text);
        }
        self.emit_changed();
    }

    /// Delete `length` characters starting at `offset`. Creates an undo snapshot.
    pub fn delete_range(&self, offset: i32, length: i32) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();
            inner.delete_range(offset, length);
        }
        self.emit_changed();
    }

    /// Return the full document text.
    pub fn get_text(&self) -> String {
        self.inner.borrow().get_text()
    }

    /// Return `None` if empty, `Some(text)` otherwise.
    pub fn get_text_opt(&self) -> Option<String> {
        let t = self.get_text();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    }

    /// Return document length in characters.
    pub fn get_length(&self) -> i32 {
        self.inner.borrow().get_length()
    }

    /// Return substring at `[offset, offset+length)`.
    pub fn get_text_range(&self, offset: i32, length: i32) -> String {
        self.inner.borrow().get_text_range(offset, length)
    }

    /// Merge a style change into existing runs.
    /// `bold`/`italic`/`underline`: `None` = preserve existing, `Some(val)` = set.
    /// `color`: `None` = preserve existing.
    /// `font_size`: `None` = preserve existing, `Some(val)` = set absolute size.
    pub fn add_style_run(
        &self,
        offset: i32,
        length: i32,
        bold: Option<bool>,
        italic: Option<bool>,
        underline: Option<bool>,
        color: Option<RGBA>,
        font_size: Option<i32>,
    ) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();
            inner.add_style_run(offset, length, bold, italic, underline, color, font_size);
        }
        self.emit_changed();
    }

    /// Toggle styles over a range: enable if not uniformly present, otherwise remove.
    pub fn toggle_style(
        &self,
        offset: i32,
        length: i32,
        bold: bool,
        italic: bool,
        underline: bool,
    ) {
        if length <= 0 {
            return;
        }
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();

            let all_have =
                inner.style_active_across_range(offset, length, bold, italic, underline);
            if all_have {
                inner.remove_style_flags_in_range(offset, length, bold, italic, underline);
            } else {
                inner.add_style_run(
                    offset,
                    length,
                    if bold { Some(true) } else { None },
                    if italic { Some(true) } else { None },
                    if underline { Some(true) } else { None },
                    None,
                    None,
                );
            }
        }
        self.emit_changed();
    }

    /// Resolve the effective style at a character offset.
    pub fn get_style_at(&self, offset: i32) -> StyleRun {
        self.inner.borrow().get_style_at(offset)
    }

    /// Return a clone of all style runs.
    pub fn get_style_runs(&self) -> Vec<StyleRun> {
        self.inner.borrow().style_runs.clone()
    }

    /// Insert an inline graphic (placeholder U+FFFC) at `offset`.
    pub fn insert_graphic(
        &self,
        offset: i32,
        texture: Option<gdk::Texture>,
        width: i32,
        height: i32,
    ) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();

            // Insert placeholder U+FFFC
            let byte_offset = char_offset_to_byte(&inner.text, offset);
            inner.text.insert_str(byte_offset, "\u{FFFC}");

            // Shift existing runs at or after the insertion point
            for r in &mut inner.style_runs {
                if r.offset >= offset {
                    r.offset += 1;
                }
            }

            let gr = Graphic {
                offset,
                width,
                height,
                texture: texture.clone(),
                src: None,
                draw_func: None,
            };

            let run = StyleRun {
                offset,
                length: 1,
                is_graphic: true,
                graphic: Some(gr),
                ..Default::default()
            };

            // Insert in sorted order by offset
            let pos = inner
                .style_runs
                .iter()
                .position(|r| r.offset > offset)
                .unwrap_or(inner.style_runs.len());
            inner.style_runs.insert(pos, run);
        }
        self.emit_changed();
    }

    /// Insert a custom-drawn graphic (charts, graphs, etc.) at `offset`.
    pub fn insert_custom_graphic<F>(
        &self,
        offset: i32,
        width: i32,
        height: i32,
        draw_func: F,
    ) where
        F: Fn(&pangocairo::cairo::Context, i32, i32) + 'static,
    {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();

            let byte_offset = char_offset_to_byte(&inner.text, offset);
            inner.text.insert_str(byte_offset, "\u{FFFC}");

            for r in &mut inner.style_runs {
                if r.offset >= offset {
                    r.offset += 1;
                }
            }

            let gr = Graphic::with_draw_func(offset, width, height, draw_func);

            let run = StyleRun {
                offset,
                length: 1,
                is_graphic: true,
                graphic: Some(gr),
                ..Default::default()
            };

            let pos = inner
                .style_runs
                .iter()
                .position(|r| r.offset > offset)
                .unwrap_or(inner.style_runs.len());
            inner.style_runs.insert(pos, run);
        }
        self.emit_changed();
    }

    /// Set font family on a range of text.
    pub fn set_font_family(&self, offset: i32, length: i32, family: Option<&str>) {
        if length <= 0 {
            return;
        }
        {
            let mut inner = self.inner.borrow_mut();
            let range_end = offset + length;
            let mut new_runs: Vec<StyleRun> = Vec::new();

            for run in &inner.style_runs {
                let rs = run.offset;
                let re = rs + run.length;
                if re <= offset || rs >= range_end {
                    new_runs.push(run.clone());
                    continue;
                }
                if rs < offset {
                    let mut pre = run.clone();
                    pre.length = offset - rs;
                    new_runs.push(pre);
                }
                let ms = rs.max(offset);
                let me = re.min(range_end);
                let mut mid = run.clone();
                mid.offset = ms;
                mid.length = me - ms;
                mid.font_family = family.map(|s| s.to_string());
                new_runs.push(mid);
                if re > range_end {
                    let mut suf = run.clone();
                    suf.offset = range_end;
                    suf.length = re - range_end;
                    new_runs.push(suf);
                }
            }

            if inner.style_runs.is_empty() {
                new_runs.push(StyleRun {
                    offset,
                    length,
                    font_family: family.map(|s| s.to_string()),
                    ..Default::default()
                });
            }

            inner.style_runs = new_runs;
        }
        self.emit_changed();
    }

    /// Set a hyperlink URL on a range of text. Pass `None` to remove.
    pub fn set_link(&self, offset: i32, length: i32, url: Option<&str>) {
        if length <= 0 {
            return;
        }
        {
            let mut inner = self.inner.borrow_mut();
            let range_end = offset + length;
            let mut new_runs: Vec<StyleRun> = Vec::new();

            for run in &inner.style_runs {
                let rs = run.offset;
                let re = rs + run.length;
                if re <= offset || rs >= range_end {
                    new_runs.push(run.clone());
                    continue;
                }
                if rs < offset {
                    let mut pre = run.clone();
                    pre.length = offset - rs;
                    new_runs.push(pre);
                }
                let ms = rs.max(offset);
                let me = re.min(range_end);
                let mut mid = run.clone();
                mid.offset = ms;
                mid.length = me - ms;
                mid.link_url = url.map(|s| s.to_string());
                if url.is_some_and(|u| !u.is_empty()) {
                    mid.underline = true;
                    mid.color = RGBA::new(0.0, 0.0, 0.8, 1.0);
                }
                new_runs.push(mid);
                if re > range_end {
                    let mut suf = run.clone();
                    suf.offset = range_end;
                    suf.length = re - range_end;
                    new_runs.push(suf);
                }
            }

            if inner.style_runs.is_empty() {
                let mut nr = StyleRun {
                    offset,
                    length,
                    link_url: url.map(|s| s.to_string()),
                    ..Default::default()
                };
                if url.is_some_and(|u| !u.is_empty()) {
                    nr.underline = true;
                    nr.color = RGBA::new(0.0, 0.0, 0.8, 1.0);
                }
                new_runs.push(nr);
            }

            inner.style_runs = new_runs;
        }
        self.emit_changed();
    }

    /// Get link URL at a character offset.
    pub fn get_link_at(&self, offset: i32) -> Option<String> {
        let inner = self.inner.borrow();
        for run in &inner.style_runs {
            if offset >= run.offset
                && offset < run.offset + run.length
                && run.link_url.as_ref().is_some_and(|u| !u.is_empty())
            {
                return run.link_url.clone();
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // Paragraph attribute APIs
    // -----------------------------------------------------------------------

    /// Set text alignment for a character range.
    pub fn set_alignment(&self, offset: i32, length: i32, align: Alignment) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();
            inner.para_attrs.push(ParaAttr {
                offset,
                length,
                alignment: align,
                mask: PARA_ATTR_ALIGNMENT,
                ..Default::default()
            });
        }
        self.emit_changed();
    }

    /// Cycle bullet type: None -> Circle -> Square -> Disk -> None.
    pub fn toggle_bullet(&self, offset: i32, length: i32) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();
            let cur = inner.get_para_attr(offset, length);
            let new_bullet = cur.bullet.next();
            inner.para_attrs.push(ParaAttr {
                offset,
                length,
                bullet: new_bullet,
                mask: PARA_ATTR_BULLET,
                ..Default::default()
            });
        }
        self.emit_changed();
    }

    /// Adjust indent by `delta_pixels` (clamped to >= 0).
    pub fn indent(&self, offset: i32, length: i32, delta_pixels: i32) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();
            let cur = inner.get_para_attr(offset, length);
            let new_indent = (cur.indent + delta_pixels).max(0);
            inner.para_attrs.push(ParaAttr {
                offset,
                length,
                indent: new_indent,
                mask: PARA_ATTR_INDENT,
                ..Default::default()
            });
        }
        self.emit_changed();
    }

    /// Set quote level for a character range.
    pub fn set_quote_level(&self, offset: i32, length: i32, level: i32) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();
            inner.para_attrs.push(ParaAttr {
                offset,
                length,
                quote_level: level,
                mask: PARA_ATTR_QUOTE_LEVEL,
                ..Default::default()
            });
        }
        self.emit_changed();
    }

    /// Insert a horizontal rule at `offset` (inserts a newline).
    pub fn insert_hr(&self, offset: i32) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();

            let byte_offset = char_offset_to_byte(&inner.text, offset);
            inner.text.insert_str(byte_offset, "\n");

            inner.para_attrs.push(ParaAttr {
                offset,
                length: 1,
                is_hr: true,
                mask: PARA_ATTR_HR,
                ..Default::default()
            });

            for run in &mut inner.style_runs {
                if run.offset >= offset {
                    run.offset += 1;
                }
            }
        }
        self.emit_changed();
    }

    /// Set tab stops for a paragraph range.
    pub fn set_tab_stops(&self, offset: i32, length: i32, tab_stops: Vec<TabStop>) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();
            inner.para_attrs.push(ParaAttr {
                offset,
                length,
                tab_stops,
                mask: PARA_ATTR_TAB_STOPS,
                ..Default::default()
            });
        }
        self.emit_changed();
    }

    /// Remove paragraph attributes that overlap a range and adjust offsets.
    pub fn remove_para_attrs_in_range(&self, offset: i32, length: i32) {
        if length <= 0 {
            return;
        }
        let mut inner = self.inner.borrow_mut();
        let del_end = offset + length;

        inner.para_attrs.retain_mut(|p| {
            let a = p.offset;
            let b = p.offset + p.length;
            if b <= offset || a >= del_end {
                if a >= del_end {
                    p.offset -= length;
                }
                true
            } else {
                false
            }
        });
    }

    /// Set text direction for a paragraph range.
    pub fn set_direction(&self, offset: i32, length: i32, direction: Direction) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();
            inner.para_attrs.push(ParaAttr {
                offset,
                length,
                direction,
                mask: PARA_ATTR_DIRECTION,
                ..Default::default()
            });
        }
        self.emit_changed();
    }

    /// Insert a page break (form feed) at `offset`.
    pub fn insert_page_break(&self, offset: i32) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();

            let byte_offset = char_offset_to_byte(&inner.text, offset);
            inner.text.insert_str(byte_offset, "\x0C");

            inner.para_attrs.push(ParaAttr {
                offset,
                length: 1,
                page_break: true,
                mask: PARA_ATTR_PAGE_BREAK,
                ..Default::default()
            });

            for run in &mut inner.style_runs {
                if run.offset >= offset {
                    run.offset += 1;
                }
            }
        }
        self.emit_changed();
    }

    /// Get merged paragraph attributes for a character range.
    pub fn get_para_attr(&self, offset: i32, length: i32) -> ParaAttr {
        self.inner.borrow().get_para_attr(offset, length)
    }

    // -----------------------------------------------------------------------
    // Undo / Redo
    // -----------------------------------------------------------------------

    /// Undo the last operation. No-op if undo stack is empty.
    pub fn undo(&self) {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.undo_stack.is_empty() {
                return;
            }
            let cur = inner.take_snapshot();
            let prev = inner.undo_stack.pop().unwrap();
            inner.restore_snapshot(&prev);
            inner.redo_stack.push(cur);
        }
        self.emit_changed();
    }

    /// Redo the last undone operation. No-op if redo stack is empty.
    pub fn redo(&self) {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.redo_stack.is_empty() {
                return;
            }
            let cur = inner.take_snapshot();
            let next = inner.redo_stack.pop().unwrap();
            inner.restore_snapshot(&next);
            inner.undo_stack.push(cur);
        }
        self.emit_changed();
    }

    /// Paste formatted text with styles as a single atomic undo operation.
    pub fn paste_formatted(&self, offset: i32, text: &str, style_runs: &[StyleRun]) {
        if text.is_empty() {
            return;
        }
        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();

            let byte_offset = char_offset_to_byte(&inner.text, offset);
            inner.text.insert_str(byte_offset, text);

            let inserted_chars = text.chars().count() as i32;
            for run in &mut inner.style_runs {
                if run.offset >= offset {
                    run.offset += inserted_chars;
                }
            }

            for src_run in style_runs {
                let mut new_run = src_run.clone();
                new_run.offset = offset + src_run.offset;
                inner.style_runs.push(new_run);
            }
        }
        self.emit_changed();
    }

    /// Copy a range of text+style runs from `self` to `dst` at `dst_offset`.
    pub fn copy_range(
        &self,
        src_offset: i32,
        src_length: i32,
        dst: &Document,
        dst_offset: i32,
    ) -> Result<(), ()> {
        let (text, runs) = {
            let inner = self.inner.borrow();
            let text = inner.get_text_range(src_offset, src_length);

            let mut runs = Vec::new();
            let copy_start = src_offset;
            let copy_end = src_offset + src_length;
            for r in &inner.style_runs {
                let run_start = r.offset;
                let run_end = r.offset + r.length;
                if run_end <= copy_start || run_start >= copy_end {
                    continue;
                }
                let off = run_start.max(copy_start) - copy_start;
                let len = run_end.min(copy_end) - run_start.max(copy_start);
                let mut nr = r.clone();
                nr.offset = off;
                nr.length = len;
                runs.push(nr);
            }
            (text, runs)
        };

        dst.paste_formatted(dst_offset, &text, &runs);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Attr list generation
    // -----------------------------------------------------------------------

    /// Build a `PangoAttrList` representing all style runs.
    pub fn get_attr_list(&self) -> pango::AttrList {
        self.inner.borrow().get_attr_list()
    }

    /// Build a `PangoAttrList` for a specific character range (byte-relative indices).
    /// Set spell error ranges. Each entry is (char_offset, char_length).
    /// Renders as wavy red underline (Pango Underline::Error).
    /// Only triggers a redraw if errors actually changed.
    pub fn set_spell_errors(&self, errors: Vec<(i32, i32)>) {
        let mut inner = self.inner.borrow_mut();
        if inner.spell_errors == errors { return; } // no change, skip redraw
        inner.spell_errors = errors;
        drop(inner);
        self.emit_changed();
    }

    /// Clear all spell error marks.
    pub fn clear_spell_errors(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.spell_errors.is_empty() { return; }
        inner.spell_errors.clear();
        drop(inner);
        self.emit_changed();
    }

    pub fn get_attr_list_for_range(
        &self,
        range_offset: i32,
        range_length: i32,
    ) -> pango::AttrList {
        self.inner
            .borrow()
            .get_attr_list_for_range(range_offset, range_length)
    }

    // -----------------------------------------------------------------------
    // Navigation helpers
    // -----------------------------------------------------------------------

    pub fn find_word_boundary_left(&self, offset: i32) -> i32 {
        self.inner.borrow().find_word_boundary_left(offset)
    }

    pub fn find_word_boundary_right(&self, offset: i32) -> i32 {
        self.inner.borrow().find_word_boundary_right(offset)
    }

    pub fn find_line_start(&self, offset: i32) -> i32 {
        self.inner.borrow().find_line_start(offset)
    }

    pub fn find_line_end(&self, offset: i32) -> i32 {
        self.inner.borrow().find_line_end(offset)
    }

    pub fn get_word_bounds(&self, offset: i32) -> (i32, i32) {
        self.inner.borrow().get_word_bounds(offset)
    }

    pub fn is_word_boundary_char(&self, offset: i32) -> bool {
        self.inner.borrow().is_word_boundary_char(offset)
    }

    // -----------------------------------------------------------------------
    // Find / Replace
    // -----------------------------------------------------------------------

    /// Find text in document. Returns char offset or -1 if not found.
    pub fn find_text(
        &self,
        search_text: &str,
        start_offset: i32,
        end_offset: i32,
        case_sensitive: bool,
    ) -> i32 {
        self.inner
            .borrow()
            .find_text(search_text, start_offset, end_offset, case_sensitive)
    }

    /// Replace text in document. Returns number of replacements made.
    pub fn replace_text(
        &self,
        search_text: &str,
        replace_text: &str,
        start_offset: i32,
        end_offset: i32,
        replace_all: bool,
        case_sensitive: bool,
    ) -> i32 {
        if search_text.is_empty() {
            return 0;
        }
        let search_len = search_text.chars().count() as i32;
        let replace_len = replace_text.chars().count() as i32;

        let mut replacements = 0i32;
        let mut current_offset = start_offset;
        let mut end_offset = end_offset;

        {
            let mut inner = self.inner.borrow_mut();
            inner.push_undo();

            loop {
                let found =
                    inner.find_text(search_text, current_offset, end_offset, case_sensitive);
                if found < 0 {
                    break;
                }

                inner.delete_range(found, search_len);
                if !replace_text.is_empty() {
                    inner.insert_text(found, replace_text);
                }

                replacements += 1;
                current_offset = found + replace_len;

                if !replace_all {
                    break;
                }
                end_offset += replace_len - search_len;
            }
        }

        if replacements > 0 {
            self.emit_changed();
        }
        replacements
    }

    /// Insert a small subset of HTML-like markup into the document at `offset`.
    /// Supported tags: `<b>`, `<i>`, `<u>`, `<color=#rrggbb>`, `<hr/>`
    pub fn insert_markup(&self, offset: i32, markup: &str) {
        let mut plain = String::new();
        let mut style_stack: Vec<(&str, usize)> = Vec::new();
        let mut runs: Vec<StyleRun> = Vec::new();
        let mut i = 0;
        let chars: Vec<char> = markup.chars().collect();
        let len = chars.len();

        while i < len {
            if chars[i] == '<' {
                i += 1;
                let mut tag = String::new();
                while i < len && chars[i] != '>' {
                    tag.push(chars[i]);
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
                let tag_lower = tag.to_lowercase();

                if tag_lower == "b" {
                    style_stack.push(("b", plain.chars().count()));
                } else if tag_lower == "/b" {
                    if let Some(pos) = style_stack.iter().rposition(|(t, _)| *t == "b") {
                        let (_, start) = style_stack.remove(pos);
                        let end = plain.chars().count();
                        if end > start {
                            runs.push(StyleRun {
                                offset: start as i32,
                                length: (end - start) as i32,
                                bold: true,
                                ..Default::default()
                            });
                        }
                    }
                } else if tag_lower == "i" {
                    style_stack.push(("i", plain.chars().count()));
                } else if tag_lower == "/i" {
                    if let Some(pos) = style_stack.iter().rposition(|(t, _)| *t == "i") {
                        let (_, start) = style_stack.remove(pos);
                        let end = plain.chars().count();
                        if end > start {
                            runs.push(StyleRun {
                                offset: start as i32,
                                length: (end - start) as i32,
                                italic: true,
                                ..Default::default()
                            });
                        }
                    }
                } else if tag_lower == "u" {
                    style_stack.push(("u", plain.chars().count()));
                } else if tag_lower == "/u" {
                    if let Some(pos) = style_stack.iter().rposition(|(t, _)| *t == "u") {
                        let (_, start) = style_stack.remove(pos);
                        let end = plain.chars().count();
                        if end > start {
                            runs.push(StyleRun {
                                offset: start as i32,
                                length: (end - start) as i32,
                                underline: true,
                                ..Default::default()
                            });
                        }
                    }
                } else if tag_lower.starts_with("color=") {
                    style_stack.push(("color", plain.chars().count()));
                } else if tag_lower == "/color" {
                    if let Some(pos) = style_stack.iter().rposition(|(t, _)| *t == "color") {
                        let (_, start) = style_stack.remove(pos);
                        let end = plain.chars().count();
                        if end > start {
                            runs.push(StyleRun {
                                offset: start as i32,
                                length: (end - start) as i32,
                                color: RGBA::new(1.0, 0.0, 0.0, 1.0),
                                ..Default::default()
                            });
                        }
                    }
                } else if tag_lower == "hr/" || tag_lower == "hr" {
                    plain.push('\n');
                }
            } else {
                plain.push(chars[i]);
                i += 1;
            }
        }

        self.paste_formatted(offset, &plain, &runs);
    }

    /// Set the pixel dimensions of the graphic at the given character offset.
    pub fn set_graphic_size(&self, graphic_offset: i32, width: i32, height: i32) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.set_graphic_size(graphic_offset, width, height);
        }
        self.emit_changed();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // === Basic text operations =============================================

    #[test]
    fn test_new_document_is_empty() {
        let doc = Document::new();
        assert_eq!(doc.get_text(), "");
        assert_eq!(doc.get_length(), 0);
    }

    #[test]
    fn test_insert_text() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        assert_eq!(doc.get_text(), "Hello");
        assert_eq!(doc.get_length(), 5);
    }

    #[test]
    fn test_insert_text_at_offset() {
        let doc = Document::new();
        doc.insert_text(0, "Hllo");
        doc.insert_text(1, "e");
        assert_eq!(doc.get_text(), "Hello");
    }

    #[test]
    fn test_insert_text_line_ending_normalization() {
        let doc = Document::new();
        doc.insert_text(0, "line1\r\nline2\rline3");
        assert_eq!(doc.get_text(), "line1\nline2\nline3");
    }

    #[test]
    fn test_delete_range() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.delete_range(5, 6);
        assert_eq!(doc.get_text(), "Hello");
    }

    #[test]
    fn test_get_text_range() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        assert_eq!(doc.get_text_range(0, 5), "Hello");
        assert_eq!(doc.get_text_range(6, 5), "World");
    }

    #[test]
    fn test_get_text_range_empty() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        assert_eq!(doc.get_text_range(0, 0), "");
    }

    // === Style run management ==============================================

    #[test]
    fn test_add_style_run_bold() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.add_style_run(0, 5, Some(true), None, None, None, None);

        let style = doc.get_style_at(2);
        assert!(style.bold);
        assert!(!style.italic);

        let style2 = doc.get_style_at(7);
        assert!(!style2.bold);
    }

    #[test]
    fn test_add_style_run_multiple() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.add_style_run(0, 5, Some(true), None, None, None, None);
        doc.add_style_run(0, 5, None, Some(true), None, None, None);

        let style = doc.get_style_at(2);
        assert!(style.bold);
        assert!(style.italic);
    }

    #[test]
    fn test_add_style_run_with_color() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        let red = RGBA::new(1.0, 0.0, 0.0, 1.0);
        doc.add_style_run(0, 5, None, None, None, Some(red), None);

        let style = doc.get_style_at(2);
        assert!((style.color.red() - 1.0).abs() < f32::EPSILON);
        assert!((style.color.green() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_add_style_run_font_size() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.add_style_run(0, 5, None, None, None, None, Some(24));

        let style = doc.get_style_at(2);
        assert_eq!(style.font_size, 24);
    }

    #[test]
    fn test_toggle_style_on() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.toggle_style(0, 5, true, false, false);

        assert!(doc.get_style_at(2).bold);
    }

    #[test]
    fn test_toggle_style_off() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.add_style_run(0, 5, Some(true), None, None, None, None);
        doc.toggle_style(0, 5, true, false, false);

        assert!(!doc.get_style_at(2).bold);
    }

    #[test]
    fn test_style_runs_adjusted_on_insert() {
        let doc = Document::new();
        doc.insert_text(0, "HelloWorld");
        doc.add_style_run(5, 5, Some(true), None, None, None, None);

        doc.insert_text(0, "XX");

        assert!(doc.get_style_at(7).bold);
    }

    #[test]
    fn test_style_runs_removed_on_delete() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.add_style_run(6, 5, Some(true), None, None, None, None);

        doc.delete_range(6, 5);

        let runs = doc.get_style_runs();
        assert!(runs.is_empty() || runs.iter().all(|r| r.offset + r.length <= 6));
    }

    #[test]
    fn test_get_style_runs() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.add_style_run(0, 5, Some(true), None, None, None, None);

        let runs = doc.get_style_runs();
        assert!(!runs.is_empty());
        assert!(runs.iter().any(|r| r.bold && r.offset == 0));
    }

    // === Font family and links =============================================

    #[test]
    fn test_set_font_family() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.add_style_run(0, 5, Some(false), None, None, None, None);
        doc.set_font_family(0, 5, Some("Courier"));

        let runs = doc.get_style_runs();
        assert!(runs
            .iter()
            .any(|r| r.font_family.as_deref() == Some("Courier")));
    }

    #[test]
    fn test_set_link() {
        let doc = Document::new();
        doc.insert_text(0, "Click here");
        doc.add_style_run(0, 10, Some(false), None, None, None, None);
        doc.set_link(0, 10, Some("https://example.com"));

        assert_eq!(doc.get_link_at(5), Some("https://example.com".to_string()));
    }

    #[test]
    fn test_get_link_at_no_link() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        assert_eq!(doc.get_link_at(2), None);
    }

    // === Paragraph attributes ==============================================

    #[test]
    fn test_set_alignment() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.set_alignment(0, 11, Alignment::Center);

        assert_eq!(doc.get_para_attr(0, 11).alignment, Alignment::Center);
    }

    #[test]
    fn test_toggle_bullet() {
        let doc = Document::new();
        doc.insert_text(0, "Item 1");

        doc.toggle_bullet(0, 6);
        assert_eq!(doc.get_para_attr(0, 6).bullet, BulletType::Circle);

        doc.toggle_bullet(0, 6);
        assert_eq!(doc.get_para_attr(0, 6).bullet, BulletType::Square);

        doc.toggle_bullet(0, 6);
        assert_eq!(doc.get_para_attr(0, 6).bullet, BulletType::Disk);

        doc.toggle_bullet(0, 6);
        assert_eq!(doc.get_para_attr(0, 6).bullet, BulletType::None);
    }

    #[test]
    fn test_indent() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");

        doc.indent(0, 5, 20);
        assert_eq!(doc.get_para_attr(0, 5).indent, 20);

        doc.indent(0, 5, 10);
        assert_eq!(doc.get_para_attr(0, 5).indent, 30);

        doc.indent(0, 5, -50);
        assert_eq!(doc.get_para_attr(0, 5).indent, 0);
    }

    #[test]
    fn test_set_quote_level() {
        let doc = Document::new();
        doc.insert_text(0, "Quoted text");
        doc.set_quote_level(0, 11, 2);

        assert_eq!(doc.get_para_attr(0, 11).quote_level, 2);
    }

    #[test]
    fn test_insert_hr() {
        let doc = Document::new();
        doc.insert_text(0, "Before\nAfter");
        let len_before = doc.get_length();
        doc.insert_hr(7);

        assert_eq!(doc.get_length(), len_before + 1);
        assert!(doc.get_para_attr(7, 1).is_hr);
    }

    #[test]
    fn test_set_direction() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.set_direction(0, 5, Direction::Rtl);

        assert_eq!(doc.get_para_attr(0, 5).direction, Direction::Rtl);
    }

    #[test]
    fn test_insert_page_break() {
        let doc = Document::new();
        doc.insert_text(0, "Page1");
        let len_before = doc.get_length();
        doc.insert_page_break(5);

        assert_eq!(doc.get_length(), len_before + 1);
        assert!(doc.get_para_attr(5, 1).page_break);
    }

    #[test]
    fn test_set_tab_stops() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        let tabs = vec![TabStop { position: 100 }, TabStop { position: 200 }];
        doc.set_tab_stops(0, 5, tabs);

        let attr = doc.get_para_attr(0, 5);
        assert_eq!(attr.tab_stops.len(), 2);
        assert_eq!(attr.tab_stops[0].position, 100);
        assert_eq!(attr.tab_stops[1].position, 200);
    }

    #[test]
    fn test_remove_para_attrs_in_range() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.set_alignment(0, 11, Alignment::Center);

        doc.remove_para_attrs_in_range(0, 11);

        assert_eq!(doc.get_para_attr(0, 11).alignment, Alignment::Left);
    }

    // === Undo / Redo =======================================================

    #[test]
    fn test_undo_insert() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.undo();
        assert_eq!(doc.get_text(), "");
    }

    #[test]
    fn test_redo_insert() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.undo();
        doc.redo();
        assert_eq!(doc.get_text(), "Hello");
    }

    #[test]
    fn test_undo_delete() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.delete_range(5, 6);
        assert_eq!(doc.get_text(), "Hello");

        doc.undo();
        assert_eq!(doc.get_text(), "Hello World");
    }

    #[test]
    fn test_undo_style_run() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.add_style_run(0, 5, Some(true), None, None, None, None);
        assert!(doc.get_style_at(2).bold);

        doc.undo();
        assert!(!doc.get_style_at(2).bold);
    }

    #[test]
    fn test_undo_multiple() {
        let doc = Document::new();
        doc.insert_text(0, "A");
        doc.insert_text(1, "B");
        doc.insert_text(2, "C");
        assert_eq!(doc.get_text(), "ABC");

        doc.undo();
        assert_eq!(doc.get_text(), "AB");
        doc.undo();
        assert_eq!(doc.get_text(), "A");
        doc.undo();
        assert_eq!(doc.get_text(), "");
    }

    #[test]
    fn test_new_edit_clears_redo_stack() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.undo();
        doc.insert_text(0, "World");
        doc.redo(); // should be no-op
        assert_eq!(doc.get_text(), "World");
    }

    #[test]
    fn test_undo_empty_stack() {
        let doc = Document::new();
        doc.undo();
        assert_eq!(doc.get_text(), "");
    }

    #[test]
    fn test_redo_empty_stack() {
        let doc = Document::new();
        doc.redo();
        assert_eq!(doc.get_text(), "");
    }

    // === Graphic insertion =================================================

    #[test]
    fn test_insert_graphic() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.insert_graphic(5, None, 100, 50);

        assert_eq!(doc.get_length(), 6);
        assert!(doc.get_text().contains('\u{FFFC}'));

        let style = doc.get_style_at(5);
        assert!(style.is_graphic);
        let gr = style.graphic.unwrap();
        assert_eq!(gr.width, 100);
        assert_eq!(gr.height, 50);
    }

    #[test]
    fn test_insert_graphic_shifts_runs() {
        let doc = Document::new();
        doc.insert_text(0, "HelloWorld");
        doc.add_style_run(5, 5, Some(true), None, None, None, None);

        doc.insert_graphic(5, None, 100, 50);

        assert!(doc.get_style_at(6).bold);
    }

    #[test]
    fn test_undo_graphic() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.insert_graphic(5, None, 100, 50);
        assert_eq!(doc.get_length(), 6);

        doc.undo();
        assert_eq!(doc.get_length(), 5);
        assert_eq!(doc.get_text(), "Hello");
    }

    // === Paste formatted ===================================================

    #[test]
    fn test_paste_formatted() {
        let doc = Document::new();
        doc.insert_text(0, "Hello ");

        let runs = vec![StyleRun {
            offset: 0,
            length: 5,
            bold: true,
            ..Default::default()
        }];
        doc.paste_formatted(6, "World", &runs);

        assert_eq!(doc.get_text(), "Hello World");
        assert!(doc.get_style_at(6).bold);
    }

    #[test]
    fn test_copy_range() {
        let src = Document::new();
        src.insert_text(0, "Hello World");
        src.add_style_run(0, 5, Some(true), None, None, None, None);

        let dst = Document::new();
        src.copy_range(0, 5, &dst, 0).unwrap();

        assert_eq!(dst.get_text(), "Hello");
        assert!(dst.get_style_at(2).bold);
    }

    // === Navigation helpers ================================================

    #[test]
    fn test_find_word_boundary_left() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        assert_eq!(doc.find_word_boundary_left(8), 6);
        assert_eq!(doc.find_word_boundary_left(0), 0);
    }

    #[test]
    fn test_find_word_boundary_right() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        assert_eq!(doc.find_word_boundary_right(0), 6);
        assert_eq!(doc.find_word_boundary_right(6), 11);
    }

    #[test]
    fn test_find_line_start() {
        let doc = Document::new();
        doc.insert_text(0, "Line1\nLine2\nLine3");
        assert_eq!(doc.find_line_start(8), 6);
        assert_eq!(doc.find_line_start(0), 0);
    }

    #[test]
    fn test_find_line_end() {
        let doc = Document::new();
        doc.insert_text(0, "Line1\nLine2\nLine3");
        assert_eq!(doc.find_line_end(0), 5);
        assert_eq!(doc.find_line_end(6), 11);
    }

    #[test]
    fn test_get_word_bounds() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        assert_eq!(doc.get_word_bounds(3), (0, 5));
        assert_eq!(doc.get_word_bounds(8), (6, 11));
    }

    #[test]
    fn test_is_word_boundary_char() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        assert!(!doc.is_word_boundary_char(0));
        assert!(doc.is_word_boundary_char(5));
    }

    // === Find / Replace ====================================================

    #[test]
    fn test_find_text_case_sensitive() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World Hello");
        assert_eq!(doc.find_text("Hello", 0, -1, true), 0);
        assert_eq!(doc.find_text("Hello", 1, -1, true), 12);
        assert_eq!(doc.find_text("hello", 0, -1, true), -1);
    }

    #[test]
    fn test_find_text_case_insensitive() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        assert_eq!(doc.find_text("hello", 0, -1, false), 0);
        assert_eq!(doc.find_text("WORLD", 0, -1, false), 6);
    }

    #[test]
    fn test_replace_text_single() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        let count = doc.replace_text("World", "Rust", 0, -1, false, true);
        assert_eq!(count, 1);
        assert_eq!(doc.get_text(), "Hello Rust");
    }

    #[test]
    fn test_replace_text_all() {
        let doc = Document::new();
        doc.insert_text(0, "aaa bbb aaa");
        let count = doc.replace_text("aaa", "xxx", 0, -1, true, true);
        assert_eq!(count, 2);
        assert_eq!(doc.get_text(), "xxx bbb xxx");
    }

    // === Attr list generation ==============================================

    #[test]
    fn test_get_attr_list_empty() {
        let doc = Document::new();
        let _list = doc.get_attr_list();
    }

    #[test]
    fn test_get_attr_list_with_styles() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.add_style_run(0, 5, Some(true), Some(true), None, None, None);

        let list = doc.get_attr_list();
        let _iter = list.iterator();
    }

    #[test]
    fn test_get_attr_list_for_range() {
        let doc = Document::new();
        doc.insert_text(0, "Hello World");
        doc.add_style_run(0, 5, Some(true), None, None, None, None);

        let _list = doc.get_attr_list_for_range(0, 5);
    }

    // === Change notification ===============================================

    #[test]
    fn test_document_changed_callback() {
        let doc = Document::new();
        let changed = std::rc::Rc::new(std::cell::Cell::new(false));
        let changed_clone = changed.clone();
        doc.connect_changed(move || {
            changed_clone.set(true);
        });

        doc.insert_text(0, "Hello");
        assert!(changed.get());
    }

    #[test]
    fn test_selection_changed_callback_exists() {
        let doc = Document::new();
        doc.connect_selection_changed(|| {});
    }

    // === Insert markup =====================================================

    #[test]
    fn test_insert_markup_bold() {
        let doc = Document::new();
        doc.insert_markup(0, "<b>Hello</b> World");
        assert_eq!(doc.get_text(), "Hello World");
        assert!(doc.get_style_at(2).bold);
    }

    #[test]
    fn test_insert_markup_nested() {
        let doc = Document::new();
        doc.insert_markup(0, "<b><i>Bold Italic</i></b>");
        assert_eq!(doc.get_text(), "Bold Italic");
        assert!(doc.get_style_at(2).bold);
        assert!(doc.get_style_at(2).italic);
    }

    // === Edge cases ========================================================

    #[test]
    fn test_delete_range_zero_length() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.delete_range(0, 0);
        assert_eq!(doc.get_text(), "Hello");
    }

    #[test]
    fn test_toggle_style_zero_length() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.toggle_style(0, 0, true, false, false);
        assert!(!doc.get_style_at(0).bold);
    }

    #[test]
    fn test_unicode_text() {
        let doc = Document::new();
        doc.insert_text(0, "Hej v\u{00e4}rlden \u{1f600}");
        let text = doc.get_text();
        assert!(text.contains('\u{00e4}'));
        assert!(text.contains('\u{1f600}'));
    }

    #[test]
    fn test_style_run_on_unicode() {
        let doc = Document::new();
        doc.insert_text(0, "caf\u{00e9} latte");
        doc.add_style_run(0, 4, Some(true), None, None, None, None);

        assert!(doc.get_style_at(3).bold);
        assert!(!doc.get_style_at(5).bold);
    }

    #[test]
    fn test_undo_redo_para_attrs() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.set_alignment(0, 5, Alignment::Center);
        assert_eq!(doc.get_para_attr(0, 5).alignment, Alignment::Center);

        doc.undo();
        assert_eq!(doc.get_para_attr(0, 5).alignment, Alignment::Left);

        doc.redo();
        assert_eq!(doc.get_para_attr(0, 5).alignment, Alignment::Center);
    }

    #[test]
    fn test_set_graphic_size() {
        let doc = Document::new();
        doc.insert_text(0, "Hello");
        doc.insert_graphic(5, None, 100, 50);

        doc.set_graphic_size(5, 200, 100);

        let style = doc.get_style_at(5);
        assert!(style.is_graphic);
        let gr = style.graphic.unwrap();
        assert_eq!(gr.width, 200);
        assert_eq!(gr.height, 100);
    }

    #[test]
    fn test_format_eq() {
        let a = StyleRun {
            offset: 0,
            length: 5,
            bold: true,
            italic: false,
            underline: false,
            color: RGBA::new(1.0, 0.0, 0.0, 1.0),
            font_size: 12,
            font_family: None,
            is_graphic: false,
            graphic: None,
            link_url: None,
        };
        let b = StyleRun {
            offset: 10,
            length: 3,
            ..a.clone()
        };
        assert!(a.format_eq(&b));

        let c = StyleRun {
            bold: false,
            ..a.clone()
        };
        assert!(!a.format_eq(&c));
    }
}
