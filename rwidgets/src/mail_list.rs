//! MailList — modern owner-drawn columnar message list widget.
//!
//! Replaces GtkTreeView with a clean columnar list with sortable headers.
//! Layout:
//!   Header:  ●  | From          | Subject          | Date       | Size
//!   Row:     ●  | John Doe      | Re: Meeting...   | Mar 24     | 3.2K
//!
//! Features: column headers, sort by click, multi-select, keyboard nav,
//! DragSource, theme colors, scrollable via GtkScrolledWindow.

use gtk4::prelude::*;
use gtk4::glib;
use pangocairo::cairo;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Mutex;

// Global shared theme — updated by the app, read by all MailList instances.
static GLOBAL_THEME: Mutex<Option<ThemeColors>> = Mutex::new(None);

/// Set the global theme for all MailList widgets. Call this from the app's
/// theme toggle, then queue_draw on all open lists.
pub fn set_global_theme(colors: ThemeColors) {
    *GLOBAL_THEME.lock().unwrap() = Some(colors);
}

fn current_theme(fallback: &ThemeColors) -> ThemeColors {
    GLOBAL_THEME.lock().ok()
        .and_then(|g| *g)
        .unwrap_or(*fallback)
}

// ─── Constants ───────────────────────────────────────────────────────────────

const ROW_HEIGHT: f64 = 28.0;
const HEADER_HEIGHT: f64 = 30.0;
const FONT_SIZE: f64 = 12.5;
const HEADER_FONT: f64 = 11.0;
const UNREAD_DOT_R: f64 = 3.5;
const SEP_HEIGHT: f64 = 1.0;

// Column layout: fractions of total width
const COL_STATUS_W: f64 = 28.0;  // fixed px
const COL_PRI_W: f64 = 24.0;     // fixed px
const COL_DATE_W: f64 = 90.0;    // fixed px
const COL_SIZE_W: f64 = 55.0;    // fixed px
// From and Subject share remaining space: 40% / 60%
const FROM_FRAC: f64 = 0.40;

// ─── Theme ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    pub bg: (f64, f64, f64),
    pub header_bg: (f64, f64, f64, f64),
    pub text: (f64, f64, f64),
    pub text_secondary: (f64, f64, f64),
    pub hover: (f64, f64, f64, f64),
    pub sel_bg: (f64, f64, f64, f64),
    pub accent: (f64, f64, f64),
    pub separator: (f64, f64, f64, f64),
    pub unread_dot: (f64, f64, f64),
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            bg: (1.0, 1.0, 1.0),
            header_bg: (0.96, 0.96, 0.96, 1.0),
            text: (0.1, 0.1, 0.1),
            text_secondary: (0.45, 0.45, 0.45),
            hover: (0.0, 0.0, 0.0, 0.03),
            sel_bg: (0.19, 0.51, 0.79, 0.12),
            accent: (0.19, 0.51, 0.79),
            separator: (0.0, 0.0, 0.0, 0.08),
            unread_dot: (0.15, 0.39, 0.92),
        }
    }
}

// ─── Row data ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MsgRow {
    pub index: i32,
    pub from: String,
    pub subject: String,
    pub date: String,
    pub size: String,
    pub unread: bool,
    pub priority: i8,
    pub label_color: String,
}

// ─── Sort ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortCol { Status, Priority, From, Subject, Date, Size }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortDir { Asc, Desc }

// ─── Widget state ────────────────────────────────────────────────────────────

const RESIZE_HIT: f64 = 5.0; // px either side of column border for resize grab


#[derive(Debug, Clone, PartialEq)]
pub struct ColumnConfig {
    pub status: bool,
    pub priority: bool,
    pub attachments: bool,
    pub label: bool,
    pub who: bool,
    pub date: bool,
    pub size: bool,
    pub server: bool,
    pub mood: bool,
    pub junk: bool,
    /// Draw a horizontal separator line between each row.
    pub draw_horiz_lines: bool,
    /// Draw vertical separator lines between columns.
    pub draw_vert_lines: bool,
    /// Auto-mark read after message is selected for this many seconds (0 = disabled).
    pub mark_read_delay_secs: u32,
}

impl Default for ColumnConfig {
    fn default() -> Self {
        Self {
            status: true, priority: true, attachments: false, label: false,
            who: true, date: true, size: true, server: false, mood: false, junk: false,
            draw_horiz_lines: true, draw_vert_lines: false, mark_read_delay_secs: 0,
        }
    }
}

