//! Flat-form marks, XFA policy, tab order and navigation.

mod common;

use common::*;
use papyrine_cos::{Document, Object, ObjectKind};
use papyrine_fill::form::{FormTree, XfaState};
use papyrine_fill::*;

fn flat_pdf(rotate: i64) -> Vec<u8> {
    let mut p = Pdf::new();
    p.add("<< /Type /Catalog /Pages 2 0 R >>");
    p.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    p.add(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Rotate {rotate} /Contents 4 0 R /Resources << >> >>"));
    p.stream("", b"0.8 g 100 600 300 40 re f");
    p.build(1)
}

fn annots(doc: &Document) -> Vec<Object> {
    let page = doc.page(0).unwrap();
    let a = page.dict_get("Annots").unwrap();
    if a.kind().unwrap() != ObjectKind::Array {
        return vec![];
    }
    a.array_items().unwrap()
}

fn rect(a: &Object) -> [f64; 4] {
    let r = a.dict_get("Rect").unwrap();
    let v: Vec<f64> = r
        .array_items()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    [v[0], v[1], v[2], v[3]]
}

#[test]
fn flat_text_and_marks_round_trip() {
    let doc = open(flat_pdf(0));
    let mut h = history();
    let mut t = AddFlatText::new(0, 100.0, 500.0, "Hello, flat world");
    t.font_size = 14.0;
    run(&doc, &mut h, t);
    run(&doc, &mut h, AddMark::new(0, MarkKind::Check, 150.0, 400.0));
    run(&doc, &mut h, AddMark::new(0, MarkKind::Cross, 200.0, 400.0));
    let mut dot = AddMark::new(0, MarkKind::Dot, 250.0, 400.0);
    dot.size = 20.0;
    run(&doc, &mut h, dot);
    let mut wrapped = AddFlatText::new(
        0,
        300.0,
        300.0,
        "a long sentence that has to wrap inside its box",
    );
    wrapped.width = Some(90.0);
    run(&doc, &mut h, wrapped);
    for d in [&doc, &roundtrip(&doc)] {
        let a = annots(d);
        assert_eq!(a.len(), 5);
        assert_eq!(
            a[0].dict_get("Subtype").unwrap().name().unwrap(),
            b"FreeText"
        );
        assert_eq!(
            a[0].dict_get("IT").unwrap().name().unwrap(),
            b"FreeTextTypeWriter"
        );
        let r = rect(&a[0]);
        assert_eq!(r[0], 100.0);
        assert_eq!(r[3], 500.0, "top-left anchored");
        assert!(r[2] - r[0] > 80.0);
        // No border at all.
        assert_eq!(
            a[0].dict_get("BS")
                .unwrap()
                .dict_get("W")
                .unwrap()
                .as_int()
                .unwrap(),
            0
        );
        let rr = rect(&a[4]);
        assert!(
            rr[2] - rr[0] <= 90.5 && rr[3] - rr[1] > 30.0,
            "wrapped box {rr:?}"
        );
    }
    let bytes = write(&doc);
    qpdf_check(&bytes, "flat");
    let t = render_page(&bytes, 0, 1584);
    assert!(
        ink_in(&t, 2.0, 792.0, [100.0, 486.0, 260.0, 502.0]) > 80,
        "typed text renders"
    );
    assert!(
        ink_in(&t, 2.0, 792.0, [144.0, 394.0, 156.0, 406.0]) > 15,
        "check mark renders"
    );
    assert!(
        ink_in(&t, 2.0, 792.0, [194.0, 394.0, 206.0, 406.0]) > 15,
        "cross renders"
    );
    assert!(
        ink_in(&t, 2.0, 792.0, [242.0, 390.0, 258.0, 410.0]) > 40,
        "bullet renders"
    );
}

#[test]
fn flat_text_is_upright_on_rotated_pages() {
    let scale = 2.0;
    for rot in [0i64, 90, 180, 270] {
        let doc = open(flat_pdf(rot));
        let mut h = history();
        run(
            &doc,
            &mut h,
            AddFlatText::new(0, 300.0, 400.0, "Upright text"),
        );
        let bytes = write(&doc);
        qpdf_check(&bytes, "rotated flat");
        let t = render_page(&bytes, 0, 1584);
        let (pw, ph) = (612.0, 792.0);
        // Where the displayed top-left corner (user point 300,400) lands on screen.
        let (sx, sy) = match rot {
            90 => (400.0, 300.0),
            180 => (pw - 300.0, 400.0),
            270 => (ph - 400.0, pw - 300.0),
            _ => (300.0, ph - 400.0),
        };
        let r = rect(&annots(&doc)[0]);
        let (bw, bh) = if rot % 180 == 0 {
            (r[2] - r[0], r[3] - r[1])
        } else {
            (r[3] - r[1], r[2] - r[0])
        };
        let in_box = ink_dev(
            &t,
            [sx * scale, sy * scale, (sx + bw) * scale, (sy + bh) * scale],
        );
        let total = ink_dev(&t, [0.0, 0.0, t.width as f64, t.height as f64]);
        assert!(in_box > 100, "rot {rot}: ink in the displayed box {in_box}");
        assert!(
            in_box as f64 / total as f64 > 0.95,
            "rot {rot}: {in_box} of {total} outside the box"
        );
    }
}

#[test]
fn flat_text_rejects_unencodable_characters() {
    let doc = open(flat_pdf(0));
    let mut h = history();
    let e = h
        .execute(&doc, Box::new(AddFlatText::new(0, 100.0, 500.0, "日本語")))
        .unwrap_err();
    assert!(e.to_string().contains("cannot show"), "{e}");
    let e = h
        .execute(&doc, Box::new(AddFlatText::new(5, 100.0, 500.0, "x")))
        .unwrap_err();
    assert!(e.to_string().contains("out of range"), "{e}");
}

/// A form with an XFA array: template + datasets packets.
fn xfa_form(base_profile: &str, needs_rendering: bool) -> Vec<u8> {
    let mut p = Pdf::new();
    p.add(format!(
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R {} >>",
        if needs_rendering {
            "/NeedsRendering true"
        } else {
            ""
        }
    ));
    p.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    p.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [4 0 R] >>");
    p.add("<< /Type /Annot /Subtype /Widget /FT /Tx /T (f1) /Rect [10 10 100 30] /F 4 /P 3 0 R /DA (/Helv 10 Tf 0 g) >>");
    p.add("PLACEHOLDER");
    let tpl = p.stream(
        "",
        format!("<template xmlns=\"http://www.xfa.org/schema/xfa-template/3.3/\" baseProfile=\"{base_profile}\"/>").as_bytes(),
    );
    let data = p.stream("", b"<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data><f1>old</f1></xfa:data></xfa:datasets>");
    p.set(
        5,
        format!("<< /Fields [4 0 R] /DA (/Helv 10 Tf 0 g) /XFA [(template) {tpl} 0 R (datasets) {data} 0 R] >>"),
    );
    p.build(1)
}

#[test]
fn static_xfa_is_fillable_and_the_stale_packet_is_dropped() {
    // Both the Designer "static" profile and the IRS family (no base profile, formServer
    // `acrobat12.0static`) are hybrids whose AcroForm is live.
    for profile in ["full", "interactiveForms"] {
        assert_eq!(
            FormTree::load(&open(xfa_form(profile, false))).unwrap().xfa,
            XfaState::Static,
            "{profile}"
        );
    }
    let doc = open(xfa_form("interactiveForms", false));
    let st = form_status(&doc).unwrap();
    assert!(!st.dynamic_xfa && st.xfa == XfaState::Static);
    let mut h = history();
    run(&doc, &mut h, SetTextValue::new("f1", "new value"));
    for d in [&doc, &roundtrip(&doc)] {
        let f = FormTree::load(d).unwrap();
        assert_eq!(
            f.xfa,
            XfaState::None,
            "XFA packet removed with the first edit"
        );
        assert_eq!(f.by_name("f1").unwrap().value.as_text(), Some("new value"));
    }
    // Undo brings the packet back exactly.
    h.undo(&doc).unwrap();
    assert_eq!(FormTree::load(&doc).unwrap().xfa, XfaState::Static);
    qpdf_check(&write(&doc), "xfa");
}

#[test]
fn dynamic_xfa_is_read_only_with_a_notice() {
    for (profile, nr) in [("full", true), ("interactiveForms", true)] {
        let doc = open(xfa_form(profile, nr));
        let st = form_status(&doc).unwrap();
        assert!(st.dynamic_xfa, "{profile} {nr}");
        let mut h = history();
        let e = h
            .execute(&doc, Box::new(SetTextValue::new("f1", "x")))
            .unwrap_err();
        assert!(e.to_string().contains("dynamic XFA"), "{e}");
        assert!(!h.can_undo());
        assert_eq!(FormTree::load(&doc).unwrap().xfa, XfaState::Dynamic);
    }
}

fn tabbed_form(tabs: &str) -> Vec<u8> {
    let mut p = Pdf::new();
    p.add("<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>");
    p.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    p.add("PLACEHOLDER");
    p.add("PLACEHOLDER");
    // Fields laid out in a 2x2 grid but stored in a scrambled order:
    //   a(top-left) b(top-right) / c(bottom-left) d(bottom-right)
    let mk = |p: &mut Pdf, name: &str, r: &str| {
        p.add(format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect {r} /F 4 /P 3 0 R >>"
        ))
    };
    let d = mk(&mut p, "d", "[200 100 280 120]");
    let a = mk(&mut p, "a", "[100 200 180 220]");
    let c = mk(&mut p, "c", "[100 100 180 120]");
    let b = mk(&mut p, "b", "[200 200 280 220]");
    p.set(
        3,
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] {tabs} /Annots [{d} 0 R {a} 0 R {c} 0 R {b} 0 R] >>"),
    );
    p.set(
        4,
        format!("<< /Fields [{d} 0 R {a} 0 R {c} 0 R {b} 0 R] /DA (/Helv 0 Tf 0 g) >>"),
    );
    p.build(1)
}

