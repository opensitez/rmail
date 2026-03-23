//! Input handling for rEditCtrl: mouse clicks, drag selection, keyboard
//! navigation, character input, clipboard shortcuts, image hit-testing
//! and resize dragging, link popover display.
//!
//! Pure-logic helpers (word/line boundary finding, hit-test geometry,
//! key-dispatch decisions) are kept separate from GTK signal wiring so
//! they can be unit-tested without a display server.

use crate::document::DocumentApi;
use crate::state::EditorState;

use gdk4::Key;
use gdk4::ModifierType;
use gtk4::prelude::*;


// ---------------------------------------------------------------------------
// Public geometry / pure-logic helpers (testable without GTK)
// ---------------------------------------------------------------------------

/// Padding applied on each side of the drawing area (matches C constant 6).
pub const PADDING: f64 = 6.0;

/// Minimum paragraph height in pixels (for empty lines).
pub const MIN_PARA_HEIGHT: i32 = 14;

/// Pixel size of the resize-handle hit zone at each corner of a graphic.
pub const HANDLE_HIT_SIZE: f64 = 12.0;

/// Minimum image dimension during resize.
pub const MIN_IMAGE_DIM: i32 = 32;

/// Result of [`hit_test_handles`]: which corner handle was hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Check whether a point (mx, my) hits one of the four resize handles of a
/// rectangle at (gx, gy) with size (gw, gh).  Returns `Some(corner)` on hit.
pub fn hit_test_handles(
    mx: f64,
    my: f64,
    gx: f64,
    gy: f64,
    gw: i32,
    gh: i32,
) -> Option<HandleCorner> {
    let corners: [(f64, f64, HandleCorner); 4] = [
        (gx - 4.0, gy - 4.0, HandleCorner::TopLeft),
        (gx + gw as f64 - 8.0, gy - 4.0, HandleCorner::TopRight),
        (gx - 4.0, gy + gh as f64 - 8.0, HandleCorner::BottomLeft),
        (gx + gw as f64 - 8.0, gy + gh as f64 - 8.0, HandleCorner::BottomRight),
    ];
    for &(cx, cy, corner) in &corners {
        if mx >= cx && mx <= cx + HANDLE_HIT_SIZE && my >= cy && my <= cy + HANDLE_HIT_SIZE {
            return Some(corner);
        }
    }
    None
}

/// Check whether point (mx, my) is inside the bounding box of a graphic
/// at (gx, gy) with size (gw, gh).
pub fn point_in_rect(mx: f64, my: f64, gx: f64, gy: f64, gw: i32, gh: i32) -> bool {
    mx >= gx && mx <= gx + gw as f64 && my >= gy && my <= gy + gh as f64
}

/// Compute new image dimensions during aspect-ratio-preserving resize.
/// `orig_w`, `orig_h` are the original image dimensions.
/// `dx`, `dy` are the mouse deltas from drag start.
/// Returns `(new_w, new_h)`, each clamped to [`MIN_IMAGE_DIM`].
pub fn compute_resize(orig_w: i32, orig_h: i32, dx: f64, dy: f64) -> (i32, i32) {
    let aspect = if orig_h > 0 {
        orig_w as f64 / orig_h as f64
    } else {
        1.0
    };
    if dx.abs() > dy.abs() {
        let new_w = (orig_w as f64 + dx).max(MIN_IMAGE_DIM as f64) as i32;
        let new_h = (new_w as f64 / aspect).max(MIN_IMAGE_DIM as f64) as i32;
        (new_w, new_h)
    } else {
        let new_h = (orig_h as f64 + dy).max(MIN_IMAGE_DIM as f64) as i32;
        let new_w = (new_h as f64 * aspect).max(MIN_IMAGE_DIM as f64) as i32;
        (new_w, new_h)
    }
}

// -- Word / line boundary helpers (operate on a &str, char-indexed) ---------

/// Return `true` if `ch` is a word character (alphanumeric or underscore).
pub fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Find the start and end (exclusive) of the word surrounding `pos` in
/// `text` (char-indexed).  If `pos` sits on a non-word character, the
/// single character at `pos` is returned as the selection.
pub fn word_bounds(text: &str, pos: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut start = pos;
    let mut end = pos;

    // Expand left
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    // Expand right
    while end < len && is_word_char(chars[end]) {
        end += 1;
    }
    // If no expansion (clicked on non-word char), select the single char
    if start == end && pos < len {
        (pos, pos + 1)
    } else {
        (start, end)
    }
}

/// Find the line (paragraph) boundaries around `pos` (char-indexed).
/// Returns `(line_start, line_end)` where `line_end` is one past the
/// trailing newline (if present).
pub fn line_bounds(text: &str, pos: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut start = pos;
    let mut end = pos;

    while start > 0 && chars[start - 1] != '\n' {
        start -= 1;
    }
    while end < len {
        if chars[end] == '\n' {
            end += 1;
            break;
        }
        end += 1;
    }
    (start, end)
}

/// Determine what action a key press maps to.  This is the pure decision
/// function; it does not touch any GTK or document state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Insert a character.
    InsertChar(char),
    /// Insert a newline.
    InsertNewline,
    /// Insert a tab.
    InsertTab,
    /// Delete backward (Backspace).
    Backspace,
    /// Delete forward (Delete).
    Delete,
    /// Navigation: move caret.
    Navigate(NavKind),
    /// Clipboard copy.
    Copy,
    /// Clipboard cut.
    Cut,
    /// Clipboard paste.
    Paste,
    /// Select all.
    SelectAll,
    /// Undo.
    Undo,
    /// Redo.
    Redo,
    /// Key not handled by the editor.
    Unhandled,
}

/// Navigation sub-kinds carried inside [`KeyAction::Navigate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKind {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    CtrlHome,
    CtrlEnd,
    WordLeft,
    WordRight,
    PageUp,
    PageDown,
}

/// Map a raw key press to a [`KeyAction`].
///
/// `editable` controls whether modification keys (Return, Backspace, char
/// input, Cut, Paste, Undo, Redo) are allowed.  Navigation and Copy/SelectAll
/// always work.
pub fn classify_key(keyval: Key, mods: ModifierType, editable: bool) -> KeyAction {
    let cmd = mods.contains(ModifierType::CONTROL_MASK)
        || mods.contains(ModifierType::META_MASK);
    let shift = mods.contains(ModifierType::SHIFT_MASK);
    let alt = mods.contains(ModifierType::ALT_MASK);

    // Command shortcuts (Ctrl/Cmd held, no Alt)
    if cmd && !alt {
        let lower = keyval.to_lower();
        if lower == Key::c {
            return KeyAction::Copy;
        }
        if lower == Key::a {
            return KeyAction::SelectAll;
        }
        // Modification shortcuts -- blocked when read-only
        if editable {
            if lower == Key::z {
                return if shift {
                    KeyAction::Redo
                } else {
                    KeyAction::Undo
                };
            }
            if lower == Key::y {
                return KeyAction::Redo;
            }
            if lower == Key::x {
                return KeyAction::Cut;
            }
            if lower == Key::v {
                return KeyAction::Paste;
            }
        }
    }

    // Navigation keys (always allowed)
    match keyval {
        k if k == Key::Left => {
            if cmd {
                return KeyAction::Navigate(NavKind::WordLeft);
            }
            return KeyAction::Navigate(NavKind::Left);
        }
        k if k == Key::Right => {
            if cmd {
                return KeyAction::Navigate(NavKind::WordRight);
            }
            return KeyAction::Navigate(NavKind::Right);
        }
        k if k == Key::Up => return KeyAction::Navigate(NavKind::Up),
        k if k == Key::Down => return KeyAction::Navigate(NavKind::Down),
        k if k == Key::Home => {
            if cmd {
                return KeyAction::Navigate(NavKind::CtrlHome);
            }
            return KeyAction::Navigate(NavKind::Home);
        }
        k if k == Key::End => {
            if cmd {
                return KeyAction::Navigate(NavKind::CtrlEnd);
            }
            return KeyAction::Navigate(NavKind::End);
        }
        k if k == Key::Page_Up => return KeyAction::Navigate(NavKind::PageUp),
        k if k == Key::Page_Down => return KeyAction::Navigate(NavKind::PageDown),
        _ => {}
    }

    // Read-only: block all remaining keys
    if !editable {
        return KeyAction::Unhandled;
    }

    // Editing keys
    if keyval == Key::Return || keyval == Key::KP_Enter {
        return KeyAction::InsertNewline;
    }
    if keyval == Key::Tab {
        return KeyAction::InsertTab;
    }
    if keyval == Key::BackSpace {
        return KeyAction::Backspace;
    }
    if keyval == Key::Delete {
        return KeyAction::Delete;
    }

    // Character input
    if let Some(ch) = keyval.to_unicode() {
        if !ch.is_control() {
            return KeyAction::InsertChar(ch);
        }
    }

    KeyAction::Unhandled
}

