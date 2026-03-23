//! Settings dialog — declarative schema + generic renderer.
//!
//! Port of gtk_settings.c + settings_common.c.
//! Each settings page is defined as data (SettingsPanel + SettingsField),
//! and a single generic renderer builds the GTK widgets.

pub mod panels;

use gtk4::prelude::*;
use gtk4::glib;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

// ── Schema types ──

/// One field in a settings panel.
pub enum SettingsField {
    /// Text entry with label (form_row style).
    Text {
        section: &'static str,
        key: &'static str,
        label: &'static str,
        placeholder: &'static str,
    },
    /// Spin button with prefix/suffix labels.
    Int {
        section: &'static str,
        key: &'static str,
        prefix: &'static str,
        suffix: &'static str,
        default: i32,
        min: i32,
        max: i32,
    },
    /// Checkbox.
    Bool {
        section: &'static str,
        key: &'static str,
        label: &'static str,
        default: bool,
    },
    /// Radio button group (mutually exclusive). The `keys` array has one
    /// section/key per option; reading back picks the active one.
    Radio {
        keys: &'static [(&'static str, &'static str)], // (section, key) per option
        label: &'static str,
        options: &'static [&'static str],
        default: usize,
    },
    /// Dropdown.
    Choice {
        section: &'static str,
        key: &'static str,
        label: &'static str,
        options: &'static [&'static str],
        default: usize,
    },
    /// Color picker button. Stores as "r,g,b" (0.0-1.0) in conf.
    Color {
        section: &'static str,
        key: &'static str,
        label: &'static str,
        default_rgb: (f64, f64, f64),
    },
    /// Combined text entry + color picker on one row (for label settings).
    TextWithColor {
        section: &'static str,
        text_key: &'static str,
        color_key: &'static str,
        label: &'static str,
        placeholder: &'static str,
        default_rgb: (f64, f64, f64),
    },
    /// Horizontal separator.
    Separator,
    /// Start a group box (frame with bold header).
    GroupStart { label: &'static str },
    /// End the current group box.
    GroupEnd,
}

/// A complete settings panel (one sidebar entry).
pub struct SettingsPanel {
    pub id: &'static str,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub icon: &'static str,
    pub fields: &'static [SettingsField],
}

// ── Widget registry ──
// Mirrors the C g_object_set_data pattern: stores widget refs keyed by
// "section.key" so sync_to_conf can read them back.

type WidgetMap = Rc<RefCell<HashMap<String, gtk4::Widget>>>;

fn widget_key(section: &str, key: &str) -> String {
    format!("{}.{}", section, key)
}

// ── UI helpers (port of settings_common.c) ──

/// page_title: bold title + dim subtitle
fn page_title(text: &str, subtitle: &str) -> gtk4::Widget {
    let bx = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    bx.set_margin_bottom(8);

    let lbl = gtk4::Label::new(Some(text));
    lbl.set_halign(gtk4::Align::Start);
    lbl.add_css_class("title-2");
    bx.append(&lbl);

    if !subtitle.is_empty() {
        let sub = gtk4::Label::new(Some(subtitle));
        sub.set_halign(gtk4::Align::Start);
        sub.add_css_class("dim-label");
        sub.set_wrap(true);
        bx.append(&sub);
    }
    bx.upcast()
}

/// form_row: right-aligned dim label (140px) + expanding widget
fn form_row(label_text: &str, widget: &impl IsA<gtk4::Widget>) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    row.set_margin_start(4);
    row.set_margin_end(4);
    row.set_margin_top(6);
    row.set_margin_bottom(6);

    let lbl = gtk4::Label::new(Some(label_text));
    lbl.set_halign(gtk4::Align::End);
    lbl.set_width_chars(15);
    lbl.set_size_request(140, -1);
    lbl.add_css_class("dim-label");
    row.append(&lbl);

    widget.set_hexpand(true);
    row.append(widget);
    row
}

/// group_box: frame with optional bold header (port of settings_common.c group_box)
fn group_box(title: &str) -> (gtk4::Frame, gtk4::Box) {
    let frame = gtk4::Frame::new(None);
    frame.add_css_class("boxed-list-separate");
    frame.set_margin_top(6);
    frame.set_margin_bottom(6);

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    frame.set_child(Some(&vbox));

    if !title.is_empty() {
        let header = gtk4::Label::new(Some(title));
        header.set_halign(gtk4::Align::Start);
        header.set_margin_start(8);
        header.set_margin_top(8);
        header.set_margin_bottom(4);
        header.add_css_class("dim-label");
        header.add_css_class("heading");
        vbox.append(&header);
        vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    }

    (frame, vbox)
}

