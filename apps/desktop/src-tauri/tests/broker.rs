//! Broker integration tests: real engine and renderer children, no window.

mod common;

use common::*;
use papyrine_host::api::{EngineCommand, HostEvent, MetadataFields, SaveOptions, SearchOptions};

#[test]
fn open_renders_and_caches_tiles() {
    let h = harness();
    let p = h.pdf("a.pdf", 5);
    let info = h.open(&p);
    assert_eq!(info.page_count, 5);
    assert_eq!(
        (
            info.pages[0].width,
            info.pages[0].height,
            info.pages[0].rotation
        ),
        (612.0, 792.0, 0)
    );
    assert_eq!(info.meta.title, "Host test document");
    assert_eq!(info.meta.author, "Tester");
    assert_eq!(info.meta.pdf_version, "1.4");
    assert!(!info.dirty && !info.can_undo && !info.repaired);

    let t = h
        .broker
        .get_tile_blocking(&info.doc_id, 0, 1.0, 0, 0)
        .unwrap();
    assert_eq!((t.width, t.height), (512, 512));
    assert!(has_ink(&t), "page 1 shows its text");
    let again = h
        .broker
        .get_tile_blocking(&info.doc_id, 0, 1.0, 0, 0)
        .unwrap();
    assert!(
        std::sync::Arc::ptr_eq(&t, &again),
        "second request is an L1 hit"
    );
    // Edge tile is clipped to the page (612 x 792 at 1.0 -> 100 x 280 in tile (1,1)).
    let edge = h
        .broker
        .get_tile_blocking(&info.doc_id, 0, 1.0, 1, 1)
        .unwrap();
    assert_eq!((edge.width, edge.height), (100, 280));
    assert!(
        h.broker
            .get_tile_blocking(&info.doc_id, 9, 1.0, 0, 0)
            .is_err()
    );
    h.broker.close_document(&info.doc_id).unwrap();
    assert!(
        h.broker
            .get_tile_blocking(&info.doc_id, 0, 1.0, 0, 0)
            .is_err()
    );
}

#[test]
fn edit_undo_redo_save_and_reopen_round_trip() {
    let h = harness();
    let p = h.pdf("b.pdf", 4);
    let original = std::fs::read(&p).unwrap();
    let info = h.open(&p);
    let id = info.doc_id.clone();

    let before = h.broker.get_tile_blocking(&id, 1, 1.0, 0, 0).unwrap();
    let r = h.broker.execute(&id, &rotate(&[1], 90)).unwrap();
    assert_eq!(r.info.pages[1].rotation, 90);
    assert_eq!(
        (r.info.pages[1].width, r.info.pages[1].height),
        (792.0, 612.0)
    );
    assert!(r.info.dirty && r.info.can_undo && !r.info.can_redo);
    assert_eq!(r.invalidated_pages, vec![1]);
    assert!(
        h.events()
            .iter()
            .any(|e| matches!(e, HostEvent::DocumentChanged { .. }))
    );
    let after = h.broker.get_tile_blocking(&id, 1, 1.0, 0, 0).unwrap();
    assert_ne!(
        before.rgba, after.rgba,
        "the rotated page renders differently"
    );
    assert_eq!(after.width.max(after.height), 512);

    let u = h.broker.undo(&id).unwrap();
    assert_eq!(u.info.pages[1].rotation, 0);
    assert!(!u.info.can_undo && u.info.can_redo);
    let again = h.broker.get_tile_blocking(&id, 1, 1.0, 0, 0).unwrap();
    assert_eq!(before.rgba, again.rgba, "undo is exact");
    let rd = h.broker.redo(&id).unwrap();
    assert_eq!(rd.info.pages[1].rotation, 90);

    let saved = saved_info(h.broker.save(&id, &SaveOptions::default()).unwrap());
    assert!(
        !saved.dirty && saved.can_undo,
        "history survives an incremental save"
    );
    let bytes = std::fs::read(&p).unwrap();
    assert!(bytes.len() > original.len());
    assert_eq!(
        &bytes[..original.len()],
        &original[..],
        "original bytes are an exact prefix"
    );
    qpdf_check(&p);

    // Edit again after saving, then reopen from disk in a second document.
    h.broker.execute(&id, &rotate(&[0], 180)).unwrap();
    h.broker.save(&id, &SaveOptions::default()).unwrap();
    let second = h.open(&p);
    assert_eq!(second.pages[1].rotation, 90);
    assert_eq!(second.pages[0].rotation, 180);
    let t = h
        .broker
        .get_tile_blocking(&second.doc_id, 2, 1.0, 0, 0)
        .unwrap();
    assert!(has_ink(&t));
}

