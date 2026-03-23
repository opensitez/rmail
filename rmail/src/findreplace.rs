//! Find/Replace dialog for the reditctrl editor.
//!
//! Port of find.c. Shows a popover or bar with Find/Replace entries,
//! Next/Previous/Replace/Replace All buttons.

use gtk4::prelude::*;
use gtk4::glib;
use reditctrl::gtk_widget::EditorWidget;
use std::rc::Rc;

/// Show a Find/Replace bar attached to an editor widget.
/// Returns the bar widget to embed in the compose layout.
pub fn build_find_bar(editor: &Rc<EditorWidget>) -> gtk4::Widget {
    let bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    bar.add_css_class("toolbar");
    bar.set_margin_start(6);
    bar.set_margin_end(6);
    bar.set_margin_top(2);
    bar.set_margin_bottom(2);

    // Find entry
    let find_entry = gtk4::Entry::new();
    find_entry.set_placeholder_text(Some("Find…"));
    find_entry.set_hexpand(true);
    find_entry.set_width_chars(20);
    bar.append(&find_entry);

    // Next / Previous
    let next_btn = gtk4::Button::from_icon_name("go-down-symbolic");
    next_btn.set_tooltip_text(Some("Find Next"));
    let prev_btn = gtk4::Button::from_icon_name("go-up-symbolic");
    prev_btn.set_tooltip_text(Some("Find Previous"));
    bar.append(&prev_btn);
    bar.append(&next_btn);

    bar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

    // Replace entry
    let replace_entry = gtk4::Entry::new();
    replace_entry.set_placeholder_text(Some("Replace…"));
    replace_entry.set_width_chars(20);
    bar.append(&replace_entry);

    // Replace / Replace All
    let replace_btn = gtk4::Button::with_label("Replace");
    let replace_all_btn = gtk4::Button::with_label("All");
    replace_all_btn.set_tooltip_text(Some("Replace All"));
    bar.append(&replace_btn);
    bar.append(&replace_all_btn);

    // Case sensitive toggle
    let case_btn = gtk4::ToggleButton::with_label("Aa");
    case_btn.set_tooltip_text(Some("Case Sensitive"));
    case_btn.set_can_focus(false);
    bar.append(&case_btn);

    // Status label
    let status = gtk4::Label::new(None);
    status.add_css_class("dim-label");
    bar.append(&status);

    // ── Wire Find Next ──
    {
        let e = editor.clone();
        let fe = find_entry.clone();
        let cb = case_btn.clone();
        let sl = status.clone();
        let find_next = move || {
            let text = fe.text().to_string();
            if text.is_empty() { sl.set_text(""); return; }
            let found = e.editor().find_text(&text, cb.is_active());
            if found >= 0 {
                sl.set_text(&format!("Found at {}", found));
                e.queue_draw();
            } else {
                sl.set_text("Not found");
            }
        };

        let fn_clone = find_next.clone();
        next_btn.connect_clicked(move |_| fn_clone());

        let fn_clone2 = find_next.clone();
        find_entry.connect_activate(move |_| fn_clone2());
    }

    // ── Wire Find Previous (search backwards by moving caret to before selection) ──
    {
        let e = editor.clone();
        let fe = find_entry.clone();
        let cb = case_btn.clone();
        let sl = status.clone();
        prev_btn.connect_clicked(move |_| {
            let text = fe.text().to_string();
            if text.is_empty() { return; }
            // Move caret backwards before searching
            let (sel_start, _) = e.editor().get_selection_bounds();
            if sel_start > 0 {
                e.editor().select_range(sel_start - 1, sel_start - 1);
            }
            let found = e.editor().find_text(&text, cb.is_active());
            if found >= 0 {
                sl.set_text(&format!("Found at {}", found));
            } else {
                sl.set_text("Not found");
            }
            e.queue_draw();
        });
    }

    // ── Wire Replace ──
    {
        let e = editor.clone();
        let fe = find_entry.clone();
        let re = replace_entry.clone();
        let cb = case_btn.clone();
        let sl = status.clone();
        replace_btn.connect_clicked(move |_| {
            let search = fe.text().to_string();
            let replace = re.text().to_string();
            if search.is_empty() { return; }
            let count = e.editor().replace_text(&search, &replace, false, cb.is_active());
            sl.set_text(if count > 0 { "Replaced" } else { "Not found" });
            e.queue_draw();
        });
    }

    // ── Wire Replace All ──
    {
        let e = editor.clone();
        let fe = find_entry.clone();
        let re = replace_entry.clone();
        let cb = case_btn.clone();
        let sl = status.clone();
        replace_all_btn.connect_clicked(move |_| {
            let search = fe.text().to_string();
            let replace = re.text().to_string();
            if search.is_empty() { return; }
            let count = e.editor().replace_text(&search, &replace, true, cb.is_active());
            sl.set_text(&format!("{} replaced", count));
            e.queue_draw();
        });
    }

    bar.upcast()
}
