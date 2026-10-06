//! Interop: qpdf --check, pdf.js (node, test-only), Poppler (test-only) and PDFium.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::*;
use papyrine_annotate::*;
use serde_json::Value;

fn js_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/js")
}

/// Run the pdf.js oracle; `None` when node or `npm install` (in tests/js) is missing.
fn pdfjs(bytes: &[u8], page: usize, points: &[[f64; 2]]) -> Option<Value> {
    pdfjs_regions(bytes, page, points, &[])
}

fn pdfjs_regions(
    bytes: &[u8],
    page: usize,
    points: &[[f64; 2]],
    regions: &[[f64; 4]],
) -> Option<Value> {
    if !have("node") || !js_dir().join("node_modules/pdfjs-dist").is_dir() {
        assert!(
            !tools_required(),
            "pdf.js oracle missing: run `npm install` in crates/papyrine-annotate/tests/js"
        );
        eprintln!("pdf.js oracle not installed: skipping");
        return None;
    }
    let (_d, p) = scratch("in.pdf", bytes);
    let o = Command::new("node")
        .current_dir(js_dir())
        .arg("check.mjs")
        .arg(&p)
        .arg(page.to_string())
        .arg(serde_json::to_string(points).unwrap())
        .arg(serde_json::to_string(regions).unwrap())
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "pdf.js failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    Some(serde_json::from_slice(&o.stdout).expect("oracle json"))
}

fn all_types() -> papyrine_cos::Document {
    let doc = open(build_pdf(1));
    let mut h = history();
    let p = || AnnotProps::default().with_author("Ada");
    let cmds: Vec<AddAnnotation> = vec![
        AddAnnotation::highlight(
            0,
            vec![Quad::from_rect(20.0, 350.0, 120.0, 380.0)],
            p().with_contents("hl"),
        ),
        AddAnnotation::underline(
            0,
            vec![Quad::from_rect(20.0, 300.0, 120.0, 330.0)],
            p().with_width(1.5),
        ),
        AddAnnotation::strike_out(
            0,
            vec![Quad::from_rect(140.0, 300.0, 240.0, 330.0)],
            p().with_width(1.5),
        ),
        AddAnnotation::squiggly(0, vec![Quad::from_rect(260.0, 300.0, 360.0, 330.0)], p()),
        AddAnnotation::sticky_note(
            0,
            20.0,
            280.0,
            p().with_contents("a note")
                .with_color(Color::rgb(1.0, 0.85, 0.0)),
        ),
        AddAnnotation::text_box(
            0,
            [180.0, 200.0, 380.0, 280.0],
            TextStyle::default(),
            "Boxed Привет",
            p().with_fill(Color::rgb(1.0, 1.0, 0.85)).with_width(1.0),
        ),
        AddAnnotation::pen(
            0,
            vec![vec![[20.0, 200.0], [40.0, 240.0], [60.0, 210.0]]],
            p().with_width(3.0),
        ),
        AddAnnotation::rectangle(
            0,
            [20.0, 120.0, 100.0, 180.0],
            p().with_width(3.0).with_fill(Color::rgb(0.8, 1.0, 0.8)),
        ),
        AddAnnotation::oval(0, [120.0, 120.0, 200.0, 180.0], p().with_width(3.0)),
        AddAnnotation::line(0, [220.0, 130.0], [300.0, 180.0], p().with_width(2.0)),
        AddAnnotation::arrow(0, [320.0, 130.0], [380.0, 180.0], p().with_width(2.0)),
    ];
    for c in cmds {
        run(&doc, &mut h, c);
    }
    doc
}

