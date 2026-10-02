mod common;

use common::*;
use papyrine_cos::Document;
use papyrine_ops::*;
use proptest::prelude::*;

// ---- shadow verifier -------------------------------------------------------------------------

/// Sets /Title on the Info dictionary without recording it first.
struct ForgetsToTouch;

impl Command for ForgetsToTouch {
    fn name(&self) -> &'static str {
        "buggy_untouched"
    }
    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.buggy")
    }
    fn params(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let doc = cx.doc();
        let info = doc.trailer().unwrap().dict_get("Info").unwrap();
        info.dict_set("Title", &doc.new_string("sneaky").unwrap())
            .unwrap();
        cx.changeset()
    }
}

/// Records the page but edits its content stream (a different object).
struct TouchesWrongObject;

impl Command for TouchesWrongObject {
    fn name(&self) -> &'static str {
        "buggy_wrong_object"
    }
    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.buggy")
    }
    fn params(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let page = cx.doc().page(0).unwrap();
        cx.touch(&page).unwrap();
        let contents = page.dict_get("Contents").unwrap();
        contents.stream_replace(b"q Q", None, None).unwrap();
        cx.changeset()
    }
}

/// Mutates after taking the change set, so the recorded after-image is stale.
struct MutatesAfterChangeset;

impl Command for MutatesAfterChangeset {
    fn name(&self) -> &'static str {
        "buggy_late"
    }
    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.buggy")
    }
    fn params(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let page = cx.doc().page(0).unwrap();
        cx.touch(&page).unwrap();
        page.dict_set("Rotate", &cx.doc().new_int(90)).unwrap();
        let cs = cx.changeset()?;
        page.dict_set("Rotate", &cx.doc().new_int(180)).unwrap();
        Ok(cs)
    }
}

#[test]
fn verifier_catches_unrecorded_mutation() {
    let doc = open(build_pdf(2));
    let mut h = History::new();
    h.set_verify(true);
    match h.execute(&doc, Box::new(ForgetsToTouch)) {
        Err(Error::Unrecorded { command, mismatch }) => {
            assert_eq!(command, "buggy_untouched");
            assert_eq!(mismatch.unrecorded, vec![papyrine_cos::ObjId::new(3, 0)]);
        }
        other => panic!("expected Unrecorded, got {other:?}"),
    }
    assert!(!h.can_undo());

    match h.execute(&doc, Box::new(TouchesWrongObject)) {
        Err(Error::Unrecorded { mismatch, .. }) => {
            assert_eq!(mismatch.unrecorded, vec![papyrine_cos::ObjId::new(5, 0)]);
        }
        other => panic!("expected Unrecorded, got {other:?}"),
    }
    match h.execute(&doc, Box::new(MutatesAfterChangeset)) {
        Err(Error::Unrecorded { mismatch, .. }) => assert_eq!(mismatch.stale_after.len(), 1),
        other => panic!("expected Unrecorded, got {other:?}"),
    }
}

#[test]
fn verifier_off_lets_the_bug_through() {
    // Documents why the verifier runs in every test: without it nothing notices.
    let doc = open(build_pdf(1));
    let mut h = History::new();
    h.set_verify(false);
    h.execute(&doc, Box::new(ForgetsToTouch)).unwrap();
    let before_undo = snapshot(&doc);
    h.undo(&doc).unwrap();
    // The unrecorded change survives undo.
    assert_matches(
        &doc,
        &before_undo,
        "undo cannot revert what was never recorded",
    );
}

#[test]
fn created_ids_are_detected_by_max_id() {
    let doc = open(build_pdf(2));
    let cx = EditContext::new(&doc).unwrap();
    let a = doc.make_indirect(&doc.new_dict()).unwrap();
    let b = doc.new_stream(b"x").unwrap();
    let cs = cx.changeset().unwrap();
    assert_eq!(cs.created, vec![a.id().unwrap(), b.id().unwrap()]);
    assert!(cs.touched.is_empty());
    assert_eq!(cs.after.len(), 2);
}

#[test]
fn touch_rejects_direct_objects() {
    let doc = open(build_pdf(1));
    let mut cx = EditContext::new(&doc).unwrap();
    assert!(matches!(
        cx.touch(&doc.new_dict()),
        Err(Error::NotIndirect(_))
    ));
}

