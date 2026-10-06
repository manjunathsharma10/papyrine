//! Edge cases: eraser, delete rules, metadata edits, direct annotations, encryption,
//! hostile parameters, performance.

mod common;

use std::path::PathBuf;
use std::time::Instant;

use common::*;
use papyrine_annotate::*;
use papyrine_cos::{Document, ObjectKind, OpenOptions};

fn generated(name: &str) -> Option<Vec<u8>> {
    let p: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "../../corpus/cache/generated",
        name,
    ]
    .iter()
    .collect();
    std::fs::read(p).ok()
}

#[test]
fn eraser_splits_deletes_and_is_exactly_undoable() {
    let doc = open(build_pdf(1));
    let mut h = history();
    let s = run(
        &doc,
        &mut h,
        AddAnnotation::pen(
            0,
            vec![
                vec![[50.0, 100.0], [350.0, 100.0]],
                vec![[50.0, 200.0], [350.0, 200.0]],
            ],
            AnnotProps::default().with_width(4.0),
        ),
    );
    let t = AnnotRef::id(s.created[0]);
    let before = snapshot(&doc);
    // Nothing under the eraser: no change at all.
    let sum = run(
        &doc,
        &mut h,
        EraseInk::new(0, t.clone(), vec![[200.0, 150.0]], 5.0),
    );
    assert!(sum.touched.is_empty() && sum.created.is_empty());
    assert_matches(&doc, &before, "no-op erase");
    h.undo(&doc).unwrap().unwrap(); // undoing a no-op entry changes nothing either
    assert_matches(&doc, &before, "undo of a no-op erase");
    h.redo(&doc).unwrap().unwrap();
    // Cut the first stroke in the middle.
    run(
        &doc,
        &mut h,
        EraseInk::new(0, t.clone(), vec![[200.0, 90.0], [200.0, 110.0]], 5.0),
    );
    let a = &model_annots(roundtrip(&doc), 0)[0];
    assert_eq!(a.ink_list.len(), 3);
    // Regenerated /Rect still covers every remaining stroke.
    for s in &a.ink_list {
        for p in s {
            assert!(p.x >= a.rect.x0 && p.x <= a.rect.x1 && p.y >= a.rect.y0 && p.y <= a.rect.y1);
        }
    }
    // Wipe everything: the annotation disappears.
    let mid = snapshot(&doc);
    run(
        &doc,
        &mut h,
        EraseInk::new(
            0,
            t.clone(),
            vec![[0.0, 0.0], [400.0, 400.0], [0.0, 400.0], [400.0, 0.0]],
            150.0,
        ),
    );
    assert!(model_annots(roundtrip(&doc), 0).is_empty());
    h.undo(&doc).unwrap().unwrap();
    assert_matches(&doc, &mid, "undo of erase-all");
    // Only ink annotations can be erased.
    let r = run(
        &doc,
        &mut h,
        AddAnnotation::rectangle(0, [10.0, 10.0, 50.0, 50.0], AnnotProps::default()),
    );
    let bad = EraseInk::new(0, AnnotRef::id(r.created[0]), vec![[20.0, 20.0]], 5.0);
    let snap = snapshot(&doc);
    assert!(h.execute(&doc, Box::new(bad)).is_err());
    assert_matches(&doc, &snap, "refused erase");
    // Bad parameters.
    for (path, radius) in [
        (vec![], 5.0),
        (vec![[1.0, 1.0]], 0.0),
        (vec![[f64::NAN, 1.0]], 3.0),
        (vec![[1.0, 1.0]], -2.0),
    ] {
        assert!(
            h.execute(&doc, Box::new(EraseInk::new(0, t.clone(), path, radius)))
                .is_err()
        );
    }
}

