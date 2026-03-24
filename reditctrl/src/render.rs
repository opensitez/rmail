//! Cairo/Pango rendering engine for rEditCtrl.
//!
//! Handles paragraph-by-paragraph rendering with viewport clipping,
//! quote marks, bullet points, alignment, style attributes, selection
//! highlighting, caret drawing, inline image rendering, image selection
//! borders with resize handles, horizontal rules, page breaks, and
//! scroll-to-caret logic.
//!
//! Port of gEditCtrl/gedit-render.c (896 lines).

use pangocairo::cairo;
use cairo::Context as CairoContext;
use gdk4::prelude::{TextureExt, TextureExtManual};
use pango::{Alignment as PangoAlignment, AttrList, Layout, TabArray, WrapMode};
use std::f64::consts::PI;

use crate::document::{
    Alignment, BulletType, Direction, Document, Graphic, ParaAttr,
};
use crate::state::EditorState;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Left/right margin around content.
const MARGIN: f64 = 6.0;

/// Default line height when a paragraph has zero pixel height.
const DEFAULT_LINE_HEIGHT: f64 = 14.0;

/// Width per quote-level indent, in pixels.
const QUOTE_LEVEL_WIDTH: f64 = 12.0;

/// Bullet indent width in pixels.
const BULLET_INDENT: f64 = 12.0;

/// Quote bar width in pixels.
const QUOTE_BAR_WIDTH: f64 = 2.0;

/// Quote bar spacing from the left edge of its slot.
const QUOTE_BAR_SPACING: f64 = 4.0;

/// Tab stop interval in pixels.
const TAB_INTERVAL: i32 = 40;

/// Number of tab stops to create.
const TAB_COUNT: i32 = 10;

/// Horizontal rule total height (including padding).
const HR_HEIGHT: f64 = 14.0;

/// Page break total height.
const PAGE_BREAK_HEIGHT: f64 = 24.0;

/// Resize handle size in pixels.
const HANDLE_SIZE: f64 = 8.0;

// ─── Quote-mark colors ──────────────────────────────────────────────────────

/// Standard email quote colors cycling through 5 levels:
/// Blue, Green, Maroon, Red, Purple.
const QUOTE_COLORS: [(f64, f64, f64); 5] = [
    (0.0, 0.0, 0.8), // Blue
    (0.0, 0.5, 0.0), // Green
    (0.5, 0.0, 0.0), // Maroon
    (0.8, 0.0, 0.0), // Red
    (0.5, 0.0, 0.5), // Purple
];

// ─── Color helper ────────────────────────────────────────────────────────────

/// An RGBA color with f64 components in [0.0, 1.0].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

impl Color {
    pub const WHITE: Color = Color {
        red: 1.0,
        green: 1.0,
        blue: 1.0,
        alpha: 1.0,
    };
    pub const BLACK: Color = Color {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        alpha: 1.0,
    };
    pub const fn new(red: f64, green: f64, blue: f64, alpha: f64) -> Self {
        Color {
            red,
            green,
            blue,
            alpha,
        }
    }
}

// ─── Theme ──────────────────────────────────────────────────────────────────

/// Optional theme colors for the editor.
#[derive(Clone, Debug)]
pub struct ThemeColors {
    pub bg: Color,
    pub text: Color,
    pub caret: Color,
    pub sel_bg: Color,
}

// ─── Viewport ───────────────────────────────────────────────────────────────

/// Visible viewport bounds (scroll position + page size).
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub top: f64,
    pub bottom: f64,
}

impl Viewport {
    pub fn new(top: f64, bottom: f64) -> Self {
        Viewport { top, bottom }
    }

    /// Returns `true` if the rectangle [y, y + height) overlaps this viewport.
    /// Matches the C condition: `y + para_h >= vis_top && y <= vis_bottom`.
    #[inline]
    pub fn overlaps(&self, y: f64, height: f64) -> bool {
        y + height >= self.top && y <= self.bottom
    }

    /// Returns `true` if `y` is past (below) the viewport bottom.
    /// Matches the C early-exit: `if (y > vis_bottom) break;`.
    #[inline]
    pub fn past_bottom(&self, y: f64) -> bool {
        y > self.bottom
    }
}

// ─── Paragraph splitting ────────────────────────────────────────────────────

/// A single paragraph extracted from the document text, with offsets.
#[derive(Clone, Debug, PartialEq)]
pub struct Paragraph {
    /// UTF-8 text of this paragraph (excluding the trailing newline).
    pub text: String,
    /// Character offset of the first char of this paragraph in the full text.
    pub char_offset: usize,
    /// Number of characters in the paragraph text (not counting the newline).
    pub char_count: usize,
    /// Byte offset in the full text where this paragraph starts.
    pub byte_offset: usize,
    /// Byte length of the paragraph text (not counting the newline).
    pub byte_len: usize,
}

/// Split document text into paragraphs on `'\n'` boundaries.
///
/// Each paragraph excludes its trailing newline.  The newline itself is
/// consumed (it advances the running char/byte offset) but not included
/// in any `Paragraph::text`.
///
/// Matches the C loop that walks `cur` / `cur_ptr` splitting on `'\n'`.
pub fn split_paragraphs(text: &str) -> Vec<Paragraph> {
    let mut paras = Vec::new();
    let mut char_offset: usize = 0;
    let mut byte_offset: usize = 0;

    for line in text.split('\n') {
        let char_count = line.chars().count();
        let byte_len = line.len();
        paras.push(Paragraph {
            text: line.to_owned(),
            char_offset,
            char_count,
            byte_offset,
            byte_len,
        });
        // Advance past the paragraph text + the newline separator.
        char_offset += char_count + 1;
        byte_offset += byte_len + 1;
    }

    paras
}

// ─── Char-to-byte within a paragraph ────────────────────────────────────────

