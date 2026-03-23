//! Search tab — multi-criteria mailbox search.
//!
//! Port of searchwin.c CreateSearchPanel / build_search_panel.
//! Layout: criteria rows + control bar + results TreeView + status label.
//! Uses macmbx::ops::search for text search and local comparison for numeric.

use gtk4::prelude::*;
use gtk4::glib;
use macmbx::ops::SearchField;
use macmbx::toc::{MsgFlags, Toc};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

// ── Category constants (matches searchwin.h SC_*) ──

const CATEGORIES: &[(&str, i32)] = &[
    ("Anywhere",       1),
    ("Headers",        2),
    ("Body",           3),
    ("From",           16),
    ("To",             17),
    ("Subject",        18),
    ("Cc",             19),
    ("Bcc",            20),
    ("Any Recipient",  21),
    ("Status",         7),
    ("Priority",       9),
    ("Junk Score",     8),
    ("Size (KB)",      13),
    ("Date",           12),
    ("Age",            14),
    ("Summary",        6),
];

const TEXT_RELATIONS: &[&str] = &[
    "contains", "contains word", "doesn't contain",
    "is", "is not", "starts with", "ends with",
];

const NUM_RELATIONS: &[&str] = &[
    "is", "is not", "greater than", "less than",
];

fn is_numeric(cat: i32) -> bool {
    matches!(cat, 7 | 8 | 9 | 12 | 13 | 14)  // status, junk, priority, date, size, age
}

fn category_to_search_field(cat: i32) -> Option<SearchField> {
    match cat {
        1 => Some(SearchField::All),       // Anywhere
        2 => Some(SearchField::All),       // Headers — approximate
        3 => Some(SearchField::Body),
        6 => Some(SearchField::All),       // Summary
        16 => Some(SearchField::From),
        17 => Some(SearchField::Header("To".into())),
        18 => Some(SearchField::Subject),
        19 => Some(SearchField::Header("Cc".into())),
        20 => Some(SearchField::Header("Bcc".into())),
        21 => Some(SearchField::Header("To".into())),  // Any Recipient — search To first, then Cc
        _ => None, // numeric
    }
}

// ── Criterion ──

struct Criterion {
    category: i32,
    relation: i32,     // 1-based: contains=1, etc
    text: String,
    specifier: i64,    // parsed numeric for numeric criteria
}

// ── Numeric matching (port of match_numeric) ──

fn match_numeric(msg: &macmbx::toc::MsgSum, crit: &Criterion) -> bool {
    let cmp_result = match crit.category {
        7 => { // Status
            let val = msg.state as i64;
            return if crit.specifier == val { crit.relation == 1 } // is
                   else { crit.relation == 2 }; // is not
        }
        9 => { // Priority
            let val = if msg.priority == 0 { 3 } else { msg.priority as i64 };
            crit.specifier.cmp(&val)
        }
        13 => { // Size (KB)
            let size_k = (msg.length as i64 + 1023) / 1024;
            size_k.cmp(&crit.specifier)
        }
        8 => { // Junk Score
            (msg.spam_score as i64).cmp(&crit.specifier)
        }
        14 => { // Age (days)
            let now = unsafe { libc::time(std::ptr::null_mut()) } as i64;
            let diff_days = (now - msg.seconds as i64) / 86400;
            diff_days.cmp(&crit.specifier)
        }
        12 => { // Date (year)
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            let t = msg.seconds as libc::time_t;
            unsafe { libc::gmtime_r(&t, &mut tm); }
            let year = (tm.tm_year + 1900) as i64;
            year.cmp(&crit.specifier)
        }
        _ => return false,
    };

    match crit.relation {
        1 => cmp_result == std::cmp::Ordering::Equal,        // is / equal
        2 => cmp_result != std::cmp::Ordering::Equal,        // is not / not equal
        3 => cmp_result == std::cmp::Ordering::Greater,      // greater than
        4 => cmp_result == std::cmp::Ordering::Less,         // less than
        _ => false,
    }
}

// ── Search one TOC (port of search_one_toc) ──

struct SearchResult {
    from: String,
    subject: String,
    date: String,
    mailbox: String,
    mbox_path: String,
    msg_index: usize,
}