struct State {

    rows: Vec<MsgRow>,
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    hovered: Option<usize>,
    theme: ThemeColors,
    mbox_path: String,
    sort_col: SortCol,
    sort_dir: SortDir,
    // Resizable column widths (persisted across redraws)
    from_w: f64,
    date_w: f64,
    size_w: f64,
    // Drag resize state
    resize_col: Option<usize>, // which border is being dragged (0=from|subj, 1=subj|date, 2=date|size)
    resize_start_x: f64,
    resize_start_w: f64,
    drag_popover: Option<gtk4::Popover>,
    col_config: ColumnConfig,
}

// ─── Column geometry ─────────────────────────────────────────────────────────

struct ColLayout {
    status_x: f64, status_w: f64,
    pri_x: f64, pri_w: f64,
    attach_x: f64, attach_w: f64,
    label_x: f64, label_w: f64,
    from_x: f64, from_w: f64,
    subject_x: f64, subject_w: f64,
    date_x: f64, date_w: f64,
    size_x: f64, size_w: f64,
    server_x: f64, server_w: f64,
    mood_x: f64, mood_w: f64,
    junk_x: f64, junk_w: f64,
}

fn col_layout_from_state(width: f64, state: &State) -> ColLayout {
    let mut x = 0.0;
    let status_x = x; let status_w = if state.col_config.status { COL_STATUS_W } else { 0.0 }; x += status_w;
    let pri_x = x;    let pri_w = if state.col_config.priority { COL_PRI_W } else { 0.0 }; x += pri_w;
    let attach_x = x; let attach_w = if state.col_config.attachments { 24.0 } else { 0.0 }; x += attach_w;
    let label_x = x;  let label_w = if state.col_config.label { 40.0 } else { 0.0 }; x += label_w;
    let from_x = x;   let from_w = if state.col_config.who { state.from_w } else { 0.0 }; x += from_w;
    
    let subject_x = x;
    
    let mut right_w = 0.0;
    let size_w = if state.col_config.size { state.size_w } else { 0.0 }; right_w += size_w;
    let date_w = if state.col_config.date { state.date_w } else { 0.0 }; right_w += date_w;
    let server_w = if state.col_config.server { 50.0 } else { 0.0 }; right_w += server_w;
    let mood_w = if state.col_config.mood { 40.0 } else { 0.0 }; right_w += mood_w;
    let junk_w = if state.col_config.junk { 40.0 } else { 0.0 }; right_w += junk_w;
    
    let subject_w = (width - x - right_w).max(60.0);
    x += subject_w;
    
    let date_x = x; x += date_w;
    let size_x = x; x += size_w;
    let server_x = x; x += server_w;
    let mood_x = x; x += mood_w;
    let junk_x = x; x += junk_w;
    
    ColLayout {
        status_x, status_w, pri_x, pri_w, attach_x, attach_w, label_x, label_w,
        from_x, from_w, subject_x, subject_w, date_x, date_w, size_x, size_w,
        server_x, server_w, mood_x, mood_w, junk_x, junk_w
    }
}

/// Find which resize border the x coordinate is near (returns 0=from|subj, 1=subj|date, 2=date|size)
fn hit_resize_border(x: f64, cl: &ColLayout) -> Option<usize> {
    let borders = [
        cl.subject_x,                // border between From and Subject
        cl.date_x,                   // border between Subject and Date
        cl.size_x,                   // border between Date and Size
    ];
    for (i, &bx) in borders.iter().enumerate() {
        if (x - bx).abs() < RESIZE_HIT { return Some(i); }
    }
    None
}

// ─── Public API ──────────────────────────────────────────────────────────────

pub struct MailListWidget {
    pub widget: gtk4::Box,  // vertical: header + scrolled area
    state: Rc<RefCell<State>>,
    area: gtk4::DrawingArea,
    pub on_selection_changed: Rc<RefCell<Option<Box<dyn Fn(&str, &[i32])>>>>,
    pub on_activate: Rc<RefCell<Option<Box<dyn Fn(&str, i32)>>>>,
    /// Called on right-click with (x, y) coordinates relative to the widget.
    pub on_right_click: Rc<RefCell<Option<Box<dyn Fn(i32, i32)>>>>,
}

impl MailListWidget {
    pub fn selected_indices(&self) -> Vec<i32> {
        let s = self.state.borrow();
        s.selected.iter().filter_map(|&i| s.rows.get(i).map(|r| r.index)).collect()
    }

