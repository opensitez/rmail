//! Compose tab — message composition with formatting toolbar.
//!
//! Port of comp.c OpenComp layout.
//! Layout (top to bottom):
//!   1. Compose toolbar: Send/Queue/Save/Attach/Image/Link/Emoji/Print + toggles + dropdowns
//!   2. Formatting bar: B/I/U/Clear | Color/Font/Size | Align L/C/R | Indent/Outdent/Bullets | Quote/HR
//!   3. Header grid: To/From/Subject/Cc/Bcc/Attachments
//!   4. Body editor (reditctrl or TextView) filling remaining space

use gtk4::prelude::*;
use gtk4::glib;
use macmbx::conf::Conf;
use macmbx::mailer::Mailer;
use reditctrl::gtk_widget::EditorWidget;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

// ── CSS ──

const COMPOSE_CSS: &str = "
.comp-toolbar button { min-height: 24px; min-width: 24px; padding: 2px 6px; }
.comp-fmt-bar button { min-height: 22px; min-width: 22px; padding: 1px 4px; }
.comp-header-grid label { font-size: 13px; color: alpha(currentColor, 0.7); }
.comp-header-grid entry { min-height: 28px; }
";

fn install_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(COMPOSE_CSS);
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

/// Pre-fill data for opening an existing message in compose mode.
pub struct ComposeData {
    pub to: String,
    pub subject: String,
    pub cc: String,
    pub bcc: String,
    pub body: String,
    pub is_sent: bool,
}

impl ComposeData {
    /// Empty data for new compose with pre-filled fields.
    pub fn default_new() -> Self {
        ComposeData { to: String::new(), subject: String::new(), cc: String::new(), bcc: String::new(), body: String::new(), is_sent: false }
    }

    /// Parse from raw message bytes using crispy MIME parser.
    /// Handles QP decoding, base64, charset conversion, multipart.
    pub fn from_raw(raw: &str) -> Self {
        // Always extract headers manually — they're reliable in both
        // MIME and plain Eudora mbox format
        let to = extract_hdr(raw, "To");
        let subject = extract_hdr(raw, "Subject");
        let cc = extract_hdr(raw, "Cc");
        let bcc = extract_hdr(raw, "Bcc");

        // Try MIME parsing for body (handles QP, base64, charset)
        let body = if let Some(parsed) = crispy::msg::parse(raw.as_bytes()) {
            parsed.body_html()
                .or_else(|| parsed.body_plain())
                .unwrap_or("")
                .to_string()
        } else {
            String::new()
        };

        // If MIME parsing gave empty body, fall back to raw body after blank line
        let body = if body.is_empty() {
            find_msg_body(raw).to_string()
        } else {
            body
        };

        ComposeData { to, subject, cc, bcc, body, is_sent: false }
    }
}

fn extract_hdr(raw: &str, name: &str) -> String {
    let needle = format!("{}:", name);
    for line in raw.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() { break; }
        if line.len() > needle.len() && line[..needle.len()].eq_ignore_ascii_case(&needle) {
            return line[needle.len()..].trim_start().to_string();
        }
    }
    String::new()
}

fn find_msg_body(raw: &str) -> &str {
    if let Some(pos) = raw.find("\r\n\r\n") { return &raw[pos + 4..]; }
    if let Some(pos) = raw.find("\n\n") { return &raw[pos + 2..]; }
    raw
}

/// Build a compose tab for a new message.
pub fn build_compose_tab(conf: &Conf, mailer: Arc<Mailer>, books: Option<Rc<macmbx::nicknames::AddressBooks>>) -> gtk4::Widget {
    build_compose_tab_inner(conf, None, mailer, books)
}

/// Build a compose tab pre-filled with structured data (for reply/forward).
pub fn build_compose_tab_with_data(conf: &Conf, data: &ComposeData, mailer: Arc<Mailer>, books: Option<Rc<macmbx::nicknames::AddressBooks>>) -> gtk4::Widget {
    build_compose_tab_inner(conf, Some(data), mailer, books)
}

/// Build a compose tab pre-filled with an existing message.
pub fn build_compose_tab_with_message(conf: &Conf, raw: &str, mailer: Arc<Mailer>, books: Option<Rc<macmbx::nicknames::AddressBooks>>) -> gtk4::Widget {
    let data = ComposeData::from_raw(raw);
    build_compose_tab_inner(conf, Some(&data), mailer, books)
}

