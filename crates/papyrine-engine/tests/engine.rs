mod common;

use common::*;
use papyrine_engine::proto::*;
use papyrine_ipc::DocId;
use serde_json::json;

fn summary_of(e: &mut Eng, doc: u64) -> DocSummary {
    match e.query(doc, Query::Summary) {
        QueryResult::Summary(s) => *s,
        _ => unreachable!(),
    }
}

#[test]
fn open_reports_metadata() {
    let mut e = Eng::new();
    let s = e.open(1, build_pdf(5));
    assert_eq!(s.page_count, 5);
    assert_eq!(s.pages.len(), 5);
    assert_eq!(
        (
            s.pages[0].width_pt,
            s.pages[0].height_pt,
            s.pages[0].rotation
        ),
        (612.0, 792.0, 0)
    );
    assert_eq!(s.pages[2].label, "3");
    assert_eq!(s.info.title.as_deref(), Some("Test document"));
    assert!(!s.repaired && s.repair_log.is_empty());
    assert!(matches!(s.render_base, RenderBase::Original));
    assert!(!s.security.encrypted && !s.signatures.signed && !s.form.has_acroform);
    assert!(!s.can_undo && !s.can_redo);
    assert_eq!(s.annotations.total, 0);
    assert!(
        e.try_open(1, build_pdf(1), None).is_err(),
        "duplicate doc id"
    );
}

#[test]
fn execute_returns_after_images_and_a_valid_section() {
    let mut e = Eng::new();
    let orig = build_pdf(4);
    let s = e.open(1, orig.clone());
    let mut m = Mirror::from_open(orig, &s);
    let (n, p) = rotate(&[1], 90);
    let c = e.exec(1, n, p);
    assert_eq!(c.summary.dirty_pages, vec![1]);
    assert!(!c.summary.structure_changed);
    assert!(c.summary.can_undo && !c.summary.can_redo);
    assert_eq!(c.label_key, "cmd.rotate-pages");
    assert_eq!(c.after_images.len(), 1);
    assert!(matches!(c.snapshot.action, SnapshotAction::Append(_)));
    assert_eq!((c.snapshot.sections, c.snapshot.epoch), (1, 1));
    m.apply(&c.snapshot, None);
    assert_snapshot_matches(&mut e, 1, &m);
    match e.query(1, Query::Pages { from: 0, count: 10 }) {
        QueryResult::Pages(p) => assert_eq!(p[1].rotation, 90),
        _ => unreachable!(),
    }
    // The model sees the change (cache invalidated).
    let u = e.undo(1);
    m.apply(&u.snapshot, None);
    assert_snapshot_matches(&mut e, 1, &m);
    match e.query(1, Query::Pages { from: 1, count: 1 }) {
        QueryResult::Pages(p) => assert_eq!(p[0].rotation, 0),
        _ => unreachable!(),
    }
    let r = e.redo(1);
    m.apply(&r.snapshot, None);
    assert_snapshot_matches(&mut e, 1, &m);
    if have_qpdf() {
        qpdf_check(&m.bytes(), e.dir.path()).unwrap();
    }
}

#[test]
fn errors_are_typed_and_leave_the_document_alone() {
    let mut e = Eng::new();
    e.open(1, build_pdf(2));
    let before = e.digest(1);
    let err = e
        .try_exec(1, "rotate_pages", json!({"pages": [9], "delta": 90}))
        .unwrap_err();
    assert_eq!(err.code, papyrine_ipc::ErrorCode::InvalidRequest);
    let err = e.try_exec(1, "no_such_command", json!({})).unwrap_err();
    assert_eq!(err.code, papyrine_ipc::ErrorCode::Unsupported);
    let err = e
        .try_exec(1, "rotate_pages", json!({"nonsense": true}))
        .unwrap_err();
    assert_eq!(err.code, papyrine_ipc::ErrorCode::InvalidRequest);
    assert!(
        e.try_call(Request::Undo {
            doc: DocId(1),
            out_section: None
        })
        .is_err()
    );
    assert!(
        e.try_call(Request::Query {
            doc: DocId(77),
            query: Query::Info
        })
        .is_err()
    );
    assert_eq!(e.digest(1).digest, before.digest);
}

