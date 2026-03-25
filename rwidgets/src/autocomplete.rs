//! Nickname autocomplete for address entry fields.
//!
//! Port of gtk_autocomplete.c.
//! Attaches to GtkEntry fields (To, Cc, Bcc). As the user types,
//! queries macmbx::nicknames::AddressBooks::complete() and shows
//! a popover with matches. Tab accepts, Escape dismisses,
//! Enter/click picks from popup, comma finalizes recipient.

use gtk4::prelude::*;
use gtk4::glib;
use macmbx::nicknames::{AddressBooks, CompleteResult};
use std::cell::RefCell;
use std::rc::Rc;

const MAX_POPUP_ROWS: usize = 10;
const MIN_PREFIX_LEN: usize = 1;

struct AutocompleteCtx {
    books: Rc<AddressBooks>,
    popover: Option<gtk4::Popover>,
    listbox: Option<gtk4::ListBox>,
    results: Vec<CompleteResult>,
    suppress: bool,
}

/// Find the start of the current recipient (after last comma).
fn find_recipient_start(text: &str, cursor: usize) -> usize {
    let bytes = text.as_bytes();
    let mut start = cursor;
    while start > 0 && bytes[start - 1] != b',' {
        start -= 1;
    }
    // Skip leading whitespace
    while start < cursor && bytes[start] == b' ' {
        start += 1;
    }
    start
}

/// Extract the prefix being typed (from last comma to cursor position).
fn get_current_prefix(entry: &gtk4::Entry) -> (String, usize) {
    let text = entry.text().to_string();
    let cursor = entry.position() as usize;
    let cursor = cursor.min(text.len());
    let start = find_recipient_start(&text, cursor);
    let prefix = text[start..cursor].to_string();
    (prefix, start)
}

/// Insert a completed address, replacing the current prefix.
fn insert_completion(entry: &gtk4::Entry, completion: &str, ctx: &RefCell<AutocompleteCtx>) {
    let text = entry.text().to_string();
    let cursor = entry.position() as usize;
    let cursor = cursor.min(text.len());
    let start = find_recipient_start(&text, cursor);

    ctx.borrow_mut().suppress = true;

    let mut buf = String::new();
    buf.push_str(&text[..start]);
    buf.push_str(completion);
    buf.push_str(", ");
    let new_cursor = buf.len();
    // Preserve anything after cursor (other recipients)
    if cursor < text.len() {
        buf.push_str(&text[cursor..]);
    }

    entry.set_text(&buf);
    entry.set_position(new_cursor as i32);

    ctx.borrow_mut().suppress = false;

    // Dismiss popup
    if let Some(ref pop) = ctx.borrow().popover {
        pop.popdown();
    }
}

/// Attach autocomplete to a GtkEntry for address completion.
/// `books` provides the nickname data via `complete()`.
pub fn attach(entry: &gtk4::Entry, books: Rc<AddressBooks>) {
    let ctx = Rc::new(RefCell::new(AutocompleteCtx {
        books,
        popover: None,
        listbox: None,
        results: Vec::new(),
        suppress: false,
    }));

    // ── Text changed → query + show popup ──
    {
        let entry_clone = entry.clone();
        let ctx_clone = ctx.clone();
        entry.connect_changed(move |_| {
            if ctx_clone.borrow().suppress { return; }

            let (prefix, _start) = get_current_prefix(&entry_clone);
            let books = ctx_clone.borrow().books.clone();

            // Clear old results
            ctx_clone.borrow_mut().results.clear();

            if prefix.len() < MIN_PREFIX_LEN {
                if let Some(ref pop) = ctx_clone.borrow().popover {
                    pop.popdown();
                }
                return;
            }

            let results = books.complete(&prefix, MAX_POPUP_ROWS);
            if results.is_empty() {
                if let Some(ref pop) = ctx_clone.borrow().popover {
                    pop.popdown();
                }
                return;
            }

            ctx_clone.borrow_mut().results = results;

            // Defer popup to avoid GTK user-action nesting
            let e = entry_clone.clone();
            let c = ctx_clone.clone();
            glib::idle_add_local_once(move || {
                show_popup_and_typeahead(&e, &c);
            });
        });
    }

    // ── Key handler: Tab/Escape/Down/Enter ──
    {
        let entry_clone = entry.clone();
        let ctx_clone = ctx.clone();
        let key_ctrl = gtk4::EventControllerKey::new();
        key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
            let ctx = &ctx_clone;

            // Tab — accept top match
            if keyval == gdk4::Key::Tab || keyval == gdk4::Key::ISO_Left_Tab {
                let insert = {
                    let c = ctx.borrow();
                    c.results.first().and_then(|r| {
                        r.addresses.as_ref().filter(|a| !a.is_empty())
                            .or(Some(&r.display))
                            .cloned()
                    })
                };
                if let Some(text) = insert {
                    insert_completion(&entry_clone, &text, ctx);
                    return glib::Propagation::Stop;
                }
            }

            // Escape — dismiss popup
            if keyval == gdk4::Key::Escape {
                if let Some(ref pop) = ctx.borrow().popover {
                    pop.popdown();
                }
                let pos = entry_clone.position();
                entry_clone.select_region(pos, pos);
                return glib::Propagation::Stop;
            }

            // Down arrow — move focus into popup
            if keyval == gdk4::Key::Down {
                let c = ctx.borrow();
                if let (Some(ref pop), Some(ref lb)) = (&c.popover, &c.listbox) {
                    if pop.is_visible() {
                        lb.grab_focus();
                        if let Some(first) = lb.row_at_index(0) {
                            lb.select_row(Some(&first));
                        }
                        return glib::Propagation::Stop;
                    }
                }
            }

            // Enter — pick from popup or accept typeahead
            if keyval == gdk4::Key::Return || keyval == gdk4::Key::KP_Enter {
                let c = ctx.borrow();
                if let (Some(ref pop), Some(ref lb)) = (&c.popover, &c.listbox) {
                    if pop.is_visible() {
                        if let Some(sel) = lb.selected_row() {
                            let idx = sel.index() as usize;
                            drop(c);
                            let insert = {
                                let c2 = ctx.borrow();
                                c2.results.get(idx).and_then(|r| {
                                    r.addresses.as_ref().filter(|a| !a.is_empty())
                                        .or(Some(&r.display))
                                        .cloned()
                                })
                            };
                            if let Some(text) = insert {
                                insert_completion(&entry_clone, &text, ctx);
                                return glib::Propagation::Stop;
                            }
                        }
                    }
                }
            }

            glib::Propagation::Proceed
        });
        entry.add_controller(key_ctrl);
    }
}

