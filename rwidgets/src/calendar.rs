//! Calendar sidebar panel — mini calendar + agenda view.
//!
//! Port of CalendarWazooWnd.cpp. Accepts a shared CalendarStore and
//! exposes a `CalendarPanel` handle so external code can trigger refreshes.

use gtk4::prelude::*;
use gtk4::glib;
use macmbx::calendar::{CalendarEvent, CalendarStore};
use macmbx::nicknames::AddressBooks;
use crate::calendar_event_dlg::{self, EventDialogResult};
use std::cell::RefCell;
use std::rc::Rc;

/// Handle returned by `build_calendar_panel` so callers can trigger refresh.
pub struct CalendarPanel {
    pub widget: gtk4::Widget,
    refresh_upcoming: Rc<dyn Fn(&glib::DateTime)>,
    refresh_daily: Rc<dyn Fn(&glib::DateTime)>,
    calendar: gtk4::Calendar,
}

impl CalendarPanel {
    /// Re-query the store and rebuild the event lists for the currently selected day.
    pub fn refresh(&self) {
        let date = self.calendar.date();
        (self.refresh_upcoming)(&date);
        (self.refresh_daily)(&date);
    }
}

/// Build the Calendar sidebar tab.
///
/// * `store` — shared CalendarStore (same instance the full view uses)
/// * `on_expand` — called when user clicks the expand arrow
/// * `on_change` — called after the sidebar mutates data (add/edit/delete) so
///   other views can refresh themselves
pub fn build_calendar_panel<E: Fn() + 'static, C: Fn() + 'static>(
    store: Rc<CalendarStore>,
    books: Option<Rc<AddressBooks>>,
    on_expand: E,
    on_change: C,
) -> CalendarPanel {
    let on_change: Rc<dyn Fn()> = Rc::new(on_change);
    let books = Rc::new(books);

    let provider = gtk4::CssProvider::new();
    provider.load_from_data("
        .calendar-sidebar { background: transparent; }
        .cal-header { padding: 4px 8px; font-weight: bold; background: alpha(@theme_fg_color, 0.05); }
        .cal-list { background: transparent; }
        .event-row { padding: 6px 12px; border-bottom: 1px solid alpha(@borders, 0.4); }
        .event-row:hover { background: alpha(@theme_fg_color, 0.05); }
        .event-chip { min-width: 4px; border-radius: 2px; margin-right: 8px; box-shadow: 0 1px 2px alpha(black, 0.2); }
        .event-time { font-size: 0.8em; font-weight: bold; color: @theme_selected_bg_color; }
        .event-summary { font-weight: 500; font-size: 0.95em; }
        .event-location { font-size: 0.85em; opacity: 0.7; }
        .date-header { padding: 6px 12px; font-weight: bold; font-size: 0.85em; opacity: 0.7; background: alpha(@theme_fg_color, 0.03); }
        .toolbar { padding: 4px; border-bottom: 1px solid alpha(@borders, 0.4); }
    ");
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().unwrap(),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vbox.add_css_class("panel-bg");
    vbox.add_css_class("calendar-sidebar");

    // 1. Header
    let header_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    header_box.add_css_class("cal-header");
    let cal_lbl = gtk4::Label::new(Some("Calendar"));
    cal_lbl.set_hexpand(true);
    cal_lbl.set_halign(gtk4::Align::Start);
    let expand_lbl = gtk4::Label::new(Some("↗"));
    expand_lbl.set_tooltip_text(Some("Open full calendar"));
    let click = gtk4::GestureClick::new();
    click.connect_pressed(move |_, n, _, _| { if n == 1 { on_expand(); } });
    expand_lbl.add_controller(click);
    header_box.append(&cal_lbl);
    header_box.append(&expand_lbl);
    vbox.append(&header_box);
    vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // 2. Calendar list
    let cal_list_box = gtk4::ListBox::new();
    cal_list_box.set_selection_mode(gtk4::SelectionMode::None);
    cal_list_box.add_css_class("cal-list");
    for cal in store.get_calendars() {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        row.set_margin_start(8); row.set_margin_top(2); row.set_margin_bottom(2);
        let check = gtk4::CheckButton::new();
        check.set_active(cal.enabled);
        {
            let st = store.clone();
            let cal_id = cal.id.clone();
            let oc = on_change.clone();
            check.connect_toggled(move |cb| {
                st.toggle_calendar_enabled(&cal_id, cb.is_active());
                oc();
            });
        }
        row.append(&check);
        row.append(&gtk4::Label::builder().label(&cal.name).halign(gtk4::Align::Start).hexpand(true).build());
        // Sync indicator for CalDAV calendars
        if cal.source == macmbx::calendar::CalSourceType::CalDAV {
            let badge = gtk4::Label::new(Some("CalDAV"));
            badge.add_css_class("dim-label");
            badge.add_css_class("caption");
            row.append(&badge);
        }

        // Right-click context menu on calendar row
        {
            let rc = gtk4::GestureClick::new();
            rc.set_button(3);
            let st = store.clone();
            let cal_id = cal.id.clone();
            let cal_name = cal.name.clone();
            let cal_source = cal.source.clone();
            let row_w = row.clone();
            let oc = on_change.clone();
            rc.connect_pressed(move |g, _, _, _| {
                g.set_state(gtk4::EventSequenceState::Claimed);
                let popover = gtk4::Popover::new();
                popover.set_parent(&row_w);
                popover.set_has_arrow(true);
                let menu = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

                // Rename
                let rename_btn = gtk4::Button::with_label("Rename…");
                rename_btn.set_has_frame(false);
                rename_btn.add_css_class("flat");
                {
                    let st2 = st.clone();
                    let cid = cal_id.clone();
                    let cname = cal_name.clone();
                    let pop = popover.clone();
                    let rw = row_w.clone();
                    let oc2 = oc.clone();
                    rename_btn.connect_clicked(move |_| {
                        pop.popdown();
                        show_rename_popover(&rw, &st2, &cid, &cname, &oc2);
                    });
                }
                menu.append(&rename_btn);

                // Sync Now (CalDAV only)
                if cal_source == macmbx::calendar::CalSourceType::CalDAV {
                    let sync_btn = gtk4::Button::with_label("Sync Now");
                    sync_btn.set_has_frame(false);
                    sync_btn.add_css_class("flat");
                    let oc2 = oc.clone();
                    let pop = popover.clone();
                    sync_btn.connect_clicked(move |_| {
                        pop.popdown();
                        oc2(); // trigger refresh which will pick up sync
                    });
                    menu.append(&sync_btn);
                }

                // Remove
                let remove_btn = gtk4::Button::with_label("Remove");
                remove_btn.set_has_frame(false);
                remove_btn.add_css_class("flat");
                {
                    let st2 = st.clone();
                    let cid = cal_id.clone();
                    let pop = popover.clone();
                    let oc2 = oc.clone();
                    let rw = row_w.clone();
                    remove_btn.connect_clicked(move |_| {
                        pop.popdown();
                        st2.delete_calendar(&cid);
                        // Remove the row from the UI
                        if let Some(parent) = rw.parent() {
                            if let Some(lb) = parent.downcast_ref::<gtk4::ListBox>() {
                                // Find ListBoxRow parent
                                if let Some(lbr) = rw.parent() {
                                    if let Some(r) = lbr.downcast_ref::<gtk4::ListBoxRow>() {
                                        lb.remove(r);
                                    }
                                }
                            }
                        }
                        oc2();
                    });
                }
                menu.append(&remove_btn);

                popover.set_child(Some(&menu));
                popover.popup();
            });
            row.add_controller(rc);
        }

        cal_list_box.append(&row);
    }
    // "+ Add CalDAV Account" button
    let add_account_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    add_account_row.set_margin_start(8); add_account_row.set_margin_top(4); add_account_row.set_margin_bottom(4);
    let add_btn = gtk4::Button::with_label("+ Add CalDAV Account");
    add_btn.set_has_frame(false);
    add_btn.add_css_class("flat");
    add_btn.add_css_class("dim-label");
    {
        let st = store.clone();
        let clb = cal_list_box.clone();
        add_btn.connect_clicked(move |btn| {
            let st2 = st.clone();
            let clb2 = clb.clone();
            crate::caldav_account_dlg::show_caldav_account_dialog(btn, None, move |result| {
                match result {
                    crate::caldav_account_dlg::AccountDialogResult::Ok(cal) => {
                        st2.upsert_calendar(&cal);
                        // Add row to the list
                        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
                        row.set_margin_start(8); row.set_margin_top(2); row.set_margin_bottom(2);
                        let check = gtk4::CheckButton::new();
                        check.set_active(true);
                        row.append(&check);
                        row.append(&gtk4::Label::builder().label(&cal.name).halign(gtk4::Align::Start).hexpand(true).build());
                        let badge = gtk4::Label::new(Some("CalDAV"));
                        badge.add_css_class("dim-label");
                        badge.add_css_class("caption");
                        row.append(&badge);
                        clb2.append(&row);
                    }
                    crate::caldav_account_dlg::AccountDialogResult::Delete(cal_id) => {
                        st2.delete_calendar(&cal_id);
                    }
                    _ => {}
                }
            });
        });
    }
    add_account_row.append(&add_btn);
    vbox.append(&cal_list_box);
    vbox.append(&add_account_row);
    vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // 3. Mini calendar
    let calendar = gtk4::Calendar::new();
    calendar.set_margin_start(4); calendar.set_margin_end(4);
    calendar.set_margin_top(4); calendar.set_margin_bottom(4);
    vbox.append(&calendar);
    vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // 4. Toolbar
    let toolbar_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar_box.add_css_class("toolbar");
    let view_dropdown = gtk4::DropDown::from_strings(&["Upcoming", "Weekly", "Daily"]);
    view_dropdown.set_hexpand(true);
    let new_btn = gtk4::Button::with_label("+ New");
    new_btn.set_has_frame(false);
    new_btn.add_css_class("flat");
    toolbar_box.append(&view_dropdown);
    toolbar_box.append(&new_btn);
    vbox.append(&toolbar_box);

    // 5. Views stack
    let stack = gtk4::Stack::new();
    stack.set_vexpand(true);

    let upcoming_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never).build();
    let upcoming_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    upcoming_scroll.set_child(Some(&upcoming_box));
    stack.add_named(&upcoming_scroll, Some("upcoming"));

    let daily_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never).build();
    let daily_overlay = gtk4::Overlay::new();
    let daily_bg_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let hour_height = 60.0;

    for hour in 0..24 {
        let hr = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        hr.set_margin_start(8); hr.set_margin_end(8);
        hr.set_size_request(-1, hour_height as i32);
        let lbl = gtk4::Label::new(Some(&format!("{:02}:00", hour)));
        lbl.add_css_class("dim-label");
        lbl.set_size_request(45, -1);
        lbl.set_valign(gtk4::Align::Start);
        lbl.set_margin_top(4);
        let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
        sep.set_valign(gtk4::Align::Start);
        sep.set_margin_top(12);
        sep.set_hexpand(true);
        hr.append(&lbl); hr.append(&sep);
        daily_bg_box.append(&hr);
    }
    daily_overlay.set_child(Some(&daily_bg_box));

    let daily_fixed = gtk4::Fixed::new();
    let fixed_wrap = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    fixed_wrap.set_margin_start(65);
    fixed_wrap.set_hexpand(true);
    fixed_wrap.append(&daily_fixed);
    daily_overlay.add_overlay(&fixed_wrap);
    daily_scroll.set_child(Some(&daily_overlay));
    stack.add_named(&daily_scroll, Some("daily"));
    vbox.append(&stack);

    // ── Refresh closures ───────────────────────────────────────────────
    let refresh_upcoming: Rc<dyn Fn(&glib::DateTime)> = {
        let ub = upcoming_box.clone();
        let st = store.clone();
        let oc = on_change.clone();
        let bk = books.clone();
        Rc::new(move |date: &glib::DateTime| {
            while let Some(child) = ub.first_child() { ub.remove(&child); }

            let dh = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            dh.add_css_class("date-header");
            dh.append(&gtk4::Label::builder()
                .label(date.format("%a, %b %e").unwrap_or_default().as_str())
                .halign(gtk4::Align::Start).build());
            ub.append(&dh);

            let date_int = date.year() * 10000 + date.month() * 100 + date.day_of_month();
            let events = st.get_events_for_date(date_int);
            if events.is_empty() {
                let empty = gtk4::Label::new(Some("No events"));
                empty.add_css_class("dim-label");
                empty.set_margin_top(12); empty.set_margin_bottom(12);
                ub.append(&empty);
            } else {
                for ev in &events {
                    let row = build_event_row(ev);
                    attach_sidebar_event_gestures(&row, &st, &ev.uid, &oc, &bk);
                    ub.append(&row);
                }
            }
        })
    };

    let refresh_daily: Rc<dyn Fn(&glib::DateTime)> = {
        let fixed = daily_fixed.clone();
        let st = store.clone();
        let oc = on_change.clone();
        let bk = books.clone();
        Rc::new(move |date: &glib::DateTime| {
            while let Some(child) = fixed.first_child() { fixed.remove(&child); }

            let date_int = date.year() * 10000 + date.month() * 100 + date.day_of_month();
            let events = st.get_events_for_date(date_int);
            let mut x_offset = 0.0;

            for ev in events {
                if ev.all_day { continue; }
                let dt_start = glib::DateTime::from_unix_local(ev.dt_start_ts).unwrap();
                let dt_end = glib::DateTime::from_unix_local(ev.dt_end_ts).unwrap();
                let sh = dt_start.hour() as f64 + dt_start.minute() as f64 / 60.0;
                let eh = dt_end.hour() as f64 + dt_end.minute() as f64 / 60.0;
                let dur = (eh - sh).max(0.25);
                let y = sh * hour_height;
                let height = dur * hour_height;

                let evt_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                evt_box.add_css_class("event-row");
                evt_box.set_size_request(200, height as i32);
                let safe = ev.uid.replace("@","_").replace("-","_").replace(".","_");
                let p = gtk4::CssProvider::new();
                p.load_from_data(&format!(".evt-pos-{} {{ background: alpha({},0.85); box-shadow: 0 1px 3px alpha(black,0.3); border-radius: 4px; border-left: 4px solid {}; padding: 4px; font-size: 0.8em; color: white; }}", safe, ev.color, ev.color));
                evt_box.style_context().add_provider(&p, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);
                evt_box.add_css_class(&format!("evt-pos-{}", safe));
                evt_box.append(&{
                    let l = gtk4::Label::new(Some(&ev.summary));
                    l.set_halign(gtk4::Align::Start); l.set_ellipsize(pango::EllipsizeMode::End); l
                });

                attach_sidebar_event_gestures(&evt_box, &st, &ev.uid, &oc, &bk);
                fixed.put(&evt_box, x_offset, y);
                x_offset += 25.0;
                if x_offset > 150.0 { x_offset = 0.0; }
            }

            // Now marker
            let now = glib::DateTime::now_local().unwrap();
            if date.year() == now.year() && date.day_of_year() == now.day_of_year() {
                let y = (now.hour() as f64 + now.minute() as f64 / 60.0) * hour_height;
                let marker = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
                marker.set_size_request(200, 2);
                let mp = gtk4::CssProvider::new();
                mp.load_from_data("* { background-color: #ff0000; border-radius: 1px; }");
                marker.style_context().add_provider(&mp, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);
                let dot = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                dot.set_size_request(6, 6);
                let dp = gtk4::CssProvider::new();
                dp.load_from_data("* { background-color: #ff0000; border-radius: 3px; }");
                dot.style_context().add_provider(&dp, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);
                marker.append(&dot);
                fixed.put(&marker, 0.0, y);
            }
        })
    };

    // ── Interactions ───────────────────────────────────────────────────

    // Double-click daily background → new event
    {
        let gc = gtk4::GestureClick::new();
        let st = store.clone();
        let cal = calendar.clone();
        let ru = refresh_upcoming.clone();
        let rd = refresh_daily.clone();
        let dbg = daily_bg_box.clone();
        let oc = on_change.clone();
        let bk = books.clone();
        gc.connect_pressed(move |_, n, _, y| {
            if n == 2 {
                let hour = (y / hour_height).floor() as i32;
                let date = cal.date();
                if hour >= 0 && hour < 24 {
                    let dt = glib::DateTime::from_local(date.year(), date.month(), date.day_of_month(), hour, 0, 0.0).unwrap();
                    let st2 = st.clone(); let ru2 = ru.clone(); let rd2 = rd.clone(); let oc2 = oc.clone();
                    calendar_event_dlg::show_event_dialog(&dbg, &dt, None, (*bk).clone(), move |result| {
                        if let EventDialogResult::Ok(ev) = result {
                            let d = glib::DateTime::from_unix_local(ev.dt_start_ts).unwrap();
                            st2.add_event(&ev); ru2(&d); rd2(&d); oc2();
                        }
                    });
                }
            }
        });
        daily_bg_box.add_controller(gc);
    }

    // + New button
    {
        let st = store.clone();
        let cal = calendar.clone();
        let ru = refresh_upcoming.clone();
        let rd = refresh_daily.clone();
        let oc = on_change.clone();
        let bk = books.clone();
        new_btn.connect_clicked(move |btn| {
            let date = cal.date();
            let now = glib::DateTime::now_local().unwrap();
            let hour = if date.year() != now.year() || date.day_of_year() != now.day_of_year() { 9 } else { now.hour() };
            let dt = glib::DateTime::from_local(date.year(), date.month(), date.day_of_month(), hour, 0, 0.0).unwrap();
            let st2 = st.clone(); let ru2 = ru.clone(); let rd2 = rd.clone(); let oc2 = oc.clone();
            calendar_event_dlg::show_event_dialog(btn, &dt, None, (*bk).clone(), move |result| {
                if let EventDialogResult::Ok(ev) = result {
                    let d = glib::DateTime::from_unix_local(ev.dt_start_ts).unwrap();
                    st2.add_event(&ev); ru2(&d); rd2(&d); oc2();
                }
            });
        });
    }

    // View switcher
    {
        let s = stack.clone();
        view_dropdown.connect_selected_item_notify(move |dd| {
            match dd.selected() {
                0 | 1 => s.set_visible_child_name("upcoming"),
                2 => s.set_visible_child_name("daily"),
                _ => {}
            }
        });
    }

    // Day selected → refresh
    {
        let ru = refresh_upcoming.clone();
        let rd = refresh_daily.clone();
        calendar.connect_day_selected(move |cal| {
            let d = cal.date(); ru(&d); rd(&d);
        });
    }

    // Double-click mini calendar → new event
    {
        let gc = gtk4::GestureClick::new();
        let st = store.clone();
        let cal = calendar.clone();
        let ru = refresh_upcoming.clone();
        let rd = refresh_daily.clone();
        let cw = calendar.clone();
        let oc = on_change.clone();
        let bk = books.clone();
        gc.connect_pressed(move |_, n, _, _| {
            if n == 2 {
                let date = cal.date();
                let dt = glib::DateTime::from_local(date.year(), date.month(), date.day_of_month(), 9, 0, 0.0).unwrap();
                let st2 = st.clone(); let ru2 = ru.clone(); let rd2 = rd.clone(); let oc2 = oc.clone();
                calendar_event_dlg::show_event_dialog(&cw, &dt, None, (*bk).clone(), move |result| {
                    if let EventDialogResult::Ok(ev) = result {
                        let d = glib::DateTime::from_unix_local(ev.dt_start_ts).unwrap();
                        st2.add_event(&ev); ru2(&d); rd2(&d); oc2();
                    }
                });
            }
        });
        calendar.add_controller(gc);
    }

    // Initial render
    let date = calendar.date();
    refresh_upcoming(&date);
    refresh_daily(&date);

    CalendarPanel {
        widget: vbox.upcast(),
        refresh_upcoming,
        refresh_daily,
        calendar,
    }
}