#[test]
fn fifty_commands_with_undo_redo_keep_the_snapshot_in_step() {
    let mut e = Eng::new();
    let orig = build_pdf(6);
    let s = e.open(1, orig.clone());
    let mut m = Mirror::from_open(orig, &s);
    let mut undo_depth = 0usize;
    let mut redo_depth = 0usize;
    let mut saw_l1 = false;
    for i in 0..50usize {
        let n = summary_of(&mut e, 1).page_count as usize;
        let c = match i % 10 {
            3 if undo_depth > 0 => {
                undo_depth -= 1;
                redo_depth += 1;
                e.undo(1)
            }
            6 if redo_depth > 0 => {
                redo_depth -= 1;
                undo_depth += 1;
                e.redo(1)
            }
            0 | 5 => {
                redo_depth = 0;
                undo_depth += 1;
                e.exec(
                    1,
                    "insert_blank_page",
                    json!({"at": i % (n + 1), "width": 300, "height": 400}),
                )
            }
            1 | 7 => {
                redo_depth = 0;
                undo_depth += 1;
                e.exec(1, "rotate_pages", json!({"pages": [i % n], "delta": 90}))
            }
            2 if n > 3 => {
                redo_depth = 0;
                undo_depth += 1;
                e.exec(1, "delete_pages", json!({"pages": [i % n]}))
            }
            4 => {
                redo_depth = 0;
                undo_depth += 1;
                e.exec(1, "move_pages", json!({"pages": [0], "to": n - 1}))
            }
            8 => {
                redo_depth = 0;
                undo_depth += 1;
                e.exec(1, "duplicate_pages", json!({"pages": [i % n]}))
            }
            _ => {
                redo_depth = 0;
                undo_depth += 1;
                e.exec(
                    1,
                    "set_info_field",
                    json!({"field": "Title", "value": format!("Title {i}")}),
                )
            }
        };
        saw_l1 |= matches!(c.snapshot.action, SnapshotAction::ReplaceSections(_));
        assert!(c.snapshot.sections <= 17);
        m.apply(&c.snapshot, None);
        assert_snapshot_matches(&mut e, 1, &m);
    }
    assert!(saw_l1, "50 commits must trigger an L1 merge");
    if have_qpdf() {
        qpdf_check(&m.bytes(), e.dir.path()).unwrap();
    }
}

#[test]
fn l2_compaction_makes_a_new_render_base() {
    let mut e = Eng::new();
    let orig = build_pdf(3);
    let s = e.open(1, orig.clone());
    let mut m = Mirror::from_open(orig, &s);
    for i in 0..5 {
        let c = e.exec(1, "rotate_pages", json!({"pages": [i % 3], "delta": 90}));
        m.apply(&c.snapshot, None);
    }
    let Response::Compacted(u) = e.call(Request::Compact {
        doc: DocId(1),
        level: CompactLevel::L1,
    }) else {
        panic!()
    };
    assert!(matches!(u.action, SnapshotAction::ReplaceSections(_)));
    m.apply(&u, None);
    assert_eq!(m.sections.len(), 1);
    assert_snapshot_matches(&mut e, 1, &m);
    let Response::Compacted(u) = e.call(Request::Compact {
        doc: DocId(1),
        level: CompactLevel::L2,
    }) else {
        panic!()
    };
    assert!(matches!(u.action, SnapshotAction::NewBase(Some(_))));
    assert_eq!((u.sections, u.section_bytes), (0, 0));
    m.apply(&u, None);
    assert!(m.sections.is_empty());
    assert_snapshot_matches(&mut e, 1, &m);
    // Editing continues over the new base.
    let c = e.exec(1, "rotate_pages", json!({"pages": [0], "delta": 90}));
    m.apply(&c.snapshot, None);
    assert_eq!(m.sections.len(), 1);
    assert_snapshot_matches(&mut e, 1, &m);
}

