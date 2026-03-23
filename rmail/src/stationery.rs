//! Stationery management tab — template list with New/Remove/Edit.
//!
//! Port of stationerywin.c OpenStationeryWin / BuildStationeryList.
//! Layout: hero header, toolbar (New/Remove/Edit), ListBox of template names.
//! Templates are files in `<data_dir>/Stationery/`.

use gtk4::prelude::*;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

// ── CSS (matches pattern from contacts.rs / filters.rs) ──

const STNY_CSS: &str = "
.stny-hero {
    background: linear-gradient(135deg, #744210 0%, #b7791f 100%);
    padding: 14px 20px; border-radius: 0 0 10px 10px;
}
.stny-hero-title { font-size: 1.1em; font-weight: 700; color: #ffffff; }
.stny-hero-sub { font-size: 0.78em; color: rgba(255,255,255,0.7); }
.stny-count-pill {
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
        provider.load_from_data(STNY_CSS);
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

// ── State ──

struct StnyState {
    stny_dir: PathBuf,
    /// Sorted list of template file names (without path).
    templates: Vec<String>,
    /// Currently selected index, or -1.
    selected: i32,
}

impl StnyState {
    fn new(stny_dir: PathBuf) -> Self {
        let templates = Self::read_templates(&stny_dir);
        StnyState { stny_dir, templates, selected: -1 }
    }

    /// Read the Stationery directory and return sorted file names.
    fn read_templates(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        // Skip hidden files
                        if !name.starts_with('.') {
                            names.push(name.to_string());
                        }
                    }
                }
            }
        }
        names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        names
    }

    fn reload(&mut self) {
        self.templates = Self::read_templates(&self.stny_dir);
        self.selected = -1;
    }
}

// ── Populate the list box from state ──

fn fill_list(list_box: &gtk4::ListBox, state: &StnyState) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    for (i, name) in state.templates.iter().enumerate() {
        let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        row_box.set_margin_start(8);
        row_box.set_margin_end(8);
        row_box.set_margin_top(4);
        row_box.set_margin_bottom(4);

        let icon = gtk4::Image::from_icon_name("x-office-document-symbolic");
        icon.set_pixel_size(16);
        row_box.append(&icon);

        let lbl = gtk4::Label::new(Some(name));
        lbl.set_halign(gtk4::Align::Start);
        lbl.set_hexpand(true);
        row_box.append(&lbl);

        let row = gtk4::ListBoxRow::new();
        row.set_child(Some(&row_box));
        row.set_widget_name(&format!("stny:{}", i));
        list_box.append(&row);
    }
}

// ── Build the template editor (compose-like) ──

fn build_editor(stny_dir: &Path, name: &str) -> gtk4::Box {
    let editor_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    editor_box.set_margin_start(10);
    editor_box.set_margin_end(10);
    editor_box.set_margin_top(6);
    editor_box.set_margin_bottom(6);

    // Header fields (To, Subject)
    let to_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let to_lbl = gtk4::Label::new(Some("To:"));
    to_lbl.set_size_request(60, -1);
    to_lbl.set_halign(gtk4::Align::End);
    let to_entry = gtk4::Entry::new();
    to_entry.set_hexpand(true);
    to_row.append(&to_lbl);
    to_row.append(&to_entry);
    editor_box.append(&to_row);

    let subj_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let subj_lbl = gtk4::Label::new(Some("Subject:"));
    subj_lbl.set_size_request(60, -1);
    subj_lbl.set_halign(gtk4::Align::End);
    let subj_entry = gtk4::Entry::new();
    subj_entry.set_hexpand(true);
    subj_row.append(&subj_lbl);
    subj_row.append(&subj_entry);
    editor_box.append(&subj_row);

    // Body
    let body_view = gtk4::TextView::new();
    body_view.set_wrap_mode(gtk4::WrapMode::WordChar);
    let body_scroll = gtk4::ScrolledWindow::builder()
        .child(&body_view)
        .vexpand(true)
        .build();
    editor_box.append(&body_scroll);

    // Load existing template content
    let file_path = stny_dir.join(name);
    if let Ok(content) = fs::read_to_string(&file_path) {
        // Parse simple format: first line "To: ...", second "Subject: ...", rest is body.
        let mut lines = content.lines();
        if let Some(first) = lines.next() {
            if let Some(to_val) = first.strip_prefix("To: ") {
                to_entry.set_text(to_val);
            } else {
                // Not our format — put everything in body
                body_view.buffer().set_text(&content);
                return editor_box;
            }
        }
        if let Some(second) = lines.next() {
            if let Some(subj_val) = second.strip_prefix("Subject: ") {
                subj_entry.set_text(subj_val);
            }
        }
        // Skip blank line separator if present
        let remaining: Vec<&str> = lines.collect();
        let body_text = if remaining.first().map_or(false, |l| l.is_empty()) {
            remaining[1..].join("\n")
        } else {
            remaining.join("\n")
        };
        body_view.buffer().set_text(&body_text);
    }

    // Save button
    let save_btn = gtk4::Button::with_label("Save Template");
    save_btn.add_css_class("suggested-action");
    save_btn.set_halign(gtk4::Align::End);

    let fp = file_path.clone();
    let to_e = to_entry.clone();
    let subj_e = subj_entry.clone();
    let body_v = body_view.clone();
    save_btn.connect_clicked(move |_| {
        let to_text = to_e.text();
        let subj_text = subj_e.text();
        let buf = body_v.buffer();
        let (start, end) = buf.bounds();
        let body_text = buf.text(&start, &end, false);

        let content = format!("To: {}\nSubject: {}\n\n{}", to_text, subj_text, body_text);
        if let Err(e) = fs::write(&fp, &content) {
            eprintln!("rMail: failed to save stationery: {}", e);
        }
    });
    editor_box.append(&save_btn);

    editor_box
}