#[test]
fn failed_command_rolls_back() {
    struct HalfDone;
    impl Command for HalfDone {
        fn name(&self) -> &'static str {
            "half_done"
        }
        fn describe(&self) -> LocalizedText {
            LocalizedText::new("cmd.half")
        }
        fn params(&self) -> serde_json::Value {
            serde_json::Value::Null
        }
        fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
            let doc = cx.doc();
            cx.touch_page_tree()?;
            doc.remove_page(&doc.page(0).unwrap()).unwrap();
            let page = doc.page(0).unwrap();
            cx.set_key(&page, "Rotate", &doc.new_int(90))?;
            Err(Error::invalid("boom"))
        }
    }
    let doc = open(build_pdf(3));
    let snap = snapshot(&doc);
    let mut h = History::new();
    h.set_verify(true);
    assert!(h.execute(&doc, Box::new(HalfDone)).is_err());
    assert_matches(&doc, &snap, "rollback");
    assert_eq!(labels(&doc), expected_labels(3));
    assert!(!h.can_undo());
    // The document still edits normally afterwards.
    h.execute(&doc, Box::new(DeletePages::new(vec![1])))
        .unwrap();
    assert_eq!(labels(&doc), ["Page 1", "Page 3"]);
}

// ---- history: stacks, budget, spill ----------------------------------------------------------

#[test]
fn new_command_clears_redo() {
    let doc = open(build_pdf(3));
    let mut h = History::new();
    h.execute(&doc, Box::new(RotatePages::new(vec![0], 90)))
        .unwrap();
    h.execute(&doc, Box::new(DeletePages::new(vec![1])))
        .unwrap();
    h.undo(&doc).unwrap();
    assert!(h.can_redo());
    assert_eq!(h.redo_description().unwrap().key, "cmd.delete-pages");
    h.execute(&doc, Box::new(RotatePages::new(vec![2], 90)))
        .unwrap();
    assert!(!h.can_redo());
    assert_eq!(h.undo_len(), 2);
    assert!(h.redo(&doc).unwrap().is_none());
    h.clear();
    assert!(h.undo(&doc).unwrap().is_none());
}

#[test]
fn spills_to_directory_and_reloads() {
    let dir = tempfile::tempdir().unwrap();
    let store = DirStore::new(dir.path().join("hist")).unwrap();
    // Budget of one byte: everything but the newest undo entry spills.
    let mut h = History::with_store(1, Box::new(store));
    h.set_verify(true);
    let doc = open(build_pdf(6));
    let start = snapshot(&doc);
    let mut snaps = vec![start.clone()];
    let cmds: Vec<Box<dyn Command>> = vec![
        Box::new(RotatePages::new(vec![0, 1], 90)),
        Box::new(MovePages::new(vec![0], 4)),
        Box::new(InsertBlankPage::letter(2)),
        Box::new(SetInfoField::new(InfoField::Title, Some("spill".into()))),
        Box::new(DeletePages::new(vec![5])),
    ];
    for c in cmds {
        h.execute(&doc, c).unwrap();
        snaps.push(snapshot(&doc));
    }
    assert_eq!(h.spilled_entries(), 4);
    let files = |d: &std::path::Path| std::fs::read_dir(d.join("hist")).unwrap().count();
    assert_eq!(files(dir.path()), 4);
    assert!(h.resident_bytes() > 0);

    for k in (0..5).rev() {
        h.undo(&doc).unwrap().unwrap();
        assert_matches(&doc, &snaps[k], "spilled undo");
    }
    for snap in &snaps[1..] {
        h.redo(&doc).unwrap().unwrap();
        assert_matches(&doc, snap, "spilled redo");
    }
    // New command drops the redo stack (nothing to drop here) and clear() removes the files.
    h.undo(&doc).unwrap();
    h.undo(&doc).unwrap();
    h.undo(&doc).unwrap();
    h.execute(&doc, Box::new(RotatePages::new(vec![3], 90)))
        .unwrap();
    h.clear();
    assert_eq!(files(dir.path()), 0);
}

