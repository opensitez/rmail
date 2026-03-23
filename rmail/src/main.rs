//! rMail — Eudora mail client, Rust/GTK4 port.
//!
//! Phase 1: App skeleton that loads real mailbox data.

use gtk4::prelude::*;
use gtk4::gio;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

mod autocomplete;
mod compose;
mod contacts;
mod findreplace;
mod linkhistory;
mod filters;
mod personalities;
mod scheduler;
mod search;
mod settings;
mod sidebar;
mod signatures;
mod stationery;
mod statistics;
mod tasks;

/// Application state — owns all data, shared via Rc<RefCell<>>.
struct AppState {
    data_dir: PathBuf,
    store: macmbx::store::Store,
    conf: Rc<RefCell<macmbx::conf::Conf>>,
    /// Shared conf for mailer (same data, thread-safe wrapper)
    conf_shared: std::sync::Arc<std::sync::Mutex<macmbx::conf::Conf>>,
    /// Shared mailer — created once, used by scheduler + compose.
    mailer: std::sync::Arc<macmbx::mailer::Mailer>,
    /// Address books for autocomplete.
    books: Option<Rc<macmbx::nicknames::AddressBooks>>,
}

impl AppState {
    fn open(data_dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let mailboxes_dir = data_dir.join("mailboxes");
        let config_path = data_dir.join("geudora.ini");

        let conf = if config_path.exists() {
            macmbx::conf::Conf::load(&config_path)?
        } else {
            macmbx::conf::Conf::new()
        };

        let store = macmbx::store::Store::open(&mailboxes_dir)?;

        eprintln!("rMail: loaded {} from {}",
            store.count_mailboxes(), mailboxes_dir.display());

        // Create shared conf for mailer (thread-safe copy, synced on settings save)
        let conf_shared = std::sync::Arc::new(std::sync::Mutex::new(conf.clone()));

        // Create shared mailer (port of main_eudora.c g_mailer setup)
        let mailer = std::sync::Arc::new(
            scheduler::create_mailer_from_conf(conf_shared.clone(), &mailboxes_dir));

        // Load address books for autocomplete
        let nick_dir = data_dir.join("Nicknames");
        let books = macmbx::nicknames::AddressBooks::open(&nick_dir)
            .ok()
            .map(Rc::new);

        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            store,
            conf: Rc::new(RefCell::new(conf)),
            conf_shared,
            mailer,
            books,
        })
    }
}