    pub fn selected_index(&self) -> Option<i32> {
        let s = self.state.borrow();
        if s.selected.len() != 1 { return None; }
        let row_idx = *s.selected.iter().next()?;
        Some(s.rows[row_idx].index)
    }

    pub fn mbox_path(&self) -> String { self.state.borrow().mbox_path.clone() }

    pub fn remove_indices(&self, toc_indices: &[i32]) {
        let mut s = self.state.borrow_mut();
        s.rows.retain(|r| !toc_indices.contains(&r.index));
        s.selected.clear();
        drop(s);
        self.update_height();
    }

    pub fn set_theme(&self, colors: ThemeColors) {
        self.state.borrow_mut().theme = colors;
        self.area.queue_draw();
    }

    pub fn queue_draw(&self) { self.area.queue_draw(); }

    fn update_height(&self) {
        let h = self.state.borrow().rows.len() as f64 * ROW_HEIGHT;
        self.area.set_content_height(h.max(50.0) as i32);
        self.area.queue_draw();
    }

    pub fn set_column_config(&self, config: crate::mail_list::ColumnConfig) {
        self.state.borrow_mut().col_config = config;
        self.area.queue_draw();
        if let Some(parent) = self.area.parent() {
            parent.queue_draw();
        }
    }
}

pub fn build_mail_list(mbox_path: &str, rows: Vec<MsgRow>) -> MailListWidget {
    let state = Rc::new(RefCell::new(State {
        rows,
        selected: BTreeSet::new(),
        anchor: None,
        hovered: None,
        theme: ThemeColors::default(),
        mbox_path: mbox_path.to_string(),
        sort_col: SortCol::Date,
        sort_dir: SortDir::Desc,
        from_w: 180.0,
        date_w: 90.0,
        size_w: 55.0,
        resize_col: None,
        resize_start_x: 0.0,
        resize_start_w: 0.0,
        drag_popover: None,
        col_config: ColumnConfig::default(),
    }));

    let on_selection_changed: Rc<RefCell<Option<Box<dyn Fn(&str, &[i32])>>>> =
        Rc::new(RefCell::new(None));
    let on_activate: Rc<RefCell<Option<Box<dyn Fn(&str, i32)>>>> =
        Rc::new(RefCell::new(None));
    let on_right_click: Rc<RefCell<Option<Box<dyn Fn(i32, i32)>>>> =
        Rc::new(RefCell::new(None));

    // ── Header bar (drawn with cairo for consistency) ──
    let header_area = gtk4::DrawingArea::new();
    header_area.set_content_height(HEADER_HEIGHT as i32);
    {
        let st = state.clone();
        header_area.set_draw_func(move |_a, cr, width, _h| {
            let s = st.borrow();
            draw_header(cr, width as f64, &s);
        });
    }

    // ── Data area (created early so header handlers can reference it) ──
    let area = gtk4::DrawingArea::new();
    {
        let h = state.borrow().rows.len() as f64 * ROW_HEIGHT;
        area.set_content_height(h.max(50.0) as i32);
    }

    // Header click → sort (ignore if near a resize border)
    {
        let st = state.clone();
        let ha = header_area.clone();
        let ar = area.clone();
        let click = gtk4::GestureClick::new();
        click.set_button(1);
        click.connect_released(move |_g, _n, x, _y| {
            let mut s = st.borrow_mut();
            let cl = col_layout_from_state(ha.width() as f64, &s);
            // Skip if near a resize border
            if hit_resize_border(x, &cl).is_some() { return; }
            let new_col = if x < cl.pri_x { SortCol::Status }
                else if x < cl.from_x { SortCol::Priority }
                else if x < cl.subject_x { SortCol::From }
                else if x < cl.date_x { SortCol::Subject }
                else if x < cl.size_x { SortCol::Date }
                else { SortCol::Size };
            if s.sort_col == new_col {
                s.sort_dir = if s.sort_dir == SortDir::Asc { SortDir::Desc } else { SortDir::Asc };
            } else {
                s.sort_col = new_col;
                s.sort_dir = SortDir::Asc;
            }
            let col = s.sort_col;
            let dir = s.sort_dir;
            sort_rows(&mut s.rows, col, dir);
            s.selected.clear();
            drop(s);
            ha.queue_draw();
            ar.queue_draw();
        });
        header_area.add_controller(click);
    }

    // Header drag → column resize
    {
        let st = state.clone();
        let ha = header_area.clone();
        let ar = area.clone();
        let drag = gtk4::GestureDrag::new();
        drag.set_button(1);
        drag.connect_drag_begin(move |_g, x, _y| {
            let mut s = st.borrow_mut();
            let cl = col_layout_from_state(ha.width() as f64, &s);
            if let Some(border) = hit_resize_border(x, &cl) {
                s.resize_col = Some(border);
                s.resize_start_x = x;
                s.resize_start_w = match border {
                    0 => s.from_w,
                    1 => {
                        let subj_w = (ha.width() as f64 - cl.from_x - s.from_w - s.date_w - s.size_w).max(60.0);
                        subj_w
                    }
                    2 => s.date_w,
                    _ => 0.0,
                };
            } else {
                s.resize_col = None;
            }
        });
        let st2 = state.clone();
        let ha2 = header_area.clone();
        let ar2 = area.clone();
        drag.connect_drag_update(move |_g, dx, _dy| {
            let mut s = st2.borrow_mut();
            if let Some(border) = s.resize_col {
                let new_w = (s.resize_start_w + dx).max(40.0);
                match border {
                    0 => s.from_w = new_w,      // resize From column
                    1 => {
                        // resize Subject: adjust from_w inversely
                        // Actually we adjust date_w start by moving the border
                        // Simpler: resize From by dx, Subject shrinks automatically
                        s.from_w = (s.resize_start_w + dx).max(40.0); // treat as From resize
                    }
                    2 => s.date_w = new_w,       // resize Date column
                    _ => {}
                }
                ha2.queue_draw();
                ar2.queue_draw();
            }
        });
        let st3 = state.clone();
        drag.connect_drag_end(move |_, _, _| {
            st3.borrow_mut().resize_col = None;
        });
        header_area.add_controller(drag);
    }

    // Header cursor change on resize borders
    {
        let st = state.clone();
        let ha = header_area.clone();
        let motion = gtk4::EventControllerMotion::new();
        motion.connect_motion(move |_ctrl, x, _y| {
            let s = st.borrow();
            let cl = col_layout_from_state(ha.width() as f64, &s);
            let cursor_name = if hit_resize_border(x, &cl).is_some() {
                "col-resize"
            } else {
                "default"
            };
            let cursor = gtk4::gdk::Cursor::from_name(cursor_name, None);
            ha.set_cursor(cursor.as_ref());
        });
        header_area.add_controller(motion);
    }

    // Draw rows
    {
        let st = state.clone();
        area.set_draw_func(move |_a, cr, width, _h| {
            let s = st.borrow();
            draw_rows(cr, width as f64, &s);
        });
    }

    // Hover
    {
        let st = state.clone();
        let a = area.clone();
        let motion = gtk4::EventControllerMotion::new();
        motion.connect_motion(move |_, _x, y| {
            let row = (y / ROW_HEIGHT) as usize;
            let mut s = st.borrow_mut();
            let h = if row < s.rows.len() { Some(row) } else { None };
            if s.hovered != h { s.hovered = h; a.queue_draw(); }
        });
        let st2 = state.clone();
        let a2 = area.clone();
        motion.connect_leave(move |_| { st2.borrow_mut().hovered = None; a2.queue_draw(); });
        area.add_controller(motion);
    }

    // Click — selection + double-click activate (single gesture to avoid borrow conflicts)
    {
        let st = state.clone();
        let a = area.clone();
        let ha = header_area.clone();
        let osc = on_selection_changed.clone();
        let oa = on_activate.clone();
        let click = gtk4::GestureClick::new();
        click.set_button(1);
        click.connect_released(move |g, n_press, _x, y| {
            let row_idx = (y / ROW_HEIGHT) as usize;

            // Double-click → activate
            if n_press == 2 {
                let s = st.borrow();
                if row_idx >= s.rows.len() { return; }
                let mbox = s.mbox_path.clone();
                let idx = s.rows[row_idx].index;
                drop(s);
                if let Some(ref cb) = *oa.borrow() { cb(&mbox, idx); }
                return;
            }

            // Single click → selection
            let mut s = st.borrow_mut();
            if row_idx >= s.rows.len() { return; }

            let shift = g.current_event()
                .map(|e| e.modifier_state().contains(gtk4::gdk::ModifierType::SHIFT_MASK))
                .unwrap_or(false);
            let cmd = g.current_event()
                .map(|e| {
                    let m = e.modifier_state();
                    m.contains(gtk4::gdk::ModifierType::CONTROL_MASK) ||
                    m.contains(gtk4::gdk::ModifierType::META_MASK)
                }).unwrap_or(false);

            if cmd {
                if s.selected.contains(&row_idx) { s.selected.remove(&row_idx); }
                else { s.selected.insert(row_idx); }
                s.anchor = Some(row_idx);
            } else if shift {
                if let Some(anchor) = s.anchor {
                    let (lo, hi) = (anchor.min(row_idx), anchor.max(row_idx));
                    s.selected.clear();
                    for i in lo..=hi { s.selected.insert(i); }
                } else {
                    s.selected.clear(); s.selected.insert(row_idx); s.anchor = Some(row_idx);
                }
            } else {
                s.selected.clear(); s.selected.insert(row_idx); s.anchor = Some(row_idx);
            }

            let mbox = s.mbox_path.clone();
            let indices: Vec<i32> = s.selected.iter()
                .filter_map(|&i| s.rows.get(i).map(|r| r.index)).collect();
            drop(s);
            a.queue_draw();
            ha.queue_draw();
            if let Some(ref cb) = *osc.borrow() { cb(&mbox, &indices); }
        });
        area.add_controller(click);
    }

    // Keyboard
    {
        let st = state.clone();
        let a = area.clone();
        let osc = on_selection_changed.clone();
        let key = gtk4::EventControllerKey::new();
        key.connect_key_pressed(move |_, kv, _, _| {
            let mut s = st.borrow_mut();
            let len = s.rows.len();
            if len == 0 { return glib::Propagation::Proceed; }
            let cur = s.selected.iter().next().copied().unwrap_or(0);
            let new = match kv {
                gtk4::gdk::Key::Up | gtk4::gdk::Key::KP_Up => cur.saturating_sub(1),
                gtk4::gdk::Key::Down | gtk4::gdk::Key::KP_Down => (cur + 1).min(len - 1),
                gtk4::gdk::Key::Home => 0,
                gtk4::gdk::Key::End => len - 1,
                _ => return glib::Propagation::Proceed,
            };
            s.selected.clear(); s.selected.insert(new); s.anchor = Some(new);
            let mbox = s.mbox_path.clone();
            let indices: Vec<i32> = s.selected.iter()
                .filter_map(|&i| s.rows.get(i).map(|r| r.index)).collect();
            drop(s);
            a.queue_draw();
            if let Some(ref cb) = *osc.borrow() { cb(&mbox, &indices); }
            glib::Propagation::Stop
        });
        area.add_controller(key);
    }
    area.set_focusable(true);

    // Custom drag using GestureDrag
    // Start drag when mouse moves > threshold on a selected row.
    {
        let st = state.clone();
        let a = area.clone();
        
        let drag = gtk4::GestureDrag::new();
        drag.set_button(1);
        
        let st1 = state.clone();
        let a1 = area.clone();
        drag.connect_drag_begin(move |g, start_x, start_y| {
            let mut s = st1.borrow_mut();
            let row_idx = (start_y / ROW_HEIGHT) as usize;
            
            // Only drag if starting on a selected row
            if s.selected.contains(&row_idx) {
                let indices: Vec<i32> = s.selected.iter()
                    .filter_map(|&i| s.rows.get(i).map(|r| r.index)).collect();
                if !indices.is_empty() {
                    crate::drag_state::start_drag(&s.mbox_path, indices.clone());
                    a1.set_cursor(gtk4::gdk::Cursor::from_name("grabbing", None).as_ref());
                    
                    // Visual feedback setup
                    let popover = gtk4::Popover::builder()
                        .autohide(false)
                        .has_arrow(false)
                        .can_focus(false)
                        .halign(gtk4::Align::Start)
                        .valign(gtk4::Align::Start)
                        .build();
                        
                    let label_text = if indices.len() == 1 {
                        let subj = s.rows.get(row_idx).map(|r| r.subject.clone()).unwrap_or_default();
                        let max_len = 30;
                        let subj = if subj.len() > max_len { format!("{}...", &subj[..max_len]) } else { subj };
                        format!("✉ {}", subj)
                    } else {
                        format!("✉ {} messages", indices.len())
                    };
                    
                    let label = gtk4::Label::new(Some(&label_text));
                    label.set_margin_top(6);
                    label.set_margin_bottom(6);
                    label.set_margin_start(10);
                    label.set_margin_end(10);
                    label.add_css_class("suggested-action");
                    
                    popover.set_child(Some(&label));
                    popover.set_parent(&a1);
                    popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(start_x as i32, start_y as i32, 1, 1)));
                    popover.popup();
                    
                    s.drag_popover = Some(popover);
                    
                    g.set_state(gtk4::EventSequenceState::Claimed);
                    return;
                }
            }
            g.set_state(gtk4::EventSequenceState::Denied);
        });
        
        let st2 = state.clone();
        let a2 = area.clone();
        drag.connect_drag_update(move |g, dx, dy| {
            if crate::drag_state::is_dragging() {
                if let Some((sx, sy)) = g.start_point() {
                    let cx = sx + dx;
                    let cy = sy + dy;
                    
                    if let Some(pop) = &st2.borrow().drag_popover {
                        // Offset slightly so cursor isn't right on top of it, making drop hard to see
                        pop.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(cx as i32 + 10, cy as i32 + 10, 1, 1)));
                    }
                    
                    if let Some(root) = a2.root() {
                        if let Ok(root_widget) = root.dynamic_cast::<gtk4::Widget>() {
                            if let Some((rx, ry)) = a2.translate_coordinates(&root_widget, cx, cy) {
                                crate::drag_state::update_drag(&root_widget, rx, ry);
                            }
                        }
                    }
                }
            }
        });
        
        let st3 = state.clone();
        let a3 = area.clone();
        drag.connect_drag_end(move |_g, _dx, _dy| {
            if crate::drag_state::is_dragging() {
                a3.set_cursor(None);
                
                let mut s = st3.borrow_mut();
                if let Some(pop) = s.drag_popover.take() {
                    pop.popdown();
                    pop.unparent();
                }
                drop(s);
                
                if !crate::drag_state::execute_drop() {
                    crate::drag_state::cancel_drag();
                }
            }
        });
        
        area.add_controller(drag);
    }

    // Right-click — select row + fire callback (no preview update)
    {
        let st = state.clone();
        let a = area.clone();
        let orc = on_right_click.clone();
        let rclick = gtk4::GestureClick::new();
        rclick.set_button(3);
        rclick.connect_released(move |_, _, x, y| {
            let row_idx = (y / ROW_HEIGHT) as usize;
            let mut s = st.borrow_mut();
            if row_idx < s.rows.len() {
                if !s.selected.contains(&row_idx) {
                    s.selected.clear(); s.selected.insert(row_idx); s.anchor = Some(row_idx);
                }
                drop(s);
                a.queue_draw();
                // Fire right-click callback
                if let Some(ref cb) = *orc.borrow() { cb(x as i32, y as i32); }
            }
        });
        area.add_controller(rclick);
    }

    let scroll = gtk4::ScrolledWindow::builder()
        .child(&area)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .build();

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vbox.append(&header_area);
    vbox.append(&scroll);

    MailListWidget {
        widget: vbox,
        state,
        area,
        on_selection_changed,
        on_activate,
        on_right_click,
        on_mark_read: Rc::new(RefCell::new(None)),
    }
}

