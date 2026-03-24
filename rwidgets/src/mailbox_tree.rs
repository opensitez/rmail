//! MailboxTree — custom-drawn sidebar mailbox tree widget.
//!
//! Owner-drawn replacement for GtkTreeView. Renders mailbox rows with
//! icons, names, unread badges, expand/collapse arrows, hover/selection
//! highlighting, and DropTarget for message drag & drop.
//!
//! Uses GtkDrawingArea + cairo for rendering, GtkScrolledWindow for scrolling.

use gtk4::prelude::*;
use gtk4::glib;
use pangocairo::cairo;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

// ─── Constants ───────────────────────────────────────────────────────────────

const ROW_HEIGHT: f64 = 32.0;
const INDENT: f64 = 18.0;
const ICON_SIZE: f64 = 16.0;
const ICON_PAD: f64 = 8.0;
const TEXT_PAD: f64 = 6.0;
const BADGE_PAD: f64 = 8.0;
const BADGE_H_PAD: f64 = 6.0;
const BADGE_V_PAD: f64 = 2.0;
const BADGE_RADIUS: f64 = 8.0;
const ARROW_SIZE: f64 = 8.0;
const LEFT_MARGIN: f64 = 8.0;
const FONT_SIZE: f64 = 13.0;
const FONT_SIZE_BADGE: f64 = 10.0;

// ─── Flat row (flattened from tree for rendering) ────────────────────────────

#[derive(Debug, Clone)]
struct FlatRow {
    name: String,
    path: PathBuf,
    icon: &'static str,
    unread: usize,
    depth: usize,
    is_folder: bool,
    expanded: bool,
    has_children: bool,
    /// Index into the original tree for expand/collapse toggle.
    tree_indices: Vec<usize>,
}

// ─── Widget state ────────────────────────────────────────────────────────────

/// Theme colors for the sidebar.
#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    pub bg: (f64, f64, f64),
    pub text: (f64, f64, f64),
    pub hover: (f64, f64, f64, f64),  // rgba
    pub sel_bg: (f64, f64, f64, f64), // rgba
    pub accent: (f64, f64, f64),
    pub badge_bg: (f64, f64, f64),
    pub badge_fg: (f64, f64, f64),
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            bg: (0.97, 0.97, 0.97),
            text: (0.1, 0.1, 0.1),
            hover: (0.0, 0.0, 0.0, 0.04),
            sel_bg: (0.19, 0.51, 0.79, 0.15),
            accent: (0.19, 0.51, 0.79),
            badge_bg: (0.19, 0.51, 0.79),
            badge_fg: (1.0, 1.0, 1.0),
        }
    }
}

struct State {
    rows: Vec<FlatRow>,
    nodes: Vec<macmbx::store::Node>,
    selected: Option<usize>,
    hovered: Option<usize>,
    expanded_set: std::collections::HashSet<Vec<usize>>,
    theme: ThemeColors,
}

impl State {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            nodes: Vec::new(),
            selected: None,
            hovered: None,
            expanded_set: std::collections::HashSet::new(),
            theme: ThemeColors::default(),
        }
    }
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// A flat row data for external queries.
#[derive(Debug, Clone)]
pub struct MailboxInfo {
    pub name: String,
    pub path: String,
    pub is_folder: bool,
}

/// Build a modern owner-drawn mailbox tree sidebar.
///
/// Returns `(scrolled_window, activate_callback_setter, refresh_fn)`.
///
/// - The widget is a GtkScrolledWindow containing a GtkDrawingArea.
/// - `on_activate`: called with `(path, name, is_folder)` when a row is clicked.
/// - `on_drop`: called with `(src_mbox, indices, dst_mbox)` when messages are dropped.
/// - `refresh_fn`: call to rebuild the tree from a new store.
/// Callback for mailbox drag-to-folder: `(mbox_path, target_folder_path)`.
pub type OnMailboxDrop = Rc<RefCell<Option<Box<dyn Fn(&str, &str)>>>>;