#[test]
fn delete_rules_and_multi_delete() {
    let doc = open(build_pdf(1));
    let mut h = history();
    let ids: Vec<_> = (0..3)
        .map(|i| {
            run(
                &doc,
                &mut h,
                AddAnnotation::rectangle(
                    0,
                    [
                        10.0 + 40.0 * f64::from(i),
                        10.0,
                        40.0 + 40.0 * f64::from(i),
                        40.0,
                    ],
                    AnnotProps::default(),
                ),
            )
            .created[0]
        })
        .collect();
    let before = snapshot(&doc);
    run(
        &doc,
        &mut h,
        DeleteAnnotations::new(0, vec![AnnotRef::id(ids[0]), AnnotRef::id(ids[2])]),
    );
    assert_eq!(model_annots(roundtrip(&doc), 0).len(), 1);
    h.undo(&doc).unwrap().unwrap();
    assert_matches(&doc, &before, "undo multi delete");
    // Nothing to delete / unknown target / wrong page are errors that leave the document alone.
    assert!(
        h.execute(&doc, Box::new(DeleteAnnotations::new(0, vec![])))
            .is_err()
    );
    assert!(
        h.execute(
            &doc,
            Box::new(DeleteAnnotations::new(0, vec![AnnotRef::named("nope")]))
        )
        .is_err()
    );
    assert!(
        h.execute(
            &doc,
            Box::new(DeleteAnnotations::new(5, vec![AnnotRef::id(ids[0])]))
        )
        .is_err()
    );
    assert_matches(&doc, &before, "failed deletes");
    // By name works too.
    let nm = model_annots(roundtrip(&doc), 0)[0].name.clone().unwrap();
    run(
        &doc,
        &mut h,
        DeleteAnnotations::new(0, vec![AnnotRef::named(nm)]),
    );
    assert_eq!(model_annots(roundtrip(&doc), 0).len(), 2);
}

#[test]
fn widgets_and_popups_are_not_deletable_as_annotations() {
    let Some(bytes) = std::fs::read(
        [
            env!("CARGO_MANIFEST_DIR"),
            "../../corpus/cache/files/pdfjs/test/pdfs/annotation-link-text-popup.pdf",
        ]
        .iter()
        .collect::<PathBuf>(),
    )
    .ok() else {
        return;
    };
    let doc = open(bytes);
    let mut h = history();
    let page = doc.page(0).unwrap();
    for a in page.dict_get("Annots").unwrap().array_items().unwrap() {
        let sub = a.dict_get("Subtype").unwrap().name().unwrap();
        if sub == b"Popup" {
            let before = snapshot(&doc);
            let r = h.execute(
                &doc,
                Box::new(DeleteAnnotations::new(
                    0,
                    vec![AnnotRef::id(a.id().unwrap())],
                )),
            );
            assert!(r.is_err());
            assert_matches(&doc, &before, "popup delete refused");
        }
        if sub == b"Text" {
            // Deleting the note takes its popup with it.
            let n = page.dict_get("Annots").unwrap().array_len().unwrap();
            run(
                &doc,
                &mut h,
                DeleteAnnotations::new(0, vec![AnnotRef::id(a.id().unwrap())]),
            );
            let m = page.dict_get("Annots").unwrap().array_len().unwrap();
            assert_eq!(n - m, 2, "the note and its popup are removed");
            break;
        }
    }
}

#[test]
fn metadata_only_edits_of_foreign_types() {
    let Some(bytes) = std::fs::read(
        [
            env!("CARGO_MANIFEST_DIR"),
            "../../corpus/cache/files/pdfjs/test/pdfs/annotation-stamp.pdf",
        ]
        .iter()
        .collect::<PathBuf>(),
    )
    .ok() else {
        return;
    };
    let doc = open(bytes);
    let mut h = history();
    let page = doc.page(0).unwrap();
    let stamp = page
        .dict_get("Annots")
        .unwrap()
        .array_items()
        .unwrap()
        .into_iter()
        .find(|a| a.dict_get("Subtype").unwrap().name().unwrap() == b"Stamp")
        .unwrap();
    let ap_before = stamp.dict_get("AP").unwrap().unparse_with(true).unwrap();
    let before = snapshot(&doc);
    let p = AnnotProps::default()
        .with_contents("Reviewed ✓")
        .with_author("Ada");
    let sum = run(
        &doc,
        &mut h,
        UpdateAnnotation::new(0, AnnotRef::id(stamp.id().unwrap()), PropsPatch::new(p)),
    );
    assert_eq!(
        sum.touched,
        vec![stamp.id().unwrap()],
        "only the stamp dictionary changes"
    );
    assert_eq!(
        stamp.dict_get("AP").unwrap().unparse_with(true).unwrap(),
        ap_before
    );
    let items = model_annots(roundtrip(&doc), 0);
    let a = items
        .iter()
        .find(|a| a.subtype == papyrine_model::AnnotSubtype::Stamp)
        .unwrap();
    assert_eq!(a.contents.as_deref(), Some("Reviewed ✓"));
    assert_eq!(a.title.as_deref(), Some("Ada"));
    // Appearance-affecting changes and geometry edits are refused.
    let bad = UpdateAnnotation::new(
        0,
        AnnotRef::id(stamp.id().unwrap()),
        PropsPatch::new(AnnotProps::default().with_color(Color::gray(0.5))),
    );
    assert!(h.execute(&doc, Box::new(bad)).is_err());
    h.undo(&doc).unwrap().unwrap();
    assert_matches(&doc, &before, "undo metadata edit");
}

