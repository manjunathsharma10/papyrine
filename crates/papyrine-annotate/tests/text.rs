//! Text boxes: extraction, ToUnicode, font fallback and the fsType rule, RTL and CJK.

mod common;

use std::collections::BTreeMap;
use std::process::Command;

use common::*;
use papyrine_annotate::text::use_only_fonts;
use papyrine_annotate::*;
use papyrine_content::{Object as C, parse};
use papyrine_cos::{DecodeLevel, Document, Object};

fn fonts_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fonts")
}

/// Hermetic font fallback: only the test fonts below, no system scan.
fn install_test_fonts() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let d = fonts_dir();
        let load = |n: &str| std::fs::read(d.join(n)).unwrap();
        use_only_fonts(vec![
            load("NotoSansHebrew-Test.ttf"),
            load("NotoSansArabic-Test.ttf"),
            load("NotoSansJP-Test.otf"),
            load("PapyrineRestricted-Test.ttf"),
        ]);
    });
}

fn box_cmd(text: &str, size: f64) -> AddAnnotation {
    AddAnnotation::text_box(
        0,
        [20.0, 100.0, 380.0, 300.0],
        TextStyle {
            size,
            ..TextStyle::default()
        },
        text,
        AnnotProps::default(),
    )
}

/// Run the command and hand back the document plus the font report.
fn add(cmd: AddAnnotation) -> (Document, FontReport) {
    let doc = open(build_pdf(1));
    let mut h = history();
    // The report is computed by `apply`; the same text analysed up front gives the same answer.
    let (Geometry::TextBox { style, .. }, Some(text)) = (&cmd.geometry, &cmd.props.contents) else {
        unreachable!("text boxes only");
    };
    let report = papyrine_annotate::text::analyze(text, style);
    h.execute(&doc, Box::new(cmd.clone())).unwrap();
    // `apply` on a scratch document exposes the command's own report.
    let mut c2 = cmd;
    let scratch = open(build_pdf(1));
    let mut cx = papyrine_ops::EditContext::new(&scratch).unwrap();
    papyrine_ops::Command::apply(&mut c2, &mut cx).unwrap();
    assert_eq!(c2.font_report(), Some(&report));
    (doc, report)
}

/// The AP stream of the first annotation, and its font resources.
fn ap_of(doc: &Document) -> (Object, Object) {
    let a = page_annots(doc, 0).remove(0);
    let n = a.dict_get("AP").unwrap().dict_get("N").unwrap();
    let fonts = n
        .stream_dict()
        .unwrap()
        .dict_get("Resources")
        .unwrap()
        .dict_get("Font")
        .unwrap();
    (n, fonts)
}