// ─── Sort ────────────────────────────────────────────────────────────────────

fn sort_rows(rows: &mut [MsgRow], col: SortCol, dir: SortDir) {
    rows.sort_by(|a, b| {
        let cmp = match col {
            SortCol::Status => a.unread.cmp(&b.unread).reverse(),
            SortCol::Priority => a.priority.cmp(&b.priority),
            SortCol::From => a.from.to_lowercase().cmp(&b.from.to_lowercase()),
            SortCol::Subject => a.subject.to_lowercase().cmp(&b.subject.to_lowercase()),
            SortCol::Date => a.index.cmp(&b.index), // TOC order ≈ date order
            SortCol::Size => {
                let sa = parse_size(&a.size);
                let sb = parse_size(&b.size);
                sa.cmp(&sb)
            }
        };
        if dir == SortDir::Desc { cmp.reverse() } else { cmp }
    });
}

fn parse_size(s: &str) -> u64 {
    let s = s.trim();
    if s.ends_with('K') { s[..s.len()-1].parse::<u64>().unwrap_or(0) * 1024 }
    else if s.ends_with('M') { s[..s.len()-1].parse::<u64>().unwrap_or(0) * 1024 * 1024 }
    else { s.parse().unwrap_or(0) }
}

// ─── Cairo rendering — header ────────────────────────────────────────────────

