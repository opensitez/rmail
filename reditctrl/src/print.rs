//! Print support — export styled document to PDF.
//!
//! Uses render::render_to_cairo for full rich text + images.
//! Exports to "{title}.pdf" in temp dir, opens with system viewer.

use crate::document::{Document, DocumentOps};
use crate::render::{self, PrintParams, MeasureParams, content_height};
use gtk4::prelude::*;
use pangocairo::cairo;
use pango;

const MARGIN_LEFT: f64 = 36.0;
const MARGIN_RIGHT: f64 = 36.0;
const MARGIN_TOP: f64 = 50.0;
const MARGIN_BOTTOM: f64 = 36.0;

/// Print/export a document to PDF and open with system viewer.
/// Uses render_to_cairo for full styled output with images.
pub fn print_document(
    parent: &gtk4::Widget,
    doc: &Document,
    title: Option<&str>,
) {
    let doc_title = title.unwrap_or("Document");

    if doc.get_text_opt().is_none() {
        eprintln!("reditctrl: No text to print");
        return;
    }

    let print_op = gtk4::PrintOperation::new();
    print_op.set_allow_async(false);

    // PDF path
    let safe_title: String = doc_title.chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let pdf_path = std::env::temp_dir().join(format!("{}.pdf", safe_title));
    print_op.set_export_filename(pdf_path.to_str().unwrap_or("print.pdf"));

    // Shared state
    let title_owned = doc_title.to_string();
    let doc_ptr = doc as *const Document as usize; // SAFETY: doc lives for duration of print_op.run()
    let total_height_rc = std::rc::Rc::new(std::cell::Cell::new(0.0f64));
    let pango_ctx_rc: std::rc::Rc<std::cell::RefCell<Option<pango::Context>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));

    // begin-print: measure total content height, set page count
    {
        let th = total_height_rc.clone();
        let pctx = pango_ctx_rc.clone();
        let dp = doc_ptr;

        print_op.connect_begin_print(move |op, print_ctx| {
            let doc_ref = unsafe { &*(dp as *const Document) };
            let fontmap = pangocairo::FontMap::default();
            let ctx = fontmap.create_context();

            let pw = print_ctx.width();
            let content_w = (pw - MARGIN_LEFT - MARGIN_RIGHT) as i32;

            // Measure total content height using render module
            let mparams = MeasureParams {
                pango_ctx: &ctx,
                doc: doc_ref,
                width: content_w,
            };
            let total_h = content_height(&mparams) as f64;
            th.set(total_h);

            let ph = print_ctx.height();
            let page_content_h = ph - MARGIN_TOP - MARGIN_BOTTOM;
            let num_pages = ((total_h / page_content_h).ceil() as i32).max(1);
            op.set_n_pages(num_pages);

            *pctx.borrow_mut() = Some(ctx);
        });
    }

    // draw-page: render with full styles + images via render_to_cairo
    {
        let title_c = title_owned.clone();
        let pctx = pango_ctx_rc.clone();
        let dp = doc_ptr;

        print_op.connect_draw_page(move |_op, print_ctx, page_num| {
            let doc_ref = unsafe { &*(dp as *const Document) };
            let cr = print_ctx.cairo_context();
            let pw = print_ctx.width();
            let ph = print_ctx.height();
            let content_w = (pw - MARGIN_LEFT - MARGIN_RIGHT) as i32;
            let page_content_h = ph - MARGIN_TOP - MARGIN_BOTTOM;

            let pctx_borrow = pctx.borrow();
            let ctx = match pctx_borrow.as_ref() {
                Some(c) => c,
                None => return,
            };

            // Header
            cr.set_source_rgb(0.4, 0.4, 0.4);
            cr.set_font_size(9.0);
            let header = format!("{} — Page {}", title_c, page_num + 1);
            cr.move_to(MARGIN_LEFT, MARGIN_TOP - 22.0);
            let _ = cr.show_text(&header);

            // Separator line
            cr.set_source_rgb(0.7, 0.7, 0.7);
            cr.set_line_width(0.5);
            cr.move_to(MARGIN_LEFT, MARGIN_TOP - 10.0);
            cr.line_to(pw - MARGIN_RIGHT, MARGIN_TOP - 10.0);
            let _ = cr.stroke();

            // Content: use render_to_cairo for full styled rendering
            let skip = page_num as f64 * page_content_h;
            let params = PrintParams {
                cr: &cr,
                pango_ctx: ctx,
                doc: doc_ref,
                x: MARGIN_LEFT,
                y_start: MARGIN_TOP,
                width: content_w,
                skip_height: skip,
                max_height: page_content_h,
            };
            render::render_to_cairo(&params);

            // Footer
            cr.set_source_rgb(0.5, 0.5, 0.5);
            cr.set_font_size(8.0);
            let footer = format!("{}", page_num + 1);
            cr.move_to(pw / 2.0 - 5.0, ph - MARGIN_BOTTOM + 15.0);
            let _ = cr.show_text(&footer);
        });
    }

    // Run export
    let parent_window = parent
        .root()
        .and_then(|r: gtk4::Root| r.downcast::<gtk4::Window>().ok());

    match print_op.run(
        gtk4::PrintOperationAction::Export,
        parent_window.as_ref(),
    ) {
        Ok(_) => {
            if pdf_path.exists() {
                #[cfg(target_os = "macos")]
                { let _ = std::process::Command::new("open").arg(&pdf_path).spawn(); }
                #[cfg(target_os = "linux")]
                { let _ = std::process::Command::new("xdg-open").arg(&pdf_path).spawn(); }
            }
        }
        Err(e) => eprintln!("reditctrl: print/export failed: {}", e),
    }
}

// Stubs for API compat
pub struct PrintLayout;
impl PrintLayout {
    pub fn new(_: &pango::Context, _: &[String], _: f64, _: f64, _: f64) -> Self { Self }
    pub fn page_line_range(&self, _: i32) -> (usize, usize) { (0, 0) }
    pub fn header_text(&self, _: i32) -> String { String::new() }
    pub fn footer_text(&self, _: i32) -> String { String::new() }
}
pub fn render_page(_: &cairo::Context, _: &pango::Context, _: &PrintLayout, _: &[String], _: i32, _: f64, _: f64, _: f64) {}