fn search_one_toc(
    toc: &Toc,
    mbname: &str,
    mbox_path: &str,
    criteria: &[Criterion],
    match_any: bool,
) -> Vec<SearchResult> {
    if toc.msgs.is_empty() {
        return Vec::new();
    }

    let n = toc.msgs.len();
    let mut matches = vec![!match_any; n]; // AND=true, OR=false

    let mut any_criteria = false;

    for crit in criteria {
        if crit.text.is_empty() && !is_numeric(crit.category) {
            continue;
        }
        any_criteria = true;

        if is_numeric(crit.category) {
            for (m, msg) in toc.msgs.iter().enumerate() {
                let hit = match_numeric(msg, crit);
                if match_any {
                    matches[m] = matches[m] || hit;
                } else {
                    matches[m] = matches[m] && hit;
                }
            }
        } else {
            if let Some(field) = category_to_search_field(crit.category) {
                let hit_indices = macmbx::ops::search(toc, &field, &crit.text);
                if match_any {
                    for &idx in &hit_indices {
                        if idx < n { matches[idx] = true; }
                    }
                } else {
                    let mut hit_set = vec![false; n];
                    for &idx in &hit_indices {
                        if idx < n { hit_set[idx] = true; }
                    }
                    for m in 0..n {
                        matches[m] = matches[m] && hit_set[m];
                    }
                }

                // Any Recipient: also search Cc
                if crit.category == 21 {
                    let cc_hits = macmbx::ops::search(toc, &SearchField::Header("Cc".into()), &crit.text);
                    for &idx in &cc_hits {
                        if idx < n { matches[idx] = true; }
                    }
                }
            }
        }
    }

    if !any_criteria {
        return Vec::new();
    }

    let mut results = Vec::new();
    for (m, msg) in toc.msgs.iter().enumerate() {
        if !matches[m] { continue; }
        if msg.flags.contains(MsgFlags::DELETED) { continue; }

        let date = format_epoch(msg.seconds as i64);
        results.push(SearchResult {
            from: msg.from.clone(),
            subject: msg.subject.clone(),
            date,
            mailbox: mbname.to_string(),
            mbox_path: mbox_path.to_string(),
            msg_index: m,
        });
    }
    results
}

fn format_epoch(secs: i64) -> String {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm); }
    let months = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
    let mon = months.get(tm.tm_mon as usize).unwrap_or(&"???");
    format!("{} {:02}, {} {:02}:{:02}",
        mon, tm.tm_mday, 1900 + tm.tm_year, tm.tm_hour, tm.tm_min)
}

// ── Collect all mailbox paths from the store ──

fn collect_mailbox_paths(nodes: &[macmbx::store::Node], out: &mut Vec<(String, PathBuf)>) {
    for node in nodes {
        if node.node_type == macmbx::store::NodeType::Mailbox {
            out.push((node.name.clone(), node.path.clone()));
        }
        if !node.children.is_empty() {
            collect_mailbox_paths(&node.children, out);
        }
    }
}

// ── Criteria row widget ──

struct CriteriaRow {
    container: gtk4::Box,
    cat_dd: gtk4::DropDown,
    rel_dd: gtk4::DropDown,
    entry: gtk4::Entry,
}

fn build_criteria_row() -> CriteriaRow {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    row.set_margin_start(4);
    row.set_margin_end(4);
    row.set_margin_top(2);
    row.set_margin_bottom(2);

    let cat_labels: Vec<&str> = CATEGORIES.iter().map(|(l, _)| *l).collect();
    let cat_dd = gtk4::DropDown::from_strings(&cat_labels);
    cat_dd.set_size_request(130, -1);

    let rel_dd = gtk4::DropDown::from_strings(TEXT_RELATIONS);
    rel_dd.set_size_request(130, -1);

    let entry = gtk4::Entry::new();
    entry.set_placeholder_text(Some("Search text..."));
    entry.set_hexpand(true);

    row.append(&cat_dd);
    row.append(&rel_dd);
    row.append(&entry);

    // When category changes, swap relation dropdown for text vs numeric
    {
        let rel = rel_dd.clone();
        let r = row.clone();
        cat_dd.connect_selected_notify(move |dd| {
            let sel = dd.selected() as usize;
            if sel < CATEGORIES.len() {
                let cat = CATEGORIES[sel].1;
                let items = if is_numeric(cat) { NUM_RELATIONS } else { TEXT_RELATIONS };
                // Replace the relation dropdown
                let new_rel = gtk4::DropDown::from_strings(items);
                new_rel.set_size_request(130, -1);
                r.insert_child_after(&new_rel, Some(&*dd));
                r.remove(&rel);
                // Note: this closure captures the old rel_dd, so further changes
                // won't update. For now this handles the initial switch.
            }
        });
    }

    CriteriaRow { container: row, cat_dd, rel_dd, entry }
}

