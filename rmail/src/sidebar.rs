//! Sidebar — custom-drawn mailbox tree using rwidgets::mailbox_tree.
//!
//! Replaces the deprecated GtkTreeView with a modern owner-drawn widget.
//! Provides the same external API so main.rs changes are minimal.

use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub use rwidgets::mailbox_tree::ThemeColors;

/// Sidebar widget wrapping rwidgets::mailbox_tree.
pub struct SidebarWidget {
    /// The scrolled window to embed in the layout.
    pub widget: gtk4::ScrolledWindow,
    /// Set the callback for when a mailbox is activated (clicked).
    pub on_activate: Rc<RefCell<Option<Box<dyn Fn(&str, &str, bool)>>>>,
    /// Set the callback for when messages are dropped on a mailbox.
    pub on_drop: Rc<RefCell<Option<Box<dyn Fn(&str, Vec<usize>, &str)>>>>,
    /// Call to rebuild the tree from a new store.
    refresh_fn: Rc<dyn Fn(&[macmbx::store::Node])>,
    /// Set theme colors.
    set_theme_fn: Rc<dyn Fn(ThemeColors)>,
}

impl SidebarWidget {
    /// Refresh the sidebar from a store.
    pub fn refresh(&self, store: &mut macmbx::store::Store) {
        store.update_counts();
        (self.refresh_fn)(store.children());
    }

    /// Set theme colors for the sidebar.
    pub fn set_theme(&self, colors: ThemeColors) {
        (self.set_theme_fn)(colors);
    }
}

/// Build the sidebar from a macmbx Store.
pub fn build_sidebar(store: &mut macmbx::store::Store) -> SidebarWidget {
    store.update_counts();
    let (widget, on_activate, on_drop, refresh_fn, set_theme_fn, on_mailbox_drop) =
        rwidgets::mailbox_tree::build_mailbox_tree(store.children());

    // Wire drop target to do actual message transfers
    {
        let od = on_drop.clone();
        *od.borrow_mut() = Some(Box::new(|src_path: &str, indices: Vec<usize>, dst_path: &str| {
            let mut src_toc = match macmbx::toc::Toc::open(src_path) {
                Ok(t) => t,
                Err(_) => return,
            };
            let mut dst_toc = match macmbx::toc::Toc::open_or_create(
                std::path::Path::new(dst_path))
            {
                Ok(t) => t,
                Err(_) => return,
            };
            let mut sorted = indices;
            sorted.sort_unstable();
            sorted.reverse();
            let mut count = 0;
            for idx in &sorted {
                if macmbx::ops::transfer(&mut src_toc, *idx, &mut dst_toc, false).is_ok() {
                    count += 1;
                }
            }
            if count > 0 {
                let _ = src_toc.save();
                let _ = dst_toc.save();
            }
        }));
    }

    // Wire mailbox-to-folder drop: move the mbox file into the folder
    {
        let omd = on_mailbox_drop.clone();
        *omd.borrow_mut() = Some(Box::new(|mbox_path: &str, folder_path: &str| {
            let src = std::path::Path::new(mbox_path);
            let dst_dir = std::path::Path::new(folder_path);
            if let Some(name) = src.file_name() {
                let dst = dst_dir.join(name);
                if dst != src {
                    let _ = std::fs::rename(src, &dst);
                    // Also move .toc if present
                    let src_toc = src.with_extension("toc");
                    if src_toc.exists() {
                        let dst_toc = dst.with_extension("toc");
                        let _ = std::fs::rename(&src_toc, &dst_toc);
                    }
                }
            }
        }));
    }

    SidebarWidget {
        widget,
        on_activate,
        on_drop,
        refresh_fn,
        set_theme_fn,
    }
}
