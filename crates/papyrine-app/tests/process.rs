//! Process-level tests: the engine runs as a real sandboxed child of the `papyrine-app` binary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use papyrine_app::host::{EngineHost, HostConfig, HostError, SaveOutcome};
use papyrine_app::spawn::SpawnConfig;
use papyrine_engine::proto::{Query, QueryResult, SaveKindDto, SaveMode, SnapshotAction};
use papyrine_ipc::DocId;
use papyrine_journal::RealFs;
use serde_json::{Value, json};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_papyrine-app"))
}

fn spawn_cfg() -> SpawnConfig {
    SpawnConfig {
        exe: Some(bin()),
        ..SpawnConfig::default()
    }
}

/// A valid classic-xref PDF with `pages` pages.
fn build_pdf(pages: usize) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: String| {
        offsets.push(out.len());
        let n = offsets.len();
        out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    };
    obj(&mut out, "<< /Type /Catalog /Pages 2 0 R >>".into());
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    obj(
        &mut out,
        format!(
            "<< /Type /Pages /Count {pages} /Kids [{}] >>",
            kids.join(" ")
        ),
    );
    obj(
        &mut out,
        "<< /Producer (app-tests) /Title (Process test) >>".into(),
    );
    for i in 0..pages {
        let content = format!("BT /F1 12 Tf 72 720 Td (Page {}) Tj ET\n", i + 1);
        obj(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R \
                 /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>",
                5 + 2 * i
            ),
        );
        obj(
            &mut out,
            format!(
                "<< /Length {} >>\nstream\n{}endstream",
                content.len(),
                content
            ),
        );
    }
    let xref_pos = out.len();
    let n = offsets.len() + 1;
    out.extend_from_slice(format!("xref\n0 {n}\n0000000000 65535 f \n").as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {n} /Root 1 0 R /Info 3 0 R /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>] >>\nstartxref\n{xref_pos}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn doc_path(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }
    fn host(&self) -> EngineHost {
        let mut cfg = HostConfig::new(spawn_cfg(), self.dir.path().join("recovery"));
        cfg.fs = Arc::new(RealFs);
        EngineHost::new(cfg).expect("start engine")
    }
}

fn pages(host: &mut EngineHost, doc: DocId) -> Vec<(f64, f64, u16)> {
    let QueryResult::Pages(p) = host
        .query(
            doc,
            Query::Pages {
                from: 0,
                count: 100_000,
            },
        )
        .unwrap()
    else {
        unreachable!()
    };
    p.iter()
        .map(|p| (p.width_pt, p.height_pt, p.rotation))
        .collect()
}

/// A mixed edit script: organise commands, undo and redo.
fn script(i: usize, n: usize) -> (&'static str, Value) {
    match i % 10 {
        0 | 5 => (
            "insert_blank_page",
            json!({"at": i % (n + 1), "width": 300, "height": 400}),
        ),
        1 | 7 => ("rotate_pages", json!({"pages": [i % n], "delta": 90})),
        2 if n > 3 => ("delete_pages", json!({"pages": [i % n]})),
        4 => ("move_pages", json!({"pages": [0], "to": n - 1})),
        8 => ("duplicate_pages", json!({"pages": [i % n]})),
        3 => ("undo", Value::Null),
        6 => ("redo", Value::Null),
        _ => (
            "set_info_field",
            json!({"field": "Title", "value": format!("Title {i}")}),
        ),
    }
}

fn run_step(host: &mut EngineHost, doc: DocId, i: usize) {
    let n = pages(host, doc).len();
    let (name, params) = script(i, n);
    let r = match name {
        "undo" => host.undo(doc),
        "redo" => host.redo(doc),
        _ => host.execute(doc, name, params, vec![]),
    };
    match r {
        Ok(_) => {}
        // Nothing to undo/redo at this point of the script is fine.
        Err(HostError::Engine(e)) if name == "undo" || name == "redo" => {
            assert!(e.message.contains("nothing to"), "{e}");
        }
        Err(e) => panic!("step {i} ({name}) failed: {e}"),
    }
}