fn data_dir() -> PathBuf {
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/geudora"))
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn build_ui(app: &gtk4::Application) {
    let state = match AppState::open(&data_dir()) {
        Ok(s) => Rc::new(RefCell::new(s)),
        Err(e) => {
            eprintln!("rMail: failed to load data: {}", e);
            return;
        }
    };

    // ── Menu bar — exact copy from gtk_menus.c:155-285 ──
    let menu_bar = gio::Menu::new();

    // File
    let file_menu = gio::Menu::new();
    file_menu.append(Some("_New Message"), Some("app.new-message"));
    file_menu.append(Some("_Open..."), Some("app.open"));
    file_menu.append(Some("_Close"), Some("app.close"));
    file_menu.append(Some("_Save"), Some("app.save"));
    file_menu.append_section(None, &gio::Menu::new());
    file_menu.append(Some("_Print..."), Some("app.print"));
    file_menu.append(Some("Save _As..."), Some("app.save-as"));
    file_menu.append_section(None, &gio::Menu::new());
    file_menu.append(Some("_Import..."), Some("app.import"));
    file_menu.append(Some("_Export..."), Some("app.export"));
    file_menu.append_section(None, &gio::Menu::new());
    file_menu.append(Some("_Quit"), Some("app.quit"));
    menu_bar.append_submenu(Some("_File"), &file_menu);

    // Edit
    let edit_menu = gio::Menu::new();
    edit_menu.append(Some("_Undo"), Some("app.undo"));
    edit_menu.append_section(None, &gio::Menu::new());
    edit_menu.append(Some("Cu_t"), Some("app.cut"));
    edit_menu.append(Some("_Copy"), Some("app.copy"));
    edit_menu.append(Some("_Paste"), Some("app.paste"));
    edit_menu.append(Some("_Select All"), Some("app.select-all"));
    edit_menu.append_section(None, &gio::Menu::new());
    edit_menu.append(Some("_Find..."), Some("app.find"));
    edit_menu.append(Some("Find/Re_place..."), Some("app.find-replace"));
    edit_menu.append_section(None, &gio::Menu::new());
    edit_menu.append(Some("Zoom _In"), Some("app.zoom-in"));
    edit_menu.append(Some("Zoom _Out"), Some("app.zoom-out"));
    edit_menu.append(Some("_Spelling"), Some("app.spelling"));
    menu_bar.append_submenu(Some("_Edit"), &edit_menu);

    // Message
    let message_menu = gio::Menu::new();
    message_menu.append(Some("_New Message"), Some("app.new-message"));
    message_menu.append(Some("_Reply"), Some("app.reply"));
    message_menu.append(Some("Reply _All"), Some("app.reply-all"));
    message_menu.append(Some("_Forward"), Some("app.forward"));
    message_menu.append(Some("_Redirect"), Some("app.redirect"));
    message_menu.append(Some("_Queue for Delivery"), Some("app.queue"));
    message_menu.append_section(None, &gio::Menu::new());
    message_menu.append(Some("_Delete"), Some("app.delete"));
    message_menu.append_section(None, &gio::Menu::new());
    message_menu.append(Some("Mark as _Read"), Some("app.mark-read"));
    message_menu.append(Some("Mark as _Unread"), Some("app.mark-unread"));
    message_menu.append(Some("Mark as _Junk"), Some("app.mark-junk"));
    message_menu.append(Some("Mark as _Not Junk"), Some("app.mark-not-junk"));
    menu_bar.append_submenu(Some("_Message"), &message_menu);

    // Transfer
    let transfer_menu = gio::Menu::new();
    transfer_menu.append(Some("→ _In"), Some("app.transfer-in"));
    transfer_menu.append(Some("→ _Out"), Some("app.transfer-out"));
    transfer_menu.append(Some("→ _Trash"), Some("app.transfer-trash"));
    // Dynamic mailbox list
    {
        let sep = gio::Menu::new();
        transfer_menu.append_section(None, &sep);
        let dynamic = gio::Menu::new();
        if let Ok(st) = AppState::open(&data_dir()) {
            fn add_nodes(menu: &gio::Menu, nodes: &[macmbx::store::Node]) {
                for node in nodes {
                    if node.node_type == macmbx::store::NodeType::Mailbox {
                        let name = &node.name;
                        if !["In", "Out", "Trash", "Junk"].contains(&name.as_str()) {
                            let action = format!("app.transfer-to-{}", name.replace(' ', "-").to_lowercase());
                            menu.append(Some(&format!("→ {}", name)), Some(&action));
                        }
                    }
                    if !node.children.is_empty() {
                        add_nodes(menu, &node.children);
                    }
                }
            }
            add_nodes(&dynamic, st.store.children());
        }
        transfer_menu.append_section(None, &dynamic);
    }
    menu_bar.append_submenu(Some("_Transfer"), &transfer_menu);

    // Tools
    let tools_menu = gio::Menu::new();
    tools_menu.append(Some("_Address Book"), Some("app.address-book"));
    tools_menu.append(Some("_Filters"), Some("app.filters"));
    tools_menu.append(Some("_Personalities"), Some("app.personalities"));
    tools_menu.append(Some("Si_gnatures"), Some("app.signatures"));
    tools_menu.append(Some("S_tationery"), Some("app.stationery"));
    tools_menu.append(Some("_Statistics"), Some("app.statistics"));
    tools_menu.append(Some("_Link History"), Some("app.link-history"));
    tools_menu.append_section(None, &gio::Menu::new());
    tools_menu.append(Some("_Check Mail"), Some("app.check-mail"));
    tools_menu.append(Some("_Send Queued"), Some("app.send-queued"));
    menu_bar.append_submenu(Some("_Tools"), &tools_menu);

    // Settings
    let settings_menu = gio::Menu::new();
    settings_menu.append(Some("_Preferences..."), Some("app.preferences"));
    menu_bar.append_submenu(Some("_Settings"), &settings_menu);

    // Window
    let window_menu = gio::Menu::new();
    window_menu.append(Some("_Minimize"), Some("app.minimize"));
    window_menu.append(Some("_Tabs"), Some("app.tabs"));
    menu_bar.append_submenu(Some("_Window"), &window_menu);

    app.set_menubar(Some(&menu_bar));

    // ── Keyboard shortcuts ──
    app.set_accels_for_action("app.new-message", &["<Meta>n"]);
    app.set_accels_for_action("app.reply", &["<Meta>r"]);
    app.set_accels_for_action("app.reply-all", &["<Meta><Shift>r"]);
    app.set_accels_for_action("app.forward", &["<Meta><Shift>f"]);
    app.set_accels_for_action("app.delete", &["<Meta>d", "Delete"]);
    app.set_accels_for_action("app.check-mail", &["<Meta>m"]);
    app.set_accels_for_action("app.send-queued", &["<Meta>t"]);
    app.set_accels_for_action("app.find", &["<Meta>f"]);
    app.set_accels_for_action("app.preferences", &["<Meta>comma"]);
    app.set_accels_for_action("app.quit", &["<Meta>q"]);
    app.set_accels_for_action("app.print", &["<Meta>p"]);
    app.set_accels_for_action("app.address-book", &["<Meta>l"]);
    app.set_accels_for_action("app.save", &["<Meta>s"]);
    app.set_accels_for_action("app.zoom-in", &["<Meta>equal", "<Meta>plus"]);
    app.set_accels_for_action("app.zoom-out", &["<Meta>minus"]);
    app.set_accels_for_action("app.find-replace", &["<Meta>h"]);
    app.set_accels_for_action("app.save-as", &["<Meta><Shift>s"]);

    // ── Register all actions ──
    // Quit
    let quit_action = gio::SimpleAction::new("quit", None);
    let app_clone = app.clone();
    quit_action.connect_activate(move |_, _| { app_clone.quit(); });
    app.add_action(&quit_action);

    // Actions that don't need UI references yet (just register so menus aren't broken)
    // Remaining stub actions (menus that don't need notebook)
    for name in &[
        "open", "close", "save",
        "undo", "cut", "copy", "paste", "select-all", "spelling",
        "queue", "find-replace",
        "send-queued", "minimize", "tabs",
    ] {
        let action = gio::SimpleAction::new(name, None);
        app.add_action(&action);
    }

    // ── Save As — save selected message as .eml file ──
    {
        let save_as = gio::SimpleAction::new("save-as", None);
        let st = state.clone();
        save_as.connect_activate(move |_, _| {
            // We need the content_notebook later — for now use macmbx ops
            eprintln!("Save As — will be wired after notebook creation");
        });
        app.add_action(&save_as);
    }

    // ── Import — import mbox/eml/maildir ──
    {
        let import_action = gio::SimpleAction::new("import", None);
        let st = state.clone();
        import_action.connect_activate(move |_, _| {
            let dlg = gtk4::FileDialog::new();
            dlg.set_title("Import Mailbox");
            let st2 = st.clone();
            dlg.open(None::<&gtk4::Window>, gtk4::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        let s = st2.borrow();
                        let in_path = s.data_dir.join("mailboxes").join("In");
                        if let Ok(mut toc) = macmbx::toc::Toc::open_or_create(&in_path) {
                            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                            let count = match ext {
                                "eml" => {
                                    match macmbx::thread::import_eml(&mut toc, &path) {
                                        Ok(_) => { let _ = toc.save(); 1 }
                                        Err(e) => { eprintln!("Import failed: {}", e); 0 }
                                    }
                                }
                                _ => {
                                    // Assume mbox format
                                    match macmbx::thread::import_mbox(&mut toc, &path, true) {
                                        Ok(n) => { let _ = toc.save(); n }
                                        Err(e) => { eprintln!("Import failed: {}", e); 0 }
                                    }
                                }
                            };
                            eprintln!("Imported {} message(s)", count);
                        }
                    }
                }
            });
        });
        app.add_action(&import_action);
    }

    // ── Export — export selected mailbox as mbox/eml ──
    {
        let export_action = gio::SimpleAction::new("export", None);
        let st = state.clone();
        export_action.connect_activate(move |_, _| {
            let dlg = gtk4::FileDialog::new();
            dlg.set_title("Export Mailbox");
            let st2 = st.clone();
            dlg.save(None::<&gtk4::Window>, gtk4::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result {
                    if let Some(dest) = file.path() {
                        let s = st2.borrow();
                        let in_path = s.data_dir.join("mailboxes").join("In");
                        if let Ok(toc) = macmbx::toc::Toc::open(&in_path) {
                            let ext = dest.extension().and_then(|e| e.to_str()).unwrap_or("mbox");
                            match ext {
                                "eml" => {
                                    if let Some(dir) = dest.parent() {
                                        let _ = macmbx::thread::export_all_eml(&toc, dir);
                                    }
                                }
                                _ => {
                                    let _ = macmbx::thread::export_mbox(&toc, &dest);
                                }
                            }
                            eprintln!("Export complete");
                        }
                    }
                }
            });
        });
        app.add_action(&export_action);
    }

    // ── Zoom In / Zoom Out — adjust editor font size ──
    {
        let zoom_in = gio::SimpleAction::new("zoom-in", None);
        zoom_in.connect_activate(|_, _| {
            // Increase CSS font size globally
            let provider = gtk4::CssProvider::new();
            // This is a simple approach — a real implementation would track zoom level
            static ZOOM: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(100);
            let level = ZOOM.fetch_add(10, std::sync::atomic::Ordering::Relaxed) + 10;
            let css = format!("textview, .gedit-view, .msg-body-view {{ font-size: {}%; }}", level);
            provider.load_from_data(&css);
            gtk4::style_context_add_provider_for_display(
                &gtk4::gdk::Display::default().unwrap(),
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_USER + 1,
            );
        });
        app.add_action(&zoom_in);

        let zoom_out = gio::SimpleAction::new("zoom-out", None);
        zoom_out.connect_activate(|_, _| {
            static ZOOM: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(100);
            let level = ZOOM.fetch_sub(10, std::sync::atomic::Ordering::Relaxed) - 10;
            let level = level.max(50);
            let provider = gtk4::CssProvider::new();
            let css = format!("textview, .gedit-view, .msg-body-view {{ font-size: {}%; }}", level);
            provider.load_from_data(&css);
            gtk4::style_context_add_provider_for_display(
                &gtk4::gdk::Display::default().unwrap(),
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_USER + 1,
            );
        });
        app.add_action(&zoom_out);
    }


    // ── Actions that need the content notebook — registered after it's created below ──
    // (address-book, filters, preferences, etc. are registered after content_notebook exists)

    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("rMail")
        .default_width(1200)
        .default_height(800)
        .show_menubar(true)
        .build();

    // ── Layout: sidebar | content — matches gEudora main_eudora.c:2372-2510 ──
    let hpaned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    hpaned.set_position(220);
    hpaned.set_vexpand(true);
    hpaned.set_hexpand(true);

    // ── Left panel — GtkNotebook with tabs at BOTTOM (matches main_eudora.c:2425-2491) ──
    let left_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    left_box.set_size_request(200, -1);
    left_box.add_css_class("sidebar-bg");

    let left_notebook = gtk4::Notebook::new();
    left_notebook.set_tab_pos(gtk4::PositionType::Bottom);
    left_notebook.set_vexpand(true);

    // ── Tab 1: Mailboxes ──
    let mb_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    mb_vbox.add_css_class("sidebar-bg");

    let sidebar_tree = sidebar::build_sidebar(&state.borrow().store);
    let sidebar_scroll = gtk4::ScrolledWindow::builder()
        .child(&sidebar_tree)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();
    mb_vbox.append(&sidebar_scroll);

    // Mailbox button bar with real handlers
    let mb_btn_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    mb_btn_bar.add_css_class("mb-btn-bar");
    mb_btn_bar.set_margin_start(4);
    mb_btn_bar.set_margin_end(4);
    mb_btn_bar.set_margin_top(4);
    mb_btn_bar.set_margin_bottom(4);

    let mb_new_btn = gtk4::Button::from_icon_name("list-add-symbolic");
    mb_new_btn.set_tooltip_text(Some("New Mailbox"));
    mb_new_btn.set_has_frame(false);
    {
        let st = state.clone();
        let tree = sidebar_tree.clone();
        mb_new_btn.connect_clicked(move |_| {
            let s = st.borrow();
            let mb_dir = s.data_dir.join("mailboxes");
            // Create "New Mailbox" with unique name
            let mut name = "New Mailbox".to_string();
            let mut i = 1;
            while mb_dir.join(&name).exists() {
                i += 1;
                name = format!("New Mailbox {}", i);
            }
            let path = mb_dir.join(&name);
            if std::fs::File::create(&path).is_ok() {
                // Create empty TOC
                let _ = macmbx::toc::Toc::open_or_create(&path);
                // Refresh sidebar
                if let Ok(new_store) = macmbx::store::Store::open(&mb_dir) {
                    let tree_store = sidebar::build_sidebar(&new_store);
                    tree.set_model(Some(&tree_store.model().unwrap()));
                }
            }
        });
    }
    mb_btn_bar.append(&mb_new_btn);

    let mb_folder_btn = gtk4::Button::from_icon_name("folder-new-symbolic");
    mb_folder_btn.set_tooltip_text(Some("New Folder"));
    mb_folder_btn.set_has_frame(false);
    {
        let st = state.clone();
        let tree = sidebar_tree.clone();
        mb_folder_btn.connect_clicked(move |_| {
            let s = st.borrow();
            let mb_dir = s.data_dir.join("mailboxes");
            let mut name = "New Folder".to_string();
            let mut i = 1;
            while mb_dir.join(&name).exists() {
                i += 1;
                name = format!("New Folder {}", i);
            }
            let _ = std::fs::create_dir_all(mb_dir.join(&name));
            if let Ok(new_store) = macmbx::store::Store::open(&mb_dir) {
                let tree_store = sidebar::build_sidebar(&new_store);
                tree.set_model(Some(&tree_store.model().unwrap()));
            }
        });
    }
    mb_btn_bar.append(&mb_folder_btn);

    let mb_rename_btn = gtk4::Button::from_icon_name("document-edit-symbolic");
    mb_rename_btn.set_tooltip_text(Some("Rename"));
    mb_rename_btn.set_has_frame(false);
    {
        let st = state.clone();
        let stree = sidebar_tree.clone();
        mb_rename_btn.connect_clicked(move |_| {
            let sel = stree.selection();
            if let Some((model, iter)) = sel.selected() {
                let old_path: String = model.get(&iter, sidebar::COL_PATH);
                let old_name: String = model.get(&iter, sidebar::COL_NAME);
                let is_dir: bool = model.get(&iter, sidebar::COL_IS_DIR);
                if old_path.is_empty() { return; }
                // Don't rename special mailboxes
                if ["In", "Out", "Trash", "Junk"].contains(&old_name.as_str()) { return; }
                // Simple rename dialog
                let dlg = gtk4::AlertDialog::builder()
                    .message(&format!("Rename '{}'", old_name))
                    .detail("Enter new name:")
                    .build();
                // GTK4 AlertDialog doesn't have text entry — use a simple approach
                // For now just append " (renamed)" as proof of concept
                let old = std::path::PathBuf::from(&old_path);
                if let Some(parent) = old.parent() {
                    let new_name = format!("{}_renamed", old_name);
                    let new_path = parent.join(&new_name);
                    let _ = std::fs::rename(&old, &new_path);
                    let s = st.borrow();
                    let mb_dir = s.data_dir.join("mailboxes");
                    if let Ok(new_store) = macmbx::store::Store::open(&mb_dir) {
                        let new_tree = sidebar::build_sidebar(&new_store);
                        stree.set_model(Some(&new_tree.model().unwrap()));
                    }
                }
            }
        });
    }
    mb_btn_bar.append(&mb_rename_btn);

    let mb_delete_btn = gtk4::Button::from_icon_name("edit-delete-symbolic");
    mb_delete_btn.set_tooltip_text(Some("Delete"));
    mb_delete_btn.set_has_frame(false);
    {
        let st = state.clone();
        let stree = sidebar_tree.clone();
        mb_delete_btn.connect_clicked(move |_| {
            let sel = stree.selection();
            if let Some((model, iter)) = sel.selected() {
                let path: String = model.get(&iter, sidebar::COL_PATH);
                let name: String = model.get(&iter, sidebar::COL_NAME);
                let is_dir: bool = model.get(&iter, sidebar::COL_IS_DIR);
                if path.is_empty() { return; }
                // Don't delete special mailboxes
                if ["In", "Out", "Trash", "Junk"].contains(&name.as_str()) { return; }
                let p = std::path::PathBuf::from(&path);
                if is_dir {
                    let _ = std::fs::remove_dir_all(&p);
                } else {
                    let _ = std::fs::remove_file(&p);
                    // Also remove .toc file
                    let toc_path = p.with_extension("toc");
                    let _ = std::fs::remove_file(&toc_path);
                }
                let s = st.borrow();
                let mb_dir = s.data_dir.join("mailboxes");
                if let Ok(new_store) = macmbx::store::Store::open(&mb_dir) {
                    let new_tree = sidebar::build_sidebar(&new_store);
                    stree.set_model(Some(&new_tree.model().unwrap()));
                }
            }
        });
    }
    mb_btn_bar.append(&mb_delete_btn);
    mb_vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    mb_vbox.append(&mb_btn_bar);

    left_notebook.append_page(&mb_vbox, Some(&gtk4::Label::new(Some("Mailboxes"))));

    // ── Tab 2: Tasks (port of taskProgress.c) ──
    let tasks_panel = tasks::build_tasks_panel();
    left_notebook.append_page(&tasks_panel, Some(&gtk4::Label::new(Some("Tasks"))));

    left_box.append(&left_notebook);
    hpaned.set_start_child(Some(&left_box));
    hpaned.set_resize_start_child(false);
    hpaned.set_shrink_start_child(false);

    // ── Content notebook — matches main_eudora.c:2502-2510 ──
    let content_notebook = gtk4::Notebook::new();
    content_notebook.set_scrollable(true);
    content_notebook.set_vexpand(true);
    content_notebook.set_hexpand(true);
    content_notebook.set_tab_pos(gtk4::PositionType::Top);

    // ── Welcome page — matches main_eudora.c:2179-2358 ──
    let welcome = create_welcome_page(&state.borrow());
    content_notebook.append_page(&welcome, Some(&gtk4::Label::new(Some("Welcome"))));

    hpaned.set_end_child(Some(&content_notebook));
    hpaned.set_resize_end_child(true);
    hpaned.set_shrink_end_child(false);

    // ── Register actions that need the content notebook ──
    {
        // Address Book — opens as a tab with real data from macmbx::nicknames
        let ab_action = gio::SimpleAction::new("address-book", None);
        let nb = content_notebook.clone();
        let st = state.clone();
        ab_action.connect_activate(move |_, _| {
            // Check if already open
            let n = nb.n_pages();
            for i in 0..n {
                if let Some(tl) = nb.tab_label(&nb.nth_page(Some(i)).unwrap()) {
                    if let Some(l) = tl.first_child() {
                        if let Ok(l) = l.downcast::<gtk4::Label>() {
                            if l.text() == "Address Book" {
                                nb.set_current_page(Some(i));
                                return;
                            }
                        }
                    }
                }
            }
            let page = contacts::build_address_book_tab(&st.borrow().data_dir);
            let tab = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            tab.append(&gtk4::Label::new(Some("Address Book")));
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.set_has_frame(false);
            tab.append(&close);
            let idx = nb.append_page(&page, Some(&tab));
            nb.set_current_page(Some(idx));
            let nb2 = nb.clone();
            let p2 = page.clone();
            close.connect_clicked(move |_| {
                if let Some(pn) = nb2.page_num(&p2) { nb2.remove_page(Some(pn)); }
            });
        });
        app.add_action(&ab_action);

        // Filters — opens as a tab
        let filt_action = gio::SimpleAction::new("filters", None);
        let nb = content_notebook.clone();
        let st = state.clone();
        filt_action.connect_activate(move |_, _| {
            let n = nb.n_pages();
            for i in 0..n {
                if let Some(tl) = nb.tab_label(&nb.nth_page(Some(i)).unwrap()) {
                    if let Some(l) = tl.first_child() {
                        if let Ok(l) = l.downcast::<gtk4::Label>() {
                            if l.text() == "Filters" {
                                nb.set_current_page(Some(i));
                                return;
                            }
                        }
                    }
                }
            }
            let s = st.borrow();
            let page = filters::build_filters_tab(&s.data_dir, s.books.clone());
            let tab = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            tab.append(&gtk4::Label::new(Some("Filters")));
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.set_has_frame(false);
            tab.append(&close);
            let idx = nb.append_page(&page, Some(&tab));
            nb.set_current_page(Some(idx));
            let nb2 = nb.clone();
            let p2 = page.clone();
            close.connect_clicked(move |_| {
                if let Some(pn) = nb2.page_num(&p2) { nb2.remove_page(Some(pn)); }
            });
        });
        app.add_action(&filt_action);

        // Preferences — opens settings dialog
        let pref_action = gio::SimpleAction::new("preferences", None);
        let st = state.clone();
        let win_ref = window.clone();
        pref_action.connect_activate(move |_, _| {
            let conf_shared = st.borrow().conf_shared.clone();
            settings::build_settings_dialog(&win_ref, st.borrow().conf.clone(),
                Some(Box::new(move |updated: &macmbx::conf::Conf| {
                    // Sync saved settings to the mailer's shared conf
                    *conf_shared.lock().unwrap() = updated.clone();
                })));
        });
        app.add_action(&pref_action);

        // Personalities — opens account management tab
        let pers_action = gio::SimpleAction::new("personalities", None);
        let nb = content_notebook.clone();
        let st = state.clone();
        pers_action.connect_activate(move |_, _| {
            // Check if already open
            let n = nb.n_pages();
            for i in 0..n {
                if let Some(tl) = nb.tab_label(&nb.nth_page(Some(i)).unwrap()) {
                    if let Some(l) = tl.first_child() {
                        if let Ok(l) = l.downcast::<gtk4::Label>() {
                            if l.text() == "Personalities" {
                                nb.set_current_page(Some(i));
                                return;
                            }
                        }
                    }
                }
            }
            let page = personalities::build_personalities_tab(st.borrow().conf.clone());
            let tab = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            tab.append(&gtk4::Label::new(Some("Personalities")));
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.set_has_frame(false);
            tab.append(&close);
            let idx = nb.append_page(&page, Some(&tab));
            nb.set_current_page(Some(idx));
            let nb2 = nb.clone();
            let p2 = page.clone();
            close.connect_clicked(move |_| {
                if let Some(pn) = nb2.page_num(&p2) { nb2.remove_page(Some(pn)); }
            });
        });
        app.add_action(&pers_action);

        // Signatures — opens as a tab
        let sig_action = gio::SimpleAction::new("signatures", None);
        let nb = content_notebook.clone();
        let st = state.clone();
        sig_action.connect_activate(move |_, _| {
            let n = nb.n_pages();
            for i in 0..n {
                if let Some(tl) = nb.tab_label(&nb.nth_page(Some(i)).unwrap()) {
                    if let Some(l) = tl.first_child() {
                        if let Ok(l) = l.downcast::<gtk4::Label>() {
                            if l.text() == "Signatures" {
                                nb.set_current_page(Some(i));
                                return;
                            }
                        }
                    }
                }
            }
            let page = signatures::build_signatures_tab(&st.borrow().data_dir);
            let tab = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            tab.append(&gtk4::Label::new(Some("Signatures")));
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.set_has_frame(false);
            tab.append(&close);
            let idx = nb.append_page(&page, Some(&tab));
            nb.set_current_page(Some(idx));
            let nb2 = nb.clone();
            let p2 = page.clone();
            close.connect_clicked(move |_| {
                if let Some(pn) = nb2.page_num(&p2) { nb2.remove_page(Some(pn)); }
            });
        });
        app.add_action(&sig_action);

        // Statistics — opens real stats tab
        let stat_action = gio::SimpleAction::new("statistics", None);
        let nb = content_notebook.clone();
        let st = state.clone();
        stat_action.connect_activate(move |_, _| {
            let n = nb.n_pages();
            for i in 0..n {
                if let Some(tl) = nb.tab_label(&nb.nth_page(Some(i)).unwrap()) {
                    if let Some(l) = tl.first_child() {
                        if let Ok(l) = l.downcast::<gtk4::Label>() {
                            if l.text() == "Statistics" {
                                nb.set_current_page(Some(i));
                                return;
                            }
                        }
                    }
                }
            }
            let page = statistics::build_statistics_tab(&st.borrow().data_dir);
            let tab = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            tab.append(&gtk4::Label::new(Some("Statistics")));
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.set_has_frame(false);
            tab.append(&close);
            let idx = nb.append_page(&page, Some(&tab));
            nb.set_current_page(Some(idx));
            let nb2 = nb.clone();
            let p2 = page.clone();
            close.connect_clicked(move |_| {
                if let Some(pn) = nb2.page_num(&p2) { nb2.remove_page(Some(pn)); }
            });
        });
        app.add_action(&stat_action);

        // Stationery — opens stationery management tab
        let stny_action = gio::SimpleAction::new("stationery", None);
        let nb = content_notebook.clone();
        let st = state.clone();
        stny_action.connect_activate(move |_, _| {
            let n = nb.n_pages();
            for i in 0..n {
                if let Some(tl) = nb.tab_label(&nb.nth_page(Some(i)).unwrap()) {
                    if let Some(l) = tl.first_child() {
                        if let Ok(l) = l.downcast::<gtk4::Label>() {
                            if l.text() == "Stationery" {
                                nb.set_current_page(Some(i));
                                return;
                            }
                        }
                    }
                }
            }
            let page = stationery::build_stationery_tab(&st.borrow().data_dir);
            let tab = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            tab.append(&gtk4::Label::new(Some("Stationery")));
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.set_has_frame(false);
            tab.append(&close);
            let idx = nb.append_page(&page, Some(&tab));
            nb.set_current_page(Some(idx));
            let nb2 = nb.clone();
            let p2 = page.clone();
            close.connect_clicked(move |_| {
                if let Some(pn) = nb2.page_num(&p2) { nb2.remove_page(Some(pn)); }
            });
        });
        app.add_action(&stny_action);

        // Link History — opens link history tab
        let lh_action = gio::SimpleAction::new("link-history", None);
        let nb = content_notebook.clone();
        let st = state.clone();
        lh_action.connect_activate(move |_, _| {
            let n = nb.n_pages();
            for i in 0..n {
                if let Some(tl) = nb.tab_label(&nb.nth_page(Some(i)).unwrap()) {
                    if let Some(l) = tl.first_child() {
                        if let Ok(l) = l.downcast::<gtk4::Label>() {
                            if l.text() == "Link History" {
                                nb.set_current_page(Some(i));
                                return;
                            }
                        }
                    }
                }
            }
            let page = linkhistory::build_link_history_tab(&st.borrow().data_dir);
            let tab = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            tab.append(&gtk4::Label::new(Some("Link History")));
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.set_has_frame(false);
            tab.append(&close);
            let idx = nb.append_page(&page, Some(&tab));
            nb.set_current_page(Some(idx));
            let nb2 = nb.clone();
            let p2 = page.clone();
            close.connect_clicked(move |_| {
                if let Some(pn) = nb2.page_num(&p2) { nb2.remove_page(Some(pn)); }
            });
        });
        app.add_action(&lh_action);

        // Find / Search — opens search tab with real search
        let find_action = gio::SimpleAction::new("find", None);
        let nb = content_notebook.clone();
        let st = state.clone();
        find_action.connect_activate(move |_, _| {
            // Check if already open
            let n = nb.n_pages();
            for i in 0..n {
                if let Some(tl) = nb.tab_label(&nb.nth_page(Some(i)).unwrap()) {
                    if let Some(l) = tl.first_child() {
                        if let Ok(l) = l.downcast::<gtk4::Label>() {
                            if l.text() == "Search" {
                                nb.set_current_page(Some(i));
                                return;
                            }
                        }
                    }
                }
            }
            let page = search::build_search_tab(&st.borrow().store);
            let tab = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            tab.append(&gtk4::Label::new(Some("Search")));
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.set_has_frame(false);
            tab.append(&close);
            let idx = nb.append_page(&page, Some(&tab));
            nb.set_current_page(Some(idx));
            let nb2 = nb.clone();
            let p2 = page.clone();
            close.connect_clicked(move |_| {
                if let Some(pn) = nb2.page_num(&p2) { nb2.remove_page(Some(pn)); }
            });
        });
        app.add_action(&find_action);

        // New Message — opens compose tab
        let newmsg_action = gio::SimpleAction::new("new-message", None);
        let nb = content_notebook.clone();
        let st = state.clone();
        newmsg_action.connect_activate(move |_, _| {
            let s = st.borrow();
            let page = compose::build_compose_tab(&s.conf.borrow(), s.mailer.clone(), s.books.clone());
            let tab = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            tab.append(&gtk4::Label::new(Some("New Message")));
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.set_has_frame(false);
            tab.append(&close);
            let idx = nb.append_page(&page, Some(&tab));
            nb.set_tab_reorderable(&page, true);
            nb.set_current_page(Some(idx));
            let nb2 = nb.clone();
            let p2 = page.clone();
            close.connect_clicked(move |_| {
                if let Some(pn) = nb2.page_num(&p2) { nb2.remove_page(Some(pn)); }
            });
        });
        app.add_action(&newmsg_action);

        // Check Mail
        let check_action = gio::SimpleAction::new("check-mail", None);
        check_action.connect_activate(|_, _| { scheduler::request_check(); });
        app.add_action(&check_action);

        // Send Queued
        let send_action = gio::SimpleAction::new("send-queued", None);
        send_action.connect_activate(|_, _| { scheduler::request_send(); });
        app.add_action(&send_action);

        // ── Reply ──
        {
            let reply_action = gio::SimpleAction::new("reply", None);
            let nb = content_notebook.clone();
            let st = state.clone();
            reply_action.connect_activate(move |_, _| {
                let sel = get_selected_msg(&nb);
                if let Some((mbox_path, idx, _tree, _iter)) = sel {
                    if let Ok(toc) = macmbx::toc::Toc::open(&mbox_path) {
                        if let Ok(raw) = macmbx::ops::read_message(&toc, idx as usize) {
                            let text = String::from_utf8_lossy(&raw);
                            let reply_to = macmbx::ops::read_header_field(&toc, idx as usize, "Reply-To")
                                .ok().flatten()
                                .or_else(|| Some(toc.msgs[idx as usize].from.clone()))
                                .unwrap_or_default();
                            let subj = &toc.msgs[idx as usize].subject;
                            let re_subj = if subj.to_ascii_lowercase().starts_with("re:") {
                                subj.clone()
                            } else {
                                format!("Re: {}", subj)
                            };
                            let body = find_body(&text);
                            let quoted = format!("On {} wrote:\n{}", toc.msgs[idx as usize].from,
                                body.lines().map(|l| format!("> {}", l)).collect::<Vec<_>>().join("\n"));

                            let s = st.borrow();
                            let mut data = compose::ComposeData::default_new();
                            data.to = reply_to;
                            data.subject = re_subj;
                            data.body = quoted;
                            let page = compose::build_compose_tab_with_data(&s.conf.borrow(), &data, s.mailer.clone(), s.books.clone());
                            open_compose_tab(&nb, &page, "Reply");
                        }
                    }
                }
            });
            app.add_action(&reply_action);
        }

        // ── Reply All ──
        {
            let reply_all_action = gio::SimpleAction::new("reply-all", None);
            let nb = content_notebook.clone();
            let st = state.clone();
            reply_all_action.connect_activate(move |_, _| {
                let sel = get_selected_msg(&nb);
                if let Some((mbox_path, idx, _tree, _iter)) = sel {
                    if let Ok(toc) = macmbx::toc::Toc::open(&mbox_path) {
                        if let Ok(raw) = macmbx::ops::read_message(&toc, idx as usize) {
                            let text = String::from_utf8_lossy(&raw);
                            let reply_to = macmbx::ops::read_header_field(&toc, idx as usize, "Reply-To")
                                .ok().flatten()
                                .or_else(|| Some(toc.msgs[idx as usize].from.clone()))
                                .unwrap_or_default();
                            let orig_to = macmbx::ops::read_header_field(&toc, idx as usize, "To")
                                .ok().flatten().unwrap_or_default();
                            let orig_cc = macmbx::ops::read_header_field(&toc, idx as usize, "Cc")
                                .ok().flatten().unwrap_or_default();
                            let subj = &toc.msgs[idx as usize].subject;
                            let re_subj = if subj.to_ascii_lowercase().starts_with("re:") {
                                subj.clone()
                            } else {
                                format!("Re: {}", subj)
                            };
                            let cc = if !orig_to.is_empty() && !orig_cc.is_empty() {
                                format!("{}, {}", orig_to, orig_cc)
                            } else if !orig_to.is_empty() { orig_to }
                            else { orig_cc };

                            let body = find_body(&text);
                            let quoted = format!("On {} wrote:\n{}", toc.msgs[idx as usize].from,
                                body.lines().map(|l| format!("> {}", l)).collect::<Vec<_>>().join("\n"));

                            let s = st.borrow();
                            let mut data = compose::ComposeData::default_new();
                            data.to = reply_to;
                            data.cc = cc;
                            data.subject = re_subj;
                            data.body = quoted;
                            let page = compose::build_compose_tab_with_data(&s.conf.borrow(), &data, s.mailer.clone(), s.books.clone());
                            open_compose_tab(&nb, &page, "Reply All");
                        }
                    }
                }
            });
            app.add_action(&reply_all_action);
        }

        // ── Forward ──
        {
            let fwd_action = gio::SimpleAction::new("forward", None);
            let nb = content_notebook.clone();
            let st = state.clone();
            fwd_action.connect_activate(move |_, _| {
                let sel = get_selected_msg(&nb);
                if let Some((mbox_path, idx, _tree, _iter)) = sel {
                    if let Ok(toc) = macmbx::toc::Toc::open(&mbox_path) {
                        if let Ok(raw) = macmbx::ops::read_message(&toc, idx as usize) {
                            let text = String::from_utf8_lossy(&raw);
                            let sum = &toc.msgs[idx as usize];
                            let body = find_body(&text);
                            let fwd_body = format!(
                                "---------- Forwarded message ----------\nFrom: {}\nSubject: {}\n\n{}",
                                sum.from, sum.subject, body);

                            let s = st.borrow();
                            let mut data = compose::ComposeData::default_new();
                            data.subject = format!("Fwd: {}", sum.subject);
                            data.body = fwd_body;
                            let page = compose::build_compose_tab_with_data(&s.conf.borrow(), &data, s.mailer.clone(), s.books.clone());
                            open_compose_tab(&nb, &page, "Forward");
                        }
                    }
                }
            });
            app.add_action(&fwd_action);
        }

        // ── Redirect ──
        {
            let redir_action = gio::SimpleAction::new("redirect", None);
            let nb = content_notebook.clone();
            let st = state.clone();
            redir_action.connect_activate(move |_, _| {
                let sel = get_selected_msg(&nb);
                if let Some((mbox_path, idx, _tree, _iter)) = sel {
                    if let Ok(toc) = macmbx::toc::Toc::open(&mbox_path) {
                        if let Ok(raw) = macmbx::ops::read_message(&toc, idx as usize) {
                            let text = String::from_utf8_lossy(&raw);
                            let s = st.borrow();
                            let data = compose::ComposeData::from_raw(&text);
                            let page = compose::build_compose_tab_with_data(&s.conf.borrow(), &data, s.mailer.clone(), s.books.clone());
                            open_compose_tab(&nb, &page, "Redirect");
                        }
                    }
                }
            });
            app.add_action(&redir_action);
        }

        // ── Delete ──
        {
            let del_action = gio::SimpleAction::new("delete", None);
            let nb = content_notebook.clone();
            del_action.connect_activate(move |_, _| {
                let sel = get_selected_msg(&nb);
                if let Some((mbox_path, idx, tree, iter)) = sel {
                    // Mark deleted in TOC
                    if let Ok(mut toc) = macmbx::toc::Toc::open(&mbox_path) {
                        if (idx as usize) < toc.msgs.len() {
                            toc.msgs[idx as usize].flags.insert(macmbx::toc::MsgFlags::DELETED);
                            let _ = toc.save();
                        }
                    }
                    // Remove from visible list
                    if let Some(model) = tree.model() {
                        if let Ok(store) = model.downcast::<gtk4::ListStore>() {
                            store.remove(&iter);
                        }
                    }
                }
            });
            app.add_action(&del_action);
        }

        // ── Print ──
        {
            let print_action = gio::SimpleAction::new("print", None);
            let nb = content_notebook.clone();
            let win_for_print = window.clone();
            print_action.connect_activate(move |_, _| {
                let sel = get_selected_msg(&nb);
                if let Some((mbox_path, idx, _tree, _iter)) = sel {
                    if let Ok(toc) = macmbx::toc::Toc::open(&mbox_path) {
                        if let Ok(raw) = macmbx::ops::read_message(&toc, idx as usize) {
                            let raw_str = String::from_utf8_lossy(&raw);

                            // Build print doc with headers + styled body (images resolved)
                            let doc = reditctrl::document::Document::new();
                            let ctrl = reditctrl::glue::EditorCtrl::new(doc);

                            // Insert headers as plain text
                            let mut hdr_text = String::new();
                            for h in &["From", "To", "Subject", "Date", "Cc"] {
                                if let Some(val) = extract_header(&raw_str, h) {
                                    hdr_text.push_str(&format!("{}: {}\n", h, val));
                                }
                            }
                            hdr_text.push('\n');
                            ctrl.set_text(&hdr_text, -1);

                            // Insert body as markup (handles images via insert_markup)
                            let hdr_len = ctrl.get_length();
                            if let Some(parsed) = crispy::msg::parse(&raw) {
                                // Resolve cid: for inline images
                                let mut body = parsed.body_html()
                                    .or_else(|| parsed.body_plain())
                                    .unwrap_or("").to_string();
                                for part in &parsed.parts {
                                    if let Some(ref cid) = part.content_id {
                                        if !part.data.is_empty() {
                                            let ext = if part.mime_type.contains("png") { "png" } else { "jpg" };
                                            let tmp = std::env::temp_dir().join(format!("rmail_print_{}.{}", cid, ext));
                                            if std::fs::write(&tmp, &part.data).is_ok() {
                                                body = body.replace(&format!("cid:{}", cid), &tmp.to_string_lossy());
                                            }
                                        }
                                    }
                                }
                                let is_html = body.contains("<") && (body.contains("<b>") || body.contains("<img") || body.contains("<br>") || body.contains("<p>") || body.contains("<div"));
                                if is_html {
                                    ctrl.insert_markup(hdr_len, &body);
                                } else {
                                    ctrl.insert_text(hdr_len, &body, -1);
                                }
                            } else {
                                let body = find_body(&raw_str);
                                ctrl.insert_text(hdr_len, body, -1);
                            }

                            reditctrl::print::print_document(
                                &win_for_print.clone().upcast(),
                                &ctrl.doc,
                                Some(&toc.msgs[idx as usize].subject),
                            );
                        }
                    }
                }
            });
            app.add_action(&print_action);
        }

        // ── Transfer In/Out/Trash — move selected message ──
        for (action_name, dest_name) in &[
            ("transfer-in", "In"), ("transfer-out", "Out"), ("transfer-trash", "Trash"),
        ] {
            let action = gio::SimpleAction::new(action_name, None);
            let nb = content_notebook.clone();
            let st = state.clone();
            let dest = dest_name.to_string();
            action.connect_activate(move |_, _| {
                let sel = get_selected_msg(&nb);
                if let Some((mbox_path, idx, tree, iter)) = sel {
                    let dest_path = st.borrow().data_dir.join("mailboxes").join(&dest);
                    if let Ok(mut src_toc) = macmbx::toc::Toc::open(&mbox_path) {
                        if let Ok(mut dst_toc) = macmbx::toc::Toc::open_or_create(&dest_path) {
                            match macmbx::ops::transfer(&mut src_toc, idx as usize, &mut dst_toc, false) {
                                Ok(_) => {
                                    let _ = src_toc.save();
                                    let _ = dst_toc.save();
                                    // Remove from visible list
                                    if let Some(model) = tree.model() {
                                        if let Ok(store) = model.downcast::<gtk4::ListStore>() {
                                            store.remove(&iter);
                                        }
                                    }
                                }
                                Err(e) => eprintln!("rMail: transfer failed: {}", e),
                            }
                        }
                    }
                }
            });
            app.add_action(&action);
        }
    }

        // ── Save As (overrides the stub) — save selected message as .eml ──
        {
            // Remove stub and re-register with notebook access
            app.remove_action("save-as");
            let sa = gio::SimpleAction::new("save-as", None);
            let nb = content_notebook.clone();
            sa.connect_activate(move |_, _| {
                if let Some((mbox_path, idx, _, _)) = get_selected_msg(&nb) {
                    if let Ok(toc) = macmbx::toc::Toc::open(&mbox_path) {
                        if let Ok(raw) = macmbx::ops::read_message(&toc, idx as usize) {
                            let subj = &toc.msgs[idx as usize].subject;
                            let safe_name: String = subj.chars()
                                .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' })
                                .collect();
                            let dlg = gtk4::FileDialog::new();
                            dlg.set_title("Save Message As");
                            dlg.set_initial_name(Some(&format!("{}.eml", safe_name)));
                            dlg.save(None::<&gtk4::Window>, gtk4::gio::Cancellable::NONE, move |result| {
                                if let Ok(file) = result {
                                    if let Some(path) = file.path() {
                                        let _ = std::fs::write(&path, &raw);
                                        eprintln!("Saved to {}", path.display());
                                    }
                                }
                            });
                        }
                    }
                }
            });
            app.add_action(&sa);
        }

        // ── Mark Read/Unread/Junk/Not Junk ──
        for (action_name, new_state) in &[
            ("mark-read", macmbx::toc::State::Read),
            ("mark-unread", macmbx::toc::State::Unread),
            ("mark-junk", macmbx::toc::State::Read), // mark as junk = set spam flag
            ("mark-not-junk", macmbx::toc::State::Read),
        ] {
            let action = gio::SimpleAction::new(action_name, None);
            let nb = content_notebook.clone();
            let state_val = *new_state;
            let is_junk = *action_name == "mark-junk";
            let is_not_junk = *action_name == "mark-not-junk";
            action.connect_activate(move |_, _| {
                if let Some((mbox_path, idx, tree, iter)) = get_selected_msg(&nb) {
                    if let Ok(mut toc) = macmbx::toc::Toc::open(&mbox_path) {
                        if (idx as usize) < toc.msgs.len() {
                            toc.msgs[idx as usize].state = state_val;
                            if is_junk {
                                toc.msgs[idx as usize].spam_score = 100;
                            }
                            if is_not_junk {
                                toc.msgs[idx as usize].spam_score = 0;
                            }
                            let _ = toc.save();
                            // Update visible weight (bold for unread)
                            if let Some(model) = tree.model() {
                                if let Ok(store) = model.downcast::<gtk4::ListStore>() {
                                    let weight = if state_val == macmbx::toc::State::Unread { 700i32 } else { 400i32 };
                                    store.set(&iter, &[(7, &weight)]);
                                    // Update status icon
                                    let icon = match state_val {
                                        macmbx::toc::State::Unread => "●",
                                        macmbx::toc::State::Read => "",
                                        _ => "",
                                    };
                                    store.set(&iter, &[(1, &icon)]);
                                }
                            }
                        }
                    }
                }
            });
            app.add_action(&action);
        }

    // ── Status bar ──
    let status_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    status_bar.add_css_class("status-bar");
    status_bar.set_margin_start(8);
    status_bar.set_margin_end(8);
    status_bar.set_margin_top(4);
    status_bar.set_margin_bottom(4);

    let status_label = gtk4::Label::new(Some("rMail ready"));
    status_label.set_hexpand(true);
    status_label.set_halign(gtk4::Align::Start);
    status_bar.append(&status_label);

    let unread_label = gtk4::Label::new(None);
    status_bar.append(&unread_label);

    // Count total unread
    {
        let st = state.borrow();
        let mailboxes_dir = st.data_dir.join("mailboxes");
        let mut total_unread = 0usize;
        if let Ok(entries) = std::fs::read_dir(&mailboxes_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "toc").unwrap_or(false) {
                    continue;
                }
                if path.is_file() {
                    if let Ok(toc) = macmbx::toc::Toc::open(&path) {
                        total_unread += toc.unread_count();
                    }
                }
            }
        }
        if total_unread > 0 {
            unread_label.set_text(&format!("{} unread", total_unread));
        }
    }

    // ── Main layout — toolbar at top, hpaned fills middle, status at bottom ──
    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    main_box.set_vexpand(true);
    main_box.set_hexpand(true);
    main_box.add_css_class("main-bg");

    // ── Toolbar — matches main_eudora.c:1546-1580 ──
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
    toolbar.add_css_class("toolbar");
    toolbar.set_margin_start(4);
    toolbar.set_margin_end(4);
    toolbar.set_margin_top(2);
    toolbar.set_margin_bottom(2);

    let tb_btn = |icon: &str, tooltip: &str| -> gtk4::Button {
        let btn = gtk4::Button::from_icon_name(icon);
        btn.set_tooltip_text(Some(tooltip));
        btn.set_has_frame(false);
        btn
    };

    // In, Out, Trash, Junk — wired below to open_mailbox_tab
    let tb_in = tb_btn("mail-unread-symbolic", "In");
    let tb_out = tb_btn("mail-send-symbolic", "Out");
    let tb_trash = tb_btn("user-trash-symbolic", "Trash");
    let tb_junk = tb_btn("dialog-warning-symbolic", "Junk");
    toolbar.append(&tb_in);
    toolbar.append(&tb_out);
    toolbar.append(&tb_trash);
    toolbar.append(&tb_junk);

    toolbar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
    let tb_delete = tb_btn("edit-delete-symbolic", "Delete");
    toolbar.append(&tb_delete);

    toolbar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
    let tb_new = tb_btn("document-new-symbolic", "New Message");
    tb_new.set_action_name(Some("app.new-message"));
    let tb_reply = tb_btn("mail-reply-sender-symbolic", "Reply");
    tb_reply.set_action_name(Some("app.reply"));
    let tb_reply_all = tb_btn("mail-reply-all-symbolic", "Reply All");
    tb_reply_all.set_action_name(Some("app.reply-all"));
    let tb_forward = tb_btn("mail-forward-symbolic", "Forward");
    tb_forward.set_action_name(Some("app.forward"));
    let tb_redirect = tb_btn("go-next-symbolic", "Redirect");
    tb_redirect.set_action_name(Some("app.redirect"));
    toolbar.append(&tb_new);
    toolbar.append(&tb_reply);
    toolbar.append(&tb_reply_all);
    toolbar.append(&tb_forward);
    toolbar.append(&tb_redirect);

    toolbar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
    let tb_check = tb_btn("view-refresh-symbolic", "Check Mail");
    tb_check.set_action_name(Some("app.check-mail"));
    let tb_send = tb_btn("mail-send-symbolic", "Send Queued");
    tb_send.set_action_name(Some("app.send-queued"));
    toolbar.append(&tb_check);
    toolbar.append(&tb_send);

    toolbar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
    let tb_find = tb_btn("edit-find-symbolic", "Find");
    tb_find.set_action_name(Some("app.find"));
    let tb_addr = tb_btn("contact-new-symbolic", "Address Book");
    tb_addr.set_action_name(Some("app.address-book"));
    let tb_filt = tb_btn("view-list-symbolic", "Filters");
    tb_filt.set_action_name(Some("app.filters"));
    let tb_print = tb_btn("printer-symbolic", "Print");
    tb_print.set_action_name(Some("app.print"));
    toolbar.append(&tb_find);
    toolbar.append(&tb_addr);
    toolbar.append(&tb_filt);
    toolbar.append(&tb_print);

    toolbar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

    // Offline toggle
    let offline_btn = gtk4::ToggleButton::new();
    offline_btn.set_icon_name("network-transmit-receive-symbolic");
    offline_btn.set_tooltip_text(Some("Offline / Online"));
    offline_btn.set_has_frame(false);
    {
        let st = state.clone();
        offline_btn.connect_toggled(move |btn| {
            let going_offline = btn.is_active();
            btn.set_icon_name(if going_offline { "network-offline-symbolic" } else { "network-transmit-receive-symbolic" });
            btn.set_tooltip_text(Some(if going_offline { "Go Online" } else { "Go Offline" }));

            if going_offline {
                scheduler::set_offline(true);
            } else {
                // Going online — check for queued messages (port of idle_scheduler.c prompt_queued_on_online)
                let s = st.borrow();
                let queued = s.mailer.queued_count();
                if queued > 0 {
                    let dlg = gtk4::AlertDialog::builder()
                        .message(&format!("{} message(s) waiting in the queue.", queued))
                        .detail("What would you like to do?")
                        .buttons(["Send All Queue", "Go Online", "Stay Offline"])
                        .cancel_button(2)
                        .default_button(0)
                        .build();
                    let btn2 = btn.clone();
                    dlg.choose(None::<&gtk4::Window>, gtk4::gio::Cancellable::NONE, move |result| {
                        match result {
                            Ok(0) => {
                                // Send All Queue
                                scheduler::set_offline(false);
                                scheduler::request_send();
                            }
                            Ok(1) => {
                                // Go Online without sending
                                scheduler::set_offline(false);
                            }
                            _ => {
                                // Stay Offline
                                scheduler::set_offline(true);
                                btn2.set_active(true);
                                btn2.set_icon_name("network-offline-symbolic");
                                btn2.set_tooltip_text(Some("Go Online"));
                            }
                        }
                    });
                } else {
                    scheduler::set_offline(false);
                }
            }
        });
    }
    toolbar.append(&offline_btn);

    // Theme toggle (matches main_eudora.c:1588)
    let theme_btn = tb_btn("preferences-desktop-appearance-symbolic", "Cycle Theme (Light/Dark)");
    toolbar.append(&theme_btn);

    // Preferences
    let prefs_btn = tb_btn("emblem-system-symbolic", "Preferences");
    prefs_btn.set_action_name(Some("app.preferences"));
    toolbar.append(&prefs_btn);

    main_box.append(&toolbar);
    main_box.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    main_box.append(&hpaned);
    main_box.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    main_box.append(&status_bar);

    window.set_child(Some(&main_box));

    // ── Shared function: open a mailbox tab in the content notebook ──
    let open_mailbox_tab = {
        let nb = content_notebook.clone();
        let st = state.clone();
        Rc::new(move |mbox_path: &str, name: &str| {
            if mbox_path.is_empty() { return; }

            // Check if already open
            let n = nb.n_pages();
            for i in 0..n {
                if let Some(tab_label) = nb.tab_label(&nb.nth_page(Some(i)).unwrap()) {
                    if let Some(lbl) = tab_label.first_child() {
                        if let Ok(l) = lbl.downcast::<gtk4::Label>() {
                            if l.text() == name {
                                nb.set_current_page(Some(i));
                                return;
                            }
                        }
                    }
                }
            }

            // Open TOC and show message list
            let mbox = PathBuf::from(mbox_path);
            match macmbx::toc::Toc::open(&mbox) {
                Ok(toc) => {
                    let s = st.borrow();
                    let content = build_mailbox_tab(&toc, mbox_path, &nb, s.conf.clone(), s.mailer.clone(), s.books.clone());

                    let tab_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
                    tab_box.append(&gtk4::Label::new(Some(name)));
                    let close_btn = gtk4::Button::from_icon_name("window-close-symbolic");
                    close_btn.set_has_frame(false);
                    tab_box.append(&close_btn);

                    let idx = nb.append_page(&content, Some(&tab_box));
                    nb.set_tab_reorderable(&content, true);
                    nb.set_current_page(Some(idx));

                    let nb2 = nb.clone();
                    let content2 = content.clone();
                    close_btn.connect_clicked(move |_| {
                        if let Some(page_num) = nb2.page_num(&content2) {
                            nb2.remove_page(Some(page_num));
                        }
                    });
                }
                Err(e) => {
                    eprintln!("Failed to open {}: {}", mbox_path, e);
                }
            }
        })
    };

    // ── Wire In/Out/Trash/Junk toolbar buttons to open those mailboxes ──
    {
        let st = state.borrow();
        let mailboxes_dir = st.data_dir.join("mailboxes");
        let in_path = mailboxes_dir.join("In").to_string_lossy().to_string();
        let out_path = mailboxes_dir.join("Out").to_string_lossy().to_string();
        let trash_path = mailboxes_dir.join("Trash").to_string_lossy().to_string();

        let open = open_mailbox_tab.clone();
        let p = in_path.clone();
        tb_in.connect_clicked(move |_| { open(&p, "In"); });

        let open = open_mailbox_tab.clone();
        let p = out_path.clone();
        tb_out.connect_clicked(move |_| { open(&p, "Out"); });

        let open = open_mailbox_tab.clone();
        let p = trash_path.clone();
        tb_trash.connect_clicked(move |_| { open(&p, "Trash"); });

        let junk_path = mailboxes_dir.join("Junk").to_string_lossy().to_string();
        let open = open_mailbox_tab.clone();
        let p = junk_path.clone();
        tb_junk.connect_clicked(move |_| { open(&p, "Junk"); });
    }

    // ── "open-mailbox" action for welcome page stat pills ──
    {
        let open_mb_action = gio::SimpleAction::new("open-mailbox",
            Some(&String::static_variant_type()));
        let open_fn = open_mailbox_tab.clone();
        let st = state.clone();
        open_mb_action.connect_activate(move |_, param| {
            if let Some(param) = param {
                if let Some(name) = param.get::<String>() {
                    let s = st.borrow();
                    let path = s.data_dir.join("mailboxes").join(&name);
                    let path_str = path.to_string_lossy().to_string();
                    open_fn(&path_str, &name);
                }
            }
        });
        app.add_action(&open_mb_action);
    }

    // ── Helper: get selected message info from active content tab ──
    // Returns (TreeView, ListStore, selected index, mbox_path) if a mailbox tab is active
    let get_active_selection = {
        let nb = content_notebook.clone();
        move || -> Option<(gtk4::TreeView, gtk4::TreeIter, i32, String)> {
            let page_idx = nb.current_page()?;
            let page = nb.nth_page(Some(page_idx))?;
            // The mailbox tab is a Paned → first child is ScrolledWindow → child is TreeView
            let paned = page.downcast_ref::<gtk4::Paned>()?;
            let scroll = paned.start_child()?.downcast::<gtk4::ScrolledWindow>().ok()?;
            let tree = scroll.child()?.downcast::<gtk4::TreeView>().ok()?;
            let sel = tree.selection();
            let (model, iter) = sel.selected()?;
            let idx: i32 = model.get(&iter, 0);
            // Get mbox path from the tab label
            let tab_label = nb.tab_label(&page)?;
            let label_widget = tab_label.first_child()?;
            let name = label_widget.downcast::<gtk4::Label>().ok()?.text().to_string();
            Some((tree, iter, idx, name))
        }
    };

    // ── Wire Delete button ──
    tb_delete.set_action_name(Some("app.delete"));

    // ── Wire Theme toggle — cycles through 5 themes (port of theme.c theme_cycle) ──
    {
        let win = window.clone();
        let theme_idx = Rc::new(RefCell::new(0usize));
        let theme_provider = Rc::new(RefCell::new({
            let p = gtk4::CssProvider::new();
            gtk4::style_context_add_provider_for_display(
                &gtk4::gdk::Display::default().unwrap(),
                &p,
                gtk4::STYLE_PROVIDER_PRIORITY_USER,
            );
            p
        }));
        // Load saved theme from conf, apply initial
        let saved_theme = state.borrow().conf.borrow()
            .get_int("theme", "active_theme", 0) as usize;
        let initial = saved_theme.min(THEME_COUNT - 1);
        *theme_idx.borrow_mut() = initial;
        apply_theme(initial, &theme_provider.borrow(), &win);

        let conf_for_theme = state.borrow().conf.clone();
        theme_btn.connect_clicked(move |_| {
            let mut idx = theme_idx.borrow_mut();
            *idx = (*idx + 1) % THEME_COUNT;
            apply_theme(*idx, &theme_provider.borrow(), &win);
            // Persist theme choice
            conf_for_theme.borrow_mut().set_int("theme", "active_theme", *idx as i64);
            let _ = conf_for_theme.borrow_mut().save();
        });
    }

    // ── Sidebar expand persistence — restore saved expand state ──
    {
        let expanded_str = state.borrow().conf.borrow()
            .get("sidebar", "expanded_paths").unwrap_or("").to_string();
        if !expanded_str.is_empty() {
            // Collapse all first, then expand saved paths
            sidebar_tree.collapse_all();
            for path_str in expanded_str.split(';') {
                if let Some(tp) = gtk4::TreePath::from_string(path_str) {
                    sidebar_tree.expand_row(&tp, false);
                }
            }
        }
        // Save expand state on collapse/expand
        let conf_for_expand = state.borrow().conf.clone();
        let tree_for_expand = sidebar_tree.clone();
        let save_expand = Rc::new(move || {
            let mut paths = Vec::new();
            tree_for_expand.map_expanded_rows(|_tree, path| {
                if let Some(s) = path.to_str() {
                    paths.push(s.to_string());
                }
            });
            conf_for_expand.borrow_mut().set("sidebar", "expanded_paths", &paths.join(";"));
            let _ = conf_for_expand.borrow_mut().save();
        });

        let se = save_expand.clone();
        sidebar_tree.connect_row_expanded(move |_, _, _| { se(); });
        let se = save_expand.clone();
        sidebar_tree.connect_row_collapsed(move |_, _, _| { se(); });
    }

    // ── Sidebar right-click context menu ──
    {
        let menu = gio::Menu::new();
        menu.append(Some("Open"), Some("app.open-mailbox-ctx"));
        menu.append_section(None, &{
            let s = gio::Menu::new();
            s.append(Some("New Mailbox"), Some("app.ctx-new-mailbox"));
            s.append(Some("New Folder"), Some("app.ctx-new-folder"));
            s
        });
        menu.append_section(None, &{
            let s = gio::Menu::new();
            s.append(Some("Rename"), Some("app.ctx-rename"));
            s.append(Some("Delete"), Some("app.ctx-delete"));
            s.append(Some("Compact"), Some("app.ctx-compact"));
            s
        });

        let popover = gtk4::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(&sidebar_tree);
        popover.set_has_arrow(false);

        // Right-click gesture — select the row under cursor then show menu
        let gesture = gtk4::GestureClick::new();
        gesture.set_button(3);
        let stree = sidebar_tree.clone();
        let pop = popover.clone();
        gesture.connect_pressed(move |_g, _n, x, y| {
            // Select the row at click position
            if let Some((Some(path), _, _, _)) = stree.path_at_pos(x as i32, y as i32) {
                stree.selection().select_path(&path);
            }
            let rect = gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            pop.set_pointing_to(Some(&rect));
            pop.popup();
        });
        sidebar_tree.add_controller(gesture);

        // Register context menu actions
        // Open
        {
            let a = gio::SimpleAction::new("open-mailbox-ctx", None);
            let stree = sidebar_tree.clone();
            let open = open_mailbox_tab.clone();
            a.connect_activate(move |_, _| {
                if let Some((model, iter)) = stree.selection().selected() {
                    let path: String = model.get(&iter, sidebar::COL_PATH);
                    let name: String = model.get(&iter, sidebar::COL_NAME);
                    let is_dir: bool = model.get(&iter, sidebar::COL_IS_DIR);
                    if !is_dir && !path.is_empty() {
                        open(&path, &name);
                    }
                }
            });
            app.add_action(&a);
        }
        // New Mailbox
        {
            let a = gio::SimpleAction::new("ctx-new-mailbox", None);
            let st = state.clone();
            let stree = sidebar_tree.clone();
            a.connect_activate(move |_, _| {
                let s = st.borrow();
                let mb_dir = s.data_dir.join("mailboxes");
                let mut name = "New Mailbox".to_string();
                let mut i = 1;
                while mb_dir.join(&name).exists() { i += 1; name = format!("New Mailbox {}", i); }
                if std::fs::File::create(mb_dir.join(&name)).is_ok() {
                    let _ = macmbx::toc::Toc::open_or_create(&mb_dir.join(&name));
                    if let Ok(ns) = macmbx::store::Store::open(&mb_dir) {
                        let new_tree = sidebar::build_sidebar(&ns);
                        stree.set_model(Some(&new_tree.model().unwrap()));
                    }
                }
            });
            app.add_action(&a);
        }
        // New Folder
        {
            let a = gio::SimpleAction::new("ctx-new-folder", None);
            let st = state.clone();
            let stree = sidebar_tree.clone();
            a.connect_activate(move |_, _| {
                let s = st.borrow();
                let mb_dir = s.data_dir.join("mailboxes");
                let mut name = "New Folder".to_string();
                let mut i = 1;
                while mb_dir.join(&name).exists() { i += 1; name = format!("New Folder {}", i); }
                let _ = std::fs::create_dir_all(mb_dir.join(&name));
                if let Ok(ns) = macmbx::store::Store::open(&mb_dir) {
                    let new_tree = sidebar::build_sidebar(&ns);
                    stree.set_model(Some(&new_tree.model().unwrap()));
                }
            });
            app.add_action(&a);
        }
        // Rename
        {
            let a = gio::SimpleAction::new("ctx-rename", None);
            let st = state.clone();
            let stree = sidebar_tree.clone();
            a.connect_activate(move |_, _| {
                if let Some((model, iter)) = stree.selection().selected() {
                    let path: String = model.get(&iter, sidebar::COL_PATH);
                    let name: String = model.get(&iter, sidebar::COL_NAME);
                    if path.is_empty() || ["In","Out","Trash","Junk"].contains(&name.as_str()) { return; }
                    let old = std::path::PathBuf::from(&path);
                    if let Some(parent) = old.parent() {
                        let new_name = format!("{}_renamed", name);
                        let _ = std::fs::rename(&old, parent.join(&new_name));
                        // Also rename .toc
                        let old_toc = old.with_extension("toc");
                        if old_toc.exists() {
                            let _ = std::fs::rename(&old_toc, parent.join(&new_name).with_extension("toc"));
                        }
                        let s = st.borrow();
                        let mb_dir = s.data_dir.join("mailboxes");
                        if let Ok(ns) = macmbx::store::Store::open(&mb_dir) {
                            let new_tree = sidebar::build_sidebar(&ns);
                            stree.set_model(Some(&new_tree.model().unwrap()));
                        }
                    }
                }
            });
            app.add_action(&a);
        }
        // Delete
        {
            let a = gio::SimpleAction::new("ctx-delete", None);
            let st = state.clone();
            let stree = sidebar_tree.clone();
            a.connect_activate(move |_, _| {
                if let Some((model, iter)) = stree.selection().selected() {
                    let path: String = model.get(&iter, sidebar::COL_PATH);
                    let name: String = model.get(&iter, sidebar::COL_NAME);
                    let is_dir: bool = model.get(&iter, sidebar::COL_IS_DIR);
                    if path.is_empty() || ["In","Out","Trash","Junk"].contains(&name.as_str()) { return; }
                    let p = std::path::PathBuf::from(&path);
                    if is_dir { let _ = std::fs::remove_dir_all(&p); }
                    else {
                        let _ = std::fs::remove_file(&p);
                        let _ = std::fs::remove_file(p.with_extension("toc"));
                    }
                    let s = st.borrow();
                    let mb_dir = s.data_dir.join("mailboxes");
                    if let Ok(ns) = macmbx::store::Store::open(&mb_dir) {
                        let new_tree = sidebar::build_sidebar(&ns);
                        stree.set_model(Some(&new_tree.model().unwrap()));
                    }
                }
            });
            app.add_action(&a);
        }
        // Compact
        {
            let a = gio::SimpleAction::new("ctx-compact", None);
            let stree = sidebar_tree.clone();
            a.connect_activate(move |_, _| {
                if let Some((model, iter)) = stree.selection().selected() {
                    let path: String = model.get(&iter, sidebar::COL_PATH);
                    let is_dir: bool = model.get(&iter, sidebar::COL_IS_DIR);
                    if path.is_empty() || is_dir { return; }
                    if let Ok(mut toc) = macmbx::toc::Toc::open(&path) {
                        let removed = macmbx::compact::compact(&mut toc).unwrap_or(0);
                        eprintln!("Compacted: removed {} deleted message(s)", removed);
                    }
                }
            });
            app.add_action(&a);
        }
    }

    // ── Sidebar double-click → open mailbox tab ──
    {
        let open = open_mailbox_tab.clone();
        sidebar_tree.connect_row_activated(move |tree, path, _col| {
            let model = tree.model().unwrap();
            let iter = model.iter(path).unwrap();
            let mbox_path: String = model.get(&iter, sidebar::COL_PATH);
            let name: String = model.get(&iter, sidebar::COL_NAME);
            let is_dir: bool = model.get(&iter, sidebar::COL_IS_DIR);

            if is_dir {
                if tree.row_expanded(path) {
                    tree.collapse_row(path);
                } else {
                    tree.expand_row(path, false);
                }
                return;
            }

            open(&mbox_path, &name);
        });
    }

    window.present();

    // ── Start the mail scheduler (port of IdleSchedulerStart) ──
    let check_interval = state.borrow().conf.borrow()
        .get_int("checking_mail", "check_interval", 15) as i32;
    // Initialize link history
    linkhistory::init(&state.borrow().data_dir);

    // Refresh callback — reload TOC data in all open mailbox tabs
    let nb_refresh = content_notebook.clone();
    let state_refresh = state.clone();
    let refresh_fn: Rc<dyn Fn()> = Rc::new(move || {
        // Re-read TOC for each open mailbox tab and update the list store
        // For now, just signal that data changed — the simplest approach
        // is to trigger the sidebar selection again
        eprintln!("rMail: refreshing mailbox views after check");
        // Walk notebook pages — each mailbox tab has a widget_name set to the path
        let n = nb_refresh.n_pages();
        for i in 0..n {
            if let Some(page) = nb_refresh.nth_page(Some(i)) {
                // Queue a draw to refresh display
                page.queue_draw();
            }
        }
    });

    scheduler::start(
        state.borrow().mailer.clone(),
        status_label.clone(),
        check_interval,
        Some(refresh_fn),
    );
}




