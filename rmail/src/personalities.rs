//! Personalities / Accounts tab — full account management.
//!
//! Port of settings_accounts.c create_accounts_page + open_account_dialog.
//! Layout: account list (ListBox) with Add/Edit/Remove buttons.
//! Each account has: name, real_name, email, type (POP/IMAP), server,
//! smtp_server, username, check_interval, ssl_mode, leave_on_server.
//! Data lives in macmbx::conf::Conf accounts API.

use gtk4::prelude::*;
use gtk4::glib;
use macmbx::conf::{Account, Conf};
use std::cell::RefCell;
use std::rc::Rc;

// ── CSS ──

const PERS_CSS: &str = "
.pers-hero {
    background: linear-gradient(135deg, #4c1d95 0%, #6d28d9 100%);
    padding: 14px 20px; border-radius: 0 0 10px 10px;
}
.pers-hero-title { font-size: 1.1em; font-weight: 700; color: #ffffff; }
.pers-hero-sub { font-size: 0.78em; color: rgba(255,255,255,0.7); }
.pers-count-pill {
    background: rgba(255,255,255,0.20); color: #ffffff;
    border-radius: 12px; padding: 2px 10px;
    font-size: 0.78em; font-weight: 600;
}
";

fn install_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(PERS_CSS);
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

// ── Build account row for the list (port of make_account_row) ──

fn make_account_row(acct: &Account) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    let rbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    rbox.set_margin_start(8);
    rbox.set_margin_end(8);
    rbox.set_margin_top(6);
    rbox.set_margin_bottom(6);

    rbox.append(&gtk4::Image::from_icon_name("mail-send-receive-symbolic"));

    let info = gtk4::Box::new(gtk4::Orientation::Vertical, 2);

    let nm = gtk4::Label::new(None);
    nm.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&acct.name)));
    nm.set_halign(gtk4::Align::Start);
    info.append(&nm);

    let detail = format!("{} — {}  {}", acct.account_type, acct.server,
        if acct.email.is_empty() { String::new() } else { format!("({})", acct.email) });
    let dt = gtk4::Label::new(Some(&detail));
    dt.set_halign(gtk4::Align::Start);
    dt.add_css_class("dim-label");
    info.append(&dt);

    info.set_hexpand(true);
    rbox.append(&info);

    if !acct.enabled {
        rbox.set_opacity(0.5);
    }

    row.set_child(Some(&rbox));
    row
}

// ── Account edit dialog (port of open_account_dialog) ──

