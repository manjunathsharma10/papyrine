//! Organise-pages commands: round trip (apply -> write -> reopen), interop (qpdf --check,
//! Poppler, PDFium), and exact undo/redo through the shadow verifier.

mod common;
mod organise_support;

use common::{assert_matches, snapshot};
use organise_support::*;
use papyrine_cos::Document;
use papyrine_ops::*;

fn hist() -> History {
    let mut h = History::new();
    h.set_verify(true);
    h
}

/// Apply `c`, hand the live summary to `check`, write and reopen (same summary again), run
/// the three oracles on the written bytes, then undo and redo with exactness checks and leave
/// the document as it was.
fn exercise(
    doc: &Document,
    h: &mut History,
    c: impl Command + 'static,
    check: impl Fn(&Summary),
) -> Summary {
    let before = snapshot(doc);
    let before_sum = summarize(doc);
    h.execute(doc, Box::new(c)).expect("execute");
    let after = snapshot(doc);
    let sum = summarize(doc);
    check(&sum);
    let bytes = write(doc);
    interop(&bytes, &sum.texts);
    let reopened = open(bytes);
    assert!(
        reopened.repair_log().is_empty(),
        "written file needed repair"
    );
    let rt = summarize(&reopened);
    assert_eq!(rt, sum, "summary changed across write + reopen");
    h.undo(doc).unwrap().expect("undo");
    assert_matches(doc, &before, "after undo");
    assert_eq!(summarize(doc), before_sum, "summary after undo");
    h.redo(doc).unwrap().expect("redo");
    assert_matches(doc, &after, "after redo");
    assert_eq!(summarize(doc), sum, "summary after redo");
    h.undo(doc).unwrap().expect("final undo");
    assert_matches(doc, &before, "after final undo");
    sum
}