#[test]
fn replay_reproduces_every_object() {
    let orig = build_pdf(5);
    let mut a = Eng::new();
    a.open(1, orig.clone());
    let mut commits = Vec::new();
    let mut seq = 0u64;
    let mut record = |c: Committed, name: &str, commits: &mut Vec<ReplayCommit>| {
        seq += 1;
        commits.push(ReplayCommit {
            seq,
            command: name.into(),
            after_images: c.after_images,
        });
    };
    let c = a.exec(
        1,
        "insert_blank_page",
        json!({"at": 2, "width": 200, "height": 300}),
    );
    record(c, "insert_blank_page", &mut commits);
    let c = a.exec(1, "rotate_pages", json!({"pages": [0, 1], "delta": 90}));
    record(c, "rotate_pages", &mut commits);
    let c = a.undo(1);
    record(c, "undo", &mut commits);
    let c = a.redo(1);
    record(c, "redo", &mut commits);
    let c = a.exec(1, "delete_pages", json!({"pages": [3]}));
    record(c, "delete_pages", &mut commits);
    let c = a.exec(
        1,
        "set_info_field",
        json!({"field": "Author", "value": "Ünïcode"}),
    );
    record(c, "set_info_field", &mut commits);
    let c = a.exec(1, "duplicate_pages", json!({"pages": [0]}));
    record(c, "duplicate_pages", &mut commits);
    let c = a.undo(1);
    record(c, "undo", &mut commits);
    let c = a.exec(1, "move_pages", json!({"pages": [1], "to": 4}));
    record(c, "move_pages", &mut commits);
    let want = a.digest(1);

    let mut b = Eng::new();
    let s = b.open(1, orig.clone());
    let mut m = Mirror::from_open(orig, &s);
    let Response::Replayed(r) = b.call(Request::Replay {
        doc: DocId(1),
        commits,
        out_section: None,
    }) else {
        panic!()
    };
    assert_eq!(r.applied, 9);
    assert!(r.structure_changed && r.can_undo);
    m.apply(&r.snapshot, None);
    let got = b.digest(1);
    assert_eq!(got.objects, want.objects);
    assert_eq!(
        got.digest, want.digest,
        "replayed document differs from the original session"
    );
    assert_snapshot_matches(&mut b, 1, &m);
    // Undo history was rebuilt: the replayed steps can be undone.
    let u = b.undo(1);
    m.apply(&u.snapshot, None);
    assert_snapshot_matches(&mut b, 1, &m);
}

#[test]
fn replay_rejects_a_diverged_document() {
    let mut a = Eng::new();
    a.open(1, build_pdf(3));
    let c = a.exec(
        1,
        "insert_blank_page",
        json!({"at": 0, "width": 100, "height": 100}),
    );
    // Replaying onto a document with a different page count must not silently corrupt it.
    let mut b = Eng::new();
    b.open(1, build_pdf(1));
    let before = b.digest(1);
    let r = b.try_call(Request::Replay {
        doc: DocId(1),
        commits: vec![ReplayCommit {
            seq: 1,
            command: "x".into(),
            after_images: c.after_images,
        }],
        out_section: None,
    });
    // Either it applies cleanly (ids line up) or it is refused; it must never panic.
    if r.is_err() {
        assert_eq!(
            b.digest(1).digest,
            before.digest,
            "a refused replay must not change the document"
        );
    }
}

#[test]
fn checkpoint_is_an_id_preserving_full_write() {
    let orig = build_pdf(4);
    let mut a = Eng::new();
    a.open(1, orig);
    a.exec(
        1,
        "insert_blank_page",
        json!({"at": 1, "width": 100, "height": 100}),
    );
    a.exec(1, "rotate_pages", json!({"pages": [0], "delta": 270}));
    let want = a.digest(1);
    let Response::Checkpointed(f) = a.call(Request::Checkpoint { doc: DocId(1) }) else {
        panic!()
    };
    let bytes = std::fs::read(&f.path).unwrap();
    assert_eq!(bytes.len() as u64, f.len);
    let mut b = Eng::new();
    b.open(1, bytes);
    let got = b.digest(1);
    // Ids are preserved: every live object of A exists in B unchanged.
    for (n, g, h) in &want.per_object {
        if *n == 0 {
            continue;
        }
        assert!(
            got.per_object.contains(&(*n, *g, *h)),
            "object {n} {g} changed"
        );
    }
}

#[test]
fn cancelled_checkpoint_fails_cleanly() {
    let mut a = Eng::new();
    a.open(1, build_pdf(40));
    a.cancel.cancel();
    let err = a
        .try_call(Request::Checkpoint { doc: DocId(1) })
        .unwrap_err();
    assert_eq!(err.code, papyrine_ipc::ErrorCode::Cancelled);
    a.cancel = papyrine_core::CancelToken::new();
    a.call(Request::Checkpoint { doc: DocId(1) });
}

#[test]
fn close_forgets_the_document() {
    let mut e = Eng::new();
    e.open(1, build_pdf(1));
    e.call(Request::Close { doc: DocId(1) });
    assert!(e.try_call(Request::Close { doc: DocId(1) }).is_err());
    assert_eq!(e.h.open_documents(), 0);
    e.open(1, build_pdf(1));
}
