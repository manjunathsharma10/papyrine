//! Golden-image match on 50 corpus pages (ROADMAP 1.8).
//!
//! `tests/golden/golden.tsv` holds, per page, an exact hash for the platform
//! and PDFium build it was recorded on and a coarse perceptual signature that
//! tolerates rasteriser differences (see `papyrine_render::golden`). No images
//! are stored. The corpus itself is fetched, not committed: without
//! `corpus/cache` the test skips, unless `PAPYRINE_REQUIRE_CORPUS` is set (CI).
//!
//! Re-record on the reference Mac with `PAPYRINE_BLESS=1`; add another
//! platform's hash (signature must already match) with `PAPYRINE_BLESS=hash`.

use papyrine_render::golden::*;
use papyrine_render::*;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// (corpus id, page index). `generated/` entries come from `tools/gen-corpus`.
const PAGES: [(&str, usize); 50] = [
    ("generated/typical-20p.pdf", 0),
    ("generated/typical-20p.pdf", 7),
    ("generated/typical-20p.pdf", 19),
    ("irs/f1040.pdf", 0),
    ("irs/f1040.pdf", 1),
    ("irs/f1040sa.pdf", 0),
    ("irs/f1040nr.pdf", 1),
    ("irs/fw4.pdf", 0),
    ("irs/fw4.pdf", 1),
    ("irs/fw9.pdf", 0),
    ("irs/f1040es.pdf", 0),
    ("irs/f8949.pdf", 0),
    ("irs/f4562.pdf", 0),
    ("irs/f990.pdf", 0),
    ("irs/f1065.pdf", 0),
    ("irs/f2848.pdf", 0),
    ("pdfium/testing/resources/text_form_color.pdf", 0),
    ("pdfium/testing/resources/text_form_multiple.pdf", 0),
    ("pdfium/testing/resources/combobox_form.pdf", 0),
    ("pdfium/testing/resources/multiple_form_types.pdf", 0),
    (
        "pdfium/testing/resources/annotation_highlight_square_with_ap.pdf",
        0,
    ),
    (
        "pdfium/testing/resources/annotation_highlight_long_content.pdf",
        0,
    ),
    ("pdfium/testing/resources/many_rectangles.pdf", 0),
    ("pdfium/testing/resources/rectangles_with_leaky_ctm.pdf", 0),
    (
        "pdfium/testing/resources/hello_world_2_pages_split_streams.pdf",
        1,
    ),
    ("pdfium/testing/resources/bug_1396264.pdf", 0),
    ("pdfdiff/NegativeFontSize/NegativeFontSize.pdf", 0),
    ("pdfdiff/LargeMitreLimit/LargeMitreLimit.pdf", 0),
    ("pdfdiff/TextClipModeChanges/TextClippingModeChanges.pdf", 0),
    (
        "pdfdiff/OverlappingGlyphClipping/OverlappingGlyphClipping.pdf",
        0,
    ),
    ("pdfdiff/Dashing-Degenerate/DegenerateDashing.pdf", 0),
    (
        "pdfdiff/Inline-Image-Abbreviations/InlineAbbreviations.pdf",
        0,
    ),
    ("pdfdiff/DefaultColorSpaces/DefaultRGBColourSpaces.pdf", 0),
    ("pdfdiff/ColorBurn-ColorDodge/ColorBurn.pdf", 0),
    ("pdfdiff/ColorBurn-ColorDodge/ColorDodge.pdf", 0),
    (
        "pdfdiff/Atomic-Fill+Stroke/SelfIntersecting-Transparency.pdf",
        0,
    ),
    ("pdfdiff/Atomic-Fill+Stroke/FillStrokeOrdering.pdf", 0),
    ("pdfdiff/VerticalText/VerticalText.pdf", 0),
    ("pdfdiff/LineCap-Degenerate/LineCap-Degenerate.pdf", 0),
    ("pdfdiff/Negative-DashPhase/Negative-DashPhase.pdf", 0),
    ("pdfminer/samples/simple1.pdf", 0),
    ("pdfminer/samples/jo.pdf", 0),
    ("pdfminer/samples/test_pattern_colors.pdf", 0),
    ("pdfminer/samples/font-size-test.pdf", 0),
    ("pdfjs/test/pdfs/colors.pdf", 0),
    ("pdfjs/test/pdfs/coons-allflags-withfunction.pdf", 0),
    ("pdfjs/test/pdfs/annotation-squiggly.pdf", 0),
    ("pdfjs/test/pdfs/colorspace_atan.pdf", 0),
    ("pdfjs/test/pdfs/bitmap-halftone.pdf", 0),
    ("pdfjs/test/pdfs/cff_bluescale_small_zones.pdf", 0),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus_path(id: &str) -> PathBuf {
    match id.strip_prefix("generated/") {
        Some(rest) => root().join("corpus/cache/generated").join(rest),
        None => root().join("corpus/cache/files").join(id),
    }
}

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/golden.tsv")
}

struct Entry {
    id: String,
    page: usize,
    w: u32,
    h: u32,
    /// `platform=hex` pairs.
    hashes: Vec<(String, String)>,
    sig: Vec<u8>,
}