// ---------------------------------------------------------------------------
// Click-to-offset resolution using Pango layouts
// ---------------------------------------------------------------------------

/// Convert pixel coordinate to content-area x (subtract padding, clamp >= 0).
fn content_x(x: f64) -> i32 {
    (x - PADDING).max(0.0) as i32
}

/// Convert pixel coordinate to content-area y (subtract padding, clamp >= 0).
fn content_y(y: f64) -> i32 {
    (y - PADDING).max(0.0) as i32
}

/// Resolve a click position `(ix, iy)` (content-relative pixels) to a
/// character offset in the document by iterating through paragraphs and
/// using Pango layout measurement.
///
/// `layout_fn` builds a `pango::Layout` for a paragraph string at a given
/// wrap width.  Returns the resolved char offset, or `None` if the click
/// is below all text.
fn resolve_click_to_offset<D: DocumentApi + ?Sized, F>(
    doc: &D,
    area_width: i32,
    ix: i32,
    iy: i32,
    layout_fn: &F,
) -> Option<i32>
where
    F: Fn(&str, i32) -> pango::Layout,
{
    let text = doc.get_text()?;
    let width = (area_width - 12).max(10);
    let mut cur: i32 = 0;
    let mut y_off: f64 = PADDING;
    let mut cur_ptr = &text[..];

    loop {
        if cur_ptr.is_empty() {
            break;
        }
        // Find paragraph end (up to and not including '\n')
        let newline_pos = cur_ptr.find('\n');
        let para_text = match newline_pos {
            Some(pos) => &cur_ptr[..pos],
            None => cur_ptr,
        };
        let para_chars = para_text.chars().count() as i32;

        let layout = layout_fn(para_text, width);

        // Apply shape/style attrs from the document
        let attrs = doc.get_attr_list_for_range(cur, para_chars);
        layout.set_attributes(Some(&attrs));

        let (_pxw, pxh) = layout.pixel_size();
        let pxh = pxh.max(MIN_PARA_HEIGHT);

        if iy >= y_off as i32 && iy < (y_off + pxh as f64) as i32 {
            if para_chars == 0 {
                return Some(cur);
            }

            // Use Pango to map pixel position to byte index
            let (_, byte_index, trailing) = layout.xy_to_index(
                ix * pango::SCALE,
                (iy - y_off as i32) * pango::SCALE,
            );
            let byte_index = byte_index as usize;
            let clamped = byte_index.min(para_text.len());
            let mut char_index = para_text[..clamped].chars().count() as i32 + cur;
            if trailing > 0 {
                char_index += 1;
            }

            // Past-end-of-visual-line detection: if click X is past the last
            // glyph on the visual line, snap to that line's end.
            let click_line_y = iy - y_off as i32;
            let mut iter = layout.iter();
            loop {
                let line_ext = iter.line_extents().1;
                let ly = line_ext.y() / pango::SCALE;
                let lh = line_ext.height() / pango::SCALE;
                if click_line_y >= ly && click_line_y < ly + lh {
                    let line_x_end = (line_ext.x() + line_ext.width()) / pango::SCALE;
                    if ix > line_x_end {
                        if let Some(pll) = iter.line_readonly() {
                            let end_byte =
                                (pll.start_index() as usize + pll.length() as usize)
                                    .min(para_text.len());
                            char_index = para_text[..end_byte].chars().count() as i32 + cur;
                        }
                    }
                    break;
                }
                if !iter.next_line() {
                    break;
                }
            }

            if char_index > cur + para_chars {
                char_index = cur + para_chars;
            }
            return Some(char_index);
        }

        // Advance past this paragraph
        cur += para_chars;
        match newline_pos {
            Some(pos) => {
                cur += 1; // the newline itself
                cur_ptr = &cur_ptr[pos + 1..]; // skip past '\n' byte
            }
            None => break,
        }
        y_off += if pxh > 0 { pxh as f64 } else { MIN_PARA_HEIGHT as f64 };
    }

    None // click was below all text
}

/// Find the screen rectangle of a graphic at `graphic_offset`.
/// Returns `Some((gx, gy, gw, gh))` in widget coordinates, or `None`.
pub fn find_graphic_rect<D: DocumentApi + ?Sized, F>(
    doc: &D,
    graphic_offset: i32,
    area_width: i32,
    layout_fn: &F,
) -> Option<(f64, f64, i32, i32)>
where
    F: Fn(&str, i32) -> pango::Layout,
{
    let text = doc.get_text()?;
    let width = (area_width - 12).max(10);
    let mut cur: i32 = 0;
    let mut y_off: f64 = PADDING;
    let mut cur_ptr = &text[..];

    loop {
        if cur_ptr.is_empty() {
            break;
        }
        let newline_pos = cur_ptr.find('\n');
        let para_text = match newline_pos {
            Some(pos) => &cur_ptr[..pos],
            None => cur_ptr,
        };
        let para_chars = para_text.chars().count() as i32;

        let layout = layout_fn(para_text, width);
        let attrs = doc.get_attr_list_for_range(cur, para_chars);
        layout.set_attributes(Some(&attrs));
        let (_pxw, pxh) = layout.pixel_size();
        let pxh = pxh.max(MIN_PARA_HEIGHT);

        if graphic_offset >= cur && graphic_offset < cur + para_chars {
            let local_char = (graphic_offset - cur) as usize;
            let byte_offset = para_text
                .char_indices()
                .nth(local_char)
                .map(|(b, _)| b)
                .unwrap_or(para_text.len());

            let (cursor_rect, _) = layout.cursor_pos(byte_offset as i32);
            let gx = PADDING + cursor_rect.x() as f64 / pango::SCALE as f64;
            let gy = y_off + cursor_rect.y() as f64 / pango::SCALE as f64;

            for run in doc.get_style_runs() {
                if run.is_graphic && run.offset == graphic_offset {
                    if let Some(ref g) = run.graphic {
                        return Some((gx, gy, g.width, g.height));
                    }
                }
            }
        }

        cur += para_chars;
        match newline_pos {
            Some(pos) => {
                cur += 1;
                cur_ptr = &cur_ptr[pos + 1..];
            }
            None => break,
        }
        y_off += if pxh > 0 { pxh as f64 } else { MIN_PARA_HEIGHT as f64 };
    }
    None
}