/// Inner builder — handles both new and existing messages.
fn build_compose_tab_inner(conf: &Conf, prefill: Option<&ComposeData>, mailer: Arc<Mailer>, books: Option<Rc<macmbx::nicknames::AddressBooks>>) -> gtk4::Widget {
    install_css();

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vbox.add_css_class("panel-bg");

    // ══════════════════════════════════════════════════════════
    // 1. Compose toolbar (comp.c:774-881)
    // ══════════════════════════════════════════════════════════
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.add_css_class("comp-toolbar");
    toolbar.set_margin_start(6);
    toolbar.set_margin_end(6);
    toolbar.set_margin_top(4);
    toolbar.set_margin_bottom(4);

    let tb = |icon: &str, tip: &str| -> gtk4::Button {
        let btn = gtk4::Button::from_icon_name(icon);
        btn.set_tooltip_text(Some(tip));
        btn
    };

    let send_btn = tb("mail-send-symbolic", "Send Now");
    let queue_btn = tb("mail-send-receive-symbolic", "Queue for later");
    let save_btn = tb("document-save-symbolic", "Save draft");
    let attach_btn = tb("mail-attachment-symbolic", "Attach file");
    let img_btn = tb("image-x-generic-symbolic", "Insert Image");
    let link_btn = tb("insert-link-symbolic", "Insert Link");
    let spell_btn = tb("tools-check-spelling-symbolic", "Check Spelling");

    let emoji_btn = gtk4::MenuButton::new();
    emoji_btn.set_icon_name("face-smile-symbolic");
    emoji_btn.set_tooltip_text(Some("Insert Emoticon"));

    let print_btn = tb("printer-symbolic", "Print message");

    toolbar.append(&send_btn);
    toolbar.append(&queue_btn);
    toolbar.append(&save_btn);
    toolbar.append(&attach_btn);
    toolbar.append(&img_btn);
    toolbar.append(&link_btn);
    toolbar.append(&emoji_btn);
    toolbar.append(&spell_btn);
    toolbar.append(&print_btn);

    toolbar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

    // Toggle buttons: QP, Text, Wrap, Keep, RR (comp.c:829-845)
    for (label, tip) in &[
        ("QP",   "Allow Quoted-Printable encoding"),
        ("Text", "Send attachments as plain data"),
        ("Wrap", "Word-wrap message when sent"),
        ("Keep", "Keep copy after sending"),
        ("RR",   "Request return receipt"),
    ] {
        let tb = gtk4::ToggleButton::with_label(label);
        tb.set_tooltip_text(Some(tip));
        tb.set_can_focus(false);
        toolbar.append(&tb);
    }

    toolbar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

    // Spacer
    let spacer = gtk4::Label::new(None);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);

    // Encoding dropdown (comp.c:855-860)
    let enc_dd = gtk4::DropDown::from_strings(&["MIME", "BinHex", "Uuencode"]);
    enc_dd.set_tooltip_text(Some("Attachment encoding"));
    toolbar.append(&enc_dd);

    // Priority dropdown (comp.c:863-870)
    let priority_dd = gtk4::DropDown::from_strings(&["Highest", "High", "Normal", "Low", "Lowest"]);
    priority_dd.set_selected(2); // Normal
    priority_dd.set_tooltip_text(Some("Priority"));
    toolbar.append(&priority_dd);

    // Signature dropdown (comp.c:873-880)
    let sig_dd = gtk4::DropDown::from_strings(&["None", "Standard", "Alternate"]);
    sig_dd.set_selected(1); // Standard
    sig_dd.set_tooltip_text(Some("Signature"));
    toolbar.append(&sig_dd);

    vbox.append(&toolbar);
    vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // ══════════════════════════════════════════════════════════
    // 2. Formatting toolbar (comp.c:890-1013)
    // ══════════════════════════════════════════════════════════
    let fmt_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
    fmt_bar.add_css_class("comp-fmt-bar");
    fmt_bar.set_margin_start(6);
    fmt_bar.set_margin_end(6);
    fmt_bar.set_margin_top(3);
    fmt_bar.set_margin_bottom(3);

    // B / I / U toggle buttons
    let bold_tb = gtk4::ToggleButton::new();
    bold_tb.set_icon_name("format-text-bold-symbolic");
    bold_tb.set_tooltip_text(Some("Bold (Ctrl+B)"));
    bold_tb.set_can_focus(false);
    fmt_bar.append(&bold_tb);

    let italic_tb = gtk4::ToggleButton::new();
    italic_tb.set_icon_name("format-text-italic-symbolic");
    italic_tb.set_tooltip_text(Some("Italic (Ctrl+I)"));
    italic_tb.set_can_focus(false);
    fmt_bar.append(&italic_tb);

    let underline_tb = gtk4::ToggleButton::new();
    underline_tb.set_icon_name("format-text-underline-symbolic");
    underline_tb.set_tooltip_text(Some("Underline (Ctrl+U)"));
    underline_tb.set_can_focus(false);
    fmt_bar.append(&underline_tb);

    // Clear formatting
    let clear_btn = gtk4::Button::from_icon_name("edit-clear-symbolic");
    clear_btn.set_tooltip_text(Some("Clear Formatting"));
    clear_btn.set_can_focus(false);
    fmt_bar.append(&clear_btn);

    fmt_bar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

    // Color picker
    let color_dlg = gtk4::ColorDialog::new();
    let color_btn = gtk4::ColorDialogButton::new(Some(color_dlg));
    color_btn.set_can_focus(false);
    color_btn.set_tooltip_text(Some("Text Color"));
    fmt_bar.append(&color_btn);

    // Font family dropdown (comp.c:937-944)
    let font_dd = gtk4::DropDown::from_strings(
        &["Sans", "Serif", "Monospace", "Helvetica", "Times", "Courier"]);
    font_dd.set_tooltip_text(Some("Font"));
    font_dd.set_can_focus(false);
    fmt_bar.append(&font_dd);

    // Font size dropdown (comp.c:947-953)
    let size_dd = gtk4::DropDown::from_strings(
        &["8", "10", "12", "14", "16", "18", "20", "24", "28", "36"]);
    size_dd.set_selected(2); // 12pt
    size_dd.set_tooltip_text(Some("Font Size"));
    size_dd.set_can_focus(false);
    fmt_bar.append(&size_dd);

    fmt_bar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

    // Alignment (comp.c:958-970)
    let align_left = gtk4::Button::from_icon_name("format-justify-left-symbolic");
    align_left.set_tooltip_text(Some("Align Left"));
    align_left.set_can_focus(false);
    fmt_bar.append(&align_left);

    let align_center = gtk4::Button::from_icon_name("format-justify-center-symbolic");
    align_center.set_tooltip_text(Some("Center"));
    align_center.set_can_focus(false);
    fmt_bar.append(&align_center);

    let align_right = gtk4::Button::from_icon_name("format-justify-right-symbolic");
    align_right.set_tooltip_text(Some("Align Right"));
    align_right.set_can_focus(false);
    fmt_bar.append(&align_right);

    fmt_bar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

    // Indent / Outdent / Bullets (comp.c:975-991)
    let indent_btn = gtk4::Button::from_icon_name("format-indent-more-symbolic");
    indent_btn.set_tooltip_text(Some("Indent"));
    indent_btn.set_can_focus(false);
    fmt_bar.append(&indent_btn);

    let outdent_btn = gtk4::Button::from_icon_name("format-indent-less-symbolic");
    outdent_btn.set_tooltip_text(Some("Outdent"));
    outdent_btn.set_can_focus(false);
    fmt_bar.append(&outdent_btn);

    let bullet_btn = gtk4::Button::from_icon_name("view-list-symbolic");
    bullet_btn.set_tooltip_text(Some("Bullet List"));
    bullet_btn.set_can_focus(false);
    fmt_bar.append(&bullet_btn);

    fmt_bar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

    // Quote level + Horizontal rule (comp.c:996-1013)
    let qi = gtk4::Button::from_icon_name("format-text-direction-ltr-symbolic");
    qi.set_tooltip_text(Some("Increase Quote Level"));
    qi.set_can_focus(false);
    fmt_bar.append(&qi);

    let qd = gtk4::Button::from_icon_name("format-text-direction-rtl-symbolic");
    qd.set_tooltip_text(Some("Decrease Quote Level"));
    qd.set_can_focus(false);
    fmt_bar.append(&qd);

    let hr_btn = gtk4::Button::from_icon_name("object-select-symbolic");
    hr_btn.set_tooltip_text(Some("Insert Horizontal Rule"));
    hr_btn.set_can_focus(false);
    fmt_bar.append(&hr_btn);

    vbox.append(&fmt_bar);
    vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // ══════════════════════════════════════════════════════════
    // 3. Header grid (comp.c:1098-1224)
    // ══════════════════════════════════════════════════════════
    let header_grid = gtk4::Grid::new();
    header_grid.add_css_class("comp-header-grid");
    header_grid.set_row_spacing(4);
    header_grid.set_column_spacing(8);
    header_grid.set_margin_start(8);
    header_grid.set_margin_end(8);
    header_grid.set_margin_top(6);
    header_grid.set_margin_bottom(6);

    // To:
    let to_lbl = gtk4::Label::new(Some("To:"));
    to_lbl.set_halign(gtk4::Align::End);
    let to_entry = gtk4::Entry::new();
    to_entry.set_hexpand(true);
    if let Some(d) = prefill { to_entry.set_text(&d.to); }
    header_grid.attach(&to_lbl, 0, 0, 1, 1);
    header_grid.attach(&to_entry, 1, 0, 1, 1);

    // From: dropdown populated from accounts (comp.c:1115-1171)
    let from_lbl = gtk4::Label::new(Some("From:"));
    from_lbl.set_halign(gtk4::Align::End);
    let from_items = build_from_list(conf);
    let from_dd = gtk4::DropDown::from_strings(
        &from_items.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    from_dd.set_hexpand(true);
    header_grid.attach(&from_lbl, 0, 1, 1, 1);
    header_grid.attach(&from_dd, 1, 1, 1, 1);

    // Subject:
    let subj_lbl = gtk4::Label::new(Some("Subject:"));
    subj_lbl.set_halign(gtk4::Align::End);
    let subj_entry = gtk4::Entry::new();
    subj_entry.set_hexpand(true);
    if let Some(d) = prefill { subj_entry.set_text(&d.subject); }
    header_grid.attach(&subj_lbl, 0, 2, 1, 1);
    header_grid.attach(&subj_entry, 1, 2, 1, 1);

    // Update tab title when subject changes (port of MakeCompTitle)
    subj_entry.connect_changed(|entry| {
        let text = entry.text();
        let title = if text.is_empty() {
            "Compose Message".to_string()
        } else {
            let display = if text.len() > 30 { format!("{}…", &text[..27]) } else { text.to_string() };
            format!("Compose: {}", display)
        };
        // Walk up to find the Notebook and update the tab label
        let mut w: Option<gtk4::Widget> = Some(entry.clone().upcast());
        while let Some(ref curr) = w {
            if let Some(parent) = curr.parent() {
                if let Some(nb) = parent.downcast_ref::<gtk4::Notebook>() {
                    if let Some(tab_widget) = nb.tab_label(curr) {
                        if let Some(label) = tab_widget.first_child() {
                            if let Some(lbl) = label.downcast_ref::<gtk4::Label>() {
                                lbl.set_text(&title);
                            }
                        }
                    }
                    break;
                }
                w = Some(parent);
            } else { break; }
        }
    });

    // Cc:
    let cc_lbl = gtk4::Label::new(Some("Cc:"));
    cc_lbl.set_halign(gtk4::Align::End);
    let cc_entry = gtk4::Entry::new();
    cc_entry.set_hexpand(true);
    if let Some(d) = prefill { cc_entry.set_text(&d.cc); }
    header_grid.attach(&cc_lbl, 0, 3, 1, 1);
    header_grid.attach(&cc_entry, 1, 3, 1, 1);

    // Bcc:
    let bcc_lbl = gtk4::Label::new(Some("Bcc:"));
    bcc_lbl.set_halign(gtk4::Align::End);
    let bcc_entry = gtk4::Entry::new();
    bcc_entry.set_hexpand(true);
    if let Some(d) = prefill { bcc_entry.set_text(&d.bcc); }
    header_grid.attach(&bcc_lbl, 0, 4, 1, 1);
    header_grid.attach(&bcc_entry, 1, 4, 1, 1);

    // Attach nickname autocomplete to address fields (port of comp.c:1271-1275)
    if let Some(ref books) = books {
        crate::autocomplete::attach(&to_entry, books.clone());
        crate::autocomplete::attach(&cc_entry, books.clone());
        crate::autocomplete::attach(&bcc_entry, books.clone());
    }

    // Attachments: FlowBox + add button (comp.c:1200-1224)
    let att_lbl = gtk4::Label::new(Some("Attachments:"));
    att_lbl.set_halign(gtk4::Align::End);
    att_lbl.set_valign(gtk4::Align::Start);
    header_grid.attach(&att_lbl, 0, 5, 1, 1);

    let att_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    att_box.set_hexpand(true);
    let attach_flow = gtk4::FlowBox::new();
    attach_flow.set_selection_mode(gtk4::SelectionMode::None);
    attach_flow.set_max_children_per_line(20);
    attach_flow.set_hexpand(true);
    att_box.append(&attach_flow);
    let att_flow_for_gather = attach_flow.clone();

    let add_att_btn = gtk4::Button::from_icon_name("list-add-symbolic");
    add_att_btn.add_css_class("flat");
    add_att_btn.set_tooltip_text(Some("Add attachment"));
    add_att_btn.set_valign(gtk4::Align::Start);
    att_box.append(&add_att_btn);
    header_grid.attach(&att_box, 1, 5, 1, 1);

    // Wire attach buttons (toolbar + inline)
    {
        let af = attach_flow.clone();
        let add_attach = move |btn: &gtk4::Button| {
            let dlg = gtk4::FileDialog::new();
            dlg.set_title("Attach File");
            let toplevel = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            let af2 = af.clone();
            dlg.open(toplevel.as_ref(), gtk4::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        let name = path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.to_string_lossy().to_string());
                        // Add chip with remove button
                        let chip = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
                        chip.append(&gtk4::Label::new(Some(&name)));
                        let rm = gtk4::Button::from_icon_name("window-close-symbolic");
                        rm.add_css_class("flat");
                        rm.set_can_focus(false);
                        let af3 = af2.clone();
                        let chip_ref = chip.clone();
                        rm.connect_clicked(move |_| {
                            if let Some(parent) = chip_ref.parent() {
                                af3.remove(&parent);
                            }
                        });
                        chip.append(&rm);
                        af2.append(&chip);
                    }
                }
            });
        };

        let af_clone = attach_flow.clone();
        attach_btn.connect_clicked(move |btn| {
            let dlg = gtk4::FileDialog::new();
            dlg.set_title("Attach File");
            let toplevel = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            let af2 = af_clone.clone();
            dlg.open(toplevel.as_ref(), gtk4::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        add_attach_chip(&af2, &path);
                    }
                }
            });
        });

        add_att_btn.connect_clicked(move |btn| {
            let dlg = gtk4::FileDialog::new();
            dlg.set_title("Attach File");
            let toplevel = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            let af2 = attach_flow.clone();
            dlg.open(toplevel.as_ref(), gtk4::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        add_attach_chip(&af2, &path);
                    }
                }
            });
        });
    }

    // Insert Image — wired below after editor is created

    // ══════════════════════════════════════════════════════════
    // 4. Vertical paned: headers top, body bottom (comp.c:1278-1336)
    // ══════════════════════════════════════════════════════════
    let paned = gtk4::Paned::new(gtk4::Orientation::Vertical);
    paned.set_position(200);
    paned.set_vexpand(true);
    paned.set_hexpand(true);

    paned.set_start_child(Some(&header_grid));
    paned.set_resize_start_child(false);
    paned.set_shrink_start_child(false);

    // Body editor — reditctrl EditorWidget (port of gEditCtrl)
    let editor = EditorWidget::new();
    // Apply current theme colors to editor
    {
        let p = crate::current_palette();
        fn parse_color(hex: &str) -> reditctrl::render::Color {
            let hex = hex.trim_start_matches('#');
            if hex.len() >= 6 {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as f64 / 255.0;
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as f64 / 255.0;
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as f64 / 255.0;
                reditctrl::render::Color { red: r, green: g, blue: b, alpha: 1.0 }
            } else {
                reditctrl::render::Color { red: 1.0, green: 1.0, blue: 1.0, alpha: 1.0 }
            }
        }
        editor.set_theme_colors(
            parse_color(p.surface),
            parse_color(p.text),
            parse_color(p.text),   // caret = text color
            parse_color(p.accent),
        );
    }
    // Pre-fill body text if editing an existing message
    // Port of comp.c:1296-1322 — detect HTML and use insert_markup vs insert_text
    if let Some(d) = prefill {
        if !d.body.is_empty() {
            let is_html = d.body.contains("<b>") || d.body.contains("<i>")
                || d.body.contains("<img") || d.body.contains("<br>")
                || d.body.contains("<p>") || d.body.contains("<div")
                || d.body.contains("<html") || d.body.contains("<span");
            if is_html {
                editor.ctrl.insert_markup(0, &d.body);
            } else {
                editor.ctrl.set_text(&d.body, -1);
            }
        }
    }
    let editor_rc = Rc::new(editor);

    // Enable auto spell check — reads settings from conf, uses macmbx::spell,
    // results fed to reditctrl as wavy red underline marks.
    // Scan runs on a background thread — never blocks typing.
    {
        let auto_check = conf.get_bool("spell_checking", "spell_auto_check", true);
        if auto_check {
            let mut sp = macmbx::spell::Spell::new();
            sp.load_system_dict();

            // Apply settings from conf
            sp.set_ignore_caps(conf.get_bool("spell_checking", "spell_ignore_caps", false));
            sp.set_ignore_mixed_case(conf.get_bool("spell_checking", "spell_ignore_mixed_case", false));
            sp.set_ignore_digits(conf.get_bool("spell_checking", "spell_ignore_numbers", true));

            // Load user dictionary if it exists
            let user_dict = conf.path().parent()
                .map(|p| p.join("user_dictionary.txt"))
                .unwrap_or_default();
            if user_dict.exists() {
                let _ = sp.load_user_dict(&user_dict.to_string_lossy());
            }

            // Wrap in Mutex for thread safety (Spell has non-Sync callback fields)
            let sp = std::sync::Arc::new(std::sync::Mutex::new(sp));

            editor_rc.enable_auto_spell(move |text: &str| -> Vec<(i32, i32)> {
                let sp = sp.lock().unwrap();
                sp.scan(text).iter().map(|e| {
                    let char_off = text[..e.offset.min(text.len())].chars().count() as i32;
                    let char_len = e.word.chars().count() as i32;
                    (char_off, char_len)
                }).collect()
            });
        }
    }

    // Insert Image — open file dialog, insert into editor body
    {
        let ed = editor_rc.clone();
        img_btn.connect_clicked(move |btn| {
            let dlg = gtk4::FileDialog::new();
            dlg.set_title("Insert Image");
            let filter = gtk4::FileFilter::new();
            filter.add_mime_type("image/*");
            filter.set_name(Some("Images"));
            let toplevel = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            let ed2 = ed.clone();
            dlg.open(toplevel.as_ref(), gtk4::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        let url = format!("file://{}", path.display());
                        let html = format!("<img src=\"{}\">", url);
                        let pos = ed2.editor().get_caret_offset();
                        ed2.editor().insert_markup(pos, &html);
                        ed2.queue_draw();
                    }
                }
            });
        });
    }

    paned.set_end_child(Some(editor_rc.widget()));
    paned.set_resize_end_child(true);
    paned.set_shrink_end_child(false);

    // ══════════════════════════════════════════════════════════
    // 5. Wire formatting buttons to EditorCtrl (comp.c:902-1013)
    // ══════════════════════════════════════════════════════════

    // B / I / U
    {
        let e = editor_rc.clone();
        bold_tb.connect_clicked(move |_| {
            e.editor().toggle_style(true, false, false);
            e.queue_draw();
        });
    }
    {
        let e = editor_rc.clone();
        italic_tb.connect_clicked(move |_| {
            e.editor().toggle_style(false, true, false);
            e.queue_draw();
        });
    }
    {
        let e = editor_rc.clone();
        underline_tb.connect_clicked(move |_| {
            e.editor().toggle_style(false, false, true);
            e.queue_draw();
        });
    }

    // Clear formatting
    {
        let e = editor_rc.clone();
        clear_btn.connect_clicked(move |_| {
            e.editor().clear_style();
            e.queue_draw();
        });
    }

    // Font family
    {
        let e = editor_rc.clone();
        let families = ["Sans", "Serif", "Monospace", "Helvetica", "Times", "Courier"];
        font_dd.connect_selected_notify(move |dd| {
            let sel = dd.selected() as usize;
            if sel < families.len() {
                e.editor().set_font_family(families[sel]);
                e.queue_draw();
            }
        });
    }

    // Font size
    {
        let e = editor_rc.clone();
        let sizes = [8, 10, 12, 14, 16, 18, 20, 24, 28, 36];
        size_dd.connect_selected_notify(move |dd| {
            let sel = dd.selected() as usize;
            if sel < sizes.len() {
                e.editor().set_font_size(sizes[sel]);
                e.queue_draw();
            }
        });
    }

    // Indent
    {
        let e = editor_rc.clone();
        indent_btn.connect_clicked(move |_| {
            e.editor().indent(20);
            e.queue_draw();
        });
    }

    // Outdent
    {
        let e = editor_rc.clone();
        outdent_btn.connect_clicked(move |_| {
            e.editor().indent(-20);
            e.queue_draw();
        });
    }

    // Bullet
    {
        let e = editor_rc.clone();
        bullet_btn.connect_clicked(move |_| {
            e.editor().toggle_bullet();
            e.queue_draw();
        });
    }

    // Quote increase
    {
        let e = editor_rc.clone();
        qi.connect_clicked(move |_| {
            e.editor().change_quote_level(1);
            e.queue_draw();
        });
    }

    // Quote decrease
    {
        let e = editor_rc.clone();
        qd.connect_clicked(move |_| {
            e.editor().change_quote_level(-1);
            e.queue_draw();
        });
    }

    // Horizontal rule
    {
        let e = editor_rc.clone();
        hr_btn.connect_clicked(move |_| {
            e.editor().insert_hr();
            e.queue_draw();
        });
    }

    // Spell check button — scans body text for errors using macmbx::spell
    {
        let e = editor_rc.clone();
        spell_btn.connect_clicked(move |btn| {
            let text = e.editor().get_text();
            if text.is_empty() { return; }

            let mut sp = macmbx::spell::Spell::new();
            sp.load_system_dict();
            // Read settings — the button check uses fresh settings each time
            // (can't capture conf since it's borrowed at build time)
            let errors = sp.scan(&text);

            if errors.is_empty() {
                // Show "no errors" in a popover
                let pop = gtk4::Popover::new();
                pop.set_parent(btn);
                pop.set_child(Some(&gtk4::Label::new(Some("No spelling errors found"))));
                pop.popup();
                return;
            }

            // Show errors in a popover list
            let pop = gtk4::Popover::new();
            pop.set_parent(btn);
            let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
            vbox.set_margin_start(8); vbox.set_margin_end(8);
            vbox.set_margin_top(8); vbox.set_margin_bottom(8);
            vbox.append(&gtk4::Label::new(Some(&format!("{} error(s) found", errors.len()))));

            let scroll = gtk4::ScrolledWindow::builder()
                .max_content_height(200)
                .propagate_natural_height(true)
                .build();
            scroll.set_size_request(250, -1);
            let list = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
            for err in &errors {
                let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                let word_lbl = gtk4::Label::new(Some(&err.word));
                word_lbl.add_css_class("error");
                row.append(&word_lbl);
                if !err.result.suggestions.is_empty() {
                    let sug = gtk4::Label::new(Some(&format!("→ {}", err.result.suggestions.first().unwrap())));
                    sug.add_css_class("dim-label");
                    row.append(&sug);
                }
                list.append(&row);
            }
            scroll.set_child(Some(&list));
            vbox.append(&scroll);
            pop.set_child(Some(&vbox));
            pop.popup();
        });
    }

    // Emoji picker popover
    {
        let popover = gtk4::Popover::new();
        let grid = gtk4::FlowBox::new();
        grid.set_max_children_per_line(8);
        grid.set_selection_mode(gtk4::SelectionMode::None);
        let emojis = ["😀","😂","😊","😍","🤔","😢","😡","👍","👎","❤️","🔥","⭐","✅","❌","📎","📧","🎉","👋","🙏","💡","⚡","🌟","📝","🔔"];
        let e = editor_rc.clone();
        let pop = popover.clone();
        for emoji in &emojis {
            let btn = gtk4::Button::with_label(emoji);
            btn.add_css_class("flat");
            let em = emoji.to_string();
            let e2 = e.clone();
            let p2 = pop.clone();
            btn.connect_clicked(move |_| {
                e2.editor().insert_emoji(&em);
                e2.queue_draw();
                p2.popdown();
            });
            grid.append(&btn);
        }
        let scroll = gtk4::ScrolledWindow::builder()
            .child(&grid).max_content_height(200).propagate_natural_height(true).build();
        scroll.set_size_request(280, -1);
        popover.set_child(Some(&scroll));
        emoji_btn.set_popover(Some(&popover));
    }

    // Alignment
    {
        let e = editor_rc.clone();
        align_left.connect_clicked(move |_| {
            e.editor().set_alignment(reditctrl::document::Alignment::Left);
            e.queue_draw();
        });
    }
    {
        let e = editor_rc.clone();
        align_center.connect_clicked(move |_| {
            e.editor().set_alignment(reditctrl::document::Alignment::Center);
            e.queue_draw();
        });
    }
    {
        let e = editor_rc.clone();
        align_right.connect_clicked(move |_| {
            e.editor().set_alignment(reditctrl::document::Alignment::Right);
            e.queue_draw();
        });
    }

    // Color picker
    {
        let e = editor_rc.clone();
        color_btn.connect_rgba_notify(move |btn| {
            let rgba = btn.rgba();
            e.editor().set_color(&rgba);
            e.queue_draw();
        });
    }

    vbox.append(&paned);

    // Print button — use the editor's document directly (has styles + images)
    {
        let ed = editor_rc.clone();
        let subj = subj_entry.clone();
        print_btn.connect_clicked(move |btn| {
            let subj_text = subj.text();
            let title = if subj_text.is_empty() { "Compose" } else { &subj_text };
            let parent: gtk4::Widget = if let Some(root) = btn.root() {
                root.upcast()
            } else {
                btn.clone().upcast()
            };
            reditctrl::print::print_document(&parent, &ed.ctrl.doc, Some(title));
        });
    }

    // Find/Replace bar — hidden by default, toggled by ⌘H / find-replace action
    let find_bar = crate::findreplace::build_find_bar(&editor_rc);
    find_bar.set_visible(false);
    vbox.append(&find_bar);

    // ⌘F in compose toggles the find bar
    {
        let fb = find_bar.clone();
        let key_ctrl = gtk4::EventControllerKey::new();
        key_ctrl.connect_key_pressed(move |_, keyval, _, mods| {
            if mods.contains(gtk4::gdk::ModifierType::META_MASK) || mods.contains(gtk4::gdk::ModifierType::CONTROL_MASK) {
                if keyval == gtk4::gdk::Key::h || keyval == gtk4::gdk::Key::H
                    || keyval == gtk4::gdk::Key::f || keyval == gtk4::gdk::Key::F {
                    fb.set_visible(!fb.is_visible());
                    if fb.is_visible() {
                        // Focus the find entry
                        if let Some(first) = fb.first_child() {
                            if let Some(entry) = first.downcast_ref::<gtk4::Entry>() {
                                entry.grab_focus();
                            }
                        }
                    }
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        vbox.add_controller(key_ctrl);
    }

    // ══════════════════════════════════════════════════════════
    // 6. Wire Send / Queue / Save (port of comp.c CompSend/CompSave)
    // ══════════════════════════════════════════════════════════

    // Shared closure to gather message from UI
    let gather_message = {
        let to = to_entry.clone();
        let from = from_dd.clone();
        let subj = subj_entry.clone();
        let cc = cc_entry.clone();
        let bcc = bcc_entry.clone();
        let ed = editor_rc.clone();
        let att_flow = att_flow_for_gather.clone();
        Rc::new(move || -> Option<(crispy::msg::Message, String)> {
            let to_text = to.text().to_string();
            if to_text.is_empty() { return None; }

            // Get From text from dropdown
            let from_text = from.selected_item()
                .and_then(|item| item.downcast::<gtk4::StringObject>().ok())
                .map(|s| s.string().to_string())
                .unwrap_or_default();

            // Extract bare email from "Name <email>"
            let from_bare = if let Some(lt) = from_text.find('<') {
                let rest = &from_text[lt + 1..];
                rest.find('>').map(|gt| rest[..gt].to_string()).unwrap_or(from_text.clone())
            } else {
                from_text.clone()
            };

            let body_plain = ed.editor().get_text();
            let body_html_raw = ed.editor().get_html();
            // If HTML has any formatting, send both plain and HTML
            let has_html = body_html_raw.contains("<b>") || body_html_raw.contains("<i>")
                || body_html_raw.contains("<img") || body_html_raw.contains("color")
                || body_html_raw.contains("<font") || body_html_raw.contains("<u>");

            // Convert file:// image refs to inline cid: attachments
            let mut attachments = collect_attachments(&att_flow);
            let body_html = if has_html {
                Some(convert_inline_images(&body_html_raw, &mut attachments))
            } else {
                None
            };

            let msg = crispy::msg::Message {
                from: from_text.clone(),
                to: to_text,
                cc: { let s = cc.text().to_string(); if s.is_empty() { None } else { Some(s) } },
                bcc: { let s = bcc.text().to_string(); if s.is_empty() { None } else { Some(s) } },
                reply_to: None,
                subject: subj.text().to_string(),
                body_plain: Some(body_plain),
                body_html,
                attachments,
                message_id: None,
                in_reply_to: None,
                references: None,
                x_mailer: Some("rMail/1.0".to_string()),
                priority: 3,
                tz_offset: 0,
            };
            Some((msg, from_bare))
        })
    };

    // Send Now — queue + request immediate send + close tab (port of CompSend)
    {
        let gather = gather_message.clone();
        let m = mailer.clone();
        send_btn.connect_clicked(move |btn| {
            let show_err = |btn: &gtk4::Button, msg: &str| {
                if let Some(root) = btn.root() {
                    if let Ok(win) = root.downcast::<gtk4::Window>() {
                        let dlg = gtk4::AlertDialog::builder()
                            .modal(true)
                            .message("Send Failed")
                            .detail(msg)
                            .buttons(["OK"])
                            .build();
                        dlg.show(Some(&win));
                    }
                }
            };
            match gather() {
                None => show_err(btn, "Please enter a recipient in the To field."),
                Some((msg, from_bare)) => {
                    match msg.build() {
                        None => show_err(btn, "Cannot build message. Check that From and To are filled in."),
                        Some(raw) => {
                            match m.queue(raw.as_bytes(), &from_bare, &[]) {
                                Ok(_) => {
                                    crate::scheduler::request_send();
                                    close_parent_tab(btn);
                                }
                                Err(e) => show_err(btn, &format!("Failed to queue message: {}", e)),
                            }
                        }
                    }
                }
            }
        });
    }

    // Queue — save to Out as Queued, close tab (port of on_comp_queue_clicked)
    {
        let gather = gather_message.clone();
        let m = mailer.clone();
        queue_btn.connect_clicked(move |btn| {
            let show_err = |btn: &gtk4::Button, msg: &str| {
                if let Some(root) = btn.root() {
                    if let Ok(win) = root.downcast::<gtk4::Window>() {
                        let dlg = gtk4::AlertDialog::builder()
                            .modal(true).message("Queue Failed").detail(msg)
                            .buttons(["OK"]).build();
                        dlg.show(Some(&win));
                    }
                }
            };
            match gather() {
                None => show_err(btn, "Please enter a recipient in the To field."),
                Some((msg, from_bare)) => {
                    match msg.build() {
                        None => show_err(btn, "Cannot build message. Check that From and To are filled in."),
                        Some(raw) => {
                            match m.queue(raw.as_bytes(), &from_bare, &[]) {
                                Ok(_) => close_parent_tab(btn),
                                Err(e) => show_err(btn, &format!("Failed to queue: {}", e)),
                            }
                        }
                    }
                }
            }
        });
    }

    // Save draft — like original Eudora CompSave:
    // First save allocates a slot (queue), subsequent saves update in-place.
    {
        let gather = gather_message.clone();
        let m = mailer.clone();
        let draft_idx: Rc<std::cell::Cell<Option<usize>>> = Rc::new(std::cell::Cell::new(None));
        save_btn.connect_clicked(move |btn| {
            if let Some((msg, from_bare)) = gather() {
                if let Some(raw) = msg.build() {
                    let result = if let Some(idx) = draft_idx.get() {
                        // Update existing slot (like WriteComp + UpdateSum)
                        m.update_queued(idx, raw.as_bytes(), &from_bare).map(|_| idx)
                    } else {
                        // First save — allocate new slot
                        m.queue(raw.as_bytes(), &from_bare, &[])
                    };
                    match result {
                        Ok(idx) => {
                            draft_idx.set(Some(idx));
                            // Show "[Saved]" in title like original Eudora
                            if let Some(root) = btn.root() {
                                if let Ok(win) = root.downcast::<gtk4::Window>() {
                                    if let Some(title) = win.title() {
                                        let t = title.to_string();
                                        if !t.ends_with(" [Saved]") {
                                            win.set_title(Some(&format!("{} [Saved]", t)));
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => eprintln!("rMail: failed to save draft: {}", e),
                    }
                }
            }
        });
    }

    vbox.upcast()
}

/// Close the notebook tab containing this widget (port of CompSend/CompQueue closing the window).
/// Scan HTML for <img src="file://..."> and convert to cid: inline attachments.
/// Returns the modified HTML. Adds inline Attachment entries to the vec.
fn convert_inline_images(html: &str, attachments: &mut Vec<crispy::msg::Attachment>) -> String {
    let mut result = html.to_string();
    let mut cid_counter = 0u32;

    // Find all file:// image sources
    let mut search_from = 0;
    loop {
        let lower = result[search_from..].to_ascii_lowercase();
        let img_pos = match lower.find("src=\"file://") {
            Some(p) => search_from + p,
            None => break,
        };
        let url_start = img_pos + 5; // skip src="
        let url_end = match result[url_start..].find('"') {
            Some(p) => url_start + p,
            None => break,
        };
        let file_url = &result[url_start..url_end];
        let file_path = file_url.strip_prefix("file://").unwrap_or(file_url);

        // Read the file
        let path = std::path::Path::new(file_path);
        if let Ok(data) = std::fs::read(path) {
            cid_counter += 1;
            let filename = path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("image{}", cid_counter));
            let cid = format!("img{:04}@rmail", cid_counter);

            // Add as inline attachment
            attachments.push(crispy::msg::Attachment::inline_from_data(
                data, &filename, &cid,
            ));

            // Replace file:// URL with cid:
            let cid_url = format!("cid:{}", cid);
            result = format!("{}{}{}", &result[..url_start], cid_url, &result[url_end..]);
            search_from = url_start + cid_url.len() + 1;
        } else {
            search_from = url_end + 1;
        }
    }
    result
}

/// Add an attachment chip to the flow box, storing the full path in widget_name.
fn add_attach_chip(flow: &gtk4::FlowBox, path: &std::path::Path) {
    let name = path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let chip = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    // Store full path for gather_message to read
    chip.set_widget_name(&path.to_string_lossy());
    let icon = gtk4::Image::from_icon_name("mail-attachment-symbolic");
    icon.set_pixel_size(14);
    chip.append(&icon);
    chip.append(&gtk4::Label::new(Some(&name)));
    let rm = gtk4::Button::from_icon_name("window-close-symbolic");
    rm.add_css_class("flat");
    rm.set_has_frame(false);
    let flow_ref = flow.clone();
    let cr = chip.clone();
    rm.connect_clicked(move |_| {
        if let Some(p) = cr.parent() { flow_ref.remove(&p); }
    });
    chip.append(&rm);
    flow.append(&chip);
}

/// Collect attachment paths from the flow box chips.
fn collect_attachments(flow: &gtk4::FlowBox) -> Vec<crispy::msg::Attachment> {
    let mut attachments = Vec::new();
    let mut child = flow.first_child();
    while let Some(c) = child {
        // FlowBox wraps in FlowBoxChild
        if let Some(inner) = c.first_child() {
            let path_str = inner.widget_name().to_string();
            if !path_str.is_empty() && path_str != "GtkBox" {
                attachments.push(crispy::msg::Attachment {
                    path: Some(path_str),
                    ..Default::default()
                });
            }
        }
        child = c.next_sibling();
    }
    attachments
}

fn close_parent_tab(widget: &gtk4::Button) {
    // Walk up from the button to find the Notebook, then remove the page containing us
    let mut w: Option<gtk4::Widget> = Some(widget.clone().upcast());
    while let Some(ref current) = w {
        if let Some(parent) = current.parent() {
            if let Some(nb) = parent.downcast_ref::<gtk4::Notebook>() {
                if let Some(page_num) = nb.page_num(current) {
                    nb.remove_page(Some(page_num));
                    return;
                }
            }
            w = Some(parent);
        } else {
            break;
        }
    }
}

/// Build "From:" dropdown items from conf accounts.
/// Dominant personality first, then all additional accounts.
fn build_from_list(conf: &Conf) -> Vec<String> {
    let mut items = Vec::new();

    // Dominant personality from global prefs
    let email = conf.get("sending_mail", "email_address").unwrap_or("");
    let name = conf.get("sending_mail", "real_name").unwrap_or("");
    if !email.is_empty() {
        if !name.is_empty() {
            items.push(format!("{} <{}>", name, email));
        } else {
            items.push(email.to_string());
        }
    }

    // Additional accounts
    for acct in conf.get_all_accounts() {
        if !acct.enabled || acct.email.is_empty() { continue; }
        let display_name = if !acct.real_name.is_empty() { &acct.real_name } else { &acct.name };
        if !display_name.is_empty() {
            items.push(format!("{} <{}>", display_name, acct.email));
        } else {
            items.push(acct.email.clone());
        }
    }

    if items.is_empty() {
        items.push(String::new());
    }

    items
}