fn parse(text: &str) -> (String, Vec<Entry>) {
    let mut pdfium = String::new();
    let mut v = Vec::new();
    for l in text.lines() {
        if let Some(r) = l.strip_prefix("# pdfium ") {
            pdfium = r.trim().into();
        }
        if l.starts_with('#') || l.is_empty() {
            continue;
        }
        let f: Vec<&str> = l.split('\t').collect();
        assert_eq!(f.len(), 6, "bad golden line: {l}");
        v.push(Entry {
            id: f[0].into(),
            page: f[1].parse().unwrap(),
            w: f[2].parse().unwrap(),
            h: f[3].parse().unwrap(),
            hashes: f[4]
                .split(';')
                .filter(|s| !s.is_empty())
                .map(|p| {
                    let (a, b) = p.split_once('=').unwrap();
                    (a.into(), b.into())
                })
                .collect(),
            sig: from_hex(f[5]).unwrap(),
        });
    }
    (pdfium, v)
}

fn render(lib: &std::sync::Arc<Library>, id: &str, page: usize) -> Tile {
    let base = open_mmap(&corpus_path(id)).unwrap_or_else(|e| panic!("{id}: {e}"));
    let mut d = Document::open(lib, base, &[], None).unwrap_or_else(|e| panic!("{id}: {e}"));
    d.render_preview(page, GOLDEN_EDGE, &mut never_cancel)
        .unwrap_or_else(|e| panic!("{id} page {page}: {e}"))
}

#[test]
fn golden_pages_match() {
    let lib = Library::global().expect("libpdfium: run tools/fetch-pdfium");
    if !corpus_path("generated/typical-20p.pdf").exists() && !corpus_path("irs/f1040.pdf").exists()
    {
        assert!(
            std::env::var_os("PAPYRINE_REQUIRE_CORPUS").is_none(),
            "corpus/cache is required (run corpus/fetch and tools/gen-corpus)"
        );
        eprintln!("golden: corpus not present, skipping");
        return;
    }
    let platform = platform_dir_name();
    let version = lib.version().unwrap_or_default();
    let bless = std::env::var("PAPYRINE_BLESS").ok();

    if bless.as_deref() == Some("1") {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "# Golden pages: id, page, width, height, platform=fnv1a64(RGBA) pairs, 16x16 mean-RGB signature."
        );
        let _ = writeln!(out, "# pdfium {version}");
        let _ = writeln!(out, "# edge {GOLDEN_EDGE}");
        for (id, page) in PAGES {
            let t = render(&lib, id, page);
            let _ = writeln!(
                out,
                "{id}\t{page}\t{}\t{}\t{platform}={:016x}\t{}",
                t.width,
                t.height,
                fnv1a64(&t.rgba),
                to_hex(&signature(&t))
            );
        }
        std::fs::create_dir_all(golden_path().parent().unwrap()).unwrap();
        std::fs::write(golden_path(), out).unwrap();
        eprintln!("golden: blessed {} pages", PAGES.len());
        return;
    }

    let text = std::fs::read_to_string(golden_path()).expect("tests/golden/golden.tsv");
    let (gold_pdfium, mut entries) = parse(&text);
    assert_eq!(
        entries.len(),
        PAGES.len(),
        "golden.tsv does not list the 50 pages"
    );
    let (mut exact, mut exact_skipped, mut worst_mean, mut worst_max) = (0, 0, 0.0f64, 0u8);
    let mut failures = Vec::new();
    for (e, (id, page)) in entries.iter_mut().zip(PAGES) {
        assert_eq!((e.id.as_str(), e.page), (id, page));
        let t = render(&lib, id, page);
        assert_eq!(
            (t.width, t.height),
            (e.w, e.h),
            "{id} p{page}: size changed"
        );
        let d = compare(&e.sig, &signature(&t)).unwrap();
        worst_mean = worst_mean.max(d.mean);
        worst_max = worst_max.max(d.max);
        if !d.within_tolerance() {
            failures.push(format!("{id} p{page}: perceptual {d:?}"));
        }
        let hash = format!("{:016x}", fnv1a64(&t.rgba));
        match e.hashes.iter().find(|(p, _)| p == platform) {
            Some((_, want)) if gold_pdfium == version => {
                if *want == hash {
                    exact += 1;
                } else {
                    failures.push(format!(
                        "{id} p{page}: exact hash {hash} != {want} on {platform} (PDFium {version})"
                    ));
                }
            }
            _ => {
                exact_skipped += 1;
                if bless.as_deref() == Some("hash") && d.within_tolerance() {
                    e.hashes.retain(|(p, _)| p != platform);
                    e.hashes.push((platform.into(), hash));
                }
            }
        }
    }
    if bless.as_deref() == Some("hash") {
        let mut out = String::new();
        for l in text.lines().filter(|l| l.starts_with('#')) {
            let _ = writeln!(out, "{l}");
        }
        for e in &entries {
            let h: Vec<String> = e.hashes.iter().map(|(p, v)| format!("{p}={v}")).collect();
            let _ = writeln!(
                out,
                "{}\t{}\t{}\t{}\t{}\t{}",
                e.id,
                e.page,
                e.w,
                e.h,
                h.join(";"),
                to_hex(&e.sig)
            );
        }
        std::fs::write(golden_path(), out).unwrap();
    }
    eprintln!(
        "golden: 50 pages, perceptual worst mean {worst_mean:.3} max {worst_max}; exact hash matched {exact}, not applicable {exact_skipped} ({platform}, PDFium {version}, recorded with {gold_pdfium})"
    );
    assert!(
        failures.is_empty(),
        "golden mismatches:\n{}",
        failures.join("\n")
    );
}
