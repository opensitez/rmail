//! Sidebar — mailbox tree populated from real macmbx::store data.

use gtk4::prelude::*;
use gtk4::glib;

pub const COL_NAME: i32 = 0;
pub const COL_ICON: i32 = 1;
pub const COL_PATH: i32 = 2;
pub const COL_UNREAD: i32 = 3;
pub const COL_IS_DIR: i32 = 4;
pub const COL_WEIGHT: i32 = 5;
pub const COL_DISPLAY: i32 = 6;

/// Build a GtkTreeView populated from a real macmbx Store.
pub fn build_sidebar(store: &macmbx::store::Store) -> gtk4::TreeView {
    let tree_store = gtk4::TreeStore::new(&[
        glib::Type::STRING,  // 0: name
        glib::Type::STRING,  // 1: icon
        glib::Type::STRING,  // 2: full path
        glib::Type::STRING,  // 3: unread count (display)
        glib::Type::BOOL,    // 4: is directory
        glib::Type::I32,     // 5: font weight
        glib::Type::STRING,  // 6: display name (with unread)
    ]);

    // Walk the real store tree
    populate_node(&tree_store, None, store.children(), store);

    let tree = gtk4::TreeView::with_model(&tree_store);
    tree.set_headers_visible(false);
    tree.set_enable_tree_lines(true);

    // Single column: icon + name + unread
    let col = gtk4::TreeViewColumn::new();

    let icon_renderer = gtk4::CellRendererPixbuf::new();
    col.pack_start(&icon_renderer, false);
    col.add_attribute(&icon_renderer, "icon-name", COL_ICON);

    let text_renderer = gtk4::CellRendererText::new();
    col.pack_start(&text_renderer, true);
    col.add_attribute(&text_renderer, "text", COL_DISPLAY);
    col.add_attribute(&text_renderer, "weight", COL_WEIGHT);

    tree.append_column(&col);

    // Expand special mailboxes by default
    tree.expand_all();

    tree
}

fn populate_node(
    tree_store: &gtk4::TreeStore,
    parent: Option<&gtk4::TreeIter>,
    children: &[macmbx::store::Node],
    store: &macmbx::store::Store,
) {
    for node in children {
        let is_dir = node.node_type == macmbx::store::NodeType::Folder;
        let icon = if is_dir {
            "folder-symbolic"
        } else {
            match node.mbox_type {
                macmbx::toc::MailboxType::In => "mail-inbox-symbolic",
                macmbx::toc::MailboxType::Out => "mail-outbox-symbolic",
                macmbx::toc::MailboxType::Trash => "user-trash-symbolic",
                macmbx::toc::MailboxType::Junk => "mail-mark-junk-symbolic",
                _ => "mail-unread-symbolic",
            }
        };

        // Get real unread count by peeking TOC
        let mut unread = 0usize;
        let full_path = node.path.clone();
        if !is_dir {
            if let Ok(toc) = macmbx::toc::Toc::open(&full_path) {
                unread = toc.unread_count();
            }
        }

        let display = if unread > 0 {
            format!("{} ({})", node.name, unread)
        } else {
            node.name.clone()
        };

        let weight = if unread > 0 { 700 } else { 400 };

        let iter = tree_store.append(parent);
        tree_store.set(&iter, &[
            (COL_NAME as u32, &node.name),
            (COL_ICON as u32, &icon),
            (COL_PATH as u32, &node.path.to_string_lossy().to_string()),
            (COL_UNREAD as u32, &if unread > 0 { format!("({})", unread) } else { String::new() }),
            (COL_IS_DIR as u32, &is_dir),
            (COL_WEIGHT as u32, &weight),
            (COL_DISPLAY as u32, &display),
        ]);

        // Recurse into children
        if !node.children.is_empty() {
            populate_node(tree_store, Some(&iter), &node.children, store);
        }
    }
}