#[test]
fn metadata_delete_and_move_pages() {
    let h = harness();
    let p = h.pdf("c.pdf", 6);
    let info = h.open(&p);
    let id = info.doc_id;
    let r = h
        .broker
        .execute(
            &id,
            &EngineCommand::SetMetadata {
                fields: MetadataFields {
                    title: Some("New title".into()),
                    author: Some("Someone".into()),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    assert_eq!(r.info.meta.title, "New title");
    assert_eq!(r.info.meta.author, "Someone");
    let u = h.broker.undo(&id).unwrap();
    assert_eq!(u.info.meta.title, "Host test document");
    assert_eq!(u.info.meta.author, "Tester");
    h.broker.redo(&id).unwrap();

    let d = h
        .broker
        .execute(&id, &EngineCommand::DeletePages { pages: vec![0, 1] })
        .unwrap();
    assert_eq!(d.info.page_count, 4);
    assert_eq!(
        d.invalidated_pages,
        vec![0, 1, 2, 3],
        "structure changes invalidate every page"
    );
    let m = h
        .broker
        .execute(
            &id,
            &EngineCommand::MovePages {
                pages: vec![0],
                to: 3,
            },
        )
        .unwrap();
    assert_eq!(m.info.page_count, 4);
    // Page text follows the page: the old page 3 is now first, old page 3 text "Page 3".
    let t = h.broker.get_page_text(&id, 0).unwrap();
    assert!(
        t.runs.iter().any(|r| r.text.contains("Page 4")),
        "{:?}",
        t.runs
    );
    let last = h.broker.get_page_text(&id, 3).unwrap();
    assert!(
        last.runs.iter().any(|r| r.text.contains("Page 3")),
        "{:?}",
        last.runs
    );
    h.broker.save(&id, &SaveOptions::default()).unwrap();
    qpdf_check(&p);
    let re = h.open(&p);
    assert_eq!(re.page_count, 4);
    assert_eq!(re.meta.title, "New title");
}

#[test]
fn text_layer_geometry_and_search() {
    let h = harness();
    let p = h.pdf("d.pdf", 3);
    let info = h.open(&p);
    let id = info.doc_id;
    let t = h.broker.get_page_text(&id, 1).unwrap();
    let big = t
        .runs
        .iter()
        .find(|r| r.text.contains("Page 2"))
        .expect("title run");
    // 48 pt Helvetica baseline at y=700 in a 792 pt page: the run sits near the top.
    assert!(big.y > 30.0 && big.y < 80.0, "{big:?}");
    assert!(big.x > 60.0 && big.x < 90.0, "{big:?}");
    assert!(big.height > 30.0 && big.height < 70.0, "{big:?}");
    assert!(t.runs.iter().any(|r| r.text.contains("quick brown fox")));

    let job = h
        .broker
        .search(
            &id,
            "quick brown",
            SearchOptions {
                case_sensitive: false,
                whole_word: false,
                diacritic_insensitive: true,
                include_comments: false,
                include_form_values: false,
            },
        )
        .unwrap();
    h.wait_event(
        "search finished",
        |e| matches!(e, HostEvent::JobProgress { job_id, finished: true, .. } if *job_id == job),
    );
    let hits: Vec<_> = h
        .events()
        .into_iter()
        .filter_map(|e| match e {
            HostEvent::SearchHit { job_id, hit } if job_id == job => Some(hit),
            _ => None,
        })
        .collect();
    let mut pages: Vec<usize> = hits.iter().map(|h| h.page).collect();
    pages.sort();
    assert_eq!(pages, vec![0, 1, 2], "{hits:?}");
    assert!(
        hits.iter()
            .all(|h| h.match_length == "quick brown".len() && h.snippet.contains("quick brown"))
    );
    // Highlight quads are in displayed page points: the 14 pt line sits at y ~ 792 - 640.
    let q = hits[0].quads[0];
    let ys = [q[1], q[3], q[5], q[7]];
    assert!(ys.iter().all(|y| (130.0..170.0).contains(y)), "{q:?}");
}

#[test]
fn engine_killed_between_edits_replays_the_journal() {
    let h = harness();
    let p = h.pdf("e.pdf", 4);
    let info = h.open(&p);
    let id = info.doc_id;
    h.broker.execute(&id, &rotate(&[0], 90)).unwrap();
    h.broker.execute(&id, &rotate(&[2], 270)).unwrap();
    let pid1 = h.broker.stats()["enginePid"].as_u64().unwrap();

    h.broker.kill_engine();
    std::thread::sleep(std::time::Duration::from_millis(200));
    // Next edit: restart + replay + apply.
    let r = h.broker.execute(&id, &rotate(&[3], 180)).unwrap();
    let st = h.broker.stats();
    assert_ne!(st["enginePid"].as_u64().unwrap(), pid1);
    assert_eq!(st["engineRestarts"], 1);
    assert_eq!(
        r.info.pages.iter().map(|p| p.rotation).collect::<Vec<_>>(),
        vec![90, 0, 270, 180],
        "all three edits survived"
    );
    assert!(r.info.can_undo);
    assert!(h.events().iter().any(|e| matches!(e, HostEvent::Notice { code, message, .. } if code == "engine-restarted" && message == "Engine restarted; no changes lost.")));
    let saved = saved_info(h.broker.save(&id, &SaveOptions::default()).unwrap());
    assert!(!saved.dirty);
    qpdf_check(&p);
    let re = h.open(&p);
    assert_eq!(
        re.pages.iter().map(|p| p.rotation).collect::<Vec<_>>(),
        vec![90, 0, 270, 180]
    );
}

#[test]
fn renderer_crash_reloads_the_snapshot() {
    let h = harness();
    let p = h.pdf("f.pdf", 3);
    let info = h.open(&p);
    let id = info.doc_id;
    h.broker.execute(&id, &rotate(&[1], 90)).unwrap();
    let t0 = h.broker.get_tile_blocking(&id, 1, 1.0, 0, 0).unwrap();
    h.broker.kill_renderer();
    std::thread::sleep(std::time::Duration::from_millis(200));
    // Not cached: a different tile forces a render in the new renderer, which must show the edit.
    let mut ink = false;
    for ty in 0..3 {
        for tx in 0..4 {
            if let Ok(t) = h.broker.get_tile_blocking(&id, 1, 2.0, tx, ty) {
                ink |= has_ink(&t);
            }
        }
    }
    assert_eq!(h.broker.stats()["rendererRestarts"], 1);
    assert!(ink, "the restarted renderer draws the page");
    let info2 = h.broker.get_page_info(&id, 1).unwrap();
    assert_eq!(info2.rotation, 90);
    let _ = t0;
}

#[test]
fn command_that_crashes_the_engine_twice_is_quarantined() {
    let h = harness_with(Opts {
        crash_on: Some("\"to\":2".into()),
        ..Opts::default()
    });
    let p = h.pdf("g.pdf", 5);
    let info = h.open(&p);
    let id = info.doc_id;
    h.broker.execute(&id, &rotate(&[0], 90)).unwrap();
    let bad = EngineCommand::MovePages {
        pages: vec![0],
        to: 2,
    };
    let e1 = h.broker.execute(&id, &bad).unwrap_err();
    assert!(
        e1.message.contains("didn't finish")
            && e1.message.contains("Engine restarted; no changes lost"),
        "{e1}"
    );
    // Nothing was lost and nothing half-applied.
    let info = h.broker.get_page_info(&id, 0).unwrap();
    assert_eq!(info.rotation, 90);
    let e2 = h.broker.execute(&id, &bad).unwrap_err();
    assert!(e2.message.contains("crashed twice"), "{e2}");
    let e3 = h.broker.execute(&id, &bad).unwrap_err();
    assert!(e3.message.contains("disabled for this document"), "{e3}");
    // Other work continues.
    let ok = h.broker.execute(&id, &rotate(&[1], 90)).unwrap();
    assert_eq!(
        ok.info.pages.iter().map(|p| p.rotation).collect::<Vec<_>>(),
        vec![90, 90, 0, 0, 0]
    );
    assert_eq!(h.broker.stats()["engineRestarts"], 2);
}

#[test]
fn save_as_open_bytes_and_recent_files() {
    let h = harness();
    let bytes = build_pdf(2);
    let info = h.broker.open_bytes("dropped.pdf", &bytes, None).unwrap();
    assert_eq!(info.path, None);
    assert_eq!(info.name, "dropped.pdf");
    let id = info.doc_id;
    h.broker.execute(&id, &rotate(&[0], 90)).unwrap();
    let err = h.broker.save(&id, &SaveOptions::default()).unwrap_err();
    assert!(err.message.contains("Save As"), "{err}");
    let target = h.dir.path().join("saved-as.pdf");
    let s = saved_info(
        h.broker
            .save(
                &id,
                &SaveOptions {
                    path: Some(target.to_string_lossy().into()),
                    ..Default::default()
                },
            )
            .unwrap(),
    );
    assert_eq!(s.name, "saved-as.pdf");
    assert_eq!(
        s.path.as_deref(),
        Some(std::fs::canonicalize(&target).unwrap().to_str().unwrap())
    );
    assert!(!s.dirty);
    qpdf_check(&target);
    assert_eq!(h.broker.recent_files()[0].name, "saved-as.pdf");
    let re = h.open(&target);
    assert_eq!(re.pages[0].rotation, 90);
    // The original dropped file is unchanged and its temp copy is gone.
    assert!(
        std::fs::read_dir(h.dir.path().join("data/untitled"))
            .map_or(true, |mut d| d.next().is_none())
    );
}

#[test]
fn external_change_is_reported_once_and_self_saves_are_ignored() {
    let h = harness();
    let p = h.pdf("x.pdf", 2);
    let info = h.open(&p);
    let id = info.doc_id;
    h.broker.after_first_paint();
    std::thread::sleep(std::time::Duration::from_millis(500));
    // Our own save is not an external change.
    h.broker.execute(&id, &rotate(&[0], 90)).unwrap();
    h.broker.save(&id, &SaveOptions::default()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(800));
    h.broker.check_external_all();
    assert!(
        !h.events()
            .iter()
            .any(|e| matches!(e, HostEvent::FileChangedOnDisk { .. }))
    );
    // Someone else rewrites it.
    std::fs::write(&p, build_pdf(3)).unwrap();
    h.wait_event(
        "file-changed-on-disk",
        |e| matches!(e, HostEvent::FileChangedOnDisk { doc_id } if *doc_id == id),
    );
    h.broker.check_external_all();
    let n = h
        .events()
        .iter()
        .filter(|e| matches!(e, HostEvent::FileChangedOnDisk { .. }))
        .count();
    assert_eq!(n, 1, "reported once per change");
    h.broker.acknowledge_external(&id).unwrap();
}

#[test]
fn corpus_typical_20p_opens_and_renders_fast() {
    let Some(p) = corpus("typical-20p.pdf") else {
        eprintln!("skipped: corpus/cache/generated/typical-20p.pdf not generated");
        return;
    };
    let h = harness();
    // Warm the children (their start-up is not part of the open path in the app).
    h.broker.warm_up();
    std::thread::sleep(std::time::Duration::from_millis(800));
    let t0 = std::time::Instant::now();
    let info = h.open(&p);
    let t = h
        .broker
        .get_tile_blocking(&info.doc_id, 0, 1.0, 0, 0)
        .unwrap();
    let ms = t0.elapsed().as_millis();
    eprintln!("open + first tile: {ms} ms");
    assert_eq!(info.page_count, 20);
    assert!(has_ink(&t));
    assert!(ms < 500, "open + first tile took {ms} ms (budget 500)");
}