/// Parse a ToUnicode CMap stream: CID -> text.
fn read_to_unicode(font: &Object) -> BTreeMap<u16, String> {
    let tu = font.dict_get("ToUnicode").unwrap();
    let text = String::from_utf8(tu.stream_decoded(DecodeLevel::All).unwrap().to_vec()).unwrap();
    let mut m = BTreeMap::new();
    let mut in_bf = false;
    for l in text.lines() {
        let l = l.trim();
        if l.ends_with("beginbfchar") {
            in_bf = true;
        } else if l == "endbfchar" {
            in_bf = false;
        } else if in_bf && l.starts_with('<') && l.matches('<').count() == 2 {
            let parts: Vec<&str> = l
                .split(['<', '>'])
                .filter(|s| !s.trim().is_empty())
                .collect();
            let cid = u16::from_str_radix(parts[0], 16).unwrap();
            let units: Vec<u16> = parts[1]
                .as_bytes()
                .chunks(4)
                .map(|c| u16::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
                .collect();
            m.insert(cid, String::from_utf16(&units).unwrap());
        }
    }
    m
}

/// Independent decode of the appearance stream: shown strings through each font's ToUnicode,
/// joined per line (a line is one BT/ET block).
fn decode_ap_text(doc: &Document) -> Vec<String> {
    let (ap, fonts) = ap_of(doc);
    let maps: BTreeMap<String, BTreeMap<u16, String>> = fonts
        .dict_keys()
        .unwrap()
        .into_iter()
        .map(|k| {
            let name = String::from_utf8(k).unwrap();
            let f = fonts.dict_get(&name).unwrap();
            (name, read_to_unicode(&f))
        })
        .collect();
    let content = ap.stream_decoded(DecodeLevel::All).unwrap().to_vec();
    let parsed = parse(&content);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    let (mut lines, mut cur, mut font) = (Vec::new(), String::new(), String::new());
    let mut in_text = false;
    for op in &parsed.ops {
        match op.operator.as_slice() {
            b"BT" => {
                in_text = true;
                cur.clear();
            }
            b"ET" => {
                in_text = false;
                lines.push(std::mem::take(&mut cur));
            }
            b"Tf" if in_text => {
                font = String::from_utf8(op.operands[0].as_name().unwrap().to_vec()).unwrap();
            }
            b"Tj" | b"TJ" if in_text => {
                let mut strings = Vec::new();
                for o in &op.operands {
                    match o {
                        C::Str(s) => strings.push(s.clone()),
                        C::Array(a) => strings.extend(
                            a.iter()
                                .filter_map(|x| x.as_str_bytes().map(<[u8]>::to_vec)),
                        ),
                        _ => {}
                    }
                }
                for s in strings {
                    for c in s.chunks(2) {
                        let cid = u16::from_be_bytes([c[0], c[1]]);
                        if let Some(t) = maps[&font].get(&cid) {
                            cur.push_str(t);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    lines
}

fn pdftotext(bytes: &[u8]) -> Option<String> {
    if !have("pdftotext") {
        assert!(!tools_required());
        return None;
    }
    let (_d, p) = scratch("t.pdf", bytes);
    let o = Command::new("pdftotext")
        .arg("-raw")
        .arg(&p)
        .arg("-")
        .output()
        .unwrap();
    Some(String::from_utf8_lossy(&o.stdout).into_owned())
}

#[test]
fn latin_greek_cyrillic_are_extractable() {
    install_test_fonts();
    let text = "Hello, world! Привет, мир. Γειά σου κόσμε. Ünïcödé fi ffl.";
    let (doc, report) = add(box_cmd(text, 14.0));
    assert!(report.is_clean(), "{report:?}");
    // Independent decode through ToUnicode reproduces the text (one BT/ET per line).
    let joined = decode_ap_text(&doc).join(" ");
    assert_eq!(
        joined.split_whitespace().collect::<Vec<_>>(),
        text.split_whitespace().collect::<Vec<_>>()
    );
    // Poppler agrees.
    if let Some(t) = pdftotext(&write_bytes(&doc)) {
        for w in ["Hello,", "Привет,", "Γειά", "κόσμε.", "Ünïcödé"] {
            assert!(t.contains(w), "pdftotext lacks {w:?}:\n{t}");
        }
    }
    // Font structure: Type0/Identity-H over CIDFontType2 with an embedded, subset TrueType.
    let (_, fonts) = ap_of(&doc);
    let f = fonts.dict_get("F0").unwrap();
    assert_eq!(f.dict_get("Subtype").unwrap().name().unwrap(), b"Type0");
    assert_eq!(
        f.dict_get("Encoding").unwrap().name().unwrap(),
        b"Identity-H"
    );
    let cid = f.dict_get("DescendantFonts").unwrap().array_get(0).unwrap();
    assert_eq!(
        cid.dict_get("Subtype").unwrap().name().unwrap(),
        b"CIDFontType2"
    );
    let fd = cid.dict_get("FontDescriptor").unwrap();
    let file = fd.dict_get("FontFile2").unwrap();
    let font_bytes = file.stream_decoded(DecodeLevel::All).unwrap().to_vec();
    assert!(
        font_bytes.len() < 40_000,
        "subset is {} bytes",
        font_bytes.len()
    );
    let name = String::from_utf8(f.dict_get("BaseFont").unwrap().name().unwrap()).unwrap();
    assert!(name.len() > 7 && name.as_bytes()[6] == b'+', "{name}");
}

#[test]
fn ligature_glyphs_extract_as_their_letters() {
    install_test_fonts();
    let (doc, _) = add(box_cmd("office affluent", 20.0));
    assert_eq!(decode_ap_text(&doc).join(" "), "office affluent");
}

#[test]
fn cjk_uses_an_embeddable_cff_font_as_cid_font_type0() {
    install_test_fonts();
    let (doc, report) = add(box_cmd("日本語 ひらがな", 18.0));
    assert_eq!(report.fallbacks.len(), 1, "{report:?}");
    assert!(
        report.fallbacks[0].font.contains("NotoSansJP"),
        "{report:?}"
    );
    assert!(report.missing.is_empty());
    assert_eq!(decode_ap_text(&doc).join(" "), "日本語 ひらがな");
    let (_, fonts) = ap_of(&doc);
    let cjk = fonts
        .dict_keys()
        .unwrap()
        .into_iter()
        .map(|k| fonts.dict_get(&k).unwrap())
        .find(|f| {
            f.dict_get("DescendantFonts")
                .unwrap()
                .array_get(0)
                .unwrap()
                .dict_get("Subtype")
                .unwrap()
                .name()
                .unwrap()
                == b"CIDFontType0"
        })
        .expect("a CIDFontType0 font");
    let fd = cjk
        .dict_get("DescendantFonts")
        .unwrap()
        .array_get(0)
        .unwrap()
        .dict_get("FontDescriptor")
        .unwrap();
    let ff = fd.dict_get("FontFile3").unwrap();
    assert_eq!(
        ff.stream_dict()
            .unwrap()
            .dict_get("Subtype")
            .unwrap()
            .name()
            .unwrap(),
        b"OpenType"
    );
    // The subset is a tiny fraction of the source font.
    assert!(ff.stream_decoded(DecodeLevel::All).unwrap().to_vec().len() < 8_000);
    // It renders in PDFium (ink inside the box, away from the latin-only area) and extracts in Poppler.
    let bytes = write_bytes(&doc);
    qpdf_check(&bytes).unwrap();
    let r = pdfium_render(bytes.clone(), 0);
    let ink = (20..380)
        .flat_map(|x| (270..300).map(move |y| (x, y)))
        .filter(|&(x, y)| r.at(f64::from(x) + 0.5, f64::from(y) + 0.5) != WHITE)
        .count();
    assert!(ink > 150, "CJK glyphs drawn: {ink}");
    if let Some(t) = pdftotext(&bytes) {
        assert!(t.contains("日本語") && t.contains("ひらがな"), "{t}");
    }
}

#[test]
fn restricted_fonts_are_never_embedded() {
    install_test_fonts();
    // ب ت ث are only in the font whose OS/2 fsType forbids embedding.
    let (doc, report) = add(box_cmd("a\u{628}\u{62A}\u{62B}b", 18.0));
    assert!(report.fallbacks.is_empty(), "{report:?}");
    assert_eq!(report.restricted.len(), 1, "{report:?}");
    assert!(report.restricted[0].font.contains("PapyrineRestrictedTest"));
    assert_eq!(report.missing, "\u{628}\u{62A}\u{62B}");
    // Nothing from that font ended up in the file.
    let bytes = write_bytes(&doc);
    assert!(!String::from_utf8_lossy(&bytes).contains("PapyrineRestricted"));
    // The characters fell back to the bundled font's .notdef (cid 0) and the document is valid.
    qpdf_check(&bytes).unwrap();
    assert_eq!(decode_ap_text(&doc).join(""), "ab");
}

/// `/ActualText` strings of the marked-content spans in the appearance stream.
fn actual_texts(doc: &Document) -> Vec<String> {
    let (ap, _) = ap_of(doc);
    let parsed = parse(&ap.stream_decoded(DecodeLevel::All).unwrap().to_vec());
    parsed
        .ops
        .iter()
        .filter(|o| o.is("BDC"))
        .filter_map(|o| match o.operands.get(1) {
            Some(C::Dict(d)) => d
                .iter()
                .find(|(k, _)| k == b"ActualText")
                .and_then(|(_, v)| v.as_str_bytes().map(<[u8]>::to_vec)),
            _ => None,
        })
        .map(|b| {
            assert_eq!(&b[..2], &[0xFE, 0xFF]);
            String::from_utf16(
                &b[2..]
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect::<Vec<_>>(),
            )
            .unwrap()
        })
        .collect()
}

fn sorted_chars(s: &str) -> Vec<char> {
    let mut v: Vec<char> = s
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(*c, '\u{202a}'..='\u{202e}'))
        .collect();
    v.sort_unstable();
    v
}

#[test]
fn rtl_hebrew_and_arabic_extract_in_logical_order() {
    install_test_fonts();
    // ToUnicode alone gives the characters; ActualText on RTL lines gives logical order.
    for text in [
        "abc \u{5E9}\u{5DC}\u{5D5}\u{5DD} \u{5E2}\u{5D5}\u{5DC}\u{5DD} def",
        "\u{5E9}\u{5DC}\u{5D5}\u{5DD}",
        "\u{627}\u{644}\u{633}\u{644}\u{627}\u{645}",
    ] {
        let (doc, report) = add(box_cmd(text, 18.0));
        assert!(
            report.missing.is_empty() && report.restricted.is_empty(),
            "{report:?}"
        );
        let bytes = write_bytes(&doc);
        qpdf_check(&bytes).unwrap();
        assert_eq!(actual_texts(&doc), vec![text.to_string()]);
        // The ToUnicode mapping alone yields the same characters (visual order).
        let decoded = decode_ap_text(&doc).join(" ");
        assert_eq!(sorted_chars(&decoded), sorted_chars(text));
        if let Some(t) = pdftotext(&bytes) {
            assert_eq!(
                sorted_chars(t.trim_start_matches("Hello World 1")),
                sorted_chars(text),
                "{t:?}"
            );
        }
    }
}

#[test]
fn arabic_letters_are_joined_by_the_shaper() {
    // Initial/medial/final forms are different glyphs than the isolated ones; the subset test
    // font only has the letters needed, and a joined run must use glyphs from GSUB.
    let lay = papyrine_annotate::text::layout_with(
        "\u{644}\u{633}\u{645}",
        FontFamily::Sans,
        false,
        24.0,
        300.0,
        false,
    );
    // With only the bundled font the letters are missing (notdef); the shaping test needs the
    // Arabic test font, loaded through the fallback database.
    install_test_fonts();
    let lay2 = papyrine_annotate::text::layout_with(
        "\u{644}\u{633}\u{645}",
        FontFamily::Sans,
        false,
        24.0,
        300.0,
        true,
    );
    let g1: Vec<u16> = lay.lines[0].glyphs.iter().map(|g| g.gid).collect();
    let g2: Vec<u16> = lay2.lines[0].glyphs.iter().map(|g| g.gid).collect();
    assert_eq!(g1, vec![0, 0, 0]);
    assert!(g2.iter().all(|g| *g != 0));
    // lam-seen-meem joined: initial lam, medial seen, final meem are not the isolated forms.
    let face = lay2.faces[1].ttf().unwrap();
    let iso: Vec<u16> = ['\u{644}', '\u{633}', '\u{645}']
        .iter()
        .map(|c| face.glyph_index(*c).unwrap().0)
        .collect();
    // Visual order for RTL is reversed: meem, seen, lam.
    assert_ne!(g2[2], iso[0], "lam is not isolated");
    assert_ne!(g2[1], iso[1], "seen is not isolated");
    assert_ne!(g2[0], iso[2], "meem is not isolated");
}

#[test]
fn rtl_text_is_drawn_right_to_left() {
    install_test_fonts();
    // First Hebrew letter (shin) must be the rightmost ink of the word.
    let (doc, _) = add(AddAnnotation::text_box(
        0,
        [20.0, 100.0, 380.0, 300.0],
        TextStyle {
            size: 40.0,
            align: Align::Left,
            ..TextStyle::default()
        },
        "\u{5E9}\u{5DC}\u{5D5}\u{5DD}",
        AnnotProps::default(),
    ));
    let lay = papyrine_annotate::text::layout_with(
        "\u{5E9}\u{5DC}\u{5D5}\u{5DD}",
        FontFamily::Sans,
        false,
        40.0,
        300.0,
        true,
    );
    assert_eq!(lay.lines.len(), 1);
    let l = &lay.lines[0];
    assert!(l.has_rtl);
    // Visual order: last logical char (final mem) first.
    let order: Vec<_> = l.glyphs.iter().filter_map(|g| g.text.clone()).collect();
    assert_eq!(order.join(""), "\u{5DD}\u{5D5}\u{5DC}\u{5E9}");
    // And the document renders ink.
    let r = pdfium_render(write_bytes(&doc), 0);
    let ink = (20..140)
        .flat_map(|x| (240..300).map(move |y| (x, y)))
        .filter(|&(x, y)| r.at(f64::from(x) + 0.5, f64::from(y) + 0.5) != WHITE)
        .count();
    assert!(ink > 100);
}

#[test]
fn text_layout_height_fits_the_box_or_is_clipped() {
    install_test_fonts();
    let text = "word ".repeat(200);
    let h =
        papyrine_annotate::appearance::measure_text_box(&text, &TextStyle::default(), 200.0, 0.0);
    assert!(h > 300.0, "{h}");
    // A small box clips instead of overflowing.
    let (doc, _) = add(AddAnnotation::text_box(
        0,
        [20.0, 300.0, 120.0, 330.0],
        TextStyle::default(),
        &text,
        AnnotProps::default(),
    ));
    let r = pdfium_render(write_bytes(&doc), 0);
    let below = (20..140)
        .flat_map(|x| (200..298).map(move |y| (x, y)))
        .filter(|&(x, y)| r.at(f64::from(x) + 0.5, f64::from(y) + 0.5) != WHITE)
        .count();
    assert_eq!(below, 0, "text is clipped to the box");
}

#[test]
fn empty_text_box_is_valid() {
    install_test_fonts();
    let (doc, _) = add(box_cmd("", 12.0));
    qpdf_check(&write_bytes(&doc)).unwrap();
}
