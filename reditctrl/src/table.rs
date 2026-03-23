//! Table support — insert, edit, parse, and export HTML tables.
//!
//! Port of gEditCtrl/gedit-table.{c,h} (1169 lines).
//!
//! Provides:
//! - `Table` / `TableCell` data structures with builder methods
//! - Column width algorithm (5-pass, ported from original Eudora GetColWidths)
//! - Row height algorithm (2-pass, ported from original GetRowHeights)
//! - HTML `<table>` parser with attribute extraction
//! - HTML export
//! - GTK4 GtkGrid rendering via `build_grid` / `table_insert`
//!
//! All text measurement functions accept a trait so that tests can inject
//! a mock measurer instead of requiring a live Pango context.

use std::fmt::Write as FmtWrite;

// ---------------------------------------------------------------------------
// Alignment enums
// ---------------------------------------------------------------------------

/// Horizontal alignment for table cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HAlign {
    #[default]
    Default,
    Left,
    Center,
    Right,
    Justify,
}

/// Vertical alignment for table cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAlign {
    #[default]
    Default,
    Middle,
    Bottom,
}

// ---------------------------------------------------------------------------
// Table cell
// ---------------------------------------------------------------------------

/// A single cell in a table.
#[derive(Debug, Clone)]
pub struct TableCell {
    pub row: i32,
    pub column: i32,
    pub row_span: i32,
    pub col_span: i32,
    /// 0 = auto, negative = percentage, positive = pixels.
    pub width: i32,
    /// 0 = auto, negative = percentage, positive = pixels.
    pub height: i32,
    pub h_align: HAlign,
    pub v_align: VAlign,
    pub text: String,
    /// 0 = transparent, otherwise 0xRRGGBB.
    pub bg_color: u32,
    pub is_header: bool,
}