fn read_criterion(row: &CriteriaRow) -> Criterion {
    let cat_sel = row.cat_dd.selected() as usize;
    let cat = if cat_sel < CATEGORIES.len() { CATEGORIES[cat_sel].1 } else { 1 };
    let numeric = is_numeric(cat);

    let rel_sel = row.rel_dd.selected();
    let relation = rel_sel as i32 + 1; // 1-based

    let text = row.entry.text().to_string();
    let specifier = if numeric { text.parse::<i64>().unwrap_or(0) } else { 0 };

    Criterion { category: cat, relation, text, specifier }
}

// ── Public: build search tab ──

pub fn build_search_tab(store: &macmbx::store::Store) -> gtk4::Widget {
    // Collect all mailbox paths up front
    let mailbox_paths: Vec<(String, PathBuf)> = {
        let mut paths = Vec::new();
        collect_mailbox_paths(store.children(), &mut paths);
        paths
    };
    let mailbox_paths = Rc::new(mailbox_paths);

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    vbox.add_css_class("panel-bg");
    vbox.set_margin_start(6);
    vbox.set_margin_end(6);
    vbox.set_margin_top(6);
    vbox.set_margin_bottom(6);

    // ── Criteria area ──
    let criteria_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);

    // Start with one criterion row
    let first_row = build_criteria_row();
    criteria_box.append(&first_row.container);

    let criteria_rows: Rc<RefCell<Vec<CriteriaRow>>> = Rc::new(RefCell::new(Vec::new()));
    // Move first row in — but CriteriaRow isn't Clone-friendly due to widget refs.
    // Store just the widgets we need.
    // Actually, let's use a simpler approach: store the criteria_box and read children.

    // We'll track rows via a vec of (DropDown, DropDown, Entry) tuples.
    let rows: Rc<RefCell<Vec<(gtk4::DropDown, gtk4::DropDown, gtk4::Entry, gtk4::Box)>>> =
        Rc::new(RefCell::new(vec![
            (first_row.cat_dd.clone(), first_row.rel_dd.clone(), first_row.entry.clone(), first_row.container.clone())
        ]));

    vbox.append(&criteria_box);

    // ── Control bar ──
    let ctrl = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    ctrl.set_margin_top(4);
    ctrl.set_margin_bottom(4);

    // Match All / Match Any
    let match_dd = gtk4::DropDown::from_strings(&["Match All", "Match Any"]);
    match_dd.set_size_request(120, -1);
    ctrl.append(&match_dd);

    // More Criteria
    let more_btn = gtk4::Button::with_label("More Criteria");
    ctrl.append(&more_btn);

    // Fewer Criteria
    let fewer_btn = gtk4::Button::with_label("Fewer Criteria");
    fewer_btn.set_sensitive(false);
    ctrl.append(&fewer_btn);

    // Spacer
    let spacer = gtk4::Label::new(None);
    spacer.set_hexpand(true);
    ctrl.append(&spacer);

    // Search button
    let search_btn = gtk4::Button::with_label("Search");
    search_btn.add_css_class("suggested-action");
    ctrl.append(&search_btn);

    vbox.append(&ctrl);
    vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // ── Results TreeView ──
    let result_store = gtk4::ListStore::new(&[
        glib::Type::STRING,  // 0: From
        glib::Type::STRING,  // 1: Subject
        glib::Type::STRING,  // 2: Date
        glib::Type::STRING,  // 3: Mailbox
        glib::Type::STRING,  // 4: mbox_path (hidden)
        glib::Type::I32,     // 5: msg_index (hidden)
    ]);

    let tree = gtk4::TreeView::with_model(&result_store);
    tree.set_headers_visible(true);

    let cols: &[(&str, i32, i32)] = &[
        ("From", 0, 120),
        ("Subject", 1, 180),
        ("Date", 2, 130),
        ("Mailbox", 3, 100),
    ];
    for &(title, col_idx, min_w) in cols {
        let renderer = gtk4::CellRendererText::new();
        let col = gtk4::TreeViewColumn::new();
        col.set_title(title);
        col.pack_start(&renderer, true);
        col.add_attribute(&renderer, "text", col_idx);
        col.set_resizable(true);
        col.set_min_width(min_w);
        if col_idx == 1 { col.set_expand(true); }
        col.set_sort_column_id(col_idx);
        tree.append_column(&col);
    }

    let result_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .child(&tree)
        .build();
    vbox.append(&result_scroll);

    // ── Status label ──
    let status_label = gtk4::Label::new(Some("Ready."));
    status_label.set_halign(gtk4::Align::Start);
    status_label.set_margin_top(2);
    vbox.append(&status_label);

    // ── More Criteria ──
    {
        let cb = criteria_box.clone();
        let r = rows.clone();
        let fb = fewer_btn.clone();
        let sb = search_btn.clone();
        more_btn.connect_clicked(move |_| {
            let cr = build_criteria_row();
            // Enter in new entry → trigger search
            let sb2 = sb.clone();
            cr.entry.connect_activate(move |_| { sb2.emit_clicked(); });
            cb.append(&cr.container);
            r.borrow_mut().push((cr.cat_dd, cr.rel_dd, cr.entry, cr.container));
            fb.set_sensitive(true);
        });
    }

    // ── Fewer Criteria ──
    {
        let cb = criteria_box.clone();
        let r = rows.clone();
        let fb = fewer_btn.clone();
        fewer_btn.connect_clicked(move |_| {
            let mut rows = r.borrow_mut();
            if rows.len() <= 1 { return; }
            if let Some((_cat, _rel, _entry, container)) = rows.pop() {
                cb.remove(&container);
            }
            fb.set_sensitive(rows.len() > 1);
        });
    }

    // ── Search button ──
    {
        let r = rows.clone();
        let mdd = match_dd.clone();
        let rs = result_store.clone();
        let sl = status_label.clone();
        let mp = mailbox_paths.clone();
        let sb = search_btn.clone();
        search_btn.connect_clicked(move |_| {
            // Read criteria from UI
            let rows = r.borrow();
            let criteria: Vec<Criterion> = rows.iter().map(|(cat, rel, entry, _)| {
                let cat_sel = cat.selected() as usize;
                let cat_val = if cat_sel < CATEGORIES.len() { CATEGORIES[cat_sel].1 } else { 1 };
                let numeric = is_numeric(cat_val);
                let rel_sel = rel.selected();
                let relation = rel_sel as i32 + 1;
                let text = entry.text().to_string();
                let specifier = if numeric { text.parse::<i64>().unwrap_or(0) } else { 0 };
                Criterion { category: cat_val, relation, text, specifier }
            }).collect();

            let match_any = mdd.selected() == 1;

            // Clear previous results
            rs.clear();
            let mut total_hits = 0usize;

            // Search each mailbox
            for (i, (mbname, mbox_path)) in mp.iter().enumerate() {
                sl.set_text(&format!("Searching {}... ({}/{})", mbname, i + 1, mp.len()));

                // Process pending GTK events to keep UI responsive
                while glib::MainContext::default().iteration(false) {}

                let path_str = mbox_path.to_string_lossy();
                match Toc::open(mbox_path) {
                    Ok(toc) => {
                        let results = search_one_toc(&toc, mbname, &path_str, &criteria, match_any);
                        for res in &results {
                            let iter = rs.append();
                            rs.set(&iter, &[
                                (0, &res.from),
                                (1, &res.subject),
                                (2, &res.date),
                                (3, &res.mailbox),
                                (4, &res.mbox_path),
                                (5, &(res.msg_index as i32)),
                            ]);
                        }
                        total_hits += results.len();
                    }
                    Err(_) => {}
                }
            }

            sl.set_text(&format!("Found {} messages in {} mailboxes.", total_hits, mp.len()));
        });
    }

    // ── Double-click result → open message (port of on_result_activated) ──
    {
        tree.connect_row_activated(move |tv, path, _col| {
            let model = tv.model().unwrap();
            let iter = match model.iter(path) {
                Some(i) => i,
                None => return,
            };
            let mbox_path: String = model.get(&iter, 4);
            let msg_index: i32 = model.get(&iter, 5);
            let subject: String = model.get(&iter, 1);

            if mbox_path.is_empty() || msg_index < 0 { return; }

            // Read message and open in a new window
            match Toc::open(&mbox_path) {
                Ok(toc) => {
                    match macmbx::ops::read_message(&toc, msg_index as usize) {
                        Ok(raw) => {
                            let text = String::from_utf8_lossy(&raw);
                            let win = gtk4::Window::new();
                            let title = if subject.len() > 60 {
                                format!("{}…", &subject[..57])
                            } else {
                                subject.clone()
                            };
                            win.set_title(Some(&title));
                            win.set_default_size(700, 550);

                            // Reuse create_message_view from main — but we can't call it from here.
                            // Build a simple styled view inline.
                            let msg_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                            msg_vbox.add_css_class("panel-bg");

                            // Headers
                            let hdr_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
                            hdr_box.add_css_class("msg-header-box");
                            for hdr_name in &["From", "To", "Subject", "Date", "Cc"] {
                                let needle = format!("{}:", hdr_name);
                                for line in text.split('\n') {
                                    let line = line.trim_end_matches('\r');
                                    if line.is_empty() { break; }
                                    if line.len() > needle.len() && line[..needle.len()].eq_ignore_ascii_case(&needle) {
                                        let val = line[needle.len()..].trim_start();
                                        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
                                        let nl = gtk4::Label::new(Some(hdr_name));
                                        nl.add_css_class("msg-header-name");
                                        nl.set_halign(gtk4::Align::End);
                                        let cl = gtk4::Label::new(Some(":"));
                                        cl.add_css_class("msg-header-name");
                                        let vl = gtk4::Label::new(Some(val));
                                        vl.add_css_class("msg-header-value");
                                        vl.set_halign(gtk4::Align::Start);
                                        vl.set_wrap(true);
                                        vl.set_selectable(true);
                                        vl.set_hexpand(true);
                                        row.append(&nl);
                                        row.append(&cl);
                                        row.append(&vl);
                                        hdr_box.append(&row);
                                        break;
                                    }
                                }
                            }
                            msg_vbox.append(&hdr_box);
                            msg_vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

                            // Body
                            let body = if let Some(pos) = text.find("\n\n") {
                                &text[pos + 2..]
                            } else if let Some(pos) = text.find("\r\n\r\n") {
                                &text[pos + 4..]
                            } else {
                                &*text
                            };
                            let buf = gtk4::TextBuffer::new(None);
                            buf.set_text(body);
                            let tv = gtk4::TextView::with_buffer(&buf);
                            tv.set_editable(false);
                            tv.set_cursor_visible(false);
                            tv.set_wrap_mode(gtk4::WrapMode::WordChar);
                            tv.set_left_margin(12);
                            tv.set_right_margin(12);
                            tv.set_top_margin(4);
                            tv.add_css_class("msg-body-view");
                            let scroll = gtk4::ScrolledWindow::builder()
                                .child(&tv).vexpand(true).build();
                            msg_vbox.append(&scroll);

                            win.set_child(Some(&msg_vbox));
                            win.present();
                        }
                        Err(e) => eprintln!("rMail: failed to read message: {}", e),
                    }
                }
                Err(e) => eprintln!("rMail: failed to open TOC: {}", e),
            }
        });
    }

    // ── Enter in search entry → trigger search ──
    {
        let sb = search_btn.clone();
        let first_entry = rows.borrow().first()
            .map(|(_, _, e, _)| e.clone());
        if let Some(entry) = first_entry {
            entry.connect_activate(move |_| { sb.emit_clicked(); });
        }
    }

    vbox.upcast()
}
