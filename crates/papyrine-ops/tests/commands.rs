//! Per-command behaviour and round trip (apply -> write -> reopen -> verify -> undo/redo).

mod common;

use common::*;
use papyrine_cos::{Document, ObjectKind};
use papyrine_ops::*;

fn run(doc: &Document, h: &mut History, c: impl Command + 'static) -> ChangeSummary {
    h.execute(doc, Box::new(c)).expect("execute")
}

fn fresh(pages: usize) -> (Document, History) {
    let mut h = History::new();
    h.set_verify(true);
    (open(build_pdf(pages)), h)
}

fn info_title(doc: &Document) -> Option<String> {
    let info = doc.trailer().unwrap().dict_get("Info").unwrap();
    if info.kind().unwrap() != ObjectKind::Dictionary || !info.dict_has("Title").unwrap() {
        return None;
    }
    Some(decode_text_string(
        &info.dict_get("Title").unwrap().string().unwrap(),
    ))
}

/// Apply, round-trip through the writer, then undo and redo with exactness checks.
fn exercise(doc: &Document, h: &mut History, c: impl Command + 'static, check: impl Fn(&Document)) {
    let before = snapshot(doc);
    run(doc, h, c);
    let after = snapshot(doc);
    check(doc);
    check(&roundtrip(doc));
    h.undo(doc).unwrap().expect("undo");
    assert_matches(doc, &before, "after undo");
    h.redo(doc).unwrap().expect("redo");
    assert_matches(doc, &after, "after redo");
    check(doc);
    check(&roundtrip(doc));
    // Leave the document as it was so callers can chain scenarios.
    h.undo(doc).unwrap().expect("final undo");
    assert_matches(doc, &before, "after final undo");
}

#[test]
fn set_info_field_existing_and_new() {
    let (doc, mut h) = fresh(2);
    exercise(
        &doc,
        &mut h,
        SetInfoField::new(InfoField::Title, Some("Zażółć".into())),
        |d| {
            assert_eq!(info_title(d).as_deref(), Some("Zażółć"));
        },
    );
    for f in [InfoField::Author, InfoField::Subject, InfoField::Keywords] {
        exercise(&doc, &mut h, SetInfoField::new(f, Some("x".into())), |d| {
            let info = d.trailer().unwrap().dict_get("Info").unwrap();
            assert!(info.dict_has(f.key()).unwrap());
        });
    }
    exercise(
        &doc,
        &mut h,
        SetInfoField::new(InfoField::Title, None),
        |d| {
            assert_eq!(info_title(d), None);
        },
    );

    // A file with no /Info at all: the command creates it and records the trailer change.
    let nested = open(build_nested_pdf());
    let mut h = History::new();
    h.set_verify(true);
    assert!(
        nested
            .trailer()
            .unwrap()
            .dict_get("Info")
            .unwrap()
            .is_null()
            .unwrap()
    );
    let cs = h
        .execute(
            &nested,
            Box::new(SetInfoField::new(InfoField::Title, Some("T".into()))),
        )
        .unwrap();
    assert_eq!(cs.created.len(), 1);
    assert_eq!(info_title(&roundtrip(&nested)).as_deref(), Some("T"));
    h.undo(&nested).unwrap();
    assert!(
        nested
            .trailer()
            .unwrap()
            .dict_get("Info")
            .unwrap()
            .is_null()
            .unwrap()
    );
}

#[test]
fn rotate_pages() {
    let (doc, mut h) = fresh(4);
    let rot = |d: &Document, i: usize| {
        let p = d.page(i).unwrap();
        if p.dict_has("Rotate").unwrap() {
            p.dict_get("Rotate").unwrap().as_int().unwrap()
        } else {
            0
        }
    };
    exercise(&doc, &mut h, RotatePages::new(vec![1, 3], 90), |d| {
        assert_eq!([rot(d, 0), rot(d, 1), rot(d, 2), rot(d, 3)], [0, 90, 0, 90]);
    });
    // Rotating back to zero removes the key; negative deltas wrap.
    run(&doc, &mut h, RotatePages::new(vec![1], 180));
    assert_eq!(rot(&doc, 1), 180);
    run(&doc, &mut h, RotatePages::new(vec![1], -270));
    assert_eq!(rot(&doc, 1), 270);
    run(&doc, &mut h, RotatePages::new(vec![1], 90));
    assert!(!doc.page(1).unwrap().dict_has("Rotate").unwrap());
    let cs = run(&doc, &mut h, RotatePages::new(vec![0, 2], 180));
    assert_eq!(cs.repaint, RepaintHint::Pages(vec![0, 2]));
    // Errors leave nothing behind.
    let snap = snapshot(&doc);
    assert!(
        h.execute(&doc, Box::new(RotatePages::new(vec![0], 45)))
            .is_err()
    );
    assert!(
        h.execute(&doc, Box::new(RotatePages::new(vec![9], 90)))
            .is_err()
    );
    assert_matches(&doc, &snap, "after rejected rotates");
}