#[test]
fn sandboxed_engine_runs_fifty_commands_saves_and_reopens() {
    let env = Env::new();
    let path = env.doc_path("doc.pdf", &build_pdf(8));
    let mut host = env.host();
    let (doc, summary) = host.open(&path, None).unwrap();
    assert_eq!(summary.page_count, 8);
    for i in 0..50 {
        run_step(&mut host, doc, i);
    }
    let live_pages = pages(&mut host, doc);
    let live = host.digest(doc, true).unwrap();
    let SaveOutcome::Saved(saved) = host
        .save(
            doc,
            SaveMode::Policy {
                break_signatures: false,
            },
            None,
        )
        .unwrap()
    else {
        panic!("decision")
    };
    assert_eq!(saved.kind, SaveKindDto::Incremental);
    assert!(!saved.unchanged);
    assert!(matches!(saved.snapshot.action, SnapshotAction::Unchanged));
    if Command::new("qpdf").arg("--version").output().is_ok() {
        let out = Command::new("qpdf")
            .arg("--check")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    host.close(doc, true).unwrap();

    // Reopen the saved file: same pages, every object it holds equals the live one.
    let (doc2, s2) = host.open(&path, None).unwrap();
    assert!(!s2.repaired, "{:?}", s2.repair_log);
    assert_eq!(pages(&mut host, doc2), live_pages);
    let reopened = host.digest(doc2, true).unwrap();
    let live_map: std::collections::HashMap<_, _> = live
        .per_object
        .iter()
        .map(|(n, g, h)| ((*n, *g), *h))
        .collect();
    for (n, g, h) in &reopened.per_object {
        if *n == 0 {
            continue;
        }
        assert_eq!(
            live_map.get(&(*n, *g)),
            Some(h),
            "object {n} {g} differs after reopen"
        );
    }
    host.shutdown();
}

use std::process::Command;

#[test]
fn engine_child_is_sandboxed() {
    let env = Env::new();
    let path = env.doc_path("doc.pdf", &build_pdf(2));
    let mut host = env.host();
    let (doc, _) = host.open(&path, None).unwrap();
    let err = host
        .execute(doc, "test_probe", Value::Null, vec![])
        .unwrap_err();
    let HostError::Engine(e) = err else {
        panic!("{err}")
    };
    eprintln!("{}", e.message);
    for want in [
        "read_home=denied",
        "write_outside_temp=denied",
        "write_temp=allowed",
        "socket=denied",
        "spawn=denied",
    ] {
        assert!(
            e.message.contains(want),
            "expected {want} in: {}",
            e.message
        );
    }
    // The failed probe left no trace in the journal or the document.
    let rec = papyrine_journal::recover(&RealFs, host.journal(doc).unwrap().dir()).unwrap();
    assert!(rec.commits.is_empty() && rec.unfinished.is_empty());
    host.shutdown();
}

#[cfg(unix)]
fn kill9(pid: u32) {
    let st = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    assert!(st.success());
}

#[cfg(unix)]
#[test]
fn kill9_when_idle_then_restart_replays_the_journal() {
    let env = Env::new();
    let path = env.doc_path("doc.pdf", &build_pdf(6));
    let mut host = env.host();
    let (doc, _) = host.open(&path, None).unwrap();
    for i in 0..30 {
        run_step(&mut host, doc, i);
    }
    let want = host.digest(doc, true).unwrap();
    let want_pages = pages(&mut host, doc);
    kill9(host.engine_pid().unwrap());
    std::thread::sleep(std::time::Duration::from_millis(100));
    // The next request notices, restarts, restores and reports.
    let err = host.digest(doc, true).unwrap_err();
    assert!(
        matches!(&err, HostError::Engine(e) if e.code == papyrine_ipc::ErrorCode::ChildCrashed),
        "{err}"
    );
    let report = host.restart_engine().unwrap();
    assert_eq!(report.docs.len(), 1);
    assert!(report.unfinished.is_empty(), "nothing was in flight");
    assert!(report.docs[0].replayed > 0);
    let got = host.digest(doc, true).unwrap();
    assert_eq!(got.objects, want.objects);
    assert_eq!(
        got.digest, want.digest,
        "restored document differs from before the crash"
    );
    assert_eq!(pages(&mut host, doc), want_pages);
    // The restored session is fully usable, history included.
    host.execute(
        doc,
        "rotate_pages",
        json!({"pages": [0], "delta": 90}),
        vec![],
    )
    .unwrap();
    host.undo(doc).unwrap();
    host.undo(doc).unwrap();
    host.shutdown();
}

#[cfg(unix)]
#[test]
fn kill9_in_the_middle_of_a_command_reports_it_and_loses_nothing() {
    let env = Env::new();
    let path = env.doc_path("doc.pdf", &build_pdf(5));
    let mut host = env.host();
    let (doc, _) = host.open(&path, None).unwrap();
    for i in 0..12 {
        run_step(&mut host, doc, i);
    }
    let want = host.digest(doc, true).unwrap();
    let pid = host.engine_pid().unwrap();
    let killer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        kill9(pid);
    });
    let err = host
        .execute(doc, "test_sleep", json!({"ms": 20_000}), vec![])
        .unwrap_err();
    killer.join().unwrap();
    let HostError::EngineRestarted(report) = err else {
        panic!("{err}")
    };
    assert_eq!(report.unfinished.len(), 1);
    let u = &report.unfinished[0];
    assert_eq!(
        (u.command.as_str(), u.crashes, u.quarantined),
        ("test_sleep", 1, false)
    );
    // Everything committed before the kill is intact; the unfinished command was not re-run.
    let got = host.digest(doc, true).unwrap();
    assert_eq!(got.digest, want.digest);
    host.shutdown();
}

