//! Crash recovery at the broker level: kill the "host" (drop it without an orderly
//! shutdown), start a new one on the same data directory and recover.

mod common;

use common::*;
use papyrine_host::api::{HostEvent, SaveOptions};
use papyrine_journal::{BaseInfo, Journal, RealFs, RecoveryRoot, SystemClock};
use std::sync::Arc;

fn rotations(i: &papyrine_host::api::DocumentInfo) -> Vec<u16> {
    i.pages.iter().map(|p| p.rotation).collect()
}

#[test]
fn host_killed_after_edits_restore_save_and_verify() {
    let h = harness();
    let p = h.pdf("r.pdf", 4);
    let original = std::fs::read(&p).unwrap();
    let info = h.open(&p);
    h.broker.execute(&info.doc_id, &rotate(&[0], 90)).unwrap();
    h.broker.execute(&info.doc_id, &rotate(&[2], 180)).unwrap();
    h.broker.undo(&info.doc_id).unwrap();
    h.broker.execute(&info.doc_id, &rotate(&[3], 270)).unwrap();
    // kill -9 of the host: children die with it, the journal stays.
    let h = h.relaunch();
    let entries = h.broker.scan_recovery();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].mode, "restore");
    assert_eq!(entries[0].edits, 4);
    let ui = h.broker.recovery_entries();
    assert_eq!(
        (ui[0].state.as_str(), ui[0].name.as_str()),
        ("restorable", "r.pdf")
    );
    assert!(ui[0].unfinished.is_none());
    assert!(
        h.events()
            .iter()
            .any(|e| matches!(e, HostEvent::RecoveryAvailable { .. }))
    );

    let doc = h
        .broker
        .recover(&entries[0].id, "restore")
        .unwrap()
        .unwrap();
    assert_eq!(
        rotations(&doc),
        vec![90, 0, 0, 270],
        "undo was replayed too"
    );
    assert!(doc.dirty && doc.can_undo);
    assert_eq!(
        doc.path.as_deref(),
        Some(std::fs::canonicalize(&p).unwrap().to_str().unwrap())
    );
    let t = h
        .broker
        .get_tile_blocking(&doc.doc_id, 1, 1.0, 0, 0)
        .unwrap();
    assert!(has_ink(&t));
    // Editing continues on the recovered document, and the journal keeps working.
    let r = h.broker.execute(&doc.doc_id, &rotate(&[1], 90)).unwrap();
    assert_eq!(rotations(&r.info), vec![90, 90, 0, 270]);
    let saved = saved_info(h.broker.save(&doc.doc_id, &SaveOptions::default()).unwrap());
    assert!(!saved.dirty);
    let bytes = std::fs::read(&p).unwrap();
    assert_eq!(&bytes[..original.len()], &original[..]);
    qpdf_check(&p);
    let again = h.open(&p);
    assert_eq!(rotations(&again), vec![90, 90, 0, 270]);
    // The recovery entry is consumed; a further launch finds nothing to restore.
    assert!(h.broker.recovery_entries().is_empty());
    h.broker.shutdown();
    let h2 = h.relaunch();
    assert!(h2.broker.scan_recovery().is_empty());
}

#[test]
fn recovery_is_refused_when_the_original_changed() {
    let h = harness();
    let p = h.pdf("c.pdf", 3);
    let info = h.open(&p);
    h.broker.execute(&info.doc_id, &rotate(&[0], 90)).unwrap();
    let h = h.relaunch();
    std::fs::write(&p, build_pdf(5)).unwrap();
    let entries = h.broker.scan_recovery();
    assert_eq!(entries[0].mode, "refused");
    let why = entries[0].explanation.clone().unwrap();
    assert!(why.contains("original file changed"), "{why}");
    assert_eq!(h.broker.recovery_entries()[0].state, "original-changed");
    let err = h.broker.recover(&entries[0].id, "restore").unwrap_err();
    assert!(err.message.contains("original file changed"), "{err}");
    let err = h.broker.recover(&entries[0].id, "open-copy").unwrap_err();
    assert!(err.message.contains("original file changed"), "{err}");
    // Discard removes it.
    assert!(
        h.broker
            .recover(&entries[0].id, "discard")
            .unwrap()
            .is_none()
    );
    assert!(h.broker.recovery_entries().is_empty());
    assert!(h.relaunch().broker.scan_recovery().is_empty());
}