/// group_check: checkbox with margins for inside a group box
fn group_check(label: &str, active: bool) -> gtk4::CheckButton {
    let cb = gtk4::CheckButton::with_label(label);
    cb.set_active(active);
    cb.set_margin_start(8);
    cb.set_margin_top(4);
    cb.set_margin_bottom(4);
    cb
}

/// spin_row: [prefix] [spin] [suffix]
fn spin_row(prefix: &str, min: f64, max: f64, val: f64, suffix: &str) -> (gtk4::Box, gtk4::SpinButton) {
    let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    hbox.set_margin_start(8);
    hbox.set_margin_top(4);
    hbox.set_margin_bottom(4);
    if !prefix.is_empty() {
        hbox.append(&gtk4::Label::new(Some(prefix)));
    }
    let spin = gtk4::SpinButton::with_range(min, max, 1.0);
    spin.set_value(val);
    hbox.append(&spin);
    if !suffix.is_empty() {
        hbox.append(&gtk4::Label::new(Some(suffix)));
    }
    (hbox, spin)
}

// ── Generic renderer ──

/// Render a SettingsPanel into a GTK widget, reading current values from conf.
/// Stores widget references in the WidgetMap for later sync.
fn render_panel(panel: &SettingsPanel, conf: &macmbx::conf::Conf, widgets: &WidgetMap) -> gtk4::Widget {
    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    page.set_margin_start(24);
    page.set_margin_end(24);
    page.set_margin_top(20);
    page.set_margin_bottom(20);

    page.append(&page_title(panel.title, panel.subtitle));

    // Track current group box (if any)
    let mut current_group: Option<(gtk4::Frame, gtk4::Box)> = None;

    // Helper macro — append widget to current group or page
    macro_rules! append_to {
        ($w:expr) => {
            if let Some((_, ref vbox)) = current_group {
                vbox.append(&$w);
            } else {
                page.append(&$w);
            }
        };
    }

    for field in panel.fields {
        match field {
            SettingsField::GroupStart { label } => {
                // Close previous group if open
                if let Some((frame, _)) = current_group.take() {
                    page.append(&frame);
                }
                current_group = Some(group_box(label));
            }
            SettingsField::GroupEnd => {
                if let Some((frame, _)) = current_group.take() {
                    page.append(&frame);
                }
            }
            SettingsField::Separator => {
                let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
                append_to!(sep);
            }
            SettingsField::Text { section, key, label, placeholder } => {
                let entry = gtk4::Entry::new();
                entry.set_placeholder_text(Some(placeholder));
                let val = conf.get(section, key).unwrap_or("");
                entry.set_text(val);
                let row = form_row(label, &entry);
                append_to!(row);
                widgets.borrow_mut().insert(widget_key(section, key), entry.upcast());
            }
            SettingsField::Int { section, key, prefix, suffix, default, min, max } => {
                let val = conf.get_int(section, key, *default as i64) as f64;
                let (row, spin) = spin_row(prefix, *min as f64, *max as f64, val, suffix);
                append_to!(row);
                widgets.borrow_mut().insert(widget_key(section, key), spin.upcast());
            }
            SettingsField::Bool { section, key, label, default } => {
                let val = conf.get_bool(section, key, *default);
                let cb = group_check(label, val);
                append_to!(cb);
                widgets.borrow_mut().insert(widget_key(section, key), cb.upcast());
            }
            SettingsField::Radio { keys, label, options, default } => {
                // Determine which is active
                let mut active = *default;
                for (i, (section, key)) in keys.iter().enumerate() {
                    if conf.get_bool(section, key, i == *default) {
                        active = i;
                        break;
                    }
                }

                let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
                let mut first: Option<gtk4::CheckButton> = None;
                for (i, opt) in options.iter().enumerate() {
                    let rb = gtk4::CheckButton::with_label(opt);
                    if let Some(ref f) = first {
                        rb.set_group(Some(f));
                    } else {
                        first = Some(rb.clone());
                    }
                    if i == active {
                        rb.set_active(true);
                    }
                    hbox.append(&rb);
                    let (section, key) = keys[i];
                    widgets.borrow_mut().insert(widget_key(section, key), rb.upcast());
                }

                if label.is_empty() {
                    let wrapper = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
                    wrapper.set_margin_start(8);
                    wrapper.set_margin_top(4);
                    wrapper.set_margin_bottom(4);
                    wrapper.append(&hbox);
                    append_to!(wrapper);
                } else {
                    let row = form_row(label, &hbox);
                    append_to!(row);
                }
            }
            SettingsField::Choice { section, key, label, options, default } => {
                let val = conf.get_int(section, key, *default as i64) as u32;
                let string_list = gtk4::StringList::new(options);
                let dd = gtk4::DropDown::from_strings(options);
                dd.set_selected(val);
                let row = form_row(label, &dd);
                append_to!(row);
                widgets.borrow_mut().insert(widget_key(section, key), dd.upcast());
            }
            SettingsField::TextWithColor { section, text_key, color_key, label, placeholder, default_rgb } => {
                let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
                // Entry
                let entry = gtk4::Entry::new();
                entry.set_placeholder_text(Some(placeholder));
                let val = conf.get(section, text_key).unwrap_or("");
                if !val.is_empty() { entry.set_text(val); }
                entry.set_hexpand(true);
                hbox.append(&entry);
                // Color button
                let (r, g, b) = if let Some(cval) = conf.get(section, color_key) {
                    let parts: Vec<f64> = cval.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                    if parts.len() >= 3 { (parts[0], parts[1], parts[2]) } else { *default_rgb }
                } else { *default_rgb };
                let rgba = gtk4::gdk::RGBA::new(r as f32, g as f32, b as f32, 1.0);
                let cd = gtk4::ColorDialog::new();
                let cbtn = gtk4::ColorDialogButton::new(Some(cd));
                cbtn.set_rgba(&rgba);
                hbox.append(&cbtn);
                let row = form_row(label, &hbox);
                append_to!(row);
                widgets.borrow_mut().insert(widget_key(section, text_key), entry.upcast());
                widgets.borrow_mut().insert(widget_key(section, color_key), cbtn.upcast());
            }
            SettingsField::Color { section, key, label, default_rgb } => {
                // Read stored color or use default
                let (r, g, b) = if let Some(val) = conf.get(section, key) {
                    let parts: Vec<f64> = val.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                    if parts.len() >= 3 { (parts[0], parts[1], parts[2]) } else { *default_rgb }
                } else { *default_rgb };
                let rgba = gtk4::gdk::RGBA::new(r as f32, g as f32, b as f32, 1.0);
                let cd = gtk4::ColorDialog::new();
                let btn = gtk4::ColorDialogButton::new(Some(cd));
                btn.set_rgba(&rgba);
                let row = form_row(label, &btn);
                append_to!(row);
                widgets.borrow_mut().insert(widget_key(section, key), btn.upcast());
            }
        }
    }

    // Close any remaining group
    if let Some((frame, _)) = current_group.take() {
        page.append(&frame);
    }

    page.upcast()
}

