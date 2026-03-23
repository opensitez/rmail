//! Task progress panel — merged tasks + errors.
//!
//! Port of taskProgress.c. Active tasks show a spinner + text.
//! Errors show an error icon + text and persist until dismissed.
//! Used as the "Tasks" tab in the left wazoo notebook.

use gtk4::prelude::*;
use gtk4::glib;
use std::sync::{Mutex, atomic::{AtomicI32, Ordering}};
use std::collections::VecDeque;

// ── Global task/error queue (thread-safe, posted from scheduler threads) ──

static NEXT_ID: AtomicI32 = AtomicI32::new(1);

#[derive(Clone)]
enum TaskEvent {
    Add { id: i32, icon: String, text: String },
    Update { id: i32, text: String },
    Remove { id: i32 },
    Error { icon: String, text: String },
}

static EVENT_QUEUE: Mutex<Option<VecDeque<TaskEvent>>> = Mutex::new(None);

fn ensure_queue() {
    let mut q = EVENT_QUEUE.lock().unwrap();
    if q.is_none() { *q = Some(VecDeque::new()); }
}

fn post(event: TaskEvent) {
    ensure_queue();
    EVENT_QUEUE.lock().unwrap().as_mut().unwrap().push_back(event);
}

/// Add a task line. Returns a task ID for update/remove.
pub fn add_task(icon: &str, text: &str) -> i32 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    post(TaskEvent::Add { id, icon: icon.to_string(), text: text.to_string() });
    id
}

/// Update an existing task's text.
pub fn update_task(id: i32, text: &str) {
    post(TaskEvent::Update { id, text: text.to_string() });
}

/// Remove a task (it's done).
pub fn remove_task(id: i32) {
    post(TaskEvent::Remove { id });
}

/// Add a sticky error line.
pub fn add_error(icon: &str, text: &str) {
    post(TaskEvent::Error { icon: icon.to_string(), text: text.to_string() });
}

// ── Widget ──

/// Build the Tasks tab widget. Call once, embed in the left notebook.
/// Returns the widget + starts a 500ms poll timer to consume events.
pub fn build_tasks_panel() -> gtk4::Widget {
    ensure_queue();

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vbox.add_css_class("panel-bg");

    // Status bar: next/last check
    let status_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    status_bar.set_margin_start(8);
    status_bar.set_margin_end(8);
    status_bar.set_margin_top(4);
    status_bar.set_margin_bottom(4);
    let status_label = gtk4::Label::new(Some("No active tasks"));
    status_label.set_halign(gtk4::Align::Start);
    status_label.set_hexpand(true);
    status_label.add_css_class("dim-label");
    status_bar.append(&status_label);
    vbox.append(&status_bar);
    vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // Merged list: tasks + errors
    let listbox = gtk4::ListBox::new();
    listbox.set_selection_mode(gtk4::SelectionMode::None);

    let scroll = gtk4::ScrolledWindow::builder()
        .child(&listbox)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();
    vbox.append(&scroll);

    // Poll timer — drains events from background threads
    {
        let lb = listbox.clone();
        let sl = status_label.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
            let events: Vec<TaskEvent> = {
                let mut q = EVENT_QUEUE.lock().unwrap();
                match q.as_mut() {
                    Some(queue) => queue.drain(..).collect(),
                    None => Vec::new(),
                }
            };

            for event in events {
                match event {
                    TaskEvent::Add { id, icon, text } => {
                        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                        row.set_margin_start(8);
                        row.set_margin_end(8);
                        row.set_margin_top(4);
                        row.set_margin_bottom(4);
                        row.set_widget_name(&format!("task-{}", id));

                        let spinner = gtk4::Spinner::new();
                        spinner.set_spinning(true);
                        row.append(&spinner);

                        if !icon.is_empty() {
                            let img = gtk4::Image::from_icon_name(&icon);
                            img.set_pixel_size(16);
                            row.append(&img);
                        }

                        let label = gtk4::Label::new(Some(&text));
                        label.set_halign(gtk4::Align::Start);
                        label.set_hexpand(true);
                        label.set_ellipsize(pango::EllipsizeMode::End);
                        label.set_widget_name("task-label");
                        row.append(&label);

                        lb.append(&row);
                    }
                    TaskEvent::Update { id, text } => {
                        let target = format!("task-{}", id);
                        let mut child = lb.first_child();
                        while let Some(c) = child {
                            // ListBox wraps in ListBoxRow
                            if let Some(row) = c.first_child() {
                                if row.widget_name() == target {
                                    // Find the label inside
                                    let mut w = row.first_child();
                                    while let Some(ww) = w {
                                        if ww.widget_name() == "task-label" {
                                            if let Some(lbl) = ww.downcast_ref::<gtk4::Label>() {
                                                lbl.set_text(&text);
                                            }
                                            break;
                                        }
                                        w = ww.next_sibling();
                                    }
                                    break;
                                }
                            }
                            child = c.next_sibling();
                        }
                    }
                    TaskEvent::Remove { id } => {
                        let target = format!("task-{}", id);
                        let mut child = lb.first_child();
                        while let Some(c) = child {
                            let next = c.next_sibling();
                            if let Some(row) = c.first_child() {
                                if row.widget_name() == target {
                                    lb.remove(&c);
                                    break;
                                }
                            }
                            child = next;
                        }
                    }
                    TaskEvent::Error { icon, text } => {
                        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                        row.set_margin_start(8);
                        row.set_margin_end(8);
                        row.set_margin_top(4);
                        row.set_margin_bottom(4);
                        row.set_widget_name("error-row");

                        let img = gtk4::Image::from_icon_name(
                            if icon.is_empty() { "dialog-error-symbolic" } else { &icon });
                        img.set_pixel_size(16);
                        row.append(&img);

                        let label = gtk4::Label::new(Some(&text));
                        label.set_halign(gtk4::Align::Start);
                        label.set_hexpand(true);
                        label.set_ellipsize(pango::EllipsizeMode::End);
                        label.set_wrap(true);
                        label.add_css_class("error");
                        row.append(&label);

                        // Dismiss button
                        let dismiss = gtk4::Button::from_icon_name("window-close-symbolic");
                        dismiss.add_css_class("flat");
                        dismiss.set_valign(gtk4::Align::Start);
                        let lb2 = lb.clone();
                        let row_ref = row.clone();
                        dismiss.connect_clicked(move |_| {
                            if let Some(parent) = row_ref.parent() {
                                lb2.remove(&parent);
                            }
                        });
                        row.append(&dismiss);

                        lb.append(&row);
                    }
                }
            }

            // Update status label
            let mut task_count = 0;
            let mut error_count = 0;
            let mut child = lb.first_child();
            while let Some(c) = child {
                if let Some(inner) = c.first_child() {
                    let name = inner.widget_name();
                    if name.starts_with("task-") { task_count += 1; }
                    else if name == "error-row" { error_count += 1; }
                }
                child = c.next_sibling();
            }

            if task_count == 0 && error_count == 0 {
                sl.set_text("No active tasks");
            } else {
                let mut parts = Vec::new();
                if task_count > 0 { parts.push(format!("{} active", task_count)); }
                if error_count > 0 { parts.push(format!("{} error(s)", error_count)); }
                sl.set_text(&parts.join(" · "));
            }

            glib::ControlFlow::Continue
        });
    }

    vbox.upcast()
}