#[test]
fn rotate_inherited() {
    let doc = open(build_nested_pdf());
    let mut h = History::new();
    h.set_verify(true);
    run(&doc, &mut h, RotatePages::new(vec![0, 2], 90));
    assert_eq!(
        doc.page(0)
            .unwrap()
            .dict_get("Rotate")
            .unwrap()
            .as_int()
            .unwrap(),
        180
    );
    assert_eq!(
        doc.page(2)
            .unwrap()
            .dict_get("Rotate")
            .unwrap()
            .as_int()
            .unwrap(),
        90
    );
    // Going back to the inherited-zero case must keep an explicit 0 where the parent says 90.
    run(&doc, &mut h, RotatePages::new(vec![1], 270));
    assert_eq!(
        doc.page(1)
            .unwrap()
            .dict_get("Rotate")
            .unwrap()
            .as_int()
            .unwrap(),
        0
    );
}

#[test]
fn delete_pages() {
    let (doc, mut h) = fresh(5);
    exercise(&doc, &mut h, DeletePages::new(vec![3, 1]), |d| {
        assert_eq!(labels(d), ["Page 1", "Page 3", "Page 5"]);
    });
    assert!(
        h.execute(&doc, Box::new(DeletePages::new(vec![0, 1, 2, 3, 4])))
            .is_err()
    );
    assert!(
        h.execute(&doc, Box::new(DeletePages::new(vec![5])))
            .is_err()
    );
    assert!(h.execute(&doc, Box::new(DeletePages::new(vec![]))).is_err());
    let cs = run(&doc, &mut h, DeletePages::new(vec![0]));
    assert_eq!(cs.repaint, RepaintHint::All);
}

#[test]
fn move_pages() {
    let (doc, mut h) = fresh(5);
    // Move pages 1 and 2 (0-based) to the end.
    exercise(&doc, &mut h, MovePages::new(vec![1, 2], 3), |d| {
        assert_eq!(
            labels(d),
            ["Page 1", "Page 4", "Page 5", "Page 2", "Page 3"]
        );
    });
    // Move the last page to the front.
    exercise(&doc, &mut h, MovePages::new(vec![4], 0), |d| {
        assert_eq!(
            labels(d),
            ["Page 5", "Page 1", "Page 2", "Page 3", "Page 4"]
        );
    });
    // Into the middle, unsorted selection.
    exercise(&doc, &mut h, MovePages::new(vec![3, 0], 1), |d| {
        assert_eq!(
            labels(d),
            ["Page 2", "Page 1", "Page 4", "Page 3", "Page 5"]
        );
    });
    assert!(
        h.execute(&doc, Box::new(MovePages::new(vec![0], 5)))
            .is_err()
    );
}

#[test]
fn duplicate_pages() {
    let (doc, mut h) = fresh(3);
    exercise(&doc, &mut h, DuplicatePages::new(vec![0, 1]), |d| {
        assert_eq!(
            labels(d),
            ["Page 1", "Page 2", "Page 1", "Page 2", "Page 3"]
        );
    });
    let cs = run(&doc, &mut h, DuplicatePages::new(vec![2]));
    assert_eq!(cs.created.len(), 1, "one new page object: {:?}", cs.created);
    assert_eq!(labels(&doc), ["Page 1", "Page 2", "Page 3", "Page 3"]);
    // The copy is a distinct object.
    assert_ne!(doc.page(2).unwrap().id(), doc.page(3).unwrap().id());
}

#[test]
fn duplicate_pages_copies_annotations() {
    let (doc, mut h) = fresh(2);
    // Give page 0 an annotation (setup, not through History).
    let annot = doc
        .make_indirect(
            &doc.parse_object(
                "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (note) >>",
            )
            .unwrap(),
        )
        .unwrap();
    annot.dict_set("P", &doc.page(0).unwrap()).unwrap();
    let arr = doc.new_array();
    arr.array_push(&annot).unwrap();
    doc.page(0).unwrap().dict_set("Annots", &arr).unwrap();

    run(&doc, &mut h, DuplicatePages::new(vec![0]));
    let copy = doc.page(1).unwrap();
    let a2 = copy.dict_get("Annots").unwrap().array_get(0).unwrap();
    assert_ne!(a2.id(), annot.id());
    assert_eq!(a2.dict_get("P").unwrap().id(), copy.id());
    assert_eq!(a2.dict_get("Contents").unwrap().string().unwrap(), b"note");
    // Original page still points at the original annotation.
    let orig_a = doc
        .page(0)
        .unwrap()
        .dict_get("Annots")
        .unwrap()
        .array_get(0)
        .unwrap();
    assert_eq!(orig_a.id(), annot.id());
    let rt = roundtrip(&doc);
    assert_eq!(rt.page_count().unwrap(), 3);
    h.undo(&doc).unwrap();
    assert_eq!(doc.page_count().unwrap(), 2);
    assert_eq!(
        doc.page(0)
            .unwrap()
            .dict_get("Annots")
            .unwrap()
            .array_get(0)
            .unwrap()
            .id(),
        annot.id()
    );
}

