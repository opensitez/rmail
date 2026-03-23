//! Signatures management tab.
//!
//! Port of signaturewin.c OpenSignaturesWin / BuildSigList.
//! Layout: hero header, HPaned(left=signature list, right=editor with Save)
//! Uses macmbx::sigstat::Signatures for file I/O.

use gtk4::prelude::*;
use macmbx::sigstat::Signatures;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

// ── CSS ──

const SIG_CSS: &str = "
.sig-hero {
    background: linear-gradient(135deg, #4a1942 0%, #6b3fa0 100%);
    padding: 14px 20px; border-radius: 0 0 10px 10px;
}
.sig-hero-title { font-size: 1.1em; font-weight: 700; color: #ffffff; }
.sig-hero-sub { font-size: 0.78em; color: rgba(255,255,255,0.7); }
.sig-count-pill {
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
        provider.load_from_data(SIG_CSS);
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

// ── State ──

struct SigState {
    sigs: Signatures,
    selected: i32,
}

// ── Populate list box (port of signaturewin.c:246-253) ──

fn sig_fill_list(list_box: &gtk4::ListBox, sigs: &Signatures) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    for i in 0..sigs.count() {
        if let Some(entry) = sigs.entry(i) {
            let label_text = if i < 2 {
                // Standard and Alternate get a type indicator
                format!("{} ({})", entry.name, if i == 0 { "default" } else { "alternate" })
            } else {
                entry.name.clone()
            };
            let lbl = gtk4::Label::new(Some(&label_text));
            lbl.set_halign(gtk4::Align::Start);
            lbl.set_margin_start(8);
            lbl.set_margin_top(4);
            lbl.set_margin_bottom(4);
            let row = gtk4::ListBoxRow::new();
            row.set_child(Some(&lbl));
            row.set_widget_name(&format!("sig:{}", i));
            list_box.append(&row);
        }
    }
}

/// Parse a signature list row widget name to get the index.
fn parse_sig_row(row: &gtk4::ListBoxRow) -> Option<usize> {
    let name = row.widget_name();
    let name = name.as_str();
    name.strip_prefix("sig:")?.parse::<usize>().ok()
}

// ── Public: build the signatures tab ──

pub fn build_signatures_tab(data_dir: &Path) -> gtk4::Widget {
    install_css();

    let sig_dir = data_dir.join("Signatures");
    let sigs = match Signatures::open(&sig_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rMail: failed to open signatures: {}", e);
            return gtk4::Label::new(Some(&format!("Error loading signatures: {}", e))).upcast();
        }
    };

    let total = sigs.count();

    let state = Rc::new(RefCell::new(SigState {
        sigs,
        selected: -1,
    }));

    let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    outer.add_css_class("panel-bg");

    // ── Hero header (port of signaturewin.c window title/header) ──
    let hero = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    hero.add_css_class("sig-hero");

    let hero_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    hero_vbox.set_hexpand(true);
    let hero_title = gtk4::Label::new(Some("Signatures"));
    hero_title.add_css_class("sig-hero-title");
    hero_title.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_title);
    let hero_sub = gtk4::Label::new(Some("Manage email signatures"));
    hero_sub.add_css_class("sig-hero-sub");
    hero_sub.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_sub);
    hero.append(&hero_vbox);

    let count_pill = gtk4::Label::new(Some(&format!("{} signatures", total)));
    count_pill.add_css_class("sig-count-pill");
    count_pill.set_valign(gtk4::Align::Center);
    hero.append(&count_pill);
    outer.append(&hero);

    // ── Toolbar: New / Remove buttons (port of signaturewin.c:255-267) ──
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);
    toolbar.set_margin_top(6);
    toolbar.set_margin_bottom(4);

    let btn_new = gtk4::Button::with_label("New");
    let btn_remove = gtk4::Button::with_label("Remove");
    toolbar.append(&btn_new);
    toolbar.append(&btn_remove);

    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);

    outer.append(&toolbar);

    // ── HPaned: list | editor ──
    let hpaned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    hpaned.set_position(220);
    hpaned.set_vexpand(true);

    // Left: signature list (port of signaturewin.c:243-253)
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

    // Right: editor panel (port of signaturewin.c Edit button behavior)
    let right_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    right_box.add_css_class("panel-bg");
    right_box.set_margin_start(10);
    right_box.set_margin_end(10);
    right_box.set_margin_top(6);
    right_box.set_margin_bottom(6);

    let sig_name_label = gtk4::Label::new(Some("Select a signature to edit"));
    sig_name_label.set_halign(gtk4::Align::Start);
    sig_name_label.add_css_class("heading");
    right_box.append(&sig_name_label);

    let text_view = gtk4::TextView::new();
    text_view.set_wrap_mode(gtk4::WrapMode::Word);
    text_view.set_editable(true);
    text_view.set_top_margin(8);
    text_view.set_bottom_margin(8);
    text_view.set_left_margin(8);
    text_view.set_right_margin(8);
    let text_scroll = gtk4::ScrolledWindow::builder()
        .child(&text_view)
        .vexpand(true)
        .build();
    right_box.append(&text_scroll);

    let btn_save = gtk4::Button::with_label("Save");
    btn_save.add_css_class("suggested-action");
    btn_save.set_halign(gtk4::Align::End);
    right_box.append(&btn_save);

    hpaned.set_end_child(Some(&right_box));
    hpaned.set_resize_end_child(true);
    hpaned.set_shrink_end_child(false);
    outer.append(&hpaned);

    // Populate initial list
    sig_fill_list(&list_box, &state.borrow().sigs);

    // ── Selection changed -> load signature text into editor ──
    {
        let st = state.clone();
        let tv = text_view.clone();
        let name_lbl = sig_name_label.clone();
        list_box.connect_row_selected(move |_, row| {
            let row = match row {
                Some(r) => r,
                None => {
                    tv.buffer().set_text("");
                    name_lbl.set_text("Select a signature to edit");
                    let mut s = st.borrow_mut();
                    s.selected = -1;
                    return;
                }
            };
            match parse_sig_row(row) {
                Some(idx) => {
                    let mut s = st.borrow_mut();
                    s.selected = idx as i32;
                    let content = s.sigs.get(idx).unwrap_or("").to_string();
                    let sig_name = s.sigs.entry(idx)
                        .map(|e| e.name.clone())
                        .unwrap_or_default();
                    tv.buffer().set_text(&content);
                    name_lbl.set_text(&format!("Editing: {}", sig_name));
                }
                None => {
                    let mut s = st.borrow_mut();
                    s.selected = -1;
                    tv.buffer().set_text("");
                    name_lbl.set_text("Select a signature to edit");
                }
            }
        });
    }

    // ── Save button — save editor text back to signature file ──
    {
        let st = state.clone();
        let tv = text_view.clone();
        btn_save.connect_clicked(move |_| {
            let mut s = st.borrow_mut();
            let idx = s.selected;
            if idx < 0 { return; }
            let idx = idx as usize;
            let buf = tv.buffer();
            let (start, end) = buf.bounds();
            let text = buf.text(&start, &end, false).to_string();
            let _ = s.sigs.set(idx, &text);
            if let Err(e) = s.sigs.save() {
                eprintln!("rMail: failed to save signature: {}", e);
            }
        });
    }

    // ── New button — create a new signature file (port of signaturewin.c New) ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let pill = count_pill.clone();
        btn_new.connect_clicked(move |_| {
            let mut s = st.borrow_mut();
            // Generate a unique name
            let base = "New Signature";
            let mut name = base.to_string();
            let mut counter = 1u32;
            while s.sigs.find(&name).is_some() {
                counter += 1;
                name = format!("{} {}", base, counter);
            }
            let idx = s.sigs.add(&name, "");
            if let Err(e) = s.sigs.save() {
                eprintln!("rMail: failed to save new signature: {}", e);
            }
            pill.set_text(&format!("{} signatures", s.sigs.count()));
            sig_fill_list(&lb, &s.sigs);
            // Select the newly created row
            if let Some(row) = lb.row_at_index(idx as i32) {
                lb.select_row(Some(&row));
            }
        });
    }

    // ── Remove button — delete selected signature (port of signaturewin.c Remove) ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let tv = text_view.clone();
        let name_lbl = sig_name_label.clone();
        let pill = count_pill.clone();
        btn_remove.connect_clicked(move |_| {
            let mut s = st.borrow_mut();
            let idx = s.selected;
            if idx < 0 { return; }
            let idx = idx as usize;
            // Don't allow removing Standard or Alternate (indices 0, 1)
            if idx < 2 {
                eprintln!("rMail: cannot remove Standard or Alternate signatures");
                return;
            }
            if let Err(e) = s.sigs.remove(idx) {
                eprintln!("rMail: failed to remove signature: {}", e);
            }
            s.selected = -1;
            tv.buffer().set_text("");
            name_lbl.set_text("Select a signature to edit");
            pill.set_text(&format!("{} signatures", s.sigs.count()));
            sig_fill_list(&lb, &s.sigs);
        });
    }

    outer.upcast()
}
