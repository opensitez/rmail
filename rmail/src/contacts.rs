//! Address Book tab — full contact editor with tabbed detail view.
//!
//! Port of nickwin.c CreateAddressBookPanel.
//! Layout: hero header, toolbar (New/Delete/To:/Cc:/Bcc:/Save),
//! HPaned(left=nickname list by book, right=nickname entry + 6-tab detail)

use gtk4::prelude::*;
use gtk4::glib;
use macmbx::nicknames::{AddressBooks, AddressBook, Nickname};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

// ── CSS ──

const AB_CSS: &str = "
.ab-hero {
    background: linear-gradient(135deg, #1a4731 0%, #276749 100%);
    padding: 14px 20px; border-radius: 0 0 10px 10px;
}
.ab-hero-title { font-size: 1.1em; font-weight: 700; color: #ffffff; }
.ab-hero-sub { font-size: 0.78em; color: rgba(255,255,255,0.7); }
.ab-count-pill {
    background: rgba(255,255,255,0.20); color: #ffffff;
    border-radius: 12px; padding: 2px 10px;
    font-size: 0.78em; font-weight: 600;
}
.ab-section-title {
    font-weight: 700; font-size: 0.85em;
    color: alpha(@theme_fg_color, 0.55);
    padding: 6px 8px 2px;
}
";

fn install_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(AB_CSS);
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

// ── State ──

struct ABState {
    books: AddressBooks,
    nick_dir: PathBuf,
    selected_book: i32,
    selected_nick: i32,
}

// ── form_row helper (port of nickwin.c:447) ──

fn form_row(label_text: &str) -> (gtk4::Box, gtk4::Entry) {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let lbl = gtk4::Label::new(Some(label_text));
    lbl.set_size_request(90, -1);
    lbl.set_halign(gtk4::Align::End);
    let entry = gtk4::Entry::new();
    entry.set_hexpand(true);
    row.append(&lbl);
    row.append(&entry);
    (row, entry)
}

// ── Tab label with icon (port of nickwin.c:582) ──

fn ab_tab_label(icon_name: &str, text: &str) -> gtk4::Box {
    let bx = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    let icon = gtk4::Image::from_icon_name(icon_name);
    icon.set_pixel_size(14);
    bx.append(&icon);
    bx.append(&gtk4::Label::new(Some(text)));
    bx
}

// ── Populate list box (port of nickwin.c ab_fill_list) ──

fn ab_fill_list(list_box: &gtk4::ListBox, books: &AddressBooks) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    for (bi, book) in books.books.iter().enumerate() {
        // Section header (non-selectable)
        let hdr = gtk4::Label::new(Some(&book.name));
        hdr.set_halign(gtk4::Align::Start);
        hdr.add_css_class("ab-section-title");
        let hdr_row = gtk4::ListBoxRow::new();
        hdr_row.set_selectable(false);
        hdr_row.set_activatable(false);
        hdr_row.set_child(Some(&hdr));
        // Store book index — use widget name as cheap storage
        hdr_row.set_widget_name(&format!("hdr:{}", bi));
        list_box.append(&hdr_row);

        for (ni, nick) in book.entries.iter().enumerate() {
            if nick.deleted { continue; }
            let lbl = gtk4::Label::new(Some(&nick.name));
            lbl.set_halign(gtk4::Align::Start);
            lbl.set_margin_start(12);
            let nick_row = gtk4::ListBoxRow::new();
            nick_row.set_child(Some(&lbl));
            nick_row.set_widget_name(&format!("nick:{}:{}", bi, ni));
            list_box.append(&nick_row);
        }
    }
}

// ── Parse row widget name → (book_idx, nick_idx) ──

fn parse_nick_row(row: &gtk4::ListBoxRow) -> Option<(usize, usize)> {
    let name = row.widget_name();
    let name = name.as_str();
    if let Some(rest) = name.strip_prefix("nick:") {
        let mut parts = rest.split(':');
        let bi = parts.next()?.parse::<usize>().ok()?;
        let ni = parts.next()?.parse::<usize>().ok()?;
        return Some((bi, ni));
    }
    None
}

// ── Detail widgets struct ──

struct DetailWidgets {
    nick_entry: gtk4::Entry,
    addr_view: gtk4::TextView,
    notes_view: gtk4::TextView,
    photo_image: gtk4::Picture,
    /// Current photo path — read from macmbx "picture" field, updated by Select/Remove
    photo_path: RefCell<String>,
    // Personal
    full_name: gtk4::Entry,
    first_name: gtk4::Entry,
    last_name: gtk4::Entry,
    // Home
    h_address: gtk4::Entry,
    h_city: gtk4::Entry,
    h_state: gtk4::Entry,
    h_zip: gtk4::Entry,
    h_country: gtk4::Entry,
    h_phone: gtk4::Entry,
    h_fax: gtk4::Entry,
    h_mobile: gtk4::Entry,
    h_web: gtk4::Entry,
    // Work
    w_title: gtk4::Entry,
    w_company: gtk4::Entry,
    w_address: gtk4::Entry,
    w_city: gtk4::Entry,
    w_state: gtk4::Entry,
    w_zip: gtk4::Entry,
    w_country: gtk4::Entry,
    w_phone: gtk4::Entry,
    w_fax: gtk4::Entry,
    w_mobile: gtk4::Entry,
    w_web: gtk4::Entry,
    // Other
    other_email: gtk4::Entry,
    other_phone: gtk4::Entry,
    other_web: gtk4::Entry,
}

fn clear_detail(d: &DetailWidgets) {
    d.nick_entry.set_text("");
    d.addr_view.buffer().set_text("");
    d.notes_view.buffer().set_text("");
    for e in [&d.full_name, &d.first_name, &d.last_name,
              &d.h_address, &d.h_city, &d.h_state, &d.h_zip, &d.h_country,
              &d.h_phone, &d.h_fax, &d.h_mobile, &d.h_web,
              &d.w_title, &d.w_company, &d.w_address, &d.w_city, &d.w_state,
              &d.w_zip, &d.w_country, &d.w_phone, &d.w_fax, &d.w_mobile, &d.w_web,
              &d.other_email, &d.other_phone, &d.other_web] {
        e.set_text("");
    }
    d.photo_image.set_paintable(gtk4::gdk::Paintable::NONE);
    *d.photo_path.borrow_mut() = String::new();
}

fn get_field<'a>(book: &'a AddressBook, ni: usize, key: &str) -> &'a str {
    book.get_field(ni, key).unwrap_or("")
}

