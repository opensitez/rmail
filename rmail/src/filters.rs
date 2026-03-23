//! Filters tab — full rule editor.
//!
//! Port of filtwin.c CreateFiltersPanel / OpenFiltersWindow.
//! Layout: toolbar | HPaned(left=rule list, right=detail editor)

use gtk4::prelude::*;
use gtk4::glib;
use macmbx::filter::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

// ── CSS (port of theme.c filt-* classes) ──

const FILTER_CSS: &str = "
.filt-hero {
    background: linear-gradient(135deg, #1e3a5f 0%, #2d5f8a 100%);
    padding: 14px 20px; border-radius: 0 0 10px 10px;
}
.filt-hero-title { font-size: 1.1em; font-weight: 700; color: #ffffff; }
.filt-hero-sub { font-size: 0.78em; color: rgba(255,255,255,0.7); }
.filt-count-pill {
    background: rgba(255,255,255,0.20); color: #ffffff;
    border-radius: 12px; padding: 2px 10px;
    font-size: 0.78em; font-weight: 600;
}
.filt-badge-in {
    background: #3182ce; color: #ffffff; border-radius: 4px;
    padding: 1px 6px; font-size: 0.73em; font-weight: 700;
}
.filt-badge-out {
    background: #dd6b20; color: #ffffff; border-radius: 4px;
    padding: 1px 6px; font-size: 0.73em; font-weight: 700;
}
.filt-badge-man {
    background: #805ad5; color: #ffffff; border-radius: 4px;
    padding: 1px 6px; font-size: 0.73em; font-weight: 700;
}
.filt-dot { font-size: 1.2em; }
";

fn install_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(FILTER_CSS);
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

// ── Dot color for filter list (port of filtwin.c:2296-2318) ──

const LABEL_COLORS: &[&str] = &[
    "#e53e3e", "#3182ce", "#2f855a", "#dd6b20",
    "#805ad5", "#0694a2", "#97851a", "#718096",
];

fn dot_color_for_rule(rule: &Rule) -> &'static str {
    let mut has_label = false;
    let mut first_act = ActionType::None;
    for act in &rule.actions {
        if act.action_type == ActionType::Label {
            let idx = act.int_value.clamp(0, 7) as usize;
            has_label = true;
            return LABEL_COLORS[idx];
        }
        if first_act == ActionType::None && act.action_type != ActionType::None {
            first_act = act.action_type;
        }
    }
    if !has_label {
        match first_act {
            ActionType::Transfer | ActionType::Copy => return "#3182ce",
            ActionType::Stop => return "#e53e3e",
            ActionType::Junk => return "#dd6b20",
            ActionType::Forward | ActionType::Redirect => return "#2f855a",
            _ => {}
        }
    }
    "#94a3b8" // gray default
}

const HEADER_OPTIONS: &[&str] = &[
    "From:", "To:", "Cc:", "Subject:", "Reply-To:",
    "Any Recipient", "Any Header", "Body",
];

const VERB_UI: &[&str] = &[
    "contains", "doesn't contain", "is", "isn't",
    "starts with", "ends with", "appears", "doesn't appear",
    "matches regex",
];

const CONJ_UI: &[&str] = &["ignore", "and", "or", "unless"];

const ACTION_UI: &[&str] = &[
    "None", "Transfer To", "Copy To", "Set Status", "Set Priority",
    "Set Label", "Set Subject", "Play Sound", "Open Message",
    "Print", "Forward To", "Redirect To", "Reply With",
    "Mark as Junk", "Stop",
];

fn action_type_from_idx(idx: u32) -> ActionType {
    match idx {
        0 => ActionType::None,
        1 => ActionType::Transfer,
        2 => ActionType::Copy,
        3 => ActionType::Status,
        4 => ActionType::Priority,
        5 => ActionType::Label,
        6 => ActionType::Subject,
        7 => ActionType::Sound,
        8 => ActionType::Open,
        9 => ActionType::Print,
        10 => ActionType::Forward,
        11 => ActionType::Redirect,
        12 => ActionType::Reply,
        13 => ActionType::Junk,
        14 => ActionType::Stop,
        _ => ActionType::None,
    }
}

fn action_type_to_idx(at: ActionType) -> u32 {
    match at {
        ActionType::None => 0,
        ActionType::Transfer => 1,
        ActionType::Copy => 2,
        ActionType::Status => 3,
        ActionType::Priority => 4,
        ActionType::Label => 5,
        ActionType::Subject => 6,
        ActionType::Sound => 7,
        ActionType::Open => 8,
        ActionType::Print => 9,
        ActionType::Forward => 10,
        ActionType::Redirect => 11,
        ActionType::Reply => 12,
        ActionType::Junk => 13,
        ActionType::Stop => 14,
        _ => 0,
    }
}

fn verb_to_idx(v: Verb) -> u32 {
    match v {
        Verb::Contains => 0,
        Verb::NotContains => 1,
        Verb::Is => 2,
        Verb::IsNot => 3,
        Verb::StartsWith => 4,
        Verb::EndsWith => 5,
        Verb::Appears => 6,
        Verb::NotAppears => 7,
        Verb::Regex => 8,
        _ => 0,
    }
}

fn verb_from_idx(i: u32) -> Verb {
    match i {
        0 => Verb::Contains,
        1 => Verb::NotContains,
        2 => Verb::Is,
        3 => Verb::IsNot,
        4 => Verb::StartsWith,
        5 => Verb::EndsWith,
        6 => Verb::Appears,
        7 => Verb::NotAppears,
        8 => Verb::Regex,
        _ => Verb::Contains,
    }
}

fn header_to_idx(h: &str) -> u32 {
    for (i, opt) in HEADER_OPTIONS.iter().enumerate() {
        if opt.eq_ignore_ascii_case(h) {
            return i as u32;
        }
    }
    0
}

fn conj_to_idx(c: Conjunction) -> u32 {
    match c {
        Conjunction::None => 0,
        Conjunction::And => 1,
        Conjunction::Or => 2,
        Conjunction::Unless => 3,
    }
}

fn conj_from_idx(i: u32) -> Conjunction {
    match i {
        1 => Conjunction::And,
        2 => Conjunction::Or,
        3 => Conjunction::Unless,
        _ => Conjunction::None,
    }
}

// ── Action value widget builder ──
// Builds the appropriate value widget for a given action type.

fn build_action_value(at: ActionType, action: Option<&Action>, books: &Option<Rc<macmbx::nicknames::AddressBooks>>) -> gtk4::Widget {
    let str_val = action.map(|a| a.str_value.as_str()).unwrap_or("");
    let int_val = action.map(|a| a.int_value).unwrap_or(0);

    match at {
        ActionType::Transfer | ActionType::Copy => {
            // Dropdown of all mailboxes — like original Eudora
            let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            hbox.set_hexpand(true);

            // Collect mailbox names from the store
            let mut names: Vec<String> = Vec::new();
            // Standard mailboxes first
            for n in &["In", "Out", "Trash", "Junk"] {
                names.push(n.to_string());
            }
            // Walk the data dir for user mailboxes
            if let Some(home) = std::env::var("HOME").ok() {
                let mb_dir = std::path::PathBuf::from(&home)
                    .join(".local/share/geudora/mailboxes");
                if let Ok(store) = macmbx::store::Store::open(&mb_dir) {
                    fn collect(nodes: &[macmbx::store::Node], names: &mut Vec<String>) {
                        for node in nodes {
                            if node.node_type == macmbx::store::NodeType::Mailbox {
                                let n = &node.name;
                                if !["In", "Out", "Trash", "Junk"].contains(&n.as_str()) {
                                    names.push(n.clone());
                                }
                            }
                            if !node.children.is_empty() {
                                collect(&node.children, names);
                            }
                        }
                    }
                    collect(store.children(), &mut names);
                }
            }

            let strs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            let dd = gtk4::DropDown::from_strings(&strs);
            dd.set_hexpand(true);

            // Select current value
            if !str_val.is_empty() {
                for (i, n) in names.iter().enumerate() {
                    if n == str_val {
                        dd.set_selected(i as u32);
                        break;
                    }
                }
            }

            hbox.append(&dd);
            hbox.upcast()
        }
        ActionType::Status => {
            // Status with icons matching message list (port of mailbox.c state_str)
            let statuses = &[
                "●  Unread",
                "    Read",
                "↩  Replied",
                "→  Forwarded",
                "⇉  Redirected",
                "✏  Unsendable",
                "✓  Sendable",
                "⏳  Queued",
                "✉  Sent",
                "✏  Unsent",
            ];
            let dd = gtk4::DropDown::from_strings(statuses);
            dd.set_selected(int_val.clamp(0, 9) as u32);
            dd.set_hexpand(true);
            dd.upcast()
        }
        ActionType::Priority => {
            // Colored priority items matching filtwin.c:1614-1645
            let display = [
                "🔴  ↑↑  Highest",
                "🟠  ↑  High",
                "⚪  —  Normal",
                "🔵  ↓  Low",
                "🟢  ↓↓  Lowest",
                "🟣  ↑  Raise",
                "🟣  ↓  Lower",
            ];
            let sel = match int_val {
                1 => 0u32, 2 => 1, 3 => 2, 4 => 3, 5 => 4, 7 => 5, 8 => 6, _ => 2,
            };
            let dd = gtk4::DropDown::from_strings(&display);
            dd.set_selected(sel);
            dd.set_hexpand(true);
            dd.upcast()
        }
        ActionType::Label => {
            // Label with color circles matching filtwin.c:1649-1670
            let label_icons = ["⚪", "🔴", "🔵", "🟢", "🟣", "🟠", "🔵", "⚫"];
            let label_names = ["None", "Label 1", "Label 2", "Label 3",
                               "Label 4", "Label 5", "Label 6", "Label 7"];
            let display: Vec<String> = label_names.iter().enumerate()
                .map(|(i, name)| format!("{}  {}", label_icons[i], name))
                .collect();
            let strs: Vec<&str> = display.iter().map(|s| s.as_str()).collect();
            let dd = gtk4::DropDown::from_strings(&strs);
            dd.set_selected(int_val.clamp(0, 7) as u32);
            dd.set_hexpand(true);
            dd.upcast()
        }
        ActionType::Subject => {
            let entry = gtk4::Entry::new();
            entry.set_placeholder_text(Some("Subject text"));
            entry.set_text(str_val);
            entry.set_hexpand(true);
            entry.upcast()
        }
        ActionType::Sound => {
            let sounds = &["Default", "Glass", "Ping", "Pop", "Purr",
                           "Sosumi", "Submarine", "Tink"];
            let dd = gtk4::DropDown::from_strings(sounds);
            // Find matching sound name
            let sel = sounds.iter().position(|s| s.eq_ignore_ascii_case(str_val))
                .unwrap_or(0) as u32;
            dd.set_selected(sel);
            dd.set_hexpand(true);
            dd.upcast()
        }
        ActionType::Forward | ActionType::Redirect => {
            let entry = gtk4::Entry::new();
            entry.set_placeholder_text(Some(if at == ActionType::Forward {
                "Forward to address..."
            } else {
                "Redirect to address..."
            }));
            entry.set_text(str_val);
            entry.set_hexpand(true);
            // Attach nickname autocomplete (port of filtwin.c:1729-1731)
            if let Some(ref b) = books {
                crate::autocomplete::attach(&entry, b.clone());
            }
            entry.upcast()
        }
        ActionType::Open => {
            let bx = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            let chk_mb = gtk4::CheckButton::with_label("Open Mailbox");
            let chk_msg = gtk4::CheckButton::with_label("Open Message");
            chk_mb.set_active(int_val & 1 != 0);
            chk_msg.set_active(int_val & 2 != 0);
            bx.append(&chk_mb);
            bx.append(&chk_msg);
            bx.upcast()
        }
        ActionType::Junk => {
            let dd = gtk4::DropDown::from_strings(&["Mark as Junk", "Mark as Not Junk"]);
            dd.set_selected(if int_val > 0 { 0 } else { 1 });
            dd.set_hexpand(true);
            dd.upcast()
        }
        // None, Stop, Print, Reply — no value needed
        _ => {
            let lbl = gtk4::Label::new(None);
            lbl.set_hexpand(true);
            lbl.upcast()
        }
    }
}

// ── Read action value back from widget ──

fn read_action_value(at: ActionType, widget: &gtk4::Widget) -> (i32, String) {
    match at {
        ActionType::Transfer | ActionType::Copy => {
            // Widget is a Box containing a DropDown of mailbox names
            if let Some(bx) = widget.downcast_ref::<gtk4::Box>() {
                if let Some(child) = bx.first_child() {
                    if let Some(dd) = child.downcast_ref::<gtk4::DropDown>() {
                        if let Some(item) = dd.selected_item() {
                            if let Some(so) = item.downcast_ref::<gtk4::StringObject>() {
                                return (0, so.string().to_string());
                            }
                        }
                    }
                }
            }
            (0, String::new())
        }
        ActionType::Subject | ActionType::Forward | ActionType::Redirect => {
            if let Some(entry) = widget.downcast_ref::<gtk4::Entry>() {
                (0, entry.text().to_string())
            } else {
                (0, String::new())
            }
        }
        ActionType::Status => {
            if let Some(dd) = widget.downcast_ref::<gtk4::DropDown>() {
                (dd.selected() as i32, String::new())
            } else {
                (0, String::new())
            }
        }
        ActionType::Priority => {
            if let Some(dd) = widget.downcast_ref::<gtk4::DropDown>() {
                let sel = dd.selected();
                let pri = match sel { 0 => 1, 1 => 2, 2 => 3, 3 => 4, 4 => 5, 5 => 7, 6 => 8, _ => 3 };
                (pri, String::new())
            } else {
                (3, String::new())
            }
        }
        ActionType::Label => {
            if let Some(dd) = widget.downcast_ref::<gtk4::DropDown>() {
                (dd.selected() as i32, String::new())
            } else {
                (0, String::new())
            }
        }
        ActionType::Sound => {
            let sounds = &["Default", "Glass", "Ping", "Pop", "Purr",
                           "Sosumi", "Submarine", "Tink"];
            if let Some(dd) = widget.downcast_ref::<gtk4::DropDown>() {
                let sel = dd.selected() as usize;
                (0, sounds.get(sel).unwrap_or(&"Default").to_string())
            } else {
                (0, "Default".into())
            }
        }
        ActionType::Open => {
            if let Some(bx) = widget.downcast_ref::<gtk4::Box>() {
                let mut flags = 0i32;
                if let Some(c) = bx.first_child() {
                    if let Some(cb) = c.downcast_ref::<gtk4::CheckButton>() {
                        if cb.is_active() { flags |= 1; }
                    }
                    if let Some(next) = c.next_sibling() {
                        if let Some(cb) = next.downcast_ref::<gtk4::CheckButton>() {
                            if cb.is_active() { flags |= 2; }
                        }
                    }
                }
                (flags, String::new())
            } else {
                (0, String::new())
            }
        }
        ActionType::Junk => {
            if let Some(dd) = widget.downcast_ref::<gtk4::DropDown>() {
                (if dd.selected() == 0 { 1 } else { 0 }, String::new())
            } else {
                (1, String::new())
            }
        }
        _ => (0, String::new()),
    }
}

// ── State ──

struct FilterState {
    filter_set: FilterSet,
    filter_path: PathBuf,
    selected: i32,
    populating: bool,
    books: Option<Rc<macmbx::nicknames::AddressBooks>>,
}

// ── Build the filter list rows (port of filtwin.c PopulateFilterList) ──

fn populate_list(list_box: &gtk4::ListBox, fs: &FilterSet) {
    // Clear
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    for rule in &fs.rules {
        let row = gtk4::Box::new(gtk4::Orientation::Vertical, 1);
        row.set_margin_start(6);
        row.set_margin_end(6);
        row.set_margin_top(3);
        row.set_margin_bottom(3);

        // Top line: colored dot + bold name + type badges
        let top = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);

        // Colored dot based on action type (filtwin.c:2296-2327)
        let color = dot_color_for_rule(rule);
        let dot = gtk4::Label::new(Some("●"));
        dot.add_css_class("filt-dot");
        dot.set_markup(&format!("<span color='{}'>●</span>", color));
        dot.set_valign(gtk4::Align::Center);
        top.append(&dot);

        // Bold filter name
        let name_lbl = gtk4::Label::new(None);
        name_lbl.set_markup(&format!("<b>{}</b>",
            glib::markup_escape_text(&rule.name)));
        name_lbl.set_halign(gtk4::Align::Start);
        name_lbl.set_hexpand(true);
        name_lbl.set_ellipsize(pango::EllipsizeMode::End);
        top.append(&name_lbl);

        // Type badges with proper CSS classes (filtwin.c:2342-2356)
        if rule.when & WHEN_INCOMING != 0 {
            let b = gtk4::Label::new(Some("IN"));
            b.add_css_class("filt-badge-in");
            top.append(&b);
        }
        if rule.when & WHEN_OUTGOING != 0 {
            let b = gtk4::Label::new(Some("OUT"));
            b.add_css_class("filt-badge-out");
            top.append(&b);
        }
        if rule.when & WHEN_MANUAL != 0 {
            let b = gtk4::Label::new(Some("MAN"));
            b.add_css_class("filt-badge-man");
            top.append(&b);
        }

        // Disabled indicator (filtwin.c:2359-2361)
        if rule.when == 0 {
            row.set_opacity(0.5);
        }

        row.append(&top);

        // Bottom line: match summary + action summary (filtwin.c:2366-2401)
        let mut summary = String::new();
        if let Some(c) = rule.conditions.first() {
            if !c.header.is_empty() {
                summary = format!("{} {} \"{}\"", c.header, c.verb.to_str_name(), c.value);
            }
        }
        if rule.conjunction != Conjunction::None {
            if let Some(c) = rule.conditions.get(1) {
                if !c.header.is_empty() {
                    let cj = match rule.conjunction {
                        Conjunction::And => "and",
                        Conjunction::Or => "or",
                        Conjunction::Unless => "unless",
                        _ => "",
                    };
                    summary.push_str(&format!(" {} {} {} \"{}\"", cj, c.header, c.verb.to_str_name(), c.value));
                }
            }
        }
        // First non-None action → arrow summary
        if let Some(act) = rule.actions.iter().find(|a| a.action_type != ActionType::None) {
            let act_name = ACTION_UI.get(action_type_to_idx(act.action_type) as usize).unwrap_or(&"?");
            summary.push_str(&format!("  → {}", act_name));
            if !act.str_value.is_empty() {
                summary.push_str(&format!(" {}", act.str_value));
            }
        }

        // Small gray summary text with indent past dot
        let bot = gtk4::Label::new(None);
        bot.set_markup(&format!(
            "<span size='small' color='#64748b'>{}</span>",
            glib::markup_escape_text(&summary)));
        bot.set_halign(gtk4::Align::Start);
        bot.set_ellipsize(pango::EllipsizeMode::End);
        bot.set_margin_start(22); // indent past dot
        row.append(&bot);

        list_box.append(&row);
    }
}