#[test]
fn pdfjs_reads_and_renders_every_type() {
    let doc = all_types();
    let bytes = write_bytes(&doc);
    qpdf_check(&bytes).unwrap();
    let points = [
        [115.0, 352.0], // highlight
        [60.0, 150.0],  // rectangle fill
        [260.0, 155.0], // line
        [370.0, 205.0], // text box fill
        [23.5, 276.5],  // note body
        [160.0, 150.0], // oval interior (page white)
        [250.0, 100.0], // empty page area
    ];
    let Some(j) = pdfjs(&bytes, 0, &points) else {
        return;
    };
    let subtypes: Vec<&str> = j["annotations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["subtype"].as_str().unwrap())
        .collect();
    assert_eq!(
        subtypes,
        [
            "Highlight",
            "Underline",
            "StrikeOut",
            "Squiggly",
            "Text",
            "FreeText",
            "Ink",
            "Square",
            "Circle",
            "Line",
            "Line"
        ]
    );
    for a in j["annotations"].as_array().unwrap() {
        assert!(a["hasAppearance"].as_bool().unwrap(), "{a}");
        assert_eq!(a["title"], "Ada");
    }
    assert_eq!(j["annotations"][0]["contents"], "hl");
    assert_eq!(j["annotations"][5]["contents"], "Boxed Привет");
    assert_eq!(
        j["annotations"][0]["color"],
        serde_json::json!([255, 255, 0])
    );
    assert_eq!(
        j["annotations"][0]["rect"],
        serde_json::json!([20, 350, 120, 380])
    );
    let px: Vec<[i64; 3]> = serde_json::from_value(j["pixels"].clone()).unwrap();
    let near = |got: [i64; 3], want: [i64; 3], tol: i64| {
        got.iter().zip(want).all(|(a, b)| (a - b).abs() <= tol)
    };
    assert!(near(px[0], [255, 255, 0], 6), "highlight {:?}", px[0]);
    assert!(near(px[1], [204, 255, 204], 6), "rectangle {:?}", px[1]);
    assert!(
        px[2][0] > 230 && px[2][1] < 130 && px[2][2] < 130,
        "line {:?}",
        px[2]
    );
    assert!(near(px[3], [255, 255, 217], 6), "text box {:?}", px[3]);
    assert!(near(px[4], [255, 217, 0], 30), "note {:?}", px[4]);
    assert!(near(px[5], [255, 255, 255], 0), "oval interior {:?}", px[5]);
    assert!(near(px[6], [255, 255, 255], 0), "empty {:?}", px[6]);
    assert!(j["opCount"].as_u64().unwrap() > 100);
}

#[test]
fn all_renderers_agree_on_the_gallery() {
    let doc = all_types();
    let bytes = write_bytes(&doc);
    let a = pdfium_render(bytes.clone(), 0);
    let Some(b) = pdftoppm(&bytes, 0) else { return };
    // Same geometry: the set of non-white pixels differs only along anti-aliased edges.
    let ink = |r: &Raster| -> Vec<bool> {
        r.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[0] < 128 || p[1] < 128 || p[2] < 128)
            .collect()
    };
    let (ia, ib) = (ink(&a), ink(&b));
    assert_eq!((a.w, a.h), (b.w, b.h));
    let diff = ia.iter().zip(&ib).filter(|(x, y)| x != y).count();
    let total = ia.iter().filter(|x| **x).count();
    assert!(total > 2_000);
    assert!(
        (diff as f64) < (total as f64) * 0.12,
        "PDFium and Poppler disagree on {diff} of {total} ink pixels"
    );
}

#[test]
fn pdfjs_sees_the_edited_annotation_after_update_and_erase() {
    let doc = open(build_pdf(1));
    let mut h = history();
    let s = run(
        &doc,
        &mut h,
        AddAnnotation::pen(
            0,
            vec![vec![[50.0, 100.0], [350.0, 100.0]]],
            AnnotProps::default()
                .with_width(8.0)
                .with_color(Color::rgb(0.0, 0.0, 1.0)),
        ),
    );
    let t = AnnotRef::id(s.created[0]);
    run(
        &doc,
        &mut h,
        EraseInk::new(0, t.clone(), vec![[200.0, 90.0], [200.0, 110.0]], 10.0),
    );
    let bytes = write_bytes(&doc);
    qpdf_check(&bytes).unwrap();
    let r = pdfium_render(bytes.clone(), 0);
    expect_px(&r, 100.0, 100.0, [0, 0, 255]);
    expect_px(&r, 300.0, 100.0, [0, 0, 255]);
    expect_px(&r, 200.0, 100.0, [255, 255, 255]);
    if let Some(j) = pdfjs(&bytes, 0, &[[100.0, 100.0], [200.0, 100.0], [300.0, 100.0]]) {
        let px: Vec<[i64; 3]> = serde_json::from_value(j["pixels"].clone()).unwrap();
        assert_eq!(px[0], [0, 0, 255]);
        assert_eq!(px[1], [255, 255, 255]);
        assert_eq!(px[2], [0, 0, 255]);
        assert_eq!(j["annotations"][0]["inkLists"].as_array().unwrap().len(), 2);
    }
    if let Some(p) = pdftoppm(&bytes, 0) {
        expect_px(&p, 100.0, 100.0, [0, 0, 255]);
        expect_px(&p, 200.0, 100.0, [255, 255, 255]);
    }
}