fn show_popup_and_typeahead(entry: &gtk4::Entry, ctx: &Rc<RefCell<AutocompleteCtx>>) {
    let result_count = ctx.borrow().results.len();
    if result_count == 0 { return; }

    let (prefix, _start) = get_current_prefix(entry);

    // ── Type-ahead: append rest of top match as selected text ──
    {
        let tail_info = {
            let c = ctx.borrow();
            c.results.first().and_then(|first| {
                let display = &first.display;
                if display.len() > prefix.len() {
                    let lower_display = display.to_ascii_lowercase();
                    let lower_prefix = prefix.to_ascii_lowercase();
                    if lower_display.starts_with(&lower_prefix) {
                        Some(display[prefix.len()..].to_string())
                    } else { None }
                } else { None }
            })
        };
        if let Some(tail) = tail_info {
            let text = entry.text().to_string();
            let cursor = (entry.position() as usize).min(text.len());

            ctx.borrow_mut().suppress = true;
            let mut buf = String::new();
            buf.push_str(&text[..cursor]);
            buf.push_str(&tail);
            if cursor < text.len() {
                buf.push_str(&text[cursor..]);
            }
            entry.set_text(&buf);
            entry.select_region(cursor as i32, (cursor + tail.len()) as i32);
            ctx.borrow_mut().suppress = false;
        }
    }

    // ── Popup list (if multiple matches) ──
    if result_count <= 1 {
        if let Some(ref pop) = ctx.borrow().popover {
            pop.popdown();
        }
        return;
    }

    // Create popover on first use
    {
        let mut c = ctx.borrow_mut();
        if c.popover.is_none() {
            let popover = gtk4::Popover::new();
            popover.set_autohide(false);
            popover.set_has_arrow(false);
            popover.set_parent(entry);

            let sw = gtk4::ScrolledWindow::builder()
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .vscrollbar_policy(gtk4::PolicyType::Automatic)
                .max_content_height(250)
                .propagate_natural_height(true)
                .build();
            sw.set_size_request(350, -1);

            let listbox = gtk4::ListBox::new();
            listbox.set_selection_mode(gtk4::SelectionMode::Single);

            // Row activated → pick
            let entry_for_activate = entry.clone();
            let ctx_for_activate = ctx.clone();
            listbox.connect_row_activated(move |_, row| {
                let idx = row.index() as usize;
                let insert = {
                    let c = ctx_for_activate.borrow();
                    c.results.get(idx).and_then(|r| {
                        r.addresses.as_ref().filter(|a| !a.is_empty())
                            .or(Some(&r.display))
                            .cloned()
                    })
                };
                if let Some(text) = insert {
                    insert_completion(&entry_for_activate, &text, &ctx_for_activate);
                }
            });

            sw.set_child(Some(&listbox));
            popover.set_child(Some(&sw));

            c.popover = Some(popover);
            c.listbox = Some(listbox);
        }
    }

    // Populate rows
    let c = ctx.borrow();
    if let Some(ref listbox) = c.listbox {
        while let Some(child) = listbox.first_child() {
            listbox.remove(&child);
        }

        let show = result_count.min(MAX_POPUP_ROWS);
        for i in 0..show {
            let r = &c.results[i];
            let row_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            row_box.set_margin_start(6);
            row_box.set_margin_end(6);
            row_box.set_margin_top(3);
            row_box.set_margin_bottom(3);

            let name_label = gtk4::Label::new(Some(&r.display));
            name_label.set_halign(gtk4::Align::Start);
            name_label.set_ellipsize(pango::EllipsizeMode::End);
            row_box.append(&name_label);

            if let Some(ref addr) = r.addresses {
                if !addr.is_empty() {
                    let addr_label = gtk4::Label::new(Some(addr));
                    addr_label.set_halign(gtk4::Align::Start);
                    addr_label.set_ellipsize(pango::EllipsizeMode::End);
                    addr_label.set_opacity(0.6);
                    addr_label.add_css_class("dim-label");
                    addr_label.add_css_class("caption");
                    row_box.append(&addr_label);
                }
            }

            listbox.append(&row_box);
        }
    }

    if let Some(ref pop) = c.popover {
        pop.popup();
    }
}