#[test]
fn large_streams_spill_and_restore() {
    // A command that rewrites a big stream: the before-image carries the raw bytes.
    struct Rewrite(Vec<u8>);
    impl Command for Rewrite {
        fn name(&self) -> &'static str {
            "rewrite_stream"
        }
        fn describe(&self) -> LocalizedText {
            LocalizedText::new("cmd.rewrite")
        }
        fn params(&self) -> serde_json::Value {
            serde_json::Value::Null
        }
        fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
            let c = cx.doc().page(0).unwrap().dict_get("Contents").unwrap();
            cx.touch(&c)?;
            c.stream_replace(&self.0, None, None).unwrap();
            cx.changeset()
        }
    }
    let doc = open(build_pdf(1));
    let start = snapshot(&doc);
    let mut h = History::with_store(10_000, Box::new(MemStore::default()));
    h.set_verify(true);
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    h.execute(&doc, Box::new(Rewrite(big.clone()))).unwrap();
    let mid = snapshot(&doc);
    h.execute(&doc, Box::new(RotatePages::new(vec![0], 90)))
        .unwrap();
    assert_eq!(
        h.spilled_entries(),
        1,
        "the 400 KB entry exceeds the budget"
    );
    h.undo(&doc).unwrap();
    h.undo(&doc).unwrap();
    assert_matches(&doc, &start, "stream undo");
    h.redo(&doc).unwrap();
    assert_matches(&doc, &mid, "stream redo");
    let c = doc.page(0).unwrap().dict_get("Contents").unwrap();
    assert_eq!(
        c.stream_decoded(papyrine_cos::DecodeLevel::Generalized)
            .unwrap()
            .to_vec(),
        big
    );
}

// ---- property: random command and undo/redo sequences ----------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Info(u8, Option<String>),
    Rotate(Vec<u8>, i8),
    Delete(Vec<u8>),
    Move(Vec<u8>, u8),
    Dup(Vec<u8>),
    Blank(u8, u8),
    PageBox(u8, u8, Option<[i16; 4]>),
    Composite(Vec<Op>),
    Undo,
    Redo,
}

fn leaf() -> impl Strategy<Value = Op> {
    prop_oneof![
        (
            0u8..4,
            proptest::option::of("[a-zA-Z0-9 \u{e9}\u{4e2d}()\\\\]{0,12}")
        )
            .prop_map(|(f, v)| Op::Info(f, v)),
        (prop::collection::vec(any::<u8>(), 1..4), -3i8..4).prop_map(|(p, d)| Op::Rotate(p, d)),
        prop::collection::vec(any::<u8>(), 1..3).prop_map(Op::Delete),
        (prop::collection::vec(any::<u8>(), 1..4), any::<u8>()).prop_map(|(p, t)| Op::Move(p, t)),
        prop::collection::vec(any::<u8>(), 1..3).prop_map(Op::Dup),
        (any::<u8>(), 1u8..8).prop_map(|(a, s)| Op::Blank(a, s)),
        (
            any::<u8>(),
            0u8..5,
            proptest::option::of([0i16..500, 0..500, 501..900, 501..900])
        )
            .prop_map(|(p, k, r)| Op::PageBox(p, k, r)),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        6 => leaf(),
        1 => prop::collection::vec(leaf(), 1..4).prop_map(Op::Composite),
        3 => Just(Op::Undo),
        2 => Just(Op::Redo),
    ]
}

fn to_command(op: &Op, pages: usize) -> Option<Box<dyn Command>> {
    let sel = |v: &[u8]| v.iter().map(|&b| b as usize % pages).collect::<Vec<_>>();
    Some(match op {
        Op::Info(f, v) => {
            let field = [
                InfoField::Title,
                InfoField::Author,
                InfoField::Subject,
                InfoField::Keywords,
            ][*f as usize];
            Box::new(SetInfoField::new(field, v.clone()))
        }
        Op::Rotate(p, d) => Box::new(RotatePages::new(sel(p), i32::from(*d) * 90)),
        Op::Delete(p) => {
            let mut s = sel(p);
            s.sort_unstable();
            s.dedup();
            if s.len() >= pages {
                return None;
            }
            Box::new(DeletePages::new(s))
        }
        Op::Move(p, t) => {
            let mut s = sel(p);
            s.sort_unstable();
            s.dedup();
            let rest = pages - s.len();
            Box::new(MovePages::new(s, *t as usize % (rest + 1)))
        }
        Op::Dup(p) => Box::new(DuplicatePages::new(sel(p))),
        Op::Blank(a, size) => Box::new(InsertBlankPage::new(
            *a as usize % (pages + 1),
            100.0 * f64::from(*size) + 0.5,
            200.0,
        )),
        Op::PageBox(p, k, r) => {
            let kind = [
                BoxKind::MediaBox,
                BoxKind::CropBox,
                BoxKind::BleedBox,
                BoxKind::TrimBox,
                BoxKind::ArtBox,
            ][*k as usize];
            let rect = r.map(|r| r.map(f64::from));
            // The media box cannot be removed; fall back to setting it.
            let rect = if rect.is_none() && kind == BoxKind::MediaBox {
                Some([0.0, 0.0, 600.0, 800.0])
            } else {
                rect
            };
            Box::new(SetPageBox::new(*p as usize % pages, kind, rect))
        }
        Op::Composite(ops) => {
            let kids: Vec<_> = ops.iter().filter_map(|o| to_command(o, pages)).collect();
            if kids.is_empty() {
                return None;
            }
            Box::new(CompositeCommand::new(kids))
        }
        Op::Undo | Op::Redo => return None,
    })
}