fn open_account_dialog(
    parent: Option<&gtk4::Window>,
    conf: Rc<RefCell<Conf>>,
    account_index: Option<i32>,  // None = add new
    on_done: Rc<dyn Fn()>,       // callback to refresh list
) {
    let editing = account_index.is_some();
    let acct = account_index.and_then(|idx| conf.borrow().get_account(idx));

    let dlg = gtk4::Window::builder()
        .title(if editing { "Edit Account" } else { "Add Email Account" })
        .modal(true)
        .default_width(420)
        .build();
    if let Some(p) = parent {
        dlg.set_transient_for(Some(p));
    }

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);

    // ── Identity group ──
    let id_frame = gtk4::Frame::new(Some("Identity"));
    let id_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    id_box.set_margin_start(8); id_box.set_margin_end(8);
    id_box.set_margin_top(4); id_box.set_margin_bottom(8);

    let name_entry = entry_row(&id_box, "Account Name", acct.as_ref().map(|a| a.name.as_str()).unwrap_or(""));
    let realname_entry = entry_row(&id_box, "Real Name", acct.as_ref().map(|a| a.real_name.as_str()).unwrap_or(""));
    let email_entry = entry_row(&id_box, "Email Address", acct.as_ref().map(|a| a.email.as_str()).unwrap_or(""));
    id_frame.set_child(Some(&id_box));
    vbox.append(&id_frame);

    // ── Servers group ──
    let srv_frame = gtk4::Frame::new(Some("Servers"));
    let srv_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    srv_box.set_margin_start(8); srv_box.set_margin_end(8);
    srv_box.set_margin_top(4); srv_box.set_margin_bottom(8);

    let type_dd = {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let lbl = gtk4::Label::new(Some("Type"));
        lbl.set_size_request(110, -1);
        lbl.set_halign(gtk4::Align::End);
        let dd = gtk4::DropDown::from_strings(&["IMAP", "POP"]);
        if let Some(ref a) = acct {
            if a.account_type == "POP" { dd.set_selected(1); }
        }
        dd.set_hexpand(true);
        row.append(&lbl);
        row.append(&dd);
        srv_box.append(&row);
        dd
    };

    let server_entry = entry_row(&srv_box, "Incoming Server", acct.as_ref().map(|a| a.server.as_str()).unwrap_or(""));
    let smtp_entry = entry_row(&srv_box, "SMTP Server", acct.as_ref().map(|a| a.smtp_server.as_str()).unwrap_or(""));
    let user_entry = entry_row(&srv_box, "Username", acct.as_ref().map(|a| a.username.as_str()).unwrap_or(""));
    srv_frame.set_child(Some(&srv_box));
    vbox.append(&srv_frame);

    // ── Options group ──
    let opt_frame = gtk4::Frame::new(Some("Options"));
    let opt_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    opt_box.set_margin_start(8); opt_box.set_margin_end(8);
    opt_box.set_margin_top(4); opt_box.set_margin_bottom(8);

    let check_spin = {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let lbl = gtk4::Label::new(Some("Check every (min)"));
        lbl.set_size_request(110, -1);
        lbl.set_halign(gtk4::Align::End);
        let spin = gtk4::SpinButton::with_range(0.0, 999.0, 1.0);
        spin.set_value(acct.as_ref().map(|a| a.check_interval as f64).unwrap_or(5.0));
        spin.set_hexpand(true);
        row.append(&lbl);
        row.append(&spin);
        opt_box.append(&row);
        spin
    };

    let ssl_dd = {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let lbl = gtk4::Label::new(Some("SSL"));
        lbl.set_size_request(110, -1);
        lbl.set_halign(gtk4::Align::End);
        let dd = gtk4::DropDown::from_strings(&["Off", "Required (STARTTLS)", "Required (Alt Port)"]);
        if let Some(ref a) = acct {
            dd.set_selected(a.ssl_mode as u32);
        }
        dd.set_hexpand(true);
        row.append(&lbl);
        row.append(&dd);
        opt_box.append(&row);
        dd
    };

    let lmos_check = gtk4::CheckButton::with_label("Leave mail on server");
    lmos_check.set_margin_start(8);
    if let Some(ref a) = acct {
        lmos_check.set_active(a.leave_on_server);
    }
    opt_box.append(&lmos_check);
    opt_frame.set_child(Some(&opt_box));
    vbox.append(&opt_frame);

    // ── Button bar ──
    let bbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    bbar.set_halign(gtk4::Align::End);
    bbar.set_margin_top(12);
    let cancel_btn = gtk4::Button::with_label("Cancel");
    let ok_btn = gtk4::Button::with_label(if editing { "Save" } else { "Add" });
    ok_btn.add_css_class("suggested-action");
    bbar.append(&cancel_btn);
    bbar.append(&ok_btn);
    vbox.append(&bbar);

    dlg.set_child(Some(&vbox));

    // Cancel
    {
        let d = dlg.clone();
        cancel_btn.connect_clicked(move |_| d.close());
    }

    // OK / Save
    {
        let d = dlg.clone();
        let c = conf.clone();
        let done = on_done.clone();
        ok_btn.connect_clicked(move |_| {
            let name = name_entry.text().to_string();
            if name.is_empty() { return; }

            let type_str = if type_dd.selected() == 1 { "POP" } else { "IMAP" };
            let new_acct = Account {
                index: account_index.unwrap_or(-1),
                name,
                real_name: realname_entry.text().to_string(),
                email: email_entry.text().to_string(),
                account_type: type_str.to_string(),
                server: server_entry.text().to_string(),
                smtp_server: smtp_entry.text().to_string(),
                username: user_entry.text().to_string(),
                check_interval: check_spin.value() as i32,
                ssl_mode: ssl_dd.selected() as i32,
                leave_on_server: lmos_check.is_active(),
                enabled: true,
            };

            let mut conf = c.borrow_mut();
            if let Some(idx) = account_index {
                conf.update_account(idx, &new_acct);
            } else {
                conf.add_account(&new_acct);
            }
            if let Err(e) = conf.save() {
                eprintln!("rMail: failed to save accounts: {}", e);
            }

            d.close();
            done();
        });
    }

    dlg.present();
}