pub fn build_mailbox_tree(
    nodes: &[macmbx::store::Node],
) -> (
    gtk4::ScrolledWindow,
    Rc<RefCell<Option<Box<dyn Fn(&str, &str, bool)>>>>,        // on_activate
    Rc<RefCell<Option<Box<dyn Fn(&str, Vec<usize>, &str)>>>>,  // on_drop (messages)
    Rc<dyn Fn(&[macmbx::store::Node])>,                        // refresh
    Rc<dyn Fn(ThemeColors)>,                                    // set_theme
    OnMailboxDrop,                                              // on_mailbox_drop
) {
    let state = Rc::new(RefCell::new(State::new()));

    // Start with all top-level expanded
    {
        let mut s = state.borrow_mut();
        s.nodes = nodes.to_vec();
        let mut rows = Vec::new();
        flatten_nodes(nodes, 0, &[], &mut rows, &mut s.expanded_set, true);
        s.rows = rows;
    }

    let area = gtk4::DrawingArea::new();
    let on_activate: Rc<RefCell<Option<Box<dyn Fn(&str, &str, bool)>>>> =
        Rc::new(RefCell::new(None));
    let on_drop: Rc<RefCell<Option<Box<dyn Fn(&str, Vec<usize>, &str)>>>> =
        Rc::new(RefCell::new(None));

    // ── Draw function ──
    {
        let st = state.clone();
        area.set_draw_func(move |_area, cr, width, _height| {
            let s = st.borrow();
            draw_tree(cr, width as f64, &s);
        });
    }

    // ── Update content height for scrolling ──
    let update_height = {
        let st = state.clone();
        let a = area.clone();
        Rc::new(move || {
            let h = st.borrow().rows.len() as f64 * ROW_HEIGHT;
            a.set_content_height(h.max(100.0) as i32);
            a.queue_draw();
        })
    };
    update_height();

    // ── Mouse hover ──
    {
        let st = state.clone();
        let a = area.clone();
        let motion = gtk4::EventControllerMotion::new();
        motion.connect_motion(move |_ctrl, _x, y| {
            let row = (y / ROW_HEIGHT) as usize;
            let mut s = st.borrow_mut();
            let new_hover = if row < s.rows.len() { Some(row) } else { None };
            if s.hovered != new_hover {
                s.hovered = new_hover;
                a.queue_draw();
            }
        });
        let st2 = state.clone();
        let a2 = area.clone();
        motion.connect_leave(move |_ctrl| {
            st2.borrow_mut().hovered = None;
            a2.queue_draw();
        });
        area.add_controller(motion);
    }

    // ── Click handler ──
    {
        let st = state.clone();
        let a = area.clone();
        let act = on_activate.clone();
        let uh = update_height.clone();
        let click = gtk4::GestureClick::new();
        click.set_button(1);
        click.connect_released(move |_g, _n, x, y| {
            let row_idx = (y / ROW_HEIGHT) as usize;
            let mut s = st.borrow_mut();
            if row_idx >= s.rows.len() { return; }

            let has_children = s.rows[row_idx].has_children;
            let arrow_x = LEFT_MARGIN + s.rows[row_idx].depth as f64 * INDENT;
            let row_path = s.rows[row_idx].path.to_string_lossy().to_string();
            let row_name = s.rows[row_idx].name.clone();
            let is_folder = s.rows[row_idx].is_folder;
            let tree_indices = s.rows[row_idx].tree_indices.clone();

            // Check if click is on the expand arrow area
            if has_children && x >= arrow_x && x < arrow_x + ARROW_SIZE + 4.0 {
                // Toggle expand/collapse
                if s.expanded_set.contains(&tree_indices) {
                    s.expanded_set.remove(&tree_indices);
                } else {
                    s.expanded_set.insert(tree_indices);
                }
                
                // Re-flatten the tree using the cached nodes
                let cached_nodes = s.nodes.clone();
                let mut new_rows = Vec::new();
                flatten_nodes(&cached_nodes, 0, &[], &mut new_rows, &mut s.expanded_set, false);
                s.rows = new_rows;
                
                drop(s);
                a.queue_draw();
                uh();
                return;
            }

            // Select and activate
            s.selected = Some(row_idx);
            let path = row_path;
            let name = row_name;
            drop(s);
            a.queue_draw();

            if let Some(ref cb) = *act.borrow() {
                cb(&path, &name, is_folder);
            }
        });
        area.add_controller(click);
    }

    // ── Right-click ──
    {
        let st = state.clone();
        let rclick = gtk4::GestureClick::new();
        rclick.set_button(3);
        rclick.connect_released(move |_g, _n, _x, y| {
            let row_idx = (y / ROW_HEIGHT) as usize;
            let mut s = st.borrow_mut();
            if row_idx < s.rows.len() {
                s.selected = Some(row_idx);
            }
        });
        area.add_controller(rclick);
    }

    let on_mailbox_drop: OnMailboxDrop = Rc::new(RefCell::new(None));

    // ── Custom drop target: register drop zone ──
    {
        let st = state.clone();
        let st2 = state.clone();
        let a2 = area.clone();
        crate::drag_state::register_drop_zone(&area, 
            move |_x, y| {
                let s = st.borrow();
                let row_idx = (y / ROW_HEIGHT) as usize;
                if row_idx < s.rows.len() && !s.rows[row_idx].is_folder {
                    Some(s.rows[row_idx].path.to_string_lossy().to_string())
                } else {
                    None
                }
            },
            move |opt_y| {
                let mut s = st2.borrow_mut();
                if let Some(y) = opt_y {
                    let row_idx = (y / ROW_HEIGHT) as usize;
                    if row_idx < s.rows.len() && !s.rows[row_idx].is_folder {
                        let new_hover = Some(row_idx);
                        if s.hovered != new_hover {
                            s.hovered = new_hover;
                            a2.queue_draw();
                        }
                        return;
                    }
                }
                if s.hovered.is_some() {
                    s.hovered = None;
                    a2.queue_draw();
                }
            }
        );
    }

    let scroll = gtk4::ScrolledWindow::builder()
        .child(&area)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .build();

    // ── Refresh function ──
    let refresh: Rc<dyn Fn(&[macmbx::store::Node])> = {
        let st = state.clone();
        let uh = update_height.clone();
        Rc::new(move |nodes: &[macmbx::store::Node]| {
            let mut s = st.borrow_mut();
            s.nodes = nodes.to_vec();
            let mut rows = Vec::new();
            flatten_nodes(nodes, 0, &[], &mut rows, &mut s.expanded_set, false);
            s.rows = rows;
            drop(s);
            uh();
        })
    };

    // ── Theme setter ──
    let set_theme: Rc<dyn Fn(ThemeColors)> = {
        let st = state.clone();
        let a = area.clone();
        Rc::new(move |colors: ThemeColors| {
            st.borrow_mut().theme = colors;
            a.queue_draw();
        })
    };

    (scroll, on_activate, on_drop, refresh, set_theme, on_mailbox_drop)
}