/// Hit-test all graphics in the document against point (mx, my).
/// Returns `Some((char_offset, on_handle))` for the first graphic hit.
pub fn hit_test_graphic<D: DocumentApi + ?Sized, F>(
    doc: &D,
    mx: f64,
    my: f64,
    area_width: i32,
    layout_fn: &F,
) -> Option<(i32, bool)>
where
    F: Fn(&str, i32) -> pango::Layout,
{
    for run in doc.get_style_runs() {
        if !run.is_graphic || run.graphic.is_none() {
            continue;
        }
        let (gx, gy, gw, gh) =
            match find_graphic_rect(doc, run.offset, area_width, layout_fn) {
                Some(r) => r,
                None => continue,
            };
        if hit_test_handles(mx, my, gx, gy, gw, gh).is_some() {
            return Some((run.offset, true));
        }
        if point_in_rect(mx, my, gx, gy, gw, gh) {
            return Some((run.offset, false));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Link extent helpers
// ---------------------------------------------------------------------------

/// Find the full contiguous extent of a link at `offset` by examining style
/// runs for matching URLs.  Returns `(start, end, url)` or `None`.
pub fn find_link_extent<D: DocumentApi + ?Sized>(
    doc: &D,
    offset: i32,
) -> Option<(i32, i32, String)> {
    let runs = doc.get_style_runs();
    let mut url: Option<String> = None;
    let mut link_start = offset;
    let mut link_end = offset + 1;

    for run in &runs {
        if offset >= run.offset
            && offset < run.offset + run.length
            && run.link_url.is_some()
        {
            url = run.link_url.clone();
            link_start = run.offset;
            link_end = run.offset + run.length;
            break;
        }
    }

    let url = url?;

    // Extend backward
    loop {
        let mut extended = false;
        for run in &runs {
            if run.link_url.as_deref() == Some(&url)
                && run.offset + run.length == link_start
            {
                link_start = run.offset;
                extended = true;
            }
        }
        if !extended {
            break;
        }
    }
    // Extend forward
    loop {
        let mut extended = false;
        for run in &runs {
            if run.link_url.as_deref() == Some(&url) && run.offset == link_end {
                link_end = run.offset + run.length;
                extended = true;
            }
        }
        if !extended {
            break;
        }
    }

    Some((link_start, link_end, url))
}

// ---------------------------------------------------------------------------
// Pending actions communicated to the widget layer
// ---------------------------------------------------------------------------

/// Information about a link click to be consumed by the widget layer for
/// popover display.
#[derive(Debug, Clone)]
pub struct PendingLink {
    pub offset: i32,
    pub url: String,
    pub click_x: f64,
    pub click_y: f64,
}

// ---------------------------------------------------------------------------
// High-level input entry points
// ---------------------------------------------------------------------------

/// Handle a mouse button press.
///
/// `n_press`: 1 = single click, 2 = double, 3 = triple.
/// `layout_fn`: builds a Pango layout for a paragraph string at a wrap width.
///
/// Returns `true` if the event was consumed and the widget should redraw.
pub fn on_pressed<D: DocumentApi + ?Sized, F>(
    state: &mut EditorState,
    doc: &D,
    area_width: i32,
    n_press: i32,
    x: f64,
    y: f64,
    layout_fn: &F,
) -> bool
where
    F: Fn(&str, i32) -> pango::Layout,
{
    // Check graphic / resize handle hit
    if let Some((offset, on_handle)) =
        hit_test_graphic(doc, x, y, area_width, layout_fn)
    {
        if on_handle {
            state.selected_graphic = offset;
            state.resizing = true;
            state.resize_graphic_offset = offset;
            state.resize_start_x = x;
            state.resize_start_y = y;
            for run in doc.get_style_runs() {
                if run.is_graphic && run.offset == offset {
                    if let Some(ref g) = run.graphic {
                        state.resize_orig_w = g.width;
                        state.resize_orig_h = g.height;
                    }
                    break;
                }
            }
            state.dragging = false;
            return true;
        }
        // Clicked on graphic body -- select it
        state.selected_graphic = offset;
        state.caret = offset;
        state.sel_anchor = offset;
        state.sel_start = offset;
        state.sel_end = offset + 1;
        state.dragging = false;
        return true;
    }

    // Clear graphic selection
    state.selected_graphic = -1;
    state.resizing = false;

    let ix = content_x(x);
    let iy = content_y(y);

    let doc_len = doc.get_length();
    let char_offset = resolve_click_to_offset(doc, area_width, ix, iy, layout_fn)
        .unwrap_or(doc_len);

    state.caret = char_offset;
    state.sel_anchor = char_offset;
    state.sel_start = char_offset;
    state.sel_end = char_offset;
    state.dragging = true;

    // Double-click: select word.  Triple-click: select line.
    if n_press >= 2 {
        if let Some(text) = doc.get_text() {
            if n_press == 2 {
                let (ws, we) = word_bounds(&text, char_offset as usize);
                state.sel_start = ws as i32;
                state.sel_end = we as i32;
                state.sel_anchor = ws as i32;
                state.caret = we as i32;
            } else {
                let (ls, le) = line_bounds(&text, char_offset as usize);
                state.sel_start = ls as i32;
                state.sel_end = le as i32;
                state.sel_anchor = ls as i32;
                state.caret = le as i32;
            }
            state.dragging = false;
        }
    }

    true
}

/// Handle mouse motion (drag for selection or image resize).
///
/// Returns `true` if the event was consumed and the widget should redraw.
pub fn on_motion<D: DocumentApi + ?Sized, F>(
    state: &mut EditorState,
    doc: &D,
    area_width: i32,
    x: f64,
    y: f64,
    layout_fn: &F,
) -> bool
where
    F: Fn(&str, i32) -> pango::Layout,
{
    // Image resize drag
    if state.resizing && state.resize_graphic_offset >= 0 {
        let dx = x - state.resize_start_x;
        let dy = y - state.resize_start_y;
        let (new_w, new_h) =
            compute_resize(state.resize_orig_w, state.resize_orig_h, dx, dy);
        doc.set_graphic_size(state.resize_graphic_offset, new_w, new_h);
        return true;
    }

    if !state.dragging {
        return false;
    }

    let ix = content_x(x);
    let iy = content_y(y);
    let doc_len = doc.get_length();

    let char_offset = resolve_click_to_offset(doc, area_width, ix, iy, layout_fn)
        .unwrap_or(doc_len);

    // Update selection: keep anchor, move extent
    let anchor = state.sel_anchor;
    state.sel_start = anchor.min(char_offset);
    state.sel_end = anchor.max(char_offset);
    state.caret = char_offset;

    true
}

/// Handle mouse button release.
///
/// Returns `true` if the event was consumed.  If a link was clicked
/// (simple click with no drag selection), `state.pending_link` is populated
/// for the widget layer to display a popover.
pub fn on_released<D: DocumentApi + ?Sized>(
    state: &mut EditorState,
    doc: &D,
    x: f64,
    y: f64,
) -> bool {
    let was_dragging = state.dragging;
    state.dragging = false;
    state.resizing = false;
    state.resize_graphic_offset = -1;

    // Simple click (no selection drag) -- check for link
    if was_dragging && state.sel_start == state.sel_end {
        if let Some(url) = doc.get_link_at(state.caret) {
            state.pending_link = Some(PendingLink {
                offset: state.caret,
                url,
                click_x: x,
                click_y: y,
            });
            return true;
        }
    }

    true
}

/// Handle a key press.  Returns `true` if the event was consumed.
///
/// Clipboard operations (copy, cut, paste) set boolean flags on `state`
/// (`pending_copy`, `pending_cut`, `pending_paste`) for the widget layer
/// to service, since clipboard access requires GTK widget context.
pub fn on_key_pressed<D: DocumentApi + ?Sized>(
    state: &mut EditorState,
    doc: &D,
    keyval: Key,
    mods: ModifierType,
) -> bool {
    let action = classify_key(keyval, mods, state.editable);
    let shift = mods.contains(ModifierType::SHIFT_MASK);

    match action {
        KeyAction::Copy => {
            if state.sel_start != state.sel_end {
                state.pending_copy = true;
            }
            true
        }
        KeyAction::SelectAll => {
            let doc_len = doc.get_length();
            state.sel_start = 0;
            state.sel_end = doc_len;
            state.sel_anchor = 0;
            state.caret = doc_len;
            true
        }
        KeyAction::Undo => {
            doc.undo();
            true
        }
        KeyAction::Redo => {
            doc.redo();
            true
        }
        KeyAction::Cut => {
            if state.sel_start != state.sel_end {
                state.pending_cut = true;
            }
            true
        }
        KeyAction::Paste => {
            delete_selection(state, doc);
            state.pending_paste = true;
            true
        }
        KeyAction::InsertNewline => {
            delete_selection(state, doc);
            doc.insert_text(state.caret, "\n");
            state.caret += 1;
            state.sel_start = state.caret;
            state.sel_end = state.caret;
            true
        }
        KeyAction::InsertTab => {
            delete_selection(state, doc);
            doc.insert_text(state.caret, "\t");
            state.caret += 1;
            state.sel_start = state.caret;
            state.sel_end = state.caret;
            true
        }
        KeyAction::Backspace => {
            if state.sel_start != state.sel_end {
                delete_selection(state, doc);
            } else if state.caret > 0 {
                doc.delete_range(state.caret - 1, 1);
                state.caret -= 1;
                state.sel_start = state.caret;
                state.sel_end = state.caret;
                state.sel_anchor = -1;
            }
            true
        }
        KeyAction::Delete => {
            if state.sel_start != state.sel_end {
                delete_selection(state, doc);
            } else {
                doc.delete_range(state.caret, 1);
            }
            true
        }
        KeyAction::InsertChar(ch) => {
            delete_selection(state, doc);
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            doc.insert_text(state.caret, s);
            state.caret += 1;
            state.sel_start = state.caret;
            state.sel_end = state.caret;
            true
        }
        KeyAction::Navigate(nav) => {
            let newpos = compute_navigation(state, doc, nav, shift);

            if shift {
                if state.sel_anchor == -1 {
                    state.sel_anchor = state.caret;
                }
                let anchor = state.sel_anchor;
                state.sel_start = anchor.min(newpos);
                state.sel_end = anchor.max(newpos);
                state.caret = newpos;
            } else {
                state.sel_anchor = -1;
                state.caret = newpos;
                state.sel_start = newpos;
                state.sel_end = newpos;
                state.preferred_x = -1;
            }
            true
        }
        KeyAction::Unhandled => false,
    }
}

/// Delete the current selection, collapsing caret to the start.
fn delete_selection<D: DocumentApi + ?Sized>(state: &mut EditorState, doc: &D) {
    if state.sel_start != state.sel_end {
        let a = state.sel_start.min(state.sel_end);
        let b = state.sel_start.max(state.sel_end);
        doc.delete_range(a, b - a);
        state.caret = a;
        state.sel_start = a;
        state.sel_end = a;
        state.sel_anchor = -1;
    }
}

/// Compute the new caret position for a navigation action.
///
/// The `shift` parameter indicates whether Shift is held; for Left/Right
/// without Shift when a selection exists, the caret collapses to the
/// selection boundary instead of moving by one character.
fn compute_navigation<D: DocumentApi + ?Sized>(
    state: &EditorState,
    doc: &D,
    nav: NavKind,
    shift: bool,
) -> i32 {
    let doc_len = doc.get_length();
    let pos = state.caret;

    match nav {
        NavKind::CtrlHome => 0,
        NavKind::CtrlEnd => doc_len,
        NavKind::Home => doc.find_line_start(pos),
        NavKind::End => doc.find_line_end(pos),
        NavKind::WordLeft => doc.find_word_boundary_left(pos),
        NavKind::WordRight => doc.find_word_boundary_right(pos),

        NavKind::Left => {
            if !shift && state.sel_start != state.sel_end {
                state.sel_start.min(state.sel_end)
            } else if pos > 0 {
                pos - 1
            } else {
                0
            }
        }
        NavKind::Right => {
            if !shift && state.sel_start != state.sel_end {
                state.sel_start.max(state.sel_end)
            } else if pos < doc_len {
                pos + 1
            } else {
                doc_len
            }
        }

        NavKind::Up => {
            if let Some(text) = doc.get_text() {
                let chars: Vec<char> = text.chars().collect();
                if pos <= 0 {
                    return 0;
                }
                let mut prev = pos - 1;
                while prev > 0 {
                    if chars[(prev - 1) as usize] == '\n' {
                        break;
                    }
                    prev -= 1;
                }
                if prev > 0 { prev } else { 0 }
            } else {
                0
            }
        }
        NavKind::Down => {
            if let Some(text) = doc.get_text() {
                let chars: Vec<char> = text.chars().collect();
                let len = chars.len() as i32;
                let mut next = pos;
                while next < len {
                    if chars[next as usize] == '\n' {
                        next += 1;
                        break;
                    }
                    next += 1;
                }
                next
            } else {
                pos
            }
        }

        NavKind::PageUp => {
            if let Some(text) = doc.get_text() {
                let chars: Vec<char> = text.chars().collect();
                let mut p = pos;
                for _ in 0..20 {
                    if p <= 0 {
                        break;
                    }
                    p -= 1;
                    while p > 0 && chars[(p - 1) as usize] != '\n' {
                        p -= 1;
                    }
                }
                p
            } else {
                0
            }
        }
        NavKind::PageDown => {
            if let Some(text) = doc.get_text() {
                let chars: Vec<char> = text.chars().collect();
                let len = chars.len() as i32;
                let mut p = pos;
                for _ in 0..20 {
                    if p >= len {
                        break;
                    }
                    while p < len {
                        if chars[p as usize] == '\n' {
                            p += 1;
                            break;
                        }
                        p += 1;
                    }
                }
                p
            } else {
                pos
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Show link popover (GTK UI -- called from widget layer)
// ---------------------------------------------------------------------------

/// Show a link popover at the given click position.  This creates the GTK
/// popover with Visit / Edit buttons.
pub fn show_link_popover<D: DocumentApi + ?Sized>(
    area: &impl IsA<gtk4::Widget>,
    doc: &D,
    char_offset: i32,
    click_x: f64,
    click_y: f64,
) {
    let (link_start, link_end, url) = match find_link_extent(doc, char_offset) {
        Some(e) => e,
        None => return,
    };
    let link_length = link_end - link_start;

    let popover = gtk4::Popover::new();
    popover.set_parent(area.as_ref());
    let rect = gdk4::Rectangle::new(click_x as i32, click_y as i32, 1, 1);
    popover.set_pointing_to(Some(&rect));

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    vbox.set_margin_start(8);
    vbox.set_margin_end(8);
    vbox.set_margin_top(8);
    vbox.set_margin_bottom(8);
    popover.set_child(Some(&vbox));

    // Truncated URL display
    let display_url = if url.chars().count() > 50 {
        let end = url
            .char_indices()
            .nth(47)
            .map(|(i, _)| i)
            .unwrap_or(url.len());
        format!("{}...", &url[..end])
    } else {
        url.clone()
    };
    let url_label = gtk4::Label::new(Some(&display_url));
    url_label.set_opacity(0.7);
    vbox.append(&url_label);

    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    vbox.append(&btn_box);

    let visit_btn = gtk4::Button::with_label("Visit");
    let edit_btn = gtk4::Button::with_label("Edit");
    btn_box.append(&visit_btn);
    btn_box.append(&edit_btn);

    // Visit button
    {
        let url_clone = url.clone();
        let area_widget = area.as_ref().clone();
        let pop_ref = popover.clone();
        visit_btn.connect_clicked(move |_| {
            if !url_clone.is_empty() {
                if let Some(root) = area_widget.root() {
                    if let Some(win) = root.downcast_ref::<gtk4::Window>() {
                        let launcher = gtk4::UriLauncher::new(&url_clone);
                        let win_clone = win.clone();
                        glib::spawn_future_local(async move {
                            let _ = launcher.launch_future(Some(&win_clone)).await;
                        });
                    }
                }
            }
            pop_ref.popdown();
        });
    }

    // Edit button
    {
        let url_clone = url.clone();
        let area_widget = area.as_ref().clone();
        let pop_ref = popover.clone();
        edit_btn.connect_clicked(move |_| {
            pop_ref.popdown();
            show_link_edit_dialog(&area_widget, link_start, link_length, &url_clone);
        });
    }

    popover.popup();
}

/// Show the link-edit dialog (edit URL or remove link).
fn show_link_edit_dialog(
    area: &gtk4::Widget,
    _link_offset: i32,
    _link_length: i32,
    current_url: &str,
) {
    let dialog = gtk4::Window::new();
    dialog.set_title(Some("Edit Link"));
    dialog.set_modal(true);
    if let Some(root) = area.root() {
        if let Some(win) = root.downcast_ref::<gtk4::Window>() {
            dialog.set_transient_for(Some(win));
        }
    }
    dialog.set_default_size(400, -1);
    dialog.set_resizable(false);

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    dialog.set_child(Some(&vbox));

    let label = gtk4::Label::new(Some("URL:"));
    label.set_halign(gtk4::Align::Start);
    vbox.append(&label);

    let entry = gtk4::Entry::new();
    entry.set_text(current_url);
    vbox.append(&entry);

    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk4::Align::End);
    vbox.append(&btn_box);

    let remove_btn = gtk4::Button::with_label("Remove Link");
    let ok_btn = gtk4::Button::with_label("OK");
    let cancel_btn = gtk4::Button::with_label("Cancel");
    btn_box.append(&remove_btn);
    btn_box.append(&cancel_btn);
    btn_box.append(&ok_btn);

    // Cancel
    {
        let dlg = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dlg.close();
        });
    }

    // OK
    {
        let dlg = dialog.clone();
        let _entry_ref = entry.clone();
        let area_ref = area.clone();
        ok_btn.connect_clicked(move |_| {
            // The widget layer retrieves the new URL from the entry and calls
            // doc.set_link(link_offset, link_length, Some(&new_url)).
            area_ref.queue_draw();
            dlg.close();
        });
    }

    // Remove
    {
        let dlg = dialog.clone();
        let area_ref = area.clone();
        remove_btn.connect_clicked(move |_| {
            // Widget layer calls doc.set_link(link_offset, link_length, None).
            area_ref.queue_draw();
            dlg.close();
        });
    }

    dialog.present();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // == word_bounds ==

    #[test]
    fn word_bounds_simple_word() {
        let text = "hello world";
        assert_eq!(word_bounds(text, 2), (0, 5));
        assert_eq!(word_bounds(text, 0), (0, 5));
        assert_eq!(word_bounds(text, 4), (0, 5));
    }

    #[test]
    fn word_bounds_second_word() {
        let text = "hello world";
        assert_eq!(word_bounds(text, 6), (6, 11));
        assert_eq!(word_bounds(text, 10), (6, 11));
    }

    #[test]
    fn word_bounds_on_space() {
        let text = "hello world";
        assert_eq!(word_bounds(text, 5), (5, 6));
    }

    #[test]
    fn word_bounds_underscore_included() {
        let text = "foo_bar baz";
        assert_eq!(word_bounds(text, 3), (0, 7));
    }

    #[test]
    fn word_bounds_at_end() {
        let text = "abc";
        assert_eq!(word_bounds(text, 2), (0, 3));
    }

    #[test]
    fn word_bounds_empty_string() {
        let text = "";
        assert_eq!(word_bounds(text, 0), (0, 0));
    }

    #[test]
    fn word_bounds_punctuation() {
        let text = "a.b";
        // On the dot
        assert_eq!(word_bounds(text, 1), (1, 2));
    }

    #[test]
    fn word_bounds_unicode() {
        let text = "caf\u{00E9} latte";
        // Inside "cafe" (char 2)
        assert_eq!(word_bounds(text, 2), (0, 4));
    }

    // == line_bounds ==

    #[test]
    fn line_bounds_single_line() {
        let text = "hello world";
        assert_eq!(line_bounds(text, 3), (0, 11));
    }

    #[test]
    fn line_bounds_first_line_of_multi() {
        let text = "line1\nline2\nline3";
        assert_eq!(line_bounds(text, 2), (0, 6));
    }

    #[test]
    fn line_bounds_second_line() {
        let text = "line1\nline2\nline3";
        assert_eq!(line_bounds(text, 8), (6, 12));
    }

    #[test]
    fn line_bounds_last_line_no_newline() {
        let text = "line1\nline2";
        assert_eq!(line_bounds(text, 7), (6, 11));
    }

    #[test]
    fn line_bounds_empty_line() {
        let text = "abc\n\ndef";
        // pos=4 is the empty line between the two \n
        assert_eq!(line_bounds(text, 4), (4, 5));
    }

    // == is_word_char ==

    #[test]
    fn is_word_char_basics() {
        assert!(is_word_char('a'));
        assert!(is_word_char('Z'));
        assert!(is_word_char('0'));
        assert!(is_word_char('_'));
        assert!(!is_word_char(' '));
        assert!(!is_word_char('.'));
        assert!(!is_word_char('\n'));
    }

    // == classify_key ==

    #[test]
    fn classify_copy_always_works_readonly() {
        let action = classify_key(Key::c, ModifierType::CONTROL_MASK, false);
        assert_eq!(action, KeyAction::Copy);
    }

    #[test]
    fn classify_select_all_readonly() {
        let action = classify_key(Key::a, ModifierType::CONTROL_MASK, false);
        assert_eq!(action, KeyAction::SelectAll);
    }

    #[test]
    fn classify_cut_blocked_readonly() {
        let action = classify_key(Key::x, ModifierType::CONTROL_MASK, false);
        assert_eq!(action, KeyAction::Unhandled);
    }

    #[test]
    fn classify_cut_allowed_editable() {
        let action = classify_key(Key::x, ModifierType::CONTROL_MASK, true);
        assert_eq!(action, KeyAction::Cut);
    }

    #[test]
    fn classify_paste_blocked_readonly() {
        let action = classify_key(Key::v, ModifierType::CONTROL_MASK, false);
        assert_eq!(action, KeyAction::Unhandled);
    }

    #[test]
    fn classify_undo_editable() {
        let action = classify_key(Key::z, ModifierType::CONTROL_MASK, true);
        assert_eq!(action, KeyAction::Undo);
    }

    #[test]
    fn classify_redo_shift_z() {
        let action = classify_key(
            Key::z,
            ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK,
            true,
        );
        assert_eq!(action, KeyAction::Redo);
    }

    #[test]
    fn classify_redo_y() {
        let action = classify_key(Key::y, ModifierType::CONTROL_MASK, true);
        assert_eq!(action, KeyAction::Redo);
    }

    #[test]
    fn classify_return_editable() {
        let action = classify_key(Key::Return, ModifierType::empty(), true);
        assert_eq!(action, KeyAction::InsertNewline);
    }

    #[test]
    fn classify_return_readonly() {
        let action = classify_key(Key::Return, ModifierType::empty(), false);
        assert_eq!(action, KeyAction::Unhandled);
    }

    #[test]
    fn classify_backspace() {
        let action = classify_key(Key::BackSpace, ModifierType::empty(), true);
        assert_eq!(action, KeyAction::Backspace);
    }

    #[test]
    fn classify_delete() {
        let action = classify_key(Key::Delete, ModifierType::empty(), true);
        assert_eq!(action, KeyAction::Delete);
    }

    #[test]
    fn classify_tab() {
        let action = classify_key(Key::Tab, ModifierType::empty(), true);
        assert_eq!(action, KeyAction::InsertTab);
    }

    #[test]
    fn classify_arrow_left() {
        let action = classify_key(Key::Left, ModifierType::empty(), true);
        assert_eq!(action, KeyAction::Navigate(NavKind::Left));
    }

    #[test]
    fn classify_arrow_left_readonly() {
        let action = classify_key(Key::Left, ModifierType::empty(), false);
        assert_eq!(action, KeyAction::Navigate(NavKind::Left));
    }

    #[test]
    fn classify_ctrl_left_word() {
        let action = classify_key(Key::Left, ModifierType::CONTROL_MASK, true);
        assert_eq!(action, KeyAction::Navigate(NavKind::WordLeft));
    }

    #[test]
    fn classify_ctrl_right_word() {
        let action = classify_key(Key::Right, ModifierType::CONTROL_MASK, true);
        assert_eq!(action, KeyAction::Navigate(NavKind::WordRight));
    }

    #[test]
    fn classify_home() {
        let action = classify_key(Key::Home, ModifierType::empty(), true);
        assert_eq!(action, KeyAction::Navigate(NavKind::Home));
    }

    #[test]
    fn classify_ctrl_home() {
        let action = classify_key(Key::Home, ModifierType::CONTROL_MASK, true);
        assert_eq!(action, KeyAction::Navigate(NavKind::CtrlHome));
    }

    #[test]
    fn classify_end() {
        let action = classify_key(Key::End, ModifierType::empty(), true);
        assert_eq!(action, KeyAction::Navigate(NavKind::End));
    }

    #[test]
    fn classify_ctrl_end() {
        let action = classify_key(Key::End, ModifierType::CONTROL_MASK, true);
        assert_eq!(action, KeyAction::Navigate(NavKind::CtrlEnd));
    }

    #[test]
    fn classify_page_up() {
        let action = classify_key(Key::Page_Up, ModifierType::empty(), true);
        assert_eq!(action, KeyAction::Navigate(NavKind::PageUp));
    }

    #[test]
    fn classify_page_down() {
        let action = classify_key(Key::Page_Down, ModifierType::empty(), false);
        assert_eq!(action, KeyAction::Navigate(NavKind::PageDown));
    }

    #[test]
    fn classify_char_input() {
        let action = classify_key(Key::a, ModifierType::empty(), true);
        assert_eq!(action, KeyAction::InsertChar('a'));
    }

    #[test]
    fn classify_char_blocked_readonly() {
        let action = classify_key(Key::a, ModifierType::empty(), false);
        assert_eq!(action, KeyAction::Unhandled);
    }

    #[test]
    fn classify_undo_blocked_readonly() {
        let action = classify_key(Key::z, ModifierType::CONTROL_MASK, false);
        assert_eq!(action, KeyAction::Unhandled);
    }

    #[test]
    fn classify_backspace_blocked_readonly() {
        let action = classify_key(Key::BackSpace, ModifierType::empty(), false);
        assert_eq!(action, KeyAction::Unhandled);
    }

    #[test]
    fn classify_delete_blocked_readonly() {
        let action = classify_key(Key::Delete, ModifierType::empty(), false);
        assert_eq!(action, KeyAction::Unhandled);
    }

    #[test]
    fn classify_meta_c_copy() {
        // macOS uses Meta
        let action = classify_key(Key::c, ModifierType::META_MASK, true);
        assert_eq!(action, KeyAction::Copy);
    }

    #[test]
    fn classify_alt_c_not_copy() {
        // Alt+C should not trigger copy
        let action = classify_key(
            Key::c,
            ModifierType::CONTROL_MASK | ModifierType::ALT_MASK,
            true,
        );
        // With alt held, cmd shortcuts are skipped
        assert_eq!(action, KeyAction::InsertChar('c'));
    }

    // == hit_test_handles ==

    #[test]
    fn handle_hit_top_left() {
        let result = hit_test_handles(97.0, 97.0, 100.0, 100.0, 200, 150);
        assert_eq!(result, Some(HandleCorner::TopLeft));
    }

    #[test]
    fn handle_hit_bottom_right() {
        let result = hit_test_handles(295.0, 245.0, 100.0, 100.0, 200, 150);
        assert_eq!(result, Some(HandleCorner::BottomRight));
    }

    #[test]
    fn handle_miss_center() {
        let result = hit_test_handles(200.0, 175.0, 100.0, 100.0, 200, 150);
        assert_eq!(result, None);
    }

    #[test]
    fn handle_hit_top_right() {
        let result = hit_test_handles(295.0, 97.0, 100.0, 100.0, 200, 150);
        assert_eq!(result, Some(HandleCorner::TopRight));
    }

    #[test]
    fn handle_hit_bottom_left() {
        let result = hit_test_handles(97.0, 245.0, 100.0, 100.0, 200, 150);
        assert_eq!(result, Some(HandleCorner::BottomLeft));
    }

    #[test]
    fn handle_miss_just_outside() {
        // Just outside the top-left handle zone
        let result = hit_test_handles(95.0, 95.0, 100.0, 100.0, 200, 150);
        assert_eq!(result, None);
    }

    // == point_in_rect ==

    #[test]
    fn point_inside_rect() {
        assert!(point_in_rect(150.0, 150.0, 100.0, 100.0, 200, 150));
    }

    #[test]
    fn point_outside_rect() {
        assert!(!point_in_rect(50.0, 50.0, 100.0, 100.0, 200, 150));
    }

    #[test]
    fn point_on_edge() {
        assert!(point_in_rect(100.0, 100.0, 100.0, 100.0, 200, 150));
        assert!(point_in_rect(300.0, 250.0, 100.0, 100.0, 200, 150));
    }

    #[test]
    fn point_just_outside_right() {
        assert!(!point_in_rect(300.1, 150.0, 100.0, 100.0, 200, 150));
    }

    // == compute_resize ==

    #[test]
    fn resize_wider() {
        let (w, h) = compute_resize(200, 100, 50.0, 10.0);
        assert_eq!(w, 250);
        assert_eq!(h, 125); // maintain 2:1 aspect
    }

    #[test]
    fn resize_taller() {
        let (w, h) = compute_resize(200, 100, 10.0, 50.0);
        assert_eq!(h, 150);
        assert_eq!(w, 300); // maintain 2:1 aspect
    }

    #[test]
    fn resize_clamps_to_min() {
        let (w, h) = compute_resize(200, 100, -300.0, -10.0);
        assert!(w >= MIN_IMAGE_DIM);
        assert!(h >= MIN_IMAGE_DIM);
    }

    #[test]
    fn resize_square() {
        let (w, h) = compute_resize(100, 100, 30.0, 5.0);
        assert_eq!(w, 130);
        assert_eq!(h, 130);
    }

    #[test]
    fn resize_zero_height_defaults_to_aspect_one() {
        let (w, h) = compute_resize(100, 0, 50.0, 0.0);
        assert_eq!(w, 150);
        assert_eq!(h, 150);
    }

    #[test]
    fn resize_negative_both() {
        let (w, h) = compute_resize(200, 100, -100.0, -200.0);
        // dy is larger, so height-driven: 100 + (-200) = -100 => clamped to 32
        assert_eq!(h, MIN_IMAGE_DIM);
        // w = 32 * 2.0 = 64
        assert_eq!(w, 64);
    }

    // == find_link_extent (needs a mock Document) ==

    // We test find_link_extent with a minimal mock that returns style runs
    // with link_url set.  Because it only uses get_style_runs(), the mock
    // is straightforward.

    use crate::document::{DocumentApi, StyleRun};

    struct MockDoc {
        style_runs: Vec<StyleRun>,
    }

    impl DocumentApi for MockDoc {
        fn get_text(&self) -> Option<String> { None }
        fn get_length(&self) -> i32 { 0 }
        fn get_style_runs(&self) -> Vec<StyleRun> { self.style_runs.clone() }
        fn delete_range(&self, _offset: i32, _length: i32) {}
        fn insert_text(&self, _o: i32, _t: &str) {}
        fn undo(&self) {}
        fn redo(&self) {}
        fn get_link_at(&self, _o: i32) -> Option<String> { None }
        fn find_word_boundary_left(&self, _o: i32) -> i32 { 0 }
        fn find_word_boundary_right(&self, _o: i32) -> i32 { 0 }
        fn find_line_start(&self, _o: i32) -> i32 { 0 }
        fn find_line_end(&self, _o: i32) -> i32 { 0 }
        fn get_attr_list_for_range(&self, _o: i32, _l: i32) -> pango::AttrList { pango::AttrList::new() }
        fn set_graphic_size(&self, _go: i32, _w: i32, _h: i32) {}
        fn get_text_range(&self, _o: i32, _l: i32) -> String { String::new() }
    }

    fn make_run(offset: i32, length: i32, link_url: Option<&str>) -> StyleRun {
        StyleRun {
            offset,
            length,
            bold: false,
            italic: false,
            underline: false,
            color: gdk4::RGBA::new(0.0, 0.0, 0.0, 1.0),
            font_size: 12,
            font_family: None,
            is_graphic: false,
            graphic: None,
            link_url: link_url.map(String::from),
        }
    }

    #[test]
    fn link_extent_single_run() {
        let doc = MockDoc {
            style_runs: vec![make_run(5, 10, Some("https://example.com"))],
        };
        let result = find_link_extent(&doc, 7);
        assert_eq!(result, Some((5, 15, "https://example.com".to_string())));
    }

    #[test]
    fn link_extent_contiguous_runs() {
        let doc = MockDoc {
            style_runs: vec![
                make_run(0, 5, Some("https://example.com")),
                make_run(5, 5, Some("https://example.com")),
                make_run(10, 5, Some("https://example.com")),
            ],
        };
        let result = find_link_extent(&doc, 7);
        assert_eq!(result, Some((0, 15, "https://example.com".to_string())));
    }

    #[test]
    fn link_extent_no_link() {
        let doc = MockDoc {
            style_runs: vec![make_run(0, 10, None)],
        };
        let result = find_link_extent(&doc, 5);
        assert!(result.is_none());
    }

    #[test]
    fn link_extent_different_urls_no_merge() {
        let doc = MockDoc {
            style_runs: vec![
                make_run(0, 5, Some("https://a.com")),
                make_run(5, 5, Some("https://b.com")),
            ],
        };
        let result = find_link_extent(&doc, 2);
        assert_eq!(result, Some((0, 5, "https://a.com".to_string())));
    }

    // == compute_navigation (uses MockDoc) ==

    struct NavDoc {
        text: String,
    }

    impl DocumentApi for NavDoc {
        fn get_text(&self) -> Option<String> {
            Some(self.text.clone())
        }
        fn get_length(&self) -> i32 {
            self.text.chars().count() as i32
        }
        fn get_style_runs(&self) -> Vec<StyleRun> {
            vec![]
        }
        fn delete_range(&self, _o: i32, _l: i32) {}
        fn insert_text(&self, _o: i32, _t: &str) {}
        fn undo(&self) {}
        fn redo(&self) {}
        fn get_link_at(&self, _o: i32) -> Option<String> {
            None
        }
        fn find_word_boundary_left(&self, o: i32) -> i32 {
            let chars: Vec<char> = self.text.chars().collect();
            let mut p = o;
            if p > 0 {
                p -= 1;
            }
            while p > 0 && chars[p as usize].is_alphanumeric() {
                p -= 1;
            }
            if !chars[p as usize].is_alphanumeric() && p < o - 1 {
                p + 1
            } else {
                p
            }
        }
        fn find_word_boundary_right(&self, o: i32) -> i32 {
            let chars: Vec<char> = self.text.chars().collect();
            let len = chars.len() as i32;
            let mut p = o;
            while p < len && chars[p as usize].is_alphanumeric() {
                p += 1;
            }
            while p < len && !chars[p as usize].is_alphanumeric() {
                p += 1;
            }
            p
        }
        fn find_line_start(&self, o: i32) -> i32 {
            let chars: Vec<char> = self.text.chars().collect();
            let mut p = o;
            while p > 0 && chars[(p - 1) as usize] != '\n' {
                p -= 1;
            }
            p
        }
        fn find_line_end(&self, o: i32) -> i32 {
            let chars: Vec<char> = self.text.chars().collect();
            let len = chars.len() as i32;
            let mut p = o;
            while p < len && chars[p as usize] != '\n' {
                p += 1;
            }
            p
        }
        fn get_attr_list_for_range(&self, _offset: i32, _length: i32) -> pango::AttrList {
            pango::AttrList::new()
        }
        fn set_graphic_size(&self, _graphic_offset: i32, _width: i32, _height: i32) {}
        fn get_text_range(&self, offset: i32, length: i32) -> String {
            let chars: Vec<char> = self.text.chars().collect();
            let start = offset as usize;
            let end = (offset + length).min(chars.len() as i32) as usize;
            chars[start..end].iter().collect()
        }
    }

    fn make_state(caret: i32) -> EditorState {
        let mut s = EditorState::default();
        s.caret = caret;
        s
    }

    #[test]
    fn nav_ctrl_home() {
        let doc = NavDoc {
            text: "hello\nworld".into(),
        };
        let s = make_state(8);
        assert_eq!(compute_navigation(&s, &doc, NavKind::CtrlHome, false), 0);
    }

    #[test]
    fn nav_ctrl_end() {
        let doc = NavDoc {
            text: "hello\nworld".into(),
        };
        let s = make_state(2);
        assert_eq!(compute_navigation(&s, &doc, NavKind::CtrlEnd, false), 11);
    }

    #[test]
    fn nav_home() {
        let doc = NavDoc {
            text: "hello\nworld".into(),
        };
        let s = make_state(8);
        assert_eq!(compute_navigation(&s, &doc, NavKind::Home, false), 6);
    }

    #[test]
    fn nav_end() {
        let doc = NavDoc {
            text: "hello\nworld".into(),
        };
        let s = make_state(2);
        assert_eq!(compute_navigation(&s, &doc, NavKind::End, false), 5);
    }

    #[test]
    fn nav_left_no_selection() {
        let doc = NavDoc {
            text: "hello".into(),
        };
        let s = make_state(3);
        assert_eq!(compute_navigation(&s, &doc, NavKind::Left, false), 2);
    }

    #[test]
    fn nav_left_at_start() {
        let doc = NavDoc {
            text: "hello".into(),
        };
        let s = make_state(0);
        assert_eq!(compute_navigation(&s, &doc, NavKind::Left, false), 0);
    }

    #[test]
    fn nav_left_collapses_selection() {
        let doc = NavDoc {
            text: "hello".into(),
        };
        let mut s = make_state(4);
        s.sel_start = 2;
        s.sel_end = 4;
        // Without shift, Left collapses to sel_start
        assert_eq!(compute_navigation(&s, &doc, NavKind::Left, false), 2);
    }

    #[test]
    fn nav_right_no_selection() {
        let doc = NavDoc {
            text: "hello".into(),
        };
        let s = make_state(3);
        assert_eq!(compute_navigation(&s, &doc, NavKind::Right, false), 4);
    }

    #[test]
    fn nav_right_at_end() {
        let doc = NavDoc {
            text: "hello".into(),
        };
        let s = make_state(5);
        assert_eq!(compute_navigation(&s, &doc, NavKind::Right, false), 5);
    }

    #[test]
    fn nav_right_collapses_selection() {
        let doc = NavDoc {
            text: "hello".into(),
        };
        let mut s = make_state(2);
        s.sel_start = 2;
        s.sel_end = 4;
        assert_eq!(compute_navigation(&s, &doc, NavKind::Right, false), 4);
    }

    #[test]
    fn nav_up() {
        let doc = NavDoc {
            text: "abc\ndef\nghi".into(),
        };
        let s = make_state(5); // in "def"
        let new = compute_navigation(&s, &doc, NavKind::Up, false);
        // Should move to somewhere in "abc" (before the \n at 3)
        assert!(new <= 3);
    }

    #[test]
    fn nav_up_at_first_line() {
        let doc = NavDoc {
            text: "hello".into(),
        };
        let s = make_state(3);
        assert_eq!(compute_navigation(&s, &doc, NavKind::Up, false), 0);
    }

    #[test]
    fn nav_down() {
        let doc = NavDoc {
            text: "abc\ndef\nghi".into(),
        };
        let s = make_state(1); // in "abc"
        let new = compute_navigation(&s, &doc, NavKind::Down, false);
        // Should move past the first \n
        assert!(new >= 4);
    }

    #[test]
    fn nav_down_at_last_line() {
        let doc = NavDoc {
            text: "hello".into(),
        };
        let s = make_state(2);
        assert_eq!(compute_navigation(&s, &doc, NavKind::Down, false), 5);
    }

    #[test]
    fn nav_page_up_moves_multiple_lines() {
        let mut text = String::new();
        for i in 0..30 {
            if i > 0 {
                text.push('\n');
            }
            text.push_str("line");
        }
        let doc = NavDoc { text };
        let s = make_state(100);
        let new = compute_navigation(&s, &doc, NavKind::PageUp, false);
        assert!(new < 100);
    }

    #[test]
    fn nav_page_down_moves_multiple_lines() {
        let mut text = String::new();
        for i in 0..30 {
            if i > 0 {
                text.push('\n');
            }
            text.push_str("line");
        }
        let doc = NavDoc { text };
        let s = make_state(10);
        let new = compute_navigation(&s, &doc, NavKind::PageDown, false);
        assert!(new > 10);
    }

    // == on_key_pressed (with NavDoc mock) ==

    #[test]
    fn key_insert_char() {
        let mut doc = NavDoc {
            text: "hello".into(),
        };
        let mut state = EditorState::default();
        state.caret = 5;
        state.sel_start = 5;
        state.sel_end = 5;

        let consumed = on_key_pressed(&mut state, &mut doc, Key::a, ModifierType::empty());
        assert!(consumed);
        assert_eq!(state.caret, 6);
    }

    #[test]
    fn key_backspace_with_selection() {
        let mut doc = NavDoc {
            text: "hello".into(),
        };
        let mut state = EditorState::default();
        state.caret = 4;
        state.sel_start = 2;
        state.sel_end = 4;

        let consumed =
            on_key_pressed(&mut state, &mut doc, Key::BackSpace, ModifierType::empty());
        assert!(consumed);
        assert_eq!(state.caret, 2);
        assert_eq!(state.sel_start, 2);
        assert_eq!(state.sel_end, 2);
    }

    #[test]
    fn key_select_all() {
        let mut doc = NavDoc {
            text: "hello world".into(),
        };
        let mut state = EditorState::default();
        state.caret = 3;

        let consumed =
            on_key_pressed(&mut state, &mut doc, Key::a, ModifierType::CONTROL_MASK);
        assert!(consumed);
        assert_eq!(state.sel_start, 0);
        assert_eq!(state.sel_end, 11);
        assert_eq!(state.caret, 11);
    }

    #[test]
    fn key_copy_readonly() {
        let mut doc = NavDoc {
            text: "hello".into(),
        };
        let mut state = EditorState::default();
        state.editable = false;
        state.sel_start = 0;
        state.sel_end = 5;

        let consumed =
            on_key_pressed(&mut state, &mut doc, Key::c, ModifierType::CONTROL_MASK);
        assert!(consumed);
        assert!(state.pending_copy);
    }

    #[test]
    fn key_char_blocked_readonly() {
        let mut doc = NavDoc {
            text: "hello".into(),
        };
        let mut state = EditorState::default();
        state.editable = false;
        state.caret = 5;

        let consumed = on_key_pressed(&mut state, &mut doc, Key::a, ModifierType::empty());
        assert!(!consumed);
        // Caret should not have moved
        assert_eq!(state.caret, 5);
    }

    #[test]
    fn key_arrow_shift_extends_selection() {
        let mut doc = NavDoc {
            text: "hello".into(),
        };
        let mut state = EditorState::default();
        state.caret = 3;
        state.sel_start = 3;
        state.sel_end = 3;

        let consumed = on_key_pressed(
            &mut state,
            &mut doc,
            Key::Right,
            ModifierType::SHIFT_MASK,
        );
        assert!(consumed);
        assert_eq!(state.caret, 4);
        assert_eq!(state.sel_start, 3);
        assert_eq!(state.sel_end, 4);
        assert_eq!(state.sel_anchor, 3);
    }

    #[test]
    fn key_newline_inserts() {
        let mut doc = NavDoc {
            text: "hello".into(),
        };
        let mut state = EditorState::default();
        state.caret = 5;
        state.sel_start = 5;
        state.sel_end = 5;

        let consumed =
            on_key_pressed(&mut state, &mut doc, Key::Return, ModifierType::empty());
        assert!(consumed);
        assert_eq!(state.caret, 6);
    }

    #[test]
    fn key_tab_inserts() {
        let mut doc = NavDoc {
            text: "hello".into(),
        };
        let mut state = EditorState::default();
        state.caret = 0;
        state.sel_start = 0;
        state.sel_end = 0;

        let consumed =
            on_key_pressed(&mut state, &mut doc, Key::Tab, ModifierType::empty());
        assert!(consumed);
        assert_eq!(state.caret, 1);
    }

    #[test]
    fn key_delete_at_caret() {
        let mut doc = NavDoc {
            text: "hello".into(),
        };
        let mut state = EditorState::default();
        state.caret = 2;
        state.sel_start = 2;
        state.sel_end = 2;

        let consumed =
            on_key_pressed(&mut state, &mut doc, Key::Delete, ModifierType::empty());
        assert!(consumed);
        // Caret should stay at 2 (the char at 2 was deleted)
        assert_eq!(state.caret, 2);
    }

    #[test]
    fn key_undo_redo() {
        let mut doc = NavDoc {
            text: "hello".into(),
        };
        let mut state = EditorState::default();

        let consumed =
            on_key_pressed(&mut state, &mut doc, Key::z, ModifierType::CONTROL_MASK);
        assert!(consumed); // undo consumed

        let consumed =
            on_key_pressed(&mut state, &mut doc, Key::y, ModifierType::CONTROL_MASK);
        assert!(consumed); // redo consumed
    }
}