fn build_event_row(ev: &CalendarEvent) -> gtk4::Widget {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    row.add_css_class("event-row");

    let chip = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    chip.add_css_class("event-chip");
    let cp = gtk4::CssProvider::new();
    let safe = ev.uid.replace("@","_").replace("-","_").replace(".","_");
    cp.load_from_data(&format!(".ec-{} {{ background-color: {}; }}", safe, ev.color));
    chip.style_context().add_provider(&cp, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);
    chip.add_css_class(&format!("ec-{}", safe));
    row.append(&chip);

    let info = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    info.set_hexpand(true);

    let dt_s = glib::DateTime::from_unix_local(ev.dt_start_ts).unwrap();
    let dt_e = glib::DateTime::from_unix_local(ev.dt_end_ts).unwrap();
    let time_str = if ev.all_day { "All Day".to_string() }
        else { format!("{}:{:02} – {}:{:02}", dt_s.hour(), dt_s.minute(), dt_e.hour(), dt_e.minute()) };
    let tl = gtk4::Label::new(Some(&time_str));
    tl.set_halign(gtk4::Align::Start); tl.add_css_class("event-time");
    info.append(&tl);

    let sl = gtk4::Label::new(Some(&ev.summary));
    sl.set_halign(gtk4::Align::Start); sl.set_ellipsize(pango::EllipsizeMode::End);
    sl.add_css_class("event-summary");
    info.append(&sl);

    if !ev.location.is_empty() {
        let ll = gtk4::Label::new(Some(&ev.location));
        ll.set_halign(gtk4::Align::Start); ll.add_css_class("event-location");
        ll.set_ellipsize(pango::EllipsizeMode::End);
        info.append(&ll);
    }

    row.append(&info);
    row.upcast()
}