#[test]
fn insert_blank_page() {
    let (doc, mut h) = fresh(2);
    exercise(
        &doc,
        &mut h,
        InsertBlankPage::new(1, 595.276, 841.89),
        |d| {
            assert_eq!(labels(d), ["Page 1", "blank", "Page 2"]);
            let mb: Vec<f64> = d
                .page(1)
                .unwrap()
                .dict_get("MediaBox")
                .unwrap()
                .array_items()
                .unwrap()
                .iter()
                .map(|o| o.as_f64().unwrap())
                .collect();
            assert_eq!(mb, [0.0, 0.0, 595.276, 841.89]);
            assert_eq!(
                d.page(1).unwrap().dict_get("Type").unwrap().name().unwrap(),
                b"Page"
            );
        },
    );
    exercise(&doc, &mut h, InsertBlankPage::letter(0), |d| {
        assert_eq!(labels(d), ["blank", "Page 1", "Page 2"]);
    });
    exercise(&doc, &mut h, InsertBlankPage::letter(2), |d| {
        assert_eq!(labels(d), ["Page 1", "Page 2", "blank"]);
    });
    assert!(
        h.execute(&doc, Box::new(InsertBlankPage::letter(3)))
            .is_err()
    );
    assert!(
        h.execute(&doc, Box::new(InsertBlankPage::new(0, f64::NAN, 10.0)))
            .is_err()
    );
}

#[test]
fn set_page_box() {
    let (doc, mut h) = fresh(2);
    let boxv = |d: &Document, key: &str| -> Option<Vec<f64>> {
        let p = d.page(1).unwrap();
        p.dict_has(key).unwrap().then(|| {
            p.dict_get(key)
                .unwrap()
                .array_items()
                .unwrap()
                .iter()
                .map(|o| o.as_f64().unwrap())
                .collect()
        })
    };
    exercise(
        &doc,
        &mut h,
        SetPageBox::new(1, BoxKind::CropBox, Some([300.0, 400.5, 10.0, 20.0])),
        |d| {
            assert_eq!(boxv(d, "CropBox").unwrap(), [10.0, 20.0, 300.0, 400.5]);
        },
    );
    exercise(
        &doc,
        &mut h,
        SetPageBox::new(1, BoxKind::MediaBox, Some([0.0, 0.0, 200.0, 300.0])),
        |d| {
            assert_eq!(boxv(d, "MediaBox").unwrap(), [0.0, 0.0, 200.0, 300.0]);
        },
    );
    run(
        &doc,
        &mut h,
        SetPageBox::new(1, BoxKind::TrimBox, Some([1.0, 1.0, 50.0, 50.0])),
    );
    exercise(
        &doc,
        &mut h,
        SetPageBox::new(1, BoxKind::TrimBox, None),
        |d| {
            assert_eq!(boxv(d, "TrimBox"), None);
        },
    );
    assert!(
        h.execute(&doc, Box::new(SetPageBox::new(1, BoxKind::MediaBox, None)))
            .is_err()
    );
    assert!(
        h.execute(
            &doc,
            Box::new(SetPageBox::new(
                1,
                BoxKind::CropBox,
                Some([0.0, 0.0, 0.0, 5.0])
            ))
        )
        .is_err()
    );
    assert!(
        h.execute(
            &doc,
            Box::new(SetPageBox::new(
                7,
                BoxKind::CropBox,
                Some([0.0, 0.0, 9.0, 9.0])
            ))
        )
        .is_err()
    );
}