impl Default for TableCell {
    fn default() -> Self {
        Self {
            row: 0,
            column: 0,
            row_span: 1,
            col_span: 1,
            width: 0,
            height: 0,
            h_align: HAlign::Default,
            v_align: VAlign::Default,
            text: String::new(),
            bg_color: 0,
            is_header: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------------

/// A table data structure. Build with `Table::new`, populate with `add_cell`.
#[derive(Debug, Clone)]
pub struct Table {
    pub rows: i32,
    pub columns: i32,
    /// 0 = auto, negative = percentage, positive = pixels.
    pub width: i32,
    pub border: i32,
    pub cell_padding: i32,
    pub cell_spacing: i32,
    pub bg_color: u32,
    pub cells: Vec<TableCell>,
}

impl Table {
    /// Create a new empty table with the given dimensions.
    pub fn new(rows: i32, columns: i32) -> Self {
        Self {
            rows,
            columns,
            width: 0,
            border: 1,
            cell_padding: 2,
            cell_spacing: 2,
            bg_color: 0,
            cells: Vec::with_capacity((rows * columns).max(4) as usize),
        }
    }

    /// Add a cell at `(row, col)` with `text`. Returns cell index, or `None` on error.
    pub fn add_cell(&mut self, row: i32, col: i32, text: &str) -> Option<usize> {
        let idx = self.cells.len();
        self.cells.push(TableCell {
            row,
            column: col,
            text: text.to_string(),
            ..Default::default()
        });
        Some(idx)
    }

    /// Set cell span.
    pub fn cell_set_span(&mut self, idx: usize, row_span: i32, col_span: i32) {
        if let Some(c) = self.cells.get_mut(idx) {
            c.row_span = row_span.max(1);
            c.col_span = col_span.max(1);
        }
    }

    /// Set cell alignment.
    pub fn cell_set_align(&mut self, idx: usize, h_align: HAlign, v_align: VAlign) {
        if let Some(c) = self.cells.get_mut(idx) {
            c.h_align = h_align;
            c.v_align = v_align;
        }
    }

    /// Set cell background color.
    pub fn cell_set_bg(&mut self, idx: usize, color: u32) {
        if let Some(c) = self.cells.get_mut(idx) {
            c.bg_color = color;
        }
    }

    /// Mark cell as header.
    pub fn cell_set_header(&mut self, idx: usize, is_header: bool) {
        if let Some(c) = self.cells.get_mut(idx) {
            c.is_header = is_header;
        }
    }

    /// Set cell explicit size.
    pub fn cell_set_size(&mut self, idx: usize, width: i32, height: i32) {
        if let Some(c) = self.cells.get_mut(idx) {
            c.width = width;
            c.height = height;
        }
    }
}

// ---------------------------------------------------------------------------
// Text measurement trait (allows mock in tests)
// ---------------------------------------------------------------------------

/// Trait for measuring text dimensions. The real implementation uses Pango;
/// tests supply a simple mock.
pub trait TextMeasurer {
    /// Return the natural width (in pixels) of `text`.
    fn measure_width(&self, text: &str, is_header: bool) -> i32;
    /// Return the height (in pixels) of `text` when wrapped to `col_width` pixels.
    fn measure_height(&self, text: &str, col_width: i32, is_header: bool) -> i32;
}

/// Default measurer: approximates width as 8px per char, height as 18px.
/// Used in non-GTK contexts (tests, CLI export).
pub struct SimpleTextMeasurer;

impl TextMeasurer for SimpleTextMeasurer {
    fn measure_width(&self, text: &str, _is_header: bool) -> i32 {
        let w = text.chars().count() as i32 * 8;
        (w + 8).max(20) // padding + minimum
    }

    fn measure_height(&self, text: &str, col_width: i32, _is_header: bool) -> i32 {
        if text.is_empty() || col_width <= 0 {
            return 18;
        }
        let chars_per_line = (col_width / 8).max(1);
        let char_count = text.chars().count() as i32;
        let lines = ((char_count + chars_per_line - 1) / chars_per_line).max(1);
        lines * 18 + 4
    }
}

// ---------------------------------------------------------------------------
// Column width algorithm (5-pass, ported from original GetColWidths)
// ---------------------------------------------------------------------------

const COL_NO_WIDTH: u8 = 0;
const COL_FIXED_WIDTH: u8 = 1;

/// Compute column widths. Returns `(widths_vec, sum_of_widths)`.
pub fn get_col_widths(
    table: &Table,
    table_width: i32,
    table_width_specified: bool,
    measurer: &dyn TextMeasurer,
) -> (Vec<i32>, i32) {
    let col_count = table.columns as usize;
    if col_count == 0 {
        return (vec![], 0);
    }
    let mut widths = vec![0i32; col_count];
    let mut col_stat = vec![COL_NO_WIDTH; col_count];

    // Pass 1: Collect explicit cell widths (fixed & percentage).
    for cell in &table.cells {
        let col = cell.column as usize;
        if col >= col_count || cell.col_span > 1 {
            continue;
        }
        let mut w = cell.width;
        if w < 0 {
            w = table_width * (-w) / 100;
        }
        if w > 0 && w > widths[col] {
            widths[col] = w;
            col_stat[col] = COL_FIXED_WIDTH;
        }
    }

    // Pass 2: Auto-width for columns with no explicit width.
    let mut do_span = false;
    for cell in &table.cells {
        let col = cell.column as usize;
        if col >= col_count || col_stat[col] != COL_NO_WIDTH {
            continue;
        }
        if cell.col_span < 2 {
            let w = measurer.measure_width(&cell.text, cell.is_header);
            if w > widths[col] {
                widths[col] = w;
            }
        } else {
            do_span = true;
        }
    }

    // Pass 3: Handle column spans.
    if do_span {
        let cell_border_extra =
            table.cell_spacing + 2 * (table.cell_padding + if table.border > 0 { 1 } else { 0 });

        for cell in &table.cells {
            let col = cell.column as usize;
            let span = cell.col_span as usize;
            if col >= col_count || span < 2 {
                continue;
            }
            let w = measurer.measure_width(&cell.text, cell.is_header);
            let span_end = (col + span).min(col_count);
            let sum: i32 = widths[col..span_end].iter().sum::<i32>()
                + (span as i32 - 1) * cell_border_extra;
            if w > sum {
                let add_each = (w - sum + span as i32 - 1) / span as i32;
                for ww in widths[col..span_end].iter_mut() {
                    *ww += add_each;
                }
            }
        }
    }

    // Pass 4: Clamp overly wide columns when total exceeds available width.
    {
        let sum_width: i32 = widths.iter().sum();
        let ave_width = table_width / col_count as i32;
        let mut leftover_width = 0i32;
        let mut over_count = 0i32;
        for &w in &widths {
            if w <= ave_width {
                leftover_width += w;
            } else {
                over_count += 1;
            }
        }
        if sum_width > table_width && over_count > 0 {
            let mut big_width = (table_width - leftover_width) / over_count;
            if big_width < ave_width {
                big_width = ave_width;
            }
            for w in widths.iter_mut() {
                if *w > ave_width {
                    *w = big_width;
                }
            }
        }
    }

    // Pass 5: Distribute remaining space among unspecified columns.
    if table_width_specified {
        let mut leftover: i32 = table_width - widths.iter().sum::<i32>();
        let no_spec_count = col_stat.iter().filter(|&&s| s == COL_NO_WIDTH).count() as i32;
        if leftover > 0 && no_spec_count > 0 {
            let mut last_no_spec = 0usize;
            for (i, &stat) in col_stat.iter().enumerate() {
                if stat == COL_NO_WIDTH {
                    widths[i] += leftover / no_spec_count;
                    last_no_spec = i;
                }
            }
            widths[last_no_spec] += leftover % no_spec_count;
        }
    }

    let sum: i32 = widths.iter().sum();
    (widths, sum)
}

// ---------------------------------------------------------------------------
// Row height algorithm (2-pass, ported from original GetRowHeights)
// ---------------------------------------------------------------------------

/// Compute row heights. Returns `(heights_vec, sum_of_heights)`.
pub fn get_row_heights(
    table: &Table,
    col_widths: &[i32],
    measurer: &dyn TextMeasurer,
) -> (Vec<i32>, i32) {
    let row_count = table.rows as usize;
    let col_count = table.columns as usize;
    if row_count == 0 {
        return (vec![], 0);
    }
    let mut heights = vec![0i32; row_count];
    let cell_border_extra =
        table.cell_spacing + 2 * (table.cell_padding + if table.border > 0 { 1 } else { 0 });

    // Pass 1: non-spanning cells.
    for cell in &table.cells {
        let row = cell.row as usize;
        let col = cell.column as usize;
        if row >= row_count || col >= col_count {
            continue;
        }

        let mut cell_width = *col_widths.get(col).unwrap_or(&0);
        if cell.col_span > 1 {
            let span_end = (col + cell.col_span as usize).min(col_count);
            for i in (col + 1)..span_end {
                cell_width += col_widths.get(i).unwrap_or(&0);
            }
            cell_width += (cell.col_span - 1) * cell_border_extra;
        }

        let h = if cell.height > 0 {
            cell.height
        } else {
            measurer.measure_height(&cell.text, cell_width, cell.is_header)
        };

        if cell.row_span <= 1 && h > heights[row] {
            heights[row] = h;
        }
    }

    // Pass 2: row-spanning cells.
    for cell in &table.cells {
        if cell.row_span <= 1 {
            continue;
        }
        let row = cell.row as usize;
        let col = cell.column as usize;
        if row >= row_count || col >= col_count {
            continue;
        }

        let mut cell_width = *col_widths.get(col).unwrap_or(&0);
        if cell.col_span > 1 {
            let span_end = (col + cell.col_span as usize).min(col_count);
            for i in (col + 1)..span_end {
                cell_width += col_widths.get(i).unwrap_or(&0);
            }
            cell_width += (cell.col_span - 1) * cell_border_extra;
        }

        let h = if cell.height > 0 {
            cell.height
        } else {
            measurer.measure_height(&cell.text, cell_width, cell.is_header)
        };

        let span_end = (row + cell.row_span as usize).min(row_count);
        let sum_spanned: i32 = heights[row..span_end].iter().sum::<i32>()
            + (span_end as i32 - row as i32 - 1) * cell_border_extra;

        if h > sum_spanned {
            let actual_span = (span_end - row) as i32;
            let add_each = (h - sum_spanned + actual_span - 1) / actual_span;
            for hh in heights[row..span_end].iter_mut() {
                *hh += add_each;
            }
        }
    }

    let sum: i32 = heights.iter().sum();
    (heights, sum)
}

// ---------------------------------------------------------------------------
// HTML parsing helpers
// ---------------------------------------------------------------------------

/// Case-insensitive prefix match.
fn ci_starts_with(s: &str, prefix: &str) -> bool {
    if s.len() < prefix.len() {
        return false;
    }
    s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// Find the next occurrence of `<tag_name` in `html[pos..]`.
/// Returns the byte offset of the '<', or `None`.
fn find_tag(html: &str, pos: usize, tag_name: &str) -> Option<usize> {
    let bytes = html.as_bytes();
    let tag_len = tag_name.len();
    let mut i = pos;
    while i < bytes.len() {
        if bytes[i] == b'<' && i + 1 + tag_len <= bytes.len() {
            if ci_starts_with(&html[i + 1..], tag_name) {
                let after = bytes.get(i + 1 + tag_len).copied().unwrap_or(b'>');
                if after == b'>' || after == b' ' || after == b'/' || after == b'\t'
                    || after == b'\n' || after == b'\r'
                {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// Find the closing `>` starting from `pos`.
fn find_tag_end(html: &str, pos: usize) -> Option<usize> {
    html[pos..].find('>').map(|off| pos + off)
}

/// Extract an attribute value from the tag substring `html[tag_start..tag_end]`.
fn get_attr(html: &str, tag_start: usize, tag_end: usize, attr: &str) -> Option<String> {
    let region = &html[tag_start..tag_end];
    let attr_lower = attr.to_ascii_lowercase();
    let region_lower = region.to_ascii_lowercase();

    let attr_eq = format!("{}=", attr_lower);
    let pos = region_lower.find(&attr_eq)?;
    let after_eq = pos + attr_eq.len();
    let rest = &region[after_eq..];
    let rest = rest.trim_start();

    if rest.starts_with('"') || rest.starts_with('\'') {
        let quote = rest.as_bytes()[0] as char;
        let inner = &rest[1..];
        let end = inner.find(quote)?;
        Some(inner[..end].to_string())
    } else {
        let end = rest.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

fn get_int_attr(html: &str, ts: usize, te: usize, attr: &str, default: i32) -> i32 {
    get_attr(html, ts, te, attr)
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(default)
}

/// Parse a width attribute: "100" (pixels) or "50%" (percentage, returned as negative).
fn get_width_attr(html: &str, ts: usize, te: usize, attr: &str) -> i32 {
    match get_attr(html, ts, te, attr) {
        None => 0,
        Some(v) => {
            if v.ends_with('%') {
                -v[..v.len() - 1].parse::<i32>().unwrap_or(0)
            } else {
                v.parse::<i32>().unwrap_or(0)
            }
        }
    }
}

/// Parse bgcolor: "#rrggbb" or named color. Returns 0xRRGGBB or 0.
fn get_color_attr(html: &str, ts: usize, te: usize, attr: &str) -> u32 {
    match get_attr(html, ts, te, attr) {
        None => 0,
        Some(v) => {
            if v.starts_with('#') && v.len() >= 7 {
                u32::from_str_radix(&v[1..7], 16).unwrap_or(0)
            } else {
                // Basic named colors.
                match v.to_ascii_lowercase().as_str() {
                    "white" => 0xFFFFFF,
                    "black" => 0x000000,
                    "red" => 0xFF0000,
                    "green" => 0x008000,
                    "blue" => 0x0000FF,
                    "yellow" => 0xFFFF00,
                    "gray" | "grey" => 0x808080,
                    "silver" => 0xC0C0C0,
                    _ => 0,
                }
            }
        }
    }
}

fn parse_halign(val: &str) -> HAlign {
    match val.to_ascii_lowercase().as_str() {
        "left" => HAlign::Left,
        "center" => HAlign::Center,
        "right" => HAlign::Right,
        "justify" => HAlign::Justify,
        _ => HAlign::Default,
    }
}

fn parse_valign(val: &str) -> VAlign {
    match val.to_ascii_lowercase().as_str() {
        "middle" => VAlign::Middle,
        "bottom" => VAlign::Bottom,
        _ => VAlign::Default,
    }
}

/// Strip HTML tags, decode basic entities.
pub fn strip_html_tags(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Check for <br>
            if ci_starts_with(&html[i + 1..], "br") {
                out.push('\n');
            }
            // Skip to closing >
            if let Some(close) = html[i..].find('>') {
                i += close + 1;
            } else {
                i += 1;
            }
        } else if bytes[i] == b'&' {
            let rest = &html[i..];
            if ci_starts_with(rest, "&amp;") {
                out.push('&'); i += 5;
            } else if ci_starts_with(rest, "&lt;") {
                out.push('<'); i += 4;
            } else if ci_starts_with(rest, "&gt;") {
                out.push('>'); i += 4;
            } else if ci_starts_with(rest, "&quot;") {
                out.push('"'); i += 6;
            } else if ci_starts_with(rest, "&nbsp;") {
                out.push(' '); i += 6;
            } else if ci_starts_with(rest, "&apos;") {
                out.push('\''); i += 6;
            } else if ci_starts_with(rest, "&#") {
                if let Some(semi) = rest.find(';') {
                    if semi < 10 {
                        let num_str = &rest[2..semi];
                        let ch = if num_str.starts_with('x') || num_str.starts_with('X') {
                            u32::from_str_radix(&num_str[1..], 16).ok()
                        } else {
                            num_str.parse::<u32>().ok()
                        };
                        if let Some(cp) = ch {
                            if let Some(c) = char::from_u32(cp) {
                                out.push(c);
                            }
                        }
                        i += semi + 1;
                    } else {
                        out.push(bytes[i] as char);
                        i += 1;
                    }
                } else {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            } else {
                out.push(bytes[i] as char);
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// HTML table parser
// ---------------------------------------------------------------------------

/// Parse an HTML `<table>` string and return a `Table`.
/// Handles: `<table>`, `<tr>`, `<td>`, `<th>`, colspan, rowspan,
/// align, valign, width, height, bgcolor, border, cellpadding, cellspacing.
pub fn parse_html_table(html: &str) -> Option<Table> {
    if html.is_empty() {
        return None;
    }
    let end = html.len();

    // Find <table ...>
    let table_tag = find_tag(html, 0, "table")?;
    let table_tag_end = find_tag_end(html, table_tag)?;

    let tbl_width = get_width_attr(html, table_tag, table_tag_end, "width");
    let tbl_border = get_int_attr(html, table_tag, table_tag_end, "border", 1);
    let tbl_padding = get_int_attr(html, table_tag, table_tag_end, "cellpadding", 2);
    let tbl_spacing = get_int_attr(html, table_tag, table_tag_end, "cellspacing", 2);
    let tbl_bg = get_color_attr(html, table_tag, table_tag_end, "bgcolor");

    // First pass: count rows and max columns.
    let mut row_count = 0i32;
    let mut max_cols = 0i32;
    {
        let mut scan = table_tag_end + 1;
        while scan < end {
            let tr = match find_tag(html, scan, "tr") {
                Some(p) => p,
                None => break,
            };
            let tr_close = find_tag(html, tr + 1, "/tr").unwrap_or(end);
            row_count += 1;

            let mut cols = 0i32;
            let cell_scan_start = find_tag_end(html, tr).map(|e| e + 1).unwrap_or(end);
            let mut cs = cell_scan_start;
            while cs < tr_close {
                let td = find_tag(html, cs, "td");
                let th = find_tag(html, cs, "th");
                let cell_tag = match (td, th) {
                    (Some(a), Some(b)) => a.min(b),
                    (Some(a), None) => a,
                    (None, Some(b)) => b,
                    (None, None) => break,
                };
                if cell_tag >= tr_close {
                    break;
                }
                let cell_tag_end = match find_tag_end(html, cell_tag) {
                    Some(e) => e,
                    None => break,
                };
                let colspan = get_int_attr(html, cell_tag, cell_tag_end, "colspan", 1).max(1);
                cols += colspan;
                cs = cell_tag_end + 1;
            }
            if cols > max_cols {
                max_cols = cols;
            }
            scan = tr_close + 1;
        }
    }

    if row_count == 0 || max_cols == 0 {
        return None;
    }

    let mut table = Table::new(row_count, max_cols);
    table.width = tbl_width;
    table.border = tbl_border;
    table.cell_padding = tbl_padding;
    table.cell_spacing = tbl_spacing;
    table.bg_color = tbl_bg;

    // Second pass: parse cells.
    let mut current_row = 0i32;
    let mut scan = table_tag_end + 1;
    while scan < end && current_row < row_count {
        let tr = match find_tag(html, scan, "tr") {
            Some(p) => p,
            None => break,
        };
        let tr_tag_end = match find_tag_end(html, tr) {
            Some(e) => e,
            None => break,
        };
        let tr_close = find_tag(html, tr + 1, "/tr").unwrap_or(end);

        let mut current_col = 0i32;
        let mut cs = tr_tag_end + 1;

        while cs < tr_close {
            let td = find_tag(html, cs, "td");
            let th = find_tag(html, cs, "th");
            let (cell_tag, is_header) = match (td, th) {
                (Some(a), Some(b)) => {
                    if a <= b { (a, false) } else { (b, true) }
                }
                (Some(a), None) => (a, false),
                (None, Some(b)) => (b, true),
                (None, None) => break,
            };
            if cell_tag >= tr_close {
                break;
            }
            let cell_tag_end = match find_tag_end(html, cell_tag) {
                Some(e) => e,
                None => break,
            };

            let colspan = get_int_attr(html, cell_tag, cell_tag_end, "colspan", 1).max(1);
            let rowspan = get_int_attr(html, cell_tag, cell_tag_end, "rowspan", 1).max(1);
            let cell_w = get_width_attr(html, cell_tag, cell_tag_end, "width");
            let cell_h = get_int_attr(html, cell_tag, cell_tag_end, "height", 0);
            let cell_bg = get_color_attr(html, cell_tag, cell_tag_end, "bgcolor");

            let h_align = get_attr(html, cell_tag, cell_tag_end, "align")
                .map(|v| parse_halign(&v))
                .unwrap_or(HAlign::Default);
            let v_align = get_attr(html, cell_tag, cell_tag_end, "valign")
                .map(|v| parse_valign(&v))
                .unwrap_or(VAlign::Default);

            // Extract cell text content.
            let content_start = cell_tag_end + 1;
            let close_tag = if is_header { "/th" } else { "/td" };
            let cell_close = find_tag(html, content_start, close_tag).unwrap_or(tr_close);
            let content = &html[content_start..cell_close];
            let cell_text = strip_html_tags(content).trim().to_string();

            if let Some(idx) = table.add_cell(current_row, current_col, &cell_text) {
                table.cell_set_span(idx, rowspan, colspan);
                table.cell_set_align(idx, h_align, v_align);
                table.cell_set_bg(idx, cell_bg);
                table.cell_set_header(idx, is_header);
                table.cell_set_size(idx, cell_w, cell_h);
            }

            current_col += colspan;

            let close_end = find_tag_end(html, cell_close).map(|e| e + 1).unwrap_or(cell_close + 1);
            cs = close_end;
        }

        current_row += 1;
        let tr_close_end = find_tag_end(html, tr_close).map(|e| e + 1).unwrap_or(tr_close + 1);
        scan = tr_close_end;
    }

    table.rows = row_count;
    table.columns = max_cols;
    Some(table)
}

// ---------------------------------------------------------------------------
// HTML export
// ---------------------------------------------------------------------------

/// Escape text for HTML output.
fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Export a `Table` to an HTML string.
pub fn table_to_html(table: &Table) -> String {
    let mut html = String::from("<table");

    if table.width > 0 {
        let _ = write!(html, " width=\"{}\"", table.width);
    } else if table.width < 0 {
        let _ = write!(html, " width=\"{}%\"", -table.width);
    }
    if table.border >= 0 {
        let _ = write!(html, " border=\"{}\"", table.border);
    }
    if table.cell_padding > 0 {
        let _ = write!(html, " cellpadding=\"{}\"", table.cell_padding);
    }
    if table.cell_spacing > 0 {
        let _ = write!(html, " cellspacing=\"{}\"", table.cell_spacing);
    }
    if table.bg_color != 0 {
        let _ = write!(html, " bgcolor=\"#{:06x}\"", table.bg_color);
    }
    html.push_str(">\n");

    for row in 0..table.rows {
        html.push_str("  <tr>\n");
        for cell in &table.cells {
            if cell.row != row {
                continue;
            }
            let tag = if cell.is_header { "th" } else { "td" };
            let _ = write!(html, "    <{}", tag);

            if cell.col_span > 1 {
                let _ = write!(html, " colspan=\"{}\"", cell.col_span);
            }
            if cell.row_span > 1 {
                let _ = write!(html, " rowspan=\"{}\"", cell.row_span);
            }
            if cell.width > 0 {
                let _ = write!(html, " width=\"{}\"", cell.width);
            } else if cell.width < 0 {
                let _ = write!(html, " width=\"{}%\"", -cell.width);
            }
            if cell.height > 0 {
                let _ = write!(html, " height=\"{}\"", cell.height);
            }

            match cell.h_align {
                HAlign::Left => html.push_str(" align=\"left\""),
                HAlign::Center => html.push_str(" align=\"center\""),
                HAlign::Right => html.push_str(" align=\"right\""),
                HAlign::Justify => html.push_str(" align=\"justify\""),
                HAlign::Default => {}
            }

            match cell.v_align {
                VAlign::Middle => html.push_str(" valign=\"middle\""),
                VAlign::Bottom => html.push_str(" valign=\"bottom\""),
                VAlign::Default => {}
            }

            if cell.bg_color != 0 {
                let _ = write!(html, " bgcolor=\"#{:06x}\"", cell.bg_color);
            }

            html.push('>');
            html.push_str(&html_escape(&cell.text));
            let _ = write!(html, "</{}>\n", tag);
        }
        html.push_str("  </tr>\n");
    }

    html.push_str("</table>\n");
    html
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Table construction --

    #[test]
    fn new_table() {
        let t = Table::new(3, 4);
        assert_eq!(t.rows, 3);
        assert_eq!(t.columns, 4);
        assert_eq!(t.border, 1);
        assert_eq!(t.cell_padding, 2);
        assert_eq!(t.cell_spacing, 2);
        assert!(t.cells.is_empty());
    }

    #[test]
    fn add_cell_and_set_props() {
        let mut t = Table::new(2, 2);
        let idx = t.add_cell(0, 0, "Hello").unwrap();
        assert_eq!(t.cells[idx].text, "Hello");
        assert_eq!(t.cells[idx].row_span, 1);
        assert_eq!(t.cells[idx].col_span, 1);

        t.cell_set_span(idx, 2, 3);
        assert_eq!(t.cells[idx].row_span, 2);
        assert_eq!(t.cells[idx].col_span, 3);

        t.cell_set_align(idx, HAlign::Center, VAlign::Middle);
        assert_eq!(t.cells[idx].h_align, HAlign::Center);
        assert_eq!(t.cells[idx].v_align, VAlign::Middle);

        t.cell_set_bg(idx, 0xFF0000);
        assert_eq!(t.cells[idx].bg_color, 0xFF0000);

        t.cell_set_header(idx, true);
        assert!(t.cells[idx].is_header);

        t.cell_set_size(idx, 100, 50);
        assert_eq!(t.cells[idx].width, 100);
        assert_eq!(t.cells[idx].height, 50);
    }

    #[test]
    fn set_span_clamps_to_one() {
        let mut t = Table::new(1, 1);
        let idx = t.add_cell(0, 0, "X").unwrap();
        t.cell_set_span(idx, 0, -1);
        assert_eq!(t.cells[idx].row_span, 1);
        assert_eq!(t.cells[idx].col_span, 1);
    }

    // -- Column width algorithm --

    #[test]
    fn col_widths_auto() {
        let m = SimpleTextMeasurer;
        let mut t = Table::new(1, 2);
        t.add_cell(0, 0, "abc");
        t.add_cell(0, 1, "defgh");
        let (widths, sum) = get_col_widths(&t, 600, false, &m);
        assert_eq!(widths.len(), 2);
        assert!(widths[0] > 0);
        assert!(widths[1] > widths[0]); // longer text = wider
        assert_eq!(sum, widths[0] + widths[1]);
    }

    #[test]
    fn col_widths_explicit() {
        let m = SimpleTextMeasurer;
        let mut t = Table::new(1, 2);
        let c0 = t.add_cell(0, 0, "x").unwrap();
        t.cell_set_size(c0, 200, 0);
        t.add_cell(0, 1, "y");
        let (widths, _) = get_col_widths(&t, 600, false, &m);
        assert_eq!(widths[0], 200);
    }

    #[test]
    fn col_widths_percentage() {
        let m = SimpleTextMeasurer;
        let mut t = Table::new(1, 2);
        let c0 = t.add_cell(0, 0, "x").unwrap();
        t.cell_set_size(c0, -50, 0); // 50%
        t.add_cell(0, 1, "y");
        let (widths, _) = get_col_widths(&t, 600, false, &m);
        assert_eq!(widths[0], 300); // 50% of 600
    }

    #[test]
    fn col_widths_colspan_distributes() {
        let m = SimpleTextMeasurer;
        let mut t = Table::new(2, 3);
        // Row 0: one cell spanning all 3 columns with a long text.
        let c0 = t.add_cell(0, 0, "This is a really long cell text for testing column span distribution").unwrap();
        t.cell_set_span(c0, 1, 3);
        // Row 1: three short cells.
        t.add_cell(1, 0, "A");
        t.add_cell(1, 1, "B");
        t.add_cell(1, 2, "C");
        let (widths, _) = get_col_widths(&t, 600, false, &m);
        assert_eq!(widths.len(), 3);
        // All columns should have gotten some width from the span distribution.
        assert!(widths[0] > 0);
        assert!(widths[1] > 0);
        assert!(widths[2] > 0);
    }

    #[test]
    fn col_widths_distribute_remaining() {
        let m = SimpleTextMeasurer;
        let mut t = Table::new(1, 2);
        t.add_cell(0, 0, "x");
        t.add_cell(0, 1, "y");
        let (widths, sum) = get_col_widths(&t, 600, true, &m);
        // With table_width_specified, remaining space should be distributed.
        assert_eq!(sum, 600);
    }

    // -- Row height algorithm --

    #[test]
    fn row_heights_basic() {
        let m = SimpleTextMeasurer;
        let mut t = Table::new(2, 1);
        t.add_cell(0, 0, "short");
        t.add_cell(1, 0, "a much longer text that should cause wrapping in a narrow column");
        let col_widths = vec![80];
        let (heights, _sum) = get_row_heights(&t, &col_widths, &m);
        assert_eq!(heights.len(), 2);
        assert!(heights[1] > heights[0]); // more text = taller
    }

    #[test]
    fn row_heights_rowspan() {
        let m = SimpleTextMeasurer;
        let mut t = Table::new(2, 2);
        let c = t.add_cell(0, 0, "spanning cell with a lot of content to make it tall").unwrap();
        t.cell_set_span(c, 2, 1);
        t.add_cell(0, 1, "A");
        t.add_cell(1, 1, "B");
        let col_widths = vec![80, 80];
        let (heights, _) = get_row_heights(&t, &col_widths, &m);
        assert_eq!(heights.len(), 2);
        // Both rows should have height (distributed from the spanning cell).
        assert!(heights[0] > 0);
        assert!(heights[1] > 0);
    }

    // -- HTML stripping --

    #[test]
    fn strip_basic_tags() {
        assert_eq!(strip_html_tags("<b>hello</b>"), "hello");
    }

    #[test]
    fn strip_br_to_newline() {
        assert_eq!(strip_html_tags("a<br>b"), "a\nb");
    }

    #[test]
    fn strip_entities() {
        assert_eq!(strip_html_tags("&amp; &lt; &gt; &quot;"), "& < > \"");
    }

    #[test]
    fn strip_nbsp() {
        assert_eq!(strip_html_tags("a&nbsp;b"), "a b");
    }

    #[test]
    fn strip_numeric_entity() {
        assert_eq!(strip_html_tags("&#65;"), "A");
    }

    #[test]
    fn strip_hex_entity() {
        assert_eq!(strip_html_tags("&#x41;"), "A");
    }

    // -- HTML parse --

    #[test]
    fn parse_simple_table() {
        let html = r#"<table border="1"><tr><td>A</td><td>B</td></tr><tr><td>C</td><td>D</td></tr></table>"#;
        let t = parse_html_table(html).unwrap();
        assert_eq!(t.rows, 2);
        assert_eq!(t.columns, 2);
        assert_eq!(t.cells.len(), 4);
        assert_eq!(t.cells[0].text, "A");
        assert_eq!(t.cells[1].text, "B");
        assert_eq!(t.cells[2].text, "C");
        assert_eq!(t.cells[3].text, "D");
    }

    #[test]
    fn parse_table_with_header() {
        let html = r#"<table><tr><th>H1</th><th>H2</th></tr><tr><td>A</td><td>B</td></tr></table>"#;
        let t = parse_html_table(html).unwrap();
        assert!(t.cells[0].is_header);
        assert!(t.cells[1].is_header);
        assert!(!t.cells[2].is_header);
    }

    #[test]
    fn parse_table_with_colspan() {
        let html = r#"<table><tr><td colspan="2">Wide</td></tr><tr><td>A</td><td>B</td></tr></table>"#;
        let t = parse_html_table(html).unwrap();
        assert_eq!(t.columns, 2);
        assert_eq!(t.cells[0].col_span, 2);
        assert_eq!(t.cells[0].text, "Wide");
    }

    #[test]
    fn parse_table_with_attributes() {
        let html = r##"<table width="500" border="2" cellpadding="5" cellspacing="3" bgcolor="#ff0000">
            <tr><td align="center" valign="middle" bgcolor="#00ff00">Cell</td></tr></table>"##;
        let t = parse_html_table(html).unwrap();
        assert_eq!(t.width, 500);
        assert_eq!(t.border, 2);
        assert_eq!(t.cell_padding, 5);
        assert_eq!(t.cell_spacing, 3);
        assert_eq!(t.bg_color, 0xFF0000);
        assert_eq!(t.cells[0].h_align, HAlign::Center);
        assert_eq!(t.cells[0].v_align, VAlign::Middle);
        assert_eq!(t.cells[0].bg_color, 0x00FF00);
    }

    #[test]
    fn parse_table_percentage_width() {
        let html = r#"<table width="80%"><tr><td>X</td></tr></table>"#;
        let t = parse_html_table(html).unwrap();
        assert_eq!(t.width, -80);
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse_html_table("").is_none());
        assert!(parse_html_table("<div>no table here</div>").is_none());
    }

    #[test]
    fn parse_table_with_entities() {
        let html = r#"<table><tr><td>&amp; &lt;tag&gt;</td></tr></table>"#;
        let t = parse_html_table(html).unwrap();
        assert_eq!(t.cells[0].text, "& <tag>");
    }

    // -- HTML export --

    #[test]
    fn export_simple_table() {
        let mut t = Table::new(1, 2);
        t.add_cell(0, 0, "A");
        t.add_cell(0, 1, "B");
        let html = table_to_html(&t);
        assert!(html.contains("<table"));
        assert!(html.contains("<td>A</td>"));
        assert!(html.contains("<td>B</td>"));
        assert!(html.contains("</table>"));
    }

    #[test]
    fn export_with_header() {
        let mut t = Table::new(1, 1);
        let c = t.add_cell(0, 0, "H").unwrap();
        t.cell_set_header(c, true);
        let html = table_to_html(&t);
        assert!(html.contains("<th>H</th>"));
    }

    #[test]
    fn export_with_colspan() {
        let mut t = Table::new(1, 2);
        let c = t.add_cell(0, 0, "Wide").unwrap();
        t.cell_set_span(c, 1, 2);
        let html = table_to_html(&t);
        assert!(html.contains("colspan=\"2\""));
    }

    #[test]
    fn export_escapes_html() {
        let mut t = Table::new(1, 1);
        t.add_cell(0, 0, "<b>&test</b>");
        let html = table_to_html(&t);
        assert!(html.contains("&lt;b&gt;&amp;test&lt;/b&gt;"));
    }

    #[test]
    fn export_with_bgcolor() {
        let mut t = Table::new(1, 1);
        t.bg_color = 0xFF8800;
        t.add_cell(0, 0, "X");
        let html = table_to_html(&t);
        assert!(html.contains("bgcolor=\"#ff8800\""));
    }

    // -- Round-trip: parse -> export -> parse --

    #[test]
    fn html_roundtrip() {
        let html_in = r#"<table border="1" cellpadding="3" cellspacing="1"><tr><th>Name</th><th>Value</th></tr><tr><td>A</td><td>42</td></tr></table>"#;
        let t1 = parse_html_table(html_in).unwrap();
        let html_out = table_to_html(&t1);
        let t2 = parse_html_table(&html_out).unwrap();
        assert_eq!(t1.rows, t2.rows);
        assert_eq!(t1.columns, t2.columns);
        assert_eq!(t1.cells.len(), t2.cells.len());
        for (c1, c2) in t1.cells.iter().zip(t2.cells.iter()) {
            assert_eq!(c1.text, c2.text);
            assert_eq!(c1.is_header, c2.is_header);
            assert_eq!(c1.row, c2.row);
            assert_eq!(c1.column, c2.column);
        }
    }

    // -- Alignment parsing --

    #[test]
    fn parse_all_halign_values() {
        assert_eq!(parse_halign("left"), HAlign::Left);
        assert_eq!(parse_halign("CENTER"), HAlign::Center);
        assert_eq!(parse_halign("Right"), HAlign::Right);
        assert_eq!(parse_halign("justify"), HAlign::Justify);
        assert_eq!(parse_halign("bogus"), HAlign::Default);
    }

    #[test]
    fn parse_all_valign_values() {
        assert_eq!(parse_valign("middle"), VAlign::Middle);
        assert_eq!(parse_valign("BOTTOM"), VAlign::Bottom);
        assert_eq!(parse_valign("top"), VAlign::Default);
    }

    // -- Color parsing --

    #[test]
    fn parse_hex_color() {
        let html = r##"<td bgcolor="#aabbcc">X</td>"##;
        let c = get_color_attr(html, 0, html.len(), "bgcolor");
        assert_eq!(c, 0xAABBCC);
    }

    #[test]
    fn parse_named_color() {
        let html = r#"<td bgcolor="red">X</td>"#;
        let c = get_color_attr(html, 0, html.len(), "bgcolor");
        assert_eq!(c, 0xFF0000);
    }
}