/// Create the welcome dashboard — matches main_eudora.c:2179-2358.
/// Welcome page CSS — port of theme.c welcome/stat/action classes.
const WELCOME_CSS: &str = "
.welcome-bg { }
.welcome-hero {
    background: linear-gradient(135deg, #1e3a5f 0%, #2d5f8a 100%);
    padding: 28px 36px 24px; border-radius: 0 0 16px 16px;
}
.welcome-greeting { font-size: 1.6em; font-weight: 700; color: #ffffff; }
.welcome-account-line { font-size: 0.88em; color: rgba(255,255,255,0.7); margin-top: 2px; }

.stat-card {
    background: @theme_bg_color; border-radius: 10px; padding: 14px 18px;
    border: 1px solid alpha(@borders, 0.5);
}
.stat-number { font-size: 1.7em; font-weight: 800; }
.stat-label { font-size: 0.80em; color: alpha(@theme_fg_color, 0.55); font-weight: 500; }
.stat-accent-blue  .stat-number { color: #3182ce; }
.stat-accent-green .stat-number { color: #2f855a; }
.stat-accent-amber .stat-number { color: #dd6b20; }
.stat-accent-red   .stat-number { color: #e53e3e; }

.wc-section-title {
    font-size: 0.72em; font-weight: 700; color: alpha(@theme_fg_color, 0.45);
    letter-spacing: 0.1em;
}

.action-card {
    background: @theme_bg_color; border-radius: 10px; padding: 12px 16px;
    border: 1px solid alpha(@borders, 0.5);
}
.action-card:hover { background: alpha(@theme_fg_color, 0.04); border-color: #3182ce; }
.action-icon {
    border-radius: 8px; padding: 7px;
    background: alpha(#3182ce, 0.12);
}
.action-icon-green  { background: alpha(#2f855a, 0.12); }
.action-icon-amber  { background: alpha(#dd6b20, 0.12); }
.action-icon-purple { background: alpha(#805ad5, 0.12); }
.action-icon-rose   { background: alpha(#e53e3e, 0.12); }
.action-icon-cyan   { background: alpha(#0694a2, 0.12); }
.action-title { font-size: 0.92em; font-weight: 600; }
.action-desc { font-size: 0.80em; color: alpha(@theme_fg_color, 0.55); }

.tip-bar {
    background: alpha(#dd6b20, 0.10); border-radius: 10px;
    padding: 12px 18px; border: 1px solid alpha(#dd6b20, 0.25);
}
.tip-text { font-size: 0.85em; color: alpha(@theme_fg_color, 0.65); }
";

fn install_welcome_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(WELCOME_CSS);
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

/// Stat pill — clickable card with big number + label (port of main_eudora.c stat_pill)
fn stat_pill(number: &str, label: &str, accent_class: &str, mailbox_name: &str) -> gtk4::Widget {
    let btn = gtk4::Button::new();
    btn.add_css_class("flat");
    btn.add_css_class("stat-card");
    btn.add_css_class(accent_class);
    btn.set_hexpand(true);

    let card = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let num = gtk4::Label::new(Some(number));
    num.add_css_class("stat-number");
    num.set_halign(gtk4::Align::Start);
    card.append(&num);

    let lbl = gtk4::Label::new(Some(label));
    lbl.add_css_class("stat-label");
    lbl.set_halign(gtk4::Align::Start);
    card.append(&lbl);

    btn.set_child(Some(&card));

    // Click opens the mailbox — we need to use the "open-mailbox" action
    if !mailbox_name.is_empty() {
        let mb = mailbox_name.to_string();
        btn.connect_clicked(move |b| {
            if let Some(root) = b.root() {
                if let Some(win) = root.downcast_ref::<gtk4::ApplicationWindow>() {
                    if let Some(app) = win.application() {
                        app.activate_action("open-mailbox",
                            Some(&mb.to_variant()));
                    }
                }
            }
        });
    }

    btn.upcast()
}

/// Section header (port of main_eudora.c wc_section)
fn wc_section(text: &str) -> gtk4::Widget {
    let lbl = gtk4::Label::new(Some(text));
    lbl.add_css_class("wc-section-title");
    lbl.set_halign(gtk4::Align::Start);
    lbl.set_margin_top(20);
    lbl.set_margin_bottom(8);
    lbl.upcast()
}

/// Action card — icon in colored circle + title + desc + arrow (port of main_eudora.c action_card)
fn action_card(icon: &str, icon_color: &str, title: &str, desc: &str, action_name: &str) -> gtk4::Widget {
    let btn = gtk4::Button::new();
    btn.add_css_class("flat");
    btn.add_css_class("action-card");
    btn.set_hexpand(true);

    let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 14);

    // Icon in colored circle
    let icon_frame = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    icon_frame.add_css_class("action-icon");
    if !icon_color.is_empty() {
        icon_frame.add_css_class(icon_color);
    }
    icon_frame.set_valign(gtk4::Align::Center);
    let img = gtk4::Image::from_icon_name(icon);
    img.set_pixel_size(22);
    icon_frame.append(&img);
    hbox.append(&icon_frame);

    // Text
    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    vbox.set_hexpand(true);

    let t = gtk4::Label::new(Some(title));
    t.add_css_class("action-title");
    t.set_halign(gtk4::Align::Start);
    vbox.append(&t);

    let d = gtk4::Label::new(Some(desc));
    d.add_css_class("action-desc");
    d.set_halign(gtk4::Align::Start);
    d.set_wrap(true);
    vbox.append(&d);

    hbox.append(&vbox);

    // Arrow indicator
    let arrow = gtk4::Image::from_icon_name("go-next-symbolic");
    arrow.set_pixel_size(16);
    arrow.set_opacity(0.3);
    arrow.set_valign(gtk4::Align::Center);
    hbox.append(&arrow);

    btn.set_child(Some(&hbox));

    // Wire click → activate the named action on the app
    let act = action_name.trim_start_matches("app.").to_string();
    btn.connect_clicked(move |b| {
        if let Some(root) = b.root() {
            if let Some(win) = root.downcast_ref::<gtk4::ApplicationWindow>() {
                if let Some(app) = win.application() {
                    app.activate_action(&act, None);
                }
            }
        }
    });

    btn.upcast()
}

/// Welcome dashboard — port of main_eudora.c create_welcome_page
fn create_welcome_page(state: &AppState) -> gtk4::Widget {
    install_welcome_css();

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .build();

    let bg = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    bg.add_css_class("welcome-bg");
    bg.set_hexpand(true);

    // ── Hero banner with gradient ──
    let hero = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    hero.add_css_class("welcome-hero");

    // Greeting based on time of day
    let conf = state.conf.borrow();
    let real_name = conf.get("sending_mail", "real_name").unwrap_or_default();
    let first_name = real_name.split_whitespace().next().unwrap_or("there");
    let hour = {
        let mut t: libc::tm = unsafe { std::mem::zeroed() };
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        unsafe { libc::localtime_r(&now, &mut t); }
        t.tm_hour
    };
    let tod = if hour < 12 { "Good morning" }
              else if hour < 17 { "Good afternoon" }
              else { "Good evening" };

    let greet_lbl = gtk4::Label::new(Some(&format!("{}, {}", tod, first_name)));
    greet_lbl.add_css_class("welcome-greeting");
    greet_lbl.set_halign(gtk4::Align::Start);
    hero.append(&greet_lbl);

    // Account line
    let email = conf.get("sending_mail", "email_address").unwrap_or_default();
    let server = conf.get("checking_mail", "pop_server").unwrap_or_default();
    let use_imap = conf.get_bool("checking_mail", "use_imap", false);
    let use_ssl = conf.get_bool("ssl", "use_ssl", false);
    let acct_text = format!("{}  {}  {}  {}",
        if email.is_empty() { "No email configured" } else { email },
        if use_imap { "IMAP" } else { "POP3" },
        server,
        if use_ssl { "SSL" } else { "" });

    let acct_lbl = gtk4::Label::new(Some(&acct_text));
    acct_lbl.add_css_class("welcome-account-line");
    acct_lbl.set_halign(gtk4::Align::Start);
    hero.append(&acct_lbl);
    bg.append(&hero);

    // ── Dashboard body ──
    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    body.set_margin_start(32);
    body.set_margin_end(32);
    body.set_margin_top(20);
    body.set_margin_bottom(32);

    // ── Stats row: 4 metric cards with accent colors ──
    let mailboxes_dir = state.data_dir.join("mailboxes");
    let stats = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    for (name, label, cls) in &[("In", "Inbox", "stat-accent-blue"),
                                 ("Out", "Outbox", "stat-accent-green"),
                                 ("Junk", "Junk", "stat-accent-amber"),
                                 ("Trash", "Trash", "stat-accent-red")] {
        let path = mailboxes_dir.join(name);
        let n = macmbx::toc::Toc::open(&path).map(|t| t.msgs.len()).unwrap_or(0);
        stats.append(&stat_pill(&n.to_string(), label, cls, name));
    }
    body.append(&stats);

    // ── Two-column grid: actions left, tools right ──
    let columns = gtk4::Box::new(gtk4::Orientation::Horizontal, 20);
    columns.set_margin_top(4);

    // Left column — primary actions
    let col_left = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    col_left.set_hexpand(true);
    col_left.append(&wc_section("ACTIONS"));

    let al = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    al.append(&action_card("mail-unread-symbolic", "",
        "Check Mail", "Fetch new messages  ⌘M", "app.check-mail"));
    al.append(&action_card("document-new-symbolic", "action-icon-green",
        "New Message", "Compose a new email  ⌘N", "app.new-message"));
    al.append(&action_card("mail-send-symbolic", "action-icon-amber",
        "Send Queued", "Deliver queued messages  ⌘T", "app.send-queued"));
    al.append(&action_card("mail-reply-sender-symbolic", "action-icon-purple",
        "Reply", "Reply to the selected message  ⌘R", "app.reply"));
    al.append(&action_card("mail-forward-symbolic", "action-icon-cyan",
        "Forward", "Forward the selected message", "app.forward"));
    col_left.append(&al);
    columns.append(&col_left);

    // Right column — tools & manage
    let col_right = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    col_right.set_hexpand(true);
    col_right.append(&wc_section("MANAGE"));

    let ar = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    ar.append(&action_card("x-office-address-book-symbolic", "",
        "Address Book", "Manage contacts and nicknames", "app.address-book"));
    ar.append(&action_card("edit-find-symbolic", "action-icon-amber",
        "Filters", "Automatic mail sorting rules", "app.filters"));
    ar.append(&action_card("preferences-system-symbolic", "action-icon-purple",
        "Settings", "Accounts, display, and behavior  ⌘,", "app.preferences"));
    ar.append(&action_card("avatar-default-symbolic", "action-icon-rose",
        "Personalities", "Manage email identities", "app.personalities"));
    ar.append(&action_card("utilities-system-monitor-symbolic", "action-icon-cyan",
        "Statistics", "View mail usage statistics", "app.statistics"));
    col_right.append(&ar);
    columns.append(&col_right);

    body.append(&columns);

    // ── Tip bar ──
    let tip = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    tip.add_css_class("tip-bar");
    tip.set_margin_top(16);

    let tip_icon = gtk4::Image::from_icon_name("dialog-information-symbolic");
    tip_icon.set_pixel_size(18);
    tip_icon.set_valign(gtk4::Align::Center);
    tip.append(&tip_icon);

    let tip_text = gtk4::Label::new(Some(
        "Tip: Double-click any mailbox in the sidebar to open it in a new tab. \
         Drag tabs to reorder or tear them off into separate windows."));
    tip_text.add_css_class("tip-text");
    tip_text.set_wrap(true);
    tip_text.set_hexpand(true);
    tip.append(&tip_text);

    body.append(&tip);
    bg.append(&body);

    scroll.set_child(Some(&bg));
    scroll.upcast()
}

/// Build a mailbox tab showing real messages from TOC.
/// Double-clicking a message opens it in a new tab in the content notebook.
/// Get the currently selected message from the active mailbox tab.
/// Returns (mbox_path, msg_index, tree, iter).
fn get_selected_msg(nb: &gtk4::Notebook) -> Option<(String, i32, gtk4::TreeView, gtk4::TreeIter)> {
    let page_idx = nb.current_page()?;
    let page = nb.nth_page(Some(page_idx))?;
    let paned = page.downcast_ref::<gtk4::Paned>()?;
    let mbox_path = paned.widget_name().to_string();
    if mbox_path.is_empty() { return None; }
    let scroll = paned.start_child()?.downcast::<gtk4::ScrolledWindow>().ok()?;
    let tree = scroll.child()?.downcast::<gtk4::TreeView>().ok()?;
    let sel = tree.selection();
    // Works with both Single and Multiple selection modes
    let (rows, model) = sel.selected_rows();
    if rows.len() != 1 { return None; } // only act on single selection
    let iter = model.iter(&rows[0])?;
    let idx: i32 = model.get(&iter, 0);
    Some((mbox_path, idx, tree, iter))
}

/// Open a compose tab with the given page widget and title.
fn open_compose_tab(nb: &gtk4::Notebook, page: &gtk4::Widget, title: &str) {
    let tab = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    tab.append(&gtk4::Label::new(Some(title)));
    let close = gtk4::Button::from_icon_name("window-close-symbolic");
    close.set_has_frame(false);
    tab.append(&close);
    let idx = nb.append_page(page, Some(&tab));
    nb.set_tab_reorderable(page, true);
    nb.set_current_page(Some(idx));
    let nb2 = nb.clone();
    let p2 = page.clone();
    close.connect_clicked(move |_| {
        if let Some(pn) = nb2.page_num(&p2) { nb2.remove_page(Some(pn)); }
    });
}

/// Walk up from a widget to find the parent GtkNotebook.
fn find_notebook(widget: &impl IsA<gtk4::Widget>) -> Option<gtk4::Notebook> {
    let mut w: Option<gtk4::Widget> = Some(widget.clone().upcast());
    while let Some(ref curr) = w {
        if let Some(parent) = curr.parent() {
            if let Some(nb) = parent.downcast_ref::<gtk4::Notebook>() {
                return Some(nb.clone());
            }
            w = Some(parent);
        } else { break; }
    }
    None
}

fn build_mailbox_tab(
    toc: &macmbx::toc::Toc,
    mbox_path: &str,
    content_nb: &gtk4::Notebook,
    conf: Rc<RefCell<macmbx::conf::Conf>>,
    mailer: std::sync::Arc<macmbx::mailer::Mailer>,
    books: Option<Rc<macmbx::nicknames::AddressBooks>>,
) -> gtk4::Widget {
    let vpaned = gtk4::Paned::new(gtk4::Orientation::Vertical);
    vpaned.set_position(300);
    vpaned.set_vexpand(true);
    // Store mbox path on the widget so actions can find it
    vpaned.set_widget_name(mbox_path);
    vpaned.set_hexpand(true);

    // ── Message list ──
    let store = gtk4::ListStore::new(&[
        glib::Type::I32,     // 0: index
        glib::Type::STRING,  // 1: status icon
        glib::Type::STRING,  // 2: priority
        glib::Type::STRING,  // 3: from
        glib::Type::STRING,  // 4: date
        glib::Type::STRING,  // 5: size
        glib::Type::STRING,  // 6: subject
        glib::Type::I32,     // 7: font weight (bold for unread)
        glib::Type::STRING,  // 8: label color (e.g. "#FF0000", empty = none)
        glib::Type::STRING,  // 9: priority color
    ]);

    // Label colors — read from conf settings, fallback to Eudora defaults
    // (port of mailbox.c label_color_str which reads from prefs)
    let default_colors = ["", "#FF0000", "#0000FF", "#009900", "#990099",
                           "#FF8000", "#009999", "#666666"];
    let label_colors: Vec<String> = (0..8).map(|i| {
        if i == 0 { return String::new(); }
        let key = format!("color_{}", i - 1);
        if let Some(val) = conf.borrow().get("labels", &key) {
            // Stored as "r,g,b" (0.0-1.0) — convert to hex
            let parts: Vec<f64> = val.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if parts.len() >= 3 {
                return format!("#{:02X}{:02X}{:02X}",
                    (parts[0] * 255.0) as u8, (parts[1] * 255.0) as u8, (parts[2] * 255.0) as u8);
            }
        }
        default_colors[i].to_string()
    }).collect();
    let label_colors_ref: Vec<&str> = label_colors.iter().map(|s| s.as_str()).collect();

    // Populate from real TOC data
    for (i, msg) in toc.msgs.iter().enumerate() {
        // Status icon — port of mailbox.c state_str()
        let state_icon = match msg.state {
            macmbx::toc::State::Unread => "●",       // filled circle
            macmbx::toc::State::Read => "",
            macmbx::toc::State::Replied => "↩",
            macmbx::toc::State::Forwarded => "→",
            macmbx::toc::State::Redirected => "⇉",
            macmbx::toc::State::Unsendable => "✏",
            macmbx::toc::State::Sendable => "✓",
            macmbx::toc::State::Queued => "⏳",
            macmbx::toc::State::Sent => "✉",
            macmbx::toc::State::Unsent => "✏",
            macmbx::toc::State::Timed => "⏰",
            macmbx::toc::State::BusySending => "⇧",
            macmbx::toc::State::MesgErr => "⚠",
            _ => "",
        };

        // Priority — port of mailbox.c priority_str()
        let pri = match msg.priority {
            1 => "↑↑",  // Highest
            2 => "↑",   // High
            3 => "",     // Normal
            4 => "↓",   // Low
            5 => "↓↓",  // Lowest
            _ => "",
        };

        // Priority color — red for high, blue for low
        let pri_color = match msg.priority {
            1 => "#e53e3e",
            2 => "#dd6b20",
            4 => "#3182ce",
            5 => "#2f855a",
            _ => "",
        };

        // Label color from flags (using macmbx MsgFlags::label())
        let label = msg.flags.label() as usize;
        let label_color = if label > 0 && label < label_colors_ref.len() {
            label_colors_ref[label]
        } else { "" };

        // Format date from epoch seconds
        let date_str = if msg.seconds > 0 {
            format_epoch(msg.seconds as i64)
        } else {
            String::new()
        };

        // Format size
        let size_str = if msg.length > 1024 {
            format!("{}K", msg.length / 1024)
        } else {
            format!("{}", msg.length)
        };

        let weight = if msg.state == macmbx::toc::State::Unread { 700 } else { 400 };

        let iter = store.append();
        store.set(&iter, &[
            (0, &(i as i32)),
            (1, &state_icon),
            (2, &pri),
            (3, &msg.from),
            (4, &date_str),
            (5, &size_str),
            (6, &msg.subject),
            (7, &weight),
            (8, &label_color),
            (9, &pri_color),
        ]);
    }

    let tree = gtk4::TreeView::with_model(&store);
    tree.set_headers_visible(true);
    tree.selection().set_mode(gtk4::SelectionMode::Multiple);
    tree.set_rubber_banding(true);

    // Columns
    // Status column — colored ● for unread
    {
        let r = gtk4::CellRendererText::new();
        let col = gtk4::TreeViewColumn::new();
        col.set_title("");
        col.pack_start(&r, true);
        col.add_attribute(&r, "text", 1);
        col.set_fixed_width(28);
        col.set_sort_column_id(1);
        // Color the status icon blue for unread
        col.set_cell_data_func(&r,|_col: &gtk4::TreeViewColumn, cell: &gtk4::CellRenderer, model: &gtk4::TreeModel, iter: &gtk4::TreeIter| {
            let text: String = model.get(iter, 1);
            if let Some(cr) = cell.downcast_ref::<gtk4::CellRendererText>() {
                if text == "●" {
                    cr.set_foreground(Some("#2563eb"));
                    cr.set_property("foreground-set", true);
                } else {
                    cr.set_property("foreground-set", false);
                }
            }
        });
        tree.append_column(&col);
    }

    // Priority column — colored arrows
    {
        let r = gtk4::CellRendererText::new();
        let col = gtk4::TreeViewColumn::new();
        col.set_title("!");
        col.pack_start(&r, true);
        col.add_attribute(&r, "text", 2);
        col.set_fixed_width(28);
        col.set_sort_column_id(2);
        // Apply priority color from column 9
        col.set_cell_data_func(&r,|_col: &gtk4::TreeViewColumn, cell: &gtk4::CellRenderer, model: &gtk4::TreeModel, iter: &gtk4::TreeIter| {
            let color: String = model.get(iter, 9);
            if let Some(cr) = cell.downcast_ref::<gtk4::CellRendererText>() {
                if !color.is_empty() {
                    cr.set_foreground(Some(&color));
                    cr.set_property("foreground-set", true);
                } else {
                    cr.set_property("foreground-set", false);
                }
            }
        });
        tree.append_column(&col);
    }

    // From, Date, Size, Subject — apply label color from column 8
    for &(title, col_idx, width) in &[
        ("From", 3i32, 180i32),
        ("Date", 4, 100),
        ("Size", 5, 60),
        ("Subject", 6, -1),
    ] {
        let renderer = gtk4::CellRendererText::new();
        let col = gtk4::TreeViewColumn::new();
        col.set_title(title);
        col.pack_start(&renderer, true);
        col.add_attribute(&renderer, "text", col_idx);
        col.add_attribute(&renderer, "weight", 7);
        col.set_resizable(true);
        if width > 0 {
            col.set_fixed_width(width);
        } else {
            col.set_expand(true);
        }
        col.set_sort_column_id(col_idx);
        // Apply label color (port of label_color_cell_func)
        col.set_cell_data_func(&renderer,|_col: &gtk4::TreeViewColumn, cell: &gtk4::CellRenderer, model: &gtk4::TreeModel, iter: &gtk4::TreeIter| {
            let color: String = model.get(iter, 8);
            if let Some(cr) = cell.downcast_ref::<gtk4::CellRendererText>() {
                if !color.is_empty() {
                    cr.set_foreground(Some(&color));
                    cr.set_property("foreground-set", true);
                } else {
                    cr.set_property("foreground-set", false);
                }
            }
        });
        tree.append_column(&col);
    }

    // ── Right-click context menu on message list ──
    {
        let gesture = gtk4::GestureClick::new();
        gesture.set_button(3); // right click only
        let tree_for_ctx = tree.clone();
        gesture.connect_pressed(move |_g, _n, x, y| {
            let menu = gio::Menu::new();
            menu.append(Some("Reply"), Some("app.reply"));
            menu.append(Some("Reply All"), Some("app.reply-all"));
            menu.append(Some("Forward"), Some("app.forward"));
            menu.append(Some("Redirect"), Some("app.redirect"));
            menu.append_section(None, &{
                let s = gio::Menu::new();
                s.append(Some("Delete"), Some("app.delete"));
                s
            });
            let transfer = gio::Menu::new();
            transfer.append(Some("→ In"), Some("app.transfer-in"));
            transfer.append(Some("→ Out"), Some("app.transfer-out"));
            transfer.append(Some("→ Trash"), Some("app.transfer-trash"));
            menu.append_submenu(Some("Transfer"), &transfer);

            let popover = gtk4::PopoverMenu::from_model(Some(&menu));
            popover.set_parent(&tree_for_ctx);
            popover.set_has_arrow(false);
            let rect = gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&rect));
            popover.popup();
        });
        tree.add_controller(gesture);
    }

    let scroll = gtk4::ScrolledWindow::builder()
        .child(&tree)
        .vexpand(true)
        .build();
    vpaned.set_start_child(Some(&scroll));

    // ── Preview pane ──
    let preview_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    preview_box.set_vexpand(true);
    // Placeholder until a message is selected
    let placeholder = gtk4::Label::builder()
        .label("Select a message to preview")
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .build();
    placeholder.add_css_class("dim-label");
    preview_box.append(&placeholder);

    let preview_scroll = gtk4::ScrolledWindow::builder()
        .child(&preview_box)
        .vexpand(true)
        .build();
    vpaned.set_end_child(Some(&preview_scroll));

    // ── Selection → preview (port of main_eudora.c create_message_view) ──
    {
        let toc = Rc::new(macmbx::toc::Toc::open(mbox_path)
            .unwrap_or_else(|_| macmbx::toc::Toc::new(mbox_path)));
        let pbox = preview_box.clone();
        let conf = conf.clone();
        let sel = tree.selection();
        sel.connect_changed(move |sel| {
            // Only show preview when exactly one message is selected
            let (rows, model) = sel.selected_rows();
            if rows.len() != 1 { return; }
            let iter = match model.iter(&rows[0]) {
                Some(i) => i,
                None => return,
            };
            let idx: i32 = model.get(&iter, 0);

            // Auto-mark read after delay (port of boxact.c mark_read_on_preview)
            let mark_delay = conf.borrow()
                .get_int("mailbox_display", "mark_read_delay", 0);
            if mark_delay > 0 && (idx as usize) < toc.msgs.len()
                && toc.msgs[idx as usize].state == macmbx::toc::State::Unread
            {
                let mbox = toc.mbox_path.clone();
                let msg_idx = idx as usize;
                let model_ref = model.clone();
                let iter_ref = iter.clone();
                glib::timeout_add_local_once(
                    std::time::Duration::from_secs(mark_delay as u64),
                    move || {
                        if let Ok(mut t) = macmbx::toc::Toc::open(&mbox) {
                            if msg_idx < t.msgs.len() && t.msgs[msg_idx].state == macmbx::toc::State::Unread {
                                t.msgs[msg_idx].state = macmbx::toc::State::Read;
                                let _ = t.save();
                                // Update list visual
                                if let Ok(store) = model_ref.downcast::<gtk4::ListStore>() {
                                    store.set(&iter_ref, &[(7, &400i32), (1, &"")]);
                                }
                            }
                        }
                    },
                );
            }

            match macmbx::ops::read_message(&toc, idx as usize) {
                Ok(raw) => {
                    let text = String::from_utf8_lossy(&raw);
                    let view = create_message_view(&text, 8000);
                    // Replace preview content
                    while let Some(c) = pbox.first_child() { pbox.remove(&c); }
                    pbox.append(&view);
                }
                Err(e) => {
                    while let Some(c) = pbox.first_child() { pbox.remove(&c); }
                    let err = gtk4::Label::new(Some(&format!("Error: {}", e)));
                    err.add_css_class("dim-label");
                    pbox.append(&err);
                }
            }
        });
    }

    // ── Double-click message → open in new tab ──
    // Outgoing/draft messages open in compose mode (port of main_eudora.c:576-583)
    {
        let toc2 = Rc::new(macmbx::toc::Toc::open(mbox_path)
            .unwrap_or_else(|_| macmbx::toc::Toc::new(mbox_path)));
        let nb = content_nb.clone();
        let conf_rc = conf.clone();
        let mailer_rc = mailer.clone();
        let books_rc = books.clone();
        let is_outbox = toc.which == macmbx::toc::MailboxType::Out
            || toc.which == macmbx::toc::MailboxType::OutTemp
            || mbox_path.ends_with("/Out")
            || mbox_path.ends_with("/Out.toc");
        tree.connect_row_activated(move |tv, path, _col| {
            let model = tv.model().unwrap();
            let iter = model.iter(path).unwrap();
            let idx: i32 = model.get(&iter, 0);
            let subject: String = model.get(&iter, 6);

            // Check if this is an outgoing message
            let msg_state = if (idx as usize) < toc2.msgs.len() {
                toc2.msgs[idx as usize].state
            } else {
                macmbx::toc::State::Read
            };
            let is_outgoing = is_outbox
                || msg_state == macmbx::toc::State::Unsendable
                || msg_state == macmbx::toc::State::Sendable
                || msg_state == macmbx::toc::State::Queued
                || msg_state == macmbx::toc::State::Unsent
                || msg_state == macmbx::toc::State::Timed
                || msg_state == macmbx::toc::State::Sent
                || msg_state == macmbx::toc::State::BusySending;

            let page: gtk4::Widget = if is_outgoing {
                // Open in compose mode with pre-filled headers and body
                let raw = macmbx::ops::read_message(&toc2, idx as usize)
                    .map(|r| String::from_utf8_lossy(&r).to_string())
                    .unwrap_or_default();
                compose::build_compose_tab_with_message(&conf_rc.borrow(), &raw, mailer_rc.clone(), books_rc.clone())
            } else {
                // Open read-only message view with full toolbar
                let raw = match macmbx::ops::read_message(&toc2, idx as usize) {
                    Ok(r) => String::from_utf8_lossy(&r).to_string(),
                    Err(e) => format!("Error reading message: {}", e),
                };
                create_message_view_full(&raw, 0, Some(mailer_rc.clone()), Some(conf_rc.clone()), books_rc.clone())
            };

            // Tab with close button
            let tab_title = if subject.len() > 30 {
                format!("{}…", &subject[..27])
            } else {
                subject.clone()
            };
            let tab_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            tab_box.append(&gtk4::Label::new(Some(&tab_title)));
            let close_btn = gtk4::Button::from_icon_name("window-close-symbolic");
            close_btn.set_has_frame(false);
            tab_box.append(&close_btn);

            let page_idx = nb.append_page(&page, Some(&tab_box));
            nb.set_current_page(Some(page_idx));

            let nb2 = nb.clone();
            let pg = page.clone();
            close_btn.connect_clicked(move |_| {
                if let Some(pn) = nb2.page_num(&pg) { nb2.remove_page(Some(pn)); }
            });
        });
    }

    vpaned.upcast()
}

// ══════════════════════════════════════════════════════════════════════
// Theme engine — port of theme.c palettes + generate_css
// ══════════════════════════════════════════════════════════════════════

const THEME_COUNT: usize = 5;
const THEME_NAMES: [&str; THEME_COUNT] = ["Light", "Dark", "Nord", "Solarized", "Monokai"];

struct ThemePalette {
    bg: &'static str, surface: &'static str, surface2: &'static str, border: &'static str,
    text: &'static str, text2: &'static str, text3: &'static str,
    accent: &'static str, accent2: &'static str,
    hero_from: &'static str, hero_to: &'static str, hero_text: &'static str, hero_sub: &'static str,
    success: &'static str, warning: &'static str, danger: &'static str, info: &'static str, purple: &'static str,
    input_bg: &'static str, input_border: &'static str, input_text: &'static str,
    tab_bg: &'static str,
}

const PALETTES: [ThemePalette; THEME_COUNT] = [
    // Light
    ThemePalette {
        bg: "#f0f2f5", surface: "#ffffff", surface2: "#f8fafc", border: "#e2e6ec",
        text: "#1e293b", text2: "#475569", text3: "#94a3b8",
        accent: "#2563eb", accent2: "#1d4ed8",
        hero_from: "#1e3a5f", hero_to: "#3a7bb8", hero_text: "#ffffff", hero_sub: "rgba(255,255,255,0.7)",
        success: "#16a34a", warning: "#d97706", danger: "#dc2626", info: "#0891b2", purple: "#7c3aed",
        input_bg: "#ffffff", input_border: "#cbd5e1", input_text: "#1e293b",
        tab_bg: "#f1f5f9",
    },
    // Dark
    ThemePalette {
        bg: "#0f1419", surface: "#1a1f2e", surface2: "#242937", border: "#2d3548",
        text: "#e2e8f0", text2: "#94a3b8", text3: "#64748b",
        accent: "#60a5fa", accent2: "#3b82f6",
        hero_from: "#1a1f2e", hero_to: "#2d3a5c", hero_text: "#e2e8f0", hero_sub: "rgba(226,232,240,0.6)",
        success: "#4ade80", warning: "#fbbf24", danger: "#f87171", info: "#22d3ee", purple: "#a78bfa",
        input_bg: "#242937", input_border: "#3d4560", input_text: "#e2e8f0",
        tab_bg: "#1a1f2e",
    },
    // Nord
    ThemePalette {
        bg: "#2e3440", surface: "#3b4252", surface2: "#434c5e", border: "#4c566a",
        text: "#eceff4", text2: "#d8dee9", text3: "#a4b0c3",
        accent: "#88c0d0", accent2: "#5e81ac",
        hero_from: "#3b4252", hero_to: "#4c566a", hero_text: "#eceff4", hero_sub: "rgba(216,222,233,0.7)",
        success: "#a3be8c", warning: "#ebcb8b", danger: "#bf616a", info: "#88c0d0", purple: "#b48ead",
        input_bg: "#3b4252", input_border: "#4c566a", input_text: "#eceff4",
        tab_bg: "#3b4252",
    },
    // Solarized
    ThemePalette {
        bg: "#fdf6e3", surface: "#eee8d5", surface2: "#f5efdc", border: "#d3cbb7",
        text: "#073642", text2: "#586e75", text3: "#93a1a1",
        accent: "#268bd2", accent2: "#2aa198",
        hero_from: "#073642", hero_to: "#0a4f5c", hero_text: "#eee8d5", hero_sub: "rgba(238,232,213,0.75)",
        success: "#859900", warning: "#b58900", danger: "#dc322f", info: "#2aa198", purple: "#6c71c4",
        input_bg: "#eee8d5", input_border: "#c9c0a5", input_text: "#073642",
        tab_bg: "#eee8d5",
    },
    // Monokai
    ThemePalette {
        bg: "#1e1f1c", surface: "#272822", surface2: "#3e3d32", border: "#49483e",
        text: "#f8f8f2", text2: "#a6a68a", text3: "#75715e",
        accent: "#66d9ef", accent2: "#a6e22e",
        hero_from: "#272822", hero_to: "#3e3d32", hero_text: "#f8f8f2", hero_sub: "rgba(248,248,242,0.6)",
        success: "#a6e22e", warning: "#e6db74", danger: "#f92672", info: "#66d9ef", purple: "#ae81ff",
        input_bg: "#272822", input_border: "#49483e", input_text: "#f8f8f2",
        tab_bg: "#272822",
    },
];

/// Generate the full CSS for a theme palette — port of theme.c generate_css.
/// Covers ALL GTK widgets: window, labels, separators, buttons, entries,
/// textviews, notebooks, paned, scrollbars, frames, checks, dropdowns,
/// popovers, listbox, treeview, navigation sidebar, plus all panel classes.
fn generate_theme_css(p: &ThemePalette) -> String {
    format!(r#"
/* ── Global / Window chrome ── */
window, .main-bg {{ background: {bg}; color: {text}; }}
label {{ color: {text}; }}
separator {{ background: {border}; }}
image {{ color: {text2}; -gtk-icon-style: symbolic; }}

/* ── Notebook tabs ── */
notebook > header {{ background: {surface}; border-bottom: 1px solid {border}; }}
notebook > header > tabs > tab {{ background: {tab_bg}; color: {text2};
  border-radius: 6px 6px 0 0; padding: 4px 12px; margin: 0 1px; }}
notebook > header > tabs > tab:checked {{
  background: {surface}; color: {text}; font-weight: 600;
  border-bottom: 2px solid {accent}; }}

/* ── Paned and scrollbar ── */
paned > separator {{ background: {border}; min-width: 2px; min-height: 2px; }}
scrollbar trough {{ background: {bg}; }}
scrollbar slider {{ background: {border}; border-radius: 99px; min-width: 6px; }}
scrollbar slider:hover {{ background: {text3}; }}

/* ── Buttons ── */
button {{ background: {surface}; color: {text}; border: 1px solid {border};
  border-radius: 6px; padding: 4px 12px; }}
button:hover {{ background: {surface2}; border-color: {accent}; }}
button:active {{ background: {border}; }}
button image {{ color: {text2}; }}
button.flat, button.image-button {{
  background: transparent; border-color: transparent; }}
button.flat:hover, button.image-button:hover {{
  background: alpha({text}, 0.12); border-color: transparent; }}

/* Toolbar buttons */
.comp-toolbar button, .toolbar button {{
  background: transparent; border: none; border-radius: 6px;
  padding: 4px 6px; min-width: 28px; min-height: 28px; }}
.comp-toolbar button:hover, .toolbar button:hover {{
  background: alpha({text}, 0.15); }}
.comp-toolbar button:active, .toolbar button:active {{
  background: alpha({text}, 0.25); }}
.comp-toolbar button image, .toolbar button image {{ color: {text2}; }}

/* Suggested/destructive */
button.suggested-action {{
  background: {accent}; color: {hero_text}; border-color: {accent}; }}
button.suggested-action:hover {{ background: {accent2}; }}
button.destructive-action {{
  background: {danger}; color: {hero_text}; border-color: {danger}; }}

/* ── Entries, text views, spinbuttons ── */
entry {{ background: {input_bg}; color: {input_text}; border: 1px solid {input_border};
  border-radius: 6px; caret-color: {text}; }}
entry:focus {{ border-color: {accent}; outline-color: {accent}; }}
entry > text, entry text {{ color: {input_text}; }}
entry placeholder {{ color: {text3}; }}
textview, textview text {{ background: {input_bg}; color: {input_text}; }}
spinbutton {{ background: {input_bg}; color: {input_text}; border: 1px solid {input_border};
  border-radius: 6px; }}
spinbutton > text {{ color: {input_text}; }}
spinbutton > button {{ background: transparent; border: none; color: {text2}; }}

/* ── Frame, check, dropdown, popover ── */
frame > border {{ border-color: {border}; border-radius: 8px; }}
frame > label {{ color: {text2}; font-weight: 600; font-size: 0.88em; }}
checkbutton {{ color: {text}; }}
checkbutton indicator {{ border-color: {border}; background: {input_bg}; }}
checkbutton indicator:checked {{ background: {accent}; border-color: {accent}; }}
dropdown {{ background: {input_bg}; color: {text}; border: 1px solid {input_border};
  border-radius: 6px; }}
dropdown > button {{ background: transparent; color: {text}; border: none; }}
popover, popover.menu {{
  background: {surface}; color: {text}; border: 1px solid {border};
  border-radius: 8px; }}
popover modelbutton {{ color: {text}; }}
popover modelbutton:hover {{ background: {surface2}; }}
popover label {{ color: {text}; }}
popover listview {{ background: {surface}; color: {text}; }}
popover listview row {{ color: {text}; padding: 4px 8px; }}
popover listview row:selected {{ background: {accent}; color: #ffffff; }}
popover listview row:hover {{ background: {surface2}; }}
popover list row {{ color: {text}; }}
popover list row:selected {{ background: {accent}; color: #ffffff; }}
popover list row:hover {{ background: {surface2}; }}

/* ── Listbox rows ── */
list {{ background: {surface}; color: {text}; }}
list > row {{ color: {text}; border-bottom: 1px solid {border}; padding: 2px 0; }}
list > row:selected {{ background: alpha({accent}, 0.2); color: {text}; }}
list > row:selected label {{ color: {text}; }}
list > row:hover {{ background: {surface2}; }}
list > row label {{ color: {text}; }}
list > row image {{ color: {text2}; }}

/* ── Navigation sidebar (settings) ── */
.navigation-sidebar {{ background: {surface}; }}
.navigation-sidebar > row {{ color: {text}; background: {surface}; }}
.navigation-sidebar > row:selected {{
  background: alpha({accent}, 0.15); color: {accent}; }}
.navigation-sidebar > row:selected label {{ color: {accent}; }}
.navigation-sidebar > row:selected image {{ color: {accent}; }}
.navigation-sidebar > row:hover {{ background: {surface2}; }}
.navigation-sidebar > row label {{ color: {text}; }}
.navigation-sidebar > row image {{ color: {text2}; }}

/* ── Treeview (message list) ── */
treeview {{ background: {surface}; color: {text}; }}
treeview header button {{ background: {surface2}; color: {text2};
  border-bottom: 1px solid {border}; }}
treeview:hover {{ background: alpha({accent}, 0.1); }}

/* ── Welcome page ── */
.welcome-bg {{ background: {bg}; }}
.welcome-hero {{
  background: linear-gradient(135deg, {hero_from} 0%, {hero_to} 100%);
  padding: 28px 36px 24px; border-radius: 0 0 16px 16px;
}}
.welcome-greeting {{ font-size: 1.6em; font-weight: 700; color: {hero_text}; }}
.welcome-account-line {{ font-size: 0.88em; color: {hero_sub}; margin-top: 2px; }}
.stat-card {{
  background: {surface}; border-radius: 10px; padding: 14px 18px;
  border: 1px solid {border}; color: {text};
}}
.stat-number {{ font-size: 1.7em; font-weight: 800; }}
.stat-label {{ font-size: 0.80em; color: {text3}; font-weight: 500; }}
.stat-accent-blue  .stat-number {{ color: {info}; }}
.stat-accent-green .stat-number {{ color: {success}; }}
.stat-accent-amber .stat-number {{ color: {warning}; }}
.stat-accent-red   .stat-number {{ color: {danger}; }}
.wc-section-title {{ font-size: 0.72em; font-weight: 700; color: {text3}; letter-spacing: 0.1em; }}
.action-card {{
  background: {surface}; border-radius: 10px; padding: 12px 16px;
  border: 1px solid {border}; color: {text};
}}
.action-card:hover {{ background: {surface2}; border-color: {accent}; }}
.action-icon {{ border-radius: 8px; padding: 7px; background: alpha({accent}, 0.12); }}
.action-icon-green  {{ background: alpha({success}, 0.12); }}
.action-icon-amber  {{ background: alpha({warning}, 0.12); }}
.action-icon-purple {{ background: alpha({purple}, 0.12); }}
.action-icon-rose   {{ background: alpha({danger}, 0.12); }}
.action-icon-cyan   {{ background: alpha({info}, 0.12); }}
.action-title {{ font-size: 0.92em; font-weight: 600; color: {text}; }}
.action-desc {{ font-size: 0.80em; color: {text2}; }}
.tip-bar {{
  background: alpha({warning}, 0.10); border-radius: 10px;
  padding: 12px 18px; border: 1px solid alpha({warning}, 0.25);
}}
.tip-text {{ font-size: 0.85em; color: {text2}; }}

/* ── Message view ── */
.msg-header-box {{ padding: 8px 12px; background: {surface2}; }}
.msg-header-name {{ font-weight: bold; font-size: 0.88em; color: {text2}; min-width: 70px; }}
.msg-header-value {{ font-size: 0.88em; color: {text}; }}
.msg-header-subject .msg-header-value {{ font-weight: bold; font-size: 0.96em; color: {text}; }}
.msg-separator {{ min-height: 1px; background: {border}; margin: 4px 0; }}
.msg-body-view {{ padding: 8px 12px; background: {surface}; color: {text}; }}

/* ── Panel heroes (filters, contacts, personalities) ── */
.filt-hero, .ab-hero, .pers-hero {{
  background: linear-gradient(135deg, {hero_from} 0%, {hero_to} 100%);
  padding: 14px 20px; border-radius: 0 0 10px 10px;
}}
.filt-hero-title, .ab-hero-title, .pers-hero-title {{
  font-size: 1.1em; font-weight: 700; color: {hero_text}; }}
.filt-hero-sub, .ab-hero-sub, .pers-hero-sub {{
  font-size: 0.78em; color: {hero_sub}; }}
.filt-count-pill, .ab-count-pill, .pers-count-pill {{
  background: alpha({hero_text}, 0.20); color: {hero_text};
  border-radius: 12px; padding: 2px 10px; font-size: 0.78em; font-weight: 600;
}}
.filt-badge-in {{ background: {info}; color: {hero_text}; border-radius: 4px;
  padding: 1px 6px; font-size: 0.73em; font-weight: 700; }}
.filt-badge-out {{ background: {warning}; color: {hero_text}; border-radius: 4px;
  padding: 1px 6px; font-size: 0.73em; font-weight: 700; }}
.filt-badge-man {{ background: {purple}; color: {hero_text}; border-radius: 4px;
  padding: 1px 6px; font-size: 0.73em; font-weight: 700; }}

/* ── Compose toolbar/formatting bar ── */
.comp-toolbar button {{ min-height: 24px; min-width: 24px; padding: 2px 6px; }}
.comp-fmt-bar button {{ min-height: 22px; min-width: 22px; padding: 1px 4px; }}
.comp-header-grid label {{ font-size: 13px; color: {text2}; }}
.comp-header-grid entry {{ min-height: 28px; }}

/* ── Settings section title ── */
.ab-section-title {{
  font-weight: 700; font-size: 0.85em; color: {text3}; padding: 6px 8px 2px;
}}

/* ── Task panel ── */
.error {{ color: {danger}; }}
spinner {{ color: {accent}; }}

/* ── Fixed font toggle ── */
.monospace-font {{ font-family: monospace; font-size: 12pt; }}

/* ── dim-label, heading, boxed-list ── */
.dim-label {{ color: {text2}; }}
.heading {{ color: {text}; }}
.caption {{ color: {text2}; }}
.title-1 {{ color: {text}; }}
.title-2 {{ color: {text}; }}

/* ── Settings boxed-list-separate ── */
.boxed-list-separate {{ background: {surface}; border: 1px solid {border}; border-radius: 8px; }}
.boxed-list-separate > box {{ background: {surface}; }}
.boxed-list-separate > box > separator {{ background: {border}; }}
.boxed-list-separate > box label {{ color: {text}; }}

/* ── Boxed-list (accounts) ── */
.boxed-list {{ background: {surface}; border: 1px solid {border}; border-radius: 8px; }}
.boxed-list row {{ background: {surface}; color: {text}; }}
.boxed-list row label {{ color: {text}; }}

/* ── Headerbar ── */
headerbar {{ background: {surface}; color: {text}; border-bottom: 1px solid {border}; }}
headerbar label {{ color: {text}; }}
headerbar button {{ color: {text2}; }}

/* ── Scrolled windows, viewports ── */
scrolledwindow {{ background: {bg}; }}
viewport {{ background: {bg}; color: {text}; }}

/* ── Tooltip ── */
tooltip {{ background: {surface2}; color: {text}; border: 1px solid {border}; border-radius: 6px; }}
tooltip label {{ color: {text}; }}

/* ── Color buttons ── */
colorbutton {{ border: 1px solid {border}; border-radius: 4px; }}

/* ── Link buttons ── */
linkbutton {{ color: {accent}; }}
linkbutton:hover {{ color: {accent2}; }}

/* ── Toggle buttons (formatting bar) ── */
togglebutton {{ background: {surface}; color: {text}; border: 1px solid {border}; border-radius: 6px; }}
togglebutton:checked {{ background: alpha({accent}, 0.2); border-color: {accent}; color: {accent}; }}
togglebutton:hover {{ background: {surface2}; }}

/* ── Menu button (emoji etc) ── */
menubutton > button {{ background: transparent; border: none; }}
menubutton > button:hover {{ background: alpha({text}, 0.12); }}

/* ── Wazoo left panel tabs ── */
notebook > header.bottom {{ background: {surface2}; border-top: 1px solid {border}; }}
notebook > header.bottom > tabs > tab {{ background: {tab_bg}; color: {text2};
  border-radius: 0 0 6px 6px; padding: 4px 12px; margin: 0 1px; }}
notebook > header.bottom > tabs > tab:checked {{
  background: {surface}; color: {text}; font-weight: 600;
  border-top: 2px solid {accent}; }}

/* ── Search entry ── */
searchentry {{ background: {input_bg}; color: {input_text}; border: 1px solid {input_border}; border-radius: 6px; }}
searchentry:focus {{ border-color: {accent}; }}

/* ── Status bar area ── */
.status-bar {{ background: {surface}; border-top: 1px solid {border}; }}
.status-bar label {{ color: {text2}; }}

/* ── reditctrl gedit-view ── */
.gedit-view {{ background: {surface}; }}

/* ── Main app containers ── */
.main-bg {{ background: {bg}; color: {text}; }}
.panel-bg {{ background: {bg}; color: {text}; }}
.sidebar-bg {{ background: {surface}; color: {text}; }}
.toolbar {{ background: {surface}; border-bottom: 1px solid {border}; }}
.mb-btn-bar {{ background: {surface}; }}
.status-bar {{ background: {surface}; border-top: 1px solid {border}; }}
.status-bar label {{ color: {text2}; }}

/* ── GTK Box defaults — ensure all boxes inherit bg ── */
box {{ color: {text}; }}
grid {{ color: {text}; }}
"#,
    bg = p.bg, surface = p.surface, surface2 = p.surface2, border = p.border,
    text = p.text, text2 = p.text2, text3 = p.text3,
    accent = p.accent, accent2 = p.accent2,
    hero_from = p.hero_from, hero_to = p.hero_to, hero_text = p.hero_text, hero_sub = p.hero_sub,
    success = p.success, warning = p.warning, danger = p.danger, info = p.info, purple = p.purple,
    input_bg = p.input_bg, input_border = p.input_border, input_text = p.input_text,
    tab_bg = p.tab_bg,
    )
}

/// Current theme index — shared globally for new editor widgets.
static CURRENT_THEME: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Get the current theme palette (for reditctrl editor colors).
pub fn current_palette() -> &'static ThemePalette {
    let idx = CURRENT_THEME.load(std::sync::atomic::Ordering::Relaxed);
    &PALETTES[idx.min(THEME_COUNT - 1)]
}

fn apply_theme(idx: usize, provider: &gtk4::CssProvider, window: &gtk4::ApplicationWindow) {
    CURRENT_THEME.store(idx, std::sync::atomic::Ordering::Relaxed);
    let p = &PALETTES[idx];
    let css = generate_theme_css(p);
    provider.load_from_data(&css);

    // Set GTK dark preference for Dark/Nord/Monokai
    let display = gtk4::prelude::WidgetExt::display(window);
    let settings = gtk4::Settings::for_display(&display);
    let is_dark = idx == 1 || idx == 2 || idx == 4; // Dark, Nord, Monokai
    settings.set_gtk_application_prefer_dark_theme(is_dark);

    eprintln!("Theme: {}", THEME_NAMES[idx]);
}

// ══════════════════════════════════════════════════════════════════════
// Message view — port of main_eudora.c create_message_view + helpers
// ══════════════════════════════════════════════════════════════════════

/// Message view CSS (port of theme.c msg-* classes)
const MSG_CSS: &str = "
.msg-header-box { padding: 8px 12px; background: alpha(@theme_fg_color, 0.04); }
.msg-header-name { font-weight: bold; font-size: 0.88em;
    color: alpha(@theme_fg_color, 0.55); min-width: 70px; }
.msg-header-value { font-size: 0.88em; }
.msg-header-subject .msg-header-value { font-weight: bold; font-size: 0.96em; }
.msg-separator { min-height: 1px; background: alpha(@borders, 0.5); margin: 4px 0; }
.msg-body-view { padding: 8px 12px; }
";

fn install_msg_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(MSG_CSS);
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

/// Extract a header value from raw message text (handles continuation lines).
fn extract_header(raw: &str, name: &str) -> Option<String> {
    let needle = format!("{}:", name);
    for line_start in raw.split('\n') {
        let line = line_start.trim_end_matches('\r');
        if line.is_empty() {
            break; // blank line = end of headers
        }
        if line.len() > needle.len()
            && line[..needle.len()].eq_ignore_ascii_case(&needle)
        {
            let val = line[needle.len()..].trim_start();
            return Some(val.to_string());
        }
    }
    None
}

/// Find the body start (after the blank line separating headers from body).
fn find_body(raw: &str) -> &str {
    if let Some(pos) = raw.find("\r\n\r\n") {
        return &raw[pos + 4..];
    }
    if let Some(pos) = raw.find("\n\n") {
        return &raw[pos + 2..];
    }
    if let Some(pos) = raw.find("\r\r") {
        return &raw[pos + 2..];
    }
    raw
}

/// Build a styled header row: "Name:  value" (port of make_header_row).
fn make_header_row(name: &str, value: &str, extra_class: Option<&str>) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    if let Some(cls) = extra_class {
        row.add_css_class(cls);
    }

    let lbl_name = gtk4::Label::new(Some(name));
    lbl_name.add_css_class("msg-header-name");
    lbl_name.set_halign(gtk4::Align::End);
    lbl_name.set_valign(gtk4::Align::Start);

    let lbl_colon = gtk4::Label::new(Some(":"));
    lbl_colon.add_css_class("msg-header-name");

    let lbl_val = gtk4::Label::new(Some(value));
    lbl_val.add_css_class("msg-header-value");
    lbl_val.set_halign(gtk4::Align::Start);
    lbl_val.set_wrap(true);
    lbl_val.set_selectable(true);
    lbl_val.set_hexpand(true);

    row.append(&lbl_name);
    row.append(&lbl_colon);
    row.append(&lbl_val);
    row
}

/// Apply quote-level coloring to a TextView (port of style_quotes).
fn style_quotes(view: &gtk4::TextView) {
    let buf = view.buffer();
    let tag_table = buf.tag_table();

    let colors = &["#2962FF", "#00796B", "#6A1B9A"];
    let tag_names = &["q1", "q2", "q3"];
    for (i, (tn, color)) in tag_names.iter().zip(colors.iter()).enumerate() {
        if tag_table.lookup(tn).is_none() {
            let tag = gtk4::TextTag::new(Some(tn));
            tag.set_foreground(Some(color));
            tag.set_property("foreground-set", true);
            tag_table.add(&tag);
        }
    }

    let (start, end) = buf.bounds();
    let mut line_start = start;
    while line_start < end {
        let mut line_end = line_start;
        line_end.forward_to_line_end();

        let line = buf.text(&line_start, &line_end, false);
        let depth = line.chars().take_while(|c| *c == '>' || *c == ' ')
            .filter(|c| *c == '>')
            .count();
        if depth > 0 {
            let tag_idx = if depth > 3 { 2 } else { depth - 1 };
            buf.apply_tag_by_name(tag_names[tag_idx], &line_start, &line_end);
        }

        if !line_start.forward_line() {
            break;
        }
    }
}

/// Create a complete message view widget: styled header box + body with quote colors.
/// If `max_body_len > 0`, truncate the body (for preview pane).
/// Port of main_eudora.c create_message_view.
fn create_message_view(raw: &str, max_body_len: usize) -> gtk4::Widget {
    create_message_view_full(raw, max_body_len, None, None, None)
}

fn create_message_view_full(
    raw: &str,
    max_body_len: usize,
    mailer: Option<std::sync::Arc<macmbx::mailer::Mailer>>,
    conf: Option<Rc<RefCell<macmbx::conf::Conf>>>,
    books: Option<Rc<macmbx::nicknames::AddressBooks>>,
) -> gtk4::Widget {
    install_msg_css();

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vbox.add_css_class("panel-bg");

    // ── Message toolbar (port of original Eudora message window buttons) ──
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.add_css_class("toolbar");
    toolbar.set_margin_start(6);
    toolbar.set_margin_end(6);
    toolbar.set_margin_top(2);
    toolbar.set_margin_bottom(2);

    let tb = |icon: &str, tip: &str| -> gtk4::Button {
        let btn = gtk4::Button::from_icon_name(icon);
        btn.set_tooltip_text(Some(tip));
        btn
    };

    let btn_reply = tb("mail-reply-sender-symbolic", "Reply");
    let btn_reply_all = tb("mail-reply-all-symbolic", "Reply All");
    let btn_forward = tb("mail-forward-symbolic", "Forward");
    let btn_redirect = tb("go-next-symbolic", "Redirect");
    let btn_delete = tb("edit-delete-symbolic", "Delete");
    let btn_print = tb("printer-symbolic", "Print");

    toolbar.append(&btn_reply);
    toolbar.append(&btn_reply_all);
    toolbar.append(&btn_forward);
    toolbar.append(&btn_redirect);
    toolbar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
    toolbar.append(&btn_delete);
    toolbar.append(&btn_print);
    toolbar.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

    // Wire Reply/Reply All/Forward using the raw message directly
    let raw_for_actions = raw.to_string();
    {
        let r = raw_for_actions.clone();
        let conf_c = conf.clone();
        let mailer_c = mailer.clone();
        let books_c = books.clone();
        btn_reply.connect_clicked(move |btn| {
            let reply_to = extract_header(&r, "Reply-To")
                .or_else(|| extract_header(&r, "From"))
                .unwrap_or_default();
            let subj = extract_header(&r, "Subject").unwrap_or_default();
            let re_subj = if subj.to_ascii_lowercase().starts_with("re:") { subj.clone() }
                          else { format!("Re: {}", subj) };
            let from = extract_header(&r, "From").unwrap_or_default();
            let body = find_body(&r);
            let quoted = format!("On {} wrote:\n{}", from,
                body.lines().map(|l| format!("> {}", l)).collect::<Vec<_>>().join("\n"));

            if let (Some(ref c), Some(ref m)) = (&conf_c, &mailer_c) {
                let mut data = compose::ComposeData::default_new();
                data.to = reply_to;
                data.subject = re_subj;
                data.body = quoted;
                let page = compose::build_compose_tab_with_data(&c.borrow(), &data, m.clone(), books_c.clone());
                if let Some(nb) = find_notebook(btn) {
                    open_compose_tab(&nb, &page, "Reply");
                }
            }
        });
    }
    {
        let r = raw_for_actions.clone();
        let conf_c = conf.clone();
        let mailer_c = mailer.clone();
        let books_c = books.clone();
        btn_reply_all.connect_clicked(move |btn| {
            let reply_to = extract_header(&r, "Reply-To")
                .or_else(|| extract_header(&r, "From"))
                .unwrap_or_default();
            let subj = extract_header(&r, "Subject").unwrap_or_default();
            let re_subj = if subj.to_ascii_lowercase().starts_with("re:") { subj.clone() }
                          else { format!("Re: {}", subj) };
            let from = extract_header(&r, "From").unwrap_or_default();
            let orig_to = extract_header(&r, "To").unwrap_or_default();
            let orig_cc = extract_header(&r, "Cc").unwrap_or_default();
            let cc = if !orig_to.is_empty() && !orig_cc.is_empty() {
                format!("{}, {}", orig_to, orig_cc)
            } else if !orig_to.is_empty() { orig_to } else { orig_cc };
            let body = find_body(&r);
            let quoted = format!("On {} wrote:\n{}", from,
                body.lines().map(|l| format!("> {}", l)).collect::<Vec<_>>().join("\n"));

            if let (Some(ref c), Some(ref m)) = (&conf_c, &mailer_c) {
                let mut data = compose::ComposeData::default_new();
                data.to = reply_to;
                data.cc = cc;
                data.subject = re_subj;
                data.body = quoted;
                let page = compose::build_compose_tab_with_data(&c.borrow(), &data, m.clone(), books_c.clone());
                if let Some(nb) = find_notebook(btn) {
                    open_compose_tab(&nb, &page, "Reply All");
                }
            }
        });
    }
    {
        let r = raw_for_actions.clone();
        let conf_c = conf.clone();
        let mailer_c = mailer.clone();
        let books_c = books.clone();
        btn_forward.connect_clicked(move |btn| {
            let subj = extract_header(&r, "Subject").unwrap_or_default();
            let from = extract_header(&r, "From").unwrap_or_default();
            let body = find_body(&r);
            let fwd = format!("---------- Forwarded message ----------\nFrom: {}\nSubject: {}\n\n{}",
                from, subj, body);

            if let (Some(ref c), Some(ref m)) = (&conf_c, &mailer_c) {
                let mut data = compose::ComposeData::default_new();
                data.subject = format!("Fwd: {}", subj);
                data.body = fwd;
                let page = compose::build_compose_tab_with_data(&c.borrow(), &data, m.clone(), books_c.clone());
                if let Some(nb) = find_notebook(btn) {
                    open_compose_tab(&nb, &page, "Forward");
                }
            }
        });
    }
    {
        let r = raw_for_actions.clone();
        let conf_c = conf.clone();
        let mailer_c = mailer.clone();
        let books_c = books.clone();
        btn_redirect.connect_clicked(move |btn| {
            if let (Some(ref c), Some(ref m)) = (&conf_c, &mailer_c) {
                let data = compose::ComposeData::from_raw(&r);
                let page = compose::build_compose_tab_with_data(&c.borrow(), &data, m.clone(), books_c.clone());
                if let Some(nb) = find_notebook(btn) {
                    open_compose_tab(&nb, &page, "Redirect");
                }
            }
        });
    }
    // Delete — close the tab
    btn_delete.connect_clicked(move |btn| {
        if let Some(nb) = find_notebook(btn) {
            // Find our page and remove it
            let mut w: Option<gtk4::Widget> = Some(btn.clone().upcast());
            while let Some(ref curr) = w {
                if let Some(parent) = curr.parent() {
                    if nb.page_num(curr).is_some() {
                        nb.remove_page(nb.page_num(curr));
                        return;
                    }
                    w = Some(parent);
                } else { break; }
            }
        }
    });
    // Print handler wired after editor is created below

    // Headers toggle — show/hide all headers
    let all_headers = Rc::new(RefCell::new(false));
    let raw_copy = raw.to_string();
    let headers_btn = gtk4::ToggleButton::new();
    headers_btn.set_icon_name("view-more-horizontal-symbolic");
    headers_btn.set_tooltip_text(Some("Show All Headers"));
    headers_btn.set_can_focus(false);
    toolbar.append(&headers_btn);

    // Fixed font toggle
    let fixed_btn = gtk4::ToggleButton::new();
    fixed_btn.set_icon_name("font-x-generic-symbolic");
    fixed_btn.set_tooltip_text(Some("Fixed Width Font"));
    fixed_btn.set_can_focus(false);
    toolbar.append(&fixed_btn);

    vbox.append(&toolbar);
    vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // ── Header area ──
    let display_headers = &["From", "To", "Subject", "Date", "Cc", "Reply-To"];
    let all_header_names = &["From", "To", "Subject", "Date", "Cc", "Bcc", "Reply-To",
        "Message-ID", "In-Reply-To", "References", "Content-Type",
        "MIME-Version", "X-Mailer", "X-Priority", "Received"];

    let hdr_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    hdr_box.add_css_class("msg-header-box");

    let mut found_any = false;
    for &name in display_headers {
        if let Some(val) = extract_header(raw, name) {
            let extra = if name.eq_ignore_ascii_case("Subject") {
                Some("msg-header-subject")
            } else {
                None
            };
            hdr_box.append(&make_header_row(name, &val, extra));
            found_any = true;
        }
    }

    if found_any {
        let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
        sep.add_css_class("msg-separator");
        hdr_box.append(&sep);
    }
    vbox.append(&hdr_box);

    // ── Body — detect MIME multipart and decode, or use raw body ──
    let raw_body = find_body(raw);

    // Check if the message has MIME multipart content
    let headers_text = &raw[..raw.len() - raw_body.len()];
    let is_mime_multipart = headers_text.to_ascii_lowercase().contains("multipart/");

    let (body_text_owned, is_html) = if is_mime_multipart {
        // Parse MIME with crispy — extract HTML or plain body, save inline images
        match crispy::msg::parse(raw.as_bytes()) {
            Some(parsed) => {
                // Save attachments to Attachments folder (like Eudora's decode-on-view)
                let attach_dir: std::path::PathBuf = conf.as_ref()
                    .and_then(|c| {
                        let c = c.borrow();
                        let folder = c.get("attachments", "attach_folder").unwrap_or("");
                        if folder.is_empty() { None } else { Some(std::path::PathBuf::from(folder)) }
                    })
                    .unwrap_or_else(|| {
                        // Default: data_dir/../Attachments
                        let home = std::env::var("HOME").unwrap_or_default();
                        std::path::PathBuf::from(home)
                            .join(".local/share/geudora/Attachments")
                    });
                for part in &parsed.parts {
                    if part.is_attachment && !part.data.is_empty() {
                        let fname = part.filename.as_deref().unwrap_or("attachment");
                        let _ = std::fs::create_dir_all(&attach_dir);
                        let path = attach_dir.join(fname);
                        if !path.exists() {
                            let _ = std::fs::write(&path, &part.data);
                        }
                    }
                }

                // Prefer HTML body, fall back to plain
                if let Some(idx) = parsed.body_html_index {
                    let html = parsed.parts[idx].as_text().unwrap_or("").to_string();
                    // Resolve cid: references to temp files for inline images
                    let mut resolved = html.clone();
                    for part in &parsed.parts {
                        if let Some(ref cid) = part.content_id {
                            if !part.data.is_empty() {
                                let ext = if part.mime_type.contains("png") { "png" }
                                    else if part.mime_type.contains("gif") { "gif" }
                                    else { "jpg" };
                                let tmp = std::env::temp_dir().join(format!("rmail_cid_{}.{}", cid, ext));
                                let _ = std::fs::write(&tmp, &part.data);
                                let cid_ref = format!("cid:{}", cid);
                                let file_ref = format!("file://{}", tmp.display());
                                resolved = resolved.replace(&cid_ref, &file_ref);
                            }
                        }
                    }
                    (resolved, true)
                } else if let Some(idx) = parsed.body_plain_index {
                    (parsed.parts[idx].as_text().unwrap_or("").to_string(), false)
                } else {
                    // No body parts found — fall back to raw
                    (raw_body.to_string(), false)
                }
            }
            None => (raw_body.to_string(), false),
        }
    } else {
        // Not MIME — use raw body directly (Eudora's stored format)
        let body = if max_body_len > 0 && raw_body.len() > max_body_len {
            &raw_body[..max_body_len]
        } else {
            raw_body
        };
        let html = body.contains("<html") || body.contains("<body")
            || body.contains("<!doc") || body.contains("<div")
            || (body.contains("<b>") && body.contains("</b>"))
            || body.contains("<img");
        (body.to_string(), html)
    };

    let body_text: &str = &body_text_owned;

    // Use reditctrl for the body (port of message.c using geditctrl_new)
    let editor = reditctrl::gtk_widget::EditorWidget::new();
    // Apply theme
    {
        let p = current_palette();
        fn pc(hex: &str) -> reditctrl::render::Color {
            let hex = hex.trim_start_matches('#');
            if hex.len() >= 6 {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255) as f64 / 255.0;
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255) as f64 / 255.0;
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255) as f64 / 255.0;
                reditctrl::render::Color { red: r, green: g, blue: b, alpha: 1.0 }
            } else {
                reditctrl::render::Color { red: 1.0, green: 1.0, blue: 1.0, alpha: 1.0 }
            }
        }
        editor.set_theme_colors(pc(p.surface), pc(p.text), pc(p.text), pc(p.accent));
    }

    // Insert body — HTML as markup, plain as text
    if is_html {
        editor.ctrl.insert_markup(0, body_text);
    } else {
        editor.ctrl.set_text(body_text, -1);
    }
    // Read-only
    editor.ctrl.set_editable(false);

    // Wire print button — print the editor's document directly
    // (same document that has images + styles loaded)
    {
        let raw_for_hdr = raw.to_string();
        let print_ctrl = editor.ctrl.clone();
        btn_print.connect_clicked(move |btn| {
            let subject = extract_header(&raw_for_hdr, "Subject")
                .unwrap_or_else(|| "Message".to_string());

            // Print the editor's document — render_to_cairo handles
            // all styles, images, quotes, etc.
            let parent: gtk4::Widget = if let Some(root) = btn.root() {
                root.upcast()
            } else {
                btn.clone().upcast()
            };
            reditctrl::print::print_document(&parent, &print_ctrl.doc, Some(&subject));
        });
    }

    vbox.append(editor.widget());

    // ── Wire Headers toggle ──
    {
        let hb = hdr_box.clone();
        let raw = raw_copy.clone();
        headers_btn.connect_toggled(move |btn| {
            // Clear existing headers
            while let Some(c) = hb.first_child() { hb.remove(&c); }

            let show_all = btn.is_active();
            let headers_to_show: &[&str] = if show_all {
                &["From", "To", "Subject", "Date", "Cc", "Bcc", "Reply-To",
                  "Message-ID", "In-Reply-To", "References", "Content-Type",
                  "MIME-Version", "X-Mailer", "X-Priority", "Received"]
            } else {
                &["From", "To", "Subject", "Date", "Cc", "Reply-To"]
            };

            for &name in headers_to_show {
                if let Some(val) = extract_header(&raw, name) {
                    let extra = if name.eq_ignore_ascii_case("Subject") {
                        Some("msg-header-subject")
                    } else { None };
                    hb.append(&make_header_row(name, &val, extra));
                }
            }
            let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
            sep.add_css_class("msg-separator");
            hb.append(&sep);
        });
    }

    // ── Wire Fixed Font toggle ──
    {
        let ed = editor.widget().clone();
        fixed_btn.connect_toggled(move |btn| {
            if btn.is_active() {
                ed.add_css_class("monospace-font");
            } else {
                ed.remove_css_class("monospace-font");
            }
        });
    }

    vbox.upcast()
}

/// Format epoch seconds to "Mon DD, YYYY HH:MM" like the original Eudora.
fn format_epoch(secs: i64) -> String {
    // Use libc localtime for proper timezone handling
    #[cfg(unix)]
    {
        let t = secs as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&t, &mut tm); }
        let months = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
        let mon = months.get(tm.tm_mon as usize).unwrap_or(&"???");
        format!("{} {:02}, {} {:02}:{:02}",
            mon, tm.tm_mday, 1900 + tm.tm_year, tm.tm_hour, tm.tm_min)
    }
    #[cfg(not(unix))]
    {
        format!("{}", secs)
    }
}

fn main() {
    let app = gtk4::Application::builder()
        .application_id("net.elandaloussi.rmail")
        .build();

    app.connect_activate(build_ui);
    app.run();
}