#[test]
fn a_command_that_crashes_the_engine_twice_is_quarantined() {
    let env = Env::new();
    let path = env.doc_path("doc.pdf", &build_pdf(3));
    let mut host = env.host();
    let (doc, _) = host.open(&path, None).unwrap();
    host.execute(
        doc,
        "rotate_pages",
        json!({"pages": [1], "delta": 90}),
        vec![],
    )
    .unwrap();
    let want = host.digest(doc, true).unwrap();
    for attempt in 1..=2u32 {
        let err = host
            .execute(doc, "test_crash", Value::Null, vec![])
            .unwrap_err();
        let HostError::EngineRestarted(r) = err else {
            panic!("{err}")
        };
        assert_eq!(r.unfinished[0].crashes, attempt);
        assert_eq!(r.unfinished[0].quarantined, attempt >= 2);
        assert_eq!(host.digest(doc, true).unwrap().digest, want.digest);
    }
    // Third try: refused up front, the engine is not touched.
    let pid = host.engine_pid();
    let err = host
        .execute(doc, "test_crash", Value::Null, vec![])
        .unwrap_err();
    assert!(
        matches!(
            err,
            HostError::Journal(papyrine_journal::JournalError::Quarantined { .. })
        ),
        "{err}"
    );
    assert_eq!(host.engine_pid(), pid);
    // Other commands still work.
    host.execute(
        doc,
        "rotate_pages",
        json!({"pages": [2], "delta": 90}),
        vec![],
    )
    .unwrap();
    host.shutdown();
}