/// Get info about the currently selected mailbox.
pub fn get_selected(state_rows: &[FlatRow], selected: Option<usize>) -> Option<MailboxInfo> {
    let idx = selected?;
    let row = state_rows.get(idx)?;
    Some(MailboxInfo {
        name: row.name.clone(),
        path: row.path.to_string_lossy().to_string(),
        is_folder: row.is_folder,
    })
}

// ─── Tree flattening ─────────────────────────────────────────────────────────

fn flatten_nodes(
    nodes: &[macmbx::store::Node],
    depth: usize,
    parent_indices: &[usize],
    out: &mut Vec<FlatRow>,
    expanded: &mut std::collections::HashSet<Vec<usize>>,
    init_expand: bool,
) {
    for (i, node) in nodes.iter().enumerate() {
        let mut indices = parent_indices.to_vec();
        indices.push(i);

        let is_folder = node.node_type == macmbx::store::NodeType::Folder;
        let has_children = !node.children.is_empty();

        // On first init, expand top-level
        if init_expand && depth == 0 && has_children {
            expanded.insert(indices.clone());
        }

        let is_expanded = expanded.contains(&indices);

        let icon = if is_folder {
            if is_expanded { "folder-open-symbolic" } else { "folder-symbolic" }
        } else {
            match node.mbox_type {
                macmbx::toc::MailboxType::In => "mail-inbox-symbolic",
                macmbx::toc::MailboxType::Out => "mail-outbox-symbolic",
                macmbx::toc::MailboxType::Trash => "user-trash-symbolic",
                macmbx::toc::MailboxType::Junk => "mail-mark-junk-symbolic",
                _ => "mail-unread-symbolic",
            }
        };

        let unread = if is_folder { 0 } else { node.unread.unwrap_or(0) };

        out.push(FlatRow {
            name: node.name.clone(),
            path: node.path.clone(),
            icon,
            unread,
            depth,
            is_folder,
            expanded: is_expanded,
            has_children,
            tree_indices: indices.clone(),
        });

        if is_expanded && has_children {
            flatten_nodes(&node.children, depth + 1, &indices, out, expanded, init_expand);
        }
    }
}

// ─── Cairo rendering ─────────────────────────────────────────────────────────

