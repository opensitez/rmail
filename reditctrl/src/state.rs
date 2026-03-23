//! Editor state — caret, selection, drag, image resize, theme colors.
//!
//! Ported from gEditCtrl/gedit-state.{h,c}.

use gdk4::RGBA;

use crate::input::PendingLink;

/// Core editor state. One instance per editor widget.
#[derive(Debug, Clone)]
pub struct EditorState {
    /// Caret position as UTF-8 character offset.
    pub caret: i32,
    /// Selection anchor (character offset); -1 when no selection.
    pub sel_anchor: i32,
    /// Selection start (character offset, always <= sel_end).
    pub sel_start: i32,
    /// Selection end (character offset).
    pub sel_end: i32,
    /// Whether a mouse drag is in progress.
    pub dragging: bool,
    /// Whether the caret blink is currently in the visible phase.
    pub caret_visible: bool,
    /// Preferred X pixel coordinate for vertical (up/down) navigation; -1 = unset.
    pub preferred_x: i32,

    // ── Image resize state ──────────────────────────────────────────
    /// True while dragging a resize handle on an inline image.
    pub resizing: bool,
    /// Character offset of the graphic being resized.
    pub resize_graphic_offset: i32,
    /// Mouse X at the start of the resize drag.
    pub resize_start_x: f64,
    /// Mouse Y at the start of the resize drag.
    pub resize_start_y: f64,
    /// Original image width at drag start.
    pub resize_orig_w: i32,
    /// Original image height at drag start.
    pub resize_orig_h: i32,
    /// Character offset of the currently selected graphic, -1 if none.
    pub selected_graphic: i32,

    // ── Editable ────────────────────────────────────────────────────
    /// False means read-only (selection allowed, no text changes).
    pub editable: bool,

    // ── Pending actions for the widget layer ─────────────────────────
    /// Set by input handlers when a link was clicked; widget layer shows popover.
    pub pending_link: Option<PendingLink>,
    /// Set when Cmd+C is pressed with a selection; widget layer performs copy.
    pub pending_copy: bool,
    /// Set when Cmd+X is pressed with a selection; widget layer performs cut.
    pub pending_cut: bool,
    /// Set when Cmd+V is pressed; widget layer performs paste.
    pub pending_paste: bool,

    // ── Theme colors ────────────────────────────────────────────────
    /// Whether custom theme colors have been set.
    pub has_theme: bool,
    /// Background color.
    pub bg_color: RGBA,
    /// Default text color.
    pub text_color: RGBA,
    /// Caret / cursor color.
    pub caret_color: RGBA,
    /// Selection background color.
    pub sel_bg_color: RGBA,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            caret: 0,
            sel_anchor: -1,
            sel_start: 0,
            sel_end: 0,
            dragging: false,
            caret_visible: true,
            preferred_x: -1,

            resizing: false,
            resize_graphic_offset: 0,
            resize_start_x: 0.0,
            resize_start_y: 0.0,
            resize_orig_w: 0,
            resize_orig_h: 0,
            selected_graphic: -1,

            editable: true,

            pending_link: None,
            pending_copy: false,
            pending_cut: false,
            pending_paste: false,

            has_theme: false,
            bg_color: RGBA::new(1.0, 1.0, 1.0, 1.0),   // white
            text_color: RGBA::new(0.0, 0.0, 0.0, 1.0),  // black
            caret_color: RGBA::new(0.0, 0.0, 0.0, 1.0),
            sel_bg_color: RGBA::new(0.0, 0.0, 0.0, 0.0), // unset
        }
    }
}

impl EditorState {
    /// Create a new default editor state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true when a selection is active (start != end).
    pub fn has_selection(&self) -> bool {
        self.sel_start != self.sel_end
    }

    /// Clear any active selection, resetting anchor/start/end.
    pub fn clear_selection(&mut self) {
        self.sel_anchor = -1;
        self.sel_start = 0;
        self.sel_end = 0;
    }
}

// ── UTF-8 helpers ───────────────────────────────────────────────────────