#[test]
fn text_box_edit_reuses_the_appearance_stream() {
    let doc = open(build_pdf(1));
    let mut h = history();
    let s = run(
        &doc,
        &mut h,
        AddAnnotation::text_box(
            0,
            [50.0, 200.0, 250.0, 260.0],
            TextStyle::default(),
            "one",
            AnnotProps::default(),
        ),
    );
    let t = AnnotRef::id(s.created[0]);
    let n0 = doc.object_ids().unwrap().len();
    let sum = run(
        &doc,
        &mut h,
        UpdateAnnotation::new(
            0,
            t.clone(),
            PropsPatch::new(AnnotProps::default().with_contents("two lines\nof text")),
        ),
    );
    // New font objects were created, the AP stream and annotation were rewritten in place.
    assert!(sum.touched.len() <= 3, "{:?}", sum.touched);
    assert!(doc.object_ids().unwrap().len() > n0);
    let a = &model_annots(roundtrip(&doc), 0)[0];
    assert_eq!(a.contents.as_deref(), Some("two lines\nof text"));
    // Change style through a geometry update.
    run(
        &doc,
        &mut h,
        UpdateAnnotation::new(0, t, PropsPatch::default()).with_geometry(Geometry::TextBox {
            rect: [50.0, 200.0, 250.0, 260.0],
            style: TextStyle {
                family: FontFamily::Mono,
                size: 20.0,
                ..TextStyle::default()
            },
        }),
    );
    let a = &model_annots(roundtrip(&doc), 0)[0];
    assert_eq!(a.default_appearance.as_deref(), Some("/Cour 20 Tf 0 g"));
    qpdf_check(&write_bytes(&doc)).unwrap();
}

/// A direct (inline) annotation dictionary inside /Annots, and an /Annots array that is an
/// indirect object, and a page without /Annots.
#[test]
fn annots_array_shapes() {
    // Indirect /Annots array holding a direct annotation dict.
    let mut pdf = build_pdf(1);
    let s = String::from_utf8_lossy(&pdf).into_owned();
    let s = s.replace(
        "/MediaBox [0 0 400 400]",
        "/Annots 20 0 R /MediaBox [0 0 400 400]",
    );
    let s = s.replacen(
        "xref\n",
        "20 0 obj\n[<< /Type /Annot /Subtype /Square /Rect [10 10 60 60] /C [0 0 1] /F 4 >>]\nendobj\nxref\n",
        1,
    );
    pdf = s.into_bytes();
    let doc = open(pdf); // qpdf reconstructs the xref
    let mut h = history();
    let before = snapshot(&doc);
    let sum = run(
        &doc,
        &mut h,
        AddAnnotation::highlight(
            0,
            vec![Quad::from_rect(100.0, 100.0, 200.0, 120.0)],
            AnnotProps::default(),
        ),
    );
    assert!(
        sum.touched.iter().any(|i| i.num == 20),
        "indirect array is the recorded holder"
    );
    assert_eq!(model_annots(roundtrip(&doc), 0).len(), 2);
    // Edit the direct annotation by position: the holder (the array) is the recorded object.
    let nm = model_annots(roundtrip(&doc), 0);
    assert_eq!(nm[0].subtype, papyrine_model::AnnotSubtype::Square);
    let upd = run(
        &doc,
        &mut h,
        UpdateAnnotation::new(
            0,
            AnnotRef::Index(0),
            PropsPatch::new(
                AnnotProps::default()
                    .with_color(Color::rgb(0.0, 1.0, 0.0))
                    .with_width(5.0),
            ),
        ),
    );
    assert!(upd.touched.iter().any(|i| i.num == 20));
    let a = &model_annots(roundtrip(&doc), 0)[0];
    assert_eq!(a.color.as_deref(), Some(&[0.0, 1.0, 0.0][..]));
    assert!(a.has_appearance);
    assert!(
        h.execute(
            &doc,
            Box::new(UpdateAnnotation::new(
                0,
                AnnotRef::Name("missing".into()),
                PropsPatch::default()
            ))
        )
        .is_err()
    );
    h.undo(&doc).unwrap().unwrap();
    h.undo(&doc).unwrap().unwrap();
    assert_matches(&doc, &before, "undo of both edits");
}

