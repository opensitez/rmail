//! GTK4 widget assembly for rEditCtrl.
//!
//! Port of gEditCtrl/geditctrl-glue.c `geditctrl_new()`.
//! Creates a GtkScrolledWindow containing a GtkDrawingArea with all
//! signal handlers wired up: draw, click, motion, key, scroll.

use gtk4::prelude::*;
use gtk4::glib;
use gdk4::Key;
use std::cell::RefCell;
use std::rc::Rc;

use crate::glue::EditorCtrl;
use crate::input;
use crate::render::{self, DrawParams, MeasureParams, Viewport};

use crate::render::ThemeColors;
use crate::render::Color;

/// A complete GTK4 rich text editor widget.
/// Port of C `geditctrl_new()` — a ScrolledWindow + DrawingArea.
pub struct EditorWidget {
    pub scrolled: gtk4::ScrolledWindow,
    pub ctrl: Rc<EditorCtrl>,
    area: gtk4::DrawingArea,
    theme: Rc<RefCell<Option<ThemeColors>>>,
    /// Incremented on every document change — used for spell check debounce.
    change_counter: Rc<std::cell::Cell<u64>>,
}

/// Get the vadjustment value from a scrolled window.
fn get_scroll_y(scrolled: &gtk4::ScrolledWindow) -> f64 {
    scrolled.vadjustment().value()
}

impl EditorWidget {
    /// Create a new editor widget with a fresh empty document.
    pub fn new() -> Self {
        Self::with_ctrl(Rc::new(EditorCtrl::new_default()))
    }

    /// Create an editor widget wrapping an existing EditorCtrl.
    pub fn with_ctrl(ctrl: Rc<EditorCtrl>) -> Self {
        let theme: Rc<RefCell<Option<ThemeColors>>> = Rc::new(RefCell::new(None));
        let area = gtk4::DrawingArea::new();
        area.add_css_class("gedit-view");
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_can_focus(true);
        area.set_focusable(true);
        area.set_size_request(200, 100);

        let scrolled = gtk4::ScrolledWindow::new();
        scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
        scrolled.set_propagate_natural_height(false);
        scrolled.set_propagate_natural_width(false);
        scrolled.set_can_focus(true);
        scrolled.set_focusable(true);

        // Keep a reference to the scrolled window in the area via weak ref
        // for scroll offset access from draw/input handlers.
        let sw_weak = scrolled.downgrade();

        // ── Draw function ──
        {
            let c = ctrl.clone();
            let sw = sw_weak.clone();
            let th = theme.clone();
            area.set_draw_func(move |area, cr, width, height| {
                let pango_ctx = area.pango_context();
                let s = c.state.borrow();
                let sy = sw.upgrade().map(|sw| get_scroll_y(&sw)).unwrap_or(0.0);
                let viewport = Viewport::new(sy, sy + height as f64);
                let theme_ref = th.borrow();
                let theme_opt = theme_ref.as_ref();

                let params = DrawParams {
                    cr,
                    pango_ctx: &pango_ctx,
                    doc: &c.doc,
                    state: Some(&s),
                    width,
                    height,
                    viewport,
                    theme: theme_opt,
                };
                render::draw(&params);

                // Update content height for scrollbar
                let mparams = MeasureParams {
                    pango_ctx: &pango_ctx,
                    doc: &c.doc,
                    width,
                };
                let ch = render::content_height(&mparams);
                area.set_content_height(ch.max(height));
            });
        }

        // ── Document changed → redraw + bump change counter ──
        let change_counter = Rc::new(std::cell::Cell::new(0u64));
        {
            let a = area.clone();
            let cc = change_counter.clone();
            ctrl.doc.connect_changed(move || {
                cc.set(cc.get() + 1);
                a.queue_draw();
            });
        }

        // Helper: create a layout function for hit-testing
        fn make_layout_fn(pctx: &pango::Context) -> impl Fn(&str, i32) -> pango::Layout + '_ {
            move |text: &str, avail: i32| {
                let layout = pango::Layout::new(pctx);
                layout.set_text(text);
                layout.set_width(avail * pango::SCALE);
                layout.set_wrap(pango::WrapMode::WordChar);
                layout
            }
        }

        // ── Click gesture ──
        {
            let gesture = gtk4::GestureClick::new();
            gesture.set_button(gdk4::BUTTON_PRIMARY);

            let c = ctrl.clone();
            let a = area.clone();
            let sw = sw_weak.clone();
            gesture.connect_pressed(move |_g, n_press, x, y| {
                a.grab_focus();
                let sy = sw.upgrade().map(|s| get_scroll_y(&s)).unwrap_or(0.0);
                let mut s = c.state.borrow_mut();
                let w = a.width();
                let pctx = a.pango_context();
                let lf = make_layout_fn(&pctx);
                input::on_pressed(&mut s, &c.doc, w, n_press as i32, x, y + sy, &lf);
                drop(s);
                a.queue_draw();
            });

            let c2 = ctrl.clone();
            let a2 = area.clone();
            let sw2 = sw_weak.clone();
            gesture.connect_released(move |_g, _n, x, y| {
                let sy = sw2.upgrade().map(|s| get_scroll_y(&s)).unwrap_or(0.0);
                let mut s = c2.state.borrow_mut();
                input::on_released(&mut s, &c2.doc, x, y + sy);
                drop(s);
                a2.queue_draw();
            });

            area.add_controller(gesture);
        }