fn draw_header(cr: &cairo::Context, width: f64, state: &State) {
    let ct = current_theme(&state.theme);
    let t = &ct;
    let cl = col_layout_from_state(width, state);

    // Header background
    cr.set_source_rgba(t.header_bg.0, t.header_bg.1, t.header_bg.2, t.header_bg.3);
    cr.rectangle(0.0, 0.0, width, HEADER_HEIGHT);
    let _ = cr.fill();

    let ctx = pangocairo::functions::create_context(cr);
    let layout = pango::Layout::new(&ctx);
    let mut fd = pango::FontDescription::new();
    fd.set_family("sans-serif");
    fd.set_size((HEADER_FONT * pango::SCALE as f64) as i32);
    fd.set_weight(pango::Weight::Semibold);
    layout.set_font_description(Some(&fd));

    let headers = [
        (cl.status_w, "•", cl.status_x + 4.0, SortCol::Status),
        (cl.pri_w, "!", cl.pri_x + 4.0, SortCol::Priority),
        (cl.attach_w, "📎", cl.attach_x + 4.0, SortCol::Status), // fallback sort
        (cl.label_w, "Lbl", cl.label_x + 4.0, SortCol::Status), // fallback
        (cl.from_w, "From", cl.from_x + 6.0, SortCol::From),
        (cl.subject_w, "Subject", cl.subject_x + 6.0, SortCol::Subject),
        (cl.date_w, "Date", cl.date_x + 6.0, SortCol::Date),
        (cl.size_w, "Size", cl.size_x + 6.0, SortCol::Size),
        (cl.server_w, "Srv", cl.server_x + 6.0, SortCol::Status), // fallback
        (cl.mood_w, "Mood", cl.mood_x + 6.0, SortCol::Status), // fallback
        (cl.junk_w, "Junk", cl.junk_x + 6.0, SortCol::Status), // fallback
    ];

    for &(w, label, x, col) in headers.iter() {
        if w <= 0.0 { continue; }
        let is_sorted = state.sort_col == col;
        if is_sorted {
            cr.set_source_rgb(t.accent.0, t.accent.1, t.accent.2);
        } else {
            cr.set_source_rgb(t.text_secondary.0, t.text_secondary.1, t.text_secondary.2);
        }
        let display = if is_sorted {
            let arrow = if state.sort_dir == SortDir::Asc { " ▲" } else { " ▼" };
            format!("{}{}", label, arrow)
        } else {
            label.to_string()
        };
        layout.set_text(&display);
        cr.move_to(x, (HEADER_HEIGHT - HEADER_FONT) / 2.0 - 1.0);
        pangocairo::functions::show_layout(cr, &layout);
    }

    // Bottom border
    cr.set_source_rgba(t.separator.0, t.separator.1, t.separator.2, t.separator.3);
    cr.rectangle(0.0, HEADER_HEIGHT - 1.0, width, 1.0);
    let _ = cr.fill();
}