/// Attach double-click (edit) and right-click (Edit/Delete menu) to a sidebar event widget.
fn attach_sidebar_event_gestures(
    widget: &impl IsA<gtk4::Widget>,
    store: &Rc<CalendarStore>,
    uid: &str,
    on_change: &Rc<dyn Fn()>,
    books: &Rc<Option<Rc<AddressBooks>>>,
) {
    // Double-click → edit
    let gc = gtk4::GestureClick::new();
    let uid1 = uid.to_string();
    let st1 = store.clone();
    let w1 = widget.as_ref().clone();
    let oc1 = on_change.clone();
    let bk1 = books.clone();
    gc.connect_pressed(move |g, n, _, _| {
        if n == 2 {
            g.set_state(gtk4::EventSequenceState::Claimed);
            if let Some(ev) = st1.get_event_by_uid(&uid1) {
                let dt = glib::DateTime::from_unix_local(ev.dt_start_ts).unwrap();
                let st2 = st1.clone();
                let oc2 = oc1.clone();
                calendar_event_dlg::show_event_dialog(&w1, &dt, Some(&ev), (*bk1).clone(), move |result| {
                    match result {
                        EventDialogResult::Ok(u) => { st2.add_event(&u); oc2(); }
                        EventDialogResult::Delete(uid) => { st2.delete_event(&uid); oc2(); }
                        _ => {}
                    }
                });
            }
        }
    });
    widget.add_controller(gc);

    // Right-click → context menu
    let rc = gtk4::GestureClick::new();
    rc.set_button(3);
    let uid2 = uid.to_string();
    let st2 = store.clone();
    let w2 = widget.as_ref().clone();
    let oc2 = on_change.clone();
    let bk2 = books.clone();
    rc.connect_pressed(move |g, _, _, _| {
        g.set_state(gtk4::EventSequenceState::Claimed);

        let popover = gtk4::Popover::new();
        popover.set_parent(&w2);
        popover.set_has_arrow(true);

        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

        let edit_btn = gtk4::Button::with_label("Edit Event…");
        edit_btn.set_has_frame(false);
        edit_btn.add_css_class("flat");
        let uid_e = uid2.clone();
        let st_e = st2.clone();
        let w_e = w2.clone();
        let oc_e = oc2.clone();
        let pop_e = popover.clone();
        let bk_e = bk2.clone();
        edit_btn.connect_clicked(move |_| {
            pop_e.popdown();
            if let Some(ev) = st_e.get_event_by_uid(&uid_e) {
                let dt = glib::DateTime::from_unix_local(ev.dt_start_ts).unwrap();
                let st3 = st_e.clone();
                let oc3 = oc_e.clone();
                calendar_event_dlg::show_event_dialog(&w_e, &dt, Some(&ev), (*bk_e).clone(), move |result| {
                    match result {
                        EventDialogResult::Ok(u) => { st3.add_event(&u); oc3(); }
                        EventDialogResult::Delete(uid) => { st3.delete_event(&uid); oc3(); }
                        _ => {}
                    }
                });
            }
        });
        vbox.append(&edit_btn);

        let del_btn = gtk4::Button::with_label("Delete Event");
        del_btn.set_has_frame(false);
        del_btn.add_css_class("flat");
        let uid_d = uid2.clone();
        let st_d = st2.clone();
        let oc_d = oc2.clone();
        let pop_d = popover.clone();
        del_btn.connect_clicked(move |_| {
            pop_d.popdown();
            st_d.delete_event(&uid_d);
            oc_d();
        });
        vbox.append(&del_btn);

        popover.set_child(Some(&vbox));
        popover.popup();
    });
    widget.add_controller(rc);
}

