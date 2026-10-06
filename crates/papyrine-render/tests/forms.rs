//! Widget appearances are drawn through the form-fill environment (FPDF_FFLDraw),
//! which is used for drawing only.

use papyrine_render::*;

/// One-page PDF with a text-field widget whose appearance stream paints a red square.
fn form_pdf(with_acroform: bool) -> Vec<u8> {
    let mut objs: Vec<String> = Vec::new();
    objs.push(format!(
        "<< /Type /Catalog /Pages 2 0 R {} >>",
        if with_acroform {
            "/AcroForm << /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) >>"
        } else {
            ""
        }
    ));
    objs.push("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into());
    objs.push(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R] >>"
            .into(),
    );
    let content = "0 0 1 rg 10 10 30 30 re f";
    objs.push(format!(
        "<< /Length {} >>\nstream\n{content}\nendstream",
        content.len()
    ));
    objs.push(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (x) /Rect [100 100 150 150] /F 4 /P 3 0 R /AP << /N 6 0 R >> >>"
            .into(),
    );
    let ap = "1 0 0 rg 0 0 50 50 re f";
    objs.push(format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 50 50] /Length {} >>\nstream\n{ap}\nendstream",
        ap.len()
    ));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for o in offs {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .bytes(),
    );
    out
}

fn open(bytes: Vec<u8>) -> Document {
    let lib = Library::global().expect("libpdfium: run tools/fetch-pdfium");
    Document::open(&lib, bytes_from_vec(bytes), &[], None).unwrap()
}

#[test]
fn widget_appearance_is_drawn_by_ffldraw() {
    let mut d = open(form_pdf(true));
    assert!(d.has_form_env());
    let t = d
        .render_tile(0, 0, TileCoord { col: 0, row: 0 }, &mut never_cancel)
        .unwrap();
    // Page content (blue square) at device x 10..40, y 160..190.
    assert_eq!(t.pixel(25, 175), [0, 0, 255, 255]);
    // Widget Rect [100 100 150 150] -> device x 100..150, y 50..100: red from the AP stream.
    assert_eq!(t.pixel(125, 75), [255, 0, 0, 255]);
    // No highlight tint outside the widget.
    assert_eq!(t.pixel(175, 25), [255, 255, 255, 255]);
}

#[test]
fn no_form_env_without_acroform_and_no_widget_pixels() {
    let mut d = open(form_pdf(false));
    assert!(!d.has_form_env());
    let t = d
        .render_tile(0, 0, TileCoord { col: 0, row: 0 }, &mut never_cancel)
        .unwrap();
    assert_eq!(t.pixel(25, 175), [0, 0, 255, 255]);
    // FPDF_RenderPageBitmap alone does not draw widgets.
    assert_eq!(t.pixel(125, 75), [255, 255, 255, 255]);
}

#[test]
fn page_cache_close_and_reload_with_forms() {
    let mut d = open(form_pdf(true));
    for _ in 0..3 {
        d.render_preview(0, 100, &mut never_cancel).unwrap();
        d.close_pages();
        assert!(d.cached_pages().is_empty());
    }
    d.reopen(&[]).unwrap();
    assert!(d.has_form_env());
    d.render_preview(0, 100, &mut never_cancel).unwrap();
}