        // ── Motion (drag selection / image resize) ──
        {
            let motion = gtk4::EventControllerMotion::new();
            let c = ctrl.clone();
            let a = area.clone();
            let sw = sw_weak.clone();
            motion.connect_motion(move |_ctrl, x, y| {
                let sy = sw.upgrade().map(|s| get_scroll_y(&s)).unwrap_or(0.0);
                let mut s = c.state.borrow_mut();
                let w = a.width();
                let pctx = a.pango_context();
                let lf = make_layout_fn(&pctx);
                let changed = input::on_motion(&mut s, &c.doc, w, x, y + sy, &lf);
                drop(s);
                if changed {
                    a.queue_draw();
                }
            });
            area.add_controller(motion);
        }

        // ── Key press on the drawing area ──
        {
            let c = ctrl.clone();
            let a = area.clone();
            let k = gtk4::EventControllerKey::new();
            k.connect_key_pressed(move |_k, keyval, _keycode, mods| {
                let mut s = c.state.borrow_mut();
                if input::on_key_pressed(&mut s, &c.doc, keyval, mods) {
                    drop(s);
                    a.queue_draw();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
            area.add_controller(k);
        }

        // ── Key press on scrolled window (backup — matching C glue.c:243-246) ──
        {
            let c = ctrl.clone();
            let a = area.clone();
            let k = gtk4::EventControllerKey::new();
            k.connect_key_pressed(move |_k, keyval, _keycode, mods| {
                let mut s = c.state.borrow_mut();
                if input::on_key_pressed(&mut s, &c.doc, keyval, mods) {
                    drop(s);
                    a.queue_draw();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
            scrolled.add_controller(k);
        }

        scrolled.set_child(Some(&area));

        // ── Caret blink timer (500ms) ──
        {
            let st = ctrl.state.clone();
            let a = area.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                if !a.is_realized() {
                    return glib::ControlFlow::Break;
                }
                let mut s = st.borrow_mut();
                s.caret_visible = !s.caret_visible;
                drop(s);
                a.queue_draw();
                glib::ControlFlow::Continue
            });
        }

        EditorWidget { scrolled, ctrl, area, theme, change_counter }
    }

    /// Get the GtkWidget (ScrolledWindow) to embed in the UI.
    pub fn widget(&self) -> &gtk4::ScrolledWindow {
        &self.scrolled
    }

    /// Access the underlying EditorCtrl for toggle_style, set_color, etc.
    pub fn editor(&self) -> &EditorCtrl {
        &self.ctrl
    }

    /// Force a redraw of the editor content.
    pub fn queue_draw(&self) {
        self.area.queue_draw();
    }

    /// Set theme colors for the editor (bg, text, caret, selection).
    /// Port of theme.c theme_apply_to_editor / geditctrl_set_theme_colors.
    pub fn set_theme_colors(&self, bg: Color, text: Color, caret: Color, sel_bg: Color) {
        *self.theme.borrow_mut() = Some(ThemeColors { bg, text, caret, sel_bg });
        self.area.queue_draw();
    }

    /// Clear theme colors (revert to default white bg / black text).
    pub fn clear_theme(&self) {
        *self.theme.borrow_mut() = None;
        self.area.queue_draw();
    }

    /// Set spell error marks on the document (wavy red underlines).
    /// Each entry is (char_offset, char_length).
    /// The caller is responsible for running the spell checker and providing
    /// the error ranges — reditctrl doesn't depend on any spell library.
    pub fn set_spell_errors(&self, errors: Vec<(i32, i32)>) {
        self.ctrl.doc.set_spell_errors(errors);
    }

    /// Clear spell error marks.
    pub fn clear_spell_errors(&self) {
        self.ctrl.doc.clear_spell_errors();
    }

    /// Enable automatic spell checking with a callback.
    /// `scan_fn` takes text and returns error ranges: Vec<(char_offset, char_length)>.
    /// Runs debounced 800ms after the last text change.
    /// Enable automatic spell checking with a callback.
    /// `scan_fn` takes text and returns error ranges: Vec<(char_offset, char_length)>.
    /// Runs on a background thread, debounced 1s after the last text change.
    /// Never blocks typing — results are posted back to the main thread.
    pub fn enable_auto_spell<F>(&self, scan_fn: F)
    where
        F: Fn(&str) -> Vec<(i32, i32)> + Send + Sync + 'static,
    {
        let ctrl = self.ctrl.clone();
        let area = self.area.clone();
        let cc = self.change_counter.clone();
        let last_dispatched = Rc::new(std::cell::Cell::new(0u64));
        let scan_arc = std::sync::Arc::new(scan_fn);

        // Pending results from background thread
        let pending: std::sync::Arc<std::sync::Mutex<Option<Vec<(i32, i32)>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            if !area.is_realized() {
                return glib::ControlFlow::Break;
            }

            // Pick up results from background thread
            if let Ok(mut lock) = pending.try_lock() {
                if let Some(errors) = lock.take() {
                    ctrl.doc.set_spell_errors(errors);
                    in_flight.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }

            // Dispatch new scan if text changed and no scan in flight
            let current = cc.get();
            if current != last_dispatched.get() && current > 0
                && !in_flight.load(std::sync::atomic::Ordering::Relaxed)
            {
                last_dispatched.set(current);
                let text = ctrl.get_text();
                if text.is_empty() {
                    ctrl.doc.clear_spell_errors();
                } else {
                    in_flight.store(true, std::sync::atomic::Ordering::Relaxed);
                    let scan = scan_arc.clone();
                    let pw = pending.clone();
                    std::thread::spawn(move || {
                        let errors = scan(&text);
                        *pw.lock().unwrap() = Some(errors);
                    });
                }
            }

            glib::ControlFlow::Continue
        });
    }
}
