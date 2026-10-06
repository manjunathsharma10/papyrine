//! Fallback to the machine's real fonts through fontdb (lazy scan). Results depend on the
//! installed fonts, so assertions are about invariants, not specific fonts.

mod common;

use std::time::Instant;

use common::*;
use papyrine_annotate::*;

#[test]
fn real_system_fonts_cover_cjk_hebrew_arabic_thai_hindi() {
    let text = "日本語 한국어 中文 עברית العربية ไทย हिन्दी";
    let doc = open(build_pdf(1));
    let mut h = history();
    let mut cmd = AddAnnotation::text_box(
        0,
        [10.0, 100.0, 390.0, 300.0],
        TextStyle {
            size: 16.0,
            ..TextStyle::default()
        },
        text,
        AnnotProps::default(),
    );
    // The first call pays for the lazy font scan; time it separately from the second.
    let t0 = Instant::now();
    let scratch_doc = open(build_pdf(1));
    let mut cx = papyrine_ops::EditContext::new(&scratch_doc).unwrap();
    papyrine_ops::Command::apply(&mut cmd, &mut cx).unwrap();
    let first = t0.elapsed();
    let t1 = Instant::now();
    h.execute(&doc, Box::new(cmd.clone())).unwrap();
    let second = t1.elapsed();
    let report = cmd.font_report().unwrap().clone();
    eprintln!("first call (incl. system font scan) {first:?}, second {second:?}");
    eprintln!("fallback fonts: {:?}", report.fallbacks);
    eprintln!("restricted (not embedded): {:?}", report.restricted);
    eprintln!("missing: {:?}", report.missing);
    // Every distinct non-space character is accounted for exactly once: drawn with a bundled
    // or embedded system font, or reported as missing.
    let covered_by_fallback: String = report
        .fallbacks
        .iter()
        .flat_map(|f| f.chars.chars())
        .collect();
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        let bundled = papyrine_annotate::text::layout_with(
            &c.to_string(),
            FontFamily::Sans,
            false,
            12.0,
            100.0,
            false,
        );
        let ok = bundled.report.missing.is_empty();
        assert!(
            ok || covered_by_fallback.contains(c) || report.missing.contains(c),
            "{c:?} unaccounted for"
        );
    }
    // A restricted font's characters are only missing, never embedded.
    for r in &report.restricted {
        assert!(!report.fallbacks.iter().any(|f| f.font == r.font));
    }
    let bytes = write_bytes(&doc);
    qpdf_check(&bytes).unwrap();
    // Whatever was embedded extracts with Poppler.
    if have("pdftotext") {
        let (_d, p) = scratch("t.pdf", &bytes);
        let o = std::process::Command::new("pdftotext")
            .arg(&p)
            .arg("-")
            .output()
            .unwrap();
        let t = String::from_utf8_lossy(&o.stdout);
        for f in &report.fallbacks {
            for c in f.chars.chars() {
                assert!(
                    t.contains(c),
                    "Poppler text lacks {c:?} from {}: {t}",
                    f.font
                );
            }
        }
    }
    // The page still renders with ink where text was embedded.
    let r = pdfium_render(bytes, 0);
    let ink = (10..390)
        .flat_map(|x| (100..300).map(move |y| (x, y)))
        .filter(|&(x, y)| r.at(f64::from(x) + 0.5, f64::from(y) + 0.5) != WHITE)
        .count();
    assert!(ink > 100);
}
