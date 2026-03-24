//! Graph drawing engine — port of Eudora's graph.c (538 lines).
//!
//! Renders bar, line, area, pie, circle, diamond, and square charts
//! using Cairo. Designed to be used as a custom draw callback with
//! `insert_custom_graphic`.
//!
//! Port of graph.c — same math, Cairo instead of QuickDraw.

use pangocairo::cairo;
use cairo::Context as CairoContext;
use std::f64::consts::PI;

// ─── Constants (port of graph.c enums) ───────────────────────────────────────

const BORDER_WIDTH: f64 = 12.0;
const MIN_Y_HT: f64 = 12.0;
const X_AXIS_HT: f64 = 20.0;
const DOT_SIZE: f64 = 8.0;
const LEGEND_BOX_HEIGHT: f64 = 12.0;
const LEGEND_BOX_WIDTH: f64 = 16.0;
const LEGEND_LINE_HT: f64 = 24.0;
const LEGEND_TEXT_SPACE: f64 = 4.0;
const Y_AXIS_WIDTH: f64 = 25.0;
const TICK_SIZE: f64 = 5.0;
const LEGEND_MIN_WIDTH: f64 = 80.0; // replaces GetRLong(STAT_LEGEND_WIDTH)
const FONT_SIZE: f64 = 9.0;

// ─── Types (port of graph.h) ─────────────────────────────────────────────────

/// Graph plot type — matches Eudora's GraphType enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphType {
    Default,
    Line,
    Area,
    Bar,
    Circle,
    Diamond,
    Square,
    Pie,
}

/// RGB color for a data series.
#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Color {
    pub const fn new(r: f64, g: f64, b: f64) -> Self {
        Self { r, g, b }
    }
}

/// One data series to be plotted.
#[derive(Debug, Clone)]
pub struct SeriesInfo {
    pub graph_type: GraphType, // per-series override, Default = use parent
    pub data: Vec<u64>,
    pub color: Color,
    pub label: String,
    pub divider: u64, // divide values by this, 0 = no modification
}

/// Complete graph specification — port of GraphData.
#[derive(Debug, Clone)]
pub struct GraphData {
    pub graph_type: GraphType,     // default graph type
    pub x_labels: Vec<String>,     // optional x-axis labels
    pub series: Vec<SeriesInfo>,
    pub data_count: usize,         // number of data points per series
    pub divider: u64,              // global divider
    pub dec_place: i32,            // decimal places for y-axis labels
    pub legend: bool,
    pub large_tick_interval: usize,
    pub med_tick_interval: usize,
}

// ─── Rect helper ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Rect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Rect {
    fn width(&self) -> f64 {
        self.right - self.left
    }
    fn height(&self) -> f64 {
        self.bottom - self.top
    }
    fn inset(&self, dx: f64, dy: f64) -> Self {
        Self {
            left: self.left + dx,
            top: self.top + dy,
            right: self.right - dx,
            bottom: self.bottom - dy,
        }
    }
}

// ─── Pango text helpers ──────────────────────────────────────────────────────

fn setup_font(cr: &CairoContext) {
    cr.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(FONT_SIZE);
}

fn text_width(cr: &CairoContext, s: &str) -> f64 {
    cr.text_extents(s).map(|e| e.width()).unwrap_or(0.0)
}

fn draw_text(cr: &CairoContext, x: f64, y: f64, s: &str) {
    cr.move_to(x, y);
    let _ = cr.show_text(s);
}

// ─── Core math (port of GetMaxDataValue, GetYAxisInfo, GetYValue) ────────────

fn get_max_data_value(series: &[SeriesInfo], data_count: usize) -> u64 {
    let mut max = 0u64;
    for s in series {
        let mut series_max = 0u64;
        for i in 0..data_count.min(s.data.len()) {
            if s.data[i] > series_max {
                series_max = s.data[i];
            }
        }
        if s.divider > 0 {
            series_max = (series_max + s.divider - 1) / s.divider;
        }
        if series_max > max {
            max = series_max;
        }
    }
    max
}