fn entry_row(parent: &gtk4::Box, label: &str, value: &str) -> gtk4::Entry {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let lbl = gtk4::Label::new(Some(label));
    lbl.set_size_request(110, -1);
    lbl.set_halign(gtk4::Align::End);
    let entry = gtk4::Entry::new();
    entry.set_text(value);
    entry.set_hexpand(true);
    row.append(&lbl);
    row.append(&entry);
    parent.append(&row);
    entry
}

// ── Refresh account list ──

fn refresh_list(list: &gtk4::ListBox, conf: &Conf) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for acct in conf.get_all_accounts() {
        list.append(&make_account_row(&acct));
    }
}

// ── Public: build the personalities tab ──

pub fn build_personalities_tab(conf: Rc<RefCell<Conf>>) -> gtk4::Widget {
    install_css();

    let accounts = conf.borrow().get_all_accounts();
    let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    outer.add_css_class("panel-bg");

    // ── Hero header ──
    let hero = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    hero.add_css_class("pers-hero");

    let hero_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    hero_vbox.set_hexpand(true);
    let hero_title = gtk4::Label::new(Some("Personalities"));
    hero_title.add_css_class("pers-hero-title");
    hero_title.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_title);
    let hero_sub = gtk4::Label::new(Some("Manage your email accounts and identities"));
    hero_sub.add_css_class("pers-hero-sub");
    hero_sub.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_sub);
    hero.append(&hero_vbox);

    let count_pill = gtk4::Label::new(Some(&format!("{} accounts", accounts.len())));
    count_pill.add_css_class("pers-count-pill");
    count_pill.set_valign(gtk4::Align::Center);
    hero.append(&count_pill);
    outer.append(&hero);

    // ── Account list ──
    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    body.set_margin_start(24);
    body.set_margin_end(24);
    body.set_margin_top(16);
    body.set_margin_bottom(16);

    let list_scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();
    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::Single);
    list.add_css_class("boxed-list");

    for acct in &accounts {
        list.append(&make_account_row(acct));
    }

    list_scroll.set_child(Some(&list));
    body.append(&list_scroll);

    // ── Button bar ──
    let btn_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_bar.set_margin_top(8);

    let add_btn = gtk4::Button::with_label("Add");
    let edit_btn = gtk4::Button::with_label("Edit");
    let remove_btn = gtk4::Button::with_label("Remove");
    edit_btn.set_sensitive(false);
    remove_btn.set_sensitive(false);

    btn_bar.append(&add_btn);
    btn_bar.append(&edit_btn);
    btn_bar.append(&remove_btn);
    body.append(&btn_bar);
    outer.append(&body);

    // ── Selection → enable/disable Edit/Remove ──
    {
        let eb = edit_btn.clone();
        let rb = remove_btn.clone();
        list.connect_row_selected(move |_, row| {
            let sel = row.is_some();
            eb.set_sensitive(sel);
            rb.set_sensitive(sel);
        });
    }

    // ── Shared refresh callback ──
    let refresh = {
        let lb = list.clone();
        let c = conf.clone();
        let cp = count_pill.clone();
        Rc::new(move || {
            refresh_list(&lb, &c.borrow());
            cp.set_text(&format!("{} accounts", c.borrow().count_accounts()));
        })
    };

    // ── Add ──
    {
        let c = conf.clone();
        let r = refresh.clone();
        let lb = list.clone();
        add_btn.connect_clicked(move |btn| {
            let win = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            open_account_dialog(win.as_ref(), c.clone(), None, r.clone());
        });
    }

    // ── Edit ──
    {
        let c = conf.clone();
        let r = refresh.clone();
        let lb = list.clone();
        edit_btn.connect_clicked(move |btn| {
            let row = lb.selected_row();
            let idx = row.map(|r| r.index()).unwrap_or(-1);
            if idx < 0 { return; }
            let win = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            open_account_dialog(win.as_ref(), c.clone(), Some(idx), r.clone());
        });
    }

    // ── Remove ──
    {
        let c = conf.clone();
        let r = refresh.clone();
        let lb = list.clone();
        remove_btn.connect_clicked(move |_| {
            let row = lb.selected_row();
            let idx = row.map(|r| r.index()).unwrap_or(-1);
            if idx < 0 { return; }
            let mut conf = c.borrow_mut();
            conf.remove_account(idx);
            if let Err(e) = conf.save() {
                eprintln!("rMail: failed to save after remove: {}", e);
            }
            drop(conf);
            r();
        });
    }

    outer.upcast()
}