fn display_nick(d: &DetailWidgets, book: &AddressBook, ni: usize) {
    let nick = &book.entries[ni];
    d.nick_entry.set_text(&nick.name);
    d.addr_view.buffer().set_text(nick.addresses.as_deref().unwrap_or(""));

    d.full_name.set_text(get_field(book, ni, "name"));
    d.first_name.set_text(get_field(book, ni, "first"));
    d.last_name.set_text(get_field(book, ni, "last"));

    d.h_address.set_text(get_field(book, ni, "address"));
    d.h_city.set_text(get_field(book, ni, "city"));
    d.h_state.set_text(get_field(book, ni, "state"));
    d.h_zip.set_text(get_field(book, ni, "zip"));
    d.h_country.set_text(get_field(book, ni, "country"));
    d.h_phone.set_text(get_field(book, ni, "phone"));
    d.h_fax.set_text(get_field(book, ni, "fax"));
    d.h_mobile.set_text(get_field(book, ni, "mobile"));
    d.h_web.set_text(get_field(book, ni, "web"));

    d.w_title.set_text(get_field(book, ni, "title"));
    d.w_company.set_text(get_field(book, ni, "company"));
    d.w_address.set_text(get_field(book, ni, "address2"));
    d.w_city.set_text(get_field(book, ni, "city2"));
    d.w_state.set_text(get_field(book, ni, "state2"));
    d.w_zip.set_text(get_field(book, ni, "zip2"));
    d.w_country.set_text(get_field(book, ni, "country2"));
    d.w_phone.set_text(get_field(book, ni, "phone2"));
    d.w_fax.set_text(get_field(book, ni, "fax2"));
    d.w_mobile.set_text(get_field(book, ni, "mobile2"));
    d.w_web.set_text(get_field(book, ni, "web2"));

    d.other_email.set_text(get_field(book, ni, "otheremail"));
    d.other_phone.set_text(get_field(book, ni, "otherphone"));
    d.other_web.set_text(get_field(book, ni, "otherweb"));

    d.notes_view.buffer().set_text(get_field(book, ni, "note"));

    // Photo — read path from macmbx "picture" field (port of nickwin.c:280-284)
    let pic = get_field(book, ni, "picture");
    *d.photo_path.borrow_mut() = pic.to_string();
    if !pic.is_empty() && std::path::Path::new(pic).exists() {
        match gtk4::gdk::Texture::from_filename(pic) {
            Ok(tex) => d.photo_image.set_paintable(Some(&tex)),
            Err(_) => d.photo_image.set_paintable(gtk4::gdk::Paintable::NONE),
        }
    } else {
        d.photo_image.set_paintable(gtk4::gdk::Paintable::NONE);
    }
}