#[test]
fn recovery_after_the_whole_app_died_restores_and_saves() {
    let env = Env::new();
    let orig = build_pdf(6);
    let path = env.doc_path("doc.pdf", &orig);
    let (want, want_pages);
    {
        let mut host = env.host();
        let (doc, _) = host.open(&path, None).unwrap();
        for i in 0..25 {
            run_step(&mut host, doc, i);
        }
        want = host.digest(doc, true).unwrap();
        want_pages = pages(&mut host, doc);
        // The app dies without closing anything: dropping the host kills the engine; the
        // journal directory stays.
    }
    let mut host = env.host();
    assert_eq!(host.recovery_root().scan().len(), 1);
    let rec = host.recover(&path, None).unwrap();
    assert!(rec.replayed > 0 && rec.unfinished.is_empty());
    assert_eq!(host.digest(rec.doc, true).unwrap().digest, want.digest);
    assert_eq!(pages(&mut host, rec.doc), want_pages);
    // Save the restored work and verify the file.
    let SaveOutcome::Saved(s) = host
        .save(
            rec.doc,
            SaveMode::Policy {
                break_signatures: false,
            },
            None,
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(s.kind, SaveKindDto::Incremental);
    assert!(
        std::fs::read(&path).unwrap().starts_with(&orig),
        "original bytes stay a prefix"
    );
    host.close(rec.doc, true).unwrap();
    let (doc2, _) = host.open(&path, None).unwrap();
    assert_eq!(pages(&mut host, doc2), want_pages);
    host.shutdown();
}

#[test]
fn recovery_is_refused_when_the_original_changed() {
    let env = Env::new();
    let path = env.doc_path("doc.pdf", &build_pdf(3));
    {
        let mut host = env.host();
        let (doc, _) = host.open(&path, None).unwrap();
        host.execute(
            doc,
            "rotate_pages",
            json!({"pages": [0], "delta": 90}),
            vec![],
        )
        .unwrap();
    }
    std::fs::write(&path, build_pdf(4)).unwrap();
    let mut host = env.host();
    let err = host.recover(&path, None).unwrap_err();
    assert!(matches!(err, HostError::OriginalChanged), "{err}");
    host.shutdown();
}

#[cfg(unix)]
#[test]
fn restart_from_a_checkpoint() {
    let env = Env::new();
    let path = env.doc_path("doc.pdf", &build_pdf(6));
    let mut host = env.host();
    let (doc, _) = host.open(&path, None).unwrap();
    for i in 0..15 {
        run_step(&mut host, doc, i);
    }
    host.checkpoint(doc).unwrap();
    for i in 15..25 {
        run_step(&mut host, doc, i);
    }
    let want = host.digest(doc, true).unwrap();
    kill9(host.engine_pid().unwrap());
    std::thread::sleep(std::time::Duration::from_millis(100));
    let report = host.restart_engine().unwrap();
    // Only the records after the checkpoint were replayed.
    assert!(report.docs[0].replayed <= 10, "{}", report.docs[0].replayed);
    let got = host.digest(doc, true).unwrap();
    // The checkpoint is an ID-preserving full write: every live object is unchanged.
    let got_map: std::collections::HashMap<_, _> = got
        .per_object
        .iter()
        .map(|(n, g, h)| ((*n, *g), *h))
        .collect();
    for (n, g, h) in &want.per_object {
        if *n == 0 {
            continue;
        }
        assert_eq!(got_map.get(&(*n, *g)), Some(h), "object {n} {g}");
    }
    host.shutdown();
}

#[test]
fn optimized_save_resets_history_and_keeps_working() {
    let env = Env::new();
    let path = env.doc_path("doc.pdf", &build_pdf(4));
    let mut host = env.host();
    let (doc, _) = host.open(&path, None).unwrap();
    host.execute(
        doc,
        "rotate_pages",
        json!({"pages": [0], "delta": 90}),
        vec![],
    )
    .unwrap();
    let SaveOutcome::Saved(s) = host
        .save(
            doc,
            SaveMode::Optimized {
                break_signatures: false,
            },
            None,
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(s.kind, SaveKindDto::Optimized);
    assert!(s.history_reset && s.notice.is_some());
    assert!(matches!(s.snapshot.action, SnapshotAction::NewBase(None)));
    assert_eq!(pages(&mut host, doc)[0].2, 90);
    host.execute(
        doc,
        "rotate_pages",
        json!({"pages": [1], "delta": 90}),
        vec![],
    )
    .unwrap();
    // Crash after an optimized save: the restart reopens the saved file.
    #[cfg(unix)]
    {
        let want = host.digest(doc, true).unwrap();
        kill9(host.engine_pid().unwrap());
        std::thread::sleep(std::time::Duration::from_millis(100));
        host.restart_engine().unwrap();
        assert_eq!(host.digest(doc, true).unwrap().digest, want.digest);
    }
    host.shutdown();
}

#[test]
fn renderer_role_starts_and_stops() {
    use papyrine_ipc::{RenderRequest, RenderResponse};
    let child = papyrine_app::spawn::spawn_renderer(&spawn_cfg()).expect("renderer child");
    let r = child
        .client
        .call_blocking(RenderRequest::Ping { nonce: 9 })
        .unwrap();
    assert!(matches!(r, RenderResponse::Pong { nonce: 9 }));
    let code = child.shutdown(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(code, Some(0));
}

fn cli(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin()).args(args).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn cli_info() {
    let env = Env::new();
    let path = env.doc_path("doc.pdf", &build_pdf(3));
    let p = path_str(&path);
    let (code, out, _) = cli(&["info", "--json", &p]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["pages"], 3);
    assert_eq!(v["title"], "Process test");
    assert_eq!(v["encrypted"], false);
    assert_eq!(v["pdf_version"], "1.4");
    assert_eq!(v["first_page"]["width_pt"], 612.0);
    let (code, out, _) = cli(&["info", &p]);
    assert_eq!(code, 0);
    assert!(
        out.contains("pages") && out.contains("Process test"),
        "{out}"
    );
    let (code, _, err) = cli(&["info", &path_str(&env.dir.path().join("missing.pdf"))]);
    assert_eq!(code, 1, "{err}");
    let (code, _, _) = cli(&["info"]);
    assert_eq!(code, 2);
    let (code, _, _) = cli(&["bogus"]);
    assert_eq!(code, 2);
    let (code, out, _) = cli(&["--version"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("papyrine "));
}

#[test]
fn cli_info_on_an_encrypted_file() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/cache/generated/enc-aes-256-r6-user.pdf");
    if !root.exists() {
        return;
    }
    let p = path_str(&root);
    let (code, _, err) = cli(&["info", &p]);
    assert_eq!(code, 3, "{err}");
    let (code, out, _) = cli(&["info", "--json", "--password", "user", &p]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["encrypted"], true);
    assert!(v["encryption"].as_str().unwrap().contains("AES"));
}