fn expect_px(r: &Raster, x: f64, y: f64, want: [u8; 3]) {
    let got = r.at(x, y);
    assert!(
        is_near(got, want, 12),
        "pixel ({x},{y}) {got:?} != {want:?}"
    );
}

#[test]
fn pdfjs_renders_embedded_text_box_fonts() {
    use papyrine_annotate::text::use_only_fonts;
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fonts");
    use_only_fonts(vec![
        std::fs::read(d.join("NotoSansJP-Test.otf")).unwrap(),
        std::fs::read(d.join("NotoSansHebrew-Test.ttf")).unwrap(),
    ]);
    let doc = open(build_pdf(1));
    let mut h = history();
    let boxes = [
        (
            [20.0, 300.0, 380.0, 340.0],
            "Plain Latin text, Привет, Γειά",
        ),
        ([20.0, 240.0, 380.0, 280.0], "日本語のテキスト"),
        (
            [20.0, 180.0, 380.0, 220.0],
            "abc \u{5E9}\u{5DC}\u{5D5}\u{5DD} def",
        ),
        ([20.0, 100.0, 380.0, 140.0], ""),
    ];
    for (rect, text) in boxes {
        run(
            &doc,
            &mut h,
            AddAnnotation::text_box(
                0,
                rect,
                TextStyle {
                    size: 20.0,
                    ..TextStyle::default()
                },
                text,
                AnnotProps::default(),
            ),
        );
    }
    let bytes = write_bytes(&doc);
    qpdf_check(&bytes).unwrap();
    let regions: Vec<[f64; 4]> = boxes.iter().map(|b| b.0).collect();
    let pdfium = pdfium_render(bytes.clone(), 0);
    let ink = |r: &[f64; 4]| {
        let mut n = 0;
        for y in (r[1] as i64)..(r[3] as i64) {
            for x in (r[0] as i64)..(r[2] as i64) {
                if pdfium.at(x as f64 + 0.5, y as f64 + 0.5) != WHITE {
                    n += 1;
                }
            }
        }
        n
    };
    if let Some(pop) = pdftoppm(&bytes, 0) {
        for (i, r) in regions.iter().take(3).enumerate() {
            let mut n = 0;
            for y in (r[1] as i64)..(r[3] as i64) {
                for x in (r[0] as i64)..(r[2] as i64) {
                    if pop.at(x as f64 + 0.5, y as f64 + 0.5) != WHITE {
                        n += 1;
                    }
                }
            }
            assert!(n > 150, "Poppler drew box {i}: {n}");
        }
    }
    let Some(j) = pdfjs_regions(&bytes, 0, &[], &regions) else {
        return;
    };
    let counts: Vec<usize> = serde_json::from_value(j["regions"].clone()).unwrap();
    for (i, r) in regions.iter().enumerate() {
        eprintln!("box {i}: pdf.js ink {} / pdfium ink {}", counts[i], ink(r));
        if i < 3 {
            assert!(counts[i] > 150, "pdf.js drew box {i}: {}", counts[i]);
            let (a, b) = (counts[i] as f64, ink(r) as f64);
            assert!(
                (a - b).abs() / b < 0.35,
                "box {i}: pdf.js {a} vs pdfium {b}"
            );
        } else {
            assert_eq!(counts[i], 0);
        }
    }
}