fn save_entry_from(entry: &gtk4::Entry, book: &mut AddressBook, ni: usize, key: &str) {
    let val = entry.text();
    book.set_field(ni, key, &val);
}

fn save_nick(d: &DetailWidgets, books: &mut AddressBooks, bi: usize, ni: usize) {
    let book = &mut books.books[bi];

    // Rename
    let new_name = d.nick_entry.text().to_string();
    if !new_name.is_empty() && new_name != book.entries[ni].name {
        book.rename(ni, &new_name);
    }

    // Addresses
    let buf = d.addr_view.buffer();
    let (start, end) = buf.bounds();
    let addr_text = buf.text(&start, &end, false).to_string();
    book.entries[ni].addresses = if addr_text.is_empty() { None } else { Some(addr_text) };

    save_entry_from(&d.full_name, book, ni, "name");
    save_entry_from(&d.first_name, book, ni, "first");
    save_entry_from(&d.last_name, book, ni, "last");

    save_entry_from(&d.h_address, book, ni, "address");
    save_entry_from(&d.h_city, book, ni, "city");
    save_entry_from(&d.h_state, book, ni, "state");
    save_entry_from(&d.h_zip, book, ni, "zip");
    save_entry_from(&d.h_country, book, ni, "country");
    save_entry_from(&d.h_phone, book, ni, "phone");
    save_entry_from(&d.h_fax, book, ni, "fax");
    save_entry_from(&d.h_mobile, book, ni, "mobile");
    save_entry_from(&d.h_web, book, ni, "web");

    save_entry_from(&d.w_title, book, ni, "title");
    save_entry_from(&d.w_company, book, ni, "company");
    save_entry_from(&d.w_address, book, ni, "address2");
    save_entry_from(&d.w_city, book, ni, "city2");
    save_entry_from(&d.w_state, book, ni, "state2");
    save_entry_from(&d.w_zip, book, ni, "zip2");
    save_entry_from(&d.w_country, book, ni, "country2");
    save_entry_from(&d.w_phone, book, ni, "phone2");
    save_entry_from(&d.w_fax, book, ni, "fax2");
    save_entry_from(&d.w_mobile, book, ni, "mobile2");
    save_entry_from(&d.w_web, book, ni, "web2");

    save_entry_from(&d.other_email, book, ni, "otheremail");
    save_entry_from(&d.other_phone, book, ni, "otherphone");
    save_entry_from(&d.other_web, book, ni, "otherweb");

    // Notes
    let nbuf = d.notes_view.buffer();
    let (ns, ne) = nbuf.bounds();
    let note_text = nbuf.text(&ns, &ne, false).to_string();
    book.set_field(ni, "note", &note_text);

    // Photo path — save back to macmbx (port of nickwin.c:362-363)
    let photo = d.photo_path.borrow().clone();
    book.set_field(ni, "picture", &photo);

    // Save to disk
    if let Err(e) = book.save() {
        eprintln!("rMail: failed to save address book: {}", e);
    }
}