fn order(doc: &Document) -> String {
    tab_order(doc, 0)
        .unwrap()
        .iter()
        .map(|s| s.name.clone())
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn tab_order_modes() {
    assert_eq!(
        order(&open(tabbed_form(""))),
        "dacb",
        "annotation order by default"
    );
    assert_eq!(order(&open(tabbed_form("/Tabs /R"))), "abcd", "rows");
    assert_eq!(order(&open(tabbed_form("/Tabs /C"))), "acbd", "columns");
    // Structure order with no structure tree falls back to annotation order.
    assert_eq!(order(&open(tabbed_form("/Tabs /S"))), "dacb");
}

#[test]
fn next_field_walks_and_wraps() {
    let doc = open(tabbed_form("/Tabs /R"));
    let ids: Vec<_> = tab_order(&doc, 0).unwrap();
    let first = next_field(&doc, None, true, true).unwrap().unwrap();
    assert_eq!(first.name, "a");
    let second = next_field(&doc, Some((first.field, first.widget)), true, true)
        .unwrap()
        .unwrap();
    assert_eq!(second.name, "b");
    let last = next_field(&doc, None, false, true).unwrap().unwrap();
    assert_eq!(last.name, "d");
    let wrap = next_field(&doc, Some((last.field, last.widget)), true, true)
        .unwrap()
        .unwrap();
    assert_eq!(wrap.name, "a");
    let back = next_field(&doc, Some((first.field, first.widget)), false, true)
        .unwrap()
        .unwrap();
    assert_eq!(back.name, "d");
    assert_eq!(ids.len(), 4);
    assert_eq!(document_tab_order(&doc).unwrap().len(), 4);
}

#[test]
fn radio_group_is_one_tab_stop() {
    let doc = open(sample_form());
    let names: Vec<String> = tab_order(&doc, 0)
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(
        names.iter().filter(|n| *n == "pick").count(),
        1,
        "{names:?}"
    );
}

#[test]
fn no_form_means_a_clear_error() {
    let doc = open(flat_pdf(0));
    let mut h = history();
    let e = h
        .execute(&doc, Box::new(SetTextValue::new("x", "y")))
        .unwrap_err();
    assert!(e.to_string().contains("no AcroForm"), "{e}");
    assert!(!form_status(&doc).unwrap().has_form);
}