// ── Public entry point ──

pub fn build_stationery_tab(data_dir: &Path) -> gtk4::Widget {
    install_css();

    let stny_dir = data_dir.join("Stationery");
    // Ensure directory exists
    let _ = fs::create_dir_all(&stny_dir);

    let state = Rc::new(RefCell::new(StnyState::new(stny_dir.clone())));
    let total = state.borrow().templates.len();

    let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    outer.add_css_class("panel-bg");

    // ── Hero header (port of stationerywin.c layout, style matches other tabs) ──
    let hero = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    hero.add_css_class("stny-hero");

    let hero_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    hero_vbox.set_hexpand(true);
    let hero_title = gtk4::Label::new(Some("Stationery"));
    hero_title.add_css_class("stny-hero-title");
    hero_title.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_title);
    let hero_sub = gtk4::Label::new(Some("Manage message templates"));
    hero_sub.add_css_class("stny-hero-sub");
    hero_sub.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_sub);
    hero.append(&hero_vbox);

    let count_pill = gtk4::Label::new(Some(&format!("{} templates", total)));
    count_pill.add_css_class("stny-count-pill");
    count_pill.set_valign(gtk4::Align::Center);
    hero.append(&count_pill);
    outer.append(&hero);

    // ── Toolbar: New / Remove / Edit (port of stationerywin.c:153-175) ──
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);
    toolbar.set_margin_top(6);
    toolbar.set_margin_bottom(4);

    let btn_new = gtk4::Button::with_label("New");
    let btn_remove = gtk4::Button::with_label("Remove");
    let btn_edit = gtk4::Button::with_label("Edit");

    // Disable until selection (port of stationerywin.c:158-159)
    btn_remove.set_sensitive(false);
    btn_edit.set_sensitive(false);

    toolbar.append(&btn_new);
    toolbar.append(&btn_remove);
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);
    toolbar.append(&btn_edit);
    outer.append(&toolbar);

    // ── HPaned: list | editor ──
    let hpaned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    hpaned.set_position(240);
    hpaned.set_vexpand(true);

    // Left: template list (port of stationerywin.c:146-150)
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

    // Right: editor area (starts with placeholder)
    let right_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    right_box.set_hexpand(true);
    right_box.set_vexpand(true);
    let placeholder = gtk4::Label::new(Some("Select a template to edit, or click New to create one."));
    placeholder.add_css_class("dim-label");
    placeholder.set_vexpand(true);
    placeholder.set_valign(gtk4::Align::Center);
    right_box.append(&placeholder);

    hpaned.set_end_child(Some(&right_box));
    hpaned.set_resize_end_child(true);
    hpaned.set_shrink_end_child(false);
    outer.append(&hpaned);

    // Populate initial list
    fill_list(&list_box, &state.borrow());

    // ── Selection changed (port of OnStationerySelected) ──
    {
        let st = state.clone();
        let btn_r = btn_remove.clone();
        let btn_e = btn_edit.clone();
        let rb = right_box.clone();
        let sd = stny_dir.clone();
        list_box.connect_row_selected(move |_, row| {
            let has_sel = row.is_some();
            btn_r.set_sensitive(has_sel);
            btn_e.set_sensitive(has_sel);

            // Clear editor area
            while let Some(child) = rb.first_child() {
                rb.remove(&child);
            }

            if let Some(row) = row {
                let name = row.widget_name();
                if let Some(idx_str) = name.as_str().strip_prefix("stny:") {
                    if let Ok(idx) = idx_str.parse::<usize>() {
                        let s = st.borrow();
                        if let Some(template_name) = s.templates.get(idx) {
                            let editor = build_editor(&sd, template_name);
                            rb.append(&editor);

                            // Update selected index
                            drop(s);
                            st.borrow_mut().selected = idx as i32;
                        }
                    }
                }
            } else {
                let ph = gtk4::Label::new(Some("Select a template to edit, or click New to create one."));
                ph.add_css_class("dim-label");
                ph.set_vexpand(true);
                ph.set_valign(gtk4::Align::Center);
                rb.append(&ph);
                st.borrow_mut().selected = -1;
            }
        });
    }

    // ── Double-click/activate opens editor (port of OnStationeryActivated) ──
    {
        let btn_e = btn_edit.clone();
        list_box.connect_row_activated(move |_, _row| {
            // Activation is equivalent to clicking Edit — editor is already shown
            // on selection, so this is a no-op (selection handler covers it).
            btn_e.emit_clicked();
        });
    }

    // ── New button (port of NewStationeryFile) ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let cp = count_pill.clone();
        let sd = stny_dir.clone();
        btn_new.connect_clicked(move |_| {
            // Generate unique name
            let s = st.borrow();
            let base = "Untitled";
            let mut name = base.to_string();
            let mut counter = 1u32;
            loop {
                if !s.templates.iter().any(|t| t == &name) {
                    break;
                }
                counter += 1;
                name = format!("{} {}", base, counter);
            }
            drop(s);

            // Create blank template file
            let file_path = sd.join(&name);
            let content = "To: \nSubject: \n\n";
            if let Err(e) = fs::write(&file_path, content) {
                eprintln!("rMail: failed to create stationery: {}", e);
                return;
            }

            // Reload
            st.borrow_mut().reload();
            let s = st.borrow();
            fill_list(&lb, &s);
            cp.set_text(&format!("{} templates", s.templates.len()));

            // Select the new entry
            if let Some(idx) = s.templates.iter().position(|t| t == &name) {
                if let Some(row) = lb.row_at_index(idx as i32) {
                    lb.select_row(Some(&row));
                }
            }
        });
    }

    // ── Remove button (port of DeleteStationery) ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let cp = count_pill.clone();
        let sd = stny_dir.clone();
        let rb = right_box.clone();
        btn_remove.connect_clicked(move |_| {
            let sel = st.borrow().selected;
            if sel < 0 { return; }
            let idx = sel as usize;

            let name = {
                let s = st.borrow();
                if idx >= s.templates.len() { return; }
                s.templates[idx].clone()
            };

            // Delete the file
            let file_path = sd.join(&name);
            if let Err(e) = fs::remove_file(&file_path) {
                eprintln!("rMail: failed to remove stationery: {}", e);
                return;
            }

            // Clear editor
            while let Some(child) = rb.first_child() {
                rb.remove(&child);
            }
            let ph = gtk4::Label::new(Some("Select a template to edit, or click New to create one."));
            ph.add_css_class("dim-label");
            ph.set_vexpand(true);
            ph.set_valign(gtk4::Align::Center);
            rb.append(&ph);

            // Reload
            st.borrow_mut().reload();
            let s = st.borrow();
            fill_list(&lb, &s);
            cp.set_text(&format!("{} templates", s.templates.len()));
        });
    }

    // ── Edit button — same as selection, editor already shown ──
    // The Edit button triggers activation on the selected row which
    // the selection handler already covers. Nothing extra needed.

    outer.upcast()
}