// ── Public: build the address book tab ──

pub fn build_address_book_tab(data_dir: &Path) -> gtk4::Widget {
    install_css();

    let nick_dir = data_dir.join("Nicknames");
    let books = match AddressBooks::open(&nick_dir) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("rMail: failed to open address books: {}", e);
            return gtk4::Label::new(Some(&format!("Error loading address books: {}", e))).upcast();
        }
    };

    // Count total contacts
    let total: usize = books.books.iter()
        .flat_map(|b| b.entries.iter())
        .filter(|n| !n.deleted)
        .count();

    let state = Rc::new(RefCell::new(ABState {
        books,
        nick_dir,
        selected_book: -1,
        selected_nick: -1,
    }));

    let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    outer.add_css_class("panel-bg");

    // ── Hero header (nickwin.c:884-921) ──
    let hero = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    hero.add_css_class("ab-hero");

    let hero_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    hero_vbox.set_hexpand(true);
    let hero_title = gtk4::Label::new(Some("Address Book"));
    hero_title.add_css_class("ab-hero-title");
    hero_title.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_title);
    let hero_sub = gtk4::Label::new(Some("Manage contacts and mailing lists"));
    hero_sub.add_css_class("ab-hero-sub");
    hero_sub.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_sub);
    hero.append(&hero_vbox);

    let count_pill = gtk4::Label::new(Some(&format!("{} contacts", total)));
    count_pill.add_css_class("ab-count-pill");
    count_pill.set_valign(gtk4::Align::Center);
    hero.append(&count_pill);
    outer.append(&hero);

    // ── Toolbar (nickwin.c:923-957) ──
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);
    toolbar.set_margin_top(6);
    toolbar.set_margin_bottom(4);

    let btn_new = gtk4::Button::with_label("New");
    let btn_del = gtk4::Button::with_label("Delete");
    let btn_to = gtk4::Button::with_label("To:");
    let btn_cc = gtk4::Button::with_label("Cc:");
    let btn_bcc = gtk4::Button::with_label("Bcc:");
    let btn_save = gtk4::Button::with_label("Save");
    btn_save.add_css_class("suggested-action");

    toolbar.append(&btn_new);
    toolbar.append(&btn_del);
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);
    toolbar.append(&btn_to);
    toolbar.append(&btn_cc);
    toolbar.append(&btn_bcc);
    toolbar.append(&btn_save);
    outer.append(&toolbar);

    // ── HPaned: list | detail ──
    let hpaned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    hpaned.set_position(240);
    hpaned.set_vexpand(true);

    // Left: list box with section headers per book
    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::Single);
    let list_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .build();
    list_scroll.set_child(Some(&list_box));
    hpaned.set_start_child(Some(&list_scroll));
    hpaned.set_resize_start_child(false);
    hpaned.set_shrink_start_child(false);

    // Right: detail panel
    let right_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    right_box.add_css_class("panel-bg");
    right_box.set_margin_start(10);
    right_box.set_margin_end(10);
    right_box.set_margin_top(6);
    right_box.set_margin_bottom(6);

    // Nickname name field
    let name_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let name_label = gtk4::Label::new(Some("Nickname:"));
    name_label.set_size_request(80, -1);
    let nick_entry = gtk4::Entry::new();
    nick_entry.set_hexpand(true);
    name_row.append(&name_label);
    name_row.append(&nick_entry);
    right_box.append(&name_row);

    // ── 6-tab notebook (port of nickwin.c build_ab_tabs) ──
    let notebook = gtk4::Notebook::new();
    notebook.set_vexpand(true);
    let m = 8;

    // Tab 1: Personal
    let p_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    p_box.set_margin_start(m); p_box.set_margin_end(m);
    p_box.set_margin_top(m); p_box.set_margin_bottom(m);
    let (r, full_name) = form_row("Full Name:");
    p_box.append(&r);
    let (r, first_name) = form_row("First Name:");
    p_box.append(&r);
    let (r, last_name) = form_row("Last Name:");
    p_box.append(&r);
    let addr_lbl = gtk4::Label::new(Some("Address(es):"));
    addr_lbl.set_halign(gtk4::Align::Start);
    p_box.append(&addr_lbl);
    let addr_view = gtk4::TextView::new();
    addr_view.set_wrap_mode(gtk4::WrapMode::WordChar);
    let addr_scroll = gtk4::ScrolledWindow::builder()
        .child(&addr_view).vexpand(true).build();
    p_box.append(&addr_scroll);
    notebook.append_page(&p_box, Some(&ab_tab_label("avatar-default-symbolic", "Personal")));

    // Tab 2: Home
    let h_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .build();
    let h_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    h_box.set_margin_start(m); h_box.set_margin_end(m);
    h_box.set_margin_top(m); h_box.set_margin_bottom(m);
    let (r, h_address) = form_row("Address:"); h_box.append(&r);
    let (r, h_city) = form_row("City:"); h_box.append(&r);
    let (r, h_state) = form_row("State:"); h_box.append(&r);
    let (r, h_zip) = form_row("Zip:"); h_box.append(&r);
    let (r, h_country) = form_row("Country:"); h_box.append(&r);
    let (r, h_phone) = form_row("Phone:"); h_box.append(&r);
    let (r, h_fax) = form_row("Fax:"); h_box.append(&r);
    let (r, h_mobile) = form_row("Mobile:"); h_box.append(&r);
    let (r, h_web) = form_row("Web:"); h_box.append(&r);
    h_scroll.set_child(Some(&h_box));
    notebook.append_page(&h_scroll, Some(&ab_tab_label("go-home-symbolic", "Home")));

    // Tab 3: Work
    let w_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .build();
    let w_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    w_box.set_margin_start(m); w_box.set_margin_end(m);
    w_box.set_margin_top(m); w_box.set_margin_bottom(m);
    let (r, w_title) = form_row("Title:"); w_box.append(&r);
    let (r, w_company) = form_row("Company:"); w_box.append(&r);
    let (r, w_address) = form_row("Address:"); w_box.append(&r);
    let (r, w_city) = form_row("City:"); w_box.append(&r);
    let (r, w_state) = form_row("State:"); w_box.append(&r);
    let (r, w_zip) = form_row("Zip:"); w_box.append(&r);
    let (r, w_country) = form_row("Country:"); w_box.append(&r);
    let (r, w_phone) = form_row("Phone:"); w_box.append(&r);
    let (r, w_fax) = form_row("Fax:"); w_box.append(&r);
    let (r, w_mobile) = form_row("Mobile:"); w_box.append(&r);
    let (r, w_web) = form_row("Web:"); w_box.append(&r);
    w_scroll.set_child(Some(&w_box));
    notebook.append_page(&w_scroll, Some(&ab_tab_label("x-office-document-symbolic", "Work")));

    // Tab 4: Other
    let o_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    o_box.set_margin_start(m); o_box.set_margin_end(m);
    o_box.set_margin_top(m); o_box.set_margin_bottom(m);
    let (r, other_email) = form_row("Other Email:"); o_box.append(&r);
    let (r, other_phone) = form_row("Other Phone:"); o_box.append(&r);
    let (r, other_web) = form_row("Other Web:"); o_box.append(&r);
    notebook.append_page(&o_box, Some(&ab_tab_label("view-more-symbolic", "Other")));

    // Tab 5: Notes
    let n_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    n_box.set_margin_start(m); n_box.set_margin_end(m);
    n_box.set_margin_top(m); n_box.set_margin_bottom(m);
    let notes_view = gtk4::TextView::new();
    notes_view.set_wrap_mode(gtk4::WrapMode::Word);
    let notes_scroll = gtk4::ScrolledWindow::builder()
        .child(&notes_view).vexpand(true).build();
    n_box.append(&notes_scroll);
    notebook.append_page(&n_box, Some(&ab_tab_label("accessories-text-editor-symbolic", "Notes")));

    // Tab 6: Photo
    let ph_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    ph_box.set_margin_start(m); ph_box.set_margin_end(m);
    ph_box.set_margin_top(m); ph_box.set_margin_bottom(m);
    let photo_image = gtk4::Picture::new();
    photo_image.set_content_fit(gtk4::ContentFit::Contain);
    photo_image.set_size_request(200, 200);
    photo_image.set_vexpand(true);
    photo_image.set_halign(gtk4::Align::Center);
    photo_image.set_valign(gtk4::Align::Center);
    let ph_frame = gtk4::Frame::new(None);
    ph_frame.set_child(Some(&photo_image));
    ph_frame.set_vexpand(true);
    ph_box.append(&ph_frame);
    let ph_btns = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    ph_btns.set_halign(gtk4::Align::Center);
    let photo_select_btn = gtk4::Button::with_label("Select Photo…");
    let photo_remove_btn = gtk4::Button::with_label("Remove");
    ph_btns.append(&photo_select_btn);
    ph_btns.append(&photo_remove_btn);
    ph_box.append(&ph_btns);
    let ph_hint = gtk4::Label::new(Some("Supports JPEG, PNG, GIF, WebP, TIFF, SVG"));
    ph_hint.add_css_class("dim-label");
    ph_box.append(&ph_hint);
    notebook.append_page(&ph_box, Some(&ab_tab_label("camera-photo-symbolic", "Photo")));

    right_box.append(&notebook);
    hpaned.set_end_child(Some(&right_box));
    hpaned.set_resize_end_child(true);
    hpaned.set_shrink_end_child(false);
    outer.append(&hpaned);

    // Wrap detail widgets for sharing
    let detail = Rc::new(DetailWidgets {
        nick_entry, addr_view, notes_view,
        photo_image: photo_image.clone(),
        photo_path: RefCell::new(String::new()),
        full_name, first_name, last_name,
        h_address, h_city, h_state, h_zip, h_country,
        h_phone, h_fax, h_mobile, h_web,
        w_title, w_company, w_address, w_city, w_state, w_zip, w_country,
        w_phone, w_fax, w_mobile, w_web,
        other_email, other_phone, other_web,
    });

    // Populate initial list
    ab_fill_list(&list_box, &state.borrow().books);

    // ── Selection changed → display detail ──
    {
        let st = state.clone();
        let d = detail.clone();
        list_box.connect_row_selected(move |_, row| {
            let row = match row {
                Some(r) => r,
                None => { clear_detail(&d); return; }
            };
            match parse_nick_row(row) {
                Some((bi, ni)) => {
                    let mut s = st.borrow_mut();
                    s.selected_book = bi as i32;
                    s.selected_nick = ni as i32;
                    display_nick(&d, &s.books.books[bi], ni);
                }
                None => {
                    let mut s = st.borrow_mut();
                    s.selected_book = -1;
                    s.selected_nick = -1;
                    clear_detail(&d);
                }
            }
        });
    }

    // ── New button ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let d = detail.clone();
        btn_new.connect_clicked(move |_| {
            let mut s = st.borrow_mut();
            if s.books.books.is_empty() { return; }
            // Add to first book
            let book = &mut s.books.books[0];
            // Generate unique name
            let base = "New Nickname";
            let count = book.entries.iter().filter(|n| !n.deleted && n.name.starts_with(base)).count();
            let name = if count == 0 { base.to_string() } else { format!("{} {}", base, count + 1) };
            book.add(&name, None);
            let _ = book.save();
            ab_fill_list(&lb, &s.books);
        });
    }

    // ── Delete button ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let d = detail.clone();
        btn_del.connect_clicked(move |_| {
            let mut s = st.borrow_mut();
            let bi = s.selected_book;
            let ni = s.selected_nick;
            if bi < 0 || ni < 0 { return; }
            let bi = bi as usize;
            let ni = ni as usize;
            if bi >= s.books.books.len() { return; }
            let book = &mut s.books.books[bi];
            if ni >= book.entries.len() { return; }
            book.remove(ni);
            let _ = book.save();
            s.selected_book = -1;
            s.selected_nick = -1;
            clear_detail(&d);
            ab_fill_list(&lb, &s.books);
        });
    }

    // ── Save button ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let d = detail.clone();
        btn_save.connect_clicked(move |_| {
            let mut s = st.borrow_mut();
            let bi = s.selected_book;
            let ni = s.selected_nick;
            if bi < 0 || ni < 0 { return; }
            save_nick(&d, &mut s.books, bi as usize, ni as usize);
            ab_fill_list(&lb, &s.books);
        });
    }

    // ── To:/Cc:/Bcc: buttons — copy addresses to clipboard ──
    for (btn, field) in [(&btn_to, "To"), (&btn_cc, "Cc"), (&btn_bcc, "Bcc")] {
        let st = state.clone();
        let field = field.to_string();
        btn.connect_clicked(move |btn| {
            let s = st.borrow();
            let bi = s.selected_book;
            let ni = s.selected_nick;
            if bi < 0 || ni < 0 { return; }
            let book = &s.books.books[bi as usize];
            if let Some(ref addrs) = book.entries[ni as usize].addresses {
                if !addrs.is_empty() {
                    if let Some(win) = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok()) {
                        let clipboard = gtk4::prelude::WidgetExt::display(&win).clipboard();
                        clipboard.set_text(addrs);
                    }
                }
            }
        });
    }

    // ── Photo select (port of nickwin.c on_photo_select / copy_photo_to_store) ──
    {
        let d = detail.clone();
        let st = state.clone();
        photo_select_btn.connect_clicked(move |btn| {
            let dlg = gtk4::FileDialog::new();
            dlg.set_title("Select Contact Photo");
            // Image file filter
            let filter = gtk4::FileFilter::new();
            filter.set_name(Some("Images"));
            filter.add_mime_type("image/*");
            let filters = gtk4::gio::ListStore::new::<gtk4::FileFilter>();
            filters.append(&filter);
            dlg.set_filters(Some(&filters));

            let toplevel = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            let d2 = d.clone();
            let st2 = st.clone();
            dlg.open(toplevel.as_ref(), gtk4::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result {
                    if let Some(src_path) = file.path() {
                        // Copy photo to Photos dir alongside Nicknames (port of copy_photo_to_store)
                        let s = st2.borrow();
                        let photos_dir = s.nick_dir.parent()
                            .unwrap_or(&s.nick_dir)
                            .join("Photos");
                        let _ = std::fs::create_dir_all(&photos_dir);

                        // Use nickname name as filename
                        let nick_name = d2.nick_entry.text().to_string();
                        let safe_name: String = nick_name.chars()
                            .map(|c| if c == '/' || c == ' ' { '_' } else { c })
                            .collect();
                        let ext = src_path.extension()
                            .map(|e| e.to_string_lossy().to_string())
                            .unwrap_or("png".to_string());
                        let dest = photos_dir.join(format!("{}.{}", safe_name, ext));

                        if std::fs::copy(&src_path, &dest).is_ok() {
                            let dest_str = dest.to_string_lossy().to_string();
                            // Display the photo
                            if let Ok(tex) = gtk4::gdk::Texture::from_filename(&dest) {
                                d2.photo_image.set_paintable(Some(&tex));
                            }
                            // Store path — will be saved to macmbx on Save
                            *d2.photo_path.borrow_mut() = dest_str;
                        }
                    }
                }
            });
        });
    }

    // ── Photo remove ──
    {
        let d = detail.clone();
        photo_remove_btn.connect_clicked(move |_| {
            d.photo_image.set_paintable(gtk4::gdk::Paintable::NONE);
            *d.photo_path.borrow_mut() = String::new();
        });
    }

    outer.upcast()
}
