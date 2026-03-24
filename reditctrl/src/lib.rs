//! # rEditCtrl — Rich Text Editor Widget for GTK4
//!
//! Rust port of gEditCtrl. A custom rich text editor built on
//! GtkDrawingArea + Cairo + Pango, providing:
//! - Styled text (bold, italic, underline, color, font family/size)
//! - Paragraph attributes (alignment, indent, quote level, bullets)
//! - Inline images with resize handles
//! - Horizontal rules and page breaks
//! - HTML markup import/export
//! - Tables
//! - Undo/redo
//! - Clipboard (cut/copy/paste with rich text)
//! - Find/replace
//! - Print rendering to cairo context
//! - Selection, caret, double/triple click
//! - Read-only mode

pub mod document;
pub mod state;
pub mod render;
pub mod input;
pub mod clipboard;
pub mod table;
pub mod print;
pub mod widget;
pub mod glue;
pub mod gtk_widget;
pub mod graph;
