//! # rWidgets — Custom GTK4 Widgets for rMail
//!
//! Owner-drawn replacements for deprecated GtkTreeView:
//! - `MailList` — message list with 2-line card rows, selection, multi-select, DnD
//! - `MailboxTree` — sidebar mailbox tree with icons, unread badges, DnD drop target

pub mod mail_list;
pub mod mailbox_tree;
pub mod drag_state;