#[test]
fn checkpoint_lets_a_changed_original_open_as_a_recovered_copy() {
    let h = harness_with(Opts::default());
    let h = h.relaunch_with(Opts::default(), |c| {
        c.journal.checkpoint_records = 3;
    });
    let p = h.pdf("k.pdf", 4);
    let info = h.open(&p);
    let id = info.doc_id;
    for (page, deg) in [(0, 90), (1, 180), (2, 270), (3, 90)] {
        h.broker.execute(&id, &rotate(&[page], deg)).unwrap();
    }
    // The checkpoint is written in the background after the commit that crossed the limit.
    let dir = h.dir.path().join("data/recovery");
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let has_checkpoint = || {
        std::fs::read_dir(&dir)
            .ok()
            .and_then(|mut d| d.next())
            .and_then(|e| e.ok())
            .map(|e| e.path().join("blobs"))
            .and_then(|b| std::fs::read_dir(b).ok())
            .is_some_and(|mut b| b.next().is_some())
    };
    while !has_checkpoint() {
        assert!(std::time::Instant::now() < end, "no checkpoint was written");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    h.broker.execute(&id, &rotate(&[0], 180)).unwrap();
    let h = h.relaunch();
    std::fs::write(&p, build_pdf(2)).unwrap();
    let entries = h.broker.scan_recovery();
    assert_eq!(entries[0].mode, "recovered-copy", "{:?}", entries[0]);
    assert_eq!(h.broker.recovery_entries()[0].state, "original-changed");
    let doc = h
        .broker
        .recover(&entries[0].id, "open-copy")
        .unwrap()
        .unwrap();
    assert_eq!(doc.path, None, "a recovered copy has no file yet");
    assert!(doc.name.contains("recovered"));
    assert_eq!(
        rotations(&doc),
        vec![270, 180, 270, 90],
        "checkpoint + later commits"
    );
    let target = h.dir.path().join("recovered-copy.pdf");
    let s = saved_info(
        h.broker
            .save(
                &doc.doc_id,
                &SaveOptions {
                    path: Some(target.to_string_lossy().into()),
                    ..Default::default()
                },
            )
            .unwrap(),
    );
    assert!(!s.dirty);
    qpdf_check(&target);
    // The user's changed original is untouched.
    assert_eq!(std::fs::read(&p).unwrap(), build_pdf(2));
}

#[test]
fn unfinished_action_is_offered_as_redo_or_skip() {
    let h = harness();
    let p = h.pdf("u.pdf", 3);
    // A previous run died between Intent and Commit of "rotate pages".
    let root = RecoveryRoot::new(Arc::new(RealFs), h.dir.path().join("data/recovery"));
    let base = BaseInfo::from_file(&p).unwrap();
    let j = Journal::create(
        &root,
        Arc::new(RealFs),
        "deadbeef00000001",
        &base,
        Default::default(),
        Arc::new(SystemClock),
    )
    .unwrap();
    j.begin(
        "rotate_pages",
        serde_json::json!({"pages": [1], "delta": 90}),
        vec![],
    )
    .unwrap();
    drop(j);
    let h = h.relaunch();
    let entries = h.broker.scan_recovery();
    assert_eq!(entries.len(), 1);
    let u = entries[0].unfinished.as_ref().expect("unfinished");
    assert_eq!(u.label, "Rotate pages");
    let ui = h.broker.recovery_entries();
    assert_eq!(ui[0].unfinished.as_ref().unwrap().title, "Rotate pages");

    let doc = h
        .broker
        .recover(&entries[0].id, "restore")
        .unwrap()
        .unwrap();
    assert_eq!(
        rotations(&doc),
        vec![0, 0, 0],
        "an unfinished action is never re-run silently"
    );
    // Editing is blocked until the person decides.
    let blocked = h
        .broker
        .execute(&doc.doc_id, &rotate(&[0], 90))
        .unwrap_err();
    assert!(blocked.message.contains("Redo or Skip"), "{blocked}");
    h.broker
        .resolve_unfinished_for(&entries[0].id, true)
        .unwrap();
    let after = h.broker.get_page_info(&doc.doc_id, 1).unwrap();
    assert_eq!(
        after.rotation, 90,
        "Redo ran the command from its stored params"
    );
    let ok = h.broker.execute(&doc.doc_id, &rotate(&[0], 90)).unwrap();
    assert_eq!(rotations(&ok.info), vec![90, 90, 0]);
}

#[test]
fn skipping_an_unfinished_action_unblocks_editing() {
    let h = harness();
    let p = h.pdf("s.pdf", 3);
    let root = RecoveryRoot::new(Arc::new(RealFs), h.dir.path().join("data/recovery"));
    let base = BaseInfo::from_file(&p).unwrap();
    let j = Journal::create(
        &root,
        Arc::new(RealFs),
        "deadbeef00000002",
        &base,
        Default::default(),
        Arc::new(SystemClock),
    )
    .unwrap();
    j.begin("delete_pages", serde_json::json!({"pages": [0]}), vec![])
        .unwrap();
    drop(j);
    let h = h.relaunch();
    let entries = h.broker.scan_recovery();
    let doc = h
        .broker
        .recover(&entries[0].id, "restore")
        .unwrap()
        .unwrap();
    h.broker
        .resolve_unfinished_for(&entries[0].id, false)
        .unwrap();
    let info = h
        .broker
        .execute(&doc.doc_id, &rotate(&[0], 90))
        .unwrap()
        .info;
    assert_eq!(info.page_count, 3, "the skipped delete never ran");
}
