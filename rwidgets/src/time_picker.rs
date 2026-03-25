//! TimePicker — a dropdown button that displays the time (AM/PM or 24h
//! depending on locale) and opens a scrollable popover of 30-minute slots.

use gtk4::prelude::*;
use gtk4::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub struct TimePicker {
    pub widget: gtk4::MenuButton,
    hour: Rc<RefCell<i32>>,
    minute: Rc<RefCell<i32>>,
}

impl TimePicker {
    pub fn new(hour: i32, minute: i32) -> Self {
        let h = Rc::new(RefCell::new(hour));
        let m = Rc::new(RefCell::new(minute));

        let btn = gtk4::MenuButton::new();
        btn.set_label(&format_time(hour, minute));
        btn.set_always_show_arrow(true);

        let popover = gtk4::Popover::new();
        popover.set_has_arrow(true);

        let scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .max_content_height(250)
            .propagate_natural_height(true)
            .build();
        scroll.set_size_request(120, -1);

        let listbox = gtk4::ListBox::new();
        listbox.set_selection_mode(gtk4::SelectionMode::Single);

        for slot in 0..48 {
            let sh = slot / 2;
            let sm = (slot % 2) * 30;
            let label = gtk4::Label::new(Some(&format_time(sh, sm)));
            label.set_halign(gtk4::Align::Start);
            label.set_margin_start(8);
            label.set_margin_end(8);
            label.set_margin_top(4);
            label.set_margin_bottom(4);
            listbox.append(&label);
        }

        {
            let b = btn.clone();
            let hr = h.clone();
            let mn = m.clone();
            let pop = popover.clone();
            listbox.connect_row_activated(move |_, row| {
                let idx = row.index() as i32;
                let sh = idx / 2;
                let sm = (idx % 2) * 30;
                *hr.borrow_mut() = sh;
                *mn.borrow_mut() = sm;
                b.set_label(&format_time(sh, sm));
                pop.popdown();
            });
        }

        scroll.set_child(Some(&listbox));
        popover.set_child(Some(&scroll));
        btn.set_popover(Some(&popover));

        {
            let hr = h.clone();
            let mn = m.clone();
            let lb = listbox.clone();
            popover.connect_show(move |_| {
                let cur_h = *hr.borrow();
                let cur_m = *mn.borrow();
                let idx = (cur_h * 2 + if cur_m >= 30 { 1 } else { 0 }).min(47);
                if let Some(row) = lb.row_at_index(idx) {
                    lb.select_row(Some(&row));
                    let r = row.clone();
                    glib::idle_add_local_once(move || { r.grab_focus(); });
                }
            });
        }

        Self { widget: btn, hour: h, minute: m }
    }

    pub fn get_hour(&self) -> i32 { *self.hour.borrow() }
    pub fn get_minute(&self) -> i32 { *self.minute.borrow() }

    pub fn set_time(&self, hour: i32, minute: i32) {
        *self.hour.borrow_mut() = hour;
        *self.minute.borrow_mut() = minute;
        self.widget.set_label(&format_time(hour, minute));
    }

    pub fn set_sensitive(&self, sensitive: bool) {
        self.widget.set_sensitive(sensitive);
    }
}

/// Format time using the system locale. Uses glib::DateTime which respects
/// the locale's 12h/24h preference via %X (locale time) or a manual check.
fn format_time(h: i32, m: i32) -> String {
    // Use glib to format — it respects LC_TIME locale settings
    if let Ok(dt) = glib::DateTime::from_local(2026, 1, 1, h, m, 0.0) {
        // %p gives AM/PM, %I gives 12h hour — use %l:%M %p for 12h, %H:%M for 24h
        // Detect: if locale's %p is non-empty, use 12h format
        let am_pm = dt.format("%p").map(|s| s.to_string()).unwrap_or_default();
        if !am_pm.is_empty() {
            // 12-hour format
            return dt.format("%l:%M %p")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| format!("{:02}:{:02}", h, m));
        }
    }
    // 24-hour fallback
    format!("{:02}:{:02}", h, m)
}
