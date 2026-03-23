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
    /// Detailed task with title (account name) and subtitle (status)
    AddDetailed { id: i32, icon: String, title: String, subtitle: String },
    Update { id: i32, text: String },
    /// Update with separate subtitle and optional progress (0.0-1.0, negative = indeterminate)
    UpdateDetailed { id: i32, subtitle: String, progress: f64 },
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

/// Add a detailed task with title + subtitle. Returns task ID.
pub fn add_detailed_task(icon: &str, title: &str, subtitle: &str) -> i32 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    post(TaskEvent::AddDetailed { id, icon: icon.to_string(), title: title.to_string(), subtitle: subtitle.to_string() });
    id
}

/// Update an existing task's text.
pub fn update_task(id: i32, text: &str) {
    post(TaskEvent::Update { id, text: text.to_string() });
}

/// Update a detailed task's subtitle and progress bar.
pub fn update_detailed_task(id: i32, subtitle: &str, progress: f64) {
    post(TaskEvent::UpdateDetailed { id, subtitle: subtitle.to_string(), progress });
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

    // Double-click on error row → show detail dialog
    {
        let lb = listbox.clone();
        let gesture = gtk4::GestureClick::new();
        gesture.set_button(1);
        gesture.connect_released(move |g, n_press, _x, y| {
            if n_press != 2 { return; }
            // Find which row was clicked
            let Some(widget) = g.widget() else { return };
            let Some(root) = widget.root() else { return };
            let window: gtk4::Window = match root.downcast::<gtk4::Window>() {
                Ok(w) => w,
                Err(_) => return,
            };

            // Walk ListBox children to find the row at y
            let mut child = lb.first_child();
            while let Some(c) = child {
                if let Some(inner) = c.first_child() {
                    if inner.widget_name() == "error-row" {
                        // Check if click is within this row's bounds
                        let (_, row_y) = c.translate_coordinates(&lb, 0.0, 0.0).unwrap_or((0.0, 0.0));
                        let row_h = c.height() as f64;
                        if y >= row_y && y < row_y + row_h {
                            // Get full error text from tooltip
                            let detail = inner.tooltip_text()
                                .map(|s| s.to_string())
                                .unwrap_or_default();
                            if !detail.is_empty() {
                                let dlg = gtk4::AlertDialog::builder()
                                    .modal(true)
                                    .message("Error Details")
                                    .detail(&detail)
                                    .buttons(["OK"])
                                    .build();
                                dlg.show(Some(&window));
                            }
                            return;
                        }
                    }
                }
                child = c.next_sibling();
            }
        });
        listbox.add_controller(gesture);
    }

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
                    TaskEvent::AddDetailed { id, icon, title, subtitle } => {
                        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                        row.set_margin_start(8);
                        row.set_margin_end(8);
                        row.set_margin_top(6);
                        row.set_margin_bottom(6);
                        row.set_widget_name(&format!("task-{}", id));

                        if !icon.is_empty() {
                            let img = gtk4::Image::from_icon_name(&icon);
                            img.set_pixel_size(16);
                            img.set_valign(gtk4::Align::Start);
                            row.append(&img);
                        }

                        // Two-line layout: title + subtitle + progress bar
                        let info = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
                        info.set_hexpand(true);

                        let title_lbl = gtk4::Label::new(Some(&title));
                        title_lbl.set_halign(gtk4::Align::Start);
                        title_lbl.set_ellipsize(pango::EllipsizeMode::End);
                        title_lbl.set_widget_name("task-label");
                        info.append(&title_lbl);

                        let sub_lbl = gtk4::Label::new(Some(&subtitle));
                        sub_lbl.set_halign(gtk4::Align::Start);
                        sub_lbl.set_ellipsize(pango::EllipsizeMode::End);
                        sub_lbl.add_css_class("dim-label");
                        sub_lbl.set_widget_name("task-subtitle");
                        info.append(&sub_lbl);

                        let pbar = gtk4::ProgressBar::new();
                        pbar.set_widget_name("task-progress");
                        pbar.pulse(); // indeterminate initially
                        info.append(&pbar);

                        row.append(&info);

                        let spinner = gtk4::Spinner::new();
                        spinner.set_spinning(true);
                        spinner.set_valign(gtk4::Align::Start);
                        row.append(&spinner);

                        lb.append(&row);
                    }
                    TaskEvent::UpdateDetailed { id, subtitle, progress } => {
                        let target = format!("task-{}", id);
                        let mut child = lb.first_child();
                        while let Some(c) = child {
                            if let Some(row) = c.first_child() {
                                if row.widget_name() == target {
                                    let mut w = row.first_child();
                                    while let Some(ww) = w {
                                        // Find the info vbox
                                        let mut inner = ww.first_child();
                                        while let Some(iw) = inner {
                                            if iw.widget_name() == "task-subtitle" {
                                                if let Some(lbl) = iw.downcast_ref::<gtk4::Label>() {
                                                    lbl.set_text(&subtitle);
                                                }
                                            }
                                            if iw.widget_name() == "task-progress" {
                                                if let Some(pb) = iw.downcast_ref::<gtk4::ProgressBar>() {
                                                    if progress < 0.0 {
                                                        pb.pulse();
                                                    } else {
                                                        pb.set_fraction(progress.min(1.0));
                                                    }
                                                }
                                            }
                                            inner = iw.next_sibling();
                                        }
                                        w = ww.next_sibling();
                                    }
                                    break;
                                }
                            }
                            child = c.next_sibling();
                        }
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
                        // Store full error text for detail dialog
                        row.set_tooltip_text(Some(&text));

                        let img = gtk4::Image::from_icon_name(
                            if icon.is_empty() { "dialog-error-symbolic" } else { &icon });
                        img.set_pixel_size(16);
                        row.append(&img);

                        let label = gtk4::Label::new(Some(&text));
                        label.set_halign(gtk4::Align::Start);
                        label.set_hexpand(true);
                        label.set_ellipsize(pango::EllipsizeMode::End);
                        label.add_css_class("error");
                        label.set_widget_name("error-label");
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