fn get_y_axis_info(max_value: u64, max_grids: i32) -> (i32, i32) {
    let max_value = if max_value < 2 { 2 } else { max_value };
    let mut max = max_value;
    let mut tens: i32 = 1;

    while max > 10 * max_grids as u64 {
        max /= 10;
        tens *= 10;
    }
    if max > 0 {
        max -= 1;
    }
    let raw_scale = max as i32 / max_grids;
    let scale = if raw_scale >= 5 {
        10
    } else if raw_scale >= 2 {
        5
    } else if raw_scale >= 1 {
        2
    } else {
        1
    };
    let y_units = scale * tens;
    let y_grids = ((max_value as i32) + y_units - 1) / y_units;
    (y_units, y_grids)
}

fn get_y_value(value: u64, bounds: &Rect, y_units: u64, y_grids: u64) -> f64 {
    let h = bounds.bottom - bounds.top;
    if y_units * y_grids == 0 {
        return bounds.bottom;
    }
    bounds.bottom - (value as f64) * h / (y_units * y_grids) as f64
}

// ─── Y-axis grid lines (port of DrawYGrids) ─────────────────────────────────

fn draw_y_grids(cr: &CairoContext, bounds: &Rect, y_units: i32, y_grids: i32, dec_places: i32) {
    cr.set_source_rgb(0.75, 0.75, 0.75);
    cr.set_line_width(0.5);
    let mut grid: i32 = 0;
    for _ in 0..=y_grids {
        let y = get_y_value(grid as u64, bounds, y_units as u64, y_grids as u64);

        // Grid line
        cr.move_to(bounds.left - TICK_SIZE, y);
        cr.line_to(bounds.right - 1.0, y);
        let _ = cr.stroke();

        // Y-axis label
        let label = if dec_places > 0 && grid > 0 {
            let s = format!("{}", grid);
            if s.len() as i32 > dec_places {
                let (whole, frac) = s.split_at(s.len() - dec_places as usize);
                format!("{}.{}", whole, frac)
            } else {
                let pad = dec_places as usize - s.len();
                format!("0.{}{}", "0".repeat(pad), s)
            }
        } else {
            format!("{}", grid)
        };

        cr.set_source_rgb(0.0, 0.0, 0.0);
        let w = text_width(cr, &label);
        draw_text(cr, bounds.left - w - 12.0, y + 4.0, &label);
        cr.set_source_rgb(0.75, 0.75, 0.75);

        grid += y_units;
    }
}

// ─── X-axis tick marks and labels (port of DrawXGrids) ───────────────────────

fn draw_x_grids(cr: &CairoContext, bounds: &Rect, data: &GraphData) {
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.set_line_width(1.0);
    let x_width = bounds.right - bounds.left - 1.0;
    let mut last_x: f64 = bounds.left;
    let mut last_pen: f64 = 0.0;

    for i in 0..=data.data_count {
        let x = bounds.left + (i as f64) * x_width / data.data_count as f64;

        // Tick mark
        if i == 0 || i == data.data_count {
            cr.move_to(x, bounds.top);
        } else {
            cr.move_to(x, bounds.bottom);
        }
        let mut tick_height = TICK_SIZE;
        if data.large_tick_interval > 0 && i % data.large_tick_interval == 0 {
            tick_height += data.large_tick_interval as f64;
        } else if data.med_tick_interval > 0 && i % data.med_tick_interval == 0 {
            tick_height += data.med_tick_interval as f64;
        }
        cr.line_to(x, bounds.bottom + tick_height);
        let _ = cr.stroke();

        // X-axis labels
        if i > 0 && i - 1 < data.x_labels.len() {
            let label = &data.x_labels[i - 1];
            if !label.is_empty() {
                let s_wd = text_width(cr, label);
                // Center between ticks if enough room
                let this_pen = if s_wd < x - last_x {
                    (x + last_x - s_wd + 2.0) / 2.0
                } else {
                    x - s_wd + 1.0
                };
                if this_pen > last_pen {
                    draw_text(cr, this_pen, bounds.bottom + X_AXIS_HT, label);
                    last_pen = this_pen + s_wd + 1.0;
                }
            }
        }
        last_x = x;
    }
}

// ─── Legend (port of DrawLegend) ─────────────────────────────────────────────