// ── Sync widgets back to Conf ──

fn sync_panel_to_conf(panel: &SettingsPanel, conf: &mut macmbx::conf::Conf, widgets: &WidgetMap) {
    let map = widgets.borrow();
    for field in panel.fields {
        match field {
            SettingsField::Text { section, key, .. } => {
                if let Some(w) = map.get(&widget_key(section, key)) {
                    if let Some(entry) = w.downcast_ref::<gtk4::Entry>() {
                        conf.set(section, key, &entry.text());
                    }
                }
            }
            SettingsField::Int { section, key, .. } => {
                if let Some(w) = map.get(&widget_key(section, key)) {
                    if let Some(spin) = w.downcast_ref::<gtk4::SpinButton>() {
                        conf.set_int(section, key, spin.value() as i64);
                    }
                }
            }
            SettingsField::Bool { section, key, .. } => {
                if let Some(w) = map.get(&widget_key(section, key)) {
                    if let Some(cb) = w.downcast_ref::<gtk4::CheckButton>() {
                        conf.set_bool(section, key, cb.is_active());
                    }
                }
            }
            SettingsField::Radio { keys, .. } => {
                for (i, (section, key)) in keys.iter().enumerate() {
                    if let Some(w) = map.get(&widget_key(section, key)) {
                        if let Some(cb) = w.downcast_ref::<gtk4::CheckButton>() {
                            conf.set_bool(section, key, cb.is_active());
                        }
                    }
                }
            }
            SettingsField::Choice { section, key, .. } => {
                if let Some(w) = map.get(&widget_key(section, key)) {
                    if let Some(dd) = w.downcast_ref::<gtk4::DropDown>() {
                        conf.set_int(section, key, dd.selected() as i64);
                    }
                }
            }
            SettingsField::TextWithColor { section, text_key, color_key, .. } => {
                if let Some(w) = map.get(&widget_key(section, text_key)) {
                    if let Some(entry) = w.downcast_ref::<gtk4::Entry>() {
                        conf.set(section, text_key, &entry.text());
                    }
                }
                if let Some(w) = map.get(&widget_key(section, color_key)) {
                    if let Some(btn) = w.downcast_ref::<gtk4::ColorDialogButton>() {
                        let rgba = btn.rgba();
                        conf.set(section, color_key, &format!("{},{},{}", rgba.red(), rgba.green(), rgba.blue()));
                    }
                }
            }
            SettingsField::Color { section, key, .. } => {
                if let Some(w) = map.get(&widget_key(section, key)) {
                    if let Some(btn) = w.downcast_ref::<gtk4::ColorDialogButton>() {
                        let rgba = btn.rgba();
                        conf.set(section, key, &format!("{},{},{}", rgba.red(), rgba.green(), rgba.blue()));
                    }
                }
            }
            SettingsField::Separator | SettingsField::GroupStart { .. } | SettingsField::GroupEnd => {}
        }
    }
}

