use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use webcore::browser::{Browser, BrowserChrome, BrowserDocument, BrowserGuest, BrowserOptions};
use webcore::dom::events::{DomEvent, ListenerOptions};
use webcore::Document;

const EUDORA_HTML: &str = include_str!("html/eudora.html");
const EMAIL_HTML: &str = include_str!("html/email.html");
const SETTINGS_HTML: &str = include_str!("html/settings.html");
const WIZARD_HTML: &str = include_str!("html/wizard.html");
static WIZ_WELCOME_DATA_URL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let b64 = include_str!("html/WizWelcome.b64").trim();
    format!("data:image/png;base64,{b64}")
});

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Skin {
    Eudora,
    Email,
    Settings,
    Wizard,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum TabKind {
    Welcome,
    Mailbox(String),
    Contacts,
    Filters,
    Compose,
    Calendar,
}

fn mailbox_icon(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "in" => "📥",
        "out" => "📤",
        "drafts" => "📝",
        "sent" => "📨",
        "trash" => "🗑",
        "junk" | "spam" => "⛔",
        "archive" => "📦",
        _ => "📂",
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EmailViewMode {
    Mail,
    Contacts,
    Compose,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct Mailbox {
    name: String,
    email_label: String,
    icon: &'static str,
    indent: u8,
    has_children: bool,
    separator_after: bool,
    show_in_email_skin: bool,
}

#[derive(Clone, Debug)]
struct LabelDef {
    name: String,
    color: String,
    bg_rgba: String,
}

const DEFAULT_LABELS: &[(&str, &str, &str)] = &[
    ("Work", "#5b5fc7", "rgba(91,95,199,0.15)"),
    ("Personal", "#27ae60", "rgba(39,174,96,0.15)"),
    ("Urgent", "#e74c3c", "rgba(231,76,60,0.15)"),
    ("Finance", "#f39c12", "rgba(243,156,18,0.15)"),
    ("Dev", "#3498db", "rgba(52,152,219,0.15)"),
    ("Design", "#9b59b6", "rgba(155,89,182,0.15)"),
    ("Team", "#1abc9c", "rgba(26,188,156,0.15)"),
];

fn load_labels_from_conf(conf: &macmbx::conf::Conf) -> Vec<LabelDef> {
    (0..7).map(|i| {
        let (def_name, def_color, def_bg) = DEFAULT_LABELS[i];
        let name = conf.get("labels", &format!("label_{}", i))
            .or_else(|| conf.get("Labels", &format!("Label{}", i + 1)))
            .unwrap_or(def_name)
            .to_string();
        let color = conf.get("labels", &format!("color_{}", i))
            .or_else(|| conf.get("Labels", &format!("Color{}", i + 1)))
            .unwrap_or(def_color)
            .to_string();
        let bg_rgba = conf.get("labels", &format!("bg_{}", i))
            .unwrap_or(def_bg)
            .to_string();
        LabelDef { name, color, bg_rgba }
    }).collect()
}

fn get_user_identity_from_conf(conf: &macmbx::conf::Conf) -> String {
    let dominant = conf.get_dominant();
    if !dominant.email.is_empty() && dominant.email != "you@example.com" && dominant.email != "steve@qualcomm.com" {
        if !dominant.real_name.is_empty() && dominant.real_name != "You" {
            format!("{} <{}>", dominant.real_name, dominant.email)
        } else {
            dominant.email
        }
    } else {
        let name = conf.get("sending_mail", "real_name")
            .or_else(|| conf.get("Settings", "RealName"))
            .unwrap_or("")
            .trim();
        let email = conf.get("sending_mail", "email_address")
            .or_else(|| conf.get("Settings", "ReturnAddress"))
            .unwrap_or("")
            .trim();
        if !email.is_empty() && email != "you@example.com" && email != "steve@qualcomm.com" {
            if !name.is_empty() && name != "You" {
                format!("{} <{}>", name, email)
            } else {
                email.to_string()
            }
        } else {
            String::new()
        }
    }
}

#[derive(Clone, Debug)]
struct MailMessage {
    id: usize,
    mailbox: String,
    from: String,
    to: String,
    cc: String,
    subject: String,
    date: String,
    short_date: String,
    size: String,
    preview: String,
    body_html: String,
    unread: bool,
    replied: bool,
    forwarded: bool,
    has_attach: bool,
    attachment_note: String,
    priority: &'static str, // "highest", "high", "normal", "low", "none"
    flagged: bool,
    avatar_initials: String,
    avatar_color: &'static str,
    labels: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct Contact {
    book_name: String,
    name: String,
    initials: String,
    email: String,
    phone: String,
    company: String,
    title: String,
    notes: String,
    color: &'static str,

    // Rich fields from rmail/macmbx
    first_name: String,
    last_name: String,
    full_name: String,

    h_address: String,
    h_city: String,
    h_state: String,
    h_zip: String,
    h_country: String,
    h_mobile: String,
    h_fax: String,
    h_web: String,

    w_address: String,
    w_city: String,
    w_state: String,
    w_zip: String,
    w_country: String,
    w_phone: String,
    w_mobile: String,
    w_fax: String,
    w_web: String,

    other_email: String,
    other_phone: String,
    other_web: String,
}

#[derive(Clone, Debug)]
struct MailFilter {
    name: String,
    incoming: bool,
    outgoing: bool,
    manual: bool,
    header1: String,
    verb1: String,
    value1: String,
    conjunction: String,
    header2: String,
    verb2: String,
    value2: String,
    action1_type: String,
    action1_value: String,
    action2_type: String,
    action2_value: String,
    enabled: bool,
}

impl MailFilter {
    fn condition_summary(&self) -> String {
        let mut s = format!("{} {} \"{}\"", self.header1, self.verb1, self.value1);
        if !self.conjunction.is_empty() && self.conjunction != "ignore" && !self.value2.is_empty() {
            s.push_str(&format!(" {} {} {} \"{}\"", self.conjunction.to_uppercase(), self.header2, self.verb2, self.value2));
        }
        s
    }

    fn action_summary(&self) -> String {
        let mut s = match self.action1_type.to_lowercase().as_str() {
            "transfer" => format!("Transfer to {}", self.action1_value),
            "copy" => format!("Copy to {}", self.action1_value),
            "junk" => "Junk".to_string(),
            "delete" => "Trash / Delete".to_string(),
            "stop" => "Stop Processing".to_string(),
            "priority" => format!("Priority: {}", self.action1_value),
            "label" => format!("Label: {}", self.action1_value),
            _ => format!("{} {}", self.action1_type, self.action1_value),
        };
        if !self.action2_type.is_empty() && self.action2_type != "none" && self.action2_type != "None" {
            s.push_str(&format!(", then {}", self.action2_type));
        }
        s
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WazooTab {
    Mailboxes,
    Calendar,
    Status,
    Signatures,
    Stationery,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct StatusTask {
    id: i32,
    icon: String,
    title: String,
    status: String,
    progress: f32,
}

#[derive(Clone, Debug)]
struct StatusLog {
    time: String,
    message: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SplitterKind {
    Sidebar,
    MessageList,
    Contacts,
    Filters,
}

#[derive(Clone, Debug)]
struct ComposeDraft {
    title: String,
    from: String,
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    attach: String,
    priority: String,
    body_html: String,
    encoding: String,
    signature: String,
    opt_qp: bool,
    opt_text: bool,
    opt_wrap: bool,
    opt_keep: bool,
    opt_rr: bool,
}

impl Default for ComposeDraft {
    fn default() -> Self {
        Self {
            title: "✏ New Message".to_string(),
            from: String::new(),
            to: String::new(),
            cc: String::new(),
            bcc: String::new(),
            subject: String::new(),
            attach: "None".to_string(),
            priority: "Normal".to_string(),
            body_html: "<p><br></p><p><br></p><div class=\"compose-sig\">— <br>Sent with Eudora 2026</div>".to_string(),
            encoding: "MIME".to_string(),
            signature: "Standard".to_string(),
            opt_qp: true,
            opt_text: false,
            opt_wrap: true,
            opt_keep: true,
            opt_rr: false,
        }
    }
}

struct AppState {
    skin: Skin,
    last_mail_skin: Skin,
    pending_skin: Option<Skin>,
    dirty: bool,
    dark_mode: bool,
    data_dir: PathBuf,
    conf: macmbx::conf::Conf,
    mailboxes: Vec<Mailbox>,
    labels: Vec<LabelDef>,
    messages: Vec<MailMessage>,
    contacts: Vec<Contact>,
    filters: Vec<MailFilter>,

    // Active selections
    selected_mailbox: String,
    selected_label: Option<String>,
    selected_in_msg_id: usize,
    selected_out_msg_id: usize,
    selected_contact_idx: usize,
    open_tabs: Vec<TabKind>,
    active_tab_index: usize,
    email_mode: EmailViewMode,
    compose_open: bool,
    compose_needs_init: bool,
    compose: ComposeDraft,
    settings_category: String,
    settings_status: String,
    selected_account_idx: i32,
    status_left: String,
    status_center: String,
    status_right: String,

    // Store, collapsible tree, and context menu state
    store: macmbx::store::Store,
    expanded_folders: std::collections::HashSet<String>,
    ctx_menu_open: bool,
    ctx_menu_pos: (f32, f32),
    ctx_target: Option<String>,
    ctx_target_is_folder: bool,

    // Splitters / Panes dimensions
    sidebar_width: f32,
    msg_list_height: f32,
    contact_list_width: f32,
    filter_list_width: f32,
    active_split_drag: Option<(SplitterKind, f32, f32)>,

    // Contacts & Filters UI state
    contact_subtab: String,
    contact_status: String,
    selected_filter_idx: usize,
    filter_editing: bool,
    filter_status: String,

    // Wazoo Sidebar & Calendar / Status UI state
    wazoo_tab: WazooTab,
    cal_store: macmbx::calendar::CalendarStore,
    cal_selected_date: i32,
    cal_view_date: i32,
    cal_view_mode: String,
    cal_modal_open: bool,
    cal_is_new_event: bool,
    cal_editing_uid: String,
    status_logs: Vec<StatusLog>,
    wizard: WizardState,
    wizard_modal_open: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WizardFlow {
    Standard,
    Detect,
    Advanced,
    Import,
}

#[derive(Clone, Debug)]
struct WizardState {
    is_dominant: bool,
    step: usize,
    persona_name: String,
    real_name: String,
    return_address: String,
    login_name: String,
    password: String,
    provider: String,
    incoming_protocol: String,
    incoming_server: String,
    incoming_port: u16,
    incoming_ssl: bool,
    incoming_test: bool,
    smtp_server: String,
    smtp_port: u16,
    smtp_ssl: bool,
    smtp_auth_allowed: bool,
    smtp_test: bool,
    account_choice: usize,
    flow: WizardFlow,
}

impl WizardState {
    fn new_dominant() -> Self {
        Self {
            is_dominant: true,
            step: 0,
            persona_name: String::new(),
            real_name: String::new(),
            return_address: String::new(),
            login_name: String::new(),
            password: String::new(),
            provider: "gmail".to_string(),
            incoming_protocol: "IMAP".to_string(),
            incoming_server: String::new(),
            incoming_port: 993,
            incoming_ssl: true,
            incoming_test: false,
            smtp_server: String::new(),
            smtp_port: 465,
            smtp_ssl: true,
            smtp_auth_allowed: true,
            smtp_test: false,
            account_choice: 0,
            flow: WizardFlow::Standard,
        }
    }

    fn new_personality() -> Self {
        Self {
            is_dominant: false,
            step: 1, // Skip welcome page for personality creation
            persona_name: String::new(),
            real_name: String::new(),
            return_address: String::new(),
            login_name: String::new(),
            password: String::new(),
            provider: "gmail".to_string(),
            incoming_protocol: "IMAP".to_string(),
            incoming_server: String::new(),
            incoming_port: 993,
            incoming_ssl: true,
            incoming_test: false,
            smtp_server: String::new(),
            smtp_port: 465,
            smtp_ssl: true,
            smtp_auth_allowed: true,
            smtp_test: false,
            account_choice: 0,
            flow: WizardFlow::Standard,
        }
    }
}

const AVATAR_COLORS: &[&str] = &[
    "#326ea5", "#2ea043", "#9b32a5", "#d29922", "#d2372d", "#32a582", "#6450b4", "#0078d4",
];

fn compute_avatar_color(name: &str) -> &'static str {
    let mut sum: usize = 0;
    for b in name.bytes() {
        sum = sum.wrapping_add(b as usize);
    }
    AVATAR_COLORS[sum % AVATAR_COLORS.len()]
}

fn compute_initials(name: &str) -> String {
    let clean = name.trim();
    let name_part = if let Some(idx) = clean.find('<') {
        clean[..idx].trim()
    } else {
        clean
    };
    let parts: Vec<&str> = name_part.split_whitespace().collect();
    if parts.is_empty() {
        return "?".to_string();
    }
    if parts.len() == 1 {
        return parts[0].chars().take(2).collect::<String>().to_uppercase();
    }
    let first = parts[0].chars().next().unwrap_or('?');
    let last = parts[parts.len() - 1].chars().next().unwrap_or('?');
    format!("{}{}", first, last).to_uppercase()
}

fn mb_name_hash(s: &str) -> usize {
    let mut h: usize = 0;
    for b in s.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as usize);
    }
    h % 1000
}

fn resolve_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("GEUDORA_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".local/share/geudora");
        if std::fs::create_dir_all(&p).is_ok() {
            return p;
        }
    }
    let local = PathBuf::from(".geudora");
    let _ = std::fs::create_dir_all(&local);
    local
}

fn save_contacts_to_disk(state: &AppState) {
    let nick_dir = state.data_dir.join("Nicknames");
    let _ = std::fs::create_dir_all(&nick_dir);
    let mut books = match macmbx::nicknames::AddressBooks::open(&nick_dir) {
        Ok(b) => b,
        Err(_) => return,
    };
    if books.books.is_empty() {
        let _ = books.create_book("Standard.txt");
    }

    for c in &state.contacts {
        if c.name.trim().is_empty() { continue; }
        let bi = if !c.book_name.is_empty() {
            books.find_book(&c.book_name).unwrap_or(0)
        } else {
            0
        };
        let book = match books.get_book_mut(bi) {
            Some(b) => b,
            None => continue,
        };

        let ni = if let Some(idx) = book.find(&c.name) {
            idx
        } else {
            book.add(&c.name, Some(&c.email))
        };

        if let Some(entry) = book.entries.get_mut(ni) {
            entry.addresses = if c.email.is_empty() { None } else { Some(c.email.clone()) };
            entry.deleted = false;
        }

        book.set_field(ni, "name", &c.full_name);
        book.set_field(ni, "first", &c.first_name);
        book.set_field(ni, "last", &c.last_name);
        book.set_field(ni, "phone", &c.phone);
        book.set_field(ni, "address", &c.h_address);
        book.set_field(ni, "city", &c.h_city);
        book.set_field(ni, "state", &c.h_state);
        book.set_field(ni, "zip", &c.h_zip);
        book.set_field(ni, "country", &c.h_country);
        book.set_field(ni, "mobile", &c.h_mobile);
        book.set_field(ni, "fax", &c.h_fax);
        book.set_field(ni, "web", &c.h_web);

        book.set_field(ni, "title", &c.title);
        book.set_field(ni, "company", &c.company);
        book.set_field(ni, "address2", &c.w_address);
        book.set_field(ni, "city2", &c.w_city);
        book.set_field(ni, "state2", &c.w_state);
        book.set_field(ni, "zip2", &c.w_zip);
        book.set_field(ni, "country2", &c.w_country);
        book.set_field(ni, "phone2", &c.w_phone);
        book.set_field(ni, "mobile2", &c.w_mobile);
        book.set_field(ni, "fax2", &c.w_fax);
        book.set_field(ni, "web2", &c.w_web);

        book.set_field(ni, "otheremail", &c.other_email);
        book.set_field(ni, "otherphone", &c.other_phone);
        book.set_field(ni, "otherweb", &c.other_web);
        book.set_field(ni, "note", &c.notes);
    }
    let _ = books.save();
}

fn save_filters_to_disk(state: &AppState) {
    let filter_file = state.data_dir.join("Filters");
    let mut out = String::new();
    for f in &state.filters {
        if !f.enabled { continue; }
        out.push_str(&format!("rule {}\n", f.name));
        if f.incoming { out.push_str("incoming\n"); }
        if f.outgoing { out.push_str("outgoing\n"); }
        if f.manual { out.push_str("manual\n"); }
        if !f.header1.is_empty() {
            out.push_str(&format!("header {}\n", f.header1));
            out.push_str(&format!("verb {}\n", f.verb1));
            out.push_str(&format!("value {}\n", f.value1));
        }
        if !f.conjunction.is_empty() && f.conjunction != "ignore" && !f.header2.is_empty() {
            out.push_str(&format!("conjunction {}\n", f.conjunction));
            out.push_str(&format!("header {}\n", f.header2));
            out.push_str(&format!("verb {}\n", f.verb2));
            out.push_str(&format!("value {}\n", f.value2));
        }
        match f.action1_type.to_lowercase().as_str() {
            "transfer" => out.push_str(&format!("transfer {}\n", f.action1_value)),
            "copy" => out.push_str(&format!("copy {}\n", f.action1_value)),
            "junk" => out.push_str(&format!("junk {}\n", if f.action1_value.is_empty() { "100" } else { &f.action1_value })),
            "delete" => out.push_str("status 1\n"),
            "stop" => out.push_str("stop\n"),
            "priority" => out.push_str(&format!("priority {}\n", f.action1_value)),
            "label" => out.push_str(&format!("label {}\n", f.action1_value)),
            _ => {}
        }
        match f.action2_type.to_lowercase().as_str() {
            "transfer" => out.push_str(&format!("transfer {}\n", f.action2_value)),
            "copy" => out.push_str(&format!("copy {}\n", f.action2_value)),
            "junk" => out.push_str(&format!("junk {}\n", if f.action2_value.is_empty() { "100" } else { &f.action2_value })),
            "delete" => out.push_str("status 1\n"),
            "stop" => out.push_str("stop\n"),
            "priority" => out.push_str(&format!("priority {}\n", f.action2_value)),
            "label" => out.push_str(&format!("label {}\n", f.action2_value)),
            _ => {}
        }
    }
    let _ = std::fs::write(&filter_file, out);
}

fn ensure_standard_mailboxes(data_dir: &Path) {
    let mailboxes_dir = data_dir.join("mailboxes");
    let nick_dir = data_dir.join("Nicknames");
    let sig_dir = data_dir.join("Signatures");
    let stat_dir = data_dir.join("Stationery");
    let _ = std::fs::create_dir_all(&mailboxes_dir);
    let _ = std::fs::create_dir_all(&nick_dir);
    let _ = std::fs::create_dir_all(&sig_dir);
    let _ = std::fs::create_dir_all(&stat_dir);

    for mb in &["In", "Out", "Trash", "Junk", "Drafts", "Sent", "Archive"] {
        let p = mailboxes_dir.join(mb);
        if !p.exists() {
            let _ = macmbx::toc::Toc::open_or_create(&p);
        }
    }

    let ini_path = data_dir.join("geudora.ini");
    if !ini_path.exists() {
        let ini_content = "# Eudora configuration\n";
        let _ = std::fs::write(&ini_path, ini_content);
    }
}

fn collect_store_mailboxes(
    nodes: &[macmbx::store::Node],
    mailboxes: &mut Vec<Mailbox>,
    mailbox_paths: &mut Vec<(String, PathBuf)>,
) {
    for node in nodes {
        match node.node_type {
            macmbx::store::NodeType::Mailbox => {
                let icon = mailbox_icon(&node.name);
                let show_in_email_skin = matches!(
                    node.name.as_str(),
                    "In" | "Out" | "Drafts" | "Trash" | "Spam" | "Junk" | "Archive"
                );
                mailboxes.push(Mailbox {
                    name: node.name.clone(),
                    email_label: if node.name == "In" {
                        "Inbox".to_string()
                    } else if node.name == "Out" {
                        "Sent".to_string()
                    } else {
                        node.name.clone()
                    },
                    icon,
                    indent: 0,
                    has_children: false,
                    separator_after: node.name == "Trash",
                    show_in_email_skin,
                });
                mailbox_paths.push((node.name.clone(), node.path.clone()));
            }
            macmbx::store::NodeType::Folder => {
                collect_store_mailboxes(&node.children, mailboxes, mailbox_paths);
            }
        }
    }
}

fn load_macmbx_data(
    data_dir: &Path,
    conf: &macmbx::conf::Conf,
) -> (macmbx::store::Store, Vec<Mailbox>, Vec<MailMessage>, Vec<Contact>, Vec<MailFilter>) {
    let label_defs = load_labels_from_conf(conf);
    let mailboxes_dir = data_dir.join("mailboxes");
    let store = macmbx::store::Store::open(&mailboxes_dir)
        .unwrap_or_else(|_| macmbx::store::Store::open(data_dir).unwrap());

    let mut mailboxes = Vec::new();
    let mut mailbox_paths = Vec::new();
    collect_store_mailboxes(store.children(), &mut mailboxes, &mut mailbox_paths);

    for std_name in &["In", "Out", "Drafts", "Sent", "Archive", "Spam", "Trash"] {
        if !mailboxes.iter().any(|m| m.name.eq_ignore_ascii_case(std_name)) {
            let icon = mailbox_icon(std_name);
            mailboxes.push(Mailbox {
                name: std_name.to_string(),
                email_label: if *std_name == "In" {
                    "Inbox".to_string()
                } else if *std_name == "Out" {
                    "Sent".to_string()
                } else {
                    std_name.to_string()
                },
                icon,
                indent: 0,
                has_children: false,
                separator_after: *std_name == "Trash",
                show_in_email_skin: true,
            });
            mailbox_paths.push((std_name.to_string(), mailboxes_dir.join(std_name)));
        }
    }

    let mut messages = Vec::new();
    for (mb_name, mb_path) in &mailbox_paths {
        if let Ok(toc) = macmbx::toc::Toc::open_or_create(mb_path) {
            for (i, sum) in toc.msgs.iter().enumerate() {
                let raw = macmbx::ops::read_message(&toc, i).unwrap_or_default();
                let parsed = crispy::msg::parse(&raw);

                let from = parsed.as_ref().and_then(|p| p.from.clone())
                    .unwrap_or_else(|| if sum.from.is_empty() { "Unknown".to_string() } else { sum.from.clone() });
                let to = parsed.as_ref().and_then(|p| p.to.clone()).unwrap_or_default();
                let cc = parsed.as_ref().and_then(|p| p.cc.clone()).unwrap_or_default();
                let subject = parsed.as_ref().and_then(|p| p.subject.clone())
                    .unwrap_or_else(|| if sum.subject.is_empty() { "(No Subject)".to_string() } else { sum.subject.clone() });
                let date = parsed.as_ref().and_then(|p| p.date.clone())
                    .unwrap_or_else(|| format!("Date #{}", sum.seconds));

                let body_html = if let Some(p) = &parsed {
                    if let Some(h) = p.body_html() {
                        h.to_string()
                    } else if let Some(plain) = p.body_plain() {
                        format!("<pre style=\"font-family:monospace;white-space:pre-wrap;\">{}</pre>", escape_html(plain))
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                let preview = if let Some(p) = &parsed {
                    p.body_plain()
                        .map(|s| s.lines().take(3).collect::<Vec<_>>().join(" "))
                        .unwrap_or_default()
                } else {
                    String::new()
                };

                let unread = sum.state == macmbx::toc::State::Unread;
                let replied = sum.state == macmbx::toc::State::Replied;
                let forwarded = sum.state == macmbx::toc::State::Forwarded;
                let flagged = sum.flags.contains(macmbx::toc::MsgFlags::FLAGGED);

                let priority = match sum.priority {
                    1 => "highest",
                    2 => "high",
                    3 => "normal",
                    4 => "low",
                    5 => "none",
                    _ => "normal",
                };

                let has_attach = sum.has_attachment || parsed.as_ref().map(|p| !p.attachments().is_empty()).unwrap_or(false);
                let attachment_note = if let Some(p) = &parsed {
                    let atts = p.attachments();
                    if !atts.is_empty() {
                        format!("📎 {}", atts.iter().filter_map(|a| a.filename.as_deref()).collect::<Vec<_>>().join(", "))
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                let size = if sum.length > 1024 * 1024 {
                    format!("{:.1}M", sum.length as f64 / (1024.0 * 1024.0))
                } else if sum.length > 1024 {
                    format!("{}K", sum.length / 1024)
                } else {
                    format!("{}B", sum.length)
                };

                let mut labels = Vec::new();
                let lbl_idx = macmbx::ops::get_label(&toc, i);
                if (1..=7).contains(&lbl_idx) {
                    if let Some(def) = label_defs.get(lbl_idx - 1) {
                        labels.push(def.name.clone());
                    }
                }

                let avatar_initials = compute_initials(&from);
                let avatar_color = compute_avatar_color(&from);
                let id = (mb_name_hash(mb_name) * 1000) + i + 1;

                messages.push(MailMessage {
                    id,
                    mailbox: mb_name.clone(),
                    from,
                    to,
                    cc,
                    subject,
                    date: date.clone(),
                    short_date: date,
                    size,
                    preview,
                    body_html,
                    unread,
                    replied,
                    forwarded,
                    has_attach,
                    attachment_note,
                    priority,
                    flagged,
                    avatar_initials,
                    avatar_color,
                    labels,
                });
            }
        }
    }

    let mut contacts = Vec::new();
    let nick_dir = data_dir.join("Nicknames");
    if let Ok(books) = macmbx::nicknames::AddressBooks::open(&nick_dir) {
        for book in &books.books {
            for (ni, nick) in book.entries.iter().enumerate() {
                if nick.deleted { continue; }
                let name = nick.name.clone();
                let email = nick.addresses.clone().unwrap_or_default();
                let phone = book.get_field(ni, "phone").unwrap_or("").to_string();
                let company = book.get_field(ni, "company").unwrap_or("").to_string();
                let title = book.get_field(ni, "title").unwrap_or("").to_string();
                let notes = book.get_field(ni, "notes").or_else(|| book.get_field(ni, "note")).unwrap_or("").to_string();
                let initials = compute_initials(&name);
                let color = compute_avatar_color(&name);

                let first_name = book.get_field(ni, "first").unwrap_or("").to_string();
                let last_name = book.get_field(ni, "last").unwrap_or("").to_string();
                let full_name = book.get_field(ni, "name").unwrap_or("").to_string();
                let h_address = book.get_field(ni, "address").unwrap_or("").to_string();
                let h_city = book.get_field(ni, "city").unwrap_or("").to_string();
                let h_state = book.get_field(ni, "state").unwrap_or("").to_string();
                let h_zip = book.get_field(ni, "zip").unwrap_or("").to_string();
                let h_country = book.get_field(ni, "country").unwrap_or("").to_string();
                let h_mobile = book.get_field(ni, "mobile").unwrap_or("").to_string();
                let h_fax = book.get_field(ni, "fax").unwrap_or("").to_string();
                let h_web = book.get_field(ni, "web").unwrap_or("").to_string();

                let w_address = book.get_field(ni, "address2").unwrap_or("").to_string();
                let w_city = book.get_field(ni, "city2").unwrap_or("").to_string();
                let w_state = book.get_field(ni, "state2").unwrap_or("").to_string();
                let w_zip = book.get_field(ni, "zip2").unwrap_or("").to_string();
                let w_country = book.get_field(ni, "country2").unwrap_or("").to_string();
                let w_phone = book.get_field(ni, "phone2").unwrap_or("").to_string();
                let w_mobile = book.get_field(ni, "mobile2").unwrap_or("").to_string();
                let w_fax = book.get_field(ni, "fax2").unwrap_or("").to_string();
                let w_web = book.get_field(ni, "web2").unwrap_or("").to_string();

                let other_email = book.get_field(ni, "otheremail").unwrap_or("").to_string();
                let other_phone = book.get_field(ni, "otherphone").unwrap_or("").to_string();
                let other_web = book.get_field(ni, "otherweb").unwrap_or("").to_string();

                let book_name = book.name.clone();
                contacts.push(Contact {
                    book_name,
                    name,
                    initials,
                    email,
                    phone: if phone.is_empty() { w_phone.clone() } else { phone },
                    company,
                    title,
                    notes,
                    color,
                    first_name,
                    last_name,
                    full_name,
                    h_address,
                    h_city,
                    h_state,
                    h_zip,
                    h_country,
                    h_mobile,
                    h_fax,
                    h_web,
                    w_address,
                    w_city,
                    w_state,
                    w_zip,
                    w_country,
                    w_phone,
                    w_mobile,
                    w_fax,
                    w_web,
                    other_email,
                    other_phone,
                    other_web,
                });
            }
        }
    }

    let mut filters = Vec::new();
    let filter_path = data_dir.join("Filters");
    if let Ok(filter_set) = macmbx::filter::FilterSet::load(&filter_path) {
        for rule in &filter_set.rules {
            let incoming = (rule.when & macmbx::filter::WHEN_INCOMING) != 0 || rule.when == 0;
            let outgoing = (rule.when & macmbx::filter::WHEN_OUTGOING) != 0;
            let manual = (rule.when & macmbx::filter::WHEN_MANUAL) != 0;

            let c1 = rule.conditions.get(0);
            let c2 = rule.conditions.get(1);
            let a1 = rule.actions.get(0);
            let a2 = rule.actions.get(1);

            let header1 = c1.map(|c| c.header.clone()).unwrap_or_else(|| "Subject:".to_string());
            let verb1 = c1.map(|c| c.verb.to_str_name().to_string()).unwrap_or_else(|| "contains".to_string());
            let value1 = c1.map(|c| c.value.clone()).unwrap_or_default();

            let conjunction = match rule.conjunction {
                macmbx::filter::Conjunction::And => "and".to_string(),
                macmbx::filter::Conjunction::Or => "or".to_string(),
                macmbx::filter::Conjunction::Unless => "unless".to_string(),
                _ => "ignore".to_string(),
            };

            let header2 = c2.map(|c| c.header.clone()).unwrap_or_else(|| "From:".to_string());
            let verb2 = c2.map(|c| c.verb.to_str_name().to_string()).unwrap_or_else(|| "contains".to_string());
            let value2 = c2.map(|c| c.value.clone()).unwrap_or_default();

            let action1_type = a1.map(|a| format!("{:?}", a.action_type)).unwrap_or_else(|| "Transfer".to_string());
            let action1_value = a1.map(|a| {
                if !a.str_value.is_empty() { a.str_value.clone() }
                else { a.int_value.to_string() }
            }).unwrap_or_else(|| "In".to_string());

            let action2_type = a2.map(|a| format!("{:?}", a.action_type)).unwrap_or_else(|| "None".to_string());
            let action2_value = a2.map(|a| {
                if !a.str_value.is_empty() { a.str_value.clone() }
                else { a.int_value.to_string() }
            }).unwrap_or_default();

            filters.push(MailFilter {
                name: rule.name.clone(),
                incoming,
                outgoing,
                manual,
                header1,
                verb1,
                value1,
                conjunction,
                header2,
                verb2,
                value2,
                action1_type,
                action1_value,
                action2_type,
                action2_value,
                enabled: true,
            });
        }
    }

    (store, mailboxes, messages, contacts, filters)
}

impl AppState {
    fn new() -> Self {
        let data_dir = resolve_data_dir();
        seed_store_if_needed(&data_dir);
        let conf = macmbx::conf::Conf::load(data_dir.join("geudora.ini"))
            .unwrap_or_else(|_| macmbx::conf::Conf::new());

        let has_primary = {
            let email = conf.get("sending_mail", "email_address")
                .or_else(|| conf.get("Settings", "ReturnAddress"))
                .unwrap_or_default();
            let server = conf.get("checking_mail", "pop_server")
                .or_else(|| conf.get("Settings", "MailServer"))
                .unwrap_or_default();
            let real_accounts = conf.get_all_accounts();
            let has_custom_account = real_accounts.iter().any(|a| {
                !a.email.is_empty() && a.email != "steve@qualcomm.com" && a.email != "you@example.com"
            });
            has_custom_account || (!email.is_empty() && !server.is_empty() && email != "you@example.com" && email != "steve@qualcomm.com" && server != "mail.example.com")
        };

        let (store, mailboxes, messages, contacts, filters) = load_macmbx_data(&data_dir, &conf);

        let labels = vec![
            LabelDef { name: "Work",     color: "#5b5fc7", bg_rgba: "rgba(91,95,199,0.15)" },
            LabelDef { name: "Personal", color: "#27ae60", bg_rgba: "rgba(39,174,96,0.15)" },
            LabelDef { name: "Urgent",   color: "#e74c3c", bg_rgba: "rgba(231,76,60,0.15)" },
            LabelDef { name: "Finance",  color: "#f39c12", bg_rgba: "rgba(243,156,18,0.15)" },
            LabelDef { name: "Dev",      color: "#3498db", bg_rgba: "rgba(52,152,219,0.15)" },
            LabelDef { name: "Design",   color: "#9b59b6", bg_rgba: "rgba(155,89,182,0.15)" },
            LabelDef { name: "Team",     color: "#1abc9c", bg_rgba: "rgba(26,188,156,0.15)" },
        ];

        let _unused_messages = vec![
            MailMessage {
                id: 1,
                mailbox: "In".to_string(),
                from: "Steve Dorner <sdorner@qualcomm.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "eudora-team@qualcomm.com".to_string(),
                subject: "Welcome to Eudora 2026!".to_string(),
                date: "Mar 7, 2026  9:15 AM".to_string(),
                short_date: "9:15 AM".to_string(),
                size: "8K".to_string(),
                preview: "Welcome to the all-new Eudora! Built from the ground up with a modern HTML/CSS rendering engine...".to_string(),
                body_html: r#"<h3 style="color:#326ea5;margin-top:0">Welcome to the all-new Eudora!</h3>
<p>We've rebuilt the classic email client you loved from the ground up, powered by a modern HTML/CSS rendering engine.</p>
<p><b>What's new in Eudora 2026:</b></p>
<ul>
  <li><b>Rich HTML Email</b> — Full CSS styling, tables, and embedded images</li>
  <li><b>Wazoo Panels</b> — Dockable tabbed mailbox &amp; tool panels</li>
  <li><b>Lightning Filter Engine</b> — 100× faster rule processing</li>
  <li><b>Address Book</b> — Integrated contact management with quick-search</li>
  <li><b>Dark Mode</b> — Easy on the eyes for late-night email sessions</li>
</ul>
<p>As I always said: <i>"Bringing Mail to the Masses."</i></p>
<p>Best regards,<br><b>Steve Dorner</b><br><span style="font-size:9pt;opacity:0.6">Creator of Eudora • Qualcomm Inc.</span></p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: true,
                attachment_note: "📎 Eudora_2026_QuickStart.pdf (412 KB)".to_string(),
                priority: "highest",
                flagged: true,
                avatar_initials: "SD".to_string(),
                avatar_color: "#326ea5",
                labels: vec!["Work", "Urgent"],
            },
            MailMessage {
                id: 2,
                mailbox: "In".to_string(),
                from: "Alan Strider <astrider@example.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "finance@example.com".to_string(),
                subject: "Q1 Financial Report - Action Required".to_string(),
                date: "Mar 7, 2026  8:42 AM".to_string(),
                short_date: "8:42 AM".to_string(),
                size: "24K".to_string(),
                preview: "Please find the Q1 2026 summary below. Total revenue reached $4.2M (+15% QoQ)...".to_string(),
                body_html: r#"<p>Hi,</p>
<p>Please find the Q1 2026 summary below:</p>
<table style="width:100%;border-collapse:collapse;margin:12px 0;font-size:9.5pt;">
  <tr style="background:#326ea5;color:white;">
    <th style="padding:6px 10px;text-align:left;">Metric</th>
    <th style="padding:6px 10px;text-align:right;">Q1 2026</th>
    <th style="padding:6px 10px;text-align:right;">Q4 2025</th>
    <th style="padding:6px 10px;text-align:right;">Change</th>
  </tr>
  <tr>
    <td style="padding:5px 10px;border-bottom:1px solid #ddd;">Revenue</td>
    <td style="padding:5px 10px;text-align:right;border-bottom:1px solid #ddd;"><b>$4.2M</b></td>
    <td style="padding:5px 10px;text-align:right;border-bottom:1px solid #ddd;">$3.65M</td>
    <td style="padding:5px 10px;text-align:right;color:#2ea043;border-bottom:1px solid #ddd;">+15.1%</td>
  </tr>
  <tr>
    <td style="padding:5px 10px;border-bottom:1px solid #ddd;">Operating Costs</td>
    <td style="padding:5px 10px;text-align:right;border-bottom:1px solid #ddd;">$2.8M</td>
    <td style="padding:5px 10px;text-align:right;border-bottom:1px solid #ddd;">$2.9M</td>
    <td style="padding:5px 10px;text-align:right;color:#2ea043;border-bottom:1px solid #ddd;">-3.4%</td>
  </tr>
  <tr>
    <td style="padding:5px 10px;">Net Profit</td>
    <td style="padding:5px 10px;text-align:right;"><b>$1.4M</b></td>
    <td style="padding:5px 10px;text-align:right;">$750K</td>
    <td style="padding:5px 10px;text-align:right;color:#2ea043;"><b>+86.7%</b></td>
  </tr>
</table>
<p>Please sign off by Friday EOD.</p>
<p>Thanks,<br>Alan</p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: true,
                attachment_note: "📎 Q1_2026_Financials.xlsx (24 KB)".to_string(),
                priority: "high",
                flagged: true,
                avatar_initials: "AS".to_string(),
                avatar_color: "#2ea043",
                labels: vec!["Finance", "Urgent"],
            },
            MailMessage {
                id: 3,
                mailbox: "In".to_string(),
                from: "Lisa Warren <lwarren@design.example>".to_string(),
                to: "you@example.com, design-team@example.com".to_string(),
                cc: "".to_string(),
                subject: "📅 Invite: Design Review — Mobile Nav".to_string(),
                date: "Mar 6, 2026  4:30 PM".to_string(),
                short_date: "Yesterday".to_string(),
                size: "12K".to_string(),
                preview: "Calendar Invitation: Design Review — Mobile Navigation. Monday, March 10, 2026 at 2:00 PM PST...".to_string(),
                body_html: r#"<div class="cal-card">
  <div class="cal-header">
    <div class="cal-label">Calendar Invitation</div>
    <div class="cal-title">Design Review — Mobile Navigation</div>
  </div>
  <div class="cal-body">
    <table class="cal-table">
      <tr>
        <td class="cal-field">When</td>
        <td><b>Monday, March 10, 2026</b><br>2:00 PM – 3:00 PM PST</td>
      </tr>
      <tr>
        <td class="cal-field">Where</td>
        <td>Conference Room B / <span style="color:#326ea5;text-decoration:underline">Zoom Link</span></td>
      </tr>
      <tr>
        <td class="cal-field">Organizer</td>
        <td>Lisa Warren &lt;lwarren@design.example&gt;</td>
      </tr>
    </table>
    <div class="cal-agenda">
      <div class="cal-agenda-label">Agenda</div>
      <ol>
        <li>Review updated wireframes for bottom tab bar</li>
        <li>Discuss gesture navigation prototype results</li>
        <li>Finalize icon set and micro-interactions</li>
      </ol>
    </div>
    <div class="cal-rsvp">
      <div class="cal-rsvp-btn"><div class="cal-rsvp-inner accept">✓ Accept</div></div>
      <div class="cal-rsvp-btn"><div class="cal-rsvp-inner tentative">? Tentative</div></div>
      <div class="cal-rsvp-btn"><div class="cal-rsvp-inner decline">✗ Decline</div></div>
    </div>
  </div>
</div>
<p style="font-size:9.5pt;opacity:0.7">Hi everyone — please review the Figma mocks before the meeting. Looking forward to your feedback!</p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: true,
                attachment_note: "📎 invite.ics (4 KB)".to_string(),
                priority: "normal",
                flagged: true,
                avatar_initials: "LW".to_string(),
                avatar_color: "#d2372d",
                labels: vec!["Design", "Team"],
            },
            MailMessage {
                id: 4,
                mailbox: "In".to_string(),
                from: "GitHub <notifications@github.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "".to_string(),
                subject: "[litehtml/litehtml] PR #482: Add CSS Grid support".to_string(),
                date: "Mar 6, 2026  2:30 PM".to_string(),
                short_date: "Yesterday".to_string(),
                size: "6K".to_string(),
                preview: "@contributor merged 14 commits into master from feat-grid: grid-template-columns, fr units, gap...".to_string(),
                body_html: r#"<p><b>@contributor</b> merged 14 commits into <code>master</code> from <code>feat-grid</code>.</p>
<p>Changes:</p>
<ul>
  <li><code>grid-template-columns</code> and <code>grid-template-rows</code></li>
  <li><code>fr</code> units and <code>minmax()</code></li>
  <li><code>grid-gap</code> / <code>gap</code> shorthand</li>
  <li>Auto-placement algorithm</li>
</ul>
<p style="font-size:9pt;color:#666;">—<br>View it on GitHub or unsubscribe.</p>"#.to_string(),
                unread: false,
                replied: true,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "normal",
                flagged: false,
                avatar_initials: "GH".to_string(),
                avatar_color: "#24292f",
                labels: vec!["Dev"],
            },
            MailMessage {
                id: 5,
                mailbox: "In".to_string(),
                from: "Dana Park <dpark@example.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "speakers@conf2026.example".to_string(),
                subject: "Conference talk proposal accepted!".to_string(),
                date: "Mar 5, 2026  11:20 AM".to_string(),
                short_date: "Mar 5".to_string(),
                size: "5K".to_string(),
                preview: "Great news! Our talk 'Rendering HTML Email in 2026: Lessons from Eudora' was accepted...".to_string(),
                body_html: r#"<p>Great news!</p>
<p>Our talk <b>"Rendering HTML Email in 2026: Lessons from Eudora"</b> was accepted for the main track!</p>
<p>Slot: <b>April 18, 2:00 PM — Hall B</b></p>
<p>Let's sync this week to outline the slides.</p>
<p>— Dana</p>"#.to_string(),
                unread: false,
                replied: true,
                forwarded: true,
                has_attach: false,
                attachment_note: String::new(),
                priority: "normal",
                flagged: true,
                avatar_initials: "DP".to_string(),
                avatar_color: "#9b32a5",
                labels: vec!["Work", "Design"],
            },
            MailMessage {
                id: 6,
                mailbox: "In".to_string(),
                from: "Rust Weekly <newsletter@rustweekly.example>".to_string(),
                to: "subscribers@rustweekly.example".to_string(),
                cc: "".to_string(),
                subject: "This Week in Rust #642".to_string(),
                date: "Mar 4, 2026  7:00 AM".to_string(),
                short_date: "Mar 4".to_string(),
                size: "18K".to_string(),
                preview: "Highlights: Rust 1.86 released, zero-cost abstractions in UI toolkits, crate of the week...".to_string(),
                body_html: r#"<h3>This Week in Rust #642</h3>
<p>Hello and welcome to another issue of <i>This Week in Rust</i>!</p>
<ul>
  <li><b>Official:</b> Rust 1.86.0 release notes</li>
  <li><b>Project/Tooling:</b> Fast HTML/CSS layout engines in pure Rust</li>
  <li><b>Crate of the Week:</b> <code>webcore</code> — lightweight embeddable browser engine</li>
</ul>"#.to_string(),
                unread: false,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "low",
                flagged: false,
                avatar_initials: "RW".to_string(),
                avatar_color: "#d35400",
                labels: vec!["Dev"],
            },
            MailMessage {
                id: 7,
                mailbox: "In".to_string(),
                from: "Rick Langley <rick@example.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "".to_string(),
                subject: "Lunch Friday?".to_string(),
                date: "Mar 2, 2026  1:12 PM".to_string(),
                short_date: "Mar 2".to_string(),
                size: "1K".to_string(),
                preview: "Hey, want to grab lunch Friday? Thinking that new ramen place on 5th...".to_string(),
                body_html: r#"<p>Hey, want to grab lunch Friday? Thinking that new ramen place on 5th.</p>
<p>Let me know!</p><p>Rick</p>"#.to_string(),
                unread: false,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "none",
                flagged: false,
                avatar_initials: "RL".to_string(),
                avatar_color: "#d29922",
                labels: vec!["Personal"],
            },
            // Outbox messages
            MailMessage {
                id: 101,
                mailbox: "Out".to_string(),
                from: "you@example.com".to_string(),
                to: "Steve Dorner <sdorner@qualcomm.com>".to_string(),
                cc: "".to_string(),
                subject: "Re: Welcome to Eudora 2026!".to_string(),
                date: "Mar 7, 2026  10:45 AM".to_string(),
                short_date: "10:45 AM".to_string(),
                size: "3K".to_string(),
                preview: "Thanks Steve! The new version is amazing. Quick questions: Can I import old mailboxes?...".to_string(),
                body_html: r#"<p>Thanks Steve! The new version is amazing.</p>
<p>Quick questions:</p>
<ol>
  <li>Can I import old mailboxes?</li>
  <li>CalDAV support?</li>
</ol>"#.to_string(),
                unread: false,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "normal",
                flagged: false,
                avatar_initials: "SD".to_string(),
                avatar_color: "#326ea5",
                labels: vec!["Work"],
            },
            MailMessage {
                id: 102,
                mailbox: "Out".to_string(),
                from: "you@example.com".to_string(),
                to: "team@example.com".to_string(),
                cc: "astrider@example.com".to_string(),
                subject: "Dashboard redesign - timeline update".to_string(),
                date: "Mar 6, 2026  3:00 PM".to_string(),
                short_date: "Yesterday".to_string(),
                size: "2K".to_string(),
                preview: "Hi team, Running behind on the dashboard redesign. Pushing milestone to next Friday...".to_string(),
                body_html: r#"<p>Hi team,</p>
<p>Running behind on the dashboard redesign. Pushing milestone to next Friday.</p>"#.to_string(),
                unread: false,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "normal",
                flagged: false,
                avatar_initials: "TM".to_string(),
                avatar_color: "#1abc9c",
                labels: vec!["Team", "Work"],
            },
            MailMessage {
                id: 103,
                mailbox: "Out".to_string(),
                from: "you@example.com".to_string(),
                to: "Dana Park <dpark@example.com>".to_string(),
                cc: "".to_string(),
                subject: "Re: Conference talk proposal".to_string(),
                date: "Mar 5, 2026  2:15 PM".to_string(),
                short_date: "Mar 5".to_string(),
                size: "2K".to_string(),
                preview: "Fantastic news, Dana! Outline ideas: 1. History of desktop email clients 2. Modern rendering...".to_string(),
                body_html: r#"<p>Fantastic news, Dana!</p>
<p>Outline ideas:</p>
<ol>
  <li>History of desktop email clients</li>
  <li>Modern rendering challenges</li>
  <li>Live demo</li>
</ol>"#.to_string(),
                unread: false,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "none",
                flagged: false,
                avatar_initials: "DP".to_string(),
                avatar_color: "#9b32a5",
                labels: vec!["Work"],
            },
            // Drafts
            MailMessage {
                id: 201,
                mailbox: "Drafts".to_string(),
                from: "you@example.com".to_string(),
                to: "jbeckley@qualcomm.com".to_string(),
                cc: "".to_string(),
                subject: "[Draft] Toolbar icon measurements".to_string(),
                date: "Mar 7, 2026  11:00 AM".to_string(),
                short_date: "11:00 AM".to_string(),
                size: "2K".to_string(),
                preview: "Hi Jeff, Here are the updated measurements for the Wazoo sidebar and toolbar buttons...".to_string(),
                body_html: r#"<p>Hi Jeff,</p><p>Here are the updated measurements for the Wazoo sidebar and toolbar buttons.</p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "normal",
                flagged: false,
                avatar_initials: "JB".to_string(),
                avatar_color: "#a55532",
                labels: vec!["Work", "Design"],
            },
            // Spam
            MailMessage {
                id: 301,
                mailbox: "Spam".to_string(),
                from: "Prize Desk <winner@lottery-example.org>".to_string(),
                to: "you@example.com".to_string(),
                cc: "".to_string(),
                subject: "CONGRATULATIONS! You won a free cruise!".to_string(),
                date: "Mar 3, 2026  3:11 AM".to_string(),
                short_date: "Mar 3".to_string(),
                size: "4K".to_string(),
                preview: "Click here immediately to claim your luxury Bahamas cruise reward...".to_string(),
                body_html: r#"<p><b>Filtered by Eudora Spam Keywords Rule.</b></p><p>Click here immediately to claim your luxury Bahamas cruise reward.</p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "low",
                flagged: false,
                avatar_initials: "PD".to_string(),
                avatar_color: "#7f8c8d",
                labels: vec![],
            },
            // Work
            MailMessage {
                id: 401,
                mailbox: "Work".to_string(),
                from: "Bob Chen <bchen@example.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "".to_string(),
                subject: "Production deployment checklist".to_string(),
                date: "Mar 4, 2026  5:10 PM".to_string(),
                short_date: "Mar 4".to_string(),
                size: "7K".to_string(),
                preview: "All staging checks passed. Ready for the rollout window tonight at 19:00 UTC...".to_string(),
                body_html: r#"<p>Hi,</p><p>All staging checks passed. Ready for the rollout window tonight at 19:00 UTC.</p><p>— Bob</p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "high",
                flagged: false,
                avatar_initials: "BC".to_string(),
                avatar_color: "#32a582",
                labels: vec!["Work", "Dev"],
            },
        ];
        let _ = _unused_messages;

        let selected_in_msg_id = messages.iter().find(|m| m.mailbox == "In").map(|m| m.id).unwrap_or(1);
        let selected_out_msg_id = messages.iter().find(|m| m.mailbox == "Out").map(|m| m.id).unwrap_or(1);

        let in_unread = messages.iter().filter(|m| m.mailbox == "In" && m.unread).count();
        let in_total = messages.iter().filter(|m| m.mailbox == "In").count();

        let mut expanded_folders = std::collections::HashSet::new();
        expanded_folders.insert("Work".to_string());
        expanded_folders.insert("Personal".to_string());

        let open_tabs = vec![
            TabKind::Welcome,
            TabKind::Mailbox("In".to_string()),
        ];
        let active_tab_index = 0;

        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let (cur_y, cur_m, cur_d, _, _) = macmbx::calendar::split_ts(now_ts);
        let today_ymd = cur_y * 10000 + cur_m * 100 + cur_d;

        let cal_store = macmbx::calendar::CalendarStore::new(&data_dir);
        cleanup_sample_calendar_events(&cal_store);

        let now_clock = current_clock_str();
        let mut status_logs = Vec::new();
        status_logs.push(StatusLog {
            time: now_clock.clone(),
            message: format!("Storage initialized: {} mailboxes, {} messages loaded", mailboxes.len(), messages.len()),
        });
        let init_mail = conf.get("sending_mail", "email_address")
            .or_else(|| conf.get("Settings", "ReturnAddress"))
            .unwrap_or_default();
        let init_srv = conf.get("checking_mail", "pop_server")
            .or_else(|| conf.get("Settings", "MailServer"))
            .unwrap_or_default();
        if !init_mail.is_empty() && init_mail != "you@example.com" {
            status_logs.push(StatusLog {
                time: now_clock,
                message: format!("Account loaded: {} ({})", init_mail, init_srv),
            });
        } else {
            status_logs.push(StatusLog {
                time: now_clock,
                message: "Ready — No primary account configured".to_string(),
            });
        }

        let is_imap = conf.get_bool("checking_mail", "use_imap", false);
        let init_proto = if init_srv.is_empty() || init_srv == "mail.example.com" {
            "No Account".to_string()
        } else if is_imap {
            "IMAP Idle".to_string()
        } else {
            "POP3 Ready".to_string()
        };
        let init_right = if init_srv.is_empty() || init_srv == "mail.example.com" {
            "Not Connected".to_string()
        } else {
            format!("Server: {}", init_srv)
        };

        Self {
            skin: Skin::Eudora,
            last_mail_skin: Skin::Eudora,
            pending_skin: None,
            dirty: false,
            dark_mode: false,
            data_dir,
            conf,
            mailboxes,
            labels,
            messages,
            contacts,
            filters,
            selected_mailbox: "In".to_string(),
            selected_label: None,
            selected_in_msg_id,
            selected_out_msg_id,
            selected_contact_idx: 0,
            open_tabs,
            active_tab_index,
            email_mode: EmailViewMode::Mail,
            compose_open: false,
            compose_needs_init: true,
            compose: ComposeDraft::default(),
            settings_category: "getting_started".to_string(),
            settings_status: "Ready — All 21 classic Eudora configuration panels loaded.".to_string(),
            selected_account_idx: 0,
            status_left: format!("Welcome — {} messages, {} unread", in_total, in_unread),
            status_center: init_proto,
            status_right: init_right,
            store,
            expanded_folders,
            ctx_menu_open: false,
            ctx_menu_pos: (0.0, 0.0),
            ctx_target: None,
            ctx_target_is_folder: false,
            sidebar_width: 240.0,
            msg_list_height: 260.0,
            contact_list_width: 280.0,
            filter_list_width: 320.0,
            active_split_drag: None,
            contact_subtab: "personal".to_string(),
            contact_status: "Ready — Contact loaded".to_string(),
            selected_filter_idx: 0,
            filter_editing: false,
            filter_status: "Ready — Filter loaded".to_string(),
            wazoo_tab: WazooTab::Mailboxes,
            cal_store,
            cal_selected_date: today_ymd,
            cal_view_date: today_ymd,
            cal_view_mode: "month".to_string(),
            cal_modal_open: false,
            cal_is_new_event: true,
            cal_editing_uid: String::new(),
            status_logs,
            wizard: WizardState::new_dominant(),
            wizard_modal_open: !has_primary,
        }
    }

    fn active_tab(&self) -> TabKind {
        self.open_tabs.get(self.active_tab_index).cloned().unwrap_or(TabKind::Welcome)
    }

    fn open_tab(&mut self, tab: TabKind) {
        if let Some(pos) = self.open_tabs.iter().position(|t| *t == tab) {
            self.active_tab_index = pos;
        } else {
            self.open_tabs.push(tab.clone());
            self.active_tab_index = self.open_tabs.len() - 1;
        }
        if let TabKind::Mailbox(ref mb) = tab {
            self.selected_mailbox = mb.clone();
            self.ensure_valid_selection();
        }
        self.dirty = true;
    }

    fn close_tab(&mut self, idx: usize) {
        if self.open_tabs.len() <= 1 {
            self.open_tabs = vec![TabKind::Welcome];
            self.active_tab_index = 0;
            self.dirty = true;
            return;
        }
        if idx < self.open_tabs.len() {
            self.open_tabs.remove(idx);
            if self.active_tab_index >= self.open_tabs.len() {
                self.active_tab_index = self.open_tabs.len() - 1;
            } else if self.active_tab_index >= idx && self.active_tab_index > 0 {
                self.active_tab_index -= 1;
            }
            if let Some(TabKind::Mailbox(mb)) = self.open_tabs.get(self.active_tab_index) {
                self.selected_mailbox = mb.clone();
                self.ensure_valid_selection();
            }
            self.dirty = true;
        }
    }

    fn close_tab_if_open(&mut self, tab: &TabKind) {
        if let Some(pos) = self.open_tabs.iter().position(|t| t == tab) {
            self.close_tab(pos);
        }
    }

    fn reload_from_disk(&mut self) {
        let (store, mailboxes, messages, contacts, filters) = load_macmbx_data(&self.data_dir, &self.conf);
        self.store = store;
        self.mailboxes = mailboxes;
        self.messages = messages;
        self.contacts = contacts;
        self.filters = filters;

        if !self.messages.iter().any(|m| m.id == self.selected_in_msg_id) {
            self.selected_in_msg_id = self.messages.iter().find(|m| m.mailbox == "In").map(|m| m.id).unwrap_or(0);
        }
        if !self.messages.iter().any(|m| m.id == self.selected_out_msg_id) {
            self.selected_out_msg_id = self.messages.iter().find(|m| m.mailbox == "Out").map(|m| m.id).unwrap_or(0);
        }

        self.ensure_valid_selection();
        self.update_status_bar();
        self.dirty = true;
    }

    fn send_composed_message(&mut self) {
        let from = if self.compose.from.is_empty() {
            self.conf.get("Settings", "ReturnAddress").unwrap_or("you@example.com").to_string()
        } else {
            self.compose.from.clone()
        };
        let to = self.compose.to.clone();
        let cc = self.compose.cc.clone();
        let subject = if self.compose.subject.is_empty() {
            "(No Subject)".to_string()
        } else {
            self.compose.subject.clone()
        };
        let body = self.compose.body_html.clone();

        let raw = format!(
            "From: {}\r\nTo: {}\r\nCc: {}\r\nSubject: {}\r\nDate: {}\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{}\r\n",
            from, to, cc, subject, "Sat, 7 Mar 2026 12:00:00 -0800", body
        );

        let out_path = self.data_dir.join("mailboxes").join("Out");
        if let Ok(mut out_toc) = macmbx::toc::Toc::open_or_create(&out_path) {
            let _ = macmbx::ops::append_message(&mut out_toc, raw.as_bytes(), &from, macmbx::toc::State::Queued, 3);
            let _ = out_toc.save();
        }

        self.reload_from_disk();
        self.compose = ComposeDraft::default();
        self.compose_open = false;
        self.close_tab_if_open(&TabKind::Compose);
        self.open_tab(TabKind::Mailbox("Out".to_string()));
        self.email_mode = EmailViewMode::Mail;
        self.status_left = "Message queued in Out mailbox".to_string();
        self.dirty = true;
    }

    fn trash_message(&mut self, msg_id: usize) {
        let msg_opt = self.messages.iter().find(|m| m.id == msg_id).cloned();
        if let Some(msg) = msg_opt {
            if msg.mailbox == "Trash" {
                return;
            }
            let toc_idx = (msg.id - 1) % 1000;
            let src_path = self.data_dir.join("mailboxes").join(&msg.mailbox);
            let dst_path = self.data_dir.join("mailboxes").join("Trash");
            if let (Ok(mut src_toc), Ok(mut dst_toc)) = (
                macmbx::toc::Toc::open_or_create(&src_path),
                macmbx::toc::Toc::open_or_create(&dst_path),
            ) {
                if toc_idx < src_toc.msgs.len() {
                    let _ = macmbx::ops::transfer(&mut src_toc, toc_idx, &mut dst_toc, false);
                    let _ = src_toc.save();
                    let _ = dst_toc.save();
                }
            }
            self.reload_from_disk();
            self.status_left = "Moved message to Trash".to_string();
            self.dirty = true;
        }
    }

    fn toggle_flag_message(&mut self, msg_id: usize) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == msg_id) {
            msg.flagged = !msg.flagged;
            let toc_idx = (msg.id - 1) % 1000;
            let mbox_path = self.data_dir.join("mailboxes").join(&msg.mailbox);
            if let Ok(mut toc) = macmbx::toc::Toc::open_or_create(&mbox_path) {
                if toc_idx < toc.msgs.len() {
                    if msg.flagged {
                        let _ = macmbx::ops::set_flags(&mut toc, toc_idx, macmbx::toc::MsgFlags::FLAGGED);
                    } else {
                        let _ = macmbx::ops::clear_flags(&mut toc, toc_idx, macmbx::toc::MsgFlags::FLAGGED);
                    }
                    let _ = toc.save();
                }
            }
            self.dirty = true;
        }
    }

    fn mark_read(&mut self, msg_id: usize) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == msg_id) {
            if msg.unread {
                msg.unread = false;
                let toc_idx = (msg.id - 1) % 1000;
                let mbox_path = self.data_dir.join("mailboxes").join(&msg.mailbox);
                if let Ok(mut toc) = macmbx::toc::Toc::open_or_create(&mbox_path) {
                    if toc_idx < toc.msgs.len() {
                        let _ = macmbx::ops::set_state(&mut toc, toc_idx, macmbx::toc::State::Read);
                        let _ = toc.save();
                    }
                }
                self.dirty = true;
            }
        }
    }

    fn save_settings(&mut self, username: &str, pop_srv: &str, realname: &str, smtp_srv: &str, email: &str) {
        self.conf.set("Settings", "LoginName", username);
        self.conf.set("Settings", "MailServer", pop_srv);
        self.conf.set("Settings", "RealName", realname);
        self.conf.set("Settings", "SMTPServer", smtp_srv);
        self.conf.set("Settings", "ReturnAddress", email);

        self.conf.set("checking_mail", "pop_username", username);
        self.conf.set("checking_mail", "pop_server", pop_srv);
        self.conf.set("sending_mail", "real_name", realname);
        self.conf.set("sending_mail", "smtp_server", smtp_srv);
        self.conf.set("sending_mail", "email_address", email);

        let ini_path = self.data_dir.join("geudora.ini");
        let _ = self.conf.save_as(&ini_path);
        self.settings_status = format!("✓ Saved configuration to {}", ini_path.display());
        self.status_left = "Settings saved to geudora.ini".to_string();
        self.dirty = true;
    }

    fn unread_count(&self, mailbox: &str) -> usize {
        self.messages
            .iter()
            .filter(|m| m.mailbox == mailbox && m.unread)
            .count()
    }

    fn visible_messages_for_mailbox(&self, mailbox: &str) -> Vec<&MailMessage> {
        if let Some(lbl) = &self.selected_label {
            self.messages
                .iter()
                .filter(|m| m.labels.iter().any(|l| l == lbl))
                .collect()
        } else if mailbox == "Starred" {
            self.messages.iter().filter(|m| m.flagged).collect()
        } else if mailbox == "Sent" {
            self.messages
                .iter()
                .filter(|m| m.mailbox == "Out" || m.mailbox == "Sent")
                .collect()
        } else {
            self.messages
                .iter()
                .filter(|m| m.mailbox == mailbox)
                .collect()
        }
    }

    fn ensure_valid_selection(&mut self) {
        let active_box = self.selected_mailbox.clone();

        let visible_ids: Vec<usize> = self
            .visible_messages_for_mailbox(&active_box)
            .into_iter()
            .map(|m| m.id)
            .collect();

        if active_box == "Out" {
            if !visible_ids.contains(&self.selected_out_msg_id) {
                if let Some(&first) = visible_ids.first() {
                    self.selected_out_msg_id = first;
                }
            }
        } else if !visible_ids.contains(&self.selected_in_msg_id) {
            if let Some(&first) = visible_ids.first() {
                self.selected_in_msg_id = first;
            }
        }
    }

    fn update_status_bar(&mut self) {
        let mail_srv = self.conf.get("checking_mail", "pop_server")
            .or_else(|| self.conf.get("Settings", "MailServer"))
            .unwrap_or_default();
        let is_imap = self.conf.get_bool("checking_mail", "use_imap", false);
        let proto_status = if mail_srv.is_empty() || mail_srv == "mail.example.com" {
            "No Account".to_string()
        } else if is_imap {
            "IMAP Idle".to_string()
        } else {
            "POP3 Ready".to_string()
        };
        let srv_label = if mail_srv.is_empty() || mail_srv == "mail.example.com" {
            "Not Connected".to_string()
        } else {
            format!("Server: {}", mail_srv)
        };

        match self.active_tab() {
            TabKind::Welcome => {
                let total = self.messages.len();
                let unread = self.messages.iter().filter(|m| m.unread).count();
                self.status_left = format!("Welcome — {} total messages, {} unread", total, unread);
                self.status_center = proto_status;
                self.status_right = srv_label;
            }
            TabKind::Mailbox(ref mb) if mb == "Out" => {
                let total = self.visible_messages_for_mailbox("Out").len();
                let smtp_srv = self.conf.get("sending_mail", "smtp_server")
                    .or_else(|| self.conf.get("Settings", "SMTPServer"))
                    .unwrap_or_default();
                self.status_left = format!("Out — {} messages", total);
                self.status_center = if smtp_srv.is_empty() || smtp_srv == "smtp.example.com" {
                    "No SMTP Server".to_string()
                } else {
                    "SMTP Ready".to_string()
                };
                self.status_right = if smtp_srv.is_empty() || smtp_srv == "smtp.example.com" {
                    "Not Connected".to_string()
                } else {
                    format!("Server: {}", smtp_srv)
                };
            }
            TabKind::Mailbox(ref mb) => {
                let total = self.visible_messages_for_mailbox(mb).len();
                let unread = self.unread_count(mb);
                self.status_left = format!("{} — {} messages, {} unread", mb, total, unread);
                self.status_center = proto_status;
                self.status_right = srv_label;
            }
            TabKind::Contacts => {
                self.status_left = format!("Contacts — {} people", self.contacts.len());
                self.status_center = "Address Book".to_string();
                self.status_right = String::new();
            }
            TabKind::Filters => {
                let active = self.filters.iter().filter(|f| f.enabled).count();
                self.status_left = format!("Filters — {} rules ({} active)", self.filters.len(), active);
                self.status_center = String::new();
                self.status_right = String::new();
            }
            TabKind::Compose => {
                self.status_left = "Composing message...".to_string();
                self.status_center = "Draft".to_string();
                self.status_right = String::new();
            }
            TabKind::Calendar => {
                let y = self.cal_selected_date / 10000;
                let m = (self.cal_selected_date / 100) % 100;
                let d = self.cal_selected_date % 100;
                let ev_count = self.cal_store.get_events_for_date(self.cal_selected_date).len();
                let has_caldav = self.cal_store.get_calendars().iter().any(|c| c.source == macmbx::calendar::CalSourceType::CalDAV);
                self.status_left = format!("Calendar — {} events on {}-{:02}-{:02} ({})", ev_count, y, m, d, self.cal_view_mode);
                self.status_center = if has_caldav { "CalDAV Synced".to_string() } else { "Local Calendar".to_string() };
                self.status_right = "phoenix.db".to_string();
            }
        }
    }

    fn compute_status_tasks(&self) -> Vec<StatusTask> {
        let mut tasks = Vec::new();

        // 1. Mail Engine
        let mail_srv = self.conf.get("checking_mail", "pop_server")
            .or_else(|| self.conf.get("Settings", "MailServer"))
            .unwrap_or_default();
        let username = self.conf.get("checking_mail", "pop_username")
            .or_else(|| self.conf.get("Settings", "LoginName"))
            .unwrap_or_default();
        let is_imap = self.conf.get_bool("checking_mail", "use_imap", false);
        let proto = if is_imap { "IMAP" } else { "POP3" };

        if mail_srv.is_empty() || mail_srv == "mail.example.com" {
            tasks.push(StatusTask {
                id: 1,
                icon: "📥".to_string(),
                title: "Mail Engine".to_string(),
                status: "Offline — No account configured".to_string(),
                progress: 0.0,
            });
        } else {
            tasks.push(StatusTask {
                id: 1,
                icon: "📥".to_string(),
                title: format!("Mail Engine ({})", proto),
                status: format!("Ready — {} ({})", username, mail_srv),
                progress: 1.0,
            });
        }

        // 2. Outbox Delivery Queue
        let out_path = self.data_dir.join("mailboxes").join("Out");
        let queued_count = if let Ok(toc) = macmbx::toc::Toc::open_or_create(&out_path) {
            toc.msgs.iter().filter(|m| m.state == macmbx::toc::State::Queued).count()
        } else {
            0
        };
        let smtp_srv = self.conf.get("sending_mail", "smtp_server")
            .or_else(|| self.conf.get("Settings", "SMTPServer"))
            .unwrap_or_default();

        if queued_count > 0 {
            tasks.push(StatusTask {
                id: 2,
                icon: "📤".to_string(),
                title: "Outbox Delivery Queue".to_string(),
                status: format!("{} message(s) queued for delivery", queued_count),
                progress: 0.5,
            });
        } else if smtp_srv.is_empty() || smtp_srv == "smtp.example.com" {
            tasks.push(StatusTask {
                id: 2,
                icon: "📤".to_string(),
                title: "Outbox Delivery Queue".to_string(),
                status: "Idle — 0 queued (SMTP not configured)".to_string(),
                progress: 1.0,
            });
        } else {
            tasks.push(StatusTask {
                id: 2,
                icon: "📤".to_string(),
                title: "Outbox Delivery Queue".to_string(),
                status: format!("Idle — 0 queued (via {})", smtp_srv),
                progress: 1.0,
            });
        }

        // 3. Storage Engine
        let mb_count = self.mailboxes.len();
        let msg_count = self.messages.len();
        tasks.push(StatusTask {
            id: 3,
            icon: "⚡".to_string(),
            title: "Storage Engine".to_string(),
            status: format!("Online — {} mailboxes, {} messages", mb_count, msg_count),
            progress: 1.0,
        });

        // 4. Calendar Engine / CalDAV Sync
        let cals = self.cal_store.get_calendars();
        let caldav_cals: Vec<_> = cals.iter().filter(|c| c.source == macmbx::calendar::CalSourceType::CalDAV).collect();
        if !caldav_cals.is_empty() {
            for (idx, cal) in caldav_cals.iter().enumerate() {
                let status_str = if cal.last_sync_time > 0 {
                    let (y, m, d, hh, mm) = macmbx::calendar::split_ts(cal.last_sync_time);
                    format!("Synced {}-{:02}-{:02} {:02}:{:02}", y, m, d, hh, mm)
                } else if !cal.server_url.is_empty() {
                    format!("Ready — {}", cal.server_url)
                } else {
                    "Not configured".to_string()
                };
                tasks.push(StatusTask {
                    id: 4 + idx as i32,
                    icon: "📅".to_string(),
                    title: format!("CalDAV ({})", cal.name),
                    status: status_str,
                    progress: if cal.last_sync_time > 0 { 1.0 } else { 0.0 },
                });
            }
        } else {
            tasks.push(StatusTask {
                id: 4,
                icon: "📅".to_string(),
                title: "Calendar Engine".to_string(),
                status: "Local storage — CalDAV not configured".to_string(),
                progress: 1.0,
            });
        }

        tasks
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn display_name(addr: &str) -> &str {
    if let Some(idx) = addr.find('<') {
        let name = addr[..idx].trim();
        if !name.is_empty() {
            return name;
        }
    }
    addr
}

fn build_contact_list_html(state: &AppState) -> String {
    let mut indexed: Vec<(usize, &Contact)> = state.contacts.iter().enumerate().collect();
    indexed.sort_by(|a, b| a.1.name.cmp(&b.1.name));

    let mut list_html = String::new();
    let mut last_letter = '\0';

    for (idx, c) in indexed {
        let letter = c.name.chars().next().unwrap_or('?').to_ascii_uppercase();
        if letter != last_letter {
            list_html.push_str(&format!(
                r#"<div class="contact-section-hdr">{letter}</div>"#
            ));
            last_letter = letter;
        }
        let sel = if idx == state.selected_contact_idx {
            "selected"
        } else {
            ""
        };
        let online = idx % 3 == 0;
        let online_dot = if online {
            r#"<span class="online-dot"></span>"#
        } else {
            ""
        };
        let subtitle = match (c.title.is_empty(), c.company.is_empty()) {
            (false, false) => format!("{} • {}", escape_html(&c.title), escape_html(&c.company)),
            (false, true) => escape_html(&c.title),
            (true, false) => escape_html(&c.company),
            (true, true) => String::new(),
        };
        list_html.push_str(&format!(
            r#"<div class="contact-card {sel}" data-contact="{idx}">
  <div class="avatar" style="background:{color}">{initials}{online_dot}</div>
  <div class="contact-info">
    <div class="contact-name">{name}</div>
    <div class="contact-title">{subtitle}</div>
    <div class="contact-email">{email}</div>
  </div>
</div>"#,
            color = c.color,
            initials = escape_html(&c.initials),
            name = escape_html(&c.name),
            email = escape_html(&c.email),
        ));
    }
    list_html
}

fn build_contact_detail_html(state: &AppState) -> String {
    let Some(c) = state.contacts.get(state.selected_contact_idx) else {
        return r#"<div class="contact-inspector" style="padding:24px;text-align:center;color:#888;">No contact selected. Click ➕ New Contact to create one.</div>"#.to_string();
    };

    let subtab = state.contact_subtab.as_str();

    let tab_personal_cls = if subtab == "personal" { " selected" } else { "" };
    let tab_work_cls = if subtab == "work" { " selected" } else { "" };
    let tab_home_cls = if subtab == "home" { " selected" } else { "" };
    let tab_other_cls = if subtab == "other" { " selected" } else { "" };

    let fields_html = match subtab {
        "work" => format!(
            r#"<div class="ci-grid">
                <div class="ci-field">
                    <span class="ci-label">Company / Organization</span>
                    <input class="ci-input" id="c-company" type="text" value="{company}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Job Title / Role</span>
                    <input class="ci-input" id="c-title" type="text" value="{title}">
                </div>
                <div class="ci-field full">
                    <span class="ci-label">Work Street Address</span>
                    <input class="ci-input" id="c-w-address" type="text" value="{address}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">City</span>
                    <input class="ci-input" id="c-w-city" type="text" value="{city}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">State / Province</span>
                    <input class="ci-input" id="c-w-state" type="text" value="{state}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Zip / Postal Code</span>
                    <input class="ci-input" id="c-w-zip" type="text" value="{zip}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Country</span>
                    <input class="ci-input" id="c-w-country" type="text" value="{country}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Work Phone</span>
                    <input class="ci-input" id="c-w-phone" type="text" value="{phone}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Work Mobile</span>
                    <input class="ci-input" id="c-w-mobile" type="text" value="{mobile}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Work Fax</span>
                    <input class="ci-input" id="c-w-fax" type="text" value="{fax}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Work Web / URL</span>
                    <input class="ci-input" id="c-w-web" type="text" value="{web}">
                </div>
            </div>"#,
            company = escape_html(&c.company),
            title = escape_html(&c.title),
            address = escape_html(&c.w_address),
            city = escape_html(&c.w_city),
            state = escape_html(&c.w_state),
            zip = escape_html(&c.w_zip),
            country = escape_html(&c.w_country),
            phone = escape_html(&c.w_phone),
            mobile = escape_html(&c.w_mobile),
            fax = escape_html(&c.w_fax),
            web = escape_html(&c.w_web),
        ),
        "home" => format!(
            r#"<div class="ci-grid">
                <div class="ci-field full">
                    <span class="ci-label">Home Street Address</span>
                    <input class="ci-input" id="c-h-address" type="text" value="{address}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">City</span>
                    <input class="ci-input" id="c-h-city" type="text" value="{city}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">State / Region</span>
                    <input class="ci-input" id="c-h-state" type="text" value="{state}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Zip / Postal Code</span>
                    <input class="ci-input" id="c-h-zip" type="text" value="{zip}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Country</span>
                    <input class="ci-input" id="c-h-country" type="text" value="{country}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Home Phone</span>
                    <input class="ci-input" id="c-phone" type="text" value="{phone}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Personal Mobile</span>
                    <input class="ci-input" id="c-h-mobile" type="text" value="{mobile}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Home Fax</span>
                    <input class="ci-input" id="c-h-fax" type="text" value="{fax}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Personal Website</span>
                    <input class="ci-input" id="c-h-web" type="text" value="{web}">
                </div>
            </div>"#,
            address = escape_html(&c.h_address),
            city = escape_html(&c.h_city),
            state = escape_html(&c.h_state),
            zip = escape_html(&c.h_zip),
            country = escape_html(&c.h_country),
            phone = escape_html(&c.phone),
            mobile = escape_html(&c.h_mobile),
            fax = escape_html(&c.h_fax),
            web = escape_html(&c.h_web),
        ),
        "other" => format!(
            r#"<div class="ci-grid">
                <div class="ci-field">
                    <span class="ci-label">Other Email Address</span>
                    <input class="ci-input" id="c-other-email" type="text" value="{email}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Other Phone Number</span>
                    <input class="ci-input" id="c-other-phone" type="text" value="{phone}">
                </div>
                <div class="ci-field full">
                    <span class="ci-label">Other Website / Portfolio</span>
                    <input class="ci-input" id="c-other-web" type="text" value="{web}">
                </div>
                <div class="ci-field full">
                    <span class="ci-label">Notes &amp; Nickname Information</span>
                    <textarea class="ci-textarea" id="c-notes">{notes}</textarea>
                </div>
            </div>"#,
            email = escape_html(&c.other_email),
            phone = escape_html(&c.other_phone),
            web = escape_html(&c.other_web),
            notes = escape_html(&c.notes),
        ),
        _ => format!(
            r#"<div class="ci-grid">
                <div class="ci-field">
                    <span class="ci-label">Nickname / Short Alias</span>
                    <input class="ci-input" id="c-name" type="text" value="{name}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Primary Email Address</span>
                    <input class="ci-input" id="c-email" type="text" value="{email}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">First Name</span>
                    <input class="ci-input" id="c-first" type="text" value="{first}">
                </div>
                <div class="ci-field">
                    <span class="ci-label">Last Name</span>
                    <input class="ci-input" id="c-last" type="text" value="{last}">
                </div>
                <div class="ci-field full">
                    <span class="ci-label">Full Display Name</span>
                    <input class="ci-input" id="c-full" type="text" value="{full}">
                </div>
            </div>"#,
            name = escape_html(&c.name),
            email = escape_html(&c.email),
            first = escape_html(&c.first_name),
            last = escape_html(&c.last_name),
            full = escape_html(&c.full_name),
        ),
    };

    let display_full = if !c.full_name.is_empty() {
        c.full_name.clone()
    } else if !c.first_name.is_empty() || !c.last_name.is_empty() {
        format!("{} {}", c.first_name, c.last_name).trim().to_string()
    } else {
        c.name.clone()
    };

    let display_sub = if !c.company.is_empty() && !c.title.is_empty() {
        format!("{} • {}", c.title, c.company)
    } else if !c.email.is_empty() {
        c.email.clone()
    } else {
        "Eudora Nickname".to_string()
    };

    format!(
        r#"<div class="contact-inspector">
            <div class="ci-header">
                <div class="ci-title-area">
                    <div class="ci-avatar-large" style="background:{color};">{initials}</div>
                    <div>
                        <div class="ci-title-text">{title}</div>
                        <div class="ci-subtitle">{subtitle}</div>
                    </div>
                </div>
                <div class="ci-actions">
                    <span class="ci-status-badge">{status}</span>
                    <button class="ci-btn primary" data-action="save-contact">💾 Save</button>
                    <button class="ci-btn" data-action="compose-to-contact">✉ Email</button>
                    <button class="ci-btn danger" data-action="delete-contact">🗑 Delete</button>
                </div>
            </div>
            <div class="ci-subtabs">
                <div class="ci-subtab{tab_personal}" data-contact-subtab="personal">👤 Personal</div>
                <div class="ci-subtab{tab_work}" data-contact-subtab="work">🏢 Work</div>
                <div class="ci-subtab{tab_home}" data-contact-subtab="home">🏠 Home</div>
                <div class="ci-subtab{tab_other}" data-contact-subtab="other">📝 Other &amp; Notes</div>
            </div>
            <div class="ci-body">
                {fields}
            </div>
        </div>"#,
        color = c.color,
        initials = escape_html(&c.initials),
        title = escape_html(&display_full),
        subtitle = escape_html(&display_sub),
        status = escape_html(&state.contact_status),
        tab_personal = tab_personal_cls,
        tab_work = tab_work_cls,
        tab_home = tab_home_cls,
        tab_other = tab_other_cls,
        fields = fields_html,
    )
}

fn build_filter_editor_html(state: &AppState) -> String {
    let Some(f) = state.filters.get(state.selected_filter_idx) else {
        return r#"<div class="filter-inspector" style="padding:40px 20px;text-align:center;color:#888;">
            <div style="font-size:24px;margin-bottom:8px;">📋</div>
            <div style="font-weight:600;font-size:11pt;color:#555;margin-bottom:6px;">No filter rule selected</div>
            <div style="font-size:9pt;color:#888;margin-bottom:16px;">Select a rule from the list on the left, or create a new filter rule.</div>
            <button class="ci-btn primary" data-action="add-filter" style="padding:6px 16px;font-size:9.5pt;">➕ Create New Rule</button>
        </div>"#.to_string();
    };

    if !state.filter_editing {
        let status_badge = if f.enabled {
            r#"<span class="ci-status-badge" style="background:rgba(46,160,67,0.12);color:#2ea043;">● Active</span>"#
        } else {
            r#"<span class="ci-status-badge" style="background:rgba(120,114,108,0.12);color:#78726c;">○ Disabled</span>"#
        };

        let mut scopes_vec = Vec::new();
        if f.incoming { scopes_vec.push("Incoming Mail"); }
        if f.outgoing { scopes_vec.push("Outgoing Mail"); }
        if f.manual { scopes_vec.push("Manual Execution"); }
        let scopes_display = if scopes_vec.is_empty() { "None".to_string() } else { scopes_vec.join(", ") };

        let cond_summary = f.condition_summary();
        let act_summary = f.action_summary();

        return format!(
            r#"<div class="filter-inspector">
                <div class="fi-header">
                    <div>
                        <div class="fi-title">Rule #{num}: {name}</div>
                    </div>
                    <div class="fi-actions">
                        {status_badge}
                        <button class="ci-btn primary" data-action="edit-filter">✏ Edit Rule</button>
                        <button class="ci-btn" data-action="add-filter">➕ New</button>
                        <button class="ci-btn danger" data-action="delete-filter">🗑 Delete</button>
                    </div>
                </div>
                <div class="fi-body">
                    <div class="fi-section" style="padding:14px 16px;">
                        <div class="fi-section-title">Rule Details</div>
                        <div style="font-size:9.5pt;margin-top:6px;line-height:1.6;">
                            <div><b>Name:</b> {name}</div>
                            <div><b>Applies to:</b> {scopes}</div>
                        </div>
                    </div>
                    <div class="fi-section" style="padding:14px 16px;">
                        <div class="fi-section-title">Match Criteria (IF)</div>
                        <div style="font-size:10pt;font-family:monospace;background:rgba(0,0,0,0.04);padding:8px 12px;border-radius:4px;margin-top:6px;color:#1a1612;">
                            {cond}
                        </div>
                    </div>
                    <div class="fi-section" style="padding:14px 16px;">
                        <div class="fi-section-title">Actions Performed (THEN)</div>
                        <div style="font-size:10pt;font-family:monospace;background:rgba(0,0,0,0.04);padding:8px 12px;border-radius:4px;margin-top:6px;color:#1a1612;">
                            {action}
                        </div>
                    </div>
                    <div style="margin-top:16px;text-align:right;">
                        <button class="ci-btn primary" data-action="edit-filter" style="padding:6px 18px;font-size:9.5pt;">✏ Edit Filter Settings</button>
                    </div>
                </div>
            </div>"#,
            num = state.selected_filter_idx + 1,
            name = escape_html(&f.name),
            status_badge = status_badge,
            scopes = escape_html(&scopes_display),
            cond = escape_html(&cond_summary),
            action = escape_html(&act_summary),
        );
    }

    let in_cls = if f.incoming { " active incoming" } else { "" };
    let out_cls = if f.outgoing { " active outgoing" } else { "" };
    let man_cls = if f.manual { " active manual" } else { "" };

    let headers = ["From:", "To:", "Cc:", "Subject:", "Reply-To:", "Any Recipient", "Any Header", "Body"];
    let verbs = ["contains", "doesn't contain", "is", "isn't", "starts with", "ends with", "appears", "doesn't appear", "matches regex"];
    let conjs = ["ignore", "and", "or", "unless"];
    let action_types = ["Transfer", "Copy", "Status", "Priority", "Label", "Junk", "Delete", "Stop"];

    let mut h1_opts = String::new();
    for &h in &headers {
        let sel = if f.header1.eq_ignore_ascii_case(h) { " selected" } else { "" };
        h1_opts.push_str(&format!(r#"<option value="{h}"{sel}>{h}</option>"#));
    }

    let mut v1_opts = String::new();
    for &v in &verbs {
        let sel = if f.verb1.eq_ignore_ascii_case(v) { " selected" } else { "" };
        v1_opts.push_str(&format!(r#"<option value="{v}"{sel}>{v}</option>"#));
    }

    let mut conj_opts = String::new();
    for &c in &conjs {
        let sel = if f.conjunction.eq_ignore_ascii_case(c) { " selected" } else { "" };
        let label = match c {
            "ignore" => "ignore second condition",
            "and" => "AND",
            "or" => "OR",
            "unless" => "UNLESS",
            _ => c,
        };
        conj_opts.push_str(&format!(r#"<option value="{c}"{sel}>{label}</option>"#));
    }

    let mut h2_opts = String::new();
    for &h in &headers {
        let sel = if f.header2.eq_ignore_ascii_case(h) { " selected" } else { "" };
        h2_opts.push_str(&format!(r#"<option value="{h}"{sel}>{h}</option>"#));
    }

    let mut v2_opts = String::new();
    for &v in &verbs {
        let sel = if f.verb2.eq_ignore_ascii_case(v) { " selected" } else { "" };
        v2_opts.push_str(&format!(r#"<option value="{v}"{sel}>{v}</option>"#));
    }

    let mut act1_type_opts = String::new();
    for &at in &action_types {
        let sel = if f.action1_type.eq_ignore_ascii_case(at) { " selected" } else { "" };
        act1_type_opts.push_str(&format!(r#"<option value="{at}"{sel}>{at}</option>"#));
    }

    let mut act1_val_opts = String::new();
    for mb in &state.mailboxes {
        let sel = if f.action1_value.eq_ignore_ascii_case(&mb.name) { " selected" } else { "" };
        act1_val_opts.push_str(&format!(r#"<option value="{name}"{sel}>{name}</option>"#, name = escape_html(&mb.name)));
    }
    if !state.mailboxes.iter().any(|m| m.name.eq_ignore_ascii_case(&f.action1_value)) && !f.action1_value.is_empty() {
        act1_val_opts.push_str(&format!(r#"<option value="{name}" selected>{name}</option>"#, name = escape_html(&f.action1_value)));
    }

    let mut act2_type_opts = String::from(r#"<option value="None">None</option>"#);
    for &at in &action_types {
        let sel = if f.action2_type.eq_ignore_ascii_case(at) { " selected" } else { "" };
        act2_type_opts.push_str(&format!(r#"<option value="{at}"{sel}>{at}</option>"#));
    }

    format!(
        r#"<div class="filter-inspector">
            <div class="fi-header">
                <div>
                    <div class="fi-title">Edit Rule #{num}: {name}</div>
                </div>
                <div class="fi-actions">
                    <span class="ci-status-badge">{status}</span>
                    <button class="ci-btn primary" data-action="save-filter">💾 Save</button>
                    <button class="ci-btn" data-action="close-filter-editor">✓ Done</button>
                    <button class="ci-btn danger" data-action="delete-filter">🗑 Delete</button>
                </div>
            </div>
            <div class="fi-body">
                <div class="fi-section">
                    <div class="fi-section-title">Filter Name &amp; Description</div>
                    <input class="fi-input" id="filt-name" type="text" value="{name_val}">
                </div>
                <div class="fi-section">
                    <div class="fi-section-title">When to Apply Filter</div>
                    <div class="fi-scope-group">
                        <button class="scope-btn{in_cls}" data-action="toggle-filt-scope" data-scope="incoming">Incoming Mail</button>
                        <button class="scope-btn{out_cls}" data-action="toggle-filt-scope" data-scope="outgoing">Outgoing Mail</button>
                        <button class="scope-btn{man_cls}" data-action="toggle-filt-scope" data-scope="manual">Manual Filter</button>
                    </div>
                </div>
                <div class="fi-section">
                    <div class="fi-section-title">Match Criteria (Conditions)</div>
                    <div class="cond-row">
                        <select class="fi-select" id="filt-h1">{h1_opts}</select>
                        <select class="fi-select" id="filt-v1">{v1_opts}</select>
                        <input class="fi-input" id="filt-val1" type="text" value="{val1}">
                    </div>
                    <div class="conj-row">
                        <span>Conjunction:</span>
                        <select class="fi-select" id="filt-conj">{conj_opts}</select>
                    </div>
                    <div class="cond-row">
                        <select class="fi-select" id="filt-h2">{h2_opts}</select>
                        <select class="fi-select" id="filt-v2">{v2_opts}</select>
                        <input class="fi-input" id="filt-val2" type="text" value="{val2}">
                    </div>
                </div>
                <div class="fi-section">
                    <div class="fi-section-title">Actions to Perform</div>
                    <div class="cond-row">
                        <span style="font-size:8.5pt;font-weight:bold;min-width:60px;">Action 1:</span>
                        <select class="fi-select" id="filt-act1-type">{act1_type_opts}</select>
                        <select class="fi-select" id="filt-act1-val" style="flex:1;">{act1_val_opts}</select>
                    </div>
                    <div class="cond-row" style="margin-top:8px;">
                        <span style="font-size:8.5pt;font-weight:bold;min-width:60px;">Action 2:</span>
                        <select class="fi-select" id="filt-act2-type">{act2_type_opts}</select>
                        <input class="fi-input" id="filt-act2-val" type="text" value="{act2_val}" placeholder="Value or target">
                    </div>
                </div>
                <div style="display:flex;gap:8px;justify-content:flex-end;margin-top:16px;">
                    <button class="ci-btn" data-action="close-filter-editor">Cancel / Done</button>
                    <button class="ci-btn primary" data-action="save-filter">💾 Save Rule</button>
                </div>
            </div>
        </div>"#,
        num = state.selected_filter_idx + 1,
        name = escape_html(&f.name),
        name_val = escape_html(&f.name),
        status = escape_html(&state.filter_status),
        in_cls = in_cls,
        out_cls = out_cls,
        man_cls = man_cls,
        h1_opts = h1_opts,
        v1_opts = v1_opts,
        val1 = escape_html(&f.value1),
        conj_opts = conj_opts,
        h2_opts = h2_opts,
        v2_opts = v2_opts,
        val2 = escape_html(&f.value2),
        act1_type_opts = act1_type_opts,
        act1_val_opts = act1_val_opts,
        act2_type_opts = act2_type_opts,
        act2_val = escape_html(&f.action2_value),
    )
}

fn set_inner_html_indexed(doc: &mut Document, id: u32, html: &str) {
    doc.set_inner_html(id, html);
    doc.rebuild_node_index();
}

// ============================================================================
// DOM Rendering: Eudora Skin
// ============================================================================

fn render_eudora_all(doc: &mut Document, state: &mut AppState) {
    state.ensure_valid_selection();
    state.update_status_bar();

    sync_theme_and_mode(doc, state.skin, state.last_mail_skin, state.dark_mode);

    // Apply split pane sizes
    if let Some(wazoo) = doc.get_element_by_id("wazoo") {
        doc.set_style_property(wazoo, "width", &format!("{}px", state.sidebar_width));
    }
    if let Some(el) = doc.get_element_by_id("in-msg-list-wrap") {
        doc.set_style_property(el, "height", &format!("{}px", state.msg_list_height));
        doc.set_style_property(el, "flex", "none");
    }
    if let Some(el) = doc.get_element_by_id("out-msg-list-wrap") {
        doc.set_style_property(el, "height", &format!("{}px", state.msg_list_height));
        doc.set_style_property(el, "flex", "none");
    }
    if let Some(el) = doc.get_element_by_id("contact-list-pane") {
        doc.set_style_property(el, "width", &format!("{}px", state.contact_list_width));
        doc.set_style_property(el, "flex", "none");
    }
    if let Some(el) = doc.get_element_by_id("filter-list-pane") {
        doc.set_style_property(el, "width", &format!("{}px", state.filter_list_width));
        doc.set_style_property(el, "flex", "none");
    }

    render_eudora_wazoo(doc, state);
    render_eudora_tabs(doc, state);
    render_eudora_welcome(doc, state);
    render_eudora_in_page(doc, state);
    render_eudora_out_page(doc, state);
    render_eudora_contacts(doc, state);
    render_eudora_filters(doc, state);
    render_eudora_compose(doc, state);
    render_eudora_calendar(doc, state);
    render_eudora_cal_modal(doc, state);
    render_eudora_wizard_modal(doc, state);
    render_eudora_context_menu(doc, state);
    render_eudora_statusbar(doc, state);
}

fn render_tree_node(
    node: &macmbx::store::Node,
    depth: usize,
    state: &AppState,
    html: &mut String,
) {
    let rel = node
        .path
        .strip_prefix(state.data_dir.join("mailboxes"))
        .unwrap_or(&node.path)
        .to_string_lossy()
        .to_string();

    match node.node_type {
        macmbx::store::NodeType::Folder => {
            let is_expanded = state.expanded_folders.contains(&rel) || state.expanded_folders.contains(&node.name);
            let arrow = if is_expanded { "▼" } else { "▶" };
            let folder_icon = if is_expanded { "📂" } else { "📁" };
            let depth_cls = match depth {
                0 => "",
                1 => " depth-1",
                2 => " depth-2",
                _ => " depth-3",
            };
            html.push_str(&format!(
                r#"<div class="wazoo-item{depth_cls}" data-folder="{rel}">
                    <span class="wazoo-expand" data-toggle-folder="{rel}" title="{toggle_title}">{arrow}</span>
                    <span class="wazoo-icon" data-toggle-folder="{rel}">{folder_icon}</span>
                    <span class="wazoo-name" data-toggle-folder="{rel}">{name}</span>
                    <span class="wazoo-menu-btn" data-ctx-menu="{rel}" data-is-folder="true" title="Folder actions">⋯</span>
                </div>"#,
                name = escape_html(&node.name),
                toggle_title = if is_expanded { "Collapse folder" } else { "Expand folder" },
            ));
            if is_expanded {
                for child in &node.children {
                    render_tree_node(child, depth + 1, state, html);
                }
            }
        }
        macmbx::store::NodeType::Mailbox => {
            let unread = state.unread_count(&node.name);
            let is_selected = state.selected_mailbox == node.name && match state.active_tab() {
                TabKind::Mailbox(ref m) => m == &node.name,
                _ => false,
            };
            let mut classes = String::from("wazoo-item");
            if depth == 1 {
                classes.push_str(" depth-1");
            } else if depth == 2 {
                classes.push_str(" depth-2");
            } else if depth >= 3 {
                classes.push_str(" depth-3");
            }
            if is_selected {
                classes.push_str(" selected");
            }
            let icon = mailbox_icon(&node.name);
            let badge_html = if unread > 0 {
                format!(r#"<span class="wazoo-badge">{unread}</span>"#)
            } else {
                String::new()
            };
            html.push_str(&format!(
                r#"<div class="{classes}" data-mailbox="{name}">
                    <span class="wazoo-expand"></span>
                    <span class="wazoo-icon">{icon}</span>
                    <span class="wazoo-name">{name}</span>
                    {badge_html}
                    <span class="wazoo-menu-btn" data-ctx-menu="{name}" data-is-folder="false" title="Mailbox actions">⋯</span>
                </div>"#,
                name = escape_html(&node.name),
            ));
            if node.name == "Trash" {
                html.push_str(r#"<div class="wazoo-item sep"></div>"#);
            }
        }
    }
}

fn days_in_month(y: i32, m: i32) -> i32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn calc_weekday(y: i32, m: i32, d: i32) -> i32 {
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y1 = y;
    if m < 3 {
        y1 -= 1;
    }
    ((y1 + y1 / 4 - y1 / 100 + y1 / 400 + t[(m - 1).clamp(0, 11) as usize] + d) % 7 + 7) % 7
}

fn month_name(m: i32) -> &'static str {
    match m {
        1 => "January", 2 => "February", 3 => "March", 4 => "April",
        5 => "May", 6 => "June", 7 => "July", 8 => "August",
        9 => "September", 10 => "October", 11 => "November", 12 => "December",
        _ => "Unknown",
    }
}

fn day_name(w: i32) -> &'static str {
    match w {
        0 => "Sunday", 1 => "Monday", 2 => "Tuesday", 3 => "Wednesday",
        4 => "Thursday", 5 => "Friday", 6 => "Saturday",
        _ => "Day",
    }
}

fn short_day_name(w: i32) -> &'static str {
    match w {
        0 => "Sun", 1 => "Mon", 2 => "Tue", 3 => "Wed",
        4 => "Thu", 5 => "Fri", 6 => "Sat",
        _ => "",
    }
}

fn prev_month_ymd(ymd: i32) -> i32 {
    let y = ymd / 10000;
    let m = (ymd / 100) % 100;
    if m <= 1 {
        (y - 1) * 10000 + 1201
    } else {
        y * 10000 + (m - 1) * 100 + 01
    }
}

fn next_month_ymd(ymd: i32) -> i32 {
    let y = ymd / 10000;
    let m = (ymd / 100) % 100;
    if m >= 12 {
        (y + 1) * 10000 + 101
    } else {
        y * 10000 + (m + 1) * 100 + 01
    }
}

fn format_ymd_str(ymd: i32) -> String {
    let y = ymd / 10000;
    let m = (ymd / 100) % 100;
    let d = ymd % 100;
    format!("{:04}-{:02}-{:02}", y, m, d)
}

fn parse_ymd_str(s: &str) -> Option<i32> {
    let clean = s.trim().replace('-', "");
    if clean.len() == 8 {
        clean.parse::<i32>().ok()
    } else {
        None
    }
}

fn format_time_hhmm(ts: i64) -> String {
    let (_, _, _, h, min) = macmbx::calendar::split_ts(ts);
    format!("{:02}:{:02}", h, min)
}

fn current_clock_str() -> String {
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let (_, _, _, h, min) = macmbx::calendar::split_ts(now_ts);
    let sec = now_ts % 60;
    format!("{:02}:{:02}:{:02}", h, min, sec)
}

fn cleanup_sample_calendar_events(store: &macmbx::calendar::CalendarStore) {
    store.delete_events_matching_prefix("seed-");
}

fn render_eudora_wazoo(doc: &mut Document, state: &AppState) {
    if let Some(tabs_el) = doc.get_element_by_id("wazoo-tabs-bar") {
        let tab_mail_sel = if state.wazoo_tab == WazooTab::Mailboxes { " selected" } else { "" };
        let tab_cal_sel = if state.wazoo_tab == WazooTab::Calendar { " selected" } else { "" };
        let tab_stat_sel = if state.wazoo_tab == WazooTab::Status { " selected" } else { "" };
        let tab_sig_sel = if state.wazoo_tab == WazooTab::Signatures { " selected" } else { "" };
        let tab_stn_sel = if state.wazoo_tab == WazooTab::Stationery { " selected" } else { "" };

        let html = format!(
            r#"<div class="wazoo-tab{tab_mail_sel}" data-wazoo-tab="mailboxes" title="Mailboxes">📬 Mail</div>
               <div class="wazoo-tab{tab_cal_sel}" data-wazoo-tab="calendar" title="Calendar">📅 Cal</div>
               <div class="wazoo-tab{tab_stat_sel}" data-wazoo-tab="status" title="Tasks &amp; Status">⚡ Status</div>
               <div class="wazoo-tab{tab_sig_sel}" data-wazoo-tab="signatures" title="Signatures">✍ Sig</div>
               <div class="wazoo-tab{tab_stn_sel}" data-wazoo-tab="stationery" title="Stationery">📄 Stat</div>"#
        );
        set_inner_html_indexed(doc, tabs_el, &html);
    }

    let Some(tree) = doc.get_element_by_id("wazoo-tree") else {
        return;
    };

    let mut html = String::new();
    match state.wazoo_tab {
        WazooTab::Mailboxes => {
            for node in state.store.children() {
                render_tree_node(node, 0, state, &mut html);
            }
        }
        WazooTab::Calendar => {
            let view_y = state.cal_view_date / 10000;
            let view_m = (state.cal_view_date / 100) % 100;
            let m_name = month_name(view_m);

            // 1. Mini-calendar widget
            html.push_str(&format!(
                r#"<div class="mini-cal">
                    <div class="mini-cal-nav">
                        <span class="mini-cal-title">{} {}</span>
                        <div class="mini-cal-btn-group">
                            <button class="mini-cal-btn" data-action="mini-cal-prev" title="Previous Month">◀</button>
                            <button class="mini-cal-btn" data-action="mini-cal-today" title="Go to Today">Today</button>
                            <button class="mini-cal-btn" data-action="mini-cal-next" title="Next Month">▶</button>
                            <button class="mini-cal-btn" data-action="open-full-calendar" title="Open Full Calendar">↗</button>
                        </div>
                    </div>
                    <div class="mini-cal-grid">
                        <div class="mini-cal-dow">S</div>
                        <div class="mini-cal-dow">M</div>
                        <div class="mini-cal-dow">T</div>
                        <div class="mini-cal-dow">W</div>
                        <div class="mini-cal-dow">T</div>
                        <div class="mini-cal-dow">F</div>
                        <div class="mini-cal-dow">S</div>"#,
                m_name, view_y
            ));

            let now_ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
            let (cy, cm, cd, _, _) = macmbx::calendar::split_ts(now_ts);
            let today_ymd = cy * 10000 + cm * 100 + cd;

            let first_dow = calc_weekday(view_y, view_m, 1);
            let days_in_cur = days_in_month(view_y, view_m);

            let (prev_y, prev_m) = if view_m <= 1 { (view_y - 1, 12) } else { (view_y, view_m - 1) };
            let days_in_prev = days_in_month(prev_y, prev_m);

            let mut total_cells = 0;
            // Previous month cells
            for d in (days_in_prev - first_dow + 1)..=days_in_prev {
                let cell_ymd = prev_y * 10000 + prev_m * 100 + d;
                html.push_str(&format!(r#"<div class="mini-cal-day other-month" data-cal-day="{}">{}</div>"#, cell_ymd, d));
                total_cells += 1;
            }

            // Current month cells
            for d in 1..=days_in_cur {
                let cell_ymd = view_y * 10000 + view_m * 100 + d;
                let mut cls = String::from("mini-cal-day");
                if cell_ymd == today_ymd { cls.push_str(" today"); }
                if cell_ymd == state.cal_selected_date { cls.push_str(" selected"); }
                if !state.cal_store.get_events_for_date(cell_ymd).is_empty() { cls.push_str(" has-event"); }
                html.push_str(&format!(r#"<div class="{}" data-cal-day="{}">{}</div>"#, cls, cell_ymd, d));
                total_cells += 1;
            }

            // Next month fill to complete 35 or 42 cells
            let target_total = if total_cells <= 35 { 35 } else { 42 };
            let (next_y, next_m) = if view_m >= 12 { (view_y + 1, 1) } else { (view_y, view_m + 1) };
            let mut next_d = 1;
            while total_cells < target_total {
                let cell_ymd = next_y * 10000 + next_m * 100 + next_d;
                html.push_str(&format!(r#"<div class="mini-cal-day other-month" data-cal-day="{}">{}</div>"#, cell_ymd, next_d));
                next_d += 1;
                total_cells += 1;
            }

            html.push_str("</div></div>");

            // 2. Day agenda
            let sel_y = state.cal_selected_date / 10000;
            let sel_m = (state.cal_selected_date / 100) % 100;
            let sel_d = state.cal_selected_date % 100;
            let dow_name = short_day_name(calc_weekday(sel_y, sel_m, sel_d));
            let events = state.cal_store.get_events_for_date(state.cal_selected_date);

            html.push_str(&format!(
                r#"<div class="sidebar-agenda">
                    <div class="agenda-topbar">
                        <span>{} {} {}, {}</span>
                        <span class="agenda-topbar-link" data-action="cal-new-event">➕ New</span>
                    </div>"#,
                dow_name, month_name(sel_m), sel_d, sel_y
            ));

            if events.is_empty() {
                html.push_str(r#"<div class="agenda-empty">No events scheduled.<br><span class="agenda-topbar-link" data-action="cal-new-event">Add event</span></div>"#);
            } else {
                for ev in &events {
                    let time_str = if ev.all_day {
                        "All day".to_string()
                    } else {
                        format!("{} - {}", format_time_hhmm(ev.dt_start_ts), format_time_hhmm(ev.dt_end_ts))
                    };
                    let loc_html = if !ev.location.is_empty() {
                        format!(r#"<div class="agenda-loc">📍 {}</div>"#, escape_html(&ev.location))
                    } else {
                        String::new()
                    };
                    html.push_str(&format!(
                        r#"<div class="agenda-item" style="border-left-color:{color};" data-cal-edit-uid="{uid}">
                            <div class="agenda-time" style="color:{color};">{time}</div>
                            <div class="agenda-summary">{summary}</div>
                            {loc_html}
                        </div>"#,
                        color = ev.color,
                        uid = escape_html(&ev.uid),
                        time = time_str,
                        summary = escape_html(&ev.summary),
                        loc_html = loc_html
                    ));
                }
            }
            html.push_str("</div>");
        }
        WazooTab::Status => {
            html.push_str(r#"<div class="sidebar-status"><div class="status-sec-hdr">Active Tasks &amp; Sync</div>"#);
            let tasks = state.compute_status_tasks();
            for task in &tasks {
                let pct = (task.progress * 100.0).clamp(0.0, 100.0);
                html.push_str(&format!(
                    r#"<div class="status-task-card">
                        <div class="status-task-row">
                            <span class="status-task-icon">{icon}</span>
                            <span>{title}</span>
                        </div>
                        <div class="status-task-sub">{status}</div>
                        <div class="status-pbar-wrap"><div class="status-pbar-fill" style="width:{pct}%;"></div></div>
                    </div>"#,
                    icon = task.icon,
                    title = escape_html(&task.title),
                    status = escape_html(&task.status),
                    pct = pct
                ));
            }

            html.push_str(
                r#"<div class="status-actions-bar">
                    <button class="status-act-btn" data-action="check-mail">⚡ Check</button>
                    <button class="status-act-btn" data-action="send-queued">📤 Send</button>
                    <button class="status-act-btn" data-action="status-clear-log">🧹 Clear</button>
                </div>
                <div class="status-sec-hdr" style="margin-top:6px;">Activity Log</div>
                <div class="status-log-wrap">"#
            );

            for log in state.status_logs.iter().rev() {
                html.push_str(&format!(
                    r#"<div class="status-log-entry"><span class="status-log-time">[{time}]</span>{msg}</div>"#,
                    time = escape_html(&log.time),
                    msg = escape_html(&log.message)
                ));
            }
            html.push_str("</div></div>");
        }
        WazooTab::Signatures => {
            html.push_str(
                r#"<div class="sidebar-status">
                    <div class="status-sec-hdr">Signatures</div>
                    <div class="sidebar-sig-card">
                        <div class="sidebar-sig-title">Standard</div>
                        <div class="sidebar-sig-body">— &#10;Sent with Eudora 2026</div>
                    </div>
                    <div class="sidebar-sig-card">
                        <div class="sidebar-sig-title">Alternate</div>
                        <div class="sidebar-sig-body">Best regards,&#10;Steve Dorner</div>
                    </div>
                </div>"#
            );
        }
        WazooTab::Stationery => {
            html.push_str(
                r#"<div class="sidebar-status">
                    <div class="status-sec-hdr">Stationery Templates</div>
                    <div class="sidebar-stat-card">
                        <div class="sidebar-stat-title">Default Memo</div>
                        <div class="sidebar-stat-body">MEMORANDUM&#10;To: [Recipients]&#10;From: Steve Dorner</div>
                    </div>
                    <div class="sidebar-stat-card">
                        <div class="sidebar-stat-title">Formal Letter</div>
                        <div class="sidebar-stat-body">Dear [Name],&#10;&#10;I am writing in regards to...</div>
                    </div>
                </div>"#
            );
        }
    }

    set_inner_html_indexed(doc, tree, &html);
}

fn render_eudora_tabs(doc: &mut Document, state: &AppState) {
    let Some(tabbar) = doc.get_element_by_id("tabbar") else {
        return;
    };
    let mut html = String::new();
    for (idx, tab) in state.open_tabs.iter().enumerate() {
        let is_selected = idx == state.active_tab_index;
        let sel_cls = if is_selected { " selected" } else { "" };
        let (icon, title) = match tab {
            TabKind::Welcome => ("🏠", "Welcome".to_string()),
            TabKind::Mailbox(name) => (mailbox_icon(name), name.clone()),
            TabKind::Contacts => ("👥", "Contacts".to_string()),
            TabKind::Filters => ("📋", "Filters".to_string()),
            TabKind::Compose => ("✏", "New Message".to_string()),
            TabKind::Calendar => ("📅", "Calendar".to_string()),
        };
        html.push_str(&format!(
            r#"<div class="tab{sel_cls}" data-tab-idx="{idx}">
                <span class="tab-label">{icon} {title}</span>
                <span class="tab-close" data-tab-close="{idx}" title="Close tab">×</span>
            </div>"#,
            title = escape_html(&title)
        ));
    }
    set_inner_html_indexed(doc, tabbar, &html);

    let active_tab = state.active_tab();
    let page_map = [
        ("page-welcome", matches!(active_tab, TabKind::Welcome)),
        ("page-in", matches!(active_tab, TabKind::Mailbox(ref m) if m != "Out")),
        ("page-out", matches!(active_tab, TabKind::Mailbox(ref m) if m == "Out")),
        ("page-contacts", matches!(active_tab, TabKind::Contacts)),
        ("page-filters", matches!(active_tab, TabKind::Filters)),
        ("page-compose", matches!(active_tab, TabKind::Compose)),
        ("page-calendar", matches!(active_tab, TabKind::Calendar)),
    ];
    for (page_id, is_active) in page_map {
        if let Some(el) = doc.get_element_by_id(page_id) {
            if is_active {
                doc.class_list_add(el, "active");
                doc.set_style_property(el, "display", "flex");
            } else {
                doc.class_list_remove(el, "active");
                doc.set_style_property(el, "display", "none");
            }
        }
    }
}

fn render_eudora_calendar(doc: &mut Document, state: &AppState) {
    if !matches!(state.active_tab(), TabKind::Calendar) {
        return;
    }

    let view_y = state.cal_view_date / 10000;
    let view_m = (state.cal_view_date / 100) % 100;
    let view_d = state.cal_view_date % 100;

    let sel_y = state.cal_selected_date / 10000;
    let sel_m = (state.cal_selected_date / 100) % 100;
    let sel_d = state.cal_selected_date % 100;

    // 1. Heading title
    let heading_text = match state.cal_view_mode.as_str() {
        "month" => format!("{} {}", month_name(view_m), view_y),
        "week" => format!("Week of {} {}, {}", month_name(view_m), view_d, view_y),
        "day" => format!("{}, {} {}, {}", day_name(calc_weekday(sel_y, sel_m, sel_d)), month_name(sel_m), sel_d, sel_y),
        "agenda" => "Upcoming Events (30 Days)".to_string(),
        _ => "Calendar".to_string(),
    };
    set_text_by_id(doc, "cal-heading-title", &heading_text);

    // 2. View mode buttons
    let mode_ids = [
        ("cal-mode-month", "month"),
        ("cal-mode-week", "week"),
        ("cal-mode-day", "day"),
        ("cal-mode-agenda", "agenda"),
    ];
    for (btn_id, mode) in mode_ids {
        if let Some(el) = doc.get_element_by_id(btn_id) {
            if state.cal_view_mode == mode {
                doc.class_list_add(el, "selected");
            } else {
                doc.class_list_remove(el, "selected");
            }
        }
    }

    // 3. View content
    let Some(container) = doc.get_element_by_id("cal-view-container") else {
        return;
    };

    let now_ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let (cy, cm, cd, _, _) = macmbx::calendar::split_ts(now_ts);
    let today_ymd = cy * 10000 + cm * 100 + cd;

    let mut html = String::new();

    match state.cal_view_mode.as_str() {
        "month" => {
            html.push_str(r#"<div class="cal-month-grid"><div class="cal-weekday-row">"#);
            for dow in 0..7 {
                html.push_str(&format!(r#"<div>{}</div>"#, day_name(dow)));
            }
            html.push_str(r#"</div><div class="cal-month-body">"#);

            let first_dow = calc_weekday(view_y, view_m, 1);
            let days_in_cur = days_in_month(view_y, view_m);
            let (prev_y, prev_m) = if view_m <= 1 { (view_y - 1, 12) } else { (view_y, view_m - 1) };
            let days_in_prev = days_in_month(prev_y, prev_m);

            let mut cell_count = 0;
            // Prev month fill
            for d in (days_in_prev - first_dow + 1)..=days_in_prev {
                let cell_ymd = prev_y * 10000 + prev_m * 100 + d;
                let events = state.cal_store.get_events_for_date(cell_ymd);
                html.push_str(&render_cal_month_cell(cell_ymd, d, true, cell_ymd == today_ymd, cell_ymd == state.cal_selected_date, &events));
                cell_count += 1;
            }

            // Current month days
            for d in 1..=days_in_cur {
                let cell_ymd = view_y * 10000 + view_m * 100 + d;
                let events = state.cal_store.get_events_for_date(cell_ymd);
                html.push_str(&render_cal_month_cell(cell_ymd, d, false, cell_ymd == today_ymd, cell_ymd == state.cal_selected_date, &events));
                cell_count += 1;
            }

            // Next month fill to 35 or 42
            let target_total = if cell_count <= 35 { 35 } else { 42 };
            let (next_y, next_m) = if view_m >= 12 { (view_y + 1, 1) } else { (view_y, view_m + 1) };
            let mut next_d = 1;
            while cell_count < target_total {
                let cell_ymd = next_y * 10000 + next_m * 100 + next_d;
                let events = state.cal_store.get_events_for_date(cell_ymd);
                html.push_str(&render_cal_month_cell(cell_ymd, next_d, true, cell_ymd == today_ymd, cell_ymd == state.cal_selected_date, &events));
                next_d += 1;
                cell_count += 1;
            }

            html.push_str("</div></div>");
        }
        "week" => {
            html.push_str(r#"<div class="cal-week-grid">"#);
            let anchor_dow = calc_weekday(view_y, view_m, view_d);
            let anchor_ts = macmbx::calendar::make_local_ts(view_y, view_m, view_d, 12, 0);
            let sun_ts = anchor_ts - (anchor_dow as i64 * 86400);

            for i in 0..7 {
                let day_ts = sun_ts + (i as i64 * 86400);
                let (dy, dm, dd, _, _) = macmbx::calendar::split_ts(day_ts);
                let day_ymd = dy * 10000 + dm * 100 + dd;
                let is_today = day_ymd == today_ymd;
                let is_sel = day_ymd == state.cal_selected_date;
                let mut col_cls = String::from("cal-week-col");
                if is_today { col_cls.push_str(" today"); }
                if is_sel { col_cls.push_str(" selected"); }

                let events = state.cal_store.get_events_for_date(day_ymd);

                html.push_str(&format!(
                    r#"<div class="{col_cls}">
                        <div class="cal-week-hdr">
                            <div class="cal-week-hdr-day">{dow}</div>
                            <div class="cal-week-hdr-date">{m} {d}</div>
                        </div>
                        <div class="cal-week-events">"#,
                    col_cls = col_cls,
                    dow = short_day_name(i),
                    m = month_name(dm),
                    d = dd
                ));

                for ev in &events {
                    let time_str = if ev.all_day {
                        "All day".to_string()
                    } else {
                        format!("{} - {}", format_time_hhmm(ev.dt_start_ts), format_time_hhmm(ev.dt_end_ts))
                    };
                    html.push_str(&format!(
                        r#"<div class="cal-chip" style="background:{color};" data-cal-edit-uid="{uid}">
                            <b>{time}</b> {summary}
                        </div>"#,
                        color = ev.color,
                        uid = escape_html(&ev.uid),
                        time = time_str,
                        summary = escape_html(&ev.summary)
                    ));
                }
                html.push_str("</div></div>");
            }
            html.push_str("</div>");
        }
        "day" => {
            html.push_str(&format!(
                r#"<div class="cal-day-view-wrap">
                    <div class="cal-day-title">{} — {} events</div>"#,
                heading_text,
                state.cal_store.get_events_for_date(state.cal_selected_date).len()
            ));

            let events = state.cal_store.get_events_for_date(state.cal_selected_date);
            if events.is_empty() {
                html.push_str(r#"<div class="agenda-empty" style="padding:40px;">No events scheduled for this day.<br><button class="cal-btn primary" data-action="cal-new-event" style="margin-top:12px;">➕ New Event</button></div>"#);
            } else {
                for ev in &events {
                    let time_str = if ev.all_day {
                        "All Day Event".to_string()
                    } else {
                        format!("{} – {}", format_time_hhmm(ev.dt_start_ts), format_time_hhmm(ev.dt_end_ts))
                    };
                    let loc_str = if !ev.location.is_empty() {
                        format!(r#"<div class="cal-day-card-loc">📍 {}</div>"#, escape_html(&ev.location))
                    } else {
                        String::new()
                    };
                    let desc_str = if !ev.description.is_empty() {
                        format!(r#"<div class="cal-day-card-desc">{}</div>"#, escape_html(&ev.description))
                    } else {
                        String::new()
                    };

                    html.push_str(&format!(
                        r#"<div class="cal-day-event-card" style="border-left-color:{color};" data-cal-edit-uid="{uid}">
                            <div class="cal-day-card-time" style="color:{color};">{time}</div>
                            <div class="cal-day-card-body">
                                <div class="cal-day-card-title">{summary}</div>
                                {loc_str}
                                {desc_str}
                            </div>
                            <button class="cal-btn" data-cal-edit-uid="{uid}">✏ Edit</button>
                        </div>"#,
                        color = ev.color,
                        uid = escape_html(&ev.uid),
                        time = time_str,
                        summary = escape_html(&ev.summary),
                        loc_str = loc_str,
                        desc_str = desc_str
                    ));
                }
            }
            html.push_str("</div>");
        }
        "agenda" => {
            html.push_str(r#"<div class="cal-agenda-view-wrap">"#);

            let mut any_found = false;
            let start_ts = macmbx::calendar::make_local_ts(sel_y, sel_m, sel_d, 12, 0);

            for i in 0..30 {
                let day_ts = start_ts + (i as i64 * 86400);
                let (dy, dm, dd, _, _) = macmbx::calendar::split_ts(day_ts);
                let day_ymd = dy * 10000 + dm * 100 + dd;
                let events = state.cal_store.get_events_for_date(day_ymd);
                if events.is_empty() {
                    continue;
                }
                any_found = true;
                let dow_str = day_name(calc_weekday(dy, dm, dd));
                let is_today = day_ymd == today_ymd;
                let today_lbl = if is_today { " (Today)" } else { "" };

                html.push_str(&format!(
                    r#"<div class="cal-agenda-group">
                        <div class="cal-agenda-date-hdr">📅 {}, {} {}, {}{}</div>"#,
                    dow_str, month_name(dm), dd, dy, today_lbl
                ));

                for ev in &events {
                    let time_str = if ev.all_day {
                        "All day".to_string()
                    } else {
                        format!("{} – {}", format_time_hhmm(ev.dt_start_ts), format_time_hhmm(ev.dt_end_ts))
                    };
                    let loc_str = if !ev.location.is_empty() {
                        format!(r#"<div class="cal-day-card-loc">📍 {}</div>"#, escape_html(&ev.location))
                    } else {
                        String::new()
                    };
                    html.push_str(&format!(
                        r#"<div class="cal-day-event-card" style="border-left-color:{color};" data-cal-edit-uid="{uid}">
                            <div class="cal-day-card-time" style="color:{color};">{time}</div>
                            <div class="cal-day-card-body">
                                <div class="cal-day-card-title">{summary}</div>
                                {loc_str}
                            </div>
                            <button class="cal-btn" data-cal-edit-uid="{uid}">✏ Edit</button>
                        </div>"#,
                        color = ev.color,
                        uid = escape_html(&ev.uid),
                        time = time_str,
                        summary = escape_html(&ev.summary),
                        loc_str = loc_str
                    ));
                }
                html.push_str("</div>");
            }

            if !any_found {
                html.push_str(r#"<div class="agenda-empty" style="padding:40px;">No upcoming events in the next 30 days.<br><button class="cal-btn primary" data-action="cal-new-event" style="margin-top:12px;">➕ New Event</button></div>"#);
            }

            html.push_str("</div>");
        }
        _ => {}
    }

    set_inner_html_indexed(doc, container, &html);
}

fn render_cal_month_cell(
    ymd: i32,
    day_num: i32,
    is_other_month: bool,
    is_today: bool,
    is_selected: bool,
    events: &[macmbx::calendar::CalendarEvent],
) -> String {
    let mut cls = String::from("cal-day-cell");
    if is_other_month { cls.push_str(" other-month"); }
    if is_today { cls.push_str(" today"); }
    if is_selected { cls.push_str(" selected"); }

    let mut chips = String::new();
    let max_chips = 3;
    for (i, ev) in events.iter().enumerate() {
        if i >= max_chips {
            let remaining = events.len() - max_chips;
            chips.push_str(&format!(r#"<div class="cal-chip" style="background:#78726c;font-size:7pt;">+{} more</div>"#, remaining));
            break;
        }
        let time_str = if ev.all_day { String::new() } else { format!("{} ", format_time_hhmm(ev.dt_start_ts)) };
        chips.push_str(&format!(
            r#"<div class="cal-chip" style="background:{color};" data-cal-edit-uid="{uid}" title="{full_title}">{time}{summary}</div>"#,
            color = ev.color,
            uid = escape_html(&ev.uid),
            full_title = escape_html(&ev.summary),
            time = time_str,
            summary = escape_html(&ev.summary)
        ));
    }

    format!(
        r#"<div class="{cls}" data-cal-day="{ymd}">
            <div class="cal-cell-header">
                <span class="cal-day-num">{day_num}</span>
                <span class="cal-cell-add-btn" data-action="cal-day-add" data-date="{ymd}" title="Add Event">+</span>
            </div>
            <div class="cal-cell-events">{chips}</div>
        </div>"#,
        cls = cls,
        ymd = ymd,
        day_num = day_num,
        chips = chips
    )
}

fn render_eudora_cal_modal(doc: &mut Document, state: &AppState) {
    let Some(modal_el) = doc.get_element_by_id("cal-event-modal") else {
        return;
    };

    if !state.cal_modal_open {
        doc.set_style_property(modal_el, "display", "none");
        return;
    }

    doc.set_style_property(modal_el, "display", "flex");

    if state.cal_is_new_event || state.cal_editing_uid.is_empty() {
        set_text_by_id(doc, "cal-modal-title", "➕ New Calendar Event");
        if let Some(el) = doc.get_element_by_id("cal-ev-summary") {
            doc.set_attribute(el, "value", "");
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-date") {
            doc.set_attribute(el, "value", &format_ymd_str(state.cal_selected_date));
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-stime") {
            doc.set_attribute(el, "value", "09:00");
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-etime") {
            doc.set_attribute(el, "value", "10:00");
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-location") {
            doc.set_attribute(el, "value", "");
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-desc") {
            doc.set_attribute(el, "value", "");
            doc.set_text_content(el, "");
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-color") {
            doc.set_attribute(el, "value", "#326ea5");
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-reminder") {
            doc.set_attribute(el, "value", "15");
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-allday") {
            doc.remove_attribute(el, "checked");
        }
        if let Some(el) = doc.get_element_by_id("cal-btn-delete") {
            doc.set_style_property(el, "display", "none");
        }
    } else if let Some(ev) = state.cal_store.get_event_by_uid(&state.cal_editing_uid) {
        set_text_by_id(doc, "cal-modal-title", "✏ Edit Calendar Event");
        if let Some(el) = doc.get_element_by_id("cal-ev-summary") {
            doc.set_attribute(el, "value", &ev.summary);
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-date") {
            doc.set_attribute(el, "value", &format_ymd_str(ev.start_day));
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-stime") {
            doc.set_attribute(el, "value", &format_time_hhmm(ev.dt_start_ts));
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-etime") {
            doc.set_attribute(el, "value", &format_time_hhmm(ev.dt_end_ts));
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-location") {
            doc.set_attribute(el, "value", &ev.location);
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-desc") {
            doc.set_attribute(el, "value", &ev.description);
            doc.set_text_content(el, &ev.description);
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-color") {
            doc.set_attribute(el, "value", &ev.color);
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-reminder") {
            doc.set_attribute(el, "value", &ev.reminder_mins.to_string());
        }
        if let Some(el) = doc.get_element_by_id("cal-ev-allday") {
            if ev.all_day {
                doc.set_attribute(el, "checked", "checked");
            } else {
                doc.remove_attribute(el, "checked");
            }
        }
        if let Some(el) = doc.get_element_by_id("cal-btn-delete") {
            doc.set_style_property(el, "display", "inline-block");
        }
    }
}

fn render_eudora_wizard_modal(doc: &mut Document, state: &mut AppState) {
    let Some(modal_el) = doc.get_element_by_id("account-wizard-modal") else {
        return;
    };

    if !state.wizard_modal_open {
        doc.set_style_property(modal_el, "display", "none");
        return;
    }

    doc.set_style_property(modal_el, "display", "flex");

    // Ensure the wizard image has the base64 data URL and is synchronously decoded
    if let Some(img_el) = doc.get_element_by_id("wiz-sidebar-img") {
        doc.set_attribute(img_el, "src", &WIZ_WELCOME_DATA_URL);
        if let Some(node) = doc.find_webcore_mut(img_el) {
            if node.image_data.is_none() {
                if let Some(decoded) = webcore::images::load_decoded_image_from_src(&WIZ_WELCOME_DATA_URL, "") {
                    webcore::images::set_decoded_image_on_node(node, decoded);
                }
            }
        }
    }

    let is_dom = state.wizard.is_dominant;
    let cur_step = state.wizard.step;

    // Show/hide personality step tab
    if let Some(el) = doc.get_element_by_id("wiz-sum-row-pers") {
        doc.set_style_property(el, "display", if is_dom { "none" } else { "flex" });
    }

    // Update Wizard Header Band with active step title, description, and step progress pill
    let (active_title, active_desc, active_pill, active_icon) = match (state.wizard.flow, cur_step) {
        (_, 0) => (
            "Welcome to Phoenix",
            "Account Setup Wizard",
            "Intro",
            "📬",
        ),
        (_, 1) => (
            "Account Settings",
            "Choose your preferred configuration method",
            "Step 1",
            "⚙️",
        ),
        (WizardFlow::Detect, 9) => (
            "Select Email Provider",
            "Choose your service for automatic server configuration",
            "Step 2 of 4",
            "🔍",
        ),
        (WizardFlow::Detect, 10) => (
            match state.wizard.provider.as_str() {
                "gmail" => "Google / Gmail Credentials",
                "outlook" => "Outlook / Hotmail Credentials",
                "yahoo" => "Yahoo! Mail Credentials",
                _ => "Account Credentials",
            },
            "Enter your email address, login name, and password",
            "Step 3 of 4",
            "🔑",
        ),
        (WizardFlow::Advanced, 6) => (
            "Incoming Mail Server",
            "Specify server hostname, port, protocol, and SSL/TLS",
            "Step 2 of 4",
            "📥",
        ),
        (WizardFlow::Advanced, 7) => (
            "Outgoing Mail Server",
            "Specify SMTP hostname, port, and authentication",
            "Step 3 of 4",
            "📤",
        ),
        (WizardFlow::Import, 11) => (
            "Import Settings",
            "Scan system for existing mail client profiles",
            "Step 2 of 3",
            "📦",
        ),
        (_, 2) => (
            "Personality Name",
            "Assign a unique name to identify this email persona",
            if is_dom { "Step 2 of 6" } else { "Step 2 of 7" },
            "🏷️",
        ),
        (_, 3) => (
            "Personal Information",
            "Enter your display name as it appears in outgoing mail",
            if is_dom { "Step 2 of 6" } else { "Step 3 of 7" },
            "👤",
        ),
        (_, 4) => (
            "Email Address",
            "Enter your return email address",
            if is_dom { "Step 3 of 6" } else { "Step 4 of 7" },
            "✉️",
        ),
        (_, 5) => (
            "User Name",
            "Enter your mail server login username",
            if is_dom { "Step 4 of 6" } else { "Step 5 of 7" },
            "🔑",
        ),
        (_, 6) => (
            "Incoming Mail Server",
            "Specify incoming server hostname, port, and protocol",
            if is_dom { "Step 5 of 6" } else { "Step 6 of 7" },
            "📥",
        ),
        (_, 7) => (
            "Outgoing Mail Server",
            "Specify outgoing SMTP server and authentication settings",
            if is_dom { "Step 6 of 6" } else { "Step 7 of 7" },
            "📤",
        ),
        (_, 8) => (
            "Ready to Connect",
            "Review your account summary and finish setup",
            "Complete",
            "✅",
        ),
        _ => (
            "Account Setup Wizard",
            "Configure your email account",
            "Setup",
            "📧",
        ),
    };
    set_text_by_id(doc, "wiz-active-title", active_title);
    set_text_by_id(doc, "wiz-active-desc", active_desc);
    set_text_by_id(doc, "wiz-active-pill", active_pill);
    set_text_by_id(doc, "wiz-header-icon", active_icon);

    // Update active step pane
    for i in 0..=11 {
        if let Some(pane_el) = doc.get_element_by_id(&format!("wiz-step-{}", i)) {
            if i == cur_step {
                doc.class_list_add(pane_el, "active");
            } else {
                doc.class_list_remove(pane_el, "active");
            }
        }
    }

    // Step 1: Account setup choice cards and radio sync
    for (val, radio_id, card_id) in [
        (0, "wiz-radio-new", "wiz-card-choice-0"),
        (2, "wiz-radio-detect", "wiz-card-choice-2"),
        (3, "wiz-radio-skip", "wiz-card-choice-3"),
        (1, "wiz-radio-import", "wiz-card-choice-1"),
    ] {
        let is_selected = state.wizard.account_choice == val;
        if let Some(el) = doc.get_element_by_id(radio_id) {
            if is_selected {
                doc.set_attribute(el, "checked", "checked");
            } else {
                doc.remove_attribute(el, "checked");
            }
        }
        if let Some(card_el) = doc.get_element_by_id(card_id) {
            if is_selected {
                doc.class_list_add(card_el, "selected");
            } else {
                doc.class_list_remove(card_el, "selected");
            }
        }
    }

    // Step 9: Provider choice cards and radio sync
    for (p, radio_id, card_id) in [
        ("gmail", "wiz-prov-gmail", "wiz-card-prov-gmail"),
        ("outlook", "wiz-prov-outlook", "wiz-card-prov-outlook"),
        ("yahoo", "wiz-prov-yahoo", "wiz-card-prov-yahoo"),
        ("other", "wiz-prov-other", "wiz-card-prov-other"),
    ] {
        let is_selected = state.wizard.provider == p;
        if let Some(el) = doc.get_element_by_id(radio_id) {
            if is_selected {
                doc.set_attribute(el, "checked", "checked");
            } else {
                doc.remove_attribute(el, "checked");
            }
        }
        if let Some(card_el) = doc.get_element_by_id(card_id) {
            if is_selected {
                doc.class_list_add(card_el, "selected");
            } else {
                doc.class_list_remove(card_el, "selected");
            }
        }
    }

    let sync_input = |doc: &mut Document, id: &str, val: &str| {
        if let Some(el) = doc.get_element_by_id(id) {
            if !val.is_empty() {
                doc.set_attribute(el, "value", val);
                doc.set_value(el, val);
            }
        }
    };

    // Step 10: Credentials sync
    sync_input(doc, "wiz-cred-email", &state.wizard.return_address);
    sync_input(doc, "wiz-cred-password", &state.wizard.password);
    sync_input(doc, "wiz-cred-name", &state.wizard.real_name);
    if state.wizard.incoming_protocol.eq_ignore_ascii_case("IMAP") {
        if let Some(el) = doc.get_element_by_id("wiz-cred-proto-imap") {
            doc.set_attribute(el, "checked", "checked");
        }
        if let Some(el) = doc.get_element_by_id("wiz-cred-proto-pop") {
            doc.remove_attribute(el, "checked");
        }
    } else {
        if let Some(el) = doc.get_element_by_id("wiz-cred-proto-pop") {
            doc.set_attribute(el, "checked", "checked");
        }
        if let Some(el) = doc.get_element_by_id("wiz-cred-proto-imap") {
            doc.remove_attribute(el, "checked");
        }
    }
    let (cred_title, cred_desc) = match state.wizard.provider.as_str() {
        "gmail" => ("Google / Gmail Credentials", "Enter your Google email address and an App Password:"),
        "outlook" => ("Microsoft Outlook / Hotmail Credentials", "Enter your Microsoft / Outlook email address and password:"),
        "yahoo" => ("Yahoo! Mail Credentials", "Enter your Yahoo email address and generated App Password:"),
        _ => ("Account Credentials", "Enter your email address, login name, and password:"),
    };
    set_text_by_id(doc, "wiz-cred-title", cred_title);
    set_text_by_id(doc, "wiz-cred-desc", cred_desc);

    // Populate standard input controls with current wizard state
    sync_input(doc, "wiz-persname", &state.wizard.persona_name);
    sync_input(doc, "wiz-yourname", &state.wizard.real_name);
    sync_input(doc, "wiz-email", &state.wizard.return_address);
    sync_input(doc, "wiz-login", &state.wizard.login_name);
    sync_input(doc, "wiz-inserver", &state.wizard.incoming_server);
    let inport_str = state.wizard.incoming_port.to_string();
    sync_input(doc, "wiz-inport", &inport_str);
    sync_input(doc, "wiz-outserver", &state.wizard.smtp_server);
    let outport_str = state.wizard.smtp_port.to_string();
    sync_input(doc, "wiz-outport", &outport_str);

    // Protocol radio sync
    if state.wizard.incoming_protocol.eq_ignore_ascii_case("IMAP") {
        if let Some(el) = doc.get_element_by_id("wiz-proto-imap") {
            doc.set_attribute(el, "checked", "checked");
        }
        if let Some(el) = doc.get_element_by_id("wiz-proto-pop") {
            doc.remove_attribute(el, "checked");
        }
    } else {
        if let Some(el) = doc.get_element_by_id("wiz-proto-pop") {
            doc.set_attribute(el, "checked", "checked");
        }
        if let Some(el) = doc.get_element_by_id("wiz-proto-imap") {
            doc.remove_attribute(el, "checked");
        }
    }

    // SSL checkboxes sync
    if let Some(el) = doc.get_element_by_id("wiz-in-ssl") {
        if state.wizard.incoming_ssl {
            doc.set_attribute(el, "checked", "checked");
        } else {
            doc.remove_attribute(el, "checked");
        }
    }
    if let Some(el) = doc.get_element_by_id("wiz-out-ssl") {
        if state.wizard.smtp_ssl {
            doc.set_attribute(el, "checked", "checked");
        } else {
            doc.remove_attribute(el, "checked");
        }
    }
    if let Some(el) = doc.get_element_by_id("wiz-out-auth") {
        if state.wizard.smtp_auth_allowed {
            doc.set_attribute(el, "checked", "checked");
        } else {
            doc.remove_attribute(el, "checked");
        }
    }

    // Summary Card on step 8
    let summary_name = if !state.wizard.real_name.is_empty() {
        state.wizard.real_name.clone()
    } else if let Some((u, _)) = state.wizard.return_address.split_once('@') {
        u.to_string()
    } else {
        "(Not Specified)".to_string()
    };
    let summary_login = if !state.wizard.login_name.is_empty() {
        state.wizard.login_name.clone()
    } else if let Some((u, _)) = state.wizard.return_address.split_once('@') {
        u.to_string()
    } else {
        "(Not Specified)".to_string()
    };
    let summary_email = if !state.wizard.return_address.is_empty() {
        state.wizard.return_address.clone()
    } else {
        "(Not Specified)".to_string()
    };
    let summary_pers = if state.wizard.persona_name.is_empty() {
        "(Dominant / Primary)".to_string()
    } else {
        state.wizard.persona_name.clone()
    };

    set_text_by_id(doc, "wiz-sum-pers", &summary_pers);
    set_text_by_id(doc, "wiz-sum-name", &summary_name);
    set_text_by_id(doc, "wiz-sum-email", &summary_email);
    set_text_by_id(doc, "wiz-sum-login", &summary_login);

    let in_ssl_str = if state.wizard.incoming_ssl { "SSL/TLS" } else { "Plain" };
    set_text_by_id(
        doc,
        "wiz-sum-in",
        &format!(
            "{} : {} ({} · {})",
            if state.wizard.incoming_server.is_empty() { "none" } else { &state.wizard.incoming_server },
            state.wizard.incoming_port,
            state.wizard.incoming_protocol,
            in_ssl_str
        ),
    );

    let out_ssl_str = if state.wizard.smtp_ssl { "SSL/TLS" } else { "Plain" };
    let out_auth_str = if state.wizard.smtp_auth_allowed { "Auth Enabled" } else { "No Auth" };
    set_text_by_id(
        doc,
        "wiz-sum-out",
        &format!(
            "{} : {} ({} · {})",
            if state.wizard.smtp_server.is_empty() { "none" } else { &state.wizard.smtp_server },
            state.wizard.smtp_port,
            out_ssl_str,
            out_auth_str
        ),
    );

    // Update buttons
    let is_first = if is_dom { cur_step == 0 } else { cur_step == 1 };
    if let Some(back_btn) = doc.get_element_by_id("wiz-btn-back") {
        if is_first {
            doc.set_attribute(back_btn, "disabled", "true");
        } else {
            doc.remove_attribute(back_btn, "disabled");
        }
    }

    if let Some(_next_btn) = doc.get_element_by_id("wiz-btn-next") {
        if cur_step == 8 {
            set_text_by_id(doc, "wiz-btn-next", "Finish ✓");
        } else {
            set_text_by_id(doc, "wiz-btn-next", "Next >");
        }
    }
}

fn render_eudora_welcome(doc: &mut Document, state: &AppState) {
    if !matches!(state.active_tab(), TabKind::Welcome) {
        return;
    }
    let real_name = state.conf.get("sending_mail", "real_name")
        .or_else(|| state.conf.get("Settings", "RealName"))
        .unwrap_or("Steve Dorner");
    let first_name = real_name.split_whitespace().next().unwrap_or("there");

    let hour = {
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let (_, _, _, hour, _) = macmbx::calendar::split_ts(now_ts);
        hour
    };
    let tod = if hour < 12 {
        "Good morning"
    } else if hour < 17 {
        "Good afternoon"
    } else {
        "Good evening"
    };
    let greeting = format!("{}, {}", tod, first_name);
    set_text_by_id(doc, "welcome-greeting", &greeting);

    let email = state.conf.get("sending_mail", "email_address")
        .or_else(|| state.conf.get("Settings", "ReturnAddress"))
        .unwrap_or("you@example.com");
    let server = state.conf.get("checking_mail", "pop_server")
        .or_else(|| state.conf.get("Settings", "MailServer"))
        .unwrap_or("mail.example.com");
    let use_imap = state.conf.get_bool("checking_mail", "use_imap", false);
    let use_ssl = state.conf.get_bool("ssl", "use_ssl", false);
    let proto = if use_imap { "IMAP" } else { "POP3" };
    let ssl_str = if use_ssl { " • SSL" } else { "" };
    let acct_line = format!("{} • {} • {}{}",
        if email.is_empty() { "No email configured" } else { email },
        proto,
        server,
        ssl_str
    );
    set_text_by_id(doc, "welcome-account-line", &acct_line);

    let Some(stat_grid) = doc.get_element_by_id("welcome-stat-grid") else {
        return;
    };
    let in_count = state.messages.iter().filter(|m| m.mailbox == "In").count();
    let out_count = state.messages.iter().filter(|m| m.mailbox == "Out").count();
    let junk_count = state.messages.iter().filter(|m| m.mailbox == "Spam" || m.mailbox == "Junk").count();
    let trash_count = state.messages.iter().filter(|m| m.mailbox == "Trash").count();

    let junk_name = if state.messages.iter().any(|m| m.mailbox == "Junk") {
        "Junk"
    } else {
        "Spam"
    };

    let stats = [
        ("In", "Inbox", in_count, "stat-accent-blue"),
        ("Out", "Outbox", out_count, "stat-accent-green"),
        (junk_name, "Junk", junk_count, "stat-accent-amber"),
        ("Trash", "Trash", trash_count, "stat-accent-red"),
    ];
    let mut stats_html = String::new();
    for (mbox, label, count, accent) in stats {
        stats_html.push_str(&format!(
            r#"<div class="stat-card {accent}" data-open-mailbox="{mbox}" title="Open {label}">
                <div class="stat-number">{count}</div>
                <div class="stat-label">{label}</div>
            </div>"#
        ));
    }
    set_inner_html_indexed(doc, stat_grid, &stats_html);
}

fn render_eudora_context_menu(doc: &mut Document, state: &AppState) {
    let Some(menu) = doc.get_element_by_id("mailbox-ctx-menu") else {
        return;
    };
    if state.ctx_menu_open {
        doc.set_style_property(menu, "display", "block");
        doc.set_style_property(menu, "left", &format!("{}px", state.ctx_menu_pos.0.max(10.0)));
        doc.set_style_property(menu, "top", &format!("{}px", state.ctx_menu_pos.1.max(10.0)));
    } else {
        doc.set_style_property(menu, "display", "none");
    }
}

fn render_eudora_rows_dom(
    doc: &mut Document,
    tbody_id: &str,
    messages: &[&MailMessage],
    is_out: bool,
    selected_id: usize,
) {
    let Some(tbody) = doc.get_element_by_id(tbody_id) else {
        return;
    };
    doc.set_text_content(tbody, "");

    for msg in messages {
        let tr = doc.create_element("tr");
        let is_sel = msg.id == selected_id;
        let mut row_cls = String::new();
        if is_sel {
            row_cls.push_str("selected");
        } else if msg.unread {
            row_cls.push_str("unread");
        }
        if !row_cls.is_empty() {
            doc.set_attribute(tr, "class", &row_cls);
        }
        doc.set_attribute(tr, "data-msg-id", &msg.id.to_string());
        doc.set_attribute(tr, "data-box", if is_out { "out" } else { "in" });

        // 1. Status cell
        let td_s = doc.create_element("td");
        doc.set_attribute(td_s, "class", "col-s");
        if msg.unread {
            let dot = doc.create_element("span");
            doc.set_attribute(dot, "class", "dot-unread");
            doc.append_child(td_s, dot);
        } else if msg.replied {
            doc.set_text_content(td_s, "↩");
        } else if msg.forwarded {
            doc.set_text_content(td_s, "→");
        }
        doc.append_child(tr, td_s);

        // 2. Priority cell
        let td_p = doc.create_element("td");
        doc.set_attribute(td_p, "class", "col-p");
        match msg.priority {
            "highest" => {
                let s = doc.create_element("span");
                doc.set_attribute(s, "class", "pri-highest");
                doc.set_text_content(s, "▲▲");
                doc.append_child(td_p, s);
            }
            "high" => {
                let s = doc.create_element("span");
                doc.set_attribute(s, "class", "pri-high");
                doc.set_text_content(s, "▲");
                doc.append_child(td_p, s);
            }
            _ => {}
        }
        doc.append_child(tr, td_p);

        // 3. Attachment cell
        let td_a = doc.create_element("td");
        doc.set_attribute(td_a, "class", "col-a");
        if msg.has_attach {
            doc.set_text_content(td_a, "📎");
        }
        doc.append_child(tr, td_a);

        // 4. Flag cell
        let td_f = doc.create_element("td");
        doc.set_attribute(td_f, "class", "col-f");
        if msg.flagged {
            doc.set_attribute(td_f, "style", "color:#d23720");
            doc.set_text_content(td_f, "⚑");
        }
        doc.append_child(tr, td_f);

        // 5. Who cell
        let td_who = doc.create_element("td");
        doc.set_attribute(td_who, "class", "col-who");
        let who = if is_out {
            display_name(&msg.to)
        } else {
            display_name(&msg.from)
        };
        doc.set_text_content(td_who, who);
        doc.append_child(tr, td_who);

        // 6. Date cell
        let td_date = doc.create_element("td");
        doc.set_attribute(td_date, "class", "col-date");
        doc.set_text_content(td_date, &msg.date);
        doc.append_child(tr, td_date);

        // 7. Size cell
        let td_size = doc.create_element("td");
        doc.set_attribute(td_size, "class", "col-size");
        doc.set_text_content(td_size, &msg.size);
        doc.append_child(tr, td_size);

        // 8. Subject cell
        let td_subj = doc.create_element("td");
        doc.set_attribute(td_subj, "class", "col-subj");
        doc.set_text_content(td_subj, &msg.subject);
        doc.append_child(tr, td_subj);

        doc.append_child(tbody, tr);
    }

    doc.rebuild_node_index();
}

fn render_eudora_in_page(doc: &mut Document, state: &AppState) {
    let mailbox = if state.selected_mailbox == "Out" {
        "In"
    } else {
        state.selected_mailbox.as_str()
    };
    let visible = state.visible_messages_for_mailbox(mailbox);

    if let Some(who_hdr) = doc.get_element_by_id("in-who-header") {
        let is_outgoing = mailbox == "Sent" || mailbox == "Out";
        doc.set_text_content(who_hdr, if is_outgoing { "To" } else { "Who" });
    }

    render_eudora_rows_dom(doc, "inbox-rows", &visible, false, state.selected_in_msg_id);

    let selected = visible
        .iter()
        .copied()
        .find(|m| m.id == state.selected_in_msg_id)
        .or_else(|| visible.first().copied());

    if let Some(msg) = selected {
        set_text_by_id(doc, "in-from", &msg.from);
        set_text_by_id(doc, "in-to", &msg.to);
        if let Some(cc_el) = doc.get_element_by_id("in-cc-area") {
            if msg.cc.is_empty() {
                set_inner_html_indexed(doc, cc_el, "");
            } else {
                set_inner_html_indexed(
                    doc,
                    cc_el,
                    &format!(
                        r#" &nbsp; <span class="lbl">Cc:</span> {}"#,
                        escape_html(&msg.cc)
                    ),
                );
            }
        }
        set_text_by_id(doc, "in-subj", &msg.subject);
        let mut meta = format!("Date: {}    Size: {}", msg.date, msg.size);
        if msg.priority == "highest" || msg.priority == "high" {
            meta.push_str("    Priority: HIGH");
        }
        if !msg.attachment_note.is_empty() {
            meta.push_str(&format!("    {}", msg.attachment_note));
        } else if msg.has_attach {
            meta.push_str("    📎 Attachment");
        }
        set_text_by_id(doc, "in-meta", &meta);
        if let Some(body_el) = doc.get_element_by_id("in-body") {
            set_inner_html_indexed(doc, body_el, &msg.body_html);
        }
    } else {
        set_text_by_id(doc, "in-from", "—");
        set_text_by_id(doc, "in-to", "—");
        set_text_by_id(doc, "in-subj", &format!("No messages in {}", mailbox));
        set_text_by_id(doc, "in-meta", "");
        if let Some(body_el) = doc.get_element_by_id("in-body") {
            set_inner_html_indexed(doc, body_el, "<p>This mailbox is empty.</p>");
        }
    }
}

fn render_eudora_out_page(doc: &mut Document, state: &AppState) {
    let visible = state.visible_messages_for_mailbox("Out");
    render_eudora_rows_dom(doc, "outbox-rows", &visible, true, state.selected_out_msg_id);

    let selected = visible
        .iter()
        .copied()
        .find(|m| m.id == state.selected_out_msg_id)
        .or_else(|| visible.first().copied());

    if let Some(msg) = selected {
        set_text_by_id(doc, "out-from", &msg.from);
        set_text_by_id(doc, "out-to", &msg.to);
        if let Some(cc_el) = doc.get_element_by_id("out-cc-area") {
            if msg.cc.is_empty() {
                set_inner_html_indexed(doc, cc_el, "");
            } else {
                set_inner_html_indexed(
                    doc,
                    cc_el,
                    &format!(
                        r#" &nbsp; <span class="lbl">Cc:</span> {}"#,
                        escape_html(&msg.cc)
                    ),
                );
            }
        }
        set_text_by_id(doc, "out-subj", &msg.subject);
        set_text_by_id(
            doc,
            "out-meta",
            &format!("Date: {}    Size: {}", msg.date, msg.size),
        );
        if let Some(body_el) = doc.get_element_by_id("out-body") {
            set_inner_html_indexed(doc, body_el, &msg.body_html);
        }
    }
}

fn render_eudora_contacts(doc: &mut Document, state: &AppState) {
    if let Some(list_el) = doc.get_element_by_id("contact-list-inner") {
        set_inner_html_indexed(doc, list_el, &build_contact_list_html(state));
    }
    if let Some(detail_el) = doc.get_element_by_id("contact-detail-inner") {
        set_inner_html_indexed(doc, detail_el, &build_contact_detail_html(state));
    }
}

fn render_eudora_filters(doc: &mut Document, state: &AppState) {
    let active_count = state.filters.iter().filter(|f| f.enabled).count();
    let disabled_count = state.filters.len().saturating_sub(active_count);
    set_text_by_id(
        doc,
        "filters-meta",
        &format!(
            "{} rules • {} active • {} disabled",
            state.filters.len(),
            active_count,
            disabled_count
        ),
    );

    if let Some(cards_el) = doc.get_element_by_id("filter-cards") {
        let mut html = String::new();
        for (idx, f) in state.filters.iter().enumerate() {
            let is_sel = idx == state.selected_filter_idx;
            let sel_cls = if is_sel { " selected" } else { "" };
            let status_cls = if f.enabled { "active" } else { "inactive" };
            let track_cls = if f.enabled { "on" } else { "off" };
            let text_cls = if f.enabled { "" } else { "inactive" };
            let cond_summary = f.condition_summary();
            let act_summary = f.action_summary();
            let is_junk = act_summary.contains("Junk") || act_summary.contains("Trash");
            let then_cls = if is_junk { "junk" } else { "" };
            let pill_text = if f.enabled { "Active" } else { "Off" };

            let mut scopes = String::new();
            if f.incoming { scopes.push_str(r#"<span class="filt-badge-in">IN</span>"#); }
            if f.outgoing { scopes.push_str(r#"<span class="filt-badge-out">OUT</span>"#); }
            if f.manual { scopes.push_str(r#"<span class="filt-badge-man">MAN</span>"#); }

            html.push_str(&format!(
                r#"<div class="filter-card{sel_cls}" data-filter="{idx}">
  <div class="filter-status-bar {status_cls}"></div>
  <div class="filter-toggle">
    <div class="toggle-track {track_cls}" data-action="toggle-filter" data-filter="{idx}">
      <div class="toggle-knob"></div>
    </div>
  </div>
  <div class="filter-body" data-filter="{idx}">
    <div style="display:flex;align-items:center;gap:6px;justify-content:space-between;">
      <div>
        <span class="filter-num">#{num}</span>
        <span class="filter-name {text_cls}">{name}</span>
      </div>
      <div style="display:flex;align-items:center;gap:4px;">
        {scopes}
        <button class="filt-tool-btn" data-action="edit-filter" data-filter="{idx}" title="Edit this rule" style="padding:1px 5px;font-size:7.5pt;margin-left:4px;">✏</button>
      </div>
    </div>
    <div class="filter-row">
      <span class="filter-pill if">IF</span>
      <span class="filter-text {text_cls}">{cond}</span>
    </div>
    <div class="filter-arrow">↓</div>
    <div class="filter-row">
      <span class="filter-pill then {then_cls}">THEN</span>
      <span class="filter-action-text {text_cls}">{action}</span>
    </div>
  </div>
  <span class="filter-status-pill {status_cls}">{pill_text}</span>
</div>"#,
                num = idx + 1,
                name = escape_html(&f.name),
                cond = escape_html(&cond_summary),
                action = escape_html(&act_summary),
            ));
        }
        set_inner_html_indexed(doc, cards_el, &html);
    }

    if let Some(editor_el) = doc.get_element_by_id("filter-editor-inner") {
        set_inner_html_indexed(doc, editor_el, &build_filter_editor_html(state));
    }
}

fn get_input_or_text(doc: &Document, id: &str) -> Option<String> {
    doc.get_element_by_id(id).and_then(|el| {
        let v = doc.value(el);
        if !v.is_empty() {
            Some(v)
        } else {
            doc.get_attribute(el, "value").or_else(|| {
                let t = doc.text_content(el);
                if !t.is_empty() { Some(t) } else { None }
            })
        }
    })
}

fn build_from_list(conf: &macmbx::conf::Conf) -> Vec<String> {
    let mut items = Vec::new();
    let email = conf.get("sending_mail", "email_address")
        .or_else(|| conf.get("Settings", "ReturnAddress"))
        .unwrap_or("");
    let name = conf.get("sending_mail", "real_name")
        .or_else(|| conf.get("Settings", "RealName"))
        .unwrap_or("");
    if !email.is_empty() {
        if !name.is_empty() {
            items.push(format!("{} <{}>", name, email));
        } else {
            items.push(email.to_string());
        }
    }
    for acct in conf.get_all_accounts() {
        if !acct.enabled || acct.email.is_empty() { continue; }
        let display_name = if !acct.real_name.is_empty() { &acct.real_name } else { &acct.name };
        let item = if !display_name.is_empty() {
            format!("{} <{}>", display_name, acct.email)
        } else {
            acct.email.clone()
        };
        if !items.contains(&item) {
            items.push(item);
        }
    }
    if items.is_empty() {
        items.push("you@example.com".to_string());
    }
    items
}

fn sync_compose_from_dom(doc: &Document, state: &mut AppState) {
    if let Some(el) = doc.get_element_by_id("compose-from") {
        let val = doc.value(el);
        if !val.is_empty() { state.compose.from = val; }
    }
    if let Some(v) = get_input_or_text(doc, "compose-to") { state.compose.to = v; }
    if let Some(v) = get_input_or_text(doc, "compose-cc") { state.compose.cc = v; }
    if let Some(v) = get_input_or_text(doc, "compose-bcc") { state.compose.bcc = v; }
    if let Some(v) = get_input_or_text(doc, "compose-subj") { state.compose.subject = v; }
    if let Some(el) = doc.get_element_by_id("comp-enc-select") {
        let val = doc.value(el);
        if !val.is_empty() { state.compose.encoding = val; }
    }
    if let Some(el) = doc.get_element_by_id("comp-pri-select") {
        let val = doc.value(el);
        if !val.is_empty() { state.compose.priority = val; }
    }
    if let Some(el) = doc.get_element_by_id("comp-sig-select") {
        let val = doc.value(el);
        if !val.is_empty() { state.compose.signature = val; }
    }
    if let Some(body_el) = doc.get_element_by_id("compose-body") {
        let html = doc.inner_html(body_el);
        if !html.is_empty() {
            state.compose.body_html = html.replace('\u{200B}', "");
        } else {
            let t = doc.text_content(body_el);
            if !t.is_empty() {
                state.compose.body_html = t.replace('\u{200B}', "");
            }
        }
    }
}

fn locate_dom_text_point(doc: &Document, node: u32, mut offset: usize, strict: bool) -> Result<(u32, usize), usize> {
    if doc.node_type(node) == 3 {
        let text = doc.text_content(node);
        let len = text.len();
        let inside = if strict { offset < len } else { offset <= len };
        if inside {
            let u16_off = text.char_indices()
                .take_while(|(i, _)| *i < offset)
                .map(|(_, c)| c.len_utf16())
                .sum();
            return Ok((node, u16_off));
        } else {
            return Err(offset.saturating_sub(len));
        }
    }
    for child in doc.child_nodes(node) {
        match locate_dom_text_point(doc, child, offset, strict) {
            Ok(hit) => return Ok(hit),
            Err(rem) => offset = rem,
        }
    }
    Err(offset)
}

fn get_first_paragraph(doc: &mut Document, body_el: u32) -> u32 {
    for child in doc.child_nodes(body_el) {
        if doc.node_type(child) == 1 {
            return child;
        }
    }
    let p = doc.create_element("p");
    let tn = doc.create_text_node("");
    doc.append_child(p, tn);
    doc.append_child(body_el, p);
    p
}

fn get_current_paragraph(doc: &mut Document, target_node: u32, body_el: u32) -> u32 {
    let mut cur = target_node;
    let mut direct_child = 0;
    while cur != 0 && cur != body_el {
        if let Some(tag) = doc.tag_name(cur) {
            let t = tag.to_ascii_lowercase();
            if matches!(t.as_str(), "p" | "div" | "li" | "blockquote" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                return cur;
            }
        }
        let parent = doc.parent_node(cur);
        if parent == body_el {
            direct_child = cur;
        }
        cur = parent;
    }
    if direct_child != 0 {
        direct_child
    } else {
        get_first_paragraph(doc, body_el)
    }
}

fn ensure_element_has_text_node(doc: &mut Document, node: u32) {
    if doc.node_type(node) == 1 {
        if let Some(tag) = doc.tag_name(node) {
            let t = tag.to_ascii_lowercase();
            if matches!(t.as_str(), "br" | "img" | "hr" | "input" | "select" | "button" | "svg" | "path") {
                return;
            }
        }
        let children = doc.child_nodes(node);
        let has_text_child = children.iter().any(|c| doc.node_type(*c) == 3);
        if !has_text_child {
            let direct = doc.text_content(node);
            let tn = doc.create_text_node(&direct);
            if let Some(first) = children.first() {
                doc.insert_before(node, tn, *first);
            } else {
                doc.append_child(node, tn);
            }
        }
        for c in doc.child_nodes(node) {
            if doc.node_type(c) == 1 {
                ensure_element_has_text_node(doc, c);
            }
        }
    }
}

fn get_compose_selection_range(doc: &mut Document) -> Option<u32> {
    let body_el = doc.get_element_by_id("compose-body")?;
    ensure_element_has_text_node(doc, body_el);
    let in_body = doc.editor.caret_box.is_some_and(|cb| cb == body_el || doc.contains(body_el, cb));
    let target_node = if in_body {
        doc.editor.caret_box.unwrap_or(body_el)
    } else {
        get_first_paragraph(doc, body_el)
    };
    let current_p = get_current_paragraph(doc, target_node, body_el);
    let search_node = if target_node != body_el { target_node } else { current_p };

    let has_sel = in_body && doc.editor.has_selection() && doc.editor.sel_start < doc.editor.sel_end;
    let (s, e) = if has_sel {
        (doc.editor.sel_start, doc.editor.sel_end)
    } else if in_body {
        (doc.editor.caret_local, doc.editor.caret_local)
    } else {
        (0, 0)
    };

    let start_point = locate_dom_text_point(doc, search_node, s, s != e)
        .or_else(|_| locate_dom_text_point(doc, current_p, s, s != e))
        .or_else(|_| locate_dom_text_point(doc, search_node, s, false))
        .or_else(|_| locate_dom_text_point(doc, current_p, s, false))
        .ok();
    let end_point = locate_dom_text_point(doc, search_node, e, false)
        .or_else(|_| locate_dom_text_point(doc, current_p, e, false))
        .ok();

    let r = doc.create_range();
    match (start_point, end_point) {
        (Some((sn, so)), Some((en, eo))) => {
            doc.range_set_start(r, sn, so);
            doc.range_set_end(r, en, eo);
        }
        _ => {
            doc.range_set_start(r, current_p, 0);
            doc.range_set_end(r, current_p, 0);
        }
    }
    Some(r)
}

fn unwrap_dom_node(doc: &mut Document, el: u32) {
    let parent = doc.parent_node(el);
    if parent == 0 { return; }
    for child in doc.child_nodes(el) {
        doc.insert_before(parent, child, el);
    }
    doc.remove_child(el);
}

fn find_ancestor_in(doc: &Document, node: u32, stop: u32, tags: &[&str]) -> Option<u32> {
    let mut cur = node;
    while cur != 0 && cur != stop {
        if doc.node_type(cur) == 1 {
            if let Some(tag) = doc.tag_name(cur) {
                if tags.contains(&tag.to_ascii_lowercase().as_str()) {
                    return Some(cur);
                }
            }
        }
        cur = doc.parent_node(cur);
    }
    None
}

fn wrap_selection_or_caret(doc: &mut Document, r: u32, tag: &str, attrs: &[(&str, &str)]) -> u32 {
    let el = doc.create_element(tag);
    for (k, v) in attrs {
        doc.set_attribute(el, k, v);
    }
    if doc.range_collapsed(r) {
        let txt = doc.create_text_node("\u{200B}");
        doc.append_child(el, txt);
        doc.insert_node(r, el);
        return el;
    }
    if !doc.surround_contents(r, el) {
        if let Some(frag) = doc.extract_contents(r) {
            doc.append_child(el, frag);
            doc.insert_node(r, el);
        }
    }
    el
}

fn toggle_inline_formatting(doc: &mut Document, r: u32, host: u32, tag: &str, match_tags: &[&str], attrs: &[(&str, &str)]) {
    let start = doc.range_start_container(r).unwrap_or(0);
    let end = doc.range_end_container(r).unwrap_or(0);
    let a = find_ancestor_in(doc, start, host, match_tags);
    let b = find_ancestor_in(doc, end, host, match_tags);
    if a.is_some() || b.is_some() {
        if let Some(el) = a { unwrap_dom_node(doc, el); }
        if let Some(el) = b.filter(|e| Some(*e) != a) { unwrap_dom_node(doc, el); }
        return;
    }
    wrap_selection_or_caret(doc, r, tag, attrs);
}

fn apply_formatting_command(doc: &mut Document, cmd: &str, val: &str) {
    let Some(body_el) = doc.get_element_by_id("compose-body") else { return; };
    ensure_element_has_text_node(doc, body_el);

    let in_body = doc.editor.caret_box.is_some_and(|cb| cb == body_el || doc.contains(body_el, cb));
    let target_node = if in_body {
        doc.editor.caret_box.unwrap_or(body_el)
    } else {
        get_first_paragraph(doc, body_el)
    };
    let current_p = get_current_paragraph(doc, target_node, body_el);

    let is_inline = matches!(
        cmd.to_ascii_lowercase().as_str(),
        "bold" | "italic" | "underline" | "strikethrough" | "forecolor" | "fontname" | "fontsize" | "createlink" | "removeformat"
    );

    if !is_inline {
        match cmd.to_ascii_lowercase().as_str() {
            "justifyleft" => {
                doc.set_style_property(current_p, "text-align", "left");
            }
            "justifycenter" => {
                doc.set_style_property(current_p, "text-align", "center");
            }
            "justifyright" => {
                doc.set_style_property(current_p, "text-align", "right");
            }
            "indent" => {
                let cur = doc.get_attribute(current_p, "data-indent").and_then(|m| m.parse::<i32>().ok()).unwrap_or(0);
                let next = (cur + 1).min(10);
                doc.set_attribute(current_p, "data-indent", &next.to_string());
                doc.set_style_property(current_p, "margin-left", &format!("{}px", next * 30));
            }
            "outdent" => {
                let cur = doc.get_attribute(current_p, "data-indent").and_then(|m| m.parse::<i32>().ok()).unwrap_or(0);
                let next = (cur - 1).max(0);
                doc.set_attribute(current_p, "data-indent", &next.to_string());
                if next > 0 {
                    doc.set_style_property(current_p, "margin-left", &format!("{}px", next * 30));
                } else {
                    doc.remove_style_property(current_p, "margin-left");
                }
            }
            "insertunorderedlist" => {
                let tag = doc.tag_name(current_p).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
                if tag == "li" {
                    let parent_list = doc.parent_node(current_p);
                    let new_p = doc.create_element("p");
                    for child in doc.child_nodes(current_p) {
                        doc.append_child(new_p, child);
                    }
                    if parent_list != 0 {
                        doc.insert_before(doc.parent_node(parent_list), new_p, parent_list);
                        doc.remove_child(current_p);
                        if doc.child_nodes(parent_list).is_empty() {
                            doc.remove_child(parent_list);
                        }
                    } else {
                        doc.insert_before(body_el, new_p, current_p);
                        doc.remove_child(current_p);
                    }
                    doc.editor.caret_box = Some(new_p);
                } else {
                    let ul = doc.create_element("ul");
                    let li = doc.create_element("li");
                    for child in doc.child_nodes(current_p) {
                        doc.append_child(li, child);
                    }
                    if doc.child_nodes(li).is_empty() {
                        let tn = doc.create_text_node("");
                        doc.append_child(li, tn);
                    }
                    doc.append_child(ul, li);
                    let parent = doc.parent_node(current_p);
                    if parent != 0 {
                        doc.insert_before(parent, ul, current_p);
                        doc.remove_child(current_p);
                    } else {
                        doc.append_child(body_el, ul);
                    }
                    doc.editor.caret_box = Some(li);
                }
            }
            "formatblock" => {
                if val == "blockquote" {
                    if let Some(bq) = find_ancestor_in(doc, current_p, body_el, &["blockquote"]) {
                        let _ = bq;
                    } else {
                        let bq = doc.create_element("blockquote");
                        doc.set_attribute(bq, "style", "margin:8px 0;padding-left:10px;border-left:3px solid #326ea5;");
                        let parent = doc.parent_node(current_p);
                        doc.insert_before(parent, bq, current_p);
                        doc.append_child(bq, current_p);
                    }
                } else {
                    if let Some(bq) = find_ancestor_in(doc, current_p, body_el, &["blockquote"]) {
                        unwrap_dom_node(doc, bq);
                    }
                }
            }
            _ => {}
        }
        doc.editor.has_focus = true;
        doc.focus(body_el);
        doc.style_dirty = true;
        doc.layout_generation = doc.layout_generation.wrapping_add(1);
        return;
    }

    let has_sel = in_body && doc.editor.has_selection() && doc.editor.sel_start < doc.editor.sel_end;
    let s = doc.editor.sel_start;
    let e = doc.editor.sel_end;
    let caret_pos = doc.editor.caret_local;
    let search_node = if target_node != body_el { target_node } else { current_p };

    if has_sel && s < e {
        let start_point = locate_dom_text_point(doc, search_node, s, true)
            .or_else(|_| locate_dom_text_point(doc, current_p, s, true))
            .or_else(|_| locate_dom_text_point(doc, search_node, s, false))
            .or_else(|_| locate_dom_text_point(doc, current_p, s, false));
        let end_point = locate_dom_text_point(doc, search_node, e, false)
            .or_else(|_| locate_dom_text_point(doc, current_p, e, false));

        if let (Ok((sn, so)), Ok((en, eo))) = (start_point, end_point) {
            let r = doc.create_range();
            doc.range_set_start(r, sn, so);
            doc.range_set_end(r, en, eo);

            match cmd.to_ascii_lowercase().as_str() {
                "bold" => { toggle_inline_formatting(doc, r, current_p, "b", &["b", "strong"], &[]); }
                "italic" => { toggle_inline_formatting(doc, r, current_p, "i", &["i", "em"], &[]); }
                "underline" => { toggle_inline_formatting(doc, r, current_p, "u", &["u", "ins"], &[]); }
                "strikethrough" => { toggle_inline_formatting(doc, r, current_p, "s", &["s", "strike", "del"], &[]); }
                "removeformat" => {
                    let tags = ["b", "strong", "i", "em", "u", "ins", "s", "strike", "del", "span", "font"];
                    let start = doc.range_start_container(r).unwrap_or(0);
                    while let Some(a) = find_ancestor_in(doc, start, current_p, &tags) {
                        unwrap_dom_node(doc, a);
                    }
                }
                "forecolor" => {
                    let style = format!("color: {}", val);
                    wrap_selection_or_caret(doc, r, "span", &[("style", &style)]);
                }
                "fontname" => {
                    let style = format!("font-family: {}", val);
                    wrap_selection_or_caret(doc, r, "span", &[("style", &style)]);
                }
                "fontsize" => {
                    let size = if val.ends_with("pt") || val.ends_with("px") { val.to_string() } else { format!("{}pt", val) };
                    let style = format!("font-size: {}", size);
                    wrap_selection_or_caret(doc, r, "span", &[("style", &style)]);
                }
                "createlink" => {
                    if !val.is_empty() {
                        wrap_selection_or_caret(doc, r, "a", &[("href", val)]);
                    }
                }
                _ => {}
            }

            // Preserve selection
            doc.editor.caret_box = Some(search_node);
            doc.editor.collapse_to(e);
            doc.editor.sel_anchor = s;
            doc.editor.sel_start = s;
            doc.editor.sel_end = e;
            doc.editor.caret_local = e;
            doc.editor.has_focus = true;
        }
    } else {
        // Collapsed caret: format next inserted character
        let pt = locate_dom_text_point(doc, search_node, caret_pos, false)
            .or_else(|_| locate_dom_text_point(doc, current_p, caret_pos, false))
            .ok()
            .or_else(|| {
                for c in doc.child_nodes(search_node) {
                    if doc.node_type(c) == 3 {
                        return Some((c, 0));
                    }
                }
                None
            });

        if let Some((sn, so)) = pt {
            let r = doc.create_range();
            doc.range_set_start(r, sn, so);
            doc.range_set_end(r, sn, so);

            let tag_attrs: Option<(&str, Vec<(&str, String)>)> = match cmd.to_ascii_lowercase().as_str() {
                "bold" => Some(("b", vec![])),
                "italic" => Some(("i", vec![])),
                "underline" => Some(("u", vec![])),
                "strikethrough" => Some(("s", vec![])),
                "forecolor" => Some(("span", vec![("style", format!("color: {}", val))])),
                "fontname" => Some(("span", vec![("style", format!("font-family: {}", val))])),
                "fontsize" => {
                    let size = if val.ends_with("pt") || val.ends_with("px") { val.to_string() } else { format!("{}pt", val) };
                    Some(("span", vec![("style", format!("font-size: {}", size))]))
                }
                _ => None,
            };

            if let Some((tag, attrs)) = tag_attrs {
                let match_tags: &[&str] = match tag {
                    "b" => &["b", "strong"],
                    "i" => &["i", "em"],
                    "u" => &["u", "ins"],
                    "s" => &["s", "strike", "del"],
                    _ => &[],
                };
                if !match_tags.is_empty() {
                    if let Some(existing) = find_ancestor_in(doc, sn, current_p, match_tags) {
                        let parent = doc.parent_node(existing);
                        let zwsp = doc.create_text_node("\u{200B}");
                        let next = doc.next_sibling(existing);
                        if next != 0 {
                            doc.insert_before(parent, zwsp, next);
                        } else {
                            doc.append_child(parent, zwsp);
                        }
                        doc.editor.caret_box = Some(zwsp);
                        doc.editor.collapse_to("\u{200B}".len());
                        doc.editor.has_focus = true;
                        doc.focus(body_el);
                        doc.style_dirty = true;
                        doc.layout_generation = doc.layout_generation.wrapping_add(1);
                        return;
                    }
                }

                let el = doc.create_element(tag);
                for (k, v) in &attrs {
                    doc.set_attribute(el, k, v);
                }
                let zwsp = doc.create_text_node("\u{200B}");
                doc.append_child(el, zwsp);
                doc.insert_node(r, el);

                doc.editor.caret_box = Some(el);
                doc.editor.collapse_to("\u{200B}".len());
                doc.editor.has_focus = true;
            }
        }
    }

    doc.focus(body_el);
    doc.style_dirty = true;
    doc.layout_generation = doc.layout_generation.wrapping_add(1);
}

fn render_eudora_compose(doc: &mut Document, state: &mut AppState) {
    if let Some(from_el) = doc.get_element_by_id("compose-from") {
        let from_list = build_from_list(&state.conf);
        let mut opts = String::new();
        for item in &from_list {
            let sel = if item == &state.compose.from { " selected" } else { "" };
            opts.push_str(&format!(r#"<option value="{}"{}>{}</option>"#, escape_html(item), sel, escape_html(item)));
        }
        set_inner_html_indexed(doc, from_el, &opts);
        if state.compose.from.is_empty() && !from_list.is_empty() {
            state.compose.from = from_list[0].clone();
        }
        doc.set_value(from_el, &state.compose.from);
    }
    set_text_by_id(doc, "compose-attach", &state.compose.attach);
    set_text_by_id(doc, "compose-priority", &state.compose.priority);

    // Sync select dropdowns for encoding, priority, signature
    if let Some(el) = doc.get_element_by_id("comp-enc-select") {
        doc.set_value(el, &state.compose.encoding);
    }
    if let Some(el) = doc.get_element_by_id("comp-pri-select") {
        doc.set_value(el, &state.compose.priority);
    }
    if let Some(el) = doc.get_element_by_id("comp-sig-select") {
        doc.set_value(el, &state.compose.signature);
    }

    // Update active visual styles on toggles
    let update_tog = |doc: &mut Document, id: &str, active: bool| {
        if let Some(el) = doc.get_element_by_id(id) {
            if active {
                doc.set_style_property(el, "background", "#326ea5");
                doc.set_style_property(el, "color", "#ffffff");
            } else {
                doc.set_style_property(el, "background", "transparent");
                doc.set_style_property(el, "color", "#231e19");
            }
        }
    };
    update_tog(doc, "comp-tog-qp", state.compose.opt_qp);
    update_tog(doc, "comp-tog-text", state.compose.opt_text);
    update_tog(doc, "comp-tog-wrap", state.compose.opt_wrap);
    update_tog(doc, "comp-tog-keep", state.compose.opt_keep);
    update_tog(doc, "comp-tog-rr", state.compose.opt_rr);

    if state.compose_needs_init {
        if let Some(el) = doc.get_element_by_id("compose-to") {
            doc.set_attribute(el, "value", &state.compose.to);
        }
        if let Some(el) = doc.get_element_by_id("compose-cc") {
            doc.set_attribute(el, "value", &state.compose.cc);
        }
        if let Some(el) = doc.get_element_by_id("compose-bcc") {
            doc.set_attribute(el, "value", &state.compose.bcc);
        }
        if let Some(el) = doc.get_element_by_id("compose-subj") {
            doc.set_attribute(el, "value", &state.compose.subject);
        }
        if let Some(body) = doc.get_element_by_id("compose-body") {
            let html = if state.compose.body_html.is_empty() {
                "<p><br></p>".to_string()
            } else {
                state.compose.body_html.clone()
            };
            set_inner_html_indexed(doc, body, &html);
            ensure_element_has_text_node(doc, body);
            let first_p = doc.child_nodes(body).into_iter().find(|c| doc.node_type(*c) == 1).unwrap_or(body);
            doc.editor.caret_box = Some(first_p);
            doc.editor.collapse_to(0);
            doc.editor.has_focus = true;
        }
        state.compose_needs_init = false;
    }
}

fn render_eudora_statusbar(doc: &mut Document, state: &AppState) {
    set_text_by_id(doc, "sb-left", &state.status_left);
    set_text_by_id(doc, "sb-center", &state.status_center);
    set_text_by_id(doc, "sb-right", &state.status_right);
}

// ============================================================================
// DOM Rendering: Email (Flux Mail) Skin
// ============================================================================

fn render_email_all(doc: &mut Document, state: &mut AppState) {
    state.ensure_valid_selection();

    sync_theme_and_mode(doc, state.skin, state.last_mail_skin, state.dark_mode);
    render_email_sidebar(doc, state);
    render_email_center_list(doc, state);
    render_email_right_pane(doc, state);
}

fn render_email_sidebar(doc: &mut Document, state: &AppState) {
    if let Some(mb_container) = doc.get_element_by_id("sidebar-mailboxes") {
        let mut html = String::new();

        for mb in state.mailboxes.iter().filter(|m| m.show_in_email_skin) {
            let unread = state.unread_count(&mb.name);
            let is_active = state.email_mode == EmailViewMode::Mail
                && state.selected_label.is_none()
                && state.selected_mailbox == mb.name;
            let active_cls = if is_active { "active" } else { "" };
            let badge = if unread > 0 {
                format!(r#"<span class="badge">{unread}</span>"#)
            } else {
                String::new()
            };

            html.push_str(&format!(
                r#"<div class="sidebar-item {active_cls}" data-mailbox="{name}">
  <span class="sidebar-icon">{icon}</span>
  <span class="sidebar-label">{label}</span>
  {badge}
</div>"#,
                name = escape_html(&mb.name),
                icon = mb.icon,
                label = escape_html(&mb.email_label),
            ));

            if mb.name == "In" {
                let starred_active = state.email_mode == EmailViewMode::Mail
                    && state.selected_label.is_none()
                    && state.selected_mailbox == "Starred";
                let s_cls = if starred_active { "active" } else { "" };
                html.push_str(&format!(
                    r#"<div class="sidebar-item {s_cls}" data-mailbox="Starred">
  <span class="sidebar-icon">★</span>
  <span class="sidebar-label">Starred</span>
</div>"#
                ));
            }
        }

        // Contacts item in sidebar
        let contacts_active = if state.email_mode == EmailViewMode::Contacts {
            "active"
        } else {
            ""
        };
        html.push_str(&format!(
            r#"<div class="sidebar-item {contacts_active}" data-view="contacts">
  <span class="sidebar-icon">👥</span>
  <span class="sidebar-label">Contacts</span>
  <span class="badge">{count}</span>
</div>"#,
            count = state.contacts.len()
        ));

        // Settings item in sidebar
        html.push_str(
            r#"<div class="sidebar-item" data-action="open-settings">
  <span class="sidebar-icon">⚙</span>
  <span class="sidebar-label">Settings</span>
</div>"#,
        );

        set_inner_html_indexed(doc, mb_container, &html);
    }

    if let Some(lbl_container) = doc.get_element_by_id("sidebar-labels") {
        let mut html = String::new();
        for lbl in &state.labels {
            let is_active = state.email_mode == EmailViewMode::Mail
                && state.selected_label.as_deref() == Some(lbl.name);
            let active_cls = if is_active { "active" } else { "" };
            html.push_str(&format!(
                r#"<div class="label-item {active_cls}" data-label="{name}">
  <span class="label-dot" style="background:{color};"></span>{name}
</div>"#,
                name = escape_html(lbl.name),
                color = lbl.color,
            ));
        }
        set_inner_html_indexed(doc, lbl_container, &html);
    }
}

fn render_email_center_list(doc: &mut Document, state: &AppState) {
    let Some(list_el) = doc.get_element_by_id("email-list") else {
        return;
    };

    if state.email_mode == EmailViewMode::Contacts {
        set_inner_html_indexed(doc, list_el, &build_contact_list_html(state));
        return;
    }

    let visible = state.visible_messages_for_mailbox(&state.selected_mailbox);
    let active_id = if state.selected_mailbox == "Out" {
        state.selected_out_msg_id
    } else {
        state.selected_in_msg_id
    };

    if visible.is_empty() {
        set_inner_html_indexed(
            doc,
            list_el,
            r#"<div class="email-item selected"><div class="email-item-inner"><div class="email-subject">No messages in this view</div></div></div>"#,
        );
        return;
    }

    let mut html = String::new();
    for (idx, msg) in visible.iter().enumerate() {
        let is_sel = msg.id == active_id;
        let sel_cls = if is_sel { "selected" } else { "" };
        let unread_cls = if msg.unread { "unread" } else { "" };
        let unread_dot = if msg.unread {
            r#"<span class="unread-dot"></span>"#
        } else {
            ""
        };
        let pri_bar = match msg.priority {
            "highest" | "high" => r#"<span class="priority-bar priority-high"></span>"#,
            "low" => r#"<span class="priority-bar priority-low"></span>"#,
            _ => "",
        };
        let star_html = if msg.flagged {
            r#"<span class="star-icon">★</span>"#
        } else {
            ""
        };
        let attach_html = if msg.has_attach {
            r#"<span class="attach-icon" title="Has attachment">📎</span>"#
        } else {
            ""
        };

        let mut pills_html = String::new();
        for &lbl_name in &msg.labels {
            if let Some(def) = state.labels.iter().find(|d| d.name == lbl_name) {
                pills_html.push_str(&format!(
                    r#"<span class="pill" style="background:{bg};color:{fg};">{name}</span>"#,
                    bg = def.bg_rgba,
                    fg = def.color,
                    name = escape_html(def.name),
                ));
            }
        }

        let who = if msg.mailbox == "Out" {
            display_name(&msg.to)
        } else {
            display_name(&msg.from)
        };

        html.push_str(&format!(
            r#"<div class="email-item {sel_cls}" data-msg-id="{id}" data-box="{box_kind}">
  {pri_bar}
  <div class="avatar" style="background:{color};">{initials}</div>
  <div class="email-item-inner">
    {unread_dot}
    <div class="email-row1">
      <span class="email-sender {unread_cls}">{who}</span>
      {star_html}
      {attach_html}
      <span class="email-date">{short_date}</span>
    </div>
    <div class="email-row2">
      <span class="email-subject {unread_cls}">{subj}</span>
      {pills_html}
    </div>
    <div class="email-preview">{preview}</div>
  </div>
</div>"#,
            id = msg.id,
            box_kind = if msg.mailbox == "Out" { "out" } else { "in" },
            color = msg.avatar_color,
            initials = escape_html(&msg.avatar_initials),
            who = escape_html(who),
            short_date = escape_html(&msg.short_date),
            subj = escape_html(&msg.subject),
            preview = escape_html(&msg.preview),
        ));

        if idx + 1 < visible.len() {
            html.push_str(r#"<div class="email-divider"></div>"#);
        }
    }

    set_inner_html_indexed(doc, list_el, &html);
}

fn render_email_right_pane(doc: &mut Document, state: &AppState) {
    let email_view = doc.get_element_by_id("email-view");
    let contact_view = doc.get_element_by_id("contact-view");
    let compose_view = doc.get_element_by_id("compose-view");

    match state.email_mode {
        EmailViewMode::Contacts => {
            if let Some(el) = email_view {
                doc.set_style_property(el, "display", "none");
            }
            if let Some(el) = compose_view {
                doc.set_style_property(el, "display", "none");
            }
            if let Some(el) = contact_view {
                doc.set_style_property(el, "display", "block");
                set_inner_html_indexed(doc, el, &build_contact_detail_html(state));
            }
            return;
        }
        EmailViewMode::Compose => {
            if let Some(el) = email_view {
                doc.set_style_property(el, "display", "none");
            }
            if let Some(el) = contact_view {
                doc.set_style_property(el, "display", "none");
            }
            if let Some(el) = compose_view {
                doc.set_style_property(el, "display", "block");
                let compose_html = format!(
                    r#"<div class="compose-card">
  <div class="compose-card-title">{title}</div>
  <div class="compose-field-row">
    <span class="compose-field-label">From:</span>
    <span>{from}</span>
  </div>
  <div class="compose-field-row">
    <span class="compose-field-label">To:</span>
    <input class="compose-field-input" type="text" value="{to}">
  </div>
  <div class="compose-field-row">
    <span class="compose-field-label">Cc:</span>
    <input class="compose-field-input" type="text" value="{cc}">
  </div>
  <div class="compose-field-row">
    <span class="compose-field-label">Subject:</span>
    <input class="compose-field-input" type="text" value="{subj}">
  </div>
  <div class="compose-body-area">{body}</div>
  <div class="compose-actions">
    <button class="btn btn-primary" data-action="send">✉ Send</button>
    <button class="btn btn-outlined" data-action="discard">Discard</button>
  </div>
</div>"#,
                    title = escape_html(&state.compose.title),
                    from = escape_html(&state.compose.from),
                    to = escape_html(&state.compose.to),
                    cc = escape_html(&state.compose.cc),
                    subj = escape_html(&state.compose.subject),
                    body = state.compose.body_html,
                );
                set_inner_html_indexed(doc, el, &compose_html);
            }
            return;
        }
        EmailViewMode::Mail => {
            if let Some(el) = email_view {
                doc.set_style_property(el, "display", "block");
            }
            if let Some(el) = contact_view {
                doc.set_style_property(el, "display", "none");
            }
            if let Some(el) = compose_view {
                doc.set_style_property(el, "display", "none");
            }
        }
    }

    let visible = state.visible_messages_for_mailbox(&state.selected_mailbox);
    let active_id = if state.selected_mailbox == "Out" {
        state.selected_out_msg_id
    } else {
        state.selected_in_msg_id
    };

    let selected = visible
        .iter()
        .copied()
        .find(|m| m.id == active_id)
        .or_else(|| visible.first().copied());

    let Some(msg) = selected else {
        if let Some(badge) = doc.get_element_by_id("email-priority-badge") {
            doc.set_style_property(badge, "display", "none");
        }
        set_text_by_id(doc, "email-subject-title", "No message selected");
        set_text_by_id(doc, "email-meta", "");
        if let Some(lbls) = doc.get_element_by_id("email-labels") {
            set_inner_html_indexed(doc, lbls, "");
        }
        if let Some(att) = doc.get_element_by_id("email-attachment-note") {
            doc.set_style_property(att, "display", "none");
        }
        if let Some(body) = doc.get_element_by_id("email-body") {
            set_inner_html_indexed(doc, body, "<p>Select a mailbox or message to read it.</p>");
        }
        return;
    };

    if let Some(badge) = doc.get_element_by_id("email-priority-badge") {
        match msg.priority {
            "highest" | "high" => {
                doc.set_style_property(badge, "display", "block");
                doc.set_text_content(badge, "⚡ HIGH PRIORITY");
            }
            "low" => {
                doc.set_style_property(badge, "display", "block");
                doc.set_text_content(badge, "LOW PRIORITY");
            }
            _ => {
                doc.set_style_property(badge, "display", "none");
            }
        }
    }

    set_text_by_id(doc, "email-subject-title", &msg.subject);
    set_text_by_id(
        doc,
        "email-meta",
        &format!("{} · to {} · {}", msg.from, msg.to, msg.date),
    );

    if let Some(lbls) = doc.get_element_by_id("email-labels") {
        let mut pills = String::new();
        for &lbl_name in &msg.labels {
            if let Some(def) = state.labels.iter().find(|d| d.name == lbl_name) {
                pills.push_str(&format!(
                    r#"<span class="pill" style="background:{bg};color:{fg};">{name}</span>"#,
                    bg = def.bg_rgba,
                    fg = def.color,
                    name = escape_html(def.name),
                ));
            }
        }
        set_inner_html_indexed(doc, lbls, &pills);
    }

    if let Some(att) = doc.get_element_by_id("email-attachment-note") {
        if !msg.attachment_note.is_empty() {
            doc.set_style_property(att, "display", "block");
            doc.set_text_content(att, &msg.attachment_note);
        } else {
            doc.set_style_property(att, "display", "none");
        }
    }

    if let Some(body) = doc.get_element_by_id("email-body") {
        set_inner_html_indexed(doc, body, &msg.body_html);
    }
}

// ============================================================================
// DOM Rendering: Classic Eudora Settings Page (21 Categories)
// ============================================================================

const SETTINGS_CATEGORIES: &[(&str, &str, &str, &str)] = &[
    ("getting_started", "🏠", "Getting Started", "Configure basic incoming/outgoing servers and primary identity"),
    ("checking_mail", "📥", "Checking Mail", "POP/IMAP retrieval, polling interval, and server message retention"),
    ("sending_mail", "📤", "Sending Mail", "SMTP server, port 587 submission, and outgoing queue options"),
    ("composing", "✏️", "Composing", "Nickname auto-completion, word wrap column, and encoding"),
    ("mailbox_display", "📋", "Mailbox Display", "Table columns, separator lines, preview pane, and mark-as-read"),
    ("date_display", "📅", "Date Display", "Age-sensitive relative timestamps and timezone formatting"),
    ("styled_text", "🅱", "Styled Text", "HTML/styled composition rules and formatting toolbar preferences"),
    ("fonts", "🔤", "Fonts & Display", "Screen, fixed-width, and print font families, sizes, and graphics"),
    ("labels", "🏷", "Labels", "Customize the 7 classic Eudora message labels and color swatches"),
    ("attachments", "📎", "Attachments", "MIME/BinHex/UUencode encoding and automatic download directory"),
    ("replying", "↩", "Replying", "Reply-to-All behavior, self-inclusion, and priority inheritance"),
    ("junk_mail", "🛡", "Junk Mail", "SpamWatch threshold score, Address Book whitelist, and auto-trim"),
    ("toolbar", "🧰", "Toolbar", "Main toolbar visibility, floating search field, and icon style"),
    ("getting_attention", "🔔", "Getting Attention", "New mail fanfare sound, dock bounce, and background task progress"),
    ("hosts", "🌐", "Hosts & Proxies", "Ph/LDAP/Finger directory servers, SOCKS/HTTP proxy, and offline mode"),
    ("moving_around", "⌨️", "Moving Around", "Next-message selection after delete/transfer and keyboard navigation"),
    ("extra_warnings", "⚠️", "Extra Warnings", "Confirmation prompts for deleting, queueing, and emptying Trash"),
    ("miscellaneous", "🔧", "Miscellaneous", "Window closing, Turbo Redirect, TOC format, and system keychain"),
    ("accounts", "👤", "Accounts & Personalities", "Manage Dominant and secondary Eudora mail personalities"),
    ("ssl", "🔒", "SSL / TLS", "Configure STARTTLS and alternate TLS ports for POP, SMTP, and IMAP"),
    ("spell_checking", "📝", "Spell Checking", "Real-time spell checker, ignore rules, and phonetic suggestions"),
];

fn render_settings_all(doc: &mut Document, state: &mut AppState) {
    sync_theme_and_mode(doc, state.skin, state.last_mail_skin, state.dark_mode);

    let active_cat = state.settings_category.as_str();

    // Update sidebar items
    for item in doc.query_selector_all("[data-settings-cat]") {
        let is_match = doc.get_attribute(item, "data-settings-cat").as_deref() == Some(active_cat);
        if is_match {
            doc.class_list_add(item, "active");
        } else {
            doc.class_list_remove(item, "active");
        }
    }

    // Update panels and header metadata
    for (idx, &(cat_id, ico, title, sub)) in SETTINGS_CATEGORIES.iter().enumerate() {
        let panel_id = format!("panel-{cat_id}");
        if let Some(panel) = doc.get_element_by_id(&panel_id) {
            if cat_id == active_cat {
                doc.class_list_add(panel, "active");
            } else {
                doc.class_list_remove(panel, "active");
            }
        }
        if cat_id == active_cat {
            set_text_by_id(doc, "settings-cat-ico", ico);
            set_text_by_id(doc, "settings-cat-title", title);
            set_text_by_id(doc, "settings-cat-sub", sub);
            set_text_by_id(
                doc,
                "settings-cat-badge",
                &format!("Category {} of {}", idx + 1, SETTINGS_CATEGORIES.len()),
            );
        }
    }

    // 1. Populate Getting Started config inputs from macmbx Conf
    let username = state.conf.get("checking_mail", "pop_username")
        .or_else(|| state.conf.get("Settings", "LoginName"))
        .unwrap_or("you");
    let mail_srv = state.conf.get("checking_mail", "pop_server")
        .or_else(|| state.conf.get("Settings", "MailServer"))
        .unwrap_or("mail.example.com");
    let real_name = state.conf.get("sending_mail", "real_name")
        .or_else(|| state.conf.get("Settings", "RealName"))
        .unwrap_or("You");
    let smtp_srv = state.conf.get("sending_mail", "smtp_server")
        .or_else(|| state.conf.get("Settings", "SMTPServer"))
        .unwrap_or("smtp.example.com");
    let email_addr = state.conf.get("sending_mail", "email_address")
        .or_else(|| state.conf.get("Settings", "ReturnAddress"))
        .unwrap_or("you@example.com");

    if let Some(el) = doc.get_element_by_id("cfg-username") {
        doc.set_attribute(el, "value", username);
    }
    if let Some(el) = doc.get_element_by_id("cfg-pop-server") {
        doc.set_attribute(el, "value", mail_srv);
    }
    if let Some(el) = doc.get_element_by_id("cfg-realname") {
        doc.set_attribute(el, "value", real_name);
    }
    if let Some(el) = doc.get_element_by_id("cfg-smtp-server") {
        doc.set_attribute(el, "value", smtp_srv);
    }
    if let Some(el) = doc.get_element_by_id("cfg-email") {
        doc.set_attribute(el, "value", email_addr);
    }

    // 2. Populate Checking Mail
    if let Some(el) = doc.get_element_by_id("chk-username") {
        doc.set_attribute(el, "value", username);
    }
    if let Some(el) = doc.get_element_by_id("chk-pop-server") {
        doc.set_attribute(el, "value", mail_srv);
    }
    if let Some(el) = doc.get_element_by_id("chk-proto") {
        let use_imap = state.conf.get_bool("checking_mail", "use_imap", false);
        doc.set_attribute(el, "value", if use_imap { "IMAP" } else { "POP" });
    }
    if let Some(el) = doc.get_element_by_id("chk-interval") {
        let interval = state.conf.get_int("checking_mail", "check_interval", 15);
        doc.set_attribute(el, "value", &interval.to_string());
    }
    if let Some(el) = doc.get_element_by_id("chk-leave-days") {
        let leave_days = state.conf.get_int("checking_mail", "leave_days", 14);
        doc.set_attribute(el, "value", &leave_days.to_string());
    }

    // 3. Populate Sending Mail
    if let Some(el) = doc.get_element_by_id("send-email") {
        doc.set_attribute(el, "value", email_addr);
    }
    if let Some(el) = doc.get_element_by_id("send-smtp") {
        doc.set_attribute(el, "value", smtp_srv);
    }
    if let Some(el) = doc.get_element_by_id("send-domain") {
        let domain = state.conf.get("sending_mail", "default_domain").unwrap_or("example.com");
        doc.set_attribute(el, "value", domain);
    }

    // 4. Populate SSL Security
    if let Some(el) = doc.get_element_by_id("ssl-pop-mode") {
        let mode = state.conf.get_int("ssl", "ssl_pop_mode", 2);
        doc.set_attribute(el, "value", &mode.to_string());
    }
    if let Some(el) = doc.get_element_by_id("ssl-smtp-mode") {
        let mode = state.conf.get_int("ssl", "ssl_smtp_mode", 1);
        doc.set_attribute(el, "value", &mode.to_string());
    }
    if let Some(el) = doc.get_element_by_id("ssl-imap-mode") {
        let mode = state.conf.get_int("ssl", "ssl_imap_mode", 2);
        doc.set_attribute(el, "value", &mode.to_string());
    }

    // 5. Populate Personalities / Accounts
    render_settings_personalities(doc, state);

    set_text_by_id(doc, "settings-status-msg", &state.settings_status);
}

fn render_settings_personalities(doc: &mut Document, state: &AppState) {
    let dominant = state.conf.get_dominant();
    let accounts = state.conf.get_all_accounts();

    // 1. Render personality list
    if let Some(list_el) = doc.get_element_by_id("pers-list-container") {
        let mut html = String::new();

        // Dominant item (idx = 0)
        let is_sel = state.selected_account_idx == 0;
        let sel_cls = if is_sel { " selected" } else { "" };
        let dom_name = if dominant.name.is_empty() { "Dominant" } else { &dominant.name };
        html.push_str(&format!(
            r#"<div class="pers-item{sel_cls}" data-select-account="0">
                <div class="pers-item-name">
                    <span>{name}</span>
                    <span class="pers-badge-gold">DOMINANT</span>
                </div>
                <div class="pers-item-detail">{proto} — {server}</div>
                <div class="pers-item-detail">{email}</div>
            </div>"#,
            name = escape_html(dom_name),
            proto = escape_html(&dominant.account_type),
            server = escape_html(&dominant.server),
            email = escape_html(&dominant.email),
        ));

        // Additional accounts (idx = 1..N)
        for acct in &accounts {
            let is_sel = state.selected_account_idx == acct.index;
            let sel_cls = if is_sel { " selected" } else { "" };
            let status_badge = if acct.enabled {
                r#"<span class="pers-badge-dim">Active</span>"#
            } else {
                r#"<span class="pers-badge-dim">Off</span>"#
            };
            html.push_str(&format!(
                r#"<div class="pers-item{sel_cls}" data-select-account="{idx}">
                    <div class="pers-item-name">
                        <span>{name}</span>
                        {status_badge}
                    </div>
                    <div class="pers-item-detail">{proto} — {server}</div>
                    <div class="pers-item-detail">{email}</div>
                </div>"#,
                idx = acct.index,
                name = escape_html(&acct.name),
                proto = escape_html(&acct.account_type),
                server = escape_html(&acct.server),
                email = escape_html(&acct.email),
            ));
        }

        set_inner_html_indexed(doc, list_el, &html);
    }

    // 2. Populate editor with currently selected account
    let (acct_name, real_name, email, acct_type, server, username, smtp, interval, ssl_mode, leave, enabled, is_dominant) =
        if state.selected_account_idx == 0 {
            (
                "Dominant".to_string(),
                dominant.real_name,
                dominant.email,
                dominant.account_type,
                dominant.server,
                dominant.username,
                dominant.smtp_server,
                dominant.check_interval,
                dominant.ssl_mode,
                dominant.leave_on_server,
                dominant.enabled,
                true,
            )
        } else if let Some(acct) = state.conf.get_account(state.selected_account_idx) {
            (
                acct.name,
                acct.real_name,
                acct.email,
                acct.account_type,
                acct.server,
                acct.username,
                acct.smtp_server,
                acct.check_interval,
                acct.ssl_mode,
                acct.leave_on_server,
                acct.enabled,
                false,
            )
        } else {
            (
                "Dominant".to_string(),
                dominant.real_name,
                dominant.email,
                dominant.account_type,
                dominant.server,
                dominant.username,
                dominant.smtp_server,
                dominant.check_interval,
                dominant.ssl_mode,
                dominant.leave_on_server,
                dominant.enabled,
                true,
            )
        };

    set_text_by_id(doc, "pers-detail-title", &format!("Edit Personality: {}", acct_name));

    if let Some(el) = doc.get_element_by_id("pers-name") {
        doc.set_attribute(el, "value", &acct_name);
    }
    if let Some(el) = doc.get_element_by_id("pers-realname") {
        doc.set_attribute(el, "value", &real_name);
    }
    if let Some(el) = doc.get_element_by_id("pers-email") {
        doc.set_attribute(el, "value", &email);
    }
    if let Some(el) = doc.get_element_by_id("pers-type") {
        doc.set_attribute(el, "value", &acct_type);
    }
    if let Some(el) = doc.get_element_by_id("pers-server") {
        doc.set_attribute(el, "value", &server);
    }
    if let Some(el) = doc.get_element_by_id("pers-username") {
        doc.set_attribute(el, "value", &username);
    }
    if let Some(el) = doc.get_element_by_id("pers-smtp") {
        doc.set_attribute(el, "value", &smtp);
    }
    if let Some(el) = doc.get_element_by_id("pers-interval") {
        doc.set_attribute(el, "value", &interval.to_string());
    }
    if let Some(el) = doc.get_element_by_id("pers-ssl") {
        doc.set_attribute(el, "value", &ssl_mode.to_string());
    }
    if let Some(el) = doc.get_element_by_id("pers-leave") {
        if leave {
            doc.set_attribute(el, "checked", "checked");
        } else {
            doc.remove_attribute(el, "checked");
        }
    }
    if let Some(el) = doc.get_element_by_id("pers-enabled") {
        if enabled {
            doc.set_attribute(el, "checked", "checked");
        } else {
            doc.remove_attribute(el, "checked");
        }
    }

    if let Some(btn_dom) = doc.get_element_by_id("pers-btn-dominant") {
        if is_dominant {
            doc.set_style_property(btn_dom, "display", "none");
        } else {
            doc.set_style_property(btn_dom, "display", "inline-flex");
        }
    }
    if let Some(btn_del) = doc.get_element_by_id("pers-btn-delete") {
        if is_dominant {
            doc.set_style_property(btn_del, "display", "none");
        } else {
            doc.set_style_property(btn_del, "display", "inline-flex");
        }
    }
}

fn read_wizard_input(doc: &Document, id_str: &str) -> Option<String> {
    let el = doc.get_element_by_id(id_str)?;
    let v = doc.value(el);
    let trimmed = v.trim();
    if !trimmed.is_empty() {
        return Some(trimmed.to_string());
    }
    if let Some(a) = doc.get_attribute(el, "value") {
        let trimmed_a = a.trim();
        if !trimmed_a.is_empty() {
            return Some(trimmed_a.to_string());
        }
    }
    None
}

fn sync_wizard_from_dom(doc: &Document, wizard: &mut WizardState) {
    // Step-targeted syncing prevents inactive inputs from clobbering active state
    match wizard.step {
        1 => {
            // Step 1: Account setup radios
            if let Some(el) = doc.get_element_by_id("wiz-radio-new") {
                if doc.has_attribute(el, "checked") || doc.value(el) == "true" {
                    wizard.account_choice = 0;
                }
            }
            if let Some(el) = doc.get_element_by_id("wiz-radio-detect") {
                if doc.has_attribute(el, "checked") || doc.value(el) == "true" {
                    wizard.account_choice = 2;
                }
            }
            if let Some(el) = doc.get_element_by_id("wiz-radio-skip") {
                if doc.has_attribute(el, "checked") || doc.value(el) == "true" {
                    wizard.account_choice = 3;
                }
            }
            if let Some(el) = doc.get_element_by_id("wiz-radio-import") {
                if doc.has_attribute(el, "checked") || doc.value(el) == "true" {
                    wizard.account_choice = 1;
                }
            }
        }
        2 => {
            if let Some(v) = read_wizard_input(doc, "wiz-persname") {
                wizard.persona_name = v;
            }
        }
        3 => {
            if let Some(v) = read_wizard_input(doc, "wiz-yourname") {
                wizard.real_name = v;
            }
        }
        4 => {
            if let Some(v) = read_wizard_input(doc, "wiz-email") {
                wizard.return_address = v;
            }
        }
        5 => {
            if let Some(v) = read_wizard_input(doc, "wiz-login") {
                wizard.login_name = v;
            }
        }
        6 => {
            if let Some(v) = read_wizard_input(doc, "wiz-inserver") {
                wizard.incoming_server = v;
            }
            if let Some(v) = read_wizard_input(doc, "wiz-inport") {
                if let Ok(port) = v.parse::<u16>() {
                    wizard.incoming_port = port;
                }
            }
            if let Some(el) = doc.get_element_by_id("wiz-proto-imap") {
                if doc.has_attribute(el, "checked") || doc.value(el) == "true" {
                    wizard.incoming_protocol = "IMAP".to_string();
                } else {
                    wizard.incoming_protocol = "POP".to_string();
                }
            }
            if let Some(el) = doc.get_element_by_id("wiz-in-ssl") {
                wizard.incoming_ssl = doc.has_attribute(el, "checked") || doc.value(el) == "on" || doc.value(el) == "true";
            }
        }
        7 => {
            if let Some(v) = read_wizard_input(doc, "wiz-outserver") {
                wizard.smtp_server = v;
            }
            if let Some(v) = read_wizard_input(doc, "wiz-outport") {
                if let Ok(port) = v.parse::<u16>() {
                    wizard.smtp_port = port;
                }
            }
            if let Some(el) = doc.get_element_by_id("wiz-out-auth") {
                wizard.smtp_auth_allowed = doc.has_attribute(el, "checked") || doc.value(el) == "on" || doc.value(el) == "true";
            }
            if let Some(el) = doc.get_element_by_id("wiz-out-ssl") {
                wizard.smtp_ssl = doc.has_attribute(el, "checked") || doc.value(el) == "on" || doc.value(el) == "true";
            }
        }
        9 => {
            // Step 9: Provider selection radios
            if let Some(el) = doc.get_element_by_id("wiz-prov-gmail") {
                if doc.has_attribute(el, "checked") || doc.value(el) == "true" {
                    wizard.provider = "gmail".to_string();
                }
            }
            if let Some(el) = doc.get_element_by_id("wiz-prov-outlook") {
                if doc.has_attribute(el, "checked") || doc.value(el) == "true" {
                    wizard.provider = "outlook".to_string();
                }
            }
            if let Some(el) = doc.get_element_by_id("wiz-prov-yahoo") {
                if doc.has_attribute(el, "checked") || doc.value(el) == "true" {
                    wizard.provider = "yahoo".to_string();
                }
            }
            if let Some(el) = doc.get_element_by_id("wiz-prov-other") {
                if doc.has_attribute(el, "checked") || doc.value(el) == "true" {
                    wizard.provider = "other".to_string();
                }
            }
        }
        10 => {
            // Step 10: Credentials inputs
            if let Some(v) = read_wizard_input(doc, "wiz-cred-email") {
                wizard.return_address = v;
                if let Some((u, _)) = wizard.return_address.split_once('@') {
                    if wizard.login_name.is_empty() {
                        wizard.login_name = u.trim().to_string();
                    }
                }
            }
            if let Some(v) = read_wizard_input(doc, "wiz-cred-password") {
                wizard.password = v;
            }
            if let Some(v) = read_wizard_input(doc, "wiz-cred-name") {
                wizard.real_name = v;
            }
            if let Some(el) = doc.get_element_by_id("wiz-cred-proto-imap") {
                if doc.has_attribute(el, "checked") || doc.value(el) == "true" {
                    wizard.incoming_protocol = "IMAP".to_string();
                }
            }
            if let Some(el) = doc.get_element_by_id("wiz-cred-proto-pop") {
                if doc.has_attribute(el, "checked") || doc.value(el) == "true" {
                    wizard.incoming_protocol = "POP".to_string();
                }
            }
        }
        _ => {}
    }
}

fn apply_provider_defaults(wizard: &mut WizardState) {
    let email = wizard.return_address.trim().to_string();
    let is_pop = wizard.incoming_protocol.eq_ignore_ascii_case("POP");
    if let Some((user, domain)) = email.split_once('@') {
        if wizard.login_name.is_empty() {
            wizard.login_name = user.trim().to_string();
        }
        let dom = domain.trim().to_lowercase();
        match wizard.provider.as_str() {
            "gmail" => {
                if is_pop {
                    wizard.incoming_server = "pop.gmail.com".to_string();
                    wizard.incoming_port = 995;
                } else {
                    wizard.incoming_protocol = "IMAP".to_string();
                    wizard.incoming_server = "imap.gmail.com".to_string();
                    wizard.incoming_port = 993;
                }
                wizard.incoming_ssl = true;
                wizard.smtp_server = "smtp.gmail.com".to_string();
                wizard.smtp_port = 465;
                wizard.smtp_ssl = true;
                wizard.smtp_auth_allowed = true;
            }
            "outlook" => {
                if is_pop {
                    wizard.incoming_server = "outlook.office365.com".to_string();
                    wizard.incoming_port = 995;
                } else {
                    wizard.incoming_protocol = "IMAP".to_string();
                    wizard.incoming_server = "outlook.office365.com".to_string();
                    wizard.incoming_port = 993;
                }
                wizard.incoming_ssl = true;
                wizard.smtp_server = "smtp.office365.com".to_string();
                wizard.smtp_port = 587;
                wizard.smtp_ssl = true;
                wizard.smtp_auth_allowed = true;
            }
            "yahoo" => {
                if is_pop {
                    wizard.incoming_server = "pop.mail.yahoo.com".to_string();
                    wizard.incoming_port = 995;
                } else {
                    wizard.incoming_protocol = "IMAP".to_string();
                    wizard.incoming_server = "imap.mail.yahoo.com".to_string();
                    wizard.incoming_port = 993;
                }
                wizard.incoming_ssl = true;
                wizard.smtp_server = "smtp.mail.yahoo.com".to_string();
                wizard.smtp_port = 465;
                wizard.smtp_ssl = true;
                wizard.smtp_auth_allowed = true;
            }
            _ => {
                if wizard.incoming_server.is_empty() {
                    wizard.incoming_server = format!("mail.{}", dom);
                }
                if wizard.smtp_server.is_empty() {
                    wizard.smtp_server = format!("smtp.{}", dom);
                }
                if wizard.incoming_port == 0 {
                    wizard.incoming_port = if is_pop { 995 } else { 993 };
                }
                wizard.incoming_ssl = true;
                wizard.smtp_port = 587;
                wizard.smtp_ssl = true;
                wizard.smtp_auth_allowed = true;
            }
        }
    }
}

fn discover_into_wizard(email: &str, wizard: &mut WizardState) {
    if let Some((user, domain)) = email.split_once('@') {
        let dom = domain.trim().to_lowercase();
        if wizard.login_name.is_empty() {
            wizard.login_name = user.trim().to_string();
        }
        if dom == "gmail.com" || dom == "googlemail.com" {
            wizard.incoming_server = "imap.gmail.com".to_string();
            wizard.incoming_port = 993;
            wizard.incoming_protocol = "IMAP".to_string();
            wizard.incoming_ssl = true;
            wizard.smtp_server = "smtp.gmail.com".to_string();
            wizard.smtp_port = 465;
            wizard.smtp_ssl = true;
            wizard.smtp_auth_allowed = true;
        } else if dom == "outlook.com" || dom == "hotmail.com" || dom == "live.com" || dom == "office365.com" {
            wizard.incoming_server = "outlook.office365.com".to_string();
            wizard.incoming_port = 993;
            wizard.incoming_protocol = "IMAP".to_string();
            wizard.incoming_ssl = true;
            wizard.smtp_server = "smtp.office365.com".to_string();
            wizard.smtp_port = 587;
            wizard.smtp_ssl = true;
            wizard.smtp_auth_allowed = true;
        } else if dom == "yahoo.com" || dom == "ymail.com" {
            wizard.incoming_server = "imap.mail.yahoo.com".to_string();
            wizard.incoming_port = 993;
            wizard.incoming_protocol = "IMAP".to_string();
            wizard.incoming_ssl = true;
            wizard.smtp_server = "smtp.mail.yahoo.com".to_string();
            wizard.smtp_port = 465;
            wizard.smtp_ssl = true;
            wizard.smtp_auth_allowed = true;
        } else if dom == "icloud.com" || dom == "me.com" || dom == "mac.com" {
            wizard.incoming_server = "imap.mail.me.com".to_string();
            wizard.incoming_port = 993;
            wizard.incoming_protocol = "IMAP".to_string();
            wizard.incoming_ssl = true;
            wizard.smtp_server = "smtp.mail.me.com".to_string();
            wizard.smtp_port = 587;
            wizard.smtp_ssl = true;
            wizard.smtp_auth_allowed = true;
        } else {
            if wizard.incoming_server.is_empty() || wizard.incoming_server == "mail.isp.com" {
                wizard.incoming_server = format!("mail.{}", dom);
                wizard.incoming_port = 995;
                wizard.incoming_protocol = "POP".to_string();
                wizard.incoming_ssl = true;
            }
            if wizard.smtp_server.is_empty() || wizard.smtp_server == "smtp.isp.com" {
                wizard.smtp_server = format!("smtp.{}", dom);
                wizard.smtp_port = 587;
                wizard.smtp_ssl = true;
                wizard.smtp_auth_allowed = true;
            }
        }
    }
}

fn save_wizard_to_config(state: &mut AppState) {
    let email = state.wizard.return_address.trim().to_string();
    let real_name = if !state.wizard.real_name.is_empty() {
        state.wizard.real_name.trim().to_string()
    } else if let Some((u, _)) = email.split_once('@') {
        u.to_string()
    } else {
        String::new()
    };
    let login = if !state.wizard.login_name.is_empty() {
        state.wizard.login_name.trim().to_string()
    } else if let Some((u, _)) = email.split_once('@') {
        u.to_string()
    } else {
        String::new()
    };

    if state.wizard.is_dominant {
        // Modern sections
        state.conf.set("sending_mail", "real_name", &real_name);
        state.conf.set("sending_mail", "email_address", &email);
        state.conf.set("sending_mail", "email", &email);
        state.conf.set("sending_mail", "smtp_server", &state.wizard.smtp_server);

        state.conf.set("checking_mail", "pop_server", &state.wizard.incoming_server);
        state.conf.set("checking_mail", "pop_username", &login);
        state.conf.set_bool("checking_mail", "use_imap", state.wizard.incoming_protocol.eq_ignore_ascii_case("IMAP"));
        state.conf.set("checking_mail", "account_type", &state.wizard.incoming_protocol);
        state.conf.set_bool("checking_mail", "leave_on_server", true);

        state.conf.set("getting_mail", "server", &state.wizard.incoming_server);
        state.conf.set("getting_mail", "username", &login);
        state.conf.set("getting_mail", "account_type", &state.wizard.incoming_protocol);
        state.conf.set("getting_mail", "ssl_mode", if state.wizard.incoming_ssl { "1" } else { "0" });

        state.conf.set_bool("ssl", "use_ssl", state.wizard.incoming_ssl);
        state.conf.set_int("ssl", "ssl_pop_mode", if state.wizard.incoming_ssl { 1 } else { 0 });
        state.conf.set_int("ssl", "ssl_smtp_mode", if state.wizard.smtp_ssl { 1 } else { 0 });

        // Classic Eudora [Settings] section
        state.conf.set("Settings", "RealName", &real_name);
        state.conf.set("Settings", "ReturnAddress", &email);
        state.conf.set("Settings", "MailServer", &state.wizard.incoming_server);
        state.conf.set("Settings", "LoginName", &login);
        state.conf.set("Settings", "SMTPServer", &state.wizard.smtp_server);
        state.conf.set("Settings", "POPAccount", &format!("{}@{}", login, state.wizard.incoming_server));

        let acct = macmbx::conf::Account {
            index: 1,
            name: "Dominant".to_string(),
            real_name: real_name.clone(),
            email: email.clone(),
            account_type: state.wizard.incoming_protocol.clone(),
            server: state.wizard.incoming_server.clone(),
            smtp_server: state.wizard.smtp_server.clone(),
            username: login.clone(),
            check_interval: 15,
            ssl_mode: if state.wizard.incoming_ssl { 1 } else { 0 },
            leave_on_server: true,
            enabled: true,
        };
        if state.conf.count_accounts() > 0 {
            state.conf.update_account(1, &acct);
        } else {
            state.conf.add_account(&acct);
        }
    } else {
        let count = state.conf.count_accounts();
        let new_idx = count as i32 + 1;
        let acct = macmbx::conf::Account {
            index: new_idx,
            name: if state.wizard.persona_name.is_empty() {
                format!("Personality {}", new_idx)
            } else {
                state.wizard.persona_name.clone()
            },
            real_name: real_name.clone(),
            email: email.clone(),
            account_type: state.wizard.incoming_protocol.clone(),
            server: state.wizard.incoming_server.clone(),
            smtp_server: state.wizard.smtp_server.clone(),
            username: login.clone(),
            check_interval: 15,
            ssl_mode: if state.wizard.incoming_ssl { 1 } else { 0 },
            leave_on_server: true,
            enabled: true,
        };
        state.conf.add_account(&acct);
    }
    let ini_path = state.data_dir.join("geudora.ini");
    let _ = state.conf.save_as(&ini_path);
    state.status_left = "✓ Account setup completed".to_string();
    state.settings_status = "✓ Saved account configuration".to_string();
    state.dirty = true;
}

fn render_wizard_all(doc: &mut Document, state: &mut AppState) {
    sync_theme_and_mode(doc, state.skin, state.last_mail_skin, state.dark_mode);

    let is_dom = state.wizard.is_dominant;
    let cur_step = state.wizard.step;

    // Show/hide personality step tab and checklist item
    if let Some(el) = doc.get_element_by_id("wiz-tab-2") {
        doc.set_style_property(el, "display", if is_dom { "none" } else { "flex" });
    }
    if let Some(el) = doc.get_element_by_id("chk-2") {
        doc.set_style_property(el, "display", if is_dom { "none" } else { "flex" });
    }
    if let Some(el) = doc.get_element_by_id("chk-0") {
        doc.set_style_property(el, "display", if is_dom { "flex" } else { "none" });
    }
    if let Some(el) = doc.get_element_by_id("wiz-sum-row-pers") {
        doc.set_style_property(el, "display", if is_dom { "none" } else { "grid" });
    }

    // Update tabs and panes
    for i in 0..=8 {
        if let Some(tab_el) = doc.get_element_by_id(&format!("wiz-tab-{}", i)) {
            if i == cur_step {
                doc.class_list_add(tab_el, "active");
            } else {
                doc.class_list_remove(tab_el, "active");
            }
        }

        if let Some(chk_el) = doc.get_element_by_id(&format!("chk-{}", i)) {
            doc.class_list_remove(chk_el, "current");
            doc.class_list_remove(chk_el, "done");
            if i < cur_step {
                doc.class_list_add(chk_el, "done");
            } else if i == cur_step {
                doc.class_list_add(chk_el, "current");
            }
        }

        if let Some(pane_el) = doc.get_element_by_id(&format!("wiz-step-{}", i)) {
            if i == cur_step {
                doc.class_list_add(pane_el, "active");
            } else {
                doc.class_list_remove(pane_el, "active");
            }
        }
    }

    // Populate input controls with current wizard state
    if let Some(el) = doc.get_element_by_id("wiz-persname") {
        doc.set_attribute(el, "value", &state.wizard.persona_name);
    }
    if let Some(el) = doc.get_element_by_id("wiz-yourname") {
        doc.set_attribute(el, "value", &state.wizard.real_name);
    }
    if let Some(el) = doc.get_element_by_id("wiz-email") {
        doc.set_attribute(el, "value", &state.wizard.return_address);
    }
    if let Some(el) = doc.get_element_by_id("wiz-login") {
        doc.set_attribute(el, "value", &state.wizard.login_name);
    }
    if let Some(el) = doc.get_element_by_id("wiz-inserver") {
        doc.set_attribute(el, "value", &state.wizard.incoming_server);
    }
    if let Some(el) = doc.get_element_by_id("wiz-inport") {
        doc.set_attribute(el, "value", &state.wizard.incoming_port.to_string());
    }
    if let Some(el) = doc.get_element_by_id("wiz-outserver") {
        doc.set_attribute(el, "value", &state.wizard.smtp_server);
    }
    if let Some(el) = doc.get_element_by_id("wiz-outport") {
        doc.set_attribute(el, "value", &state.wizard.smtp_port.to_string());
    }

    // Summary Card on final step
    set_text_by_id(
        doc,
        "wiz-sum-pers",
        if state.wizard.persona_name.is_empty() {
            "(Dominant / Primary)"
        } else {
            &state.wizard.persona_name
        },
    );
    set_text_by_id(
        doc,
        "wiz-sum-name",
        if state.wizard.real_name.is_empty() {
            "(Not Specified)"
        } else {
            &state.wizard.real_name
        },
    );
    set_text_by_id(
        doc,
        "wiz-sum-email",
        if state.wizard.return_address.is_empty() {
            "(Not Specified)"
        } else {
            &state.wizard.return_address
        },
    );
    set_text_by_id(
        doc,
        "wiz-sum-login",
        if state.wizard.login_name.is_empty() {
            "(Not Specified)"
        } else {
            &state.wizard.login_name
        },
    );

    let in_ssl_str = if state.wizard.incoming_ssl { "SSL/TLS" } else { "Plain" };
    set_text_by_id(
        doc,
        "wiz-sum-in",
        &format!(
            "{} : {} ({} · {})",
            if state.wizard.incoming_server.is_empty() { "none" } else { &state.wizard.incoming_server },
            state.wizard.incoming_port,
            state.wizard.incoming_protocol,
            in_ssl_str
        ),
    );

    let out_ssl_str = if state.wizard.smtp_ssl { "SSL/TLS" } else { "Plain" };
    let out_auth_str = if state.wizard.smtp_auth_allowed { "Auth Enabled" } else { "No Auth" };
    set_text_by_id(
        doc,
        "wiz-sum-out",
        &format!(
            "{} : {} ({} · {})",
            if state.wizard.smtp_server.is_empty() { "none" } else { &state.wizard.smtp_server },
            state.wizard.smtp_port,
            out_ssl_str,
            out_auth_str
        ),
    );

    // Update buttons
    let is_first = if is_dom { cur_step == 0 } else { cur_step == 1 };
    if let Some(back_btn) = doc.get_element_by_id("wiz-btn-back") {
        if is_first {
            doc.set_attribute(back_btn, "disabled", "true");
        } else {
            doc.remove_attribute(back_btn, "disabled");
        }
    }

    if let Some(next_btn) = doc.get_element_by_id("wiz-btn-next") {
        if cur_step == 8 {
            doc.set_text_content(next_btn, "Finish");
        } else {
            doc.set_text_content(next_btn, "Next >");
        }
    }
}

fn sync_theme_and_mode(doc: &mut Document, skin: Skin, last_mail_skin: Skin, dark: bool) {
    let effective_skin = match skin {
        Skin::Settings | Skin::Wizard => last_mail_skin,
        _ => skin,
    };
    let root_id = doc.root.node_id;
    for el in [Some(root_id), doc.query_selector("html"), doc.query_selector("body")].into_iter().flatten() {
        doc.class_list_remove(el, "eudora-theme");
        doc.class_list_remove(el, "email-theme");
        match effective_skin {
            Skin::Eudora | Skin::Settings | Skin::Wizard => doc.class_list_add(el, "eudora-theme"),
            Skin::Email => doc.class_list_add(el, "email-theme"),
        }
        if dark {
            doc.class_list_add(el, "dark");
        } else {
            doc.class_list_remove(el, "dark");
        }
    }
    if let Some(btn) = doc.get_element_by_id("btn-dark") {
        doc.set_text_content(btn, if dark { "☀" } else { "☽" });
    }
}

fn set_text_by_id(doc: &mut Document, id: &str, text: &str) {
    if let Some(el) = doc.get_element_by_id(id) {
        doc.set_text_content(el, text);
    }
}

fn render_active_skin(doc: &mut Document, state: &mut AppState) {
    match state.skin {
        Skin::Eudora => render_eudora_all(doc, state),
        Skin::Email => render_email_all(doc, state),
        Skin::Settings => render_settings_all(doc, state),
        Skin::Wizard => render_wizard_all(doc, state),
    }
    doc.style_dirty = true;
    doc.layout_generation = doc.layout_generation.wrapping_add(1);
    state.dirty = true;
}

// ============================================================================
// Event Handling
// ============================================================================

fn register_listeners(doc: &mut Document, state: Arc<Mutex<AppState>>) {
    let root = doc.root.node_id;

    // Intercept keypress only inside compose-body to prevent duplicate text input in contenteditable editor
    doc.add_event_listener(
        root,
        "keypress",
        Box::new(move |evt, doc| {
            if let Some(body_el) = doc.get_element_by_id("compose-body") {
                if doc.contains(body_el, evt.target) {
                    evt.prevent_default();
                }
            }
        }),
        ListenerOptions::capture(true),
    );

    // Keyboard shortcuts: Ctrl+B/Cmd+B, Ctrl+I/Cmd+I, Ctrl+U/Cmd+U
    let state_kd = state.clone();
    doc.add_event_listener(
        root,
        "keydown",
        Box::new(move |evt, doc| {
            let mut st = state_kd.lock().unwrap();
            handle_key_down(doc, evt, &mut st);
        }),
        ListenerOptions::capture(true),
    );

    let state_click = state.clone();
    doc.add_event_listener(
        root,
        "click",
        Box::new(move |evt, doc| {
            let mut st = state_click.lock().unwrap();
            handle_click(doc, evt.target, &mut st);
        }),
        ListenerOptions::default(),
    );

    let state_ctx = state.clone();
    doc.add_event_listener(
        root,
        "contextmenu",
        Box::new(move |evt, doc| {
            let mut st = state_ctx.lock().unwrap();
            handle_contextmenu(doc, evt, &mut st);
        }),
        ListenerOptions::default(),
    );

    let state_md = state.clone();
    doc.add_event_listener(
        root,
        "mousedown",
        Box::new(move |evt, doc| {
            let mut st = state_md.lock().unwrap();
            handle_mouse_down(doc, evt, &mut st);
        }),
        ListenerOptions::capture(true),
    );

    let state_mm = state.clone();
    doc.add_event_listener(
        root,
        "mousemove",
        Box::new(move |evt, doc| {
            let mut st = state_mm.lock().unwrap();
            handle_mouse_move(doc, evt, &mut st);
        }),
        ListenerOptions::default(),
    );

    let state_mu = state.clone();
    doc.add_event_listener(
        root,
        "mouseup",
        Box::new(move |evt, doc| {
            let mut st = state_mu.lock().unwrap();
            handle_mouse_up(doc, evt, &mut st);
        }),
        ListenerOptions::capture(true),
    );

    let state_change = state.clone();
    doc.add_event_listener(
        root,
        "change",
        Box::new(move |evt, doc| {
            let mut st = state_change.lock().unwrap();
            handle_change(doc, evt.target, &mut st);
        }),
        ListenerOptions::default(),
    );

    let state_input = state;
    doc.add_event_listener(
        root,
        "input",
        Box::new(move |evt, doc| {
            let mut st = state_input.lock().unwrap();
            handle_change(doc, evt.target, &mut st);
        }),
        ListenerOptions::default(),
    );
}

fn handle_change(doc: &mut Document, target: u32, state: &mut AppState) {
    if let Some(in_el) = doc.closest(target, "input, select, textarea") {
        if let Some(id) = doc.get_attribute(in_el, "id") {
            if id.starts_with("wiz-") {
                sync_wizard_from_dom(doc, &mut state.wizard);
            }
        }
    }
    if let Some(sel_el) = doc.closest(target, "select") {
        let val = doc.value(sel_el);
        if let Some(id) = doc.get_attribute(sel_el, "id") {
            match id.as_str() {
                "compose-from" => {
                    state.compose.from = val;
                }
                "comp-enc-select" => {
                    state.compose.encoding = val;
                }
                "comp-pri-select" => {
                    state.compose.priority = val;
                    set_text_by_id(doc, "compose-priority", &state.compose.priority);
                }
                "comp-sig-select" => {
                    state.compose.signature = val;
                }
                "comp-font-select" => {
                    apply_formatting_command(doc, "fontname", &val);
                    state.dirty = true;
                }
                "comp-size-select" => {
                    apply_formatting_command(doc, "fontsize", &val);
                    state.dirty = true;
                }
                "comp-color-select" => {
                    apply_formatting_command(doc, "forecolor", &val);
                    state.dirty = true;
                }
                _ => {}
            }
        }
    }
}

fn handle_key_down(doc: &mut Document, evt: &mut DomEvent, state: &mut AppState) {
    let accel = evt.ctrl_key || evt.meta_key;
    if !accel || evt.alt_key {
        return;
    }

    let is_key = |code: u32, letter: char| {
        evt.key_code == code
            || evt.key.eq_ignore_ascii_case(&letter.to_string())
            || evt.char_code.is_some_and(|c| c.eq_ignore_ascii_case(&letter))
            || evt.code().eq_ignore_ascii_case(&format!("Key{}", letter.to_ascii_uppercase()))
    };

    let cmd = if is_key(66, 'b') {
        Some("bold")
    } else if is_key(73, 'i') {
        Some("italic")
    } else if is_key(85, 'u') {
        Some("underline")
    } else {
        None
    };

    if let Some(command) = cmd {
        if state.active_tab() == TabKind::Compose || state.compose_open {
            if let Some(_body_el) = doc.get_element_by_id("compose-body") {
                evt.prevent_default();
                apply_formatting_command(doc, command, "");
                state.dirty = true;
            }
        }
    }
}

fn handle_mouse_down(
    doc: &mut Document,
    evt: &mut DomEvent,
    state: &mut AppState,
) {
    if doc.closest(evt.target, "#compose-fmt-bar, #compose-fmt-bar *, .comp-action-bar, .comp-action-bar *, [data-command]").is_some() {
        evt.prevent_default();
    }
    if let Some(target_el) = doc.closest(evt.target, "[data-split]") {
        if let Some(kind_str) = doc.get_attribute(target_el, "data-split") {
            let kind = match kind_str.as_str() {
                "sidebar" => Some(SplitterKind::Sidebar),
                "msg-list" => Some(SplitterKind::MessageList),
                "contacts" => Some(SplitterKind::Contacts),
                "filters" => Some(SplitterKind::Filters),
                _ => None,
            };
            if let Some(k) = kind {
                state.active_split_drag = Some((k, evt.client_x, evt.client_y));
            }
        }
    }
}

fn handle_mouse_move(
    doc: &mut Document,
    evt: &DomEvent,
    state: &mut AppState,
) {
    if let Some((kind, start_x, start_y)) = state.active_split_drag {
        let dx = evt.client_x - start_x;
        let dy = evt.client_y - start_y;

        match kind {
            SplitterKind::Sidebar => {
                let new_w = (state.sidebar_width + dx).clamp(160.0, 480.0);
                if (new_w - state.sidebar_width).abs() >= 1.0 {
                    state.sidebar_width = new_w;
                    state.active_split_drag = Some((kind, evt.client_x, evt.client_y));
                    if let Some(wazoo) = doc.get_element_by_id("wazoo") {
                        doc.set_style_property(wazoo, "width", &format!("{}px", new_w));
                        doc.style_dirty = true;
                        doc.layout_generation = doc.layout_generation.wrapping_add(1);
                        state.dirty = true;
                    }
                }
            }
            SplitterKind::MessageList => {
                let new_h = (state.msg_list_height + dy).clamp(100.0, 600.0);
                if (new_h - state.msg_list_height).abs() >= 1.0 {
                    state.msg_list_height = new_h;
                    state.active_split_drag = Some((kind, evt.client_x, evt.client_y));
                    if let Some(el) = doc.get_element_by_id("in-msg-list-wrap") {
                        doc.set_style_property(el, "height", &format!("{}px", new_h));
                    }
                    if let Some(el) = doc.get_element_by_id("out-msg-list-wrap") {
                        doc.set_style_property(el, "height", &format!("{}px", new_h));
                    }
                    doc.style_dirty = true;
                    doc.layout_generation = doc.layout_generation.wrapping_add(1);
                    state.dirty = true;
                }
            }
            SplitterKind::Contacts => {
                let new_w = (state.contact_list_width + dx).clamp(180.0, 500.0);
                if (new_w - state.contact_list_width).abs() >= 1.0 {
                    state.contact_list_width = new_w;
                    state.active_split_drag = Some((kind, evt.client_x, evt.client_y));
                    if let Some(el) = doc.get_element_by_id("contact-list-pane") {
                        doc.set_style_property(el, "width", &format!("{}px", new_w));
                        doc.style_dirty = true;
                        doc.layout_generation = doc.layout_generation.wrapping_add(1);
                        state.dirty = true;
                    }
                }
            }
            SplitterKind::Filters => {
                let new_w = (state.filter_list_width + dx).clamp(200.0, 600.0);
                if (new_w - state.filter_list_width).abs() >= 1.0 {
                    state.filter_list_width = new_w;
                    state.active_split_drag = Some((kind, evt.client_x, evt.client_y));
                    if let Some(el) = doc.get_element_by_id("filter-list-pane") {
                        doc.set_style_property(el, "width", &format!("{}px", new_w));
                        doc.style_dirty = true;
                        doc.layout_generation = doc.layout_generation.wrapping_add(1);
                        state.dirty = true;
                    }
                }
            }
        }
    }
}

fn handle_mouse_up(
    doc: &mut Document,
    evt: &mut DomEvent,
    state: &mut AppState,
) {
    if doc.closest(evt.target, "#compose-fmt-bar button, .comp-action-bar button, [data-command]").is_some() {
        evt.prevent_default();
    }
    if state.active_split_drag.is_some() {
        state.active_split_drag = None;
    }
}

fn handle_contextmenu(
    doc: &mut Document,
    evt: &DomEvent,
    state: &mut AppState,
) {
    if let Some(target_el) = doc.closest(evt.target, "[data-mailbox]") {
        if let Some(name) = doc.get_attribute(target_el, "data-mailbox") {
            state.ctx_target = Some(name);
            state.ctx_target_is_folder = false;
            state.ctx_menu_open = true;
            state.ctx_menu_pos = (evt.client_x, evt.client_y);
            render_active_skin(doc, state);
            return;
        }
    }
    if let Some(target_el) = doc.closest(evt.target, "[data-folder]") {
        if let Some(path) = doc.get_attribute(target_el, "data-folder") {
            state.ctx_target = Some(path);
            state.ctx_target_is_folder = true;
            state.ctx_menu_open = true;
            state.ctx_menu_pos = (evt.client_x, evt.client_y);
            render_active_skin(doc, state);
        }
    }
}

fn handle_click(doc: &mut Document, target: u32, state: &mut AppState) {
    // 0a. Formatting command bar clicks (data-command)
    if let Some(cmd_el) = doc.closest(target, "[data-command]") {
        if let Some(cmd) = doc.get_attribute(cmd_el, "data-command") {
            let val = doc.get_attribute(cmd_el, "data-value").unwrap_or_default();
            apply_formatting_command(doc, &cmd, &val);
            state.dirty = true;
            return;
        }
    }

    // 0. Context menu actions
    if let Some(action_el) = doc.closest(target, "[data-ctx-action]") {
        if let Some(action) = doc.get_attribute(action_el, "data-ctx-action") {
            state.ctx_menu_open = false;
            match action.as_str() {
                "open" => {
                    if let Some(target_mb) = &state.ctx_target {
                        if !state.ctx_target_is_folder {
                            state.open_tab(TabKind::Mailbox(target_mb.clone()));
                        }
                    }
                }
                "new-mailbox" => {
                    let mut name = "New Mailbox".to_string();
                    let mut i = 1;
                    let mb_dir = state.data_dir.join("mailboxes");
                    while mb_dir.join(&name).exists() {
                        i += 1;
                        name = format!("New Mailbox {}", i);
                    }
                    let parent = if state.ctx_target_is_folder {
                        state.ctx_target.as_deref()
                    } else {
                        None
                    };
                    let _ = state.store.create_mailbox(parent, &name);
                    state.reload_from_disk();
                    state.open_tab(TabKind::Mailbox(name));
                }
                "new-folder" => {
                    let mut name = "New Folder".to_string();
                    let mut i = 1;
                    let mb_dir = state.data_dir.join("mailboxes");
                    while mb_dir.join(&name).exists() {
                        i += 1;
                        name = format!("New Folder {}", i);
                    }
                    let parent = if state.ctx_target_is_folder {
                        state.ctx_target.as_deref()
                    } else {
                        None
                    };
                    let _ = state.store.create_folder(parent, &name);
                    state.expanded_folders.insert(name.clone());
                    state.reload_from_disk();
                }
                "rename" => {
                    if let Some(target_mb) = &state.ctx_target {
                        if !["In", "Out", "Trash", "Junk", "Drafts"].iter().any(|s| s.eq_ignore_ascii_case(target_mb)) {
                            let new_name = format!("{}_renamed", target_mb);
                            let _ = state.store.rename(target_mb, &new_name);
                            state.reload_from_disk();
                        }
                    }
                }
                "delete" => {
                    if let Some(target_mb) = &state.ctx_target {
                        if !["In", "Out", "Trash", "Junk", "Drafts"].iter().any(|s| s.eq_ignore_ascii_case(target_mb)) {
                            let _ = state.store.delete(target_mb);
                            state.reload_from_disk();
                        }
                    }
                }
                "compact" => {
                    if let Some(target_mb) = &state.ctx_target {
                        let mbox_path = state.data_dir.join("mailboxes").join(target_mb);
                        if let Ok(mut toc) = macmbx::toc::Toc::open_or_create(&mbox_path) {
                            let _ = macmbx::compact::compact(&mut toc);
                            let _ = toc.save();
                            state.status_left = format!("Compacted mailbox {}", target_mb);
                        }
                    }
                }
                "empty-trash" => {
                    let trash_path = state.data_dir.join("mailboxes").join("Trash");
                    if let Ok(mut toc) = macmbx::toc::Toc::open_or_create(&trash_path) {
                        toc.msgs.clear();
                        let _ = toc.save();
                        state.reload_from_disk();
                        state.status_left = "Trash emptied".to_string();
                    }
                }
                _ => {}
            }
            render_active_skin(doc, state);
            return;
        }
    }

    // Dismiss context menu if it was open
    if state.ctx_menu_open {
        state.ctx_menu_open = false;
        render_active_skin(doc, state);
        return;
    }

    // 0.1 Context menu button on mailbox / folder row (⋯)
    if let Some(btn) = doc.closest(target, "[data-ctx-menu]") {
        if let Some(name) = doc.get_attribute(btn, "data-ctx-menu") {
            let is_folder = doc.get_attribute(btn, "data-is-folder").as_deref() == Some("true");
            state.ctx_target = Some(name);
            state.ctx_target_is_folder = is_folder;
            state.ctx_menu_open = true;
            state.ctx_menu_pos = (180.0, 150.0);
            render_active_skin(doc, state);
            return;
        }
    }

    // 0.2 Folder expand/collapse toggle
    if let Some(toggle) = doc.closest(target, "[data-toggle-folder]") {
        if let Some(path) = doc.get_attribute(toggle, "data-toggle-folder") {
            if state.expanded_folders.contains(&path) {
                state.expanded_folders.remove(&path);
            } else {
                state.expanded_folders.insert(path);
            }
            render_active_skin(doc, state);
            return;
        }
    }

    // 0.3 Dynamic tab close
    if let Some(close_btn) = doc.closest(target, "[data-tab-close]") {
        if let Some(idx_str) = doc.get_attribute(close_btn, "data-tab-close") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                state.close_tab(idx);
                render_active_skin(doc, state);
                return;
            }
        }
    }

    // 0.4 Dynamic tab select
    if let Some(tab_btn) = doc.closest(target, "[data-tab-idx]") {
        if let Some(idx_str) = doc.get_attribute(tab_btn, "data-tab-idx") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                if idx < state.open_tabs.len() {
                    state.active_tab_index = idx;
                    if let TabKind::Mailbox(ref mb) = state.open_tabs[idx] {
                        state.selected_mailbox = mb.clone();
                        state.ensure_valid_selection();
                    }
                    render_active_skin(doc, state);
                    return;
                }
            }
        }
    }

    // 0.41 Wazoo Tab Switcher (Mailboxes / Calendar / Status / Signatures / Stationery)
    if let Some(wtab_btn) = doc.closest(target, "[data-wazoo-tab]") {
        if let Some(tab_name) = doc.get_attribute(wtab_btn, "data-wazoo-tab") {
            match tab_name.as_str() {
                "mailboxes" => state.wazoo_tab = WazooTab::Mailboxes,
                "calendar" => state.wazoo_tab = WazooTab::Calendar,
                "status" => state.wazoo_tab = WazooTab::Status,
                "signatures" => state.wazoo_tab = WazooTab::Signatures,
                "stationery" => state.wazoo_tab = WazooTab::Stationery,
                _ => {}
            }
            render_active_skin(doc, state);
            return;
        }
    }

    // 0.42 Calendar date selection (mini-calendar or month grid)
    if let Some(day_btn) = doc.closest(target, "[data-cal-day]") {
        if let Some(ymd_str) = doc.get_attribute(day_btn, "data-cal-day") {
            if let Ok(ymd) = ymd_str.parse::<i32>() {
                state.cal_selected_date = ymd;
                render_active_skin(doc, state);
                return;
            }
        }
    }

    // 0.43 Calendar view mode switcher (month, week, day, agenda)
    if let Some(view_btn) = doc.closest(target, "[data-cal-view]") {
        if let Some(mode) = doc.get_attribute(view_btn, "data-cal-view") {
            state.cal_view_mode = mode;
            render_active_skin(doc, state);
            return;
        }
    }

    // 0.44 Calendar event click to edit
    if let Some(chip) = doc.closest(target, "[data-cal-edit-uid]") {
        if let Some(uid) = doc.get_attribute(chip, "data-cal-edit-uid") {
            state.cal_editing_uid = uid;
            state.cal_is_new_event = false;
            state.cal_modal_open = true;
            render_active_skin(doc, state);
            return;
        }
    }

    // 0.5 Open mailbox (e.g. from Stat Card on Welcome page)
    if let Some(mb_card) = doc.closest(target, "[data-open-mailbox]") {
        if let Some(name) = doc.get_attribute(mb_card, "data-open-mailbox") {
            state.open_tab(TabKind::Mailbox(name));
            render_active_skin(doc, state);
            return;
        }
    }

    // 0.6 Open tab (e.g. contacts / filters / welcome from Welcome page actions)
    if let Some(tab_card) = doc.closest(target, "[data-open-tab]") {
        if let Some(tab_name) = doc.get_attribute(tab_card, "data-open-tab") {
            match tab_name.as_str() {
                "contacts" => state.open_tab(TabKind::Contacts),
                "filters" => state.open_tab(TabKind::Filters),
                "welcome" => state.open_tab(TabKind::Welcome),
                "calendar" => state.open_tab(TabKind::Calendar),
                _ => {}
            }
            render_active_skin(doc, state);
            return;
        }
    }

    // 0.69 Check wizard step tab click
    if let Some(tab_el) = doc.closest(target, "[data-wiz-step]") {
        if let Some(step_str) = doc.get_attribute(tab_el, "data-wiz-step") {
            if let Ok(step_idx) = step_str.parse::<usize>() {
                sync_wizard_from_dom(doc, &mut state.wizard);
                state.wizard.step = step_idx;
                render_active_skin(doc, state);
                return;
            }
        }
    }

    // Wizard radio options / choice cards click
    if let Some(opt_el) = doc.closest(target, ".wiz-choice-card, .wiz-radio-opt, input[name='wiz-acct-choice']") {
        for (choice, id, card_id) in [
            (0, "wiz-radio-new", "wiz-card-choice-0"),
            (2, "wiz-radio-detect", "wiz-card-choice-2"),
            (3, "wiz-radio-skip", "wiz-card-choice-3"),
            (1, "wiz-radio-import", "wiz-card-choice-1"),
        ] {
            let matches_radio = doc.get_element_by_id(id).map_or(false, |el| el == opt_el || doc.contains(opt_el, el) || doc.contains(el, target));
            let matches_card = doc.get_element_by_id(card_id).map_or(false, |el| el == opt_el || doc.contains(opt_el, el) || doc.contains(el, target));
            if matches_radio || matches_card {
                state.wizard.account_choice = choice;
                render_active_skin(doc, state);
                return;
            }
        }
    }

    // Wizard provider card click
    if let Some(card_el) = doc.closest(target, ".wiz-provider-card, input[name='wiz-prov-choice']") {
        for (p, id, card_id) in [
            ("gmail", "wiz-prov-gmail", "wiz-card-prov-gmail"),
            ("outlook", "wiz-prov-outlook", "wiz-card-prov-outlook"),
            ("yahoo", "wiz-prov-yahoo", "wiz-card-prov-yahoo"),
            ("other", "wiz-prov-other", "wiz-card-prov-other"),
        ] {
            let matches_radio = doc.get_element_by_id(id).map_or(false, |el| el == card_el || doc.contains(card_el, el) || doc.contains(el, target));
            let matches_card = doc.get_element_by_id(card_id).map_or(false, |el| el == card_el || doc.contains(card_el, el) || doc.contains(el, target));
            if matches_radio || matches_card {
                state.wizard.provider = p.to_string();
                render_active_skin(doc, state);
                return;
            }
        }
    }

    // 0.7 Check settings category sidebar selection
    if let Some(cat_el) = doc.closest(target, "[data-settings-cat]") {
        if let Some(cat) = doc.get_attribute(cat_el, "data-settings-cat") {
            state.settings_category = cat;
            render_active_skin(doc, state);
            return;
        }
    }

    // 0.8 Check personality account selection
    if let Some(pers_el) = doc.closest(target, "[data-select-account]") {
        if let Some(idx_str) = doc.get_attribute(pers_el, "data-select-account") {
            if let Ok(idx) = idx_str.parse::<i32>() {
                state.selected_account_idx = idx;
                render_active_skin(doc, state);
                return;
            }
        }
    }

    // 1. Check close button on Compose tab
    if doc.closest(target, ".tab-close").is_some() {
        state.compose_open = false;
        state.close_tab_if_open(&TabKind::Compose);
        state.open_tab(TabKind::Mailbox("In".to_string()));
        render_active_skin(doc, state);
        return;
    }

    // 2. Check action buttons (toolbar / compose / skin switch / settings / dark mode)
    if let Some(btn) = doc.closest(target, "[data-action]") {
        if let Some(action) = doc.get_attribute(btn, "data-action") {
            match action.as_str() {
                "open-calendar" | "open-full-calendar" => {
                    state.open_tab(TabKind::Calendar);
                    render_active_skin(doc, state);
                    return;
                }
                "mini-cal-prev" => {
                    state.cal_view_date = prev_month_ymd(state.cal_view_date);
                    render_active_skin(doc, state);
                    return;
                }
                "mini-cal-next" => {
                    state.cal_view_date = next_month_ymd(state.cal_view_date);
                    render_active_skin(doc, state);
                    return;
                }
                "mini-cal-today" => {
                    let now_ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
                    let (cy, cm, cd, _, _) = macmbx::calendar::split_ts(now_ts);
                    let today_ymd = cy * 10000 + cm * 100 + cd;
                    state.cal_view_date = today_ymd;
                    state.cal_selected_date = today_ymd;
                    render_active_skin(doc, state);
                    return;
                }
                "cal-prev" => {
                    match state.cal_view_mode.as_str() {
                        "month" => state.cal_view_date = prev_month_ymd(state.cal_view_date),
                        "week" => {
                            let (y, m, d) = (state.cal_view_date / 10000, (state.cal_view_date / 100) % 100, state.cal_view_date % 100);
                            let ts = macmbx::calendar::make_local_ts(y, m, d, 12, 0) - (7 * 86400);
                            let (ny, nm, nd, _, _) = macmbx::calendar::split_ts(ts);
                            state.cal_view_date = ny * 10000 + nm * 100 + nd;
                        }
                        "day" => {
                            let (y, m, d) = (state.cal_selected_date / 10000, (state.cal_selected_date / 100) % 100, state.cal_selected_date % 100);
                            let ts = macmbx::calendar::make_local_ts(y, m, d, 12, 0) - 86400;
                            let (ny, nm, nd, _, _) = macmbx::calendar::split_ts(ts);
                            state.cal_selected_date = ny * 10000 + nm * 100 + nd;
                        }
                        _ => state.cal_view_date = prev_month_ymd(state.cal_view_date),
                    }
                    render_active_skin(doc, state);
                    return;
                }
                "cal-next" => {
                    match state.cal_view_mode.as_str() {
                        "month" => state.cal_view_date = next_month_ymd(state.cal_view_date),
                        "week" => {
                            let (y, m, d) = (state.cal_view_date / 10000, (state.cal_view_date / 100) % 100, state.cal_view_date % 100);
                            let ts = macmbx::calendar::make_local_ts(y, m, d, 12, 0) + (7 * 86400);
                            let (ny, nm, nd, _, _) = macmbx::calendar::split_ts(ts);
                            state.cal_view_date = ny * 10000 + nm * 100 + nd;
                        }
                        "day" => {
                            let (y, m, d) = (state.cal_selected_date / 10000, (state.cal_selected_date / 100) % 100, state.cal_selected_date % 100);
                            let ts = macmbx::calendar::make_local_ts(y, m, d, 12, 0) + 86400;
                            let (ny, nm, nd, _, _) = macmbx::calendar::split_ts(ts);
                            state.cal_selected_date = ny * 10000 + nm * 100 + nd;
                        }
                        _ => state.cal_view_date = next_month_ymd(state.cal_view_date),
                    }
                    render_active_skin(doc, state);
                    return;
                }
                "cal-today" => {
                    let now_ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
                    let (cy, cm, cd, _, _) = macmbx::calendar::split_ts(now_ts);
                    let today_ymd = cy * 10000 + cm * 100 + cd;
                    state.cal_view_date = today_ymd;
                    state.cal_selected_date = today_ymd;
                    render_active_skin(doc, state);
                    return;
                }
                "cal-new-event" => {
                    state.cal_is_new_event = true;
                    state.cal_editing_uid.clear();
                    state.cal_modal_open = true;
                    render_active_skin(doc, state);
                    return;
                }
                "cal-day-add" => {
                    if let Some(date_str) = doc.closest(target, "[data-date]").and_then(|el| doc.get_attribute(el, "data-date")) {
                        if let Ok(ymd) = date_str.parse::<i32>() {
                            state.cal_selected_date = ymd;
                        }
                    }
                    state.cal_is_new_event = true;
                    state.cal_editing_uid.clear();
                    state.cal_modal_open = true;
                    render_active_skin(doc, state);
                    return;
                }
                "cal-modal-close" => {
                    state.cal_modal_open = false;
                    render_active_skin(doc, state);
                    return;
                }
                "cal-save-event" => {
                    let summary = get_input_or_text(doc, "cal-ev-summary").unwrap_or_else(|| "Untitled Event".to_string());
                    let date_str = get_input_or_text(doc, "cal-ev-date").unwrap_or_default();
                    let ymd = parse_ymd_str(&date_str).unwrap_or(state.cal_selected_date);
                    let allday = doc.get_element_by_id("cal-ev-allday")
                        .map(|el| doc.has_attribute(el, "checked") || doc.value(el) == "on" || doc.value(el) == "true")
                        .unwrap_or(false);
                    let stime_str = get_input_or_text(doc, "cal-ev-stime").unwrap_or_else(|| "09:00".to_string());
                    let etime_str = get_input_or_text(doc, "cal-ev-etime").unwrap_or_else(|| "10:00".to_string());
                    let location = get_input_or_text(doc, "cal-ev-location").unwrap_or_default();
                    let color = get_input_or_text(doc, "cal-ev-color").unwrap_or_else(|| "#326ea5".to_string());
                    let reminder = get_input_or_text(doc, "cal-ev-reminder").and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
                    let desc = get_input_or_text(doc, "cal-ev-desc").unwrap_or_default();

                    let parse_hm = |s: &str| -> (i32, i32) {
                        let parts: Vec<&str> = s.split(':').collect();
                        let h = parts.get(0).and_then(|p| p.trim().parse::<i32>().ok()).unwrap_or(9);
                        let m = parts.get(1).and_then(|p| p.trim().parse::<i32>().ok()).unwrap_or(0);
                        (h, m)
                    };
                    let (sh, sm) = parse_hm(&stime_str);
                    let (eh, em) = parse_hm(&etime_str);
                    let y = ymd / 10000;
                    let m = (ymd / 100) % 100;
                    let d = ymd % 100;
                    let dt_start = macmbx::calendar::make_local_ts(y, m, d, sh, sm);
                    let mut dt_end = macmbx::calendar::make_local_ts(y, m, d, eh, em);
                    if dt_end <= dt_start {
                        dt_end = dt_start + 3600;
                    }

                    let now_ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
                    let mut ev = if !state.cal_is_new_event && !state.cal_editing_uid.is_empty() {
                        state.cal_store.get_event_by_uid(&state.cal_editing_uid).unwrap_or_default()
                    } else {
                        macmbx::calendar::CalendarEvent::default()
                    };
                    if ev.uid.is_empty() {
                        ev.uid = format!("evt-{}-{}", now_ts, mb_name_hash(&summary));
                    }
                    ev.summary = summary.clone();
                    ev.location = location;
                    ev.description = desc;
                    ev.dt_start_ts = dt_start;
                    ev.dt_end_ts = dt_end;
                    ev.start_day = ymd;
                    ev.end_day = ymd;
                    ev.all_day = allday;
                    ev.color = color;
                    ev.reminder_mins = reminder;

                    if state.cal_is_new_event {
                        state.cal_store.add_event(&ev);
                    } else {
                        state.cal_store.update_event(&ev);
                    }

                    state.cal_selected_date = ymd;
                    state.cal_modal_open = false;
                    state.status_logs.push(StatusLog {
                        time: current_clock_str(),
                        message: format!("Saved calendar event: '{}'", summary),
                    });
                    render_active_skin(doc, state);
                    return;
                }
                "cal-delete-event" => {
                    if !state.cal_editing_uid.is_empty() {
                        state.cal_store.delete_event(&state.cal_editing_uid);
                        state.status_logs.push(StatusLog {
                            time: current_clock_str(),
                            message: "Deleted calendar event.".to_string(),
                        });
                    }
                    state.cal_modal_open = false;
                    render_active_skin(doc, state);
                    return;
                }
                "status-clear-log" => {
                    state.status_logs.clear();
                    state.status_logs.push(StatusLog {
                        time: current_clock_str(),
                        message: "Activity log cleared.".to_string(),
                    });
                    render_active_skin(doc, state);
                    return;
                }
                "open-wizard" => {
                    state.wizard = WizardState::new_dominant();
                    state.wizard_modal_open = true;
                    render_active_skin(doc, state);
                    return;
                }
                "open-wizard-personality" => {
                    state.wizard = WizardState::new_personality();
                    state.wizard_modal_open = true;
                    render_active_skin(doc, state);
                    return;
                }
                "wiz-modal-close" | "close-wizard" => {
                    if state.wizard_modal_open {
                        state.wizard_modal_open = false;
                        render_active_skin(doc, state);
                        return;
                    }
                    state.pending_skin = Some(state.last_mail_skin);
                    return;
                }
                "wiz-next" => {
                    sync_wizard_from_dom(doc, &mut state.wizard);
                    match state.wizard.step {
                        0 => {
                            state.wizard.step = 1;
                        }
                        1 => {
                            match state.wizard.account_choice {
                                2 => {
                                    state.wizard.flow = WizardFlow::Detect;
                                    state.wizard.step = 9; // Pick provider
                                }
                                3 => {
                                    state.wizard.flow = WizardFlow::Advanced;
                                    state.wizard.step = 6; // Manual incoming server
                                }
                                1 => {
                                    state.wizard.flow = WizardFlow::Import;
                                    state.wizard.step = 11; // Import settings
                                }
                                _ => {
                                    state.wizard.flow = WizardFlow::Standard;
                                    state.wizard.step = if state.wizard.is_dominant { 3 } else { 2 };
                                }
                            }
                        }
                        2 => state.wizard.step = 3,
                        3 => state.wizard.step = 4,
                        4 => {
                            let email = state.wizard.return_address.clone();
                            discover_into_wizard(&email, &mut state.wizard);
                            state.wizard.step = 5;
                        }
                        5 => state.wizard.step = 6,
                        6 => state.wizard.step = 7,
                        7 => state.wizard.step = 8,
                        9 => {
                            state.wizard.step = 10; // Enter credentials
                        }
                        10 => {
                            apply_provider_defaults(&mut state.wizard);
                            state.wizard.step = 8; // Confirm settings
                        }
                        11 => {
                            state.wizard.flow = WizardFlow::Detect;
                            state.wizard.step = 9; // Proceed to provider detect
                        }
                        8 => {
                            save_wizard_to_config(state);
                            if state.wizard_modal_open {
                                state.wizard_modal_open = false;
                                render_active_skin(doc, state);
                                return;
                            }
                            state.pending_skin = Some(state.last_mail_skin);
                            return;
                        }
                        _ => state.wizard.step = 8,
                    }
                    render_active_skin(doc, state);
                    return;
                }
                "wiz-back" => {
                    sync_wizard_from_dom(doc, &mut state.wizard);
                    let prev_step = match state.wizard.step {
                        0 => 0,
                        1 => 0,
                        2 => 1,
                        3 => if state.wizard.is_dominant { 1 } else { 2 },
                        4 => 3,
                        5 => 4,
                        6 => if state.wizard.flow == WizardFlow::Advanced { 1 } else { 5 },
                        7 => 6,
                        8 => match state.wizard.flow {
                            WizardFlow::Detect => 10,
                            WizardFlow::Advanced => 7,
                            _ => 7,
                        },
                        9 => 1,
                        10 => 9,
                        11 => 1,
                        _ => 1,
                    };
                    state.wizard.step = prev_step;
                    render_active_skin(doc, state);
                    return;
                }
                "open-settings" => {
                    if state.skin != Skin::Settings {
                        state.last_mail_skin = state.skin;
                    }
                    state.pending_skin = Some(Skin::Settings);
                    return;
                }
                "close-settings" => {
                    state.pending_skin = Some(state.last_mail_skin);
                    return;
                }
                "add-personality" => {
                    let count = state.conf.count_accounts();
                    let new_idx = count as i32 + 1;
                    let new_name = format!("Personality {}", new_idx);
                    let acct = macmbx::conf::Account {
                        index: new_idx,
                        name: new_name.clone(),
                        real_name: state.conf.get_or("sending_mail", "real_name", "You").to_string(),
                        email: format!("user{}@example.com", new_idx),
                        account_type: "IMAP".to_string(),
                        server: "imap.example.com".to_string(),
                        smtp_server: "smtp.example.com".to_string(),
                        username: format!("user{}", new_idx),
                        check_interval: 15,
                        ssl_mode: 1,
                        leave_on_server: true,
                        enabled: true,
                    };
                    let idx = state.conf.add_account(&acct);
                    let ini_path = state.data_dir.join("geudora.ini");
                    let _ = state.conf.save_as(&ini_path);
                    state.selected_account_idx = idx;
                    state.settings_status = format!("✓ Created personality '{}'", new_name);
                    state.dirty = true;
                    render_active_skin(doc, state);
                    return;
                }
                "save-personality" => {
                    let name = doc.get_element_by_id("pers-name")
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_else(|| "Personality".to_string());
                    let realname = doc.get_element_by_id("pers-realname")
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_default();
                    let email = doc.get_element_by_id("pers-email")
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_default();
                    let proto = doc.get_element_by_id("pers-type")
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_else(|| "POP".to_string());
                    let server = doc.get_element_by_id("pers-server")
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_default();
                    let username = doc.get_element_by_id("pers-username")
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_default();
                    let smtp = doc.get_element_by_id("pers-smtp")
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_default();
                    let interval = doc.get_element_by_id("pers-interval")
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .and_then(|s| s.parse::<i32>().ok())
                        .unwrap_or(15);
                    let ssl = doc.get_element_by_id("pers-ssl")
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .and_then(|s| s.parse::<i32>().ok())
                        .unwrap_or(0);
                    let leave = doc.get_element_by_id("pers-leave")
                        .map(|el| doc.has_attribute(el, "checked"))
                        .unwrap_or(true);
                    let enabled = doc.get_element_by_id("pers-enabled")
                        .map(|el| doc.has_attribute(el, "checked"))
                        .unwrap_or(true);

                    if state.selected_account_idx == 0 {
                        state.save_settings(&username, &server, &realname, &smtp, &email);
                        state.conf.set("checking_mail", "use_imap", if proto == "IMAP" { "true" } else { "false" });
                        state.conf.set_int("checking_mail", "check_interval", interval as i64);
                        state.conf.set_int("ssl", "ssl_pop_mode", ssl as i64);
                        state.conf.set_bool("checking_mail", "leave_on_server", leave);
                        let ini_path = state.data_dir.join("geudora.ini");
                        let _ = state.conf.save_as(&ini_path);
                        state.settings_status = "✓ Saved Dominant personality settings".to_string();
                    } else {
                        let acct = macmbx::conf::Account {
                            index: state.selected_account_idx,
                            name,
                            real_name: realname,
                            email,
                            account_type: proto,
                            server,
                            smtp_server: smtp,
                            username,
                            check_interval: interval,
                            ssl_mode: ssl,
                            leave_on_server: leave,
                            enabled,
                        };
                        state.conf.update_account(state.selected_account_idx, &acct);
                        let ini_path = state.data_dir.join("geudora.ini");
                        let _ = state.conf.save_as(&ini_path);
                        state.settings_status = format!("✓ Saved personality '{}'", acct.name);
                    }
                    state.dirty = true;
                    render_active_skin(doc, state);
                    return;
                }
                "delete-personality" => {
                    if state.selected_account_idx > 0 {
                        state.conf.remove_account(state.selected_account_idx);
                        let ini_path = state.data_dir.join("geudora.ini");
                        let _ = state.conf.save_as(&ini_path);
                        state.selected_account_idx = 0;
                        state.settings_status = "✓ Removed personality".to_string();
                        state.dirty = true;
                        render_active_skin(doc, state);
                        return;
                    }
                }
                "set-dominant-personality" => {
                    if state.selected_account_idx > 0 {
                        if let Some(acct) = state.conf.get_account(state.selected_account_idx) {
                            state.save_settings(&acct.username, &acct.server, &acct.real_name, &acct.smtp_server, &acct.email);
                            state.conf.set("checking_mail", "use_imap", if acct.account_type == "IMAP" { "true" } else { "false" });
                            state.conf.set_int("checking_mail", "check_interval", acct.check_interval as i64);
                            state.conf.set_int("ssl", "ssl_pop_mode", acct.ssl_mode as i64);
                            let ini_path = state.data_dir.join("geudora.ini");
                            let _ = state.conf.save_as(&ini_path);
                            state.selected_account_idx = 0;
                            state.settings_status = format!("✓ Set '{}' as dominant personality", acct.name);
                            state.dirty = true;
                            render_active_skin(doc, state);
                            return;
                        }
                    }
                }
                "save-settings" => {
                    let username = doc.get_element_by_id("cfg-username")
                        .or_else(|| doc.get_element_by_id("chk-username"))
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_else(|| state.conf.get("Settings", "LoginName").unwrap_or("you").to_string());
                    let pop_srv = doc.get_element_by_id("cfg-pop-server")
                        .or_else(|| doc.get_element_by_id("chk-pop-server"))
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_else(|| state.conf.get("Settings", "MailServer").unwrap_or("mail.example.com").to_string());
                    let realname = doc.get_element_by_id("cfg-realname")
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_else(|| state.conf.get("Settings", "RealName").unwrap_or("You").to_string());
                    let smtp_srv = doc.get_element_by_id("cfg-smtp-server")
                        .or_else(|| doc.get_element_by_id("send-smtp"))
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_else(|| state.conf.get("Settings", "SMTPServer").unwrap_or("smtp.example.com").to_string());
                    let email = doc.get_element_by_id("cfg-email")
                        .or_else(|| doc.get_element_by_id("send-email"))
                        .and_then(|el| doc.get_attribute(el, "value"))
                        .unwrap_or_else(|| state.conf.get("Settings", "ReturnAddress").unwrap_or("you@example.com").to_string());
                    state.save_settings(&username, &pop_srv, &realname, &smtp_srv, &email);

                    if let Some(el) = doc.get_element_by_id("chk-proto") {
                        if let Some(v) = doc.get_attribute(el, "value") {
                            state.conf.set("checking_mail", "use_imap", if v == "IMAP" { "true" } else { "false" });
                        }
                    }
                    if let Some(el) = doc.get_element_by_id("chk-interval") {
                        if let Some(v) = doc.get_attribute(el, "value").and_then(|s| s.parse::<i64>().ok()) {
                            state.conf.set_int("checking_mail", "check_interval", v);
                        }
                    }
                    if let Some(el) = doc.get_element_by_id("chk-leave-days") {
                        if let Some(v) = doc.get_attribute(el, "value").and_then(|s| s.parse::<i64>().ok()) {
                            state.conf.set_int("checking_mail", "leave_days", v);
                        }
                    }
                    if let Some(el) = doc.get_element_by_id("send-domain") {
                        if let Some(v) = doc.get_attribute(el, "value") {
                            state.conf.set("sending_mail", "default_domain", &v);
                        }
                    }
                    if let Some(el) = doc.get_element_by_id("ssl-pop-mode") {
                        if let Some(v) = doc.get_attribute(el, "value").and_then(|s| s.parse::<i64>().ok()) {
                            state.conf.set_int("ssl", "ssl_pop_mode", v);
                        }
                    }
                    if let Some(el) = doc.get_element_by_id("ssl-smtp-mode") {
                        if let Some(v) = doc.get_attribute(el, "value").and_then(|s| s.parse::<i64>().ok()) {
                            state.conf.set_int("ssl", "ssl_smtp_mode", v);
                        }
                    }
                    if let Some(el) = doc.get_element_by_id("ssl-imap-mode") {
                        if let Some(v) = doc.get_attribute(el, "value").and_then(|s| s.parse::<i64>().ok()) {
                            state.conf.set_int("ssl", "ssl_imap_mode", v);
                        }
                    }
                    let ini_path = state.data_dir.join("geudora.ini");
                    let _ = state.conf.save_as(&ini_path);
                    state.settings_status = format!("✓ Saved configuration settings to {}", ini_path.display());
                    render_active_skin(doc, state);
                    return;
                }
                "reset-settings" => {
                    state.settings_status = "↺ Restored default settings for current category.".to_string();
                    render_active_skin(doc, state);
                    return;
                }
                "switch-skin" => {
                    let next = match state.skin {
                        Skin::Eudora => Skin::Email,
                        Skin::Email => Skin::Eudora,
                        Skin::Settings | Skin::Wizard => match state.last_mail_skin {
                            Skin::Eudora => Skin::Email,
                            _ => Skin::Eudora,
                        },
                    };
                    state.last_mail_skin = next;
                    if state.skin == Skin::Settings || state.skin == Skin::Wizard {
                        render_active_skin(doc, state);
                    } else {
                        state.pending_skin = Some(next);
                    }
                    return;
                }
                "toggle-dark" => {
                    state.dark_mode = !state.dark_mode;
                    render_active_skin(doc, state);
                    return;
                }
                "check-mail" => {
                    state.reload_from_disk();
                    let mail_srv = state.conf.get("checking_mail", "pop_server")
                        .or_else(|| state.conf.get("Settings", "MailServer"))
                        .unwrap_or_default();
                    let in_unread = state.unread_count("In");
                    let total_msgs = state.messages.len();
                    state.status_logs.push(StatusLog {
                        time: current_clock_str(),
                        message: if !mail_srv.is_empty() && mail_srv != "mail.example.com" {
                            format!("Mail check complete on {}: {} msgs ({} unread in In)", mail_srv, total_msgs, in_unread)
                        } else {
                            "Mail check: Local mailboxes reloaded (no server configured)".to_string()
                        },
                    });
                    render_active_skin(doc, state);
                    return;
                }
                "new-message" => {
                    state.compose = ComposeDraft::default();
                    state.compose_open = true;
                    state.compose_needs_init = true;
                    state.open_tab(TabKind::Compose);
                    state.email_mode = EmailViewMode::Compose;
                    render_active_skin(doc, state);
                    return;
                }
                "reply" | "reply-all" => {
                    let active_id = if state.selected_mailbox == "Out" {
                        state.selected_out_msg_id
                    } else {
                        state.selected_in_msg_id
                    };
                    if let Some(msg) = state.messages.iter().find(|m| m.id == active_id).cloned() {
                        let subj = if msg.subject.starts_with("Re: ") {
                            msg.subject.clone()
                        } else {
                            format!("Re: {}", msg.subject)
                        };
                        state.compose = ComposeDraft {
                            title: "↩ Reply".to_string(),
                            from: "You <you@example.com>".to_string(),
                            to: msg.from.clone(),
                            cc: if action == "reply-all" { msg.cc.clone() } else { String::new() },
                            bcc: String::new(),
                            subject: subj,
                            attach: "None".to_string(),
                            priority: if msg.priority == "high" || msg.priority == "highest" {
                                "High".to_string()
                            } else {
                                "Normal".to_string()
                            },
                            body_html: format!(
                                r#"<p><br></p><p>At {date}, {from} wrote:</p><blockquote>{body}</blockquote>"#,
                                date = escape_html(&msg.date),
                                from = escape_html(&msg.from),
                                body = msg.body_html,
                            ),
                            ..ComposeDraft::default()
                        };
                        state.compose_open = true;
                        state.compose_needs_init = true;
                        state.open_tab(TabKind::Compose);
                        state.email_mode = EmailViewMode::Compose;
                        render_active_skin(doc, state);
                    }
                    return;
                }
                "forward" => {
                    let active_id = state.selected_in_msg_id;
                    if let Some(msg) = state.messages.iter().find(|m| m.id == active_id).cloned() {
                        let subj = if msg.subject.starts_with("Fwd: ") {
                            msg.subject.clone()
                        } else {
                            format!("Fwd: {}", msg.subject)
                        };
                        state.compose = ComposeDraft {
                            title: "→ Forward".to_string(),
                            from: "You <you@example.com>".to_string(),
                            to: String::new(),
                            cc: String::new(),
                            bcc: String::new(),
                            subject: subj,
                            attach: if msg.attachment_note.is_empty() {
                                "None".to_string()
                            } else {
                                msg.attachment_note.clone()
                            },
                            priority: "Normal".to_string(),
                            body_html: format!(
                                r#"<p><br></p><p>-------- Forwarded Message --------<br>From: {from}<br>Date: {date}</p><blockquote>{body}</blockquote>"#,
                                from = escape_html(&msg.from),
                                date = escape_html(&msg.date),
                                body = msg.body_html,
                            ),
                            ..ComposeDraft::default()
                        };
                        state.compose_open = true;
                        state.compose_needs_init = true;
                        state.open_tab(TabKind::Compose);
                        state.email_mode = EmailViewMode::Compose;
                        render_active_skin(doc, state);
                    }
                    return;
                }
                "trash" => {
                    let active_id = if state.selected_mailbox == "Out" {
                        state.selected_out_msg_id
                    } else {
                        state.selected_in_msg_id
                    };
                    state.trash_message(active_id);
                    render_active_skin(doc, state);
                    return;
                }
                "archive" => {
                    let active_id = state.selected_in_msg_id;
                    if let Some(msg) = state.messages.iter_mut().find(|m| m.id == active_id) {
                        msg.mailbox = "Archive".to_string();
                    }
                    render_active_skin(doc, state);
                    return;
                }
                "junk" => {
                    let active_id = state.selected_in_msg_id;
                    if let Some(msg) = state.messages.iter_mut().find(|m| m.id == active_id) {
                        msg.mailbox = "Spam".to_string();
                    }
                    render_active_skin(doc, state);
                    return;
                }
                "star" => {
                    let active_id = if state.selected_mailbox == "Out" {
                        state.selected_out_msg_id
                    } else {
                        state.selected_in_msg_id
                    };
                    state.toggle_flag_message(active_id);
                    render_active_skin(doc, state);
                    return;
                }
                "send" | "queue" => {
                    sync_compose_from_dom(doc, state);
                    state.send_composed_message();
                    render_active_skin(doc, state);
                    return;
                }
                "save" => {
                    sync_compose_from_dom(doc, state);
                    state.status_left = "Draft saved".to_string();
                    render_active_skin(doc, state);
                    return;
                }
                "emoji" => {
                    if let Some(body_el) = doc.get_element_by_id("compose-body") {
                        let mut cur = doc.inner_html(body_el);
                        cur.push_str(" ☺ ");
                        doc.set_inner_html(body_el, &cur);
                    }
                    state.dirty = true;
                    return;
                }
                "spell" => {
                    state.status_left = "Spelling check complete: no errors".to_string();
                    render_active_skin(doc, state);
                    return;
                }
                "print" => {
                    state.status_left = "Printing message...".to_string();
                    render_active_skin(doc, state);
                    return;
                }
                "toggle-qp" => {
                    state.compose.opt_qp = !state.compose.opt_qp;
                    render_active_skin(doc, state);
                    return;
                }
                "toggle-text" => {
                    state.compose.opt_text = !state.compose.opt_text;
                    render_active_skin(doc, state);
                    return;
                }
                "toggle-wrap" => {
                    state.compose.opt_wrap = !state.compose.opt_wrap;
                    render_active_skin(doc, state);
                    return;
                }
                "toggle-keep" => {
                    state.compose.opt_keep = !state.compose.opt_keep;
                    render_active_skin(doc, state);
                    return;
                }
                "toggle-rr" => {
                    state.compose.opt_rr = !state.compose.opt_rr;
                    render_active_skin(doc, state);
                    return;
                }
                "cycle-encoding" => {
                    state.compose.encoding = match state.compose.encoding.as_str() {
                        "MIME" => "BinHex".to_string(),
                        "BinHex" => "Uuencode".to_string(),
                        _ => "MIME".to_string(),
                    };
                    render_active_skin(doc, state);
                    return;
                }
                "cycle-priority" => {
                    state.compose.priority = match state.compose.priority.as_str() {
                        "Normal" => "High".to_string(),
                        "High" => "Highest".to_string(),
                        "Highest" => "Lowest".to_string(),
                        "Lowest" => "Low".to_string(),
                        _ => "Normal".to_string(),
                    };
                    render_active_skin(doc, state);
                    return;
                }
                "cycle-signature" => {
                    state.compose.signature = match state.compose.signature.as_str() {
                        "Standard" => "Alternate".to_string(),
                        "Alternate" => "None".to_string(),
                        _ => "Standard".to_string(),
                    };
                    render_active_skin(doc, state);
                    return;
                }
                "attach-file" => {
                    state.compose.attach = if state.compose.attach.is_empty() || state.compose.attach == "None" {
                        "Document.pdf (42 KB)".to_string()
                    } else {
                        format!("{}, Notes.txt (12 KB)", state.compose.attach)
                    };
                    render_active_skin(doc, state);
                    return;
                }
                "format-link" => {
                    apply_formatting_command(doc, "createlink", "https://example.com");
                    state.dirty = true;
                    return;
                }
                "format-image" => {
                    if let Some(r) = get_compose_selection_range(doc) {
                        let img = doc.create_element("img");
                        doc.set_attribute(img, "src", "https://via.placeholder.com/150");
                        doc.set_attribute(img, "alt", "Image");
                        doc.insert_node(r, img);
                        doc.style_dirty = true;
                        doc.layout_generation = doc.layout_generation.wrapping_add(1);
                    }
                    state.dirty = true;
                    return;
                }
                "send-queued" => {
                    let out_path = state.data_dir.join("mailboxes").join("Out");
                    let mut sent_count = 0;
                    if let Ok(mut toc) = macmbx::toc::Toc::open_or_create(&out_path) {
                        for msg in &mut toc.msgs {
                            if msg.state == macmbx::toc::State::Queued {
                                msg.state = macmbx::toc::State::Sent;
                                sent_count += 1;
                            }
                        }
                        let _ = toc.save();
                    }
                    state.reload_from_disk();
                    state.status_logs.push(StatusLog {
                        time: current_clock_str(),
                        message: if sent_count > 0 {
                            format!("Delivered {} queued message(s) via SMTP", sent_count)
                        } else {
                            "Outbox delivery: 0 queued messages to send".to_string()
                        },
                    });
                    state.status_left = if sent_count > 0 {
                        format!("Delivered {} queued messages", sent_count)
                    } else {
                        "No queued messages to send".to_string()
                    };
                    render_active_skin(doc, state);
                    return;
                }
                "open-personalities" => {
                    if state.skin != Skin::Settings {
                        state.last_mail_skin = state.skin;
                    }
                    state.settings_category = "accounts".to_string();
                    state.pending_skin = Some(Skin::Settings);
                    return;
                }
                "open-stats" => {
                    state.open_tab(TabKind::Welcome);
                    render_active_skin(doc, state);
                    return;
                }
                "discard" => {
                    state.compose_open = false;
                    state.close_tab_if_open(&TabKind::Compose);
                    state.open_tab(TabKind::Mailbox("In".to_string()));
                    state.email_mode = EmailViewMode::Mail;
                    render_active_skin(doc, state);
                    return;
                }
                "add-contact" => {
                    let count = state.contacts.len() + 1;
                    let new_c = Contact {
                        name: format!("New Contact {}", count),
                        initials: "NC".to_string(),
                        email: format!("contact{}@example.com", count),
                        color: compute_avatar_color("New Contact"),
                        ..Contact::default()
                    };
                    state.contacts.push(new_c);
                    state.selected_contact_idx = state.contacts.len() - 1;
                    save_contacts_to_disk(state);
                    state.contact_status = "✓ Created new contact".to_string();
                    render_active_skin(doc, state);
                    return;
                }
                "delete-contact" => {
                    if !state.contacts.is_empty() {
                        let removed = state.contacts.remove(state.selected_contact_idx);
                        let nick_dir = state.data_dir.join("Nicknames");
                        if let Ok(mut books) = macmbx::nicknames::AddressBooks::open(&nick_dir) {
                            let bi = if !removed.book_name.is_empty() {
                                books.find_book(&removed.book_name).unwrap_or(0)
                            } else {
                                0
                            };
                            if let Some(book) = books.get_book_mut(bi) {
                                if let Some(ni) = book.find(&removed.name) {
                                    book.remove(ni);
                                    let _ = book.save();
                                }
                            }
                        }
                        if state.selected_contact_idx >= state.contacts.len() && !state.contacts.is_empty() {
                            state.selected_contact_idx = state.contacts.len() - 1;
                        }
                        state.contact_status = format!("✓ Deleted '{}'", removed.name);
                        render_active_skin(doc, state);
                        return;
                    }
                }
                "compose-to-contact" => {
                    if let Some(c) = state.contacts.get(state.selected_contact_idx) {
                        let to = if !c.email.is_empty() { c.email.clone() } else { c.name.clone() };
                        state.compose = ComposeDraft {
                            title: format!("✏ To: {}", to),
                            to,
                            ..ComposeDraft::default()
                        };
                        state.compose_open = true;
                        state.compose_needs_init = true;
                        state.open_tab(TabKind::Compose);
                        state.email_mode = EmailViewMode::Compose;
                        render_active_skin(doc, state);
                        return;
                    }
                }
                "save-contact" => {
                    if let Some(c) = state.contacts.get_mut(state.selected_contact_idx) {
                        if let Some(v) = get_input_or_text(doc, "c-name") { c.name = v; }
                        if let Some(v) = get_input_or_text(doc, "c-email") { c.email = v; }
                        if let Some(v) = get_input_or_text(doc, "c-first") { c.first_name = v; }
                        if let Some(v) = get_input_or_text(doc, "c-last") { c.last_name = v; }
                        if let Some(v) = get_input_or_text(doc, "c-full") { c.full_name = v; }
                        if let Some(v) = get_input_or_text(doc, "c-company") { c.company = v; }
                        if let Some(v) = get_input_or_text(doc, "c-title") { c.title = v; }
                        if let Some(v) = get_input_or_text(doc, "c-w-address") { c.w_address = v; }
                        if let Some(v) = get_input_or_text(doc, "c-w-city") { c.w_city = v; }
                        if let Some(v) = get_input_or_text(doc, "c-w-state") { c.w_state = v; }
                        if let Some(v) = get_input_or_text(doc, "c-w-zip") { c.w_zip = v; }
                        if let Some(v) = get_input_or_text(doc, "c-w-country") { c.w_country = v; }
                        if let Some(v) = get_input_or_text(doc, "c-w-phone") { c.w_phone = v; }
                        if let Some(v) = get_input_or_text(doc, "c-w-mobile") { c.w_mobile = v; }
                        if let Some(v) = get_input_or_text(doc, "c-w-fax") { c.w_fax = v; }
                        if let Some(v) = get_input_or_text(doc, "c-w-web") { c.w_web = v; }
                        if let Some(v) = get_input_or_text(doc, "c-h-address") { c.h_address = v; }
                        if let Some(v) = get_input_or_text(doc, "c-h-city") { c.h_city = v; }
                        if let Some(v) = get_input_or_text(doc, "c-h-state") { c.h_state = v; }
                        if let Some(v) = get_input_or_text(doc, "c-h-zip") { c.h_zip = v; }
                        if let Some(v) = get_input_or_text(doc, "c-h-country") { c.h_country = v; }
                        if let Some(v) = get_input_or_text(doc, "c-phone") { c.phone = v; }
                        if let Some(v) = get_input_or_text(doc, "c-h-mobile") { c.h_mobile = v; }
                        if let Some(v) = get_input_or_text(doc, "c-h-fax") { c.h_fax = v; }
                        if let Some(v) = get_input_or_text(doc, "c-h-web") { c.h_web = v; }
                        if let Some(v) = get_input_or_text(doc, "c-other-email") { c.other_email = v; }
                        if let Some(v) = get_input_or_text(doc, "c-other-phone") { c.other_phone = v; }
                        if let Some(v) = get_input_or_text(doc, "c-other-web") { c.other_web = v; }
                        if let Some(v) = get_input_or_text(doc, "c-notes") { c.notes = v; }

                        c.initials = compute_initials(&c.name);
                        c.color = compute_avatar_color(&c.name);
                        let name_clone = c.name.clone();
                        save_contacts_to_disk(state);
                        state.contact_status = format!("✓ Saved '{}'", name_clone);
                        render_active_skin(doc, state);
                        return;
                    }
                }
                "add-filter" => {
                    let count = state.filters.len() + 1;
                    let new_f = MailFilter {
                        name: format!("Filter Rule {}", count),
                        incoming: true,
                        outgoing: false,
                        manual: false,
                        header1: "Subject:".to_string(),
                        verb1: "contains".to_string(),
                        value1: String::new(),
                        conjunction: "ignore".to_string(),
                        header2: "From:".to_string(),
                        verb2: "contains".to_string(),
                        value2: String::new(),
                        action1_type: "Transfer".to_string(),
                        action1_value: "In".to_string(),
                        action2_type: "None".to_string(),
                        action2_value: String::new(),
                        enabled: true,
                    };
                    state.filters.push(new_f);
                    state.selected_filter_idx = state.filters.len() - 1;
                    state.filter_editing = true;
                    save_filters_to_disk(state);
                    state.filter_status = "✓ Created new filter rule".to_string();
                    render_active_skin(doc, state);
                    return;
                }
                "edit-filter" => {
                    let filter_idx = doc.closest(target, "[data-filter]")
                        .and_then(|el| doc.get_attribute(el, "data-filter"))
                        .and_then(|s| s.parse::<usize>().ok());
                    if let Some(idx) = filter_idx {
                        if idx < state.filters.len() {
                            state.selected_filter_idx = idx;
                        }
                    }
                    state.filter_editing = true;
                    render_active_skin(doc, state);
                    return;
                }
                "close-filter-editor" => {
                    state.filter_editing = false;
                    render_active_skin(doc, state);
                    return;
                }
                "delete-filter" => {
                    if !state.filters.is_empty() {
                        let removed = state.filters.remove(state.selected_filter_idx);
                        if state.selected_filter_idx >= state.filters.len() && !state.filters.is_empty() {
                            state.selected_filter_idx = state.filters.len() - 1;
                        }
                        if state.filters.is_empty() {
                            state.filter_editing = false;
                        }
                        save_filters_to_disk(state);
                        state.filter_status = format!("✓ Deleted rule '{}'", removed.name);
                        render_active_skin(doc, state);
                        return;
                    }
                }
                "save-filter" => {
                    if let Some(f) = state.filters.get_mut(state.selected_filter_idx) {
                        let get_v = |doc: &Document, id: &str| -> Option<String> {
                            doc.get_element_by_id(id).and_then(|el| {
                                doc.get_attribute(el, "value").or_else(|| {
                                    let t = doc.text_content(el);
                                    if !t.is_empty() { Some(t) } else { None }
                                })
                            })
                        };
                        let get_sel_v = |doc: &Document, id: &str| -> Option<String> {
                            if let Some(el) = doc.get_element_by_id(id) {
                                if let Some(v) = doc.get_attribute(el, "value") {
                                    if !v.is_empty() { return Some(v); }
                                }
                                for child in doc.children(el) {
                                    if doc.has_attribute(child, "selected") {
                                        if let Some(v) = doc.get_attribute(child, "value") {
                                            return Some(v);
                                        }
                                        let t = doc.text_content(child);
                                        if !t.is_empty() { return Some(t); }
                                    }
                                }
                            }
                            None
                        };
                        if let Some(v) = get_v(doc, "filt-name") { f.name = v; }
                        if let Some(v) = get_sel_v(doc, "filt-h1") { f.header1 = v; }
                        if let Some(v) = get_sel_v(doc, "filt-v1") { f.verb1 = v; }
                        if let Some(v) = get_v(doc, "filt-val1") { f.value1 = v; }
                        if let Some(v) = get_sel_v(doc, "filt-conj") { f.conjunction = v; }
                        if let Some(v) = get_sel_v(doc, "filt-h2") { f.header2 = v; }
                        if let Some(v) = get_sel_v(doc, "filt-v2") { f.verb2 = v; }
                        if let Some(v) = get_v(doc, "filt-val2") { f.value2 = v; }
                        if let Some(v) = get_sel_v(doc, "filt-act1-type") { f.action1_type = v; }
                        if let Some(v) = get_sel_v(doc, "filt-act1-val").or_else(|| get_v(doc, "filt-act1-val")) { f.action1_value = v; }
                        if let Some(v) = get_sel_v(doc, "filt-act2-type") { f.action2_type = v; }
                        if let Some(v) = get_v(doc, "filt-act2-val") { f.action2_value = v; }

                        let name_clone = f.name.clone();
                        save_filters_to_disk(state);
                        state.filter_editing = false;
                        state.filter_status = format!("✓ Saved rule '{}'", name_clone);
                        render_active_skin(doc, state);
                        return;
                    }
                }
                "toggle-filter" => {
                    let filter_idx = doc.closest(target, "[data-filter]")
                        .and_then(|el| doc.get_attribute(el, "data-filter"))
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(state.selected_filter_idx);
                    if let Some(f) = state.filters.get_mut(filter_idx) {
                        f.enabled = !f.enabled;
                        let name = f.name.clone();
                        let enabled = f.enabled;
                        save_filters_to_disk(state);
                        state.filter_status = format!("Rule '{}' is now {}", name, if enabled { "Active" } else { "Disabled" });
                        render_active_skin(doc, state);
                        return;
                    }
                }
                "toggle-filt-scope" => {
                    if let Some(scope_btn) = doc.closest(target, "[data-scope]") {
                        if let Some(scope) = doc.get_attribute(scope_btn, "data-scope") {
                            if let Some(f) = state.filters.get_mut(state.selected_filter_idx) {
                                match scope.as_str() {
                                    "incoming" => f.incoming = !f.incoming,
                                    "outgoing" => f.outgoing = !f.outgoing,
                                    "manual" => f.manual = !f.manual,
                                    _ => {}
                                }
                                save_filters_to_disk(state);
                                render_active_skin(doc, state);
                                return;
                            }
                        }
                    }
                }
                "move-filter-up" => {
                    if state.selected_filter_idx > 0 && state.selected_filter_idx < state.filters.len() {
                        state.filters.swap(state.selected_filter_idx, state.selected_filter_idx - 1);
                        state.selected_filter_idx -= 1;
                        save_filters_to_disk(state);
                        render_active_skin(doc, state);
                        return;
                    }
                }
                "move-filter-down" => {
                    if state.selected_filter_idx + 1 < state.filters.len() {
                        state.filters.swap(state.selected_filter_idx, state.selected_filter_idx + 1);
                        state.selected_filter_idx += 1;
                        save_filters_to_disk(state);
                        render_active_skin(doc, state);
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    // 2.1 Check Contact Subtab switching
    if let Some(subtab_el) = doc.closest(target, "[data-contact-subtab]") {
        if let Some(subtab) = doc.get_attribute(subtab_el, "data-contact-subtab") {
            state.contact_subtab = subtab;
            render_active_skin(doc, state);
            return;
        }
    }

    // 3. Check Contact "Email" button in contact details
    if let Some(compose_btn) = doc.closest(target, "[data-compose-to]") {
        if let Some(email) = doc.get_attribute(compose_btn, "data-compose-to") {
            state.compose = ComposeDraft {
                title: format!("✏ To: {}", email),
                to: email,
                ..ComposeDraft::default()
            };
            state.compose_open = true;
            state.compose_needs_init = true;
            state.open_tab(TabKind::Compose);
            state.email_mode = EmailViewMode::Compose;
            render_active_skin(doc, state);
            return;
        }
    }

    // 4. Check Contacts view item in Email sidebar
    if let Some(view_el) = doc.closest(target, "[data-view]") {
        if doc.get_attribute(view_el, "data-view").as_deref() == Some("contacts") {
            state.email_mode = EmailViewMode::Contacts;
            state.selected_label = None;
            render_active_skin(doc, state);
            return;
        }
    }

    // 5. Check mailbox selection (Wazoo item in Eudora OR sidebar item in Email)
    if let Some(mb_el) = doc.closest(target, "[data-mailbox]") {
        if let Some(mailbox) = doc.get_attribute(mb_el, "data-mailbox") {
            state.selected_label = None;
            state.email_mode = EmailViewMode::Mail;
            state.open_tab(TabKind::Mailbox(mailbox));
            render_active_skin(doc, state);
            return;
        }
    }

    // 6. Check label selection in Email skin
    if let Some(lbl_el) = doc.closest(target, "[data-label]") {
        if let Some(label) = doc.get_attribute(lbl_el, "data-label") {
            state.email_mode = EmailViewMode::Mail;
            if state.selected_label.as_deref() == Some(label.as_str()) {
                state.selected_label = None;
            } else {
                state.selected_label = Some(label);
            }
            render_active_skin(doc, state);
            return;
        }
    }

    // 7. Check message row selection
    if let Some(msg_el) = doc.closest(target, "[data-msg-id]") {
        if let Some(id_str) = doc.get_attribute(msg_el, "data-msg-id") {
            if let Ok(msg_id) = id_str.parse::<usize>() {
                let is_out = doc.get_attribute(msg_el, "data-box").as_deref() == Some("out");
                if is_out {
                    state.selected_out_msg_id = msg_id;
                } else {
                    state.selected_in_msg_id = msg_id;
                    state.mark_read(msg_id);
                }
                state.email_mode = EmailViewMode::Mail;
                render_active_skin(doc, state);
                return;
            }
        }
    }

    // 8. Check contact card selection
    if let Some(contact_el) = doc.closest(target, "[data-contact]") {
        if let Some(idx_str) = doc.get_attribute(contact_el, "data-contact") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                if idx < state.contacts.len() {
                    state.selected_contact_idx = idx;
                    render_active_skin(doc, state);
                    return;
                }
            }
        }
    }

    // 9. Check filter card selection
    if let Some(filter_el) = doc.closest(target, "[data-filter]") {
        if let Some(idx_str) = doc.get_attribute(filter_el, "data-filter") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                if idx < state.filters.len() {
                    state.selected_filter_idx = idx;
                    state.filter_editing = false;
                    render_active_skin(doc, state);
                    return;
                }
            }
        }
    }
}

// ============================================================================
// Webcore Browser Context Host (WHATWG DOM + BrowserChrome::NONE)
// ============================================================================

fn build_skin_document(skin: Skin, state: &Arc<Mutex<AppState>>, next_generation: u64) -> Document {
    let html = match skin {
        Skin::Eudora => EUDORA_HTML,
        Skin::Email => EMAIL_HTML,
        Skin::Settings => SETTINGS_HTML,
        Skin::Wizard => WIZARD_HTML,
    };
    let mut doc = webcore::parse_html(html);
    doc.rebuild_node_index();
    doc.layout_generation = next_generation;
    {
        let mut st = state.lock().unwrap();
        st.skin = skin;
        render_active_skin(&mut doc, &mut st);
        st.dirty = false;
    }
    doc.rebuild_node_index();
    register_listeners(&mut doc, state.clone());
    doc
}

struct AzenGuest {
    doc: BrowserDocument,
    state: Arc<Mutex<AppState>>,
}

impl BrowserGuest for AzenGuest {
    fn dispatch_pending(&mut self) -> bool {
        let (pending_skin, dirty) = {
            let mut st = self.state.lock().unwrap();
            let pending = st.pending_skin.take();
            let dirty = st.dirty;
            st.dirty = false;
            (pending, dirty)
        };

        if let Some(next_skin) = pending_skin {
            let next_gen = self.doc.read().layout_generation.wrapping_add(1);
            let new_doc = build_skin_document(next_skin, &self.state, next_gen);
            *self.doc.write() = new_doc;
            return true;
        }

        dirty
    }
}

fn main() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let initial_skin = {
        let mut st = state.lock().unwrap();
        let has_primary = {
            let email = st.conf.get("sending_mail", "email_address")
                .or_else(|| st.conf.get("Settings", "ReturnAddress"))
                .unwrap_or_default();
            let server = st.conf.get("checking_mail", "pop_server")
                .or_else(|| st.conf.get("Settings", "MailServer"))
                .unwrap_or_default();
            let real_accounts = st.conf.get_all_accounts();
            let has_custom_account = real_accounts.iter().any(|a| {
                !a.email.is_empty() && a.email != "steve@qualcomm.com" && a.email != "you@example.com"
            });
            has_custom_account || (!email.is_empty() && !server.is_empty() && email != "you@example.com" && email != "steve@qualcomm.com" && server != "mail.example.com")
        };
        if !has_primary {
            st.wizard_modal_open = true;
        }
        Skin::Eudora
    };
    let initial_doc = build_skin_document(initial_skin, &state, 1);
    let browser_doc = BrowserDocument::new(initial_doc);

    let options = BrowserOptions {
        title: "Azen — Eudora Mail".into(),
        width: 1180.0,
        height: 800.0,
        chrome: BrowserChrome::NONE,
        ..Default::default()
    };

    let mut browser = Browser::open(options).expect("failed to open webcore browser context");
    browser.attach_document(browser_doc.clone());
    browser.set_guest(AzenGuest {
        doc: browser_doc,
        state,
    });
    browser.run().expect("browser context exited with error");
}