/// The file as qpdf's QDF mode prints it (streams decoded), to look for leftover content.
fn qdf_text(doc: &Document) -> String {
    let out = doc
        .write(&papyrine_cos::WriteOptions {
            qdf: true,
            static_id: true,
            stream_data: papyrine_cos::StreamMode::Uncompress,
            ..Default::default()
        })
        .unwrap()
        .into_vec();
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn insert_pages_from_pdf() {
    let dest = open(build(&Spec::rich("B", 3)));
    let src_bytes = build(&Spec::rich("A", 4));
    let mut h = hist();
    // Source pages 2 and 3 (indices 1, 2) go between B's pages 1 and 2.
    let cmd = InsertPagesFromPdf::new(src_bytes.clone(), 1).pages(vec![1, 2]);
    let sum = exercise(&dest, &mut h, cmd, |s| {
        assert_eq!(
            s.texts,
            ["T:B P:1", "T:A P:2", "T:A P:3", "T:B P:2", "T:B P:3"]
        );
        // Fields: the inserted fld2 / fld3 collide with B's and are renamed; hierarchy-free.
        let names: Vec<&str> = s.fields.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(names, ["fld1", "fld2", "fld3", "fld2_2", "fld3_2"]);
        let f = |n: &str| s.fields.iter().find(|f| f.0 == n).unwrap();
        assert_eq!(f("fld2_2").1, "A-v2");
        assert_eq!(f("fld2_2").2, [Some(1)]);
        assert_eq!(f("fld3_2").1, "A-v3");
        assert_eq!(f("fld3_2").2, [Some(2)]);
        assert_eq!(f("fld2").1, "B-v2");
        assert_eq!(f("fld2").2, [Some(3)]);
        // Named destinations that lead to inserted pages came along, B's moved with its pages.
        assert_eq!(s.dests["A.dest2"], Some(1));
        assert_eq!(s.dests["A.dest3"], Some(2));
        assert!(!s.dests.contains_key("A.dest1"));
        assert_eq!(s.dests["B.dest2"], Some(3));
        // Links: A p2 keeps both links (to A p3); A p3's links led to page 4, not inserted.
        assert_eq!(s.links[1], [Some(2), Some(2)]);
        assert!(s.links[2].is_empty(), "{:?}", s.links[2]);
        // B's own links follow their targets.
        assert_eq!(s.links[0], [Some(3), Some(3)]);
        // Bookmarks: B's, then A's (placed before nothing later, so at the end of the top level
        // but ordered by page against B's own items: B Chapter 3 targets page 5 -> after).
        let titles: Vec<(usize, &str, Option<usize>)> = s
            .outline
            .iter()
            .map(|(d, t, p)| (*d, t.as_str(), *p))
            .collect();
        assert!(titles.contains(&(0, "B Chapter 1", Some(0))), "{titles:?}");
        assert!(titles.contains(&(1, "A Section 2", Some(1))), "{titles:?}");
        assert!(titles.contains(&(0, "A Chapter 3", Some(2))), "{titles:?}");
        assert!(titles.contains(&(0, "B Chapter 3", Some(4))), "{titles:?}");
        // The inserted group sits before B Chapter 3 (which leads to a later page).
        let pos = |t: &str| titles.iter().position(|x| x.1 == t).unwrap();
        assert!(pos("A Chapter 3") < pos("B Chapter 3"));
        assert!(pos("B Chapter 1") < pos("A Chapter 1"));
        // Page labels: the new pages join the first (roman) range; B's decimal range shifts down
        // the document with its page.
        assert_eq!(s.labels, ["i", "ii", "iii", "iv", "A-5"]);
    });
    assert_eq!(sum.pages, 5);
    // The same command rebuilt from params (journal replay) behaves identically.
    let blobs = std::sync::Arc::new(MemoryBlobs::new());
    let id = blobs.put(src_bytes);
    let mut reg = CommandRegistry::with_builtin();
    InsertPagesFromPdf::register(&mut reg, blobs);
    let rec = serde_json::json!({"name": "insert_pages", "params": {"blob": id, "at": 1, "pages": [1, 2]}});
    let replayed = reg.create_from_record(&rec).unwrap();
    h.execute(&dest, replayed).unwrap();
    assert_eq!(summarize(&dest).texts, sum.texts);
}

#[test]
fn insert_pages_hierarchical_forms_and_name_collisions() {
    let hier = |tag: &str, n: usize| {
        let mut s = Spec::rich(tag, n);
        s.hier = true;
        s
    };
    // Same tag on both sides: every named destination and field name collides.
    let dest = open(build(&hier("D", 3)));
    let src = build(&hier("D", 3));
    let mut h = hist();
    exercise(
        &dest,
        &mut h,
        InsertPagesFromPdf::new(src, 3)
            .pages(vec![0, 1])
            .outline(OutlineMode::PerFile, "second.pdf"),
        |s| {
            let names: Vec<&str> = s.fields.iter().map(|f| f.0.as_str()).collect();
            // Only the two widgets that came along, under one renamed parent (no ghost kids).
            assert_eq!(
                names,
                [
                    "form1.fld1",
                    "form1.fld2",
                    "form1.fld3",
                    "form1_2.fld1",
                    "form1_2.fld2"
                ]
            );
            assert_eq!(s.fields[3].2, [Some(3)]);
            assert_eq!(s.fields[4].2, [Some(4)]);
            assert_eq!(s.fields[3].1, "D-v1");
            // Destinations: the copies got new names and lead to the copies.
            assert_eq!(s.dests["D.dest1"], Some(0));
            assert_eq!(s.dests["D.dest1_2"], Some(3));
            assert_eq!(s.dests["D.dest2_2"], Some(4));
            assert!(!s.dests.contains_key("D.dest3_2"));
            // The inserted page's named link follows the rename.
            assert_eq!(s.links[3], [Some(4), Some(4)]);
            // One bookmark for the file, holding the file's own.
            let top: Vec<&str> = s
                .outline
                .iter()
                .filter(|o| o.0 == 0)
                .map(|o| o.1.as_str())
                .collect();
            assert_eq!(top.last(), Some(&"second.pdf"));
            let wrapper = s.outline.iter().position(|o| o.1 == "second.pdf").unwrap();
            assert_eq!(s.outline[wrapper].2, Some(3));
            assert_eq!(s.outline[wrapper + 1].0, 1);
        },
    );
}

#[test]
fn insert_pages_failures_leave_the_document_alone() {
    let dest = open(build(&Spec::rich("B", 2)));
    let mut h = hist();
    let snap = snapshot(&dest);
    let src = build(&Spec::plain("A", 2));
    assert!(
        h.execute(&dest, Box::new(InsertPagesFromPdf::new(src.clone(), 9)))
            .is_err()
    );
    assert!(
        h.execute(
            &dest,
            Box::new(InsertPagesFromPdf::new(src.clone(), 0).pages(vec![5]))
        )
        .is_err()
    );
    assert!(
        h.execute(
            &dest,
            Box::new(InsertPagesFromPdf::new(b"%PDF-1.4 not really".to_vec(), 0))
        )
        .is_err()
    );
    assert_matches(&dest, &snap, "after rejected inserts");
    assert_eq!(h.undo_len(), 0);
}

#[test]
fn delete_pages_scrubs_references() {
    for hier in [false, true] {
        let mut spec = Spec::rich("A", 5);
        spec.hier = hier;
        let doc = open(build(&spec));
        let mut h = hist();
        assert!(qdf_text(&doc).contains("T:A P:3"));
        exercise(&doc, &mut h, DeletePages::new(vec![2]), |s| {
            assert_eq!(s.texts, ["T:A P:1", "T:A P:2", "T:A P:4", "T:A P:5"]);
            let names: Vec<&str> = s.fields.iter().map(|f| f.0.as_str()).collect();
            let prefix = if hier { "form1." } else { "" };
            assert_eq!(
                names,
                [1, 2, 4, 5]
                    .map(|i| format!("{prefix}fld{i}"))
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                s.fields.iter().map(|f| f.2.clone()).collect::<Vec<_>>(),
                [[Some(0)], [Some(1)], [Some(2)], [Some(3)]]
            );
            // The bookmark that led to page 3 stays as a title without a target; Section 4
            // (a named destination) follows its page.
            let ch3 = s.outline.iter().find(|o| o.1 == "A Chapter 3").unwrap();
            assert_eq!(ch3.2, None);
            let sec4 = s.outline.iter().find(|o| o.1 == "A Section 4").unwrap();
            assert_eq!(sec4.2, Some(2));
            assert!(!s.dests.contains_key("A.dest3"));
            assert_eq!(s.dests["A.dest4"], Some(2));
            // The links that led to page 3 are gone, the others stay.
            assert!(s.links[1].is_empty(), "{:?}", s.links[1]);
            assert_eq!(s.links[0], [Some(1), Some(1)]);
            assert_eq!(s.links[2], [Some(3), Some(3)]);
            // Labels: the decimal range (A-5, ...) started on the deleted page, so it now
            // starts at the next page and keeps its numbers.
            assert_eq!(s.labels, ["i", "ii", "A-5", "A-6"]);
        });
        // After write, nothing of the deleted page is left in the file.
        h.redo(&doc).unwrap();
        let q = qdf_text(&doc);
        assert!(
            !q.contains("T:A P:3"),
            "deleted page content is still in the file (hier={hier})"
        );
        assert!(q.contains("T:A P:4"));
        h.undo(&doc).unwrap();
        assert!(qdf_text(&doc).contains("T:A P:3"));
    }
}

#[test]
fn duplicate_pages_clones_form_fields() {
    let doc = open(build(&Spec::rich("A", 3)));
    let mut h = hist();
    exercise(&doc, &mut h, DuplicatePages::new(vec![0, 1]), |s| {
        assert_eq!(
            s.texts,
            ["T:A P:1", "T:A P:2", "T:A P:1", "T:A P:2", "T:A P:3"]
        );
        let names: Vec<&str> = s.fields.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(names, ["fld1", "fld2", "fld3", "fld1_2", "fld2_2"]);
        // Each page shows its own field; values were copied.
        let f = |n: &str| s.fields.iter().find(|f| f.0 == n).unwrap();
        assert_eq!(f("fld1").2, [Some(0)]);
        assert_eq!(f("fld1_2").2, [Some(2)]);
        assert_eq!(f("fld1_2").1, "A-v1");
        assert_eq!(f("fld2_2").2, [Some(3)]);
        // Labels: range starts after the copies shift (A- range started at page 3).
        assert_eq!(s.labels, ["i", "ii", "iii", "iv", "A-5"]);
    });
}

#[test]
fn insert_blank_page_sizes_and_labels() {
    let doc = open(build(&Spec::rich("A", 3)));
    let mut h = hist();
    let sz = |d: &Document, i: usize| {
        let b = d.page(i).unwrap().dict_get("MediaBox").unwrap();
        (
            b.array_get(2).unwrap().as_f64().unwrap(),
            b.array_get(3).unwrap().as_f64().unwrap(),
        )
    };
    let sum = exercise(
        &doc,
        &mut h,
        InsertBlankPage::preset(1, PageSize::A4, true),
        |s| {
            assert_eq!(s.texts, ["T:A P:1", "", "T:A P:2", "T:A P:3"]);
            assert_eq!(s.labels, ["i", "ii", "iii", "A-5"]);
        },
    );
    assert_eq!(sum.pages, 4);
    h.execute(
        &doc,
        Box::new(InsertBlankPage::preset(3, PageSize::Legal, false)),
    )
    .unwrap();
    assert_eq!(sz(&doc, 3), (612.0, 1008.0));
    h.execute(&doc, Box::new(InsertBlankPage::like_page(0, 3)))
        .unwrap();
    assert_eq!(sz(&doc, 0), (612.0, 1008.0));
    // Same size as a rotated page: the shown size is used.
    h.execute(&doc, Box::new(RotatePages::new(vec![1], 90)))
        .unwrap();
    h.execute(&doc, Box::new(InsertBlankPage::like_page(0, 1)))
        .unwrap();
    assert_eq!(sz(&doc, 0), (792.0, 612.0));
    for _ in 0..4 {
        h.undo(&doc).unwrap().expect("undo");
    }
    assert_eq!(summarize(&doc).pages, 3);
}

#[test]
fn rotate_by_selection_and_move_multi_select() {
    let doc = open(build(&Spec::plain("A", 6)));
    let mut h = hist();
    let rot = |d: &Document, i: usize| {
        let p = d.page(i).unwrap();
        if p.dict_has("Rotate").unwrap() {
            p.dict_get("Rotate").unwrap().as_int().unwrap()
        } else {
            0
        }
    };
    let odd = RotatePages::selected(&PageSelection::parse("odd").unwrap(), 6, 90).unwrap();
    exercise(&doc, &mut h, odd, |_| {});
    h.execute(
        &doc,
        Box::new(
            RotatePages::selected(&PageSelection::parse("2-5 even").unwrap(), 6, 180).unwrap(),
        ),
    )
    .unwrap();
    assert_eq!(
        (0..6).map(|i| rot(&doc, i)).collect::<Vec<_>>(),
        [0, 180, 0, 180, 0, 0]
    );
    h.undo(&doc).unwrap();
    // Move a multi-selection to the front, keeping relative order.
    let sum = exercise(&doc, &mut h, MovePages::new(vec![4, 1, 5], 0), |s| {
        assert_eq!(
            s.texts,
            [
                "T:A P:2", "T:A P:5", "T:A P:6", "T:A P:1", "T:A P:3", "T:A P:4"
            ]
        );
    });
    assert_eq!(sum.pages, 6);
}
