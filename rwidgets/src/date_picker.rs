//! DatePicker — a dropdown button that displays "Mon, Mar 25, 2026" and
//! opens a popover containing a GtkCalendar for visual date selection.

use gtk4::prelude::*;
use gtk4::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub struct DatePicker {
    pub widget: gtk4::MenuButton,
    date: Rc<RefCell<glib::DateTime>>,
}

impl DatePicker {
    pub fn new(initial: &glib::DateTime) -> Self {
        let date = Rc::new(RefCell::new(initial.clone()));

        let btn = gtk4::MenuButton::new();
        btn.set_label(&format_date(initial));
        btn.set_always_show_arrow(true);

        let popover = gtk4::Popover::new();
        popover.set_has_arrow(true);

        let calendar = gtk4::Calendar::new();
        calendar.select_day(initial);
        popover.set_child(Some(&calendar));

        // Calendar day selected → update button label + close
        {
            let b = btn.clone();
            let d = date.clone();
            let pop = popover.clone();
            calendar.connect_day_selected(move |cal| {
                let dt = cal.date();
                b.set_label(&format_date(&dt));
                *d.borrow_mut() = dt;
                pop.popdown();
            });
        }

        // On popover show, sync calendar to current date
        {
            let d = date.clone();
            let cal = calendar.clone();
            popover.connect_show(move |_| {
                cal.select_day(&d.borrow());
            });
        }

        btn.set_popover(Some(&popover));

        Self { widget: btn, date }
    }

    pub fn get_date(&self) -> glib::DateTime {
        self.date.borrow().clone()
    }

    pub fn set_date(&self, dt: &glib::DateTime) {
        *self.date.borrow_mut() = dt.clone();
        self.widget.set_label(&format_date(dt));
    }

    pub fn set_sensitive(&self, sensitive: bool) {
        self.widget.set_sensitive(sensitive);
    }
}

fn format_date(dt: &glib::DateTime) -> String {
    // "Wed, Mar 25, 2026"
    dt.format("%a, %b %e, %Y")
        .map(|s| s.to_string())
        .unwrap_or_else(|_| format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day_of_month()))
}