#[test]
fn page_without_annots_gets_an_array() {
    let doc = open(build_pdf(2));
    assert_eq!(
        doc.page(1)
            .unwrap()
            .dict_get("Annots")
            .unwrap()
            .kind()
            .unwrap(),
        ObjectKind::Null
    );
    let mut h = history();
    run(
        &doc,
        &mut h,
        AddAnnotation::sticky_note(1, 10.0, 100.0, AnnotProps::default()),
    );
    assert_eq!(model_annots(roundtrip(&doc), 1).len(), 1);
    assert!(model_annots(roundtrip(&doc), 0).is_empty());
}

#[test]
fn hostile_parameters_are_errors_not_panics() {
    let doc = open(build_pdf(1));
    let mut h = history();
    let before = snapshot(&doc);
    let nan = f64::NAN;
    let bad: Vec<AddAnnotation> = vec![
        AddAnnotation::highlight(0, vec![], AnnotProps::default()),
        AddAnnotation::highlight(
            0,
            vec![Quad([[nan, 0.0], [1.0, 1.0], [0.0, 0.0], [2.0, 2.0]])],
            AnnotProps::default(),
        ),
        AddAnnotation::highlight(
            9,
            vec![Quad::from_rect(0.0, 0.0, 9.0, 9.0)],
            AnnotProps::default(),
        ),
        AddAnnotation::rectangle(0, [0.0, 0.0, 0.2, 10.0], AnnotProps::default()),
        AddAnnotation::rectangle(0, [0.0, 0.0, 1e9, 10.0], AnnotProps::default()),
        AddAnnotation::line(0, [5.0, 5.0], [5.0, 5.0], AnnotProps::default()),
        AddAnnotation::pen(0, vec![], AnnotProps::default()),
        AddAnnotation::pen(0, vec![vec![]], AnnotProps::default()),
        AddAnnotation::rectangle(
            0,
            [0.0, 0.0, 10.0, 10.0],
            AnnotProps::default().with_opacity(2.0),
        ),
        AddAnnotation::rectangle(
            0,
            [0.0, 0.0, 10.0, 10.0],
            AnnotProps::default().with_width(-1.0),
        ),
        AddAnnotation::rectangle(
            0,
            [0.0, 0.0, 10.0, 10.0],
            AnnotProps::default().with_color(Color(vec![2.0, 0.0, 0.0])),
        ),
        AddAnnotation::rectangle(
            0,
            [0.0, 0.0, 10.0, 10.0],
            AnnotProps::default().with_color(Color(vec![0.5, 0.5])),
        ),
        AddAnnotation::text_box(
            0,
            [0.0, 0.0, 100.0, 100.0],
            TextStyle {
                size: 0.0,
                ..TextStyle::default()
            },
            "x",
            AnnotProps::default(),
        ),
        AddAnnotation::new(
            0,
            Geometry::Note {
                pos: [1.0, 1.0],
                icon: String::new(),
            },
            AnnotProps::default(),
        ),
    ];
    for (i, c) in bad.into_iter().enumerate() {
        assert!(
            h.execute(&doc, Box::new(c)).is_err(),
            "case {i} should fail"
        );
        assert_matches(
            &doc,
            &before,
            &format!("case {i} left the document untouched"),
        );
    }
    // Garbage JSON into the registry.
    let reg = registry();
    for name in [
        "annot_add",
        "annot_update",
        "annot_delete",
        "annot_erase_ink",
        "annot_reply",
    ] {
        for p in [
            serde_json::Value::Null,
            serde_json::json!({"page": "x"}),
            serde_json::json!([1, 2, 3]),
            serde_json::json!({}),
        ] {
            assert!(reg.create(name, &p).is_err(), "{name} {p}");
        }
    }
    // A huge but valid text still lays out and is clipped (no panic, bounded size).
    let big = "wörd ".repeat(8_000);
    let t0 = Instant::now();
    run(
        &doc,
        &mut h,
        AddAnnotation::text_box(
            0,
            [10.0, 10.0, 300.0, 100.0],
            TextStyle::default(),
            &big,
            AnnotProps::default(),
        ),
    );
    eprintln!("100k-char text box: {:?}", t0.elapsed());
}