fn draw_tree(cr: &cairo::Context, width: f64, state: &State) {
    let t = &state.theme;

    // Background
    cr.set_source_rgb(t.bg.0, t.bg.1, t.bg.2);
    cr.rectangle(0.0, 0.0, width, state.rows.len() as f64 * ROW_HEIGHT + 10.0);
    let _ = cr.fill();

    let font = pangocairo::functions::create_context(cr);
    let layout = pango::Layout::new(&font);
    let mut font_desc = pango::FontDescription::new();
    font_desc.set_family("sans-serif");

    for (i, row) in state.rows.iter().enumerate() {
        let y = i as f64 * ROW_HEIGHT;
        let x = LEFT_MARGIN + row.depth as f64 * INDENT;

        // Hover highlight
        if state.hovered == Some(i) {
            cr.set_source_rgba(t.hover.0, t.hover.1, t.hover.2, t.hover.3);
            cr.rectangle(0.0, y, width, ROW_HEIGHT);
            let _ = cr.fill();
        }

        // Selection highlight
        if state.selected == Some(i) {
            cr.set_source_rgba(t.sel_bg.0, t.sel_bg.1, t.sel_bg.2, t.sel_bg.3);
            cr.rectangle(0.0, y, width, ROW_HEIGHT);
            let _ = cr.fill();
            // Left accent bar
            cr.set_source_rgb(t.accent.0, t.accent.1, t.accent.2);
            cr.rectangle(0.0, y + 4.0, 3.0, ROW_HEIGHT - 8.0);
            let _ = cr.fill();
        }

        let mut cx = x;

        // Expand/collapse arrow for folders with children
        if row.has_children {
            cr.set_source_rgba(t.text.0, t.text.1, t.text.2, 0.4);
            let ax = cx + ARROW_SIZE / 2.0;
            let ay = y + ROW_HEIGHT / 2.0;
            if row.expanded {
                // Down arrow ▼
                cr.move_to(ax - 4.0, ay - 2.0);
                cr.line_to(ax + 4.0, ay - 2.0);
                cr.line_to(ax, ay + 3.0);
            } else {
                // Right arrow ▶
                cr.move_to(ax - 2.0, ay - 4.0);
                cr.line_to(ax + 3.0, ay);
                cr.line_to(ax - 2.0, ay + 4.0);
            }
            cr.close_path();
            let _ = cr.fill();
        }
        cx += ARROW_SIZE + 4.0;

        // Icon (using pango to render icon name as text — fallback)
        // In a real app we'd use gtk4::IconTheme, but in DrawingArea
        // we render a small symbolic representation
        draw_icon(cr, cx, y + (ROW_HEIGHT - ICON_SIZE) / 2.0, row.icon, ICON_SIZE);
        cx += ICON_SIZE + ICON_PAD;

        // Name
        let is_unread = row.unread > 0;
        font_desc.set_size((FONT_SIZE * pango::SCALE as f64) as i32);
        if is_unread {
            font_desc.set_weight(pango::Weight::Bold);
        } else {
            font_desc.set_weight(pango::Weight::Normal);
        }
        layout.set_font_description(Some(&font_desc));
        layout.set_text(&row.name);

        cr.set_source_rgb(t.text.0, t.text.1, t.text.2);
        cr.move_to(cx, y + (ROW_HEIGHT - FONT_SIZE) / 2.0 - 1.0);
        pangocairo::functions::show_layout(cr, &layout);

        // Unread badge
        if row.unread > 0 {
            let badge_text = format!("{}", row.unread);
            font_desc.set_size((FONT_SIZE_BADGE * pango::SCALE as f64) as i32);
            font_desc.set_weight(pango::Weight::Bold);
            layout.set_font_description(Some(&font_desc));
            layout.set_text(&badge_text);
            let (tw, _th) = layout.pixel_size();

            let bw = tw as f64 + BADGE_H_PAD * 2.0;
            let bh = FONT_SIZE_BADGE + BADGE_V_PAD * 2.0 + 2.0;
            let bx = width - bw - BADGE_PAD;
            let by = y + (ROW_HEIGHT - bh) / 2.0;

            // Badge background
            cr.set_source_rgb(t.badge_bg.0, t.badge_bg.1, t.badge_bg.2);
            rounded_rect(cr, bx, by, bw, bh, BADGE_RADIUS);
            let _ = cr.fill();

            // Badge text
            cr.set_source_rgb(t.badge_fg.0, t.badge_fg.1, t.badge_fg.2);
            cr.move_to(bx + BADGE_H_PAD, by + BADGE_V_PAD);
            pangocairo::functions::show_layout(cr, &layout);
        }
    }
}

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(h / 2.0).min(w / 2.0);
    cr.move_to(x + r, y);
    cr.line_to(x + w - r, y);
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.line_to(x + w, y + h - r);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.line_to(x + r, y + h);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.line_to(x, y + r);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
}

