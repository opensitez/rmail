//! Custom drag & drop state — shared between MailList and MailboxTree.
//!
//! No GTK DnD APIs. Just mouse tracking across our own widgets.

use std::cell::RefCell;
use gtk4::prelude::*;

#[derive(Debug, Clone)]
pub struct DragPayload {
    pub src_mbox: String,
    pub indices: Vec<i32>,
}

type DropZoneFn = Box<dyn Fn(f64, f64) -> Option<String>>;
type HoverNotifyFn = Box<dyn Fn(Option<f64>)>;

thread_local! {
    static DRAG: RefCell<Option<DragPayload>> = RefCell::new(None);
    static DROP_TARGET: RefCell<Option<String>> = RefCell::new(None);
    static DROP_CB: RefCell<Option<Box<dyn Fn(&str, Vec<usize>, &str)>>> = RefCell::new(None);
    static DROP_ZONE: RefCell<Option<(gtk4::Widget, DropZoneFn, HoverNotifyFn)>> = RefCell::new(None);
}

/// Start a drag with the given payload.
pub fn start_drag(src_mbox: &str, indices: Vec<i32>) {
    DRAG.with(|d| *d.borrow_mut() = Some(DragPayload {
        src_mbox: src_mbox.to_string(),
        indices,
    }));
}

/// Check if a drag is in progress.
pub fn is_dragging() -> bool {
    DRAG.with(|d| d.borrow().is_some())
}

/// Take the drag payload (consumes it — ends the drag).
pub fn take_drag() -> Option<DragPayload> {
    DRAG.with(|d| d.borrow_mut().take())
}

/// Cancel the drag without dropping.
pub fn cancel_drag() {
    DRAG.with(|d| *d.borrow_mut() = None);
    DROP_TARGET.with(|dt| *dt.borrow_mut() = None);
    DROP_ZONE.with(|dz| {
        if let Some((_, _, hover_notify)) = &*dz.borrow() {
            hover_notify(None);
        }
    });
}

/// Set the drop callback (called by sidebar on init).
pub fn set_drop_callback<F: Fn(&str, Vec<usize>, &str) + 'static>(f: F) {
    DROP_CB.with(|cb| *cb.borrow_mut() = Some(Box::new(f)));
}

/// Register a widget as the global drop zone
pub fn register_drop_zone<W: IsA<gtk4::Widget>, F, H>(widget: &W, handler: F, hover_notify: H)
where
    F: Fn(f64, f64) -> Option<String> + 'static,
    H: Fn(Option<f64>) + 'static
{
    DROP_ZONE.with(|dz| {
        *dz.borrow_mut() = Some((
            widget.upcast_ref::<gtk4::Widget>().clone(),
            Box::new(handler),
            Box::new(hover_notify)
        ));
    });
}

/// Update drag position by picking from the root window.
pub fn update_drag(root: &gtk4::Widget, root_x: f64, root_y: f64) {
    let mut new_target = None;
    let mut hover_y = None;
    
    let target = root.pick(root_x, root_y, gtk4::PickFlags::DEFAULT);
    
    if let Some(t) = target {
        DROP_ZONE.with(|dz| {
            if let Some((zone_widget, handler, _)) = &*dz.borrow() {
                if t.is_ancestor(zone_widget) || t == *zone_widget {
                    if let Some((lx, ly)) = root.translate_coordinates(zone_widget, root_x, root_y) {
                        new_target = handler(lx, ly);
                        hover_y = Some(ly);
                    }
                }
            }
        });
    }
    
    DROP_TARGET.with(|dt| *dt.borrow_mut() = new_target);
    DROP_ZONE.with(|dz| {
        if let Some((_, _, hover_notify)) = &*dz.borrow() {
            hover_notify(hover_y);
        }
    });
}

/// Execute the drop: if we have a payload and a target, call the callback.
/// Returns true if drop was executed.
pub fn execute_drop() -> bool {
    let payload = match take_drag() {
        Some(p) => p,
        None => return false,
    };
    let target = match DROP_TARGET.with(|dt| dt.borrow_mut().take()) {
        Some(t) => t,
        None => return false,
    };
    
    // Reset drop zone hover
    DROP_ZONE.with(|dz| {
        if let Some((_, _, hover_notify)) = &*dz.borrow() {
            hover_notify(None);
        }
    });

    if target == payload.src_mbox { return false; }
    let indices: Vec<usize> = payload.indices.iter().map(|&i| i as usize).collect();
    
    let mut executed = false;
    DROP_CB.with(|cb| {
        if let Some(ref f) = *cb.borrow() {
            f(&payload.src_mbox, indices, &target);
            executed = true;
        }
    });
    executed
}