// ── Display selected rule in detail panel ──

fn display_rule(
    rule: Option<&Rule>,
    chk_in: &gtk4::CheckButton,
    chk_out: &gtk4::CheckButton,
    chk_man: &gtk4::CheckButton,
    header1: &gtk4::DropDown,
    verb1: &gtk4::DropDown,
    value1: &gtk4::Entry,
    conj: &gtk4::DropDown,
    header2: &gtk4::DropDown,
    verb2: &gtk4::DropDown,
    value2: &gtk4::Entry,
    action_rows: &[(gtk4::DropDown, gtk4::Box)],
    books: &Option<Rc<macmbx::nicknames::AddressBooks>>,
) {
    let empty_rule = Rule::default();
    let r = rule.unwrap_or(&empty_rule);

    chk_in.set_active(r.when & WHEN_INCOMING != 0);
    chk_out.set_active(r.when & WHEN_OUTGOING != 0);
    chk_man.set_active(r.when & WHEN_MANUAL != 0);

    // Condition 1
    let c1 = r.conditions.first();
    header1.set_selected(c1.map(|c| header_to_idx(&c.header)).unwrap_or(0));
    verb1.set_selected(c1.map(|c| verb_to_idx(c.verb)).unwrap_or(0));
    value1.set_text(c1.map(|c| c.value.as_str()).unwrap_or(""));

    // Conjunction
    conj.set_selected(conj_to_idx(r.conjunction));

    // Condition 2
    let c2 = r.conditions.get(1);
    header2.set_selected(c2.map(|c| header_to_idx(&c.header)).unwrap_or(0));
    verb2.set_selected(c2.map(|c| verb_to_idx(c.verb)).unwrap_or(0));
    value2.set_text(c2.map(|c| c.value.as_str()).unwrap_or(""));

    // Enable/disable second row
    let has_two = r.conjunction != Conjunction::None;
    header2.set_sensitive(has_two);
    verb2.set_sensitive(has_two);
    value2.set_sensitive(has_two);

    // Actions
    for (i, (type_dd, value_box)) in action_rows.iter().enumerate() {
        let action = r.actions.get(i);
        let at = action.map(|a| a.action_type).unwrap_or(ActionType::None);
        type_dd.set_selected(action_type_to_idx(at));

        // Clear and rebuild value widget
        while let Some(child) = value_box.first_child() {
            value_box.remove(&child);
        }
        let val_widget = build_action_value(at, action, books);
        value_box.append(&val_widget);
    }
}

