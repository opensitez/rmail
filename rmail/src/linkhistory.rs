//! Link History panel — tracks clicked URLs.
//!
//! Port of linkwin.c / linkmng.c.
//! Stores URLs with timestamp, shows in a list, click to open in browser.

use gtk4::prelude::*;
use gtk4::glib;
use std::cell::RefCell;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

struct LinkEntry {
    url: String,
    title: String,
    timestamp: i64,
}

struct LinkState {
    entries: Vec<LinkEntry>,
    file_path: PathBuf,
}

impl LinkState {
    fn load(data_dir: &Path) -> Self {
        let file_path = data_dir.join("Link History");
        let mut entries = Vec::new();
        if let Ok(file) = std::fs::File::open(&file_path) {
            for line in io::BufReader::new(file).lines().flatten() {
                let parts: Vec<&str> = line.splitn(3, '\t').collect();
                if parts.len() >= 2 {
                    entries.push(LinkEntry {
                        url: parts[0].to_string(),
                        title: parts.get(2).unwrap_or(&parts[0]).to_string(),
                        timestamp: parts[1].parse().unwrap_or(0),
                    });
                }
            }
        }
        LinkState { entries, file_path }
    }

    fn save(&self) -> io::Result<()> {
        let mut f = std::fs::File::create(&self.file_path)?;
        for e in &self.entries {
            writeln!(f, "{}\t{}\t{}", e.url, e.timestamp, e.title)?;
        }
        Ok(())
    }

    fn add(&mut self, url: &str, title: &str) {
        let now = unsafe { libc::time(std::ptr::null_mut()) } as i64;
        // Don't duplicate — update timestamp if already exists
        if let Some(existing) = self.entries.iter_mut().find(|e| e.url == url) {
            existing.timestamp = now;
            if !title.is_empty() { existing.title = title.to_string(); }
        } else {
            self.entries.push(LinkEntry {
                url: url.to_string(),
                title: if title.is_empty() { url.to_string() } else { title.to_string() },
                timestamp: now,
            });
        }
        let _ = self.save();
    }
}

// Global link history — accessible from anywhere to record clicked links
static LINK_STATE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Initialize the link history with the data directory.
pub fn init(data_dir: &Path) {
    *LINK_STATE.lock().unwrap() = Some(data_dir.to_path_buf());
}

/// Record a clicked URL.
pub fn add_link(url: &str, title: &str) {
    if let Some(ref dir) = *LINK_STATE.lock().unwrap() {
        let mut state = LinkState::load(dir);
        state.add(url, title);
    }
}

/// Build the Link History tab.
pub fn build_link_history_tab(data_dir: &Path) -> gtk4::Widget {
    let state = Rc::new(RefCell::new(LinkState::load(data_dir)));

    let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    outer.add_css_class("panel-bg");

    // Hero header
    let hero = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    hero.add_css_class("filt-hero");
    let hero_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    hero_vbox.set_hexpand(true);
    let hero_title = gtk4::Label::new(Some("Link History"));
    hero_title.add_css_class("filt-hero-title");
    hero_title.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_title);
    let hero_sub = gtk4::Label::new(Some("URLs you've clicked in messages"));
    hero_sub.add_css_class("filt-hero-sub");
    hero_sub.set_halign(gtk4::Align::Start);
    hero_vbox.append(&hero_sub);
    hero.append(&hero_vbox);
    let count = state.borrow().entries.len();
    let count_pill = gtk4::Label::new(Some(&format!("{} links", count)));
    count_pill.add_css_class("filt-count-pill");
    count_pill.set_valign(gtk4::Align::Center);
    hero.append(&count_pill);
    outer.append(&hero);

    // Toolbar
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);
    toolbar.set_margin_top(4);
    toolbar.set_margin_bottom(4);
    let clear_btn = gtk4::Button::with_label("Clear All");
    toolbar.append(&clear_btn);
    outer.append(&toolbar);

    // Link list
    let list_store = gtk4::ListStore::new(&[
        glib::Type::STRING,  // 0: URL
        glib::Type::STRING,  // 1: title
        glib::Type::STRING,  // 2: date
    ]);

    for entry in &state.borrow().entries {
        let date = format_time(entry.timestamp);
        let iter = list_store.append();
        list_store.set(&iter, &[
            (0, &entry.url),
            (1, &entry.title),
            (2, &date),
        ]);
    }

    let tree = gtk4::TreeView::with_model(&list_store);
    tree.set_headers_visible(true);

    for &(title, col, width) in &[("URL", 0i32, 300i32), ("Title", 1, 200), ("Date", 2, 120)] {
        let r = gtk4::CellRendererText::new();
        let c = gtk4::TreeViewColumn::new();
        c.set_title(title);
        c.pack_start(&r, true);
        c.add_attribute(&r, "text", col);
        c.set_resizable(true);
        if width > 0 { c.set_min_width(width); }
        if col == 0 { c.set_expand(true); }
        c.set_sort_column_id(col);
        tree.append_column(&c);
    }

    // Double-click opens URL in browser
    tree.connect_row_activated(|tv, path, _col| {
        let model = tv.model().unwrap();
        if let Some(iter) = model.iter(path) {
            let url: String = model.get(&iter, 0);
            if !url.is_empty() {
                let _ = gtk4::gio::AppInfo::launch_default_for_uri(&url, gtk4::gio::AppLaunchContext::NONE);
            }
        }
    });

    let scroll = gtk4::ScrolledWindow::builder()
        .child(&tree).vexpand(true).build();
    outer.append(&scroll);

    // Clear button
    {
        let st = state.clone();
        let ls = list_store.clone();
        let cp = count_pill.clone();
        clear_btn.connect_clicked(move |_| {
            let mut s = st.borrow_mut();
            s.entries.clear();
            let _ = s.save();
            ls.clear();
            cp.set_text("0 links");
        });
    }

    outer.upcast()
}

fn format_time(secs: i64) -> String {
    if secs == 0 { return String::new(); }
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm); }
    let months = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
    let mon = months.get(tm.tm_mon as usize).unwrap_or(&"???");
    format!("{} {:02} {:02}:{:02}", mon, tm.tm_mday, tm.tm_hour, tm.tm_min)
}