struct State {
    snaps: Vec<Snapshot>,
    labels: Vec<Vec<String>>,
    depth: usize,
}

fn run_sequence(doc: &Document, ops: &[Op], h: &mut History) {
    let mut st = State {
        snaps: vec![snapshot(doc)],
        labels: vec![labels(doc)],
        depth: 0,
    };
    let initial = st.snaps[0].clone();
    for o in ops {
        match o {
            Op::Undo => {
                let did = h.undo(doc).unwrap();
                assert_eq!(did.is_some(), st.depth > 0);
                if did.is_some() {
                    st.depth -= 1;
                }
            }
            Op::Redo => {
                let did = h.redo(doc).unwrap();
                assert_eq!(did.is_some(), st.depth + 1 < st.snaps.len());
                if did.is_some() {
                    st.depth += 1;
                }
            }
            _ => {
                let pages = doc.page_count().unwrap();
                let Some(cmd) = to_command(o, pages) else {
                    continue;
                };
                match h.execute(doc, cmd) {
                    Ok(_) => {
                        st.snaps.truncate(st.depth + 1);
                        st.labels.truncate(st.depth + 1);
                        st.snaps.push(snapshot(doc));
                        st.labels.push(labels(doc));
                        st.depth += 1;
                    }
                    // Rejected (bad parameters for the current document): must be a no-op.
                    Err(Error::InvalidParams(_)) => {}
                    Err(e) => panic!("unexpected error for {o:?}: {e}"),
                }
            }
        }
        assert_matches(
            doc,
            &st.snaps[st.depth],
            &format!("depth {} after {o:?}", st.depth),
        );
        assert_eq!(labels(doc), st.labels[st.depth], "page order after {o:?}");
        assert_eq!(h.can_undo(), st.depth > 0);
        assert_eq!(h.can_redo(), st.depth + 1 < st.snaps.len());
    }
    // Unwind completely: exactly the original objects, then redo all the way and compare to the
    // state that was live before.
    let top = st.snaps.len() - 1;
    let end_depth = st.depth;
    while h.can_redo() {
        h.redo(doc).unwrap();
    }
    assert_matches(doc, &st.snaps[top], "full redo");
    assert_eq!(labels(doc), st.labels[top]);
    let _ = end_depth;
    while h.can_undo() {
        h.undo(doc).unwrap();
    }
    assert_matches(doc, &initial, "full undo restores the original objects");
    assert_eq!(labels(doc), st.labels[0]);
    // And the state at the top still writes out and reopens to the same page order.
    while h.can_redo() {
        h.redo(doc).unwrap();
    }
    assert_eq!(labels(&roundtrip(doc)), st.labels[top]);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, max_shrink_iters: 200, ..ProptestConfig::default() })]

    #[test]
    fn random_sequences_restore_exact_state(
        pages in 1usize..6,
        nested in any::<bool>(),
        spill in any::<bool>(),
        ops in prop::collection::vec(op(), 1..28),
    ) {
        let doc = if nested { open(build_nested_pdf()) } else { open(build_pdf(pages)) };
        let mut h = if spill {
            History::with_store(1, Box::new(MemStore::default()))
        } else {
            History::new()
        };
        h.set_verify(true);
        run_sequence(&doc, &ops, &mut h);
    }
}