// ── Save current UI state back into rule ──

fn save_rule_from_ui(
    rule: &mut Rule,
    chk_in: &gtk4::CheckButton,
    chk_out: &gtk4::CheckButton,
    chk_man: &gtk4::CheckButton,
    header1: &gtk4::DropDown,
    verb1: &gtk4::DropDown,
    value1: &gtk4::Entry,
    conj: &gtk4::DropDown,
    header2: &gtk4::DropDown,
    verb2: &gtk4::DropDown,
    value2: &gtk4::Entry,
    action_rows: &[(gtk4::DropDown, gtk4::Box)],
) {
    rule.when = 0;
    if chk_in.is_active() { rule.when |= WHEN_INCOMING; }
    if chk_out.is_active() { rule.when |= WHEN_OUTGOING; }
    if chk_man.is_active() { rule.when |= WHEN_MANUAL; }

    // Conditions
    rule.conditions.clear();

    let h1 = header1.selected() as usize;
    let h1_str = HEADER_OPTIONS.get(h1).unwrap_or(&"From:");
    let mut c1 = Condition::default();
    c1.header = h1_str.to_string();
    c1.verb = verb_from_idx(verb1.selected());
    c1.value = value1.text().to_string();
    rule.conditions.push(c1);

    rule.conjunction = conj_from_idx(conj.selected());

    let h2 = header2.selected() as usize;
    let h2_str = HEADER_OPTIONS.get(h2).unwrap_or(&"From:");
    let mut c2 = Condition::default();
    c2.header = h2_str.to_string();
    c2.verb = verb_from_idx(verb2.selected());
    c2.value = value2.text().to_string();
    rule.conditions.push(c2);

    // Actions
    rule.actions.clear();
    for (type_dd, value_box) in action_rows {
        let at = action_type_from_idx(type_dd.selected());
        let (int_val, str_val) = if let Some(val_w) = value_box.first_child() {
            read_action_value(at, &val_w)
        } else {
            (0, String::new())
        };
        rule.actions.push(Action {
            action_type: at,
            int_value: int_val,
            str_value: str_val,
        });
    }

    // Auto-name from first condition
    let c = &rule.conditions[0];
    if !c.header.is_empty() && !c.value.is_empty() {
        rule.name = format!("{}: {}", c.header, c.value);
    } else if rule.name.is_empty() {
        rule.name = "Untitled".into();
    }
}

