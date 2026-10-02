//! End-to-end: PDFium text extraction (papyrine-render) -> search.
//! Skipped when libpdfium or the corpus cache is absent.
mod common;
use common::opts;
use papyrine_core::{CancelToken, Rect};
use papyrine_render::{Document, Library, open_mmap};
use papyrine_text::*;
use std::path::PathBuf;

fn open(rel: &str) -> Option<Document> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/cache/files")
        .join(rel);
    if !p.exists() {
        eprintln!("skip {rel}: corpus not fetched");
        return None;
    }
    let Ok(lib) = Library::global() else {
        eprintln!("skip {rel}: libpdfium missing");
        return None;
    };
    Some(Document::open(&lib, open_mmap(&p).ok()?, &[], None).expect("open"))
}

fn page_input(d: &mut Document, i: usize) -> PageInput {
    let t = d.page_text(i).unwrap();
    PageInput {
        page: i,
        text: PageChars::new(
            t.chars.iter().map(|c| c.ch).collect(),
            t.chars
                .iter()
                .map(|c| Rect::new(c.loose_left, c.loose_bottom, c.loose_right, c.loose_top))
                .collect(),
        ),
        extras: vec![],
    }
}

fn search(d: &mut Document, q: &str, o: &SearchOptions) -> Vec<SearchHit> {
    let mut hits = vec![];
    let n = d.page_count();
    let pages = (0..n).map(|i| page_input(d, i));
    search_document(pages, q, o, &CancelToken::new(), |e| {
        if let SearchEvent::Hit(h) = e {
            hits.push(h)
        }
    });
    hits
}

fn text_of(d: &mut Document, h: &SearchHit) -> String {
    let t = d.page_text(h.page).unwrap();
    t.chars[h.range.clone()].iter().map(|c| c.ch).collect()
}

macro_rules! need {
    ($d:ident = $rel:expr) => {
        let Some(mut $d) = open($rel) else { return };
    };
}

#[test]
fn hebrew_mirrored() {
    need!(d = "pdfium/testing/resources/hebrew_mirrored.pdf");
    let h = search(&mut d, "בנימין", &opts());
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].quads.len(), 1);
    // RTL: the first logical char is the right-most.
    assert_eq!(h[0].range, 0..6);
    let b = h[0].quads[0].bounds();
    assert!(b.x1 > 170.0 && b.x0 < 151.0, "{b:?}");
}

#[test]
fn hebrew_paragraph() {
    need!(d = "pdfjs/test/pdfs/issue11656.pdf");
    let h = search(&mut d, "שלום עולם", &opts());
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].snippet.matched, "שלום עולם");
    assert_eq!(
        h[0].snippet.after.trim_start().split(' ').next(),
        Some("מה")
    );
    assert_eq!(search(&mut d, "שלום", &opts()).len(), 2);
    assert_eq!(search(&mut d, "מבקר המדינה", &opts()).len(), 1);
    // spans a line break: two quads
    let h = search(&mut d, "כמו כל הילדים", &opts());
    assert_eq!(h.len(), 1);
}

#[test]
fn arabic_lines() {
    need!(d = "pdfjs/test/pdfs/ArabicCIDTrueType.pdf");
    assert_eq!(search(&mut d, "الخطوط العربية", &opts()).len(), 4);
    assert_eq!(
        search(
            &mut d,
            "العربية",
            &SearchOptions {
                whole_word: true,
                ..opts()
            }
        )
        .len(),
        4
    );
    // across the \r\n between lines 1 and 2
    let h = search(&mut d, "العربية انواع", &opts());
    assert_eq!(h.len(), 3);
    assert_eq!(h[0].quads.len(), 2, "one quad per line");
    assert!(
        h[0].quads[0].bounds().y0 > h[0].quads[1].bounds().y1 - 1.0,
        "line 1 is above line 2"
    );
    assert_eq!(text_of(&mut d, &h[0]), "العربية\r\nانواع");
    // diacritic-insensitive on text without diacritics changes nothing
    let di = SearchOptions {
        diacritic_insensitive: true,
        ..opts()
    };
    assert_eq!(search(&mut d, "الْخطوط", &di).len(), 4);
    assert!(search(&mut d, "الْخطوط", &opts()).is_empty());
}