#[test]
fn encrypted_documents_keep_their_encryption() {
    for name in [
        "enc-aes-256-r6-owner-only.pdf",
        "enc-aes-128-owner-only.pdf",
        "enc-rc4-128-owner-only.pdf",
        "enc-aes-256-r6-objstm.pdf",
    ] {
        let Some(bytes) = generated(name) else {
            continue;
        };
        let Ok(doc) = Document::open_bytes(bytes, &OpenOptions::default()) else {
            eprintln!("skipping {name}");
            continue;
        };
        assert!(doc.encryption().unwrap().is_some());
        let mut h = history();
        run(
            &doc,
            &mut h,
            AddAnnotation::text_box(
                0,
                [50.0, 50.0, 250.0, 100.0],
                TextStyle::default(),
                "secret note Привет",
                AnnotProps::default().with_author("Ada"),
            ),
        );
        let out = write_bytes(&doc);
        let re = open(out.clone());
        assert!(
            re.encryption().unwrap().is_some(),
            "{name}: still encrypted"
        );
        let items = model_annots(re, 0);
        assert!(
            items
                .iter()
                .any(|a| a.contents.as_deref() == Some("secret note Привет")),
            "{name}"
        );
        // The text is not readable in the raw bytes.
        assert!(!String::from_utf8_lossy(&out).contains("secret note"));
        qpdf_check(&out).unwrap();
        eprintln!("ok {name}");
    }
}

#[test]
fn performance_budget() {
    let doc = open(build_pdf(1));
    let mut h = history();
    // 5,000 text-markup quads.
    let quads: Vec<Quad> = (0..5_000)
        .map(|i| {
            Quad::from_rect(
                10.0 + f64::from(i % 50) * 7.0,
                10.0 + f64::from(i / 50) * 3.0,
                15.0 + f64::from(i % 50) * 7.0,
                12.0 + f64::from(i / 50) * 3.0,
            )
        })
        .collect();
    let t = Instant::now();
    run(
        &doc,
        &mut h,
        AddAnnotation::highlight(0, quads, AnnotProps::default()),
    );
    let t_hl = t.elapsed();
    // One 20,000-point pen stroke.
    let pts: Vec<[f64; 2]> = (0..20_000)
        .map(|i| {
            [
                10.0 + f64::from(i) * 0.015,
                200.0 + (f64::from(i) * 0.01).sin() * 50.0,
            ]
        })
        .collect();
    let t = Instant::now();
    run(
        &doc,
        &mut h,
        AddAnnotation::pen(0, vec![pts], AnnotProps::default()),
    );
    let t_ink = t.elapsed();
    // A typical 3-line text box.
    let t = Instant::now();
    run(
        &doc,
        &mut h,
        AddAnnotation::text_box(
            0,
            [20.0, 300.0, 380.0, 380.0],
            TextStyle::default(),
            "A typical comment that wraps over about three lines of the box, with some more words to fill it.",
            AnnotProps::default(),
        ),
    );
    let t_tb = t.elapsed();
    let t = Instant::now();
    let _ = write_bytes(&doc);
    eprintln!(
        "perf (debug build): 5000-quad highlight {t_hl:?}, 20k-point ink {t_ink:?}, text box {t_tb:?}, write {:?}",
        t.elapsed()
    );
    assert!(t_hl.as_secs() < 10 && t_ink.as_secs() < 10 && t_tb.as_secs() < 5);
}