#[test]
fn composite_is_one_undo_step() {
    let (doc, mut h) = fresh(4);
    let before = snapshot(&doc);
    let c = CompositeCommand::new(vec![
        Box::new(RotatePages::new(vec![0], 90)),
        Box::new(DeletePages::new(vec![3])),
        Box::new(InsertBlankPage::letter(1)),
        Box::new(SetInfoField::new(InfoField::Author, Some("A".into()))),
    ])
    .with_label("batch");
    run(&doc, &mut h, c);
    assert_eq!(h.undo_len(), 1);
    assert_eq!(labels(&doc), ["Page 1", "blank", "Page 2", "Page 3"]);
    let after = snapshot(&doc);
    h.undo(&doc).unwrap();
    assert_matches(&doc, &before, "composite undo");
    assert_eq!(labels(&doc), expected_labels(4));
    h.redo(&doc).unwrap();
    assert_matches(&doc, &after, "composite redo");
    assert_eq!(roundtrip(&doc).page_count().unwrap(), 4);

    // A failing child rolls the whole composite back.
    let snap = snapshot(&doc);
    let bad = CompositeCommand::new(vec![
        Box::new(RotatePages::new(vec![0], 90)),
        Box::new(DeletePages::new(vec![99])),
    ]);
    assert!(h.execute(&doc, Box::new(bad)).is_err());
    assert_matches(&doc, &snap, "failed composite");
    assert_eq!(labels(&doc), ["Page 1", "blank", "Page 2", "Page 3"]);
}

#[test]
fn nested_page_tree_undo_is_exact() {
    let doc = open(build_nested_pdf());
    let mut h = History::new();
    h.set_verify(true);
    let before = snapshot(&doc);
    assert_eq!(labels(&doc), ["Page 1", "Page 2", "Page 3"]);
    run(&doc, &mut h, DeletePages::new(vec![0]));
    assert_eq!(labels(&doc), ["Page 2", "Page 3"]);
    // Flattening pushed the inherited attributes into the surviving pages.
    assert!(doc.page(0).unwrap().dict_has("MediaBox").unwrap());
    run(&doc, &mut h, MovePages::new(vec![0], 1));
    assert_eq!(labels(&doc), ["Page 3", "Page 2"]);
    h.undo(&doc).unwrap();
    h.undo(&doc).unwrap();
    assert_matches(&doc, &before, "nested undo");
    assert_eq!(labels(&doc), ["Page 1", "Page 2", "Page 3"]);
    h.redo(&doc).unwrap();
    assert_eq!(labels(&doc), ["Page 2", "Page 3"]);
    assert_eq!(labels(&roundtrip(&doc)), ["Page 2", "Page 3"]);
}

#[test]
fn registry_replay() {
    let reg = CommandRegistry::with_builtin();
    let originals: Vec<Box<dyn Command>> = vec![
        Box::new(SetInfoField::new(InfoField::Subject, Some("S".into()))),
        Box::new(RotatePages::new(vec![0, 1], 270)),
        Box::new(MovePages::new(vec![0], 1)),
        Box::new(DuplicatePages::new(vec![1])),
        Box::new(InsertBlankPage::new(1, 100.0, 200.5)),
        Box::new(SetPageBox::new(
            0,
            BoxKind::ArtBox,
            Some([1.0, 2.0, 30.0, 40.0]),
        )),
        Box::new(CompositeCommand::new(vec![Box::new(DeletePages::new(vec![2]))]).with_label("x")),
    ];
    // Replay the recorded stream on a second document and compare results.
    let (a, mut ha) = fresh(4);
    let (b, mut hb) = fresh(4);
    let records: Vec<_> = originals
        .iter()
        .map(|c| describe_command(c.as_ref()))
        .collect();
    for c in originals {
        ha.execute(&a, c).unwrap();
    }
    for r in &records {
        hb.execute(&b, reg.create_from_record(r).unwrap()).unwrap();
    }
    assert_eq!(labels(&a), labels(&b));
    assert_eq!(snapshot(&a), snapshot(&b));
    assert!(matches!(
        reg.create("nope", &serde_json::Value::Null),
        Err(Error::UnknownCommand(_))
    ));
    assert!(matches!(
        reg.create("rotate_pages", &serde_json::json!({"pages": "x"})),
        Err(Error::InvalidParams(_))
    ));
    for c in reg.names() {
        assert!(!c.is_empty());
    }
}

#[test]
fn describe_is_localizable() {
    let t = RotatePages::new(vec![0, 1], -90).describe();
    assert_eq!(t.key, "cmd.rotate-pages");
    assert_eq!(t.args["count"], "2");
    assert_eq!(t.args["degrees"], "-90");
}

/// After undoing back to a nested tree, the next page edit must still push inherited
/// attributes into the pages it flattens (qpdf's "already pushed" flag must not go stale).
#[test]
fn nested_tree_reflatten_after_undo_keeps_inheritance() {
    let doc = open(build_nested_pdf());
    let mut h = History::new();
    h.set_verify(true);
    run(&doc, &mut h, DeletePages::new(vec![2]));
    h.undo(&doc).unwrap();
    run(&doc, &mut h, DeletePages::new(vec![2]));
    for i in 0..2 {
        let p = doc.page(i).unwrap();
        assert!(
            p.dict_has("MediaBox").unwrap(),
            "page {i} lost its inherited MediaBox"
        );
        assert_eq!(p.dict_get("Rotate").unwrap().as_int().unwrap(), 90);
    }
}