/// Convert a character offset (relative to the paragraph start) to a byte
/// offset within the paragraph text.  Returns `text.len()` if `char_idx`
/// is at or past the end.
#[inline]
fn char_to_byte_in_para(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

// ─── Layout helpers ─────────────────────────────────────────────────────────

/// Create a `PangoLayout` for a paragraph with word-char wrapping and tab stops.
///
/// Equivalent to the C `gedit_layout_for_paragraph`.
pub fn layout_for_paragraph(
    pango_ctx: &pango::Context,
    text: &str,
    width: i32,
) -> Layout {
    let pl = Layout::new(pango_ctx);
    pl.set_text(if text.is_empty() { "" } else { text });
    pl.set_width(width.max(10) * pango::SCALE);
    pl.set_wrap(WrapMode::WordChar);

    let mut tabs = TabArray::new(TAB_COUNT, false);
    for i in 0..TAB_COUNT {
        tabs.set_tab(
            i,
            pango::TabAlign::Left,
            (i + 1) * TAB_INTERVAL * pango::SCALE,
        );
    }
    pl.set_tabs(Some(&tabs));
    pl
}

/// Create a layout with document style attributes applied (shapes, styles).
///
/// Equivalent to the C `gedit_layout_for_paragraph_with_attrs`.
pub fn layout_for_paragraph_with_attrs(
    pango_ctx: &pango::Context,
    doc: &Document,
    text: &str,
    width: i32,
    char_offset: i32,
    para_chars: i32,
) -> Layout {
    let pl = layout_for_paragraph(pango_ctx, text, width);
    let alist = doc.get_attr_list_for_range(char_offset, para_chars);
    pl.set_attributes(Some(&alist));
    pl
}

// ─── Quote marks ────────────────────────────────────────────────────────────

/// Draw colored vertical quote bars for the given quote level.
fn draw_quote_marks(cr: &CairoContext, x: f64, y: f64, height: f64, level: i32) {
    if level <= 0 {
        return;
    }
    cr.save().expect("cairo save");
    for i in 0..level {
        let (r, g, b) = QUOTE_COLORS[(i as usize) % QUOTE_COLORS.len()];
        cr.set_source_rgb(r, g, b);
        let bx = x + (i as f64 * QUOTE_LEVEL_WIDTH) + QUOTE_BAR_SPACING;
        cr.rectangle(bx, y, QUOTE_BAR_WIDTH, height);
        let _ = cr.fill();
    }
    cr.restore().expect("cairo restore");
}

// ─── Bullet drawing ─────────────────────────────────────────────────────────

/// Draw a bullet glyph at the given center coordinates.
fn draw_bullet(cr: &CairoContext, bullet: BulletType, cx: f64, cy: f64) {
    match bullet {
        BulletType::Circle => {
            cr.arc(cx, cy, 3.0, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
        BulletType::Square => {
            cr.rectangle(cx - 3.0, cy - 3.0, 6.0, 6.0);
            let _ = cr.fill();
        }
        BulletType::Disk => {
            cr.arc(cx, cy, 4.0, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
        BulletType::None => {}
    }
}

// ─── Selection attribute helpers ────────────────────────────────────────────

/// Add selection highlight (background + foreground) Pango attributes to an
/// AttrList for the byte range `[rel_start, rel_end)` within a paragraph.
fn apply_selection_attrs(
    attrs: &AttrList,
    rel_start: u32,
    rel_end: u32,
    sel_bg: Option<Color>,
    sel_fg: Option<Color>,
) {
    let (bg_r, bg_g, bg_b) = match sel_bg {
        Some(c) => (
            (c.red * 65535.0) as u16,
            (c.green * 65535.0) as u16,
            (c.blue * 65535.0) as u16,
        ),
        None => (45000, 52000, 65000),
    };
    let mut bg = pango::AttrColor::new_background(bg_r, bg_g, bg_b);
    bg.set_start_index(rel_start);
    bg.set_end_index(rel_end);
    attrs.insert(bg);

    let (fg_r, fg_g, fg_b) = match sel_fg {
        Some(c) => (
            (c.red * 65535.0) as u16,
            (c.green * 65535.0) as u16,
            (c.blue * 65535.0) as u16,
        ),
        None => (0, 0, 0),
    };
    let mut fg = pango::AttrColor::new_foreground(fg_r, fg_g, fg_b);
    fg.set_start_index(rel_start);
    fg.set_end_index(rel_end);
    attrs.insert(fg);
}

// ─── Inline image rendering ─────────────────────────────────────────────────

/// Render an inline image (or placeholder box) at the given position.
fn draw_inline_image(cr: &CairoContext, graphic: &Graphic, gx: f64, gy: f64) {
    let gw = graphic.width as f64;
    let gh = graphic.height as f64;

    if let Some(ref draw_func) = graphic.draw_func {
        // Custom-draw graphic (charts, graphs, etc.)
        cr.save().expect("cairo save");
        cr.translate(gx, gy);
        draw_func(cr, graphic.width, graphic.height);
        cr.restore().expect("cairo restore");
    } else if let Some(ref texture) = graphic.texture {
        let tw = texture.width() as f64;
        let th = texture.height() as f64;

        // Download texture pixels into a cairo image surface.
        // gdk4 provides Texture::download() for raw ARGB32 pixel data.
        let mut img =
            cairo::ImageSurface::create(cairo::Format::ARgb32, tw as i32, th as i32)
                .expect("create image surface");
        let stride = img.stride() as usize;
        {
            let mut data = img.data().expect("surface data");
            texture.download(data.as_mut(), stride);
        }
        // data borrow is released; mark surface dirty so cairo picks up changes.
        img.mark_dirty();

        // Scale and paint.
        cr.save().expect("cairo save");
        cr.translate(gx, gy);
        let sw = gw / tw;
        let sh = gh / th;
        cr.scale(sw, sh);
        cr.set_source_surface(&img, 0.0, 0.0)
            .expect("set source surface");
        let _ = cr.paint();
        cr.restore().expect("cairo restore");
    } else {
        // Placeholder: gray box with border.
        cr.save().expect("cairo save");
        cr.set_source_rgb(0.7, 0.7, 0.7);
        cr.rectangle(gx, gy, gw, gh);
        let _ = cr.fill();
        cr.set_source_rgb(0.4, 0.4, 0.4);
        cr.set_line_width(1.0);
        cr.rectangle(gx, gy, gw, gh);
        let _ = cr.stroke();
        cr.restore().expect("cairo restore");
    }
}

/// Draw a blue selection border and resize handles around a selected graphic.
fn draw_graphic_selection(cr: &CairoContext, gx: f64, gy: f64, gw: f64, gh: f64) {
    cr.save().expect("cairo save");

    // Blue selection border.
    cr.set_source_rgb(0.2, 0.5, 1.0);
    cr.set_line_width(2.0);
    cr.rectangle(gx - 1.0, gy - 1.0, gw + 2.0, gh + 2.0);
    let _ = cr.stroke();

    // Bottom-right resize handle (8x8).
    let hx = gx + gw - 4.0;
    let hy = gy + gh - 4.0;
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.rectangle(hx, hy, HANDLE_SIZE, HANDLE_SIZE);
    let _ = cr.fill();
    cr.set_source_rgb(0.2, 0.5, 1.0);
    cr.set_line_width(1.5);
    cr.rectangle(hx, hy, HANDLE_SIZE, HANDLE_SIZE);
    let _ = cr.stroke();

    // Handles at the other three corners.
    let corners: [(f64, f64); 3] = [
        (gx - 4.0, gy - 4.0),      // top-left
        (gx + gw - 4.0, gy - 4.0), // top-right
        (gx - 4.0, gy + gh - 4.0), // bottom-left
    ];
    for (cx, cy) in &corners {
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.rectangle(*cx, *cy, HANDLE_SIZE, HANDLE_SIZE);
        let _ = cr.fill();
        cr.set_source_rgb(0.2, 0.5, 1.0);
        cr.rectangle(*cx, *cy, HANDLE_SIZE, HANDLE_SIZE);
        let _ = cr.stroke();
    }

    cr.restore().expect("cairo restore");
}

// ─── Caret drawing ──────────────────────────────────────────────────────────

/// Draw the text caret (blinking cursor) as a vertical line.
fn draw_caret(cr: &CairoContext, x: f64, y: f64, height: f64, color: Option<Color>) {
    let c = color.unwrap_or(Color::BLACK);
    cr.set_source_rgba(c.red, c.green, c.blue, c.alpha);
    cr.set_line_width(1.0);
    cr.move_to(x, y);
    cr.line_to(x, y + height);
    let _ = cr.stroke();
}

// ─── Horizontal rule ────────────────────────────────────────────────────────

fn draw_hr(cr: &CairoContext, x_base: f64, y: f64, width: f64) {
    cr.set_source_rgb(0.5, 0.5, 0.5);
    cr.set_line_width(1.0);
    cr.move_to(x_base + 6.0, y + 7.0);
    cr.line_to(x_base + width - 6.0, y + 7.0);
    let _ = cr.stroke();
}

// ─── Page break ─────────────────────────────────────────────────────────────

fn draw_page_break(
    cr: &CairoContext,
    pango_ctx: &pango::Context,
    x_base: f64,
    y: f64,
    width: f64,
) {
    cr.set_source_rgb(0.7, 0.7, 0.7);
    cr.set_line_width(2.0);
    let dashes = [5.0, 5.0];
    cr.set_dash(&dashes, 0.0);
    cr.move_to(x_base + 6.0, y + 10.0);
    cr.line_to(x_base + width - 6.0, y + 10.0);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);

    cr.set_source_rgb(0.7, 0.7, 0.7);
    cr.move_to(x_base + 10.0, y + 5.0);
    let pb_layout = Layout::new(pango_ctx);
    pb_layout.set_text("Page Break");
    pangocairo::functions::show_layout(cr, &pb_layout);
}

// ─── Content height / measurement ───────────────────────────────────────────

/// Parameters needed for height/layout calculation without drawing.
pub struct MeasureParams<'a> {
    pub pango_ctx: &'a pango::Context,
    pub doc: &'a Document,
    pub width: i32,
}

/// Information about a single paragraph's layout, used during measurement.
#[derive(Clone, Debug)]
pub struct ParaLayout {
    pub y: f64,
    pub height: f64,
    pub char_offset: usize,
    pub char_count: usize,
}

/// Measure all paragraphs and return per-paragraph layout info plus total y.
///
/// This is the shared logic behind `gedit_doc_changed_cb` (content height),
/// `gedit_scroll_to_caret`, and `geditctrl_measure_content`.
pub fn measure_paragraphs(params: &MeasureParams) -> (Vec<ParaLayout>, f64) {
    let text = match params.doc.get_text_opt() {
        Some(t) => t,
        None => return (Vec::new(), MARGIN),
    };
    let paras = split_paragraphs(&text);
    let mut layouts = Vec::with_capacity(paras.len());
    let mut y = MARGIN;

    for para in &paras {
        let pattr = params.doc.get_para_attr(
            para.char_offset as i32,
            para.char_count.max(1) as i32,
        );

        let para_h = if pattr.is_hr {
            HR_HEIGHT
        } else if pattr.page_break {
            PAGE_BREAK_HEIGHT
        } else {
            let w = compute_layout_width(params.width, &pattr);
            let pl = layout_for_paragraph_with_attrs(
                params.pango_ctx,
                params.doc,
                &para.text,
                w,
                para.char_offset as i32,
                para.char_count as i32,
            );
            let (_, pxh) = pl.pixel_size();
            if pxh > 0 {
                pxh as f64
            } else {
                DEFAULT_LINE_HEIGHT
            }
        };

        layouts.push(ParaLayout {
            y,
            height: para_h,
            char_offset: para.char_offset,
            char_count: para.char_count,
        });

        y += para_h;
    }

    (layouts, y)
}

/// Total content height (for scrollbar).  Adds top + bottom margin.
pub fn content_height(params: &MeasureParams) -> i32 {
    let (_, y) = measure_paragraphs(params);
    (y + MARGIN) as i32
}

// ─── Viewport clipping helpers ──────────────────────────────────────────────

/// Compute the effective layout width after accounting for quote level and bullet.
pub fn compute_layout_width(avail: i32, pattr: &ParaAttr) -> i32 {
    let mut w = avail;
    if pattr.quote_level > 0 {
        w -= pattr.quote_level * (QUOTE_LEVEL_WIDTH as i32);
    }
    if pattr.bullet != BulletType::None {
        w = (w - BULLET_INDENT as i32).max(10);
    }
    w
}

/// Determine which paragraphs from a measured layout list are visible.
/// Returns indices into the `layouts` slice.
pub fn visible_paragraph_indices(layouts: &[ParaLayout], viewport: &Viewport) -> Vec<usize> {
    layouts
        .iter()
        .enumerate()
        .filter(|(_, pl)| viewport.overlaps(pl.y, pl.height))
        .map(|(i, _)| i)
        .collect()
}

// ─── Scroll-to-caret ────────────────────────────────────────────────────────

/// Result of a scroll-to-caret calculation.
#[derive(Clone, Debug)]
pub struct ScrollResult {
    /// Content height for the drawing area.
    pub content_height: i32,
    /// The y-coordinate of the paragraph containing the caret.
    pub caret_y: Option<f64>,
    /// The height of the paragraph containing the caret.
    pub caret_height: f64,
}

/// Calculate the caret's y position and total content height.
///
/// This is the pure-logic core of `gedit_scroll_to_caret` -- it does not
/// touch any widgets, only measures paragraphs and locates the caret.
pub fn compute_scroll_to_caret(params: &MeasureParams, caret: usize) -> ScrollResult {
    let (layouts, max_y) = measure_paragraphs(params);
    let ch = (max_y + MARGIN) as i32;

    let mut caret_y: Option<f64> = None;
    let mut caret_height = DEFAULT_LINE_HEIGHT;

    for pl in &layouts {
        if caret >= pl.char_offset && caret <= pl.char_offset + pl.char_count {
            caret_y = Some(pl.y);
            caret_height = pl.height;
            break;
        }
    }

    ScrollResult {
        content_height: ch,
        caret_y,
        caret_height,
    }
}

/// Given the scroll result and current scroll state, compute the new
/// scroll value.
///
/// Returns `Some(new_value)` if scrolling is needed, `None` if the caret
/// is already visible or was not found.
pub fn compute_scroll_adjustment(
    result: &ScrollResult,
    current_value: f64,
    page_size: f64,
) -> Option<f64> {
    let caret_y = result.caret_y?;
    if page_size <= 0.0 {
        return None;
    }

    let visible = caret_y >= current_value
        && caret_y + result.caret_height <= current_value + page_size;

    if visible {
        return None;
    }

    let upper = result.content_height as f64;

    if caret_y < current_value {
        // Caret is above visible area.
        Some((caret_y - 10.0).max(0.0))
    } else {
        // Caret is below visible area.
        let new_val = caret_y + result.caret_height - page_size + 10.0;
        Some(new_val.min(upper - page_size))
    }
}

// ─── Main draw callback ─────────────────────────────────────────────────────

/// Full rendering parameters for the interactive editor draw callback.
pub struct DrawParams<'a> {
    pub cr: &'a CairoContext,
    pub pango_ctx: &'a pango::Context,
    pub doc: &'a Document,
    pub state: Option<&'a EditorState>,
    pub width: i32,
    pub height: i32,
    pub viewport: Viewport,
    pub theme: Option<&'a ThemeColors>,
}

/// Main draw callback -- renders the entire visible document.
///
/// This is the Rust equivalent of `gedit_draw_cb`.  It renders
/// paragraph-by-paragraph, drawing only those that overlap the viewport.
pub fn draw(params: &DrawParams) {
    let cr = params.cr;
    let doc = params.doc;
    let state = params.state;
    let theme = params.theme;

    // Paint background.
    cr.save().expect("cairo save");
    match theme {
        Some(t) => cr.set_source_rgba(t.bg.red, t.bg.green, t.bg.blue, t.bg.alpha),
        None => cr.set_source_rgb(1.0, 1.0, 1.0),
    }
    let _ = cr.paint();

    let full_text = match doc.get_text_opt() {
        Some(t) => t,
        None => {
            // Draw trailing caret for empty document.
            if let Some(s) = state {
                if s.caret_visible {
                    let caret_color = theme.map(|t| t.caret);
                    draw_caret(cr, MARGIN, MARGIN, DEFAULT_LINE_HEIGHT, caret_color);
                }
            }
            cr.restore().expect("cairo restore");
            return;
        }
    };

    let x_base = MARGIN;
    let mut y = MARGIN;
    let avail = (params.width - 12).max(10);

    let paras = split_paragraphs(&full_text);
    let all_style_runs = doc.get_style_runs();

    let mut caret_drawn = false;

    for para in &paras {
        let pattr = doc.get_para_attr(
            para.char_offset as i32,
            para.char_count.max(1) as i32,
        );

        // ── Horizontal rule ──
        if pattr.is_hr {
            if params.viewport.overlaps(y, HR_HEIGHT) {
                draw_hr(cr, x_base, y, params.width as f64);
            }
            y += HR_HEIGHT;
            if params.viewport.past_bottom(y) {
                break;
            }
            continue;
        }

        // ── Page break ──
        if pattr.page_break {
            if params.viewport.overlaps(y, PAGE_BREAK_HEIGHT) {
                draw_page_break(cr, params.pango_ctx, x_base, y, params.width as f64);
            }
            y += PAGE_BREAK_HEIGHT;
            if params.viewport.past_bottom(y) {
                break;
            }
            continue;
        }

        // ── Normal paragraph ──
        let width_for_layout = compute_layout_width(avail, &pattr);

        let pl = layout_for_paragraph(params.pango_ctx, &para.text, width_for_layout);

        // Alignment.
        match pattr.alignment {
            Alignment::Center => pl.set_alignment(PangoAlignment::Center),
            Alignment::Right => pl.set_alignment(PangoAlignment::Right),
            Alignment::Left => pl.set_alignment(PangoAlignment::Left),
        }

        // RTL direction.
        if pattr.direction == Direction::Rtl {
            let ctx = pl.context();
            ctx.set_base_dir(pango::Direction::Rtl);
        }

        // Build draw attributes: document attrs + selection highlight.
        let alist = doc.get_attr_list_for_range(
            para.char_offset as i32,
            para.char_count as i32,
        );
        let draw_attrs = alist.clone();

        // Selection highlight.
        if let Some(s) = state {
            if s.sel_start != s.sel_end {
                let sa = s.sel_start.min(s.sel_end) as usize;
                let sb = s.sel_start.max(s.sel_end) as usize;
                let ia = sa.max(para.char_offset);
                let ib = sb.min(para.char_offset + para.char_count);
                if ib > ia {
                    let rel_start_chars = ia - para.char_offset;
                    let rel_end_chars = ib - para.char_offset;
                    let rel_start_bytes =
                        char_to_byte_in_para(&para.text, rel_start_chars) as u32;
                    let rel_end_bytes =
                        char_to_byte_in_para(&para.text, rel_end_chars) as u32;
                    let sel_bg = theme.map(|t| t.sel_bg);
                    let sel_fg = theme.map(|t| t.text);
                    apply_selection_attrs(
                        &draw_attrs,
                        rel_start_bytes,
                        rel_end_bytes,
                        sel_bg,
                        sel_fg,
                    );
                }
            }
        }

        pl.set_attributes(Some(&draw_attrs));

        let (_, pxh) = pl.pixel_size();
        let para_h = if pxh > 0 { pxh as f64 } else { DEFAULT_LINE_HEIGHT };
        let visible = params.viewport.overlaps(y, para_h);

        if !visible {
            // Off-screen: note caret presence for scroll-to-caret.
            if let Some(s) = state {
                if s.caret_visible
                    && (s.caret as usize) >= para.char_offset
                    && (s.caret as usize) <= para.char_offset + para.char_count
                {
                    caret_drawn = true;
                }
            }
            y += para_h;
            if params.viewport.past_bottom(y) {
                break;
            }
            continue;
        }

        // Get first line metrics for bullet/alignment offset.
        let (align_offset, line_height) = if let Some(first_line) = pl.line_readonly(0) {
            let l_ext = first_line.pixel_extents().1;
            (l_ext.x(), l_ext.height())
        } else {
            (0, pxh)
        };

        let mut x_para = x_base + pattr.indent as f64;

        // Quote marks.
        if pattr.quote_level > 0 {
            draw_quote_marks(cr, x_para, y, pxh as f64, pattr.quote_level);
            x_para += pattr.quote_level as f64 * QUOTE_LEVEL_WIDTH;
        }

        let mut x_draw = x_para;

        // Bullet.
        if pattr.bullet != BulletType::None {
            let bx = x_para + align_offset as f64 + 4.0;
            let by = y + (line_height as f64 / 2.0);
            match theme {
                Some(t) => {
                    cr.set_source_rgba(t.text.red, t.text.green, t.text.blue, t.text.alpha)
                }
                None => cr.set_source_rgb(0.0, 0.0, 0.0),
            }
            draw_bullet(cr, pattr.bullet, bx, by);
            x_draw += BULLET_INDENT;
        }

        // Draw paragraph text.
        cr.save().expect("cairo save");
        match theme {
            Some(t) => cr.set_source_rgba(t.text.red, t.text.green, t.text.blue, t.text.alpha),
            None => cr.set_source_rgb(0.0, 0.0, 0.0),
        }
        cr.translate(x_draw, y);
        pangocairo::functions::show_layout(cr, &pl);
        cr.restore().expect("cairo restore");

        // Draw inline graphics.
        for run in &all_style_runs {
            if run.is_graphic
                && (run.offset as usize) >= para.char_offset
                && (run.offset as usize) < para.char_offset + para.char_count
            {
                if let Some(ref graphic) = run.graphic {
                    let rel_chars = run.offset as usize - para.char_offset;
                    let rel_bytes = char_to_byte_in_para(&para.text, rel_chars) as i32;
                    let (crect, _) = pl.cursor_pos(rel_bytes);
                    let gx = x_draw + crect.x() as f64 / pango::SCALE as f64;
                    let gy = y + crect.y() as f64 / pango::SCALE as f64;

                    draw_inline_image(cr, graphic, gx, gy);

                    // Selection border + handles.
                    if let Some(s) = state {
                        if s.selected_graphic == run.offset {
                            draw_graphic_selection(
                                cr,
                                gx,
                                gy,
                                graphic.width as f64,
                                graphic.height as f64,
                            );
                        }
                    }
                }
            }
        }

        // Draw caret.
        if let Some(s) = state {
            let c = s.caret as usize;
            if s.caret_visible
                && c >= para.char_offset
                && c <= para.char_offset + para.char_count
            {
                let rel_chars = c - para.char_offset;
                let rel_bytes = char_to_byte_in_para(&para.text, rel_chars) as i32;
                let (crect, _) = pl.cursor_pos(rel_bytes);
                let cx = x_draw + crect.x() as f64 / pango::SCALE as f64;
                let cy = y + crect.y() as f64 / pango::SCALE as f64;
                let ch = crect.height() as f64 / pango::SCALE as f64;
                let caret_color = theme.map(|t| t.caret);
                draw_caret(cr, cx, cy, ch, caret_color);
                caret_drawn = true;
            }
        }

        y += para_h;
        if params.viewport.past_bottom(y) {
            break;
        }
    }

    // Draw trailing caret if not drawn (empty doc or caret after trailing newline).
    if let Some(s) = state {
        if s.caret_visible && !caret_drawn {
            let caret_color = theme.map(|t| t.caret);
            draw_caret(cr, x_base, y, DEFAULT_LINE_HEIGHT, caret_color);
        }
    }

    cr.restore().expect("cairo restore");
}

// ─── Print rendering ────────────────────────────────────────────────────────

/// Parameters for print rendering (no widget, no selection/caret).
pub struct PrintParams<'a> {
    pub cr: &'a CairoContext,
    pub pango_ctx: &'a pango::Context,
    pub doc: &'a Document,
    pub x: f64,
    pub y_start: f64,
    pub width: i32,
    pub skip_height: f64,
    pub max_height: f64,
}

/// Render styled document to a print cairo context.
///
/// Port of `geditctrl_render_to_cairo`.  Renders from `skip_height` into
/// the content, stopping after `max_height` points of page space.
///
/// Returns the content height consumed on this page.
pub fn render_to_cairo(params: &PrintParams) -> f64 {
    let cr = params.cr;
    let doc = params.doc;
    let full_text = match doc.get_text_opt() {
        Some(t) => t,
        None => return 0.0,
    };

    let mut y_content: f64 = 0.0;
    let mut y_page = params.y_start;
    let x_base = params.x;

    cr.save().expect("cairo save");
    cr.set_source_rgb(0.0, 0.0, 0.0);

    let paras = split_paragraphs(&full_text);

    for para in &paras {
        let pattr = doc.get_para_attr(
            para.char_offset as i32,
            para.char_count.max(1) as i32,
        );

        let para_h;

        if pattr.is_hr {
            para_h = HR_HEIGHT;
            if y_content + para_h > params.skip_height
                && y_page - params.y_start + para_h <= params.max_height
            {
                draw_hr(cr, x_base, y_page, params.width as f64);
                cr.set_source_rgb(0.0, 0.0, 0.0);
                y_page += para_h;
            }
        } else if pattr.page_break {
            para_h = PAGE_BREAK_HEIGHT;
            // Skip page breaks in print -- they are visual only.
        } else {
            let mut w = params.width;
            let mut x_para = x_base + pattr.indent as f64;
            if pattr.quote_level > 0 {
                w -= pattr.quote_level * (QUOTE_LEVEL_WIDTH as i32);
                x_para += pattr.quote_level as f64 * QUOTE_LEVEL_WIDTH;
            }
            if pattr.bullet != BulletType::None {
                w = (w - BULLET_INDENT as i32).max(10);
            }

            let pl = layout_for_paragraph_with_attrs(
                params.pango_ctx,
                doc,
                &para.text,
                w,
                para.char_offset as i32,
                para.char_count as i32,
            );

            match pattr.alignment {
                Alignment::Center => pl.set_alignment(PangoAlignment::Center),
                Alignment::Right => pl.set_alignment(PangoAlignment::Right),
                Alignment::Left => {}
            }

            let (_, pxh) = pl.pixel_size();
            para_h = if pxh > 0 { pxh as f64 } else { DEFAULT_LINE_HEIGHT };

            // Only render if within this page's visible range.
            if y_content + para_h > params.skip_height
                && y_page - params.y_start + para_h <= params.max_height
            {
                let mut x_draw = x_para;

                // Quote marks.
                if pattr.quote_level > 0 {
                    draw_quote_marks(
                        cr,
                        x_base + pattr.indent as f64,
                        y_page,
                        para_h,
                        pattr.quote_level,
                    );
                }

                // Bullet (print always uses circle for simplicity, matching C).
                if pattr.bullet != BulletType::None {
                    let line_h = if let Some(first_line) = pl.line_readonly(0) {
                        let l_ext = first_line.pixel_extents().1;
                        l_ext.height() as f64
                    } else {
                        para_h
                    };
                    let bx = x_draw + 4.0;
                    let by = y_page + (line_h / 2.0);
                    cr.set_source_rgb(0.0, 0.0, 0.0);
                    cr.arc(bx, by, 3.0, 0.0, 2.0 * PI);
                    let _ = cr.fill();
                    x_draw += BULLET_INDENT;
                }

                // Text.
                cr.save().expect("cairo save");
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.translate(x_draw, y_page);
                pangocairo::functions::show_layout(cr, &pl);
                cr.restore().expect("cairo restore");

                // Inline images.
                let runs = doc.get_style_runs();
                for run in &runs {
                    if run.is_graphic
                        && (run.offset as usize) >= para.char_offset
                        && (run.offset as usize) < para.char_offset + para.char_count
                    {
                        if let Some(ref graphic) = run.graphic {
                            let rel_chars = run.offset as usize - para.char_offset;
                            let rel_bytes =
                                char_to_byte_in_para(&para.text, rel_chars) as i32;
                            let (crect, _) = pl.cursor_pos(rel_bytes);
                            let gx = x_draw + crect.x() as f64 / pango::SCALE as f64;
                            let gy = y_page + crect.y() as f64 / pango::SCALE as f64;
                            draw_inline_image(cr, graphic, gx, gy);
                        }
                    }
                }

                y_page += para_h;
            } else if y_content >= params.skip_height {
                // Past this page -- stop.
                break;
            }
        }

        y_content += para_h;
    }

    cr.restore().expect("cairo restore");
    y_page - params.y_start
}

/// Measure total content height for print pagination.
///
/// Port of `geditctrl_measure_content`.  Returns total height in points.
pub fn measure_content(params: &MeasureParams) -> f64 {
    let text = match params.doc.get_text_opt() {
        Some(t) => t,
        None => return 0.0,
    };

    let paras = split_paragraphs(&text);
    let mut y: f64 = 0.0;

    for para in &paras {
        let pattr = params.doc.get_para_attr(
            para.char_offset as i32,
            para.char_count.max(1) as i32,
        );

        if pattr.is_hr {
            y += HR_HEIGHT;
        } else if pattr.page_break {
            y += PAGE_BREAK_HEIGHT;
        } else {
            let w = compute_layout_width(params.width, &pattr);
            let pl = layout_for_paragraph_with_attrs(
                params.pango_ctx,
                params.doc,
                &para.text,
                w,
                para.char_offset as i32,
                para.char_count as i32,
            );
            let (_, pxh) = pl.pixel_size();
            y += if pxh > 0 { pxh as f64 } else { DEFAULT_LINE_HEIGHT };
        }
    }

    y
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── split_paragraphs ──

    #[test]
    fn split_empty_string() {
        let paras = split_paragraphs("");
        assert_eq!(paras.len(), 1);
        assert_eq!(paras[0].text, "");
        assert_eq!(paras[0].char_offset, 0);
        assert_eq!(paras[0].char_count, 0);
        assert_eq!(paras[0].byte_offset, 0);
        assert_eq!(paras[0].byte_len, 0);
    }

    #[test]
    fn split_single_line_no_newline() {
        let paras = split_paragraphs("hello world");
        assert_eq!(paras.len(), 1);
        assert_eq!(paras[0].text, "hello world");
        assert_eq!(paras[0].char_offset, 0);
        assert_eq!(paras[0].char_count, 11);
        assert_eq!(paras[0].byte_offset, 0);
        assert_eq!(paras[0].byte_len, 11);
    }

    #[test]
    fn split_two_lines() {
        let paras = split_paragraphs("hello\nworld");
        assert_eq!(paras.len(), 2);

        assert_eq!(paras[0].text, "hello");
        assert_eq!(paras[0].char_offset, 0);
        assert_eq!(paras[0].char_count, 5);
        assert_eq!(paras[0].byte_offset, 0);
        assert_eq!(paras[0].byte_len, 5);

        assert_eq!(paras[1].text, "world");
        assert_eq!(paras[1].char_offset, 6);
        assert_eq!(paras[1].char_count, 5);
        assert_eq!(paras[1].byte_offset, 6);
        assert_eq!(paras[1].byte_len, 5);
    }

    #[test]
    fn split_trailing_newline() {
        let paras = split_paragraphs("hello\n");
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].text, "hello");
        assert_eq!(paras[1].text, "");
        assert_eq!(paras[1].char_offset, 6);
        assert_eq!(paras[1].char_count, 0);
    }

    #[test]
    fn split_multiple_empty_lines() {
        let paras = split_paragraphs("\n\n\n");
        assert_eq!(paras.len(), 4);
        for (i, para) in paras.iter().enumerate() {
            assert_eq!(para.text, "");
            assert_eq!(para.char_offset, i);
            assert_eq!(para.char_count, 0);
        }
    }

    #[test]
    fn split_unicode_multibyte() {
        let paras = split_paragraphs("caf\u{00e9}\nna\u{00ef}f");
        assert_eq!(paras.len(), 2);

        assert_eq!(paras[0].text, "caf\u{00e9}");
        assert_eq!(paras[0].char_count, 4);
        assert_eq!(paras[0].byte_len, 5);

        assert_eq!(paras[1].text, "na\u{00ef}f");
        assert_eq!(paras[1].char_offset, 5);
        assert_eq!(paras[1].char_count, 4);
        assert_eq!(paras[1].byte_offset, 6);
        assert_eq!(paras[1].byte_len, 5);
    }

    #[test]
    fn split_cjk_emoji() {
        let paras = split_paragraphs("你好\n🎉");
        assert_eq!(paras.len(), 2);

        assert_eq!(paras[0].text, "你好");
        assert_eq!(paras[0].char_count, 2);
        assert_eq!(paras[0].byte_len, 6);

        assert_eq!(paras[1].text, "🎉");
        assert_eq!(paras[1].char_offset, 3);
        assert_eq!(paras[1].char_count, 1);
        assert_eq!(paras[1].byte_offset, 7);
        assert_eq!(paras[1].byte_len, 4);
    }

    #[test]
    fn split_preserves_total_chars() {
        let text = "Line one\nLine two\n\nLine four";
        let paras = split_paragraphs(text);
        let total_chars = text.chars().count();
        let last = paras.last().unwrap();
        assert_eq!(last.char_offset + last.char_count, total_chars);
    }

    #[test]
    fn split_byte_offsets_consistent() {
        let text = "abc\ndef\nghi";
        let paras = split_paragraphs(text);
        for para in &paras {
            assert_eq!(
                &text[para.byte_offset..para.byte_offset + para.byte_len],
                para.text
            );
        }
    }

    #[test]
    fn split_unicode_byte_offsets() {
        let text = "caf\u{00e9}\nna\u{00ef}f";
        let paras = split_paragraphs(text);
        for para in &paras {
            assert_eq!(
                &text[para.byte_offset..para.byte_offset + para.byte_len],
                para.text
            );
        }
    }

    // ── char_to_byte_in_para ──

    #[test]
    fn char_to_byte_ascii() {
        assert_eq!(char_to_byte_in_para("hello", 0), 0);
        assert_eq!(char_to_byte_in_para("hello", 3), 3);
        assert_eq!(char_to_byte_in_para("hello", 5), 5);
    }

    #[test]
    fn char_to_byte_multibyte() {
        let t = "a\u{00e9}b"; // a(1) e-acute(2) b(1)
        assert_eq!(char_to_byte_in_para(t, 0), 0);
        assert_eq!(char_to_byte_in_para(t, 1), 1);
        assert_eq!(char_to_byte_in_para(t, 2), 3);
        assert_eq!(char_to_byte_in_para(t, 3), 4); // past end
    }

    #[test]
    fn char_to_byte_empty() {
        assert_eq!(char_to_byte_in_para("", 0), 0);
        assert_eq!(char_to_byte_in_para("", 5), 0);
    }

    // ── Viewport ──

    #[test]
    fn viewport_overlaps_fully_inside() {
        let vp = Viewport::new(100.0, 500.0);
        assert!(vp.overlaps(200.0, 50.0));
    }

    #[test]
    fn viewport_overlaps_crossing_top() {
        let vp = Viewport::new(100.0, 500.0);
        assert!(vp.overlaps(80.0, 50.0));
    }

    #[test]
    fn viewport_overlaps_crossing_bottom() {
        let vp = Viewport::new(100.0, 500.0);
        assert!(vp.overlaps(480.0, 50.0));
    }

    #[test]
    fn viewport_no_overlap_above() {
        let vp = Viewport::new(100.0, 500.0);
        assert!(!vp.overlaps(10.0, 50.0));
    }

    #[test]
    fn viewport_no_overlap_below() {
        let vp = Viewport::new(100.0, 500.0);
        assert!(!vp.overlaps(600.0, 50.0));
    }

    #[test]
    fn viewport_touching_top_edge() {
        let vp = Viewport::new(100.0, 500.0);
        // y=50, h=50 => y+h=100 >= 100 (true), y=50 <= 500 (true) => overlaps
        // This matches the C: y + para_h >= vis_top && y <= vis_bottom
        assert!(vp.overlaps(50.0, 50.0));
    }

    #[test]
    fn viewport_past_bottom() {
        let vp = Viewport::new(100.0, 500.0);
        assert!(vp.past_bottom(501.0));
        assert!(!vp.past_bottom(500.0));
        assert!(!vp.past_bottom(100.0));
    }

    // ── compute_layout_width ──

    #[test]
    fn layout_width_no_adjustments() {
        let pattr = ParaAttr {
            offset: 0,
            length: 10,
            alignment: Alignment::Left,
            indent: 0,
            bullet: BulletType::None,
            quote_level: 0,
            is_hr: false,
            page_break: false,
            tab_stops: vec![],
            direction: Direction::Ltr,
            mask: 0,
        };
        assert_eq!(compute_layout_width(600, &pattr), 600);
    }

    #[test]
    fn layout_width_with_quote_level() {
        let pattr = ParaAttr {
            offset: 0,
            length: 10,
            alignment: Alignment::Left,
            indent: 0,
            bullet: BulletType::None,
            quote_level: 3,
            is_hr: false,
            page_break: false,
            tab_stops: vec![],
            direction: Direction::Ltr,
            mask: 0,
        };
        assert_eq!(compute_layout_width(600, &pattr), 564);
    }

    #[test]
    fn layout_width_with_bullet() {
        let pattr = ParaAttr {
            offset: 0,
            length: 10,
            alignment: Alignment::Left,
            indent: 0,
            bullet: BulletType::Circle,
            quote_level: 0,
            is_hr: false,
            page_break: false,
            tab_stops: vec![],
            direction: Direction::Ltr,
            mask: 0,
        };
        assert_eq!(compute_layout_width(600, &pattr), 588);
    }

    #[test]
    fn layout_width_with_quote_and_bullet() {
        let pattr = ParaAttr {
            offset: 0,
            length: 10,
            alignment: Alignment::Left,
            indent: 0,
            bullet: BulletType::Disk,
            quote_level: 2,
            is_hr: false,
            page_break: false,
            tab_stops: vec![],
            direction: Direction::Ltr,
            mask: 0,
        };
        assert_eq!(compute_layout_width(600, &pattr), 564);
    }

    #[test]
    fn layout_width_minimum_clamp() {
        let pattr = ParaAttr {
            offset: 0,
            length: 10,
            alignment: Alignment::Left,
            indent: 0,
            bullet: BulletType::Square,
            quote_level: 50,
            is_hr: false,
            page_break: false,
            tab_stops: vec![],
            direction: Direction::Ltr,
            mask: 0,
        };
        assert_eq!(compute_layout_width(100, &pattr), 10);
    }

    // ── visible_paragraph_indices ──

    #[test]
    fn visible_indices_all_visible() {
        let layouts = vec![
            ParaLayout {
                y: 6.0,
                height: 20.0,
                char_offset: 0,
                char_count: 10,
            },
            ParaLayout {
                y: 26.0,
                height: 20.0,
                char_offset: 11,
                char_count: 10,
            },
            ParaLayout {
                y: 46.0,
                height: 20.0,
                char_offset: 22,
                char_count: 10,
            },
        ];
        let vp = Viewport::new(0.0, 100.0);
        let indices = visible_paragraph_indices(&layouts, &vp);
        assert_eq!(indices, vec![0, 1, 2]);
    }

    #[test]
    fn visible_indices_partial() {
        let layouts = vec![
            ParaLayout {
                y: 0.0,
                height: 20.0,
                char_offset: 0,
                char_count: 5,
            },
            ParaLayout {
                y: 20.0,
                height: 20.0,
                char_offset: 6,
                char_count: 5,
            },
            ParaLayout {
                y: 40.0,
                height: 20.0,
                char_offset: 12,
                char_count: 5,
            },
            ParaLayout {
                y: 60.0,
                height: 20.0,
                char_offset: 18,
                char_count: 5,
            },
            ParaLayout {
                y: 80.0,
                height: 20.0,
                char_offset: 24,
                char_count: 5,
            },
        ];
        let vp = Viewport::new(25.0, 65.0);
        let indices = visible_paragraph_indices(&layouts, &vp);
        assert_eq!(indices, vec![1, 2, 3]);
    }

    #[test]
    fn visible_indices_none() {
        let layouts = vec![
            ParaLayout {
                y: 0.0,
                height: 20.0,
                char_offset: 0,
                char_count: 5,
            },
            ParaLayout {
                y: 20.0,
                height: 20.0,
                char_offset: 6,
                char_count: 5,
            },
        ];
        let vp = Viewport::new(100.0, 200.0);
        let indices = visible_paragraph_indices(&layouts, &vp);
        assert!(indices.is_empty());
    }

    // ── compute_scroll_adjustment ──

    #[test]
    fn scroll_caret_visible_no_scroll() {
        let result = ScrollResult {
            content_height: 500,
            caret_y: Some(100.0),
            caret_height: 20.0,
        };
        assert_eq!(compute_scroll_adjustment(&result, 50.0, 200.0), None);
    }

    #[test]
    fn scroll_caret_above_viewport() {
        let result = ScrollResult {
            content_height: 500,
            caret_y: Some(30.0),
            caret_height: 20.0,
        };
        let adj = compute_scroll_adjustment(&result, 100.0, 200.0);
        assert_eq!(adj, Some(20.0));
    }

    #[test]
    fn scroll_caret_below_viewport() {
        let result = ScrollResult {
            content_height: 500,
            caret_y: Some(350.0),
            caret_height: 20.0,
        };
        let adj = compute_scroll_adjustment(&result, 100.0, 200.0);
        assert_eq!(adj, Some(180.0));
    }

    #[test]
    fn scroll_caret_below_clamped_to_upper() {
        let result = ScrollResult {
            content_height: 250,
            caret_y: Some(230.0),
            caret_height: 20.0,
        };
        let adj = compute_scroll_adjustment(&result, 0.0, 200.0);
        assert_eq!(adj, Some(50.0));
    }

    #[test]
    fn scroll_no_caret() {
        let result = ScrollResult {
            content_height: 500,
            caret_y: None,
            caret_height: 14.0,
        };
        assert_eq!(compute_scroll_adjustment(&result, 0.0, 200.0), None);
    }

    #[test]
    fn scroll_zero_page_size() {
        let result = ScrollResult {
            content_height: 500,
            caret_y: Some(100.0),
            caret_height: 20.0,
        };
        assert_eq!(compute_scroll_adjustment(&result, 0.0, 0.0), None);
    }

    #[test]
    fn scroll_caret_at_very_top() {
        let result = ScrollResult {
            content_height: 500,
            caret_y: Some(3.0),
            caret_height: 14.0,
        };
        let adj = compute_scroll_adjustment(&result, 50.0, 200.0);
        assert_eq!(adj, Some(0.0));
    }

    // ── Color ──

    #[test]
    fn color_constants() {
        assert_eq!(Color::WHITE.red, 1.0);
        assert_eq!(Color::BLACK.red, 0.0);
        assert_eq!(Color::BLACK.alpha, 1.0);
    }

    #[test]
    fn color_new() {
        let c = Color::new(0.5, 0.6, 0.7, 0.8);
        assert_eq!(c.red, 0.5);
        assert_eq!(c.green, 0.6);
        assert_eq!(c.blue, 0.7);
        assert_eq!(c.alpha, 0.8);
    }
}
