//! Filled values must display the same in PDFium (papyrine-render), Poppler and pdf.js.
//! Poppler, pdf.js and tesseract are test-only oracles.

mod common;

use common::*;
use papyrine_fill::*;

fn filled_sample() -> Vec<u8> {
    let doc = open(sample_form());
    let mut h = history();
    for (f, v) in [
        ("name", "Alice Smith"),
        ("qty", "1234.5"),
        ("price", "10"),
        ("date", "03/05/2024"),
        (
            "notes",
            "The quick brown fox jumps over the lazy dog and keeps running far away",
        ),
        ("comb", "AB12"),
        ("centered", "Middle"),
        ("right", "Edge"),
    ] {
        h.execute(&doc, Box::new(SetTextValue::new(f, v))).unwrap();
    }
    h.execute(&doc, Box::new(SetChoice::new("country", vec!["it".into()])))
        .unwrap();
    h.execute(
        &doc,
        Box::new(SetChoice::new("langs", vec!["Ada".into(), "C".into()])),
    )
    .unwrap();
    h.execute(&doc, Box::new(SelectRadio::by_export("pick", "y")))
        .unwrap();
    h.execute(&doc, Box::new(ToggleCheckbox::new("bare", Some(true))))
        .unwrap();
    write(&doc)
}

const EXPECT: &[(&str, &str)] = &[
    ("name", "AliceSmith"),
    ("comb", "AB12"),
    ("qty", "1,234.50"),
    ("price", "$10.00"),
    ("total", "1,244.50"),
    ("date", "5Mar2024"),
    ("country", "Italy"),
    ("centered", "Middle"),
    ("right", "Edge"),
];

#[test]
fn pdfjs_draws_and_reads_the_values() {
    if !pdfjs_available() {
        eprintln!(
            "pdf.js oracle not installed (npm install pdfjs-dist@4 in target/fill-oracle); skipped"
        );
        return;
    }
    let bytes = filled_sample();
    let w = pdfjs_widgets(&bytes);
    for (name, shown) in EXPECT {
        let a = w
            .iter()
            .find(|a| a["fieldName"] == *name)
            .unwrap_or_else(|| panic!("pdf.js lists {name}"));
        assert_eq!(squash(a["text"].as_str().unwrap()), *shown, "{name}: {a}");
    }
    let notes = w.iter().find(|a| a["fieldName"] == "notes").unwrap();
    assert_eq!(
        squash(notes["text"].as_str().unwrap()),
        squash("The quick brown fox jumps over the lazy dog and keeps running far away")
    );
    // Raw values, not display text, are what pdf.js reads from /V.
    let qty = w.iter().find(|a| a["fieldName"] == "qty").unwrap();
    assert_eq!(qty["fieldValue"], "1234.5");
    // Radio and checkbox marks: the selected radio draws the ZapfDingbats circle glyph.
    let radios: Vec<_> = w.iter().filter(|a| a["fieldName"] == "pick").collect();
    assert_eq!(radios.iter().filter(|a| a["text"] == "l").count(), 1);
    let bare = w.iter().find(|a| a["fieldName"] == "bare").unwrap();
    assert_eq!(bare["text"], "8");
}

#[test]
fn poppler_extracts_the_values() {
    if !tool_available("pdftotext") {
        eprintln!("poppler not installed; skipped");
        return;
    }
    let bytes = filled_sample();
    let text = squash(&pdftotext(&bytes));
    for (name, shown) in EXPECT {
        assert!(
            text.contains(shown),
            "{name}: `{shown}` missing from Poppler text {text:?}"
        );
    }
    assert!(text.contains("quickbrownfox") && text.contains("lazydog"));
    // A list box shows its first rows; the selected rows are highlighted but listed.
    assert!(text.contains("Ada") && text.contains("Basic"));
}

#[test]
fn pdfium_and_poppler_rasters_agree_and_ocr_reads_them() {
    let bytes = filled_sample();
    let pdfium = render_page(&bytes, 0, 1584);
    let before = render_page(&sample_form(), 0, 1584);
    // Every filled field has more ink than the empty form in PDFium.
    for (name, rect) in [
        ("name", [50.0, 700.0, 250.0, 720.0]),
        ("comb", [50.0, 660.0, 250.0, 680.0]),
        ("notes", [50.0, 560.0, 250.0, 640.0]),
        ("centered", [300.0, 700.0, 500.0, 720.0]),
        ("right", [300.0, 660.0, 500.0, 680.0]),
        ("qty", [50.0, 480.0, 150.0, 500.0]),
        ("total", [50.0, 420.0, 150.0, 440.0]),
    ] {
        let (a, b) = (
            ink_in(&pdfium, 2.0, 792.0, rect),
            ink_in(&before, 2.0, 792.0, rect),
        );
        assert!(a > b + 30, "{name}: PDFium ink {a} vs empty {b}");
    }
    if tool_available("pdftoppm") {
        // Poppler's raster must put ink where PDFium does (same stream, two renderers).
        let (w, h, rgb) = poppler_ppm(&bytes, 144);
        assert_eq!((w, h), (pdfium.width, pdfium.height));
        let mut both = 0usize;
        let mut either = 0usize;
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                let pop = rgb[i] < 160 || rgb[i + 1] < 160 || rgb[i + 2] < 160;
                let p = pdfium.pixel(x, y);
                let pdf = p[0] < 160 || p[1] < 160 || p[2] < 160;
                both += usize::from(pop && pdf);
                either += usize::from(pop || pdf);
            }
        }
        let iou = both as f64 / either as f64;
        assert!(iou > 0.55, "Poppler/PDFium ink overlap {iou:.2}");
    }
    if tool_available("tesseract") {
        // OCR each field's pixels (inside its border) from the PDFium raster.
        for (rect, word) in [
            ([50.0, 700.0, 250.0, 720.0], "AliceSmith"),
            ([300.0, 520.0, 500.0, 540.0], "Italy"),
            ([50.0, 390.0, 150.0, 410.0], "5Mar2024"),
            ([50.0, 480.0, 150.0, 500.0], "1,234.50"),
            ([50.0, 420.0, 150.0, 440.0], "1,244.50"),
            ([300.0, 700.0, 500.0, 720.0], "Middle"),
        ] {
            let crop = crop_tile(
                &pdfium,
                2.0,
                792.0,
                [rect[0] + 2.0, rect[1] + 2.0, rect[2] - 2.0, rect[3] - 2.0],
            );
            let got = squash(&ocr_line(&crop));
            assert!(
                got.contains(word),
                "OCR of PDFium raster at {rect:?}: wanted {word}, got {got:?}"
            );
        }
        let notes = crop_tile(&pdfium, 2.0, 792.0, [52.0, 562.0, 248.0, 638.0]);
        let got = squash(&ocr(&notes)).to_lowercase();
        assert!(
            got.contains("quickbrownfox") && got.contains("lazydog"),
            "notes OCR: {got}"
        );
    }
}
