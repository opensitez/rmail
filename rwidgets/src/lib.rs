//! # rWidgets — Custom GTK4 Widgets for rMail
//!
//! Owner-drawn replacements for deprecated GtkTreeView:
//! - `MailList` — message list with 2-line card rows, selection, multi-select, DnD
//! - `MailboxTree` — sidebar mailbox tree with icons, unread badges, DnD drop target
//! - `DatePicker` / `TimePicker` — date and time picker composites
//! - `autocomplete` — nickname/address autocomplete for entry fields
//! - `calendar` — calendar sidebar panel
//! - `calendar_event_dlg` — calendar event create/edit dialog

pub mod mailbox_tree;
pub mod mail_list;
pub mod drag_state;
pub mod calendar;
pub mod calendar_event_dlg;
pub mod caldav_account_dlg;
pub mod meeting_invite_dlg;
pub mod date_picker;
pub mod time_picker;
pub mod autocomplete;
