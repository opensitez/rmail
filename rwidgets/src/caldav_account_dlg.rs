//! CalDAV Account Setup Dialog.
//!
//! Simple dialog for adding/editing a CalDAV calendar account:
//! server URL, username, password, display name, color.

use gtk4::prelude::*;
use gtk4::glib;
use macmbx::calendar::{CalendarInfo, CalSourceType};
use std::cell::RefCell;
use std::rc::Rc;

pub enum AccountDialogResult {
    Ok(CalendarInfo),
    Cancelled,
    Delete(String), // calendar id
}

/// Show the CalDAV account dialog.
pub fn show_caldav_account_dialog<F: Fn(AccountDialogResult) + 'static>(
    parent: &impl IsA<gtk4::Widget>,
    existing: Option<&CalendarInfo>,
    on_done: F,
) {
    let is_edit = existing.is_some();

    let dialog = gtk4::Window::builder()
        .title(if is_edit { "Edit CalDAV Account" } else { "Add CalDAV Account" })
        .modal(true)
        .default_width(440)
        .default_height(320)
        .resizable(false)
        .build();

    if let Some(root) = parent.root() {
        if let Some(win) = root.downcast_ref::<gtk4::Window>() {
            dialog.set_transient_for(Some(win));
        }
    }

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    vbox.set_margin_start(16); vbox.set_margin_end(16);
    vbox.set_margin_top(16); vbox.set_margin_bottom(16);

    let grid = gtk4::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(8);

    let mut r = 0;

    let lbl = |text: &str| -> gtk4::Label {
        gtk4::Label::builder().label(text).halign(gtk4::Align::End).build()
    };

    // Server URL
    grid.attach(&lbl("Server URL:"), 0, r, 1, 1);
    let url_entry = gtk4::Entry::new();
    url_entry.set_hexpand(true);
    url_entry.set_placeholder_text(Some("https://caldav.example.com/dav/"));
    if let Some(cal) = existing { url_entry.set_text(&cal.server_url); }
    grid.attach(&url_entry, 1, r, 1, 1);
    r += 1;

    // Username
    grid.attach(&lbl("Username:"), 0, r, 1, 1);
    let user_entry = gtk4::Entry::new();
    user_entry.set_hexpand(true);
    if let Some(cal) = existing { user_entry.set_text(&cal.username); }
    grid.attach(&user_entry, 1, r, 1, 1);
    r += 1;

    // Password
    grid.attach(&lbl("Password:"), 0, r, 1, 1);
    let pass_entry = gtk4::Entry::new();
    pass_entry.set_hexpand(true);
    pass_entry.set_visibility(false);
    pass_entry.set_input_purpose(gtk4::InputPurpose::Password);
    if let Some(cal) = existing { pass_entry.set_text(&cal.password); }
    grid.attach(&pass_entry, 1, r, 1, 1);
    r += 1;

    // Display name
    grid.attach(&lbl("Display name:"), 0, r, 1, 1);
    let name_entry = gtk4::Entry::new();
    name_entry.set_hexpand(true);
    name_entry.set_placeholder_text(Some("Work Calendar"));
    if let Some(cal) = existing { name_entry.set_text(&cal.name); }
    grid.attach(&name_entry, 1, r, 1, 1);
    r += 1;

    // Color
    grid.attach(&lbl("Color:"), 0, r, 1, 1);
    let color_btn = gtk4::ColorButton::new();
    let default_color = existing.map(|c| c.color.as_str()).unwrap_or("#4A90D9");
    if let Ok(rgba) = gtk4::gdk::RGBA::parse(default_color) {
        color_btn.set_rgba(&rgba);
    }
    grid.attach(&color_btn, 1, r, 1, 1);

    vbox.append(&grid);

    // Buttons
    let btn_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    btn_row.set_margin_top(8);

    let result: Rc<RefCell<Option<AccountDialogResult>>> = Rc::new(RefCell::new(None));

    if is_edit {
        let del_btn = gtk4::Button::with_label("Remove Account");
        del_btn.add_css_class("destructive-action");
        btn_row.append(&del_btn);
        let res = result.clone();
        let dlg = dialog.clone();
        let id = existing.unwrap().id.clone();
        del_btn.connect_clicked(move |_| {
            *res.borrow_mut() = Some(AccountDialogResult::Delete(id.clone()));
            dlg.close();
        });
    }

    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    btn_row.append(&spacer);

    let cancel_btn = gtk4::Button::with_label("Cancel");
    let ok_btn = gtk4::Button::with_label(if is_edit { "Save" } else { "Add" });
    ok_btn.add_css_class("suggested-action");
    btn_row.append(&cancel_btn);
    btn_row.append(&ok_btn);
    vbox.append(&btn_row);
    dialog.set_child(Some(&vbox));

    {
        let dlg = dialog.clone();
        cancel_btn.connect_clicked(move |_| dlg.close());
    }

    {
        let res = result.clone();
        let dlg = dialog.clone();
        let ue = url_entry.clone();
        let use_ = user_entry.clone();
        let pe = pass_entry.clone();
        let ne = name_entry.clone();
        let cb = color_btn.clone();
        let existing_id = existing.map(|c| c.id.clone());

        ok_btn.connect_clicked(move |_| {
            let url = ue.text().trim().to_string();
            if url.is_empty() {
                ue.add_css_class("error");
                return;
            }

            let rgba = cb.rgba();
            let color = format!("#{:02x}{:02x}{:02x}",
                (rgba.red() * 255.0) as u8,
                (rgba.green() * 255.0) as u8,
                (rgba.blue() * 255.0) as u8);

            let cal = CalendarInfo {
                id: existing_id.clone().unwrap_or_else(|| glib::uuid_string_random().to_string()),
                name: {
                    let n = ne.text().trim().to_string();
                    if n.is_empty() { "CalDAV Calendar".to_string() } else { n }
                },
                color,
                enabled: true,
                source: CalSourceType::CalDAV,
                server_url: url,
                username: use_.text().to_string(),
                password: pe.text().to_string(),
                sync_token: String::new(),
                last_sync_time: 0,
            };

            *res.borrow_mut() = Some(AccountDialogResult::Ok(cal));
            dlg.close();
        });
    }

    {
        let res = result.clone();
        let on_done = Rc::new(on_done);
        dialog.connect_close_request(move |_| {
            let r = res.borrow_mut().take().unwrap_or(AccountDialogResult::Cancelled);
            on_done(r);
            glib::Propagation::Proceed
        });
    }

    dialog.present();
    url_entry.grab_focus();
}