/// Show an inline rename popover for a calendar.
fn show_rename_popover(
    parent: &impl IsA<gtk4::Widget>,
    store: &Rc<CalendarStore>,
    cal_id: &str,
    current_name: &str,
    on_change: &Rc<dyn Fn()>,
) {
    let popover = gtk4::Popover::new();
    popover.set_parent(parent.as_ref());
    popover.set_has_arrow(true);

    let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    hbox.set_margin_start(8); hbox.set_margin_end(8);
    hbox.set_margin_top(8); hbox.set_margin_bottom(8);

    let entry = gtk4::Entry::new();
    entry.set_text(current_name);
    entry.set_hexpand(true);
    let ok_btn = gtk4::Button::with_label("Rename");
    ok_btn.add_css_class("suggested-action");
    hbox.append(&entry);
    hbox.append(&ok_btn);
    popover.set_child(Some(&hbox));

    // OK button or Enter → rename
    {
        let st = store.clone();
        let cid = cal_id.to_string();
        let pop = popover.clone();
        let ent = entry.clone();
        let oc = on_change.clone();
        let pw = parent.as_ref().clone();
        let do_rename = Rc::new(move || {
            let new_name = ent.text().trim().to_string();
            if !new_name.is_empty() {
                st.rename_calendar(&cid, &new_name);
                // Update the label in the row
                // Walk siblings of the parent row to find the Label
                let mut child = pw.first_child();
                while let Some(c) = child {
                    if let Some(lbl) = c.downcast_ref::<gtk4::Label>() {
                        if lbl.halign() == gtk4::Align::Start {
                            lbl.set_label(&new_name);
                            break;
                        }
                    }
                    child = c.next_sibling();
                }
                oc();
            }
            pop.popdown();
        });

        let r1 = do_rename.clone();
        ok_btn.connect_clicked(move |_| r1());
        let r2 = do_rename.clone();
        entry.connect_activate(move |_| r2());
    }

    popover.popup();
    entry.grab_focus();
    entry.select_region(0, -1);
}