fn draw_legend(cr: &CairoContext, data: &GraphData, bounds: &Rect, mut top: f64) {
    for si in 0..data.series.len() {
        let series = &data.series[si];
        let the_type = if series.graph_type != GraphType::Default {
            series.graph_type
        } else {
            data.graph_type
        };

        let cx = bounds.right + BORDER_WIDTH;
        let cy = top;

        cr.set_source_rgb(series.color.r, series.color.g, series.color.b);
        match the_type {
            GraphType::Bar | GraphType::Pie => {
                cr.rectangle(cx, cy, LEGEND_BOX_WIDTH, LEGEND_BOX_HEIGHT);
                let _ = cr.fill();
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.rectangle(cx, cy, LEGEND_BOX_WIDTH, LEGEND_BOX_HEIGHT);
                let _ = cr.stroke();
            }
            GraphType::Line => {
                cr.set_line_width(2.0);
                cr.move_to(cx, cy + LEGEND_BOX_HEIGHT / 2.0);
                cr.line_to(cx + LEGEND_BOX_WIDTH - 1.0, cy + LEGEND_BOX_HEIGHT / 2.0);
                let _ = cr.stroke();
                cr.set_line_width(1.0);
            }
            GraphType::Area => {
                let (a0, a1, a2, a3, a4) = (6.0, 11.0, 8.0, 12.0, 7.0);
                cr.move_to(cx, cy + LEGEND_BOX_HEIGHT);
                cr.line_to(cx, cy + LEGEND_BOX_HEIGHT - a0);
                cr.line_to(cx + LEGEND_BOX_WIDTH / 4.0, cy + LEGEND_BOX_HEIGHT - a1);
                cr.line_to(cx + LEGEND_BOX_WIDTH / 2.0, cy + LEGEND_BOX_HEIGHT - a2);
                cr.line_to(cx + 3.0 * LEGEND_BOX_WIDTH / 4.0, cy + LEGEND_BOX_HEIGHT - a3);
                cr.line_to(cx + LEGEND_BOX_WIDTH, cy + LEGEND_BOX_HEIGHT - a4);
                cr.line_to(cx + LEGEND_BOX_WIDTH, cy + LEGEND_BOX_HEIGHT);
                cr.close_path();
                let _ = cr.fill_preserve();
                cr.set_source_rgb(0.0, 0.0, 0.0);
                let _ = cr.stroke();
            }
            GraphType::Circle => {
                let rx = cx + LEGEND_BOX_WIDTH / 2.0;
                let ry = cy + LEGEND_BOX_HEIGHT / 2.0;
                cr.arc(rx, ry, DOT_SIZE / 2.0, 0.0, 2.0 * PI);
                let _ = cr.fill();
            }
            GraphType::Diamond => {
                let mx = cx + LEGEND_BOX_WIDTH / 2.0;
                let my = cy + LEGEND_BOX_HEIGHT / 2.0;
                let d = DOT_SIZE / 2.0;
                cr.move_to(mx - d, my);
                cr.line_to(mx, my - d);
                cr.line_to(mx + d, my);
                cr.line_to(mx, my + d);
                cr.close_path();
                let _ = cr.fill();
            }
            GraphType::Square => {
                let sx = cx + (LEGEND_BOX_WIDTH - DOT_SIZE) / 2.0;
                let sy = cy + (LEGEND_BOX_HEIGHT - DOT_SIZE) / 2.0;
                cr.rectangle(sx, sy, DOT_SIZE, DOT_SIZE);
                let _ = cr.fill();
            }
            GraphType::Default => {}
        }

        // Legend label text
        cr.set_source_rgb(0.0, 0.0, 0.0);
        draw_text(
            cr,
            cx + LEGEND_BOX_WIDTH + LEGEND_TEXT_SPACE,
            cy + LEGEND_BOX_HEIGHT - 3.0,
            &series.label,
        );
        top += LEGEND_LINE_HT;
    }
}

// ─── Pie graph (port of DrawPieGraph) ────────────────────────────────────────

