//! Make Filter — right-click message → auto-create filter rule.
//!
//! Port of makefilter.c. Extracts From/Subject from the selected message
//! and shows a dialog to create a filter rule.

use gtk4::prelude::*;
use gtk4::glib;
use std::path::PathBuf;

/// Show the Make Filter dialog for a message with the given headers.
/// `from`: sender address, `subject`: message subject.
/// `filter_path`: path to the Filters file.
/// `mailboxes`: list of mailbox names for "Transfer to" dropdown.
pub fn show_make_filter_dialog(
    parent: &gtk4::Widget,
    from: &str,
    subject: &str,
    filter_path: &std::path::Path,
    mailboxes: Vec<String>,
) {
    let toplevel = parent.root()
        .and_then(|r| r.downcast::<gtk4::Window>().ok());

    let dlg = gtk4::Window::builder()
        .title("Make Filter")
        .default_width(480)
        .default_height(-1)
        .modal(true)
        .resizable(false)
        .build();
    if let Some(ref w) = toplevel {
        dlg.set_transient_for(Some(w));
    }

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);

    // ── Match section ──
    let match_frame = gtk4::Frame::new(Some("Match"));
    let match_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    match_box.set_margin_start(8);
    match_box.set_margin_end(8);
    match_box.set_margin_top(8);
    match_box.set_margin_bottom(8);

    // Match type radio buttons
    let match_from = gtk4::CheckButton::with_label("From contains:");
    let match_subj = gtk4::CheckButton::with_label("Subject contains:");
    let match_any = gtk4::CheckButton::with_label("Any recipient contains:");
    match_subj.set_group(Some(&match_from));
    match_any.set_group(Some(&match_from));
    match_from.set_active(true);

    // Pre-filled text entries
    let from_entry = gtk4::Entry::new();
    from_entry.set_text(from);
    from_entry.set_hexpand(true);
    let subj_entry = gtk4::Entry::new();
    subj_entry.set_text(subject);
    subj_entry.set_hexpand(true);
    let recip_entry = gtk4::Entry::new();
    recip_entry.set_text(from);
    recip_entry.set_hexpand(true);

    let row1 = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    row1.append(&match_from);
    row1.append(&from_entry);
    match_box.append(&row1);

    let row2 = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    row2.append(&match_subj);
    row2.append(&subj_entry);
    match_box.append(&row2);

    let row3 = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    row3.append(&match_any);
    row3.append(&recip_entry);
    match_box.append(&row3);

    match_frame.set_child(Some(&match_box));
    vbox.append(&match_frame);

    // ── Action section ──
    let action_frame = gtk4::Frame::new(Some("Action"));
    let action_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    action_box.set_margin_start(8);
    action_box.set_margin_end(8);
    action_box.set_margin_top(8);
    action_box.set_margin_bottom(8);

    let act_none = gtk4::CheckButton::with_label("None (just mark)");
    let act_transfer = gtk4::CheckButton::with_label("Transfer to:");
    let act_delete = gtk4::CheckButton::with_label("Delete");
    let act_junk = gtk4::CheckButton::with_label("Mark as Junk");
    act_transfer.set_group(Some(&act_none));
    act_delete.set_group(Some(&act_none));
    act_junk.set_group(Some(&act_none));
    act_none.set_active(true);

    // Mailbox dropdown for "Transfer to"
    let mbox_list: Vec<&str> = mailboxes.iter().map(|s| s.as_str()).collect();
    let mbox_strings = gtk4::StringList::new(&mbox_list);
    let mbox_dd = gtk4::DropDown::new(Some(mbox_strings), gtk4::Expression::NONE);
    mbox_dd.set_sensitive(false);
    let dd_ref = mbox_dd.clone();
    act_transfer.connect_toggled(move |t| dd_ref.set_sensitive(t.is_active()));
    let dd_ref2 = mbox_dd.clone();
    act_none.connect_toggled(move |_| dd_ref2.set_sensitive(false));
    let dd_ref3 = mbox_dd.clone();
    act_delete.connect_toggled(move |_| dd_ref3.set_sensitive(false));
    let dd_ref4 = mbox_dd.clone();
    act_junk.connect_toggled(move |_| dd_ref4.set_sensitive(false));

    let transfer_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    transfer_row.append(&act_transfer);
    transfer_row.append(&mbox_dd);

    action_box.append(&act_none);
    action_box.append(&transfer_row);
    action_box.append(&act_delete);
    action_box.append(&act_junk);

    action_frame.set_child(Some(&action_box));
    vbox.append(&action_frame);

    // ── When to apply ──
    let when_frame = gtk4::Frame::new(Some("Apply to"));
    let when_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    when_box.set_margin_start(8);
    when_box.set_margin_end(8);
    when_box.set_margin_top(8);
    when_box.set_margin_bottom(8);
    let when_incoming = gtk4::CheckButton::with_label("Incoming");
    when_incoming.set_active(true);
    let when_outgoing = gtk4::CheckButton::with_label("Outgoing");
    let when_manual = gtk4::CheckButton::with_label("Manual");
    when_box.append(&when_incoming);
    when_box.append(&when_outgoing);
    when_box.append(&when_manual);
    when_frame.set_child(Some(&when_box));
    vbox.append(&when_frame);

    // ── Buttons ──
    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk4::Align::End);
    btn_box.set_margin_top(8);
    let cancel_btn = gtk4::Button::with_label("Cancel");
    let create_btn = gtk4::Button::with_label("Create Filter");
    create_btn.add_css_class("suggested-action");
    btn_box.append(&cancel_btn);
    btn_box.append(&create_btn);
    vbox.append(&btn_box);

    let d = dlg.clone();
    cancel_btn.connect_clicked(move |_| d.close());

    // Create filter on click
    let d = dlg.clone();
    let fp = filter_path.to_path_buf();
    create_btn.connect_clicked(move |_| {
        // Determine match header and value
        let (header, value) = if match_from.is_active() {
            ("From".to_string(), from_entry.text().to_string())
        } else if match_subj.is_active() {
            ("Subject".to_string(), subj_entry.text().to_string())
        } else {
            ("Any-Recipient".to_string(), recip_entry.text().to_string())
        };

        if value.is_empty() {
            return;
        }

        // Determine action
        let action = if act_transfer.is_active() {
            let selected = mbox_dd.selected() as usize;
            let dest = mailboxes.get(selected).cloned().unwrap_or_default();
            format!("Transfer {}", dest)
        } else if act_delete.is_active() {
            "Delete".to_string()
        } else if act_junk.is_active() {
            "Junk".to_string()
        } else {
            "None".to_string()
        };

        // Build when flags
        let mut when = 0u32;
        if when_incoming.is_active() { when |= 1; }
        if when_outgoing.is_active() { when |= 2; }
        if when_manual.is_active() { when |= 4; }

        // Append to filter file
        let rule_text = format!(
            "\n#\nrule {}\nmatch {}: contains \"{}\"\naction {}\n",
            when, header, value, action
        );

        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&fp) {
            use std::io::Write;
            let _ = f.write_all(rule_text.as_bytes());
        }

        d.close();
    });

    dlg.set_child(Some(&vbox));
    dlg.present();
}