// ─── Cairo rendering — rows ─────────────────────────────────────────────────

fn draw_rows(cr: &cairo::Context, width: f64, state: &State) {
    let ct = current_theme(&state.theme);
    let t = &ct;
    let cl = col_layout_from_state(width, state);

    // Background
    cr.set_source_rgb(t.bg.0, t.bg.1, t.bg.2);
    cr.rectangle(0.0, 0.0, width, state.rows.len() as f64 * ROW_HEIGHT + 10.0);
    let _ = cr.fill();

    let ctx = pangocairo::functions::create_context(cr);
    let layout = pango::Layout::new(&ctx);
    let mut fd = pango::FontDescription::new();
    fd.set_family("sans-serif");

    for (i, row) in state.rows.iter().enumerate() {
        let y = i as f64 * ROW_HEIGHT;
        let text_y = y + (ROW_HEIGHT - FONT_SIZE) / 2.0 - 1.0;

        // Hover
        if state.hovered == Some(i) && !state.selected.contains(&i) {
            cr.set_source_rgba(t.hover.0, t.hover.1, t.hover.2, t.hover.3);
            cr.rectangle(0.0, y, width, ROW_HEIGHT);
            let _ = cr.fill();
        }

        // Selection
        if state.selected.contains(&i) {
            cr.set_source_rgba(t.sel_bg.0, t.sel_bg.1, t.sel_bg.2, t.sel_bg.3);
            cr.rectangle(0.0, y, width, ROW_HEIGHT);
            let _ = cr.fill();
            cr.set_source_rgb(t.accent.0, t.accent.1, t.accent.2);
            cr.rectangle(0.0, y + 3.0, 3.0, ROW_HEIGHT - 6.0);
            let _ = cr.fill();
        }

        let text_color = if !row.label_color.is_empty() {
            parse_hex_color(&row.label_color)
        } else { t.text };

        fd.set_size((FONT_SIZE * pango::SCALE as f64) as i32);

        // Status: unread dot
        if row.unread && cl.status_w > 0.0 {
            cr.set_source_rgb(t.unread_dot.0, t.unread_dot.1, t.unread_dot.2);
            cr.arc(cl.status_x + cl.status_w / 2.0, y + ROW_HEIGHT / 2.0,
                   UNREAD_DOT_R, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.fill();
        }

        // Priority
        let pri_str = match row.priority {
            1 => "↑↑", 2 => "↑", 4 => "↓", 5 => "↓↓", _ => "",
        };
        if !pri_str.is_empty() && cl.pri_w > 0.0 {
            let pri_color = match row.priority {
                1 => (0.90, 0.24, 0.24), 2 => (0.87, 0.42, 0.13),
                4 => (0.19, 0.51, 0.79), 5 => (0.18, 0.52, 0.36),
                _ => t.text,
            };
            cr.set_source_rgb(pri_color.0, pri_color.1, pri_color.2);
            fd.set_weight(pango::Weight::Bold);
            layout.set_font_description(Some(&fd));
            layout.set_text(pri_str);
            cr.move_to(cl.pri_x + 2.0, text_y);
            pangocairo::functions::show_layout(cr, &layout);
        }

        // From
        if cl.from_w > 0.0 {
            fd.set_weight(if row.unread { pango::Weight::Bold } else { pango::Weight::Normal });
            layout.set_font_description(Some(&fd));
            layout.set_text(&row.from);
            layout.set_width((cl.from_w as i32 - 12) * pango::SCALE);
            layout.set_ellipsize(pango::EllipsizeMode::End);
            cr.set_source_rgb(text_color.0, text_color.1, text_color.2);
            cr.move_to(cl.from_x + 6.0, text_y);
            pangocairo::functions::show_layout(cr, &layout);
        }
        layout.set_width(-1); layout.set_ellipsize(pango::EllipsizeMode::None);

        // Subject
        fd.set_weight(pango::Weight::Normal);
        layout.set_font_description(Some(&fd));
        layout.set_text(&row.subject);
        layout.set_width((cl.subject_w as i32 - 12) * pango::SCALE);
        layout.set_ellipsize(pango::EllipsizeMode::End);
        cr.set_source_rgb(t.text_secondary.0, t.text_secondary.1, t.text_secondary.2);
        cr.move_to(cl.subject_x + 6.0, text_y);
        pangocairo::functions::show_layout(cr, &layout);
        layout.set_width(-1); layout.set_ellipsize(pango::EllipsizeMode::None);

        // Date
        layout.set_text(&row.date);
        cr.set_source_rgb(t.text_secondary.0, t.text_secondary.1, t.text_secondary.2);
        cr.move_to(cl.date_x + 6.0, text_y);
        pangocairo::functions::show_layout(cr, &layout);

        // Size (right-aligned)
        layout.set_text(&row.size);
        let (sw, _) = layout.pixel_size();
        cr.move_to(cl.size_x + COL_SIZE_W - sw as f64 - 8.0, text_y);
        pangocairo::functions::show_layout(cr, &layout);

        // Horizontal row separator
        if state.col_config.draw_horiz_lines {
            cr.set_source_rgba(t.separator.0, t.separator.1, t.separator.2, t.separator.3);
            cr.rectangle(0.0, y + ROW_HEIGHT - SEP_HEIGHT, width, SEP_HEIGHT);
            let _ = cr.fill();
        }
    }

    // Vertical column separator lines
    if state.col_config.draw_vert_lines {
        cr.set_source_rgba(t.separator.0, t.separator.1, t.separator.2, t.separator.3 * 1.5);
        let total_h = state.rows.len() as f64 * ROW_HEIGHT;
        for &x in &[
            cl.from_x, cl.subject_x, cl.date_x, cl.size_x,
        ] {
            if x > 0.0 {
                cr.rectangle(x - 0.5, 0.0, 1.0, total_h);
                let _ = cr.fill();
            }
        }
    }
}

fn parse_hex_color(hex: &str) -> (f64, f64, f64) {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 { return (0.0, 0.0, 0.0); }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as f64 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as f64 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as f64 / 255.0;
    (r, g, b)
}