// ── Public: build the filters tab widget ──

pub fn build_filters_tab(data_dir: &std::path::Path, books: Option<Rc<macmbx::nicknames::AddressBooks>>) -> gtk4::Widget {
    let filter_path = data_dir.join("Filters");
    let filter_set = if filter_path.exists() {
        FilterSet::load(&filter_path).unwrap_or_else(|_| FilterSet::new())
    } else {
        FilterSet::new()
    };

    let state = Rc::new(RefCell::new(FilterState {
        filter_set,
        filter_path,
        selected: -1,
        populating: false,
        books,
    }));

    install_css();

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vbox.add_css_class("panel-bg");

    // ── Hero header (filtwin.c:2615-2638) ──
    let hero = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    hero.add_css_class("filt-hero");

    let hero_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    hero_vbox.set_hexpand(true);
    let hero_title = gtk4::Label::new(Some("Filters"));
    hero_title.add_css_class("filt-hero-title");
    hero_title.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_title);

    let hero_sub = gtk4::Label::new(Some("Automate mail sorting, labeling, and actions"));
    hero_sub.add_css_class("filt-hero-sub");
    hero_sub.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_sub);
    hero.append(&hero_vbox);

    let count_text = format!("{} filters", state.borrow().filter_set.rules.len());
    let count_pill = gtk4::Label::new(Some(&count_text));
    count_pill.add_css_class("filt-count-pill");
    count_pill.set_valign(gtk4::Align::Center);
    hero.append(&count_pill);

    vbox.append(&hero);

    // ── Toolbar ──
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);
    toolbar.set_margin_top(4);
    toolbar.set_margin_bottom(4);

    let new_btn = gtk4::Button::with_label("New");
    let remove_btn = gtk4::Button::with_label("Remove");
    let dup_btn = gtk4::Button::with_label("Duplicate");
    let up_btn = gtk4::Button::with_label("▲");
    up_btn.set_tooltip_text(Some("Move Up"));
    let down_btn = gtk4::Button::with_label("▼");
    down_btn.set_tooltip_text(Some("Move Down"));
    let save_btn = gtk4::Button::with_label("Save");
    save_btn.add_css_class("suggested-action");

    toolbar.append(&new_btn);
    toolbar.append(&remove_btn);
    toolbar.append(&dup_btn);
    toolbar.append(&up_btn);
    toolbar.append(&down_btn);
    toolbar.append(&save_btn);
    vbox.append(&toolbar);

    // ── HPaned: list | detail ──
    let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    paned.set_position(260);
    paned.set_vexpand(true);

    // ── Left: filter list ──
    let list_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .build();
    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::Single);
    list_scroll.set_child(Some(&list_box));
    paned.set_start_child(Some(&list_scroll));
    paned.set_resize_start_child(false);
    paned.set_shrink_start_child(false);

    // ── Right: detail panel ──
    let detail_scroll = gtk4::ScrolledWindow::new();
    let detail_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    detail_box.add_css_class("panel-bg");
    detail_box.set_margin_start(12);
    detail_box.set_margin_end(12);
    detail_box.set_margin_top(8);
    detail_box.set_margin_bottom(8);

    // Incoming/Outgoing/Manual checkboxes
    let type_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    let chk_incoming = gtk4::CheckButton::with_label("Incoming");
    let chk_outgoing = gtk4::CheckButton::with_label("Outgoing");
    let chk_manual = gtk4::CheckButton::with_label("Manual");
    type_row.append(&chk_incoming);
    type_row.append(&chk_outgoing);
    type_row.append(&chk_manual);
    detail_box.append(&type_row);

    // ── Match section ──
    let match_frame = gtk4::Frame::new(Some("Match"));
    let match_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    match_box.set_margin_start(8);
    match_box.set_margin_end(8);
    match_box.set_margin_top(4);
    match_box.set_margin_bottom(8);

    // Match row 1
    let row1 = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    let header_dd1 = gtk4::DropDown::from_strings(HEADER_OPTIONS);
    header_dd1.set_size_request(130, -1);
    let verb_dd1 = gtk4::DropDown::from_strings(VERB_UI);
    let value_entry1 = gtk4::Entry::new();
    value_entry1.set_placeholder_text(Some("Value"));
    value_entry1.set_hexpand(true);
    row1.append(&header_dd1);
    row1.append(&verb_dd1);
    row1.append(&value_entry1);
    match_box.append(&row1);

    // Conjunction
    let conj_dd = gtk4::DropDown::from_strings(CONJ_UI);
    match_box.append(&conj_dd);

    // Match row 2
    let row2 = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    let header_dd2 = gtk4::DropDown::from_strings(HEADER_OPTIONS);
    header_dd2.set_size_request(130, -1);
    let verb_dd2 = gtk4::DropDown::from_strings(VERB_UI);
    let value_entry2 = gtk4::Entry::new();
    value_entry2.set_placeholder_text(Some("Value"));
    value_entry2.set_hexpand(true);
    row2.append(&header_dd2);
    row2.append(&verb_dd2);
    row2.append(&value_entry2);
    match_box.append(&row2);

    match_frame.set_child(Some(&match_box));
    detail_box.append(&match_frame);

    // ── Action section ──
    let action_frame = gtk4::Frame::new(Some("Actions"));
    let action_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    action_box.set_margin_start(8);
    action_box.set_margin_end(8);
    action_box.set_margin_top(4);
    action_box.set_margin_bottom(8);

    let mut action_rows_vec: Vec<(gtk4::DropDown, gtk4::Box)> = Vec::new();
    for _ in 0..MAX_ACTIONS {
        let act_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        let type_dd = gtk4::DropDown::from_strings(ACTION_UI);
        let value_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        value_box.set_hexpand(true);

        // When action type changes, rebuild the value widget
        {
            let vb = value_box.clone();
            let st = state.clone();
            type_dd.connect_selected_notify(move |dd| {
                let at = action_type_from_idx(dd.selected());
                while let Some(child) = vb.first_child() {
                    vb.remove(&child);
                }
                let books = st.borrow().books.clone();
                let val_widget = build_action_value(at, None, &books);
                vb.append(&val_widget);
            });
        }

        act_row.append(&type_dd);
        act_row.append(&value_box);
        action_box.append(&act_row);
        action_rows_vec.push((type_dd, value_box));
    }

    action_frame.set_child(Some(&action_box));
    detail_box.append(&action_frame);

    detail_scroll.set_child(Some(&detail_box));
    paned.set_end_child(Some(&detail_scroll));
    paned.set_resize_end_child(true);
    paned.set_shrink_end_child(false);

    vbox.append(&paned);

    // Wrap action_rows in Rc for sharing across closures
    let action_rows: Rc<Vec<(gtk4::DropDown, gtk4::Box)>> = Rc::new(action_rows_vec);

    // Populate initial list
    populate_list(&list_box, &state.borrow().filter_set);

    // ── Shared save-current helper ──
    let save_current = {
        let st = state.clone();
        let ci = chk_incoming.clone();
        let co = chk_outgoing.clone();
        let cm = chk_manual.clone();
        let h1 = header_dd1.clone();
        let v1 = verb_dd1.clone();
        let e1 = value_entry1.clone();
        let cj = conj_dd.clone();
        let h2 = header_dd2.clone();
        let v2 = verb_dd2.clone();
        let e2 = value_entry2.clone();
        let ar = action_rows.clone();
        Rc::new(move || {
            let mut s = st.borrow_mut();
            if s.populating { return; }
            let idx = s.selected;
            if idx < 0 || idx as usize >= s.filter_set.rules.len() { return; }
            let rule = &mut s.filter_set.rules[idx as usize];
            save_rule_from_ui(rule, &ci, &co, &cm, &h1, &v1, &e1, &cj, &h2, &v2, &e2, &ar);
        })
    };

    // ── List selection → display in detail ──
    {
        let st = state.clone();
        let ci = chk_incoming.clone();
        let co = chk_outgoing.clone();
        let cm = chk_manual.clone();
        let h1 = header_dd1.clone();
        let v1 = verb_dd1.clone();
        let e1 = value_entry1.clone();
        let cj = conj_dd.clone();
        let h2 = header_dd2.clone();
        let v2 = verb_dd2.clone();
        let e2 = value_entry2.clone();
        let ar = action_rows.clone();
        let saver = save_current.clone();
        list_box.connect_row_selected(move |_, row| {
            if st.borrow().populating { return; }
            saver();
            let idx = row.map(|r| r.index()).unwrap_or(-1);
            st.borrow_mut().selected = idx;
            let s = st.borrow();
            let rule = if idx >= 0 && (idx as usize) < s.filter_set.rules.len() {
                Some(&s.filter_set.rules[idx as usize])
            } else {
                None
            };
            display_rule(rule, &ci, &co, &cm, &h1, &v1, &e1, &cj, &h2, &v2, &e2, &ar, &st.borrow().books);
        });
    }

    // ── Conjunction change → enable/disable second row ──
    {
        let h2 = header_dd2.clone();
        let v2 = verb_dd2.clone();
        let e2 = value_entry2.clone();
        conj_dd.connect_selected_notify(move |dd| {
            let has_two = dd.selected() != 0; // 0 = ignore
            h2.set_sensitive(has_two);
            v2.set_sensitive(has_two);
            e2.set_sensitive(has_two);
        });
    }

    // ── New button ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let saver = save_current.clone();
        let ci = chk_incoming.clone();
        let co = chk_outgoing.clone();
        let cm = chk_manual.clone();
        let h1 = header_dd1.clone();
        let v1 = verb_dd1.clone();
        let e1 = value_entry1.clone();
        let cj = conj_dd.clone();
        let h2 = header_dd2.clone();
        let v2 = verb_dd2.clone();
        let e2 = value_entry2.clone();
        let ar = action_rows.clone();
        new_btn.connect_clicked(move |_| {
            saver();
            let mut s = st.borrow_mut();
            let new_id = s.filter_set.next_id;
            s.filter_set.next_id += 1;
            let mut rule = Rule::default();
            rule.id = new_id;
            rule.name = "Untitled".into();
            rule.when = WHEN_INCOMING | WHEN_MANUAL;
            // Add empty conditions and actions
            rule.conditions.push(Condition::default());
            rule.conditions.push(Condition::default());
            for _ in 0..MAX_ACTIONS {
                rule.actions.push(Action::default());
            }
            let idx = s.filter_set.rules.len();
            s.filter_set.rules.push(rule);
            s.populating = true;
            populate_list(&lb, &s.filter_set);
            s.populating = false;
            s.selected = idx as i32;
            if let Some(row) = lb.row_at_index(idx as i32) {
                lb.select_row(Some(&row));
            }
            let rule = &s.filter_set.rules[idx];
            display_rule(Some(rule), &ci, &co, &cm, &h1, &v1, &e1, &cj, &h2, &v2, &e2, &ar, &st.borrow().books);
        });
    }

    // ── Remove button ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let ci = chk_incoming.clone();
        let co = chk_outgoing.clone();
        let cm = chk_manual.clone();
        let h1 = header_dd1.clone();
        let v1 = verb_dd1.clone();
        let e1 = value_entry1.clone();
        let cj = conj_dd.clone();
        let h2 = header_dd2.clone();
        let v2 = verb_dd2.clone();
        let e2 = value_entry2.clone();
        let ar = action_rows.clone();
        remove_btn.connect_clicked(move |_| {
            let mut s = st.borrow_mut();
            let idx = s.selected;
            if idx < 0 || idx as usize >= s.filter_set.rules.len() { return; }
            s.filter_set.rules.remove(idx as usize);
            if s.selected as usize >= s.filter_set.rules.len() && s.selected > 0 {
                s.selected -= 1;
            }
            let sel = s.selected;
            s.populating = true;
            populate_list(&lb, &s.filter_set);
            s.populating = false;
            if sel >= 0 && (sel as usize) < s.filter_set.rules.len() {
                if let Some(row) = lb.row_at_index(sel) {
                    lb.select_row(Some(&row));
                }
                let rule = &s.filter_set.rules[sel as usize];
                display_rule(Some(rule), &ci, &co, &cm, &h1, &v1, &e1, &cj, &h2, &v2, &e2, &ar, &st.borrow().books);
            } else {
                s.selected = -1;
                display_rule(None, &ci, &co, &cm, &h1, &v1, &e1, &cj, &h2, &v2, &e2, &ar, &st.borrow().books);
            }
        });
    }

    // ── Duplicate button ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let saver = save_current.clone();
        let ci = chk_incoming.clone();
        let co = chk_outgoing.clone();
        let cm = chk_manual.clone();
        let h1 = header_dd1.clone();
        let v1 = verb_dd1.clone();
        let e1 = value_entry1.clone();
        let cj = conj_dd.clone();
        let h2 = header_dd2.clone();
        let v2 = verb_dd2.clone();
        let e2 = value_entry2.clone();
        let ar = action_rows.clone();
        dup_btn.connect_clicked(move |_| {
            saver();
            let mut s = st.borrow_mut();
            let idx = s.selected;
            if idx < 0 || idx as usize >= s.filter_set.rules.len() { return; }
            let mut dup = s.filter_set.rules[idx as usize].clone();
            dup.id = s.filter_set.next_id;
            s.filter_set.next_id += 1;
            let new_idx = s.filter_set.rules.len();
            s.filter_set.rules.push(dup);
            s.populating = true;
            populate_list(&lb, &s.filter_set);
            s.populating = false;
            s.selected = new_idx as i32;
            if let Some(row) = lb.row_at_index(new_idx as i32) {
                lb.select_row(Some(&row));
            }
            let rule = &s.filter_set.rules[new_idx];
            display_rule(Some(rule), &ci, &co, &cm, &h1, &v1, &e1, &cj, &h2, &v2, &e2, &ar, &st.borrow().books);
        });
    }

    // ── Move Up ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let saver = save_current.clone();
        up_btn.connect_clicked(move |_| {
            saver();
            let mut s = st.borrow_mut();
            let idx = s.selected;
            if idx <= 0 || idx as usize >= s.filter_set.rules.len() { return; }
            s.filter_set.rules.swap(idx as usize, (idx - 1) as usize);
            s.selected = idx - 1;
            let sel = s.selected;
            s.populating = true;
            populate_list(&lb, &s.filter_set);
            s.populating = false;
            if let Some(row) = lb.row_at_index(sel) {
                lb.select_row(Some(&row));
            }
        });
    }

    // ── Move Down ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let saver = save_current.clone();
        down_btn.connect_clicked(move |_| {
            saver();
            let mut s = st.borrow_mut();
            let idx = s.selected;
            if idx < 0 || (idx + 1) as usize >= s.filter_set.rules.len() { return; }
            s.filter_set.rules.swap(idx as usize, (idx + 1) as usize);
            s.selected = idx + 1;
            let sel = s.selected;
            s.populating = true;
            populate_list(&lb, &s.filter_set);
            s.populating = false;
            if let Some(row) = lb.row_at_index(sel) {
                lb.select_row(Some(&row));
            }
        });
    }

    // ── Save button ──
    {
        let st = state.clone();
        let lb = list_box.clone();
        let saver = save_current.clone();
        save_btn.connect_clicked(move |_| {
            saver();
            let mut s = st.borrow_mut();
            if let Err(e) = s.filter_set.save() {
                eprintln!("rMail: failed to save filters: {}", e);
            }
            let sel = s.selected;
            s.populating = true;
            populate_list(&lb, &s.filter_set);
            s.populating = false;
            if sel >= 0 && (sel as usize) < s.filter_set.rules.len() {
                if let Some(row) = lb.row_at_index(sel) {
                    lb.select_row(Some(&row));
                }
            }
        });
    }

    vbox.upcast()
}
