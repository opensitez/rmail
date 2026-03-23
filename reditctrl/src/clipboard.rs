//! Clipboard — cut, copy, paste with styled text preservation.
//!
//! Port of gEditCtrl/gedit-clipboard.{c,h}.
//!
//! Internal clipboard content is serialized as JSON so that styled text
//! round-trips through the system clipboard. External (non-JSON) pastes
//! are treated as plain text. Intelligent spacing is added around pasted
//! content following the original Eudora behavior.

use crate::document::{DocumentOps, Graphic, StyleRun, RGBA};
use crate::state::EditorState;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Serialization types (JSON representation on the clipboard)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ColorJson {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct StyleRunJson {
    offset: i32,
    length: i32,
    bold: bool,
    italic: bool,
    underline: bool,
    font_size: i32,
    #[serde(default)]
    is_graphic: bool,
    #[serde(default)]
    graphic_width: Option<i32>,
    #[serde(default)]
    graphic_height: Option<i32>,
    #[serde(default)]
    color: Option<ColorJson>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ClipboardContent {
    text: String,
    styles: Vec<StyleRunJson>,
}

// ---------------------------------------------------------------------------
// Style run merging (from Carbon convert.c pattern)
// ---------------------------------------------------------------------------

/// Collapse consecutive style runs with identical formatting.
fn merge_style_runs(runs: &[StyleRun]) -> Vec<StyleRun> {
    if runs.is_empty() {
        return Vec::new();
    }
    let mut merged: Vec<StyleRun> = Vec::new();
    for run in runs {
        if let Some(last) = merged.last_mut() {
            if last.format_eq(run) {
                last.length += run.length;
                continue;
            }
        }
        merged.push(run.clone());
    }
    merged
}

// ---------------------------------------------------------------------------
// Serialization / deserialization
// ---------------------------------------------------------------------------

/// Serialize a range of the document (text + style runs) to JSON.
pub fn serialize_content(doc: &dyn DocumentOps, offset: i32, length: i32) -> String {
    let full_text = match doc.get_text_opt() {
        Some(t) => t,
        None => return String::new(),
    };

    let chars: Vec<char> = full_text.chars().collect();
    let start = offset as usize;
    let end = (offset + length) as usize;
    let text_range: String = chars[start..end.min(chars.len())].iter().collect();

    let all_runs = doc.get_style_runs();
    let sel_end = offset + length;
    let mut overlapping: Vec<StyleRun> = Vec::new();

    for run in &all_runs {
        let run_end = run.offset + run.length;
        if run_end <= offset || run.offset >= sel_end {
            continue;
        }
        let overlap_start = run.offset.max(offset);
        let overlap_end = run_end.min(sel_end);
        let overlap_len = overlap_end - overlap_start;
        let rel_offset = overlap_start - offset;

        let mut overlap_run = run.clone();
        overlap_run.offset = rel_offset;
        overlap_run.length = overlap_len;
        overlapping.push(overlap_run);
    }

    let merged = merge_style_runs(&overlapping);

    let styles_json: Vec<StyleRunJson> = merged
        .iter()
        .map(|run| {
            if run.is_graphic {
                let (gw, gh) = run
                    .graphic
                    .as_ref()
                    .map(|g| (g.width, g.height))
                    .unwrap_or((0, 0));
                StyleRunJson {
                    offset: run.offset, length: run.length,
                    bold: run.bold, italic: run.italic, underline: run.underline,
                    font_size: run.font_size, is_graphic: true,
                    graphic_width: Some(gw), graphic_height: Some(gh), color: None,
                }
            } else {
                StyleRunJson {
                    offset: run.offset, length: run.length,
                    bold: run.bold, italic: run.italic, underline: run.underline,
                    font_size: run.font_size, is_graphic: false,
                    graphic_width: None, graphic_height: None,
                    color: Some(ColorJson {
                        r: run.color.red() as f64, g: run.color.green() as f64,
                        b: run.color.blue() as f64, a: run.color.alpha() as f64,
                    }),
                }
            }
        })
        .collect();

    serde_json::to_string(&ClipboardContent { text: text_range, styles: styles_json })
        .unwrap_or_default()
}

/// Deserialize JSON clipboard content. Returns `(text, style_runs)` on success.
pub fn deserialize_content(json_str: &str) -> Option<(String, Vec<StyleRun>)> {
    let content: ClipboardContent = serde_json::from_str(json_str).ok()?;

    let styles: Vec<StyleRun> = content
        .styles
        .iter()
        .map(|sj| {
            let color = sj.color.as_ref()
                .map(|c| RGBA::new(c.r as f32, c.g as f32, c.b as f32, c.a as f32))
                .unwrap_or_else(|| RGBA::new(0.0, 0.0, 0.0, 1.0));
            let graphic = if sj.is_graphic {
                Some(Graphic::new(sj.offset, sj.graphic_width.unwrap_or(0),
                                  sj.graphic_height.unwrap_or(0), None))
            } else {
                None
            };
            StyleRun {
                offset: sj.offset, length: sj.length,
                bold: sj.bold, italic: sj.italic, underline: sj.underline,
                color, font_size: sj.font_size, font_family: None,
                is_graphic: sj.is_graphic, graphic, link_url: None,
            }
        })
        .collect();

    Some((content.text, styles))
}

// ---------------------------------------------------------------------------
// Intelligent paste spacing
// ---------------------------------------------------------------------------

/// Result of intelligent spacing analysis.
pub struct IntelligentSpaceResult {
    pub text: String,
    pub added_before: i32,
    pub added_after: i32,
}

/// Add intelligent spacing around pasted text following original Eudora behavior.
pub fn add_intelligent_paste_space(
    doc: &dyn DocumentOps,
    caret: i32,
    text: &str,
) -> IntelligentSpaceResult {
    if text.is_empty() {
        return IntelligentSpaceResult { text: text.to_string(), added_before: 0, added_after: 0 };
    }

    let full_text = match doc.get_text_opt() {
        Some(t) => t,
        None => return IntelligentSpaceResult { text: text.to_string(), added_before: 0, added_after: 0 },
    };

    let doc_chars: Vec<char> = full_text.chars().collect();
    let doc_len = doc_chars.len() as i32;
    let paste_chars: Vec<char> = text.chars().collect();

    let mut add_before = false;
    let mut add_after = false;

    if caret > 0 && caret < doc_len {
        let cb = doc_chars[(caret - 1) as usize];
        let fc = paste_chars[0];
        if !cb.is_whitespace() && !cb.is_ascii_punctuation()
            && !fc.is_whitespace() && !fc.is_ascii_punctuation()
        {
            add_before = true;
        }
    }

    if caret < doc_len && !paste_chars.is_empty() {
        let ca = doc_chars[caret as usize];
        let lc = *paste_chars.last().unwrap();
        if !ca.is_whitespace() && !ca.is_ascii_punctuation()
            && !lc.is_whitespace() && !lc.is_ascii_punctuation()
        {
            add_after = true;
        }
    }

    let mut result = String::new();
    let added_before = if add_before { result.push(' '); 1 } else { 0 };
    result.push_str(text);
    let added_after = if add_after { result.push(' '); 1 } else { 0 };

    IntelligentSpaceResult { text: result, added_before, added_after }
}

// ---------------------------------------------------------------------------
// Public clipboard operations
// ---------------------------------------------------------------------------

/// Copy the selected range to the clipboard as JSON.
pub fn clipboard_copy(doc: &dyn DocumentOps, sel_start: i32, sel_end: i32) -> Option<String> {
    if sel_start == sel_end { return None; }
    let offset = sel_start.min(sel_end);
    let length = (sel_end - sel_start).abs();
    Some(serialize_content(doc, offset, length))
}

/// Cut the selected range: copy then delete.
pub fn clipboard_cut(doc: &dyn DocumentOps, sel_start: i32, sel_end: i32) -> Option<String> {
    if sel_start == sel_end { return None; }
    let json = clipboard_copy(doc, sel_start, sel_end);
    let offset = sel_start.min(sel_end);
    let length = (sel_end - sel_start).abs();
    doc.delete_range(offset, length);
    json
}

/// Paste clipboard content at caret position. Updates editor state.
pub fn clipboard_paste(
    doc: &dyn DocumentOps,
    state: &mut EditorState,
    clipboard_text: &str,
    plain_text_only: bool,
) {
    if clipboard_text.is_empty() { return; }
    let caret = state.caret;

    if plain_text_only {
        let sr = add_intelligent_paste_space(doc, caret, clipboard_text);
        let inserted = sr.text.chars().count() as i32;
        doc.paste_formatted(caret, &sr.text, &[]);
        update_state_after_paste(state, caret, inserted);
        return;
    }

    if let Some((plain, mut styles)) = deserialize_content(clipboard_text) {
        let sr = add_intelligent_paste_space(doc, caret, &plain);
        let inserted = sr.text.chars().count() as i32;
        if sr.added_before > 0 {
            for run in &mut styles { run.offset += sr.added_before; }
        }
        doc.paste_formatted(caret, &sr.text, &styles);
        update_state_after_paste(state, caret, inserted);
    } else {
        let sr = add_intelligent_paste_space(doc, caret, clipboard_text);
        let inserted = sr.text.chars().count() as i32;
        doc.paste_formatted(caret, &sr.text, &[]);
        update_state_after_paste(state, caret, inserted);
    }
}

/// Intelligent copy: if no selection, copy the word at caret.
pub fn clipboard_intelligent_copy(doc: &dyn DocumentOps, caret: i32) -> Option<String> {
    let start = doc.find_word_boundary_left(caret);
    let end = doc.find_word_boundary_right(caret);
    if start < end { clipboard_copy(doc, start, end) } else { None }
}

/// Intelligent cut: cut the word at caret, extending to include surrounding whitespace.
pub fn clipboard_intelligent_cut(doc: &dyn DocumentOps, caret: i32) -> Option<String> {
    let mut start = doc.find_word_boundary_left(caret);
    let mut end = doc.find_word_boundary_right(caret);
    if start >= end { return None; }

    if let Some(text) = doc.get_text_opt() {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len() as i32;
        if start > 0 && chars[(start - 1) as usize].is_whitespace() { start -= 1; }
        if end < len && chars[end as usize].is_whitespace() { end += 1; }
    }

    clipboard_cut(doc, start, end)
}

fn update_state_after_paste(state: &mut EditorState, caret: i32, inserted_len: i32) {
    state.caret = caret + inserted_len;
    state.sel_start = state.caret;
    state.sel_end = state.caret;
    state.sel_anchor = -1;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{DocumentOps, Graphic, StyleRun, RGBA};
    use std::cell::RefCell;

    /// Minimal in-memory document for testing.
    struct TestDoc {
        text: RefCell<String>,
        styles: Vec<StyleRun>,
    }

    impl TestDoc {
        fn new(text: &str) -> Self {
            Self { text: RefCell::new(text.to_string()), styles: Vec::new() }
        }
        fn with_styles(text: &str, styles: Vec<StyleRun>) -> Self {
            Self { text: RefCell::new(text.to_string()), styles }
        }
        fn text(&self) -> String { self.text.borrow().clone() }
    }

    impl DocumentOps for TestDoc {
        fn get_text_opt(&self) -> Option<String> {
            let t = self.text.borrow();
            if t.is_empty() { None } else { Some(t.clone()) }
        }
        fn get_length(&self) -> i32 { self.text.borrow().chars().count() as i32 }
        fn get_style_runs(&self) -> Vec<StyleRun> { self.styles.clone() }
        fn delete_range(&self, offset: i32, length: i32) {
            let mut t = self.text.borrow_mut();
            let chars: Vec<char> = t.chars().collect();
            let s = offset as usize;
            let e = (offset + length) as usize;
            *t = chars.iter().enumerate()
                .filter(|(i, _)| *i < s || *i >= e)
                .map(|(_, c)| c).collect();
        }
        fn paste_formatted(&self, offset: i32, text: &str, _styles: &[StyleRun]) {
            let mut t = self.text.borrow_mut();
            let chars: Vec<char> = t.chars().collect();
            let bo = chars[..offset as usize].iter().collect::<String>().len();
            t.insert_str(bo, text);
        }
        fn find_word_boundary_left(&self, offset: i32) -> i32 {
            let t = self.text.borrow();
            let chars: Vec<char> = t.chars().collect();
            let len = chars.len() as i32;
            if offset < 0 || offset > len { return offset; }
            let mut s = offset;
            while s > 0 && chars[(s - 1) as usize].is_alphanumeric() { s -= 1; }
            s
        }
        fn find_word_boundary_right(&self, offset: i32) -> i32 {
            let t = self.text.borrow();
            let chars: Vec<char> = t.chars().collect();
            let len = chars.len() as i32;
            if offset < 0 || offset >= len { return offset; }
            let mut e = offset;
            while e < len && chars[e as usize].is_alphanumeric() { e += 1; }
            e
        }
    }

    fn make_style(offset: i32, length: i32, bold: bool) -> StyleRun {
        StyleRun {
            offset, length, bold, italic: false, underline: false,
            color: RGBA::new(0.0, 0.0, 0.0, 1.0), font_size: 12,
            font_family: None, is_graphic: false, graphic: None, link_url: None,
        }
    }

    #[test]
    fn merge_empty() { assert!(merge_style_runs(&[]).is_empty()); }

    #[test]
    fn merge_identical_consecutive() {
        let runs = vec![make_style(0, 3, true), make_style(3, 5, true)];
        let merged = merge_style_runs(&runs);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].length, 8);
    }

    #[test]
    fn merge_different_not_merged() {
        assert_eq!(merge_style_runs(&[make_style(0, 3, true), make_style(3, 5, false)]).len(), 2);
    }

    #[test]
    fn serialize_deserialize_roundtrip() {
        let doc = TestDoc::with_styles("hello world", vec![make_style(0, 5, true), make_style(5, 6, false)]);
        let json = serialize_content(&doc, 0, 11);
        let (text, ds) = deserialize_content(&json).unwrap();
        assert_eq!(text, "hello world");
        assert_eq!(ds.len(), 2);
        assert!(ds[0].bold);
        assert!(!ds[1].bold);
    }

    #[test]
    fn deserialize_invalid_json_returns_none() { assert!(deserialize_content("not json").is_none()); }

    #[test]
    fn serialize_partial_range() {
        let doc = TestDoc::with_styles("hello world", vec![make_style(0, 11, true)]);
        let json = serialize_content(&doc, 6, 5);
        let (text, runs) = deserialize_content(&json).unwrap();
        assert_eq!(text, "world");
        assert_eq!(runs[0].offset, 0);
        assert_eq!(runs[0].length, 5);
    }

    #[test]
    fn serialize_empty_doc() { assert!(serialize_content(&TestDoc::new(""), 0, 0).is_empty()); }

    #[test]
    fn no_space_at_document_start() {
        let r = add_intelligent_paste_space(&TestDoc::new("hello"), 0, "world");
        assert_eq!(r.text, "world");
        assert_eq!(r.added_before, 0);
    }

    #[test]
    fn space_added_between_words() {
        let r = add_intelligent_paste_space(&TestDoc::new("helloworld"), 5, "there");
        assert_eq!(r.text, " there ");
        assert_eq!(r.added_before, 1);
        assert_eq!(r.added_after, 1);
    }

    #[test]
    fn no_space_when_already_present() {
        let r = add_intelligent_paste_space(&TestDoc::new("hello world"), 6, "big");
        assert_eq!(r.added_before, 0);
        assert_eq!(r.added_after, 1);
    }

    #[test]
    fn no_space_with_punctuation() {
        let r = add_intelligent_paste_space(&TestDoc::new("hello.world"), 6, "big");
        assert_eq!(r.added_before, 0);
    }

    #[test]
    fn copy_no_selection() { assert!(clipboard_copy(&TestDoc::new("hello"), 3, 3).is_none()); }

    #[test]
    fn copy_returns_json() {
        let json = clipboard_copy(&TestDoc::new("hello world"), 0, 5).unwrap();
        assert_eq!(deserialize_content(&json).unwrap().0, "hello");
    }

    #[test]
    fn cut_removes_text() {
        let doc = TestDoc::new("hello world");
        let json = clipboard_cut(&doc, 0, 5).unwrap();
        assert_eq!(deserialize_content(&json).unwrap().0, "hello");
        assert_eq!(doc.text(), " world");
    }

    #[test]
    fn cut_no_selection() {
        let doc = TestDoc::new("hello");
        assert!(clipboard_cut(&doc, 2, 2).is_none());
        assert_eq!(doc.text(), "hello");
    }

    #[test]
    fn paste_plain_text() {
        let doc = TestDoc::new("hello world");
        let mut state = EditorState::default();
        state.caret = 5;
        clipboard_paste(&doc, &mut state, "there", true);
        assert!(doc.text().contains("there"));
        assert!(state.caret > 5);
        assert_eq!(state.sel_anchor, -1);
    }

    #[test]
    fn paste_formatted_json() {
        let source = TestDoc::with_styles("foo", vec![make_style(0, 3, true)]);
        let json = serialize_content(&source, 0, 3);
        let doc = TestDoc::new("bar baz");
        let mut state = EditorState::default();
        state.caret = 4;
        clipboard_paste(&doc, &mut state, &json, false);
        assert!(doc.text().contains("foo"));
    }

    #[test]
    fn paste_empty_noop() {
        let doc = TestDoc::new("hello");
        let mut state = EditorState::default();
        state.caret = 3;
        clipboard_paste(&doc, &mut state, "", false);
        assert_eq!(doc.text(), "hello");
    }

    #[test]
    fn paste_non_json_falls_back_to_plain() {
        let doc = TestDoc::new("hello world");
        let mut state = EditorState::default();
        state.caret = 5;
        clipboard_paste(&doc, &mut state, "there", false);
        assert!(doc.text().contains("there"));
    }

    #[test]
    fn intelligent_copy_word() {
        let json = clipboard_intelligent_copy(&TestDoc::new("hello world"), 2).unwrap();
        assert_eq!(deserialize_content(&json).unwrap().0, "hello");
    }

    #[test]
    fn intelligent_cut_word_with_space() {
        let doc = TestDoc::new("hello world");
        let json = clipboard_intelligent_cut(&doc, 2).unwrap();
        assert_eq!(deserialize_content(&json).unwrap().0, "hello");
        assert!(doc.text().len() < 11);
    }

    #[test]
    fn serialize_graphic_run() {
        let g = Graphic::new(0, 100, 50, None);
        let run = StyleRun {
            offset: 0, length: 1, bold: false, italic: false, underline: false,
            color: RGBA::new(0.0, 0.0, 0.0, 1.0), font_size: 0, font_family: None,
            is_graphic: true, graphic: Some(g), link_url: None,
        };
        let doc = TestDoc::with_styles("X", vec![run]);
        let (_, styles) = deserialize_content(&serialize_content(&doc, 0, 1)).unwrap();
        assert!(styles[0].is_graphic);
        assert_eq!(styles[0].graphic.as_ref().unwrap().width, 100);
    }

    #[test]
    fn color_preserved_in_roundtrip() {
        let run = StyleRun {
            offset: 0, length: 3, bold: false, italic: false, underline: false,
            color: RGBA::new(1.0, 0.5, 0.25, 0.75), font_size: 14, font_family: None,
            is_graphic: false, graphic: None, link_url: None,
        };
        let doc = TestDoc::with_styles("abc", vec![run]);
        let (_, styles) = deserialize_content(&serialize_content(&doc, 0, 3)).unwrap();
        let c = &styles[0].color;
        assert!((c.red() - 1.0).abs() < 0.01);
        assert!((c.green() - 0.5).abs() < 0.01);
    }
}