/// Convert a UTF-8 character index to the corresponding byte offset.
///
/// Returns 0 for an empty / `None` string.
pub fn char_to_byte(text: &str, char_index: i32) -> i32 {
    if text.is_empty() || char_index <= 0 {
        return 0;
    }
    let idx = char_index as usize;
    text.char_indices()
        .nth(idx)
        .map(|(byte_off, _)| byte_off as i32)
        .unwrap_or(text.len() as i32)
}

/// Convert a byte offset to the number of UTF-8 characters before it.
///
/// Returns 0 for an empty / `None` string.
pub fn byte_to_char(text: &str, byte_index: i32) -> i32 {
    if text.is_empty() || byte_index <= 0 {
        return 0;
    }
    let byte_idx = (byte_index as usize).min(text.len());
    text[..byte_idx].chars().count() as i32
}

/// Determine the active paragraph range around the caret/selection.
///
/// When there is no selection (`sel_start == sel_end`), the range covers
/// the single paragraph that contains the caret — from the character after
/// the preceding newline (or start-of-text) up to and including the
/// paragraph's terminating newline (or end-of-text).
///
/// When there *is* a selection, the range is simply `(sel_start, sel_end - sel_start)`.
///
/// Returns `(start_char_offset, length_in_chars)`.
pub fn get_active_para_range(state: &EditorState, text: &str) -> (i32, i32) {
    if text.is_empty() {
        return (0, 0);
    }

    let a = state.sel_start.min(state.sel_end);
    let b = state.sel_start.max(state.sel_end);

    if a == b {
        // No selection — find the paragraph around the caret.
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len() as i32;
        let pos = state.caret.max(0).min(len);

        // Scan backwards to the start of the paragraph.
        let mut st = pos;
        while st > 0 {
            if chars[(st - 1) as usize] == '\n' {
                break;
            }
            st -= 1;
        }

        // Scan forwards to the end of the paragraph (include the trailing '\n').
        let mut ed = pos;
        while ed < len {
            if chars[ed as usize] == '\n' {
                ed += 1;
                break;
            }
            ed += 1;
        }

        if ed < st {
            ed = st;
        }

        (st, ed - st)
    } else {
        (a, b - a)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // -- EditorState tests --

    #[test]
    fn default_state_values() {
        let s = EditorState::new();
        assert_eq!(s.caret, 0);
        assert_eq!(s.sel_anchor, -1);
        assert_eq!(s.sel_start, 0);
        assert_eq!(s.sel_end, 0);
        assert!(!s.dragging);
        assert!(s.caret_visible);
        assert_eq!(s.preferred_x, -1);
        assert!(!s.resizing);
        assert_eq!(s.selected_graphic, -1);
        assert!(s.editable);
        assert!(!s.has_theme);
    }

    #[test]
    fn has_selection() {
        let mut s = EditorState::new();
        assert!(!s.has_selection());
        s.sel_start = 2;
        s.sel_end = 5;
        assert!(s.has_selection());
    }

    #[test]
    fn clear_selection() {
        let mut s = EditorState::new();
        s.sel_anchor = 3;
        s.sel_start = 3;
        s.sel_end = 7;
        s.clear_selection();
        assert_eq!(s.sel_anchor, -1);
        assert!(!s.has_selection());
    }

    // -- char_to_byte tests --

    #[test]
    fn char_to_byte_empty() {
        assert_eq!(char_to_byte("", 0), 0);
        assert_eq!(char_to_byte("", 5), 0);
    }

    #[test]
    fn char_to_byte_ascii() {
        assert_eq!(char_to_byte("hello", 0), 0);
        assert_eq!(char_to_byte("hello", 3), 3);
        assert_eq!(char_to_byte("hello", 5), 5); // past end
    }

    #[test]
    fn char_to_byte_multibyte() {
        // "aé" — 'a' is 1 byte, 'é' is 2 bytes (U+00E9)
        let text = "a\u{00E9}b";
        assert_eq!(char_to_byte(text, 0), 0); // 'a'
        assert_eq!(char_to_byte(text, 1), 1); // 'é' starts at byte 1
        assert_eq!(char_to_byte(text, 2), 3); // 'b' starts at byte 3
        assert_eq!(char_to_byte(text, 3), 4); // past end
    }

    #[test]
    fn char_to_byte_negative() {
        assert_eq!(char_to_byte("hello", -1), 0);
    }

    // -- byte_to_char tests --

    #[test]
    fn byte_to_char_empty() {
        assert_eq!(byte_to_char("", 0), 0);
    }

    #[test]
    fn byte_to_char_ascii() {
        assert_eq!(byte_to_char("hello", 3), 3);
    }

    #[test]
    fn byte_to_char_multibyte() {
        let text = "a\u{00E9}b"; // 4 bytes: a(1) é(2) b(1)
        assert_eq!(byte_to_char(text, 0), 0);
        assert_eq!(byte_to_char(text, 1), 1); // 1 byte = 'a'
        assert_eq!(byte_to_char(text, 3), 2); // 3 bytes = 'a' + 'é'
        assert_eq!(byte_to_char(text, 4), 3); // all chars
    }

    #[test]
    fn byte_to_char_negative() {
        assert_eq!(byte_to_char("hello", -1), 0);
    }

    #[test]
    fn byte_to_char_past_end() {
        // Should clamp to text length.
        assert_eq!(byte_to_char("hi", 100), 2);
    }

    // -- get_active_para_range tests --

    #[test]
    fn para_range_empty() {
        let s = EditorState::new();
        assert_eq!(get_active_para_range(&s, ""), (0, 0));
    }

    #[test]
    fn para_range_single_paragraph() {
        let mut s = EditorState::new();
        s.caret = 3;
        let (start, len) = get_active_para_range(&s, "hello");
        assert_eq!(start, 0);
        assert_eq!(len, 5);
    }

    #[test]
    fn para_range_middle_paragraph() {
        // "abc\ndef\nghi" — paragraphs: [0..4) [4..8) [8..11)
        let mut s = EditorState::new();
        s.caret = 5; // inside "def"
        let (start, len) = get_active_para_range(&s, "abc\ndef\nghi");
        assert_eq!(start, 4);
        assert_eq!(len, 4); // "def\n"
    }

    #[test]
    fn para_range_last_paragraph_no_trailing_newline() {
        let mut s = EditorState::new();
        s.caret = 9; // inside "ghi"
        let (start, len) = get_active_para_range(&s, "abc\ndef\nghi");
        assert_eq!(start, 8);
        assert_eq!(len, 3); // "ghi"
    }

    #[test]
    fn para_range_with_selection() {
        let mut s = EditorState::new();
        s.sel_start = 2;
        s.sel_end = 7;
        let (start, len) = get_active_para_range(&s, "abc\ndef\nghi");
        assert_eq!(start, 2);
        assert_eq!(len, 5);
    }

    #[test]
    fn para_range_caret_at_newline() {
        // Caret right on the '\n' after "abc" (char offset 3).
        let mut s = EditorState::new();
        s.caret = 3;
        let (start, len) = get_active_para_range(&s, "abc\ndef");
        // Should belong to first paragraph "abc\n".
        assert_eq!(start, 0);
        assert_eq!(len, 4);
    }

    #[test]
    fn para_range_caret_at_start() {
        let mut s = EditorState::new();
        s.caret = 0;
        let (start, len) = get_active_para_range(&s, "abc\ndef");
        assert_eq!(start, 0);
        assert_eq!(len, 4);
    }

    #[test]
    fn para_range_multibyte() {
        // "aé\nbc" — chars: a(0) é(1) \n(2) b(3) c(4)
        let mut s = EditorState::new();
        s.caret = 1; // on 'é'
        let (start, len) = get_active_para_range(&s, "a\u{00E9}\nbc");
        assert_eq!(start, 0);
        assert_eq!(len, 3); // "aé\n"
    }
}