fn draw_pie_graph(cr: &CairoContext, data: &GraphData, bounds: &Rect) {
    if data.series.is_empty() || data.series[0].data.is_empty() {
        return;
    }

    let total: u64 = data.series[0].data.iter().sum();
    if total == 0 {
        return;
    }

    let cx = (bounds.left + bounds.right) / 2.0;
    let cy = (bounds.top + bounds.bottom) / 2.0;
    let radius = bounds.width().min(bounds.height()) / 2.0;

    let data_count = data.data_count.min(data.series[0].data.len());
    let mut sum: u64 = 0;

    for i in 0..data_count {
        let color = if i < data.series.len() {
            &data.series[i].color
        } else {
            &data.series[0].color
        };
        cr.set_source_rgb(color.r, color.g, color.b);

        let start_angle = (sum as f64) * 2.0 * PI / (total as f64) - PI / 2.0;
        let arc_angle = if i == data_count - 1 {
            2.0 * PI - (sum as f64) * 2.0 * PI / (total as f64)
        } else {
            (data.series[0].data[i] as f64) * 2.0 * PI / (total as f64)
        };

        cr.move_to(cx, cy);
        cr.arc(cx, cy, radius, start_angle, start_angle + arc_angle);
        cr.close_path();
        let _ = cr.fill();

        sum += data.series[0].data[i];
    }

    // Outline
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.set_line_width(1.0);
    cr.arc(cx, cy, radius, 0.0, 2.0 * PI);
    let _ = cr.stroke();
}

// ─── Main draw entry point (port of DrawGraph) ──────────────────────────────