// ── Dialog builder (port of create_settings_dialog in gtk_settings.c) ──

pub fn build_settings_dialog(
    parent: &gtk4::ApplicationWindow,
    conf: Rc<RefCell<macmbx::conf::Conf>>,
) {
    let all_panels = panels::all_panels();
    let widgets: WidgetMap = Rc::new(RefCell::new(HashMap::new()));

    let dialog = gtk4::Window::builder()
        .title("rMail Settings")
        .transient_for(parent)
        .default_width(780)
        .default_height(600)
        .modal(true)
        .build();

    let main_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

    let hpaned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    hpaned.set_position(190);
    hpaned.set_vexpand(true);
    hpaned.set_hexpand(true);

    // ── Sidebar ──
    let sidebar_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .build();

    let section_list = gtk4::ListBox::new();
    section_list.set_selection_mode(gtk4::SelectionMode::Single);
    section_list.add_css_class("navigation-sidebar");

    for panel in &all_panels {
        let row = gtk4::ListBoxRow::new();
        let rbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        rbox.set_margin_start(8);
        rbox.set_margin_end(8);
        rbox.set_margin_top(6);
        rbox.set_margin_bottom(6);
        rbox.append(&gtk4::Image::from_icon_name(panel.icon));
        let label = gtk4::Label::new(Some(panel.title));
        label.set_halign(gtk4::Align::Start);
        rbox.append(&label);
        row.set_child(Some(&rbox));
        section_list.append(&row);
    }

    sidebar_scroll.set_child(Some(&section_list));
    hpaned.set_start_child(Some(&sidebar_scroll));
    hpaned.set_resize_start_child(false);

    // ── Content area: one scrolled page per panel, only active one visible ──
    let content_area = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_area.set_hexpand(true);
    content_area.set_vexpand(true);

    let mut page_scrolls: Vec<gtk4::ScrolledWindow> = Vec::new();
    {
        let conf_ref = conf.borrow();
        for (i, panel) in all_panels.iter().enumerate() {
            let scroll = gtk4::ScrolledWindow::builder()
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .vscrollbar_policy(gtk4::PolicyType::Automatic)
                .vexpand(true)
                .build();

            let page_widget = render_panel(panel, &conf_ref, &widgets);
            scroll.set_child(Some(&page_widget));
            scroll.set_visible(i == 0);
            content_area.append(&scroll);
            page_scrolls.push(scroll);
        }
    }

    hpaned.set_end_child(Some(&content_area));
    hpaned.set_resize_end_child(true);
    main_vbox.append(&hpaned);

    // ── Section selection ──
    {
        let scrolls = page_scrolls.clone();
        section_list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                for (i, s) in scrolls.iter().enumerate() {
                    s.set_visible(i == idx);
                }
            }
        });
    }

    // Select first row
    if let Some(first) = section_list.row_at_index(0) {
        section_list.select_row(Some(&first));
    }

    // ── Bottom button bar ──
    main_vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    let button_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    button_bar.set_halign(gtk4::Align::End);
    button_bar.set_margin_start(12);
    button_bar.set_margin_end(12);
    button_bar.set_margin_top(10);
    button_bar.set_margin_bottom(10);

    let cancel_btn = gtk4::Button::with_label("Cancel");
    let save_btn = gtk4::Button::with_label("Save");
    save_btn.add_css_class("suggested-action");
    button_bar.append(&cancel_btn);
    button_bar.append(&save_btn);
    main_vbox.append(&button_bar);

    // Cancel
    {
        let d = dialog.clone();
        cancel_btn.connect_clicked(move |_| { d.close(); });
    }

    // Save: sync all panels back to conf, then save to disk
    {
        let d = dialog.clone();
        let wmap = widgets.clone();
        let conf_rc = conf.clone();
        save_btn.connect_clicked(move |_| {
            let panels = panels::all_panels();
            let mut c = conf_rc.borrow_mut();
            for panel in &panels {
                sync_panel_to_conf(panel, &mut c, &wmap);
            }
            if let Err(e) = c.save() {
                eprintln!("rMail: failed to save settings: {}", e);
            }
            d.close();
        });
    }

    dialog.set_child(Some(&main_vbox));
    dialog.present();
}
