//! Widget construction: standalone editor window for testing/demo.
//!
//! Ported from `gEditCtrl/editor_control.c` + `editor_control.h`.
//!
//! Provides `editor_control_new()` which creates a fully-configured
//! `EditorCtrl` pre-populated with styled demo text — useful for
//! testing the editor outside the Eudora application.

use gdk4::RGBA;

use crate::document::Document;
use crate::glue::EditorCtrl;

/// Create a new standalone editor control pre-populated with demo text.
///
/// This is the Rust equivalent of C `editor_control_new()`.
/// It creates an `EditorCtrl`, inserts demo text, and applies
/// sample styles (bold, italic, underline, color) for visual testing.
pub fn editor_control_new() -> EditorCtrl {
    let ctrl = EditorCtrl::new_default();

    let demo = concat!(
        "gEdit editor \u{2014} placeholder control\n\n",
        "This is demo text inside the editor control.\n",
        "Select some text and press the toolbar buttons to toggle styles.\n",
    );
    ctrl.set_text(demo, -1);

    // Apply demo styles.
    apply_style_by_substr(&ctrl, "demo text", true, false, false, None);
    apply_style_by_substr(&ctrl, "placeholder control", false, true, false, None);
    apply_style_by_substr(&ctrl, "Select some text", false, false, true, None);
    apply_style_by_substr(
        &ctrl,
        "toolbar buttons",
        false,
        false,
        false,
        Some("#1e90ff"),
    );

    ctrl
}

/// Create a new editor control from an existing document.
pub fn editor_control_new_with_doc(doc: Document) -> EditorCtrl {
    EditorCtrl::new(doc)
}

/// Find `substr` in the document text and apply a style to that range.
///
/// Port of the C static helper `apply_style_by_substr`.
fn apply_style_by_substr(
    ctrl: &EditorCtrl,
    substr: &str,
    bold: bool,
    italic: bool,
    underline: bool,
    color_hex: Option<&str>,
) {
    let text = ctrl.get_text();
    if text.is_empty() {
        return;
    }

    let byte_index = match text.find(substr) {
        Some(idx) => idx,
        None => return,
    };

    // Convert byte index to character offset.
    let offset = text[..byte_index].chars().count() as i32;
    let length = substr.chars().count() as i32;

    let color = color_hex
        .and_then(|hex| {
            if hex.starts_with('#') && hex.len() == 7 {
                let r = u8::from_str_radix(&hex[1..3], 16).ok()?;
                let g = u8::from_str_radix(&hex[3..5], 16).ok()?;
                let b = u8::from_str_radix(&hex[5..7], 16).ok()?;
                Some(RGBA::new(
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    1.0,
                ))
            } else {
                None
            }
        });

    let bold_opt = if bold { Some(true) } else { None };
    let italic_opt = if italic { Some(true) } else { None };
    let underline_opt = if underline { Some(true) } else { None };

    ctrl.doc.add_style_run(offset, length, bold_opt, italic_opt, underline_opt, color, None);
}

// ════════════════════════════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // Note: these tests require GTK4 to be initialized.  They are
    // compile-checked but marked with #[ignore] so they don't fail in
    // headless CI environments that lack a display server.

    #[test]
    #[ignore]
    fn editor_control_new_creates_demo_text() {
        gtk4::init().ok();
        let ctrl = editor_control_new();

        let text = ctrl.get_text();
        assert!(text.contains("gEdit editor"));
        assert!(text.contains("demo text"));
        assert!(text.contains("placeholder control"));
        assert!(text.contains("Select some text"));
        assert!(text.contains("toolbar buttons"));
    }

    #[test]
    #[ignore]
    fn editor_control_new_applies_styles() {
        gtk4::init().ok();
        let ctrl = editor_control_new();

        let runs = ctrl.doc.get_style_runs();
        // Should have style runs for the demo styles.
        assert!(runs.len() >= 4);
    }

    #[test]
    #[ignore]
    fn editor_control_new_populates_doc() {
        gtk4::init().ok();
        let ctrl = editor_control_new();
        assert!(ctrl.get_length() > 0);
    }

    // Pure unit tests for helpers (no GTK init required).

    #[test]
    fn test_parse_color_hex() {
        let hex = "#1e90ff";
        let r = u8::from_str_radix(&hex[1..3], 16).unwrap();
        let g = u8::from_str_radix(&hex[3..5], 16).unwrap();
        let b = u8::from_str_radix(&hex[5..7], 16).unwrap();
        assert_eq!(r, 0x1e);
        assert_eq!(g, 0x90);
        assert_eq!(b, 0xff);
    }
}