/// Draw a graph into the given cairo context at (0,0) to (width, height).
/// This is the callback signature for `insert_custom_graphic`.
pub fn draw_graph(cr: &CairoContext, width: i32, height: i32, data: &GraphData) {
    if data.series.is_empty() || data.data_count == 0 {
        return;
    }

    setup_font(cr);

    // White background
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.rectangle(0.0, 0.0, width as f64, height as f64);
    let _ = cr.fill();

    let mut bounds = Rect {
        left: 0.0,
        top: 0.0,
        right: width as f64,
        bottom: height as f64,
    }
    .inset(BORDER_WIDTH, BORDER_WIDTH);

    // Reserve space for legend
    if data.legend {
        let mut max_label_width: f64 = 0.0;
        for s in &data.series {
            let w = text_width(cr, &s.label);
            if w > max_label_width {
                max_label_width = w;
            }
        }
        let legend_width = (max_label_width + BORDER_WIDTH + LEGEND_TEXT_SPACE + LEGEND_BOX_WIDTH)
            .max(LEGEND_MIN_WIDTH);
        bounds.right -= legend_width;
    }

    let y_legend = bounds.top + BORDER_WIDTH;

    // Pie graph special case
    if data.graph_type == GraphType::Pie
        || (!data.series.is_empty() && data.series[0].graph_type == GraphType::Pie)
    {
        let size = bounds.width().min(bounds.height());
        let pie_bounds = Rect {
            left: bounds.left,
            top: bounds.top,
            right: bounds.left + size,
            bottom: bounds.top + size,
        };
        draw_pie_graph(cr, data, &pie_bounds);
        if data.legend {
            draw_legend(cr, data, &bounds, y_legend);
        }
        return;
    }

    // Non-pie: bar/line/area/scatter charts
    bounds.bottom -= X_AXIS_HT;

    let mut max_value = get_max_data_value(&data.series, data.data_count);
    let mut divider: u64 = 1;
    if data.divider > 0 {
        max_value = (max_value + data.divider - 1) / data.divider;
        divider = data.divider;
    }

    // Reserve Y-axis label space
    let y_label = format!("{}", max_value);
    let label_width = (text_width(cr, &y_label) + 8.0).max(Y_AXIS_WIDTH);
    bounds.left += label_width;

    let (mut y_units, y_grids) =
        get_y_axis_info(max_value, ((bounds.bottom - bounds.top) / MIN_Y_HT) as i32);

    // Plot area background
    cr.set_source_rgb(0.95, 0.95, 0.95);
    cr.rectangle(bounds.left, bounds.top, bounds.width(), bounds.height());
    let _ = cr.fill();

    cr.set_source_rgb(0.0, 0.0, 0.0);
    draw_y_grids(cr, &bounds, y_units, y_grids, data.dec_place);
    draw_x_grids(cr, &bounds, data);
    let x_width = bounds.right - bounds.left;

    // Count bar graphs for width calculation
    let mut bar_count = 0i32;
    for s in &data.series {
        let t = if s.graph_type != GraphType::Default {
            s.graph_type
        } else {
            data.graph_type
        };
        if t == GraphType::Bar {
            bar_count += 1;
        }
    }
    let (bar_width, bar_margin) = if bar_count > 0 && data.data_count > 0 {
        let slot = x_width / data.data_count as f64;
        let margin = slot / 8.0;
        let bw = ((slot - margin * 2.0) / bar_count as f64).max(1.0);
        (bw, margin)
    } else {
        (1.0, 0.0)
    };

    // Precision adjustment
    if data.dec_place > 0 && divider > 10 && y_units < 10 {
        y_units *= 10;
        divider /= 10;
    }

    // Plot each series
    let mut bar_idx = 0;
    for si in 0..data.series.len() {
        let series = &data.series[si];
        let the_type = if series.graph_type != GraphType::Default {
            series.graph_type
        } else {
            data.graph_type
        };

        cr.set_source_rgb(series.color.r, series.color.g, series.color.b);
        if the_type == GraphType::Line {
            cr.set_line_width(2.0);
        } else {
            cr.set_line_width(1.0);
        }

        let data_count = data.data_count.min(series.data.len());
        let mut x1 = bounds.left;
        let mut last_data: u64 = 0;
        let mut first_x: f64 = 0.0;
        let mut area_started = false;

        for i in 0..data_count {
            let data_value = series.data[i];
            let mut use_y_units = y_units as u64;
            if divider != 1 {
                use_y_units *= divider;
            }
            if series.divider > 0 {
                use_y_units *= series.divider;
            }
            let y = get_y_value(data_value, &bounds, use_y_units, y_grids as u64);
            let x2 = bounds.left + ((i + 1) as f64) * x_width / data.data_count as f64;
            let x = (x1 + x2) / 2.0;

            match the_type {
                GraphType::Bar => {
                    if data_value > 0 {
                        let x_left = x1 + bar_margin + bar_idx as f64 * bar_width;
                        cr.set_source_rgb(series.color.r, series.color.g, series.color.b);
                        cr.rectangle(x_left, y, bar_width, bounds.bottom - y + 1.0);
                        let _ = cr.fill();
                        if bar_width > 3.0 {
                            cr.set_source_rgb(0.0, 0.0, 0.0);
                            cr.rectangle(x_left, y, bar_width + 1.0, bounds.bottom - y + 1.0);
                            let _ = cr.stroke();
                            cr.set_source_rgb(series.color.r, series.color.g, series.color.b);
                        }
                    }
                }
                GraphType::Line => {
                    if i > 0 && (last_data > 0 || data_value > 0) {
                        cr.line_to(x, y);
                        let _ = cr.stroke();
                        cr.move_to(x, y);
                    } else if data_count > 1 {
                        cr.move_to(x, y);
                    } else {
                        cr.move_to(x1, y);
                        cr.line_to(x, y);
                        let _ = cr.stroke();
                    }
                    last_data = data_value;
                }
                GraphType::Area => {
                    if area_started {
                        cr.line_to(x, y);
                    } else {
                        area_started = true;
                        if data_count > 1 {
                            cr.move_to(x, y);
                            first_x = x;
                        } else {
                            cr.move_to(x1, y);
                            cr.line_to(x, y);
                            first_x = x1;
                        }
                    }
                }
                GraphType::Circle => {
                    if data_value > 0 {
                        cr.arc(x, y, DOT_SIZE / 2.0, 0.0, 2.0 * PI);
                        let _ = cr.fill();
                    }
                }
                GraphType::Diamond => {
                    if data_value > 0 {
                        let d = DOT_SIZE / 2.0;
                        cr.move_to(x - d, y);
                        cr.line_to(x, y - d);
                        cr.line_to(x + d, y);
                        cr.line_to(x, y + d);
                        cr.close_path();
                        let _ = cr.fill();
                    }
                }
                GraphType::Square => {
                    if data_value > 0 {
                        let d = DOT_SIZE / 2.0;
                        cr.rectangle(x - d, y - d, DOT_SIZE, DOT_SIZE);
                        let _ = cr.fill();
                    }
                }
                _ => {}
            }
            x1 = x2;
        }

        if the_type == GraphType::Bar {
            bar_idx += 1;
        } else if the_type == GraphType::Area && area_started {
            // Close the area polygon
            cr.line_to(x1, bounds.bottom);
            cr.line_to(first_x, bounds.bottom);
            cr.close_path();
            let _ = cr.fill();
        }
    }

    // Legend
    if data.legend {
        draw_legend(cr, data, &bounds, y_legend);
    }
}
