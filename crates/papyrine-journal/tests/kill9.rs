//! SIGKILL crash tests (ROADMAP 1.6): kill the host, the engine, or both at random moments
//! and verify nothing is silently lost.
//!
//! PAPYRINE_KILL_ITERS controls the iteration count (default 1000).

#![cfg(unix)]

use papyrine_journal::*;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

include!("../examples/killer_payload.rs.inc");

fn killer_exe() -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // deps
    p.pop(); // debug
    p.push("examples");
    p.push("killer");
    if !p.exists() {
        // `cargo test --test kill9` does not build examples; do it ourselves.
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let ok = Command::new(cargo)
            .args(["build", "-p", "papyrine-journal", "--example", "killer"])
            .status()
            .unwrap()
            .success();
        assert!(
            ok && p.exists(),
            "could not build the killer example ({p:?})"
        );
    }
    p
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 >> 8
    }
}

#[derive(Default, Debug)]
struct Stats {
    iterations: u32,
    intents: u64,
    commits: u64,
    unfinished: u64,
    torn_tails: u32,
    engine_deaths_handled: u32,
    abandoned: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Host,
    Engine,
    EngineThenHost,
}

fn one_iteration(i: u32, exe: &PathBuf, tmp: &std::path::Path, rng: &mut Rng, st: &mut Stats) {
    let root = tmp.join(format!("r{i}"));
    let target = match i % 3 {
        0 => Target::Host,
        1 => Target::Engine,
        _ => Target::EngineThenHost,
    };
    let mut host = Command::new(exe)
        .args(["host", root.to_str().unwrap(), "doc"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    let out = host.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        for l in BufReader::new(out).lines() {
            let Ok(l) = l else { break };
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    let mut lines: Vec<String> = Vec::new();
    let mut engine_pid: Option<i32> = None;
    // Wait for the engine to appear, then a random short run.
    while engine_pid.is_none() {
        let l = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("host did not start");
        if let Some(p) = l.strip_prefix("E ") {
            engine_pid = Some(p.parse().unwrap());
        }
        lines.push(l);
    }
    std::thread::sleep(Duration::from_micros(500 + rng.next() % 40_000));
    let latest_engine =
        |lines: &[String], rx: &mpsc::Receiver<String>, lines_out: &mut Vec<String>| {
            while let Ok(l) = rx.try_recv() {
                lines_out.push(l);
            }
            lines
                .iter()
                .chain(lines_out.iter())
                .filter_map(|l| l.strip_prefix("E ").map(|p| p.parse::<i32>().unwrap()))
                .next_back()
        };
    let mut extra = Vec::new();
    let kill = |pid: i32| unsafe {
        libc::kill(pid, libc::SIGKILL);
    };
    match target {
        Target::Host => kill(host.id() as i32),
        Target::Engine | Target::EngineThenHost => {
            let pid = latest_engine(&lines, &rx, &mut extra).unwrap();
            kill(pid);
            // Let the host notice, resolve and restart the engine, and carry on a bit.
            std::thread::sleep(Duration::from_millis(5 + rng.next() % 40));
            // The host survived the engine's death for a while; now stop it so we can verify.
            // (Target::Engine and EngineThenHost differ only in how long the host ran on.)
            if target == Target::EngineThenHost {
                std::thread::sleep(Duration::from_millis(rng.next() % 20));
            }
            kill(host.id() as i32);
        }
    }
    let _ = host.wait();
    reader.join().unwrap();
    lines.extend(extra);
    lines.extend(rx.try_iter());

    // --- verify -----------------------------------------------------------------------
    let mut acked_i = BTreeSet::new();
    let mut acked_c = BTreeSet::new();
    let mut handled_x = BTreeSet::new();
    for l in &lines {
        let (k, v) = l.split_once(' ').unwrap();
        let n: u64 = v.parse().unwrap();
        match k {
            "I" => assert!(acked_i.insert(n)),
            "C" => assert!(acked_c.insert(n)),
            "X" => assert!(handled_x.insert(n)),
            _ => {}
        }
    }
    let fs: Arc<dyn Fs> = Arc::new(RealFs);
    let rroot = RecoveryRoot::new(fs.clone(), &root);
    let (j, rec) =
        Journal::open(&rroot, fs, "doc", Options::default(), Arc::new(SystemClock)).unwrap();
    assert_eq!(rec.orphan_commits, 0, "iter {i}");
    let committed: BTreeSet<u64> = rec.commits.iter().map(|c| c.seq).collect();
    let unfinished: BTreeSet<u64> = rec.unfinished.iter().map(|c| c.seq).collect();
    let abandoned: BTreeSet<u64> = rec.abandoned.iter().copied().collect();
    assert!(
        committed.is_disjoint(&unfinished)
            && committed.is_disjoint(&abandoned)
            && unfinished.is_disjoint(&abandoned),
        "iter {i}"
    );
    for s in &acked_i {
        assert!(
            committed.contains(s) || unfinished.contains(s) || abandoned.contains(s),
            "iter {i} ({target:?}): intent {s} was acknowledged but is silently lost"
        );
    }
    for s in &acked_c {
        assert!(
            committed.contains(s),
            "iter {i}: acknowledged commit {s} is missing"
        );
    }
    for s in &handled_x {
        assert!(
            abandoned.contains(s),
            "iter {i}: handled engine death {s} not recorded"
        );
    }
    // Contiguous seq numbering: nothing in the middle vanished.
    let all: BTreeSet<u64> = committed
        .iter()
        .chain(&unfinished)
        .chain(&abandoned)
        .copied()
        .collect();
    if let Some(&max) = all.iter().next_back() {
        assert_eq!(all.len() as u64, max, "iter {i}: gap in sequence numbers");
    }
    // Content is exactly what the engine produced.
    for c in &rec.commits {
        assert_eq!(c.after_images.len(), 1);
        assert_eq!(
            c.after_images[0].data,
            payload(c.seq),
            "iter {i}: after-image {} corrupted",
            c.seq
        );
        assert_eq!(c.after_images[0].id.num as u64, c.seq);
        assert!(c.params["n"].as_u64().is_some());
        for b in &c.blobs {
            assert_eq!(j.read_blob(b).unwrap().len() as u64, b.len);
        }
    }
    for u in &rec.unfinished {
        for b in &u.blobs {
            assert!(
                j.read_blob(b).is_ok(),
                "iter {i}: blob of unfinished intent missing"
            );
        }
    }
    // The journal is usable again after recovery: resolve and run one more command.
    for u in &rec.unfinished {
        j.resolve_unfinished(u.seq, Resolution::Skip).unwrap();
    }
    let t = j
        .begin("after-recovery", serde_json::json!({}), vec![])
        .unwrap();
    let seq = t.seq();
    j.commit(
        t,
        vec![AfterImage {
            id: ObjId {
                num: 1,
                generation: 0,
            },
            data: vec![1, 2, 3],
        }],
        vec![],
    )
    .unwrap();
    drop(j);
    let rec2 = recover(&RealFs, &root.join("doc")).unwrap();
    assert!(rec2.commits.iter().any(|c| c.seq == seq));
    assert!(rec2.unfinished.is_empty());
    assert_eq!(rec2.discarded_bytes, 0);

    st.iterations += 1;
    st.intents += acked_i.len() as u64;
    st.commits += committed.len() as u64;
    st.unfinished += unfinished.len() as u64;
    st.abandoned += abandoned.len() as u64;
    st.engine_deaths_handled += handled_x.len() as u32;
    if rec.discarded_bytes > 0 {
        st.torn_tails += 1;
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn kill9_host_and_engine_at_random_points() {
    let iters: u32 = std::env::var("PAPYRINE_KILL_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);
    let exe = killer_exe();
    let tmp = tempfile::tempdir().unwrap();
    let mut rng = Rng(0x1234_5678_9ABC_DEF1 ^ iters as u64);
    let mut st = Stats::default();
    for i in 0..iters {
        one_iteration(i, &exe, tmp.path(), &mut rng, &mut st);
    }
    eprintln!("kill9 stats: {st:?}");
    // Make sure the test actually exercised the interesting cases.
    if iters >= 300 {
        assert!(
            st.unfinished > 0,
            "no kill ever landed between Intent and Commit"
        );
        assert!(
            st.engine_deaths_handled > 0,
            "no engine death was ever observed by the host"
        );
    }
}
