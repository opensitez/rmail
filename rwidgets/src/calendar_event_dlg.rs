//! Calendar Event Dialog — create/edit a single calendar event.
//!
//! Port of CalendarEventDlg.cpp. Tabbed GTK4 dialog with:
//! - Title row (always visible above tabs)
//! - Tab 1: Details — DatePicker + TimePicker dropdowns, all-day,
//!          location, URL, show-as, private, reminder
//! - Tab 2: Repeat — frequency, interval, weekly days, end condition
//! - Tab 3: People — autocomplete entry, attendee chips with ✕ remove
//! - Tab 4: Notes  — multiline description

use gtk4::prelude::*;
use gtk4::{glib, gio};
use macmbx::calendar::CalendarEvent;
use macmbx::nicknames::AddressBooks;
use crate::date_picker::DatePicker;
use crate::time_picker::TimePicker;
use std::cell::RefCell;
use std::rc::Rc;

const STATUS_LABELS: &[&str] = &["Busy", "Free", "Tentative", "Away", "Out of Office"];
#[allow(dead_code)]
const STATUS_VALUES: &[&str] = &["busy", "free", "tentative", "away", "outofoffice"];
const REPEAT_LABELS: &[&str] = &["None", "Daily", "Weekly", "Monthly", "Yearly"];
#[allow(dead_code)]
const REPEAT_VALUES: &[&str] = &["", "DAILY", "WEEKLY", "MONTHLY", "YEARLY"];
const REMINDER_LABELS: &[&str] = &[
    "None", "At start time", "5 minutes before", "10 minutes before",
    "15 minutes before", "30 minutes before", "1 hour before",
    "2 hours before", "1 day before",
];
const LABEL_NAMES: &[&str] = &["None", "Label 1", "Label 2", "Label 3", "Label 4", "Label 5", "Label 6", "Label 7"];
const REMINDER_MINS_VALUES: &[i32] = &[0, 0, 5, 10, 15, 30, 60, 120, 1440];
const DAY_NAMES: &[&str] = &["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Result from the event dialog.
pub enum EventDialogResult {
    Ok(CalendarEvent),
    Cancelled,
    Delete(String),
}

/// Show the calendar event dialog.
pub fn show_event_dialog<F: Fn(EventDialogResult) + 'static>(
    parent: &impl IsA<gtk4::Widget>,
    default_date: &glib::DateTime,
    existing: Option<&CalendarEvent>,
    books: Option<Rc<AddressBooks>>,
    on_done: F,
) {
    let is_edit = existing.is_some();

    let dialog = gtk4::Window::builder()
        .title(if is_edit { "Edit Event" } else { "New Event" })
        .modal(true)
        .default_width(520)
        .default_height(480)
        .resizable(true)
        .build();

    if let Some(root) = parent.root() {
        if let Some(win) = root.downcast_ref::<gtk4::Window>() {
            dialog.set_transient_for(Some(win));
        }
    }

    // CSS
    let css = gtk4::CssProvider::new();
    css.load_from_data("
        .event-dlg { padding: 12px; }
        .event-dlg-title entry { font-size: 1.15em; font-weight: 500; }
        .tab-page { padding: 10px; }
        .attendee-chip { padding: 4px 8px; border-radius: 12px;
                         background: alpha(@theme_selected_bg_color, 0.15);
                         margin: 2px; }
        .attendee-chip label { margin-end: 4px; }
        .attendee-chip button { min-width: 16px; min-height: 16px;
                                padding: 0; margin: 0; }
    ");
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().unwrap(),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // ── Data defaults ──────────────────────────────────────────────────
    let uid = existing
        .map(|e| e.uid.clone())
        .unwrap_or_else(|| glib::uuid_string_random().to_string());

    let (start_full, end_full) = if let Some(ev) = existing {
        (
            glib::DateTime::from_unix_local(ev.dt_start_ts).unwrap(),
            glib::DateTime::from_unix_local(ev.dt_end_ts).unwrap(),
        )
    } else {
        let s = if default_date.hour() == 0 && default_date.minute() == 0 {
            glib::DateTime::from_local(
                default_date.year(), default_date.month(), default_date.day_of_month(),
                9, 0, 0.0,
            ).unwrap()
        } else {
            default_date.clone()
        };
        let e = s.add_hours(1).unwrap();
        (s, e)
    };

    // ── Main layout ────────────────────────────────────────────────────
    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    vbox.add_css_class("event-dlg");

    // Title row
    let title_grid = gtk4::Grid::new();
    title_grid.set_column_spacing(8);
    title_grid.set_hexpand(true);
    title_grid.add_css_class("event-dlg-title");
    title_grid.attach(&lbl_end("Title:"), 0, 0, 1, 1);
    let title_entry = gtk4::Entry::new();
    title_entry.set_hexpand(true);
    if let Some(ev) = existing { title_entry.set_text(&ev.summary); }
    title_grid.attach(&title_entry, 1, 0, 1, 1);
    vbox.append(&title_grid);

    let notebook = gtk4::Notebook::new();
    notebook.set_vexpand(true);

    // ── Wrap pickers in Rc so OK handler can read them ─────────────────
    let start_dp = Rc::new(DatePicker::new(&start_full));
    let start_tp = Rc::new(TimePicker::new(start_full.hour(), start_full.minute()));
    let end_dp = Rc::new(DatePicker::new(&end_full));
    let end_tp = Rc::new(TimePicker::new(end_full.hour(), end_full.minute()));

    // ================================================================
    //  TAB 1 — Details
    // ================================================================
    let det = gtk4::Grid::new();
    det.set_row_spacing(8);
    det.set_column_spacing(8);
    det.add_css_class("tab-page");
    det.set_hexpand(true);

    let mut r = 0;

    // Start date + time
    det.attach(&lbl_end("Start date:"), 0, r, 1, 1);
    let sr = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    start_dp.widget.set_hexpand(true);
    sr.append(&start_dp.widget);
    sr.append(&start_tp.widget);
    det.attach(&sr, 1, r, 1, 1);
    r += 1;

    // End date + time
    det.attach(&lbl_end("End date:"), 0, r, 1, 1);
    let er = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    end_dp.widget.set_hexpand(true);
    er.append(&end_dp.widget);
    er.append(&end_tp.widget);
    det.attach(&er, 1, r, 1, 1);
    r += 1;

    // All-day
    let all_day_check = gtk4::CheckButton::with_label("All day event");
    if let Some(ev) = existing { all_day_check.set_active(ev.all_day); }
    det.attach(&gtk4::Label::new(None), 0, r, 1, 1);
    det.attach(&all_day_check, 1, r, 1, 1);
    r += 1;

    {
        let stp = start_tp.widget.clone();
        let etp = end_tp.widget.clone();
        let apply = move |active: bool| {
            stp.set_sensitive(!active);
            etp.set_sensitive(!active);
        };
        apply(all_day_check.is_active());
        let apply2 = apply.clone();
        all_day_check.connect_toggled(move |cb| apply2(cb.is_active()));
    }

    // Location
    det.attach(&lbl_end("Location:"), 0, r, 1, 1);
    let location_entry = gtk4::Entry::new();
    location_entry.set_hexpand(true);
    if let Some(ev) = existing { location_entry.set_text(&ev.location); }
    det.attach(&location_entry, 1, r, 1, 1);
    r += 1;

    // URL
    det.attach(&lbl_end("URL:"), 0, r, 1, 1);
    let url_entry = gtk4::Entry::new();
    url_entry.set_hexpand(true);
    det.attach(&url_entry, 1, r, 1, 1);
    r += 1;

    // Show as + Private
    det.attach(&lbl_end("Show as:"), 0, r, 1, 1);
    let status_dd = gtk4::DropDown::from_strings(STATUS_LABELS);
    status_dd.set_selected(0);
    let status_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    status_box.append(&status_dd);
    let private_check = gtk4::CheckButton::with_label("Private");
    status_box.append(&private_check);
    det.attach(&status_box, 1, r, 1, 1);
    r += 1;

    // Reminder
    det.attach(&lbl_end("Reminder:"), 0, r, 1, 1);
    let reminder_dd = gtk4::DropDown::from_strings(REMINDER_LABELS);
    reminder_dd.set_selected(0);
    if let Some(ev) = existing {
        // Find matching reminder index
        let sel = REMINDER_MINS_VALUES.iter().position(|&v| v == ev.reminder_mins).unwrap_or(0);
        reminder_dd.set_selected(sel as u32);
    }
    det.attach(&reminder_dd, 1, r, 1, 1);
    r += 1;

    // Label / Category
    det.attach(&lbl_end("Label:"), 0, r, 1, 1);
    let label_dd = gtk4::DropDown::from_strings(LABEL_NAMES);
    label_dd.set_selected(0);
    if let Some(ev) = existing {
        if !ev.label.is_empty() {
            if let Ok(idx) = ev.label.parse::<u32>() {
                if idx < LABEL_NAMES.len() as u32 { label_dd.set_selected(idx); }
            }
        }
    }
    det.attach(&label_dd, 1, r, 1, 1);

    // Pre-fill URL, status, private from existing event
    if let Some(ev) = existing {
        url_entry.set_text(&ev.url);
        private_check.set_active(ev.is_private);
        let status_idx = STATUS_VALUES.iter().position(|&v| v == ev.status).unwrap_or(0);
        status_dd.set_selected(status_idx as u32);
    }

    notebook.append_page(&det, Some(&gtk4::Label::new(Some("Details"))));

    // ================================================================
    //  TAB 2 — Repeat
    // ================================================================
    let rep = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    rep.add_css_class("tab-page");

    let freq_grid = gtk4::Grid::new();
    freq_grid.set_row_spacing(8);
    freq_grid.set_column_spacing(8);
    freq_grid.attach(&lbl_end("Repeat:"), 0, 0, 1, 1);
    let repeat_dd = gtk4::DropDown::from_strings(REPEAT_LABELS);
    repeat_dd.set_selected(0);
    freq_grid.attach(&repeat_dd, 1, 0, 1, 1);

    let interval_spin = gtk4::SpinButton::with_range(1.0, 99.0, 1.0);
    interval_spin.set_value(1.0);
    let interval_unit = gtk4::Label::new(Some("day(s)"));
    let interval_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    interval_row.append(&gtk4::Label::new(Some("Every:")));
    interval_row.append(&interval_spin);
    interval_row.append(&interval_unit);
    freq_grid.attach(&interval_row, 0, 1, 2, 1);
    rep.append(&freq_grid);

    let days_frame = gtk4::Frame::new(Some("Repeat on:"));
    let days_hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    days_hbox.set_margin_start(8); days_hbox.set_margin_end(8);
    days_hbox.set_margin_top(6); days_hbox.set_margin_bottom(6);
    for name in DAY_NAMES {
        days_hbox.append(&gtk4::CheckButton::with_label(name));
    }
    days_frame.set_child(Some(&days_hbox));
    rep.append(&days_frame);

    let ends_frame = gtk4::Frame::new(Some("Ends:"));
    let ends_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    ends_vbox.set_margin_start(8); ends_vbox.set_margin_end(8);
    ends_vbox.set_margin_top(6); ends_vbox.set_margin_bottom(6);

    let ends_never = gtk4::CheckButton::with_label("Never");
    ends_never.set_active(true);
    let ends_after = gtk4::CheckButton::with_label("After:");
    ends_after.set_group(Some(&ends_never));
    let ends_on = gtk4::CheckButton::with_label("On date:");
    ends_on.set_group(Some(&ends_never));
    let ends_count = gtk4::SpinButton::with_range(1.0, 999.0, 1.0);
    ends_count.set_value(10.0);
    ends_count.set_sensitive(false);
    let one_year = default_date.add_years(1).unwrap();
    let ends_date = Rc::new(DatePicker::new(&one_year));
    ends_date.set_sensitive(false);

    ends_vbox.append(&ends_never);
    let ar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    ar.append(&ends_after); ar.append(&ends_count);
    ar.append(&gtk4::Label::new(Some("occurrences")));
    ends_vbox.append(&ar);
    let onr = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    onr.append(&ends_on); onr.append(&ends_date.widget);
    ends_vbox.append(&onr);
    ends_frame.set_child(Some(&ends_vbox));
    rep.append(&ends_frame);

    // Visibility logic
    {
        let ir = interval_row.clone();
        let df = days_frame.clone();
        let ef = ends_frame.clone();
        let iu = interval_unit.clone();
        let uv = Rc::new(move |sel: u32| {
            let has = sel > 0;
            ir.set_visible(has);
            df.set_visible(sel == 2);
            ef.set_visible(has);
            iu.set_label(match sel { 1=>"day(s)", 2=>"week(s)", 3=>"month(s)", 4=>"year(s)", _=>"" });
        });
        uv(0);
        let uv2 = uv.clone();
        repeat_dd.connect_selected_item_notify(move |dd| uv2(dd.selected()));
    }
    {
        let ec = ends_count.clone(); let ed = ends_date.widget.clone();
        ends_never.connect_toggled(move |cb| { if cb.is_active() { ec.set_sensitive(false); ed.set_sensitive(false); }});
    }
    {
        let ec = ends_count.clone(); let ed = ends_date.widget.clone();
        ends_after.connect_toggled(move |cb| { if cb.is_active() { ec.set_sensitive(true); ed.set_sensitive(false); }});
    }
    {
        let ec = ends_count.clone(); let ed = ends_date.widget.clone();
        ends_on.connect_toggled(move |cb| { if cb.is_active() { ec.set_sensitive(false); ed.set_sensitive(true); }});
    }
    notebook.append_page(&rep, Some(&gtk4::Label::new(Some("Repeat"))));

    // ================================================================
    //  TAB 3 — People (autocomplete entry + chips with ✕)
    // ================================================================
    let ppl = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    ppl.add_css_class("tab-page");

    // Chip container (FlowBox wraps like tags)
    let chip_box = gtk4::FlowBox::new();
    chip_box.set_selection_mode(gtk4::SelectionMode::None);
    chip_box.set_homogeneous(false);
    chip_box.set_min_children_per_line(1);
    chip_box.set_max_children_per_line(10);
    let chip_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .min_content_height(80)
        .build();
    chip_scroll.set_child(Some(&chip_box));

    // Entry with autocomplete
    let att_entry = gtk4::Entry::new();
    att_entry.set_hexpand(true);
    att_entry.set_placeholder_text(Some("Type a name or email…"));
    if let Some(ref bks) = books {
        crate::autocomplete::attach(&att_entry, bks.clone());
    }

    ppl.append(&gtk4::Label::builder()
        .label("Attendees:")
        .halign(gtk4::Align::Start)
        .build());
    ppl.append(&att_entry);
    ppl.append(&chip_scroll);

    // Helper: add a chip to the FlowBox
    let add_chip = {
        let cb = chip_box.clone();
        Rc::new(move |name: &str| {
            let text = name.trim().trim_end_matches(',').trim().to_string();
            if text.is_empty() { return; }
            // Duplicate check
            let mut child = cb.first_child();
            while let Some(c) = child {
                if let Some(fb_child) = c.downcast_ref::<gtk4::FlowBoxChild>() {
                    if let Some(inner) = fb_child.child() {
                        if let Some(bx) = inner.downcast_ref::<gtk4::Box>() {
                            if let Some(lbl) = bx.first_child() {
                                if let Some(l) = lbl.downcast_ref::<gtk4::Label>() {
                                    if l.text().to_lowercase() == text.to_lowercase() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
                child = c.next_sibling();
            }

            let chip = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            chip.add_css_class("attendee-chip");
            let lbl = gtk4::Label::new(Some(&text));
            lbl.set_ellipsize(pango::EllipsizeMode::End);
            let x_btn = gtk4::Button::from_icon_name("window-close-symbolic");
            x_btn.add_css_class("flat");
            x_btn.add_css_class("circular");
            x_btn.set_has_frame(false);
            chip.append(&lbl);
            chip.append(&x_btn);
            cb.append(&chip);

            // ✕ removes this chip
            let cb2 = cb.clone();
            let chip_ref = chip.clone();
            x_btn.connect_clicked(move |_| {
                // Find the FlowBoxChild parent and remove it
                if let Some(parent) = chip_ref.parent() {
                    if let Some(fbc) = parent.downcast_ref::<gtk4::FlowBoxChild>() {
                        cb2.remove(fbc);
                    }
                }
            });
        })
    };

    // Enter / comma → add chip
    {
        let ac = add_chip.clone();
        let entry = att_entry.clone();
        att_entry.connect_activate(move |e| {
            ac(&e.text());
            entry.set_text("");
        });
    }

    // Also watch for trailing ", " (autocomplete inserts comma-space)
    {
        let ac = add_chip.clone();
        let entry_cl = att_entry.clone();
        att_entry.connect_changed(move |e| {
            let text = e.text().to_string();
            if text.ends_with(", ") {
                let name = text.trim_end_matches(", ").to_string();
                ac(&name);
                entry_cl.set_text("");
            }
        });
    }

    // Pre-populate attendee chips from existing event
    if let Some(ev) = existing {
        for att in ev.attendees.split('\n') {
            let att = att.trim();
            if !att.is_empty() { add_chip(att); }
        }
    }

    notebook.append_page(&ppl, Some(&gtk4::Label::new(Some("People"))));

    // ================================================================
    //  TAB 4 — Notes
    // ================================================================
    let notes = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    notes.add_css_class("tab-page");
    let desc_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .build();
    let desc_view = gtk4::TextView::new();
    desc_view.set_wrap_mode(gtk4::WrapMode::Word);
    desc_view.set_top_margin(4);
    desc_view.set_left_margin(4);
    desc_view.set_right_margin(4);
    if let Some(ev) = existing { desc_view.buffer().set_text(&ev.description); }
    desc_scroll.set_child(Some(&desc_view));
    notes.append(&desc_scroll);
    notebook.append_page(&notes, Some(&gtk4::Label::new(Some("Notes"))));

    vbox.append(&notebook);

    // ================================================================
    //  Button row
    // ================================================================
    let btn_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    btn_row.set_margin_top(4);

    let result: Rc<RefCell<Option<EventDialogResult>>> = Rc::new(RefCell::new(None));

    if is_edit {
        let del_btn = gtk4::Button::with_label("Delete");
        del_btn.add_css_class("destructive-action");
        btn_row.append(&del_btn);
        let res = result.clone();
        let dlg = dialog.clone();
        let uid_d = uid.clone();
        del_btn.connect_clicked(move |_| {
            *res.borrow_mut() = Some(EventDialogResult::Delete(uid_d.clone()));
            dlg.close();
        });
    }

    // Export .ics button
    if is_edit {
        let export_btn = gtk4::Button::with_label("Export .ics");
        export_btn.set_has_frame(true);
        let existing_ev = existing.cloned();
        let dlg_cl = dialog.clone();
        export_btn.connect_clicked(move |_| {
            if let Some(ref ev) = existing_ev {
                let ics_data = macmbx::ics::event_to_ics(ev);
                let chooser = gtk4::FileDialog::builder()
                    .title("Export Calendar Event")
                    .initial_name(&format!("{}.ics", ev.summary.replace(' ', "_")))
                    .build();
                let data = ics_data.clone();
                chooser.save(Some(&dlg_cl), None::<&gio::Cancellable>, move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            let _ = std::fs::write(&path, &data);
                        }
                    }
                });
            }
        });
        btn_row.append(&export_btn);

        let invite_btn = gtk4::Button::with_label("Send Invite…");
        invite_btn.set_has_frame(true);
        invite_btn.set_tooltip_text(Some("Write .ics to temp file and open a compose email"));
        {
            let existing_ev2 = existing.cloned();
            let dlg_cl2 = dialog.clone();
            invite_btn.connect_clicked(move |_| {
                if let Some(ref ev) = existing_ev2 {
                    let ics_data = macmbx::ics::event_to_ics(ev);

                    // Write to temp file
                    let tmp_dir = std::env::temp_dir();
                    let tmp_path = tmp_dir.join(format!("eudora_invite_{}.ics",
                        ev.uid.replace('@', "_").replace('/', "_")));
                    let _ = std::fs::write(&tmp_path, &ics_data);

                    // Build mailto: URI with attendees, subject, and body summary
                    let to = ev.attendees.replace('\n', ", ");
                    let subject = format!("Invitation: {}", ev.summary);
                    let mut body = format!("You are invited to: {}\n", ev.summary);
                    if !ev.location.is_empty() {
                        body.push_str(&format!("Where: {}\n", ev.location));
                    }
                    body.push_str(&format!("\n(Calendar invitation attached: {})\n", tmp_path.display()));

                    // URL-encode
                    let encode = |s: &str| -> String {
                        s.bytes().map(|b| match b {
                            b' ' => "%20".to_string(),
                            b'&' => "%26".to_string(),
                            b'=' => "%3D".to_string(),
                            b'?' => "%3F".to_string(),
                            b'\n' => "%0A".to_string(),
                            b'\r' => "".to_string(),
                            _ if b.is_ascii_alphanumeric() || b".-_~@,/:".contains(&b) => {
                                (b as char).to_string()
                            }
                            _ => format!("%{:02X}", b),
                        }).collect()
                    };

                    let mailto = format!("mailto:{}?subject={}&body={}",
                        encode(&to), encode(&subject), encode(&body));

                    // Launch via GTK
                    let launcher = gtk4::UriLauncher::new(&mailto);
                    let win = dlg_cl2.clone();
                    launcher.launch(Some(&win), None::<&gio::Cancellable>, |_| {});
                }
            });
        }
        btn_row.append(&invite_btn);
    }

    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    btn_row.append(&spacer);
    let cancel_btn = gtk4::Button::with_label("Cancel");
    let ok_btn = gtk4::Button::with_label(if is_edit { "Save" } else { "Create" });
    ok_btn.add_css_class("suggested-action");
    btn_row.append(&cancel_btn);
    btn_row.append(&ok_btn);
    vbox.append(&btn_row);
    dialog.set_child(Some(&vbox));

    // Cancel
    {
        let dlg = dialog.clone();
        cancel_btn.connect_clicked(move |_| dlg.close());
    }

    // OK — read values from Rc-wrapped pickers
    {
        let res = result.clone();
        let dlg = dialog.clone();
        let uid_ok = uid.clone();
        let te = title_entry.clone();
        let le = location_entry.clone();
        let dv = desc_view.clone();
        let adc = all_day_check.clone();
        let sdp = start_dp.clone();
        let stp = start_tp.clone();
        let edp = end_dp.clone();
        let etp = end_tp.clone();
        let rdd = repeat_dd.clone();
        let ispin = interval_spin.clone();
        let dh = days_hbox.clone();
        let en = ends_never.clone();
        let ea = ends_after.clone();
        let eo = ends_on.clone();
        let ecnt = ends_count.clone();
        let edt = ends_date.clone();
        let existing_color = existing.map(|e| e.color.clone()).unwrap_or_else(|| "#4A90D9".to_string());
        let url_e = url_entry.clone();
        let status_dd_cl = status_dd.clone();
        let private_cl = private_check.clone();
        let reminder_dd_cl = reminder_dd.clone();
        let label_dd_cl = label_dd.clone();
        let chip_box_cl = chip_box.clone();

        ok_btn.connect_clicked(move |_| {
            let summary = te.text().trim().to_string();
            if summary.is_empty() {
                te.add_css_class("error");
                glib::timeout_add_local_once(std::time::Duration::from_secs(2), {
                    let t = te.clone();
                    move || { t.remove_css_class("error"); }
                });
                return;
            }

            let sd = sdp.get_date();
            let ed = edp.get_date();
            let (sh, sm) = if adc.is_active() { (0, 0) } else { (stp.get_hour(), stp.get_minute()) };
            let (eh, em) = if adc.is_active() { (0, 0) } else { (etp.get_hour(), etp.get_minute()) };

            let start_dt = glib::DateTime::from_local(
                sd.year(), sd.month(), sd.day_of_month(), sh, sm, 0.0
            ).unwrap();
            let end_dt = glib::DateTime::from_local(
                ed.year(), ed.month(), ed.day_of_month(), eh, em, 0.0
            ).unwrap();

            let start_day = sd.year() * 10000 + sd.month() * 100 + sd.day_of_month();
            let end_day = ed.year() * 10000 + ed.month() * 100 + ed.day_of_month();

            let buf = dv.buffer();
            let desc = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();

            // Read repeat values
            let repeat_idx = rdd.selected();
            let rrule = match repeat_idx { 1=>"DAILY", 2=>"WEEKLY", 3=>"MONTHLY", 4=>"YEARLY", _=>"" }.to_string();
            let rrule_interval = ispin.value() as i32;

            // BYDAY for weekly: read day checkboxes from days_hbox children
            let rrule_byday = if repeat_idx == 2 {
                let mut codes = Vec::new();
                let day_code_list = ["SU","MO","TU","WE","TH","FR","SA"];
                let mut child = dh.first_child();
                let mut idx = 0;
                while let Some(c) = child {
                    if let Some(cb) = c.downcast_ref::<gtk4::CheckButton>() {
                        if cb.is_active() && idx < day_code_list.len() {
                            codes.push(day_code_list[idx]);
                        }
                        idx += 1;
                    }
                    child = c.next_sibling();
                }
                codes.join(",")
            } else { String::new() };

            let (rrule_count, rrule_until) = if rrule.is_empty() {
                (0, 0)
            } else if ea.is_active() {
                (ecnt.value() as i32, 0)
            } else if eo.is_active() {
                let d = edt.get_date();
                (0, d.year() * 10000 + d.month() * 100 + d.day_of_month())
            } else {
                (0, 0) // Never
            };

            // Read attendees from chip_box
            let mut attendees_list = Vec::new();
            {
                let mut child = chip_box_cl.first_child();
                while let Some(c) = child {
                    if let Some(fb_child) = c.downcast_ref::<gtk4::FlowBoxChild>() {
                        if let Some(inner) = fb_child.child() {
                            if let Some(bx) = inner.downcast_ref::<gtk4::Box>() {
                                if let Some(lbl) = bx.first_child() {
                                    if let Some(l) = lbl.downcast_ref::<gtk4::Label>() {
                                        attendees_list.push(l.text().to_string());
                                    }
                                }
                            }
                        }
                    }
                    child = c.next_sibling();
                }
            }

            let reminder_idx = reminder_dd_cl.selected() as usize;
            let reminder_mins = if reminder_idx < REMINDER_MINS_VALUES.len() {
                REMINDER_MINS_VALUES[reminder_idx]
            } else { 0 };

            let status_idx = status_dd_cl.selected() as usize;
            let status = if status_idx < STATUS_VALUES.len() {
                STATUS_VALUES[status_idx].to_string()
            } else { "busy".to_string() };

            let label_idx = label_dd_cl.selected();
            let label = if label_idx > 0 { format!("{}", label_idx) } else { String::new() };

            let ev = CalendarEvent {
                uid: uid_ok.clone(),
                summary,
                description: desc,
                location: le.text().to_string(),
                dt_start_ts: start_dt.to_unix(),
                dt_end_ts: end_dt.to_unix(),
                start_day,
                end_day,
                all_day: adc.is_active(),
                calendar_id: "local".to_string(),
                color: existing_color.clone(),
                rrule,
                rrule_interval,
                rrule_until,
                rrule_count,
                rrule_byday,
                recur_parent_uid: String::new(),
                attendees: attendees_list.join("\n"),
                status,
                url: url_e.text().to_string(),
                is_private: private_cl.is_active(),
                label,
                reminder_mins,
                ..Default::default()
            };

            *res.borrow_mut() = Some(EventDialogResult::Ok(ev));
            dlg.close();
        });
    }

    // Close → callback
    {
        let res = result.clone();
        let on_done = Rc::new(on_done);
        dialog.connect_close_request(move |_| {
            let r = res.borrow_mut().take().unwrap_or(EventDialogResult::Cancelled);
            on_done(r);
            glib::Propagation::Proceed
        });
    }

    dialog.present();
    title_entry.grab_focus();
}

fn lbl_end(text: &str) -> gtk4::Label {
    gtk4::Label::builder().label(text).halign(gtk4::Align::End).build()
}
