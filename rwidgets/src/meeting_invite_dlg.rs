//! Meeting Invite Dialog — shown when a .ics attachment is opened from email.
//!
//! Port of CHeaderView::ShowMeetingInviteDlg / TryImportIcsFile.
//! Shows event details and Accept / Tentative / Decline / Close buttons.
//! All three responses send a METHOD:REPLY ICS back to the organizer.

use gtk4::prelude::*;
use gtk4::glib;
use macmbx::calendar::{CalendarEvent, CalendarStore};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum InviteResponse {
    Accept,
    Tentative,
    Decline,
    Close,
}

/// Try to import an .ics file from an email attachment.
///
/// * `ics_data` — the raw iCalendar string
/// * `store` — the calendar store to add events to
/// * `parent` — parent widget for dialogs
/// * `self_email` — the user's own email for building REPLY ICS
/// * `on_compose_reply` — called with (to, subject, body, reply_ics_path) to
///   open a compose window with the reply .ics attached
/// * `on_calendar_changed` — called after accepting/deleting an event
pub fn try_import_ics<R: Fn(&str, &str, &str, &str) + 'static, C: Fn() + 'static>(
    ics_data: &str,
    store: &Rc<CalendarStore>,
    parent: &impl IsA<gtk4::Widget>,
    self_email: &str,
    on_compose_reply: R,
    on_calendar_changed: C,
) {
    let ev = match macmbx::ics::ics_to_event(ics_data) {
        Some(ev) => ev,
        None => {
            show_message(parent, "Calendar Import",
                "This file does not appear to be a valid calendar event (.ics).");
            return;
        }
    };

    // METHOD:CANCEL — offer to remove the event
    if ev.method == "CANCEL" {
        let msg = format!(
            "The event \u{201C}{}\u{201D} has been cancelled by the organizer.\n\n\
             Would you like to remove it from your calendar?",
            ev.summary
        );
        show_confirm(parent, "Event Cancelled", &msg, {
            let st = store.clone();
            let uid = ev.uid.clone();
            let oc = on_calendar_changed;
            move || {
                if !uid.is_empty() { st.delete_event(&uid); }
                oc();
            }
        });
        return;
    }

    // Show Accept/Tentative/Decline dialog
    let dialog = gtk4::Window::builder()
        .title("Meeting Invitation")
        .modal(true)
        .default_width(460)
        .default_height(300)
        .resizable(true)
        .build();

    if let Some(root) = parent.root() {
        if let Some(win) = root.downcast_ref::<gtk4::Window>() {
            dialog.set_transient_for(Some(win));
        }
    }

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    vbox.set_margin_start(16); vbox.set_margin_end(16);
    vbox.set_margin_top(16); vbox.set_margin_bottom(16);

    // Event info
    let mut info = String::new();
    if !ev.organizer_name.is_empty() {
        info.push_str(&format!("From:\t{}\n", ev.organizer_name));
    } else if !ev.organizer.is_empty() {
        info.push_str(&format!("From:\t{}\n", ev.organizer));
    }
    info.push_str(&format!("Event:\t{}\n", ev.summary));
    if ev.dt_start_ts > 0 {
        let dt = glib::DateTime::from_unix_local(ev.dt_start_ts).unwrap();
        if ev.all_day {
            info.push_str(&format!("When:\t{}\n",
                dt.format("%A, %B %e, %Y").unwrap_or_default()));
        } else {
            let dt_end = glib::DateTime::from_unix_local(ev.dt_end_ts).unwrap();
            info.push_str(&format!("When:\t{} \u{2013} {}\n",
                dt.format("%A, %B %e %Y  %l:%M %p").unwrap_or_default(),
                dt_end.format("%l:%M %p").unwrap_or_default()));
        }
    }
    if !ev.location.is_empty() {
        info.push_str(&format!("Where:\t{}\n", ev.location));
    }
    if !ev.description.is_empty() && ev.description.len() < 200 {
        info.push_str(&format!("\n{}", ev.description));
    }

    let info_label = gtk4::Label::new(Some(&info));
    info_label.set_halign(gtk4::Align::Start);
    info_label.set_wrap(true);
    info_label.set_selectable(true);
    vbox.append(&info_label);

    let btn_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_row.set_margin_top(12);
    let accept_btn = gtk4::Button::with_label("Accept");
    accept_btn.add_css_class("suggested-action");
    let tent_btn = gtk4::Button::with_label("Tentative");
    let decline_btn = gtk4::Button::with_label("Decline");
    decline_btn.add_css_class("destructive-action");
    let close_btn = gtk4::Button::with_label("Close");
    btn_row.append(&accept_btn);
    btn_row.append(&tent_btn);
    btn_row.append(&decline_btn);
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    btn_row.append(&spacer);
    btn_row.append(&close_btn);
    vbox.append(&btn_row);
    dialog.set_child(Some(&vbox));

    let result: Rc<RefCell<InviteResponse>> = Rc::new(RefCell::new(InviteResponse::Close));

    {
        let r = result.clone(); let d = dialog.clone();
        accept_btn.connect_clicked(move |_| { *r.borrow_mut() = InviteResponse::Accept; d.close(); });
    }
    {
        let r = result.clone(); let d = dialog.clone();
        tent_btn.connect_clicked(move |_| { *r.borrow_mut() = InviteResponse::Tentative; d.close(); });
    }
    {
        let r = result.clone(); let d = dialog.clone();
        decline_btn.connect_clicked(move |_| { *r.borrow_mut() = InviteResponse::Decline; d.close(); });
    }
    {
        let d = dialog.clone();
        close_btn.connect_clicked(move |_| d.close());
    }

    let self_email = self_email.to_string();
    {
        let r = result.clone();
        let st = store.clone();
        let ev = ev.clone();
        dialog.connect_close_request(move |_| {
            let ans = r.borrow().clone();
            if ans == InviteResponse::Close { return glib::Propagation::Proceed; }

            let partstat = match ans {
                InviteResponse::Accept => "ACCEPTED",
                InviteResponse::Tentative => "TENTATIVE",
                InviteResponse::Decline => "DECLINED",
                _ => unreachable!(),
            };

            // For Accept/Tentative: save to calendar
            if ans == InviteResponse::Accept || ans == InviteResponse::Tentative {
                let mut ev_save = ev.clone();
                ev_save.status = if ans == InviteResponse::Tentative {
                    "tentative".to_string()
                } else {
                    "busy".to_string()
                };
                st.add_event(&ev_save);
                on_calendar_changed();
            }

            // For all three: build REPLY ICS and open compose
            let reply_ics = macmbx::ics::build_reply_ics(&ev, partstat, &self_email);
            let tmp = std::env::temp_dir().join(format!("eudora_reply_{}.ics",
                ev.uid.replace('@', "_").replace('/', "_")));
            let _ = std::fs::write(&tmp, &reply_ics);

            let to = if !ev.organizer.is_empty() {
                ev.organizer.clone()
            } else {
                ev.attendees.split('\n').next().unwrap_or("").to_string()
            };

            let prefix = match ans {
                InviteResponse::Accept => "Accepted",
                InviteResponse::Tentative => "Tentative",
                InviteResponse::Decline => "Declined",
                _ => "",
            };
            let subject = format!("{}: {}", prefix, ev.summary);

            let mut body = String::new();
            match ans {
                InviteResponse::Accept => body.push_str("I have accepted the following meeting:\n\n"),
                InviteResponse::Tentative => body.push_str("I have tentatively accepted the following meeting:\n\n"),
                InviteResponse::Decline => body.push_str("I am unable to attend the following meeting:\n\n"),
                _ => {}
            }
            body.push_str(&format!("Event: {}\n", ev.summary));
            if ev.dt_start_ts > 0 {
                let dt = glib::DateTime::from_unix_local(ev.dt_start_ts).unwrap();
                body.push_str(&format!("When: {}\n",
                    dt.format("%A, %B %e %Y %l:%M %p").unwrap_or_default()));
            }
            if !ev.location.is_empty() {
                body.push_str(&format!("Where: {}\n", ev.location));
            }

            on_compose_reply(&to, &subject, &body, &tmp.to_string_lossy());

            glib::Propagation::Proceed
        });
    }

    dialog.present();
}

fn show_message(parent: &impl IsA<gtk4::Widget>, title: &str, msg: &str) {
    let dlg = gtk4::AlertDialog::builder()
        .message(title)
        .detail(msg)
        .modal(true)
        .build();
    if let Some(root) = parent.root() {
        if let Some(win) = root.downcast_ref::<gtk4::Window>() {
            dlg.show(Some(win));
        }
    }
}

fn show_confirm<F: Fn() + 'static>(parent: &impl IsA<gtk4::Widget>, title: &str, msg: &str, on_yes: F) {
    let dlg = gtk4::AlertDialog::builder()
        .message(title)
        .detail(msg)
        .modal(true)
        .buttons(["No", "Yes"])
        .default_button(1)
        .build();
    let on_yes = Rc::new(on_yes);
    if let Some(root) = parent.root() {
        if let Some(win) = root.downcast_ref::<gtk4::Window>() {
            let y = on_yes.clone();
            dlg.choose(Some(win), None::<&gtk4::gio::Cancellable>, move |result| {
                if let Ok(1) = result { y(); }
            });
        }
    }
}