/// Draw a simple icon glyph. Since we can't use GtkIconTheme in a DrawingArea,
/// we draw minimal symbolic representations.
fn draw_icon(cr: &cairo::Context, x: f64, y: f64, icon: &str, size: f64) {
    let s = size;
    let cx = x + s / 2.0;
    let cy = y + s / 2.0;

    match icon {
        "mail-inbox-symbolic" => {
            // Tray shape ⊔
            cr.set_source_rgb(0.19, 0.51, 0.79);
            cr.set_line_width(1.5);
            cr.move_to(x + 2.0, cy - 2.0);
            cr.line_to(x + 2.0, y + s - 2.0);
            cr.line_to(x + s - 2.0, y + s - 2.0);
            cr.line_to(x + s - 2.0, cy - 2.0);
            let _ = cr.stroke();
            // Arrow down
            cr.move_to(cx, y + 1.0);
            cr.line_to(cx, cy + 1.0);
            let _ = cr.stroke();
            cr.move_to(cx - 3.0, cy - 2.0);
            cr.line_to(cx, cy + 1.0);
            cr.line_to(cx + 3.0, cy - 2.0);
            let _ = cr.stroke();
        }
        "mail-outbox-symbolic" => {
            // Tray + arrow up
            cr.set_source_rgb(0.22, 0.63, 0.41);
            cr.set_line_width(1.5);
            cr.move_to(x + 2.0, cy);
            cr.line_to(x + 2.0, y + s - 2.0);
            cr.line_to(x + s - 2.0, y + s - 2.0);
            cr.line_to(x + s - 2.0, cy);
            let _ = cr.stroke();
            cr.move_to(cx, y + s - 4.0);
            cr.line_to(cx, y + 2.0);
            let _ = cr.stroke();
            cr.move_to(cx - 3.0, y + 5.0);
            cr.line_to(cx, y + 2.0);
            cr.line_to(cx + 3.0, y + 5.0);
            let _ = cr.stroke();
        }
        "user-trash-symbolic" => {
            cr.set_source_rgb(0.6, 0.6, 0.6);
            cr.set_line_width(1.5);
            // Lid
            cr.move_to(x + 3.0, y + 4.0);
            cr.line_to(x + s - 3.0, y + 4.0);
            let _ = cr.stroke();
            // Handle
            cr.move_to(cx - 2.0, y + 4.0);
            cr.line_to(cx - 2.0, y + 2.0);
            cr.line_to(cx + 2.0, y + 2.0);
            cr.line_to(cx + 2.0, y + 4.0);
            let _ = cr.stroke();
            // Body
            cr.move_to(x + 4.0, y + 5.0);
            cr.line_to(x + 5.0, y + s - 2.0);
            cr.line_to(x + s - 5.0, y + s - 2.0);
            cr.line_to(x + s - 4.0, y + 5.0);
            let _ = cr.stroke();
        }
        "mail-mark-junk-symbolic" => {
            // Flame shape
            cr.set_source_rgb(0.9, 0.24, 0.24);
            cr.set_line_width(1.5);
            cr.move_to(cx, y + s - 2.0);
            cr.curve_to(x + 3.0, cy + 2.0, x + 4.0, cy - 3.0, cx, y + 1.0);
            cr.curve_to(x + s - 4.0, cy - 3.0, x + s - 3.0, cy + 2.0, cx, y + s - 2.0);
            let _ = cr.stroke();
        }
        "folder-symbolic" | "folder-open-symbolic" => {
            cr.set_source_rgb(0.88, 0.62, 0.18);
            cr.set_line_width(1.5);
            // Folder tab
            cr.move_to(x + 2.0, y + 4.0);
            cr.line_to(x + 2.0, y + 2.0);
            cr.line_to(cx - 1.0, y + 2.0);
            cr.line_to(cx + 1.0, y + 4.0);
            let _ = cr.stroke();
            // Folder body
            if icon == "folder-open-symbolic" {
                cr.move_to(x + 1.0, y + 4.0);
                cr.line_to(x + s - 1.0, y + 4.0);
                cr.line_to(x + s - 3.0, y + s - 2.0);
                cr.line_to(x + 3.0, y + s - 2.0);
                cr.close_path();
                let _ = cr.stroke();
            } else {
                cr.rectangle(x + 2.0, y + 4.0, s - 4.0, s - 6.0);
                let _ = cr.stroke();
            }
        }
        _ => {
            // Generic mail icon ✉
            cr.set_source_rgb(0.5, 0.5, 0.5);
            cr.set_line_width(1.2);
            cr.rectangle(x + 1.0, y + 3.0, s - 2.0, s - 6.0);
            let _ = cr.stroke();
            // Envelope flap
            cr.move_to(x + 1.0, y + 3.0);
            cr.line_to(cx, cy + 1.0);
            cr.line_to(x + s - 1.0, y + 3.0);
            let _ = cr.stroke();
        }
    }
}