#[test]
fn multilingual_hello_world() {
    need!(d = "pypdf/resources/hello-world.pdf");
    let o = opts();
    assert_eq!(search(&mut d, "مرحبا بالعالم", &o).len(), 1);
    assert_eq!(search(&mut d, "بالعالم", &o).len(), 1);
    assert_eq!(search(&mut d, "ПРИВЕТ", &o).len(), 1);
    assert_eq!(
        search(
            &mut d,
            "мир",
            &SearchOptions {
                whole_word: true,
                ..opts()
            }
        )
        .len(),
        1
    );
    assert_eq!(search(&mut d, "hello world", &o).len(), 1);
    assert_eq!(search(&mut d, "你好世界", &o).len(), 1);
    assert_eq!(search(&mut d, "こんにちは", &o).len(), 1);
    assert_eq!(
        search(&mut d, "世界", &o).len(),
        2,
        "Chinese and Japanese lines"
    );
    // Thai with tone/vowel marks, exact and diacritic-insensitive
    assert_eq!(search(&mut d, "สวัสดี", &o).len(), 1);
    let di = SearchOptions {
        diacritic_insensitive: true,
        ..opts()
    };
    assert_eq!(search(&mut d, "สวสด", &di).len(), 1);
    assert!(search(&mut d, "สวสด", &o).is_empty());
}

#[test]
fn cjk_documents() {
    need!(d = "pikepdf/tests/resources/pdfa/fpdf2_cjk.pdf");
    let h = search(&mut d, "日本語", &opts());
    assert_eq!(h.len(), 1);
    let b = h[0].quads[0].bounds();
    assert!(
        (b.x0 - 100.0).abs() < 0.5 && (b.x1 - 180.0).abs() < 0.5,
        "{b:?}"
    );
    assert_eq!(
        search(&mut d, "ﾃｷｽﾄ", &opts()).len(),
        1,
        "halfwidth query folds to fullwidth"
    );

    need!(j = "pdfminer/samples/jo.pdf");
    assert_eq!(
        search(&mut j, "第四次延長のなかで主張されます", &opts()).len(),
        1
    );
    // Line break inside a phrase.
    let h = search(&mut j, "心象スケッチです", &opts());
    assert_eq!(h.len(), 1);
    assert!(search(&mut j, "電燈", &opts()).len() >= 3);

    need!(k = "pdfjs/test/pdfs/issue16538.pdf");
    assert!(!search(&mut k, "リスク中立確率", &opts()).is_empty());
    let h = search(&mut k, "リスク中立確率は一意", &opts());
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].quads.len(), 2, "wraps across two lines");
}

#[test]
fn pdfium_hyphen_marker_and_english() {
    need!(d = "pdfminer/samples/nonfree/naacl06-shinyama.pdf");
    // "Ex\u{2}isting" in the extracted text: PDFium's generated hyphen.
    let h = search(&mut d, "existing IE systems", &opts());
    assert!(!h.is_empty());
    assert_eq!(h[0].page, 0);
    assert!(
        h[0].snippet
            .matched
            .eq_ignore_ascii_case("existing IE systems"),
        "{:?}",
        h[0].snippet
    );
    assert!(search(&mut d, "preemptive information extraction", &opts()).len() >= 3);
    assert!(
        !search(
            &mut d,
            "Preemptive Information Extraction",
            &SearchOptions {
                case_sensitive: true,
                ..opts()
            }
        )
        .is_empty()
    );
    assert_eq!(search(&mut d, "zzzzqqqq", &opts()).len(), 0);
}
