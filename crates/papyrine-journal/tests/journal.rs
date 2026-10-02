use papyrine_journal::testing::{FaultFs, ManualClock, PowerLoss};
use papyrine_journal::*;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

fn base() -> BaseInfo {
    BaseInfo {
        original_path: "/tmp/x.pdf".into(),
        size: 10,
        mtime_ms: 1,
        blake3: "00".into(),
    }
}

fn img(n: u32, len: usize, seed: u8) -> AfterImage {
    AfterImage {
        id: ObjId {
            num: n,
            generation: 0,
        },
        data: (0..len).map(|i| (i as u8).wrapping_add(seed)).collect(),
    }
}

fn mem() -> (FaultFs, Arc<dyn Fs>, RecoveryRoot, ManualClock) {
    let f = FaultFs::new();
    let fs: Arc<dyn Fs> = Arc::new(f.clone());
    let root = RecoveryRoot::new(fs.clone(), "/rec");
    (f, fs, root, ManualClock::new(1_000_000))
}

fn new_journal(
    fs: &Arc<dyn Fs>,
    root: &RecoveryRoot,
    clock: &ManualClock,
    opts: Options,
) -> Journal {
    Journal::create(
        root,
        fs.clone(),
        "doc1",
        &base(),
        opts,
        Arc::new(clock.clone()),
    )
    .unwrap()
}

fn reopen(fs: &Arc<dyn Fs>, root: &RecoveryRoot, clock: &ManualClock) -> (Journal, Recovery) {
    Journal::open(
        root,
        fs.clone(),
        "doc1",
        Options::default(),
        Arc::new(clock.clone()),
    )
    .unwrap()
}

fn run(j: &Journal, name: &str, n: u32, len: usize) -> u64 {
    let t = j.begin(name, json!({"n": n}), vec![]).unwrap();
    let seq = t.seq();
    j.commit(
        t,
        vec![img(n, len, n as u8)],
        vec![ObjId {
            num: n + 100,
            generation: 0,
        }],
    )
    .unwrap();
    seq
}

#[test]
fn intent_commit_roundtrip_with_compression() {
    let (_f, fs, root, clock) = mem();
    let j = new_journal(&fs, &root, &clock, Options::default());
    run(&j, "rotate", 1, 100);
    run(&j, "insert", 2, 50_000); // > 4 KB: zstd
    j.close().unwrap();
    let log = fs.read(&j.dir().join("journal.log")).unwrap();
    assert!(
        log.len() < 20_000,
        "compressible 50 KB after-image should be stored compressed"
    );
    let (_j2, rec) = reopen(&fs, &root, &clock);
    assert_eq!(rec.commits.len(), 2);
    assert_eq!(rec.commits[1].after_images[0], img(2, 50_000, 2));
    assert_eq!(rec.commits[0].command, "rotate");
    assert_eq!(
        rec.commits[0].created,
        vec![ObjId {
            num: 101,
            generation: 0
        }]
    );
    assert!(rec.unfinished.is_empty());
    assert_eq!(rec.discarded_bytes, 0);
}

#[test]
fn trailing_intent_is_reported_unfinished_not_replayed() {
    let (_f, fs, root, clock) = mem();
    let j = new_journal(&fs, &root, &clock, Options::default());
    run(&j, "a", 1, 10);
    let t = j
        .begin("Rotate pages 3-5", json!({"pages": [3, 4, 5]}), vec![])
        .unwrap();
    let _ = t; // process dies here
    let (j2, rec) = reopen(&fs, &root, &clock);
    assert_eq!(rec.commits.len(), 1);
    assert_eq!(rec.unfinished.len(), 1);
    assert_eq!(
        rec.unfinished[0].message(),
        "Your last action, Rotate pages 3-5, didn't finish."
    );
    assert_eq!(rec.unfinished[0].params, json!({"pages": [3, 4, 5]}));
    // Must be resolved before anything else runs.
    assert!(matches!(
        j2.begin("b", json!({}), vec![]),
        Err(JournalError::Unresolved { .. })
    ));
    j2.resolve_unfinished(rec.unfinished[0].seq, Resolution::Skip)
        .unwrap();
    run(&j2, "b", 2, 10);
    let (_j3, rec3) = reopen(&fs, &root, &clock);
    assert!(rec3.unfinished.is_empty());
    assert_eq!(rec3.abandoned, vec![rec.unfinished[0].seq]);
    assert_eq!(rec3.commits.len(), 2);
}

#[test]
fn torn_tail_every_cut_point() {
    let (f, fs, root, clock) = mem();
    let j = new_journal(&fs, &root, &clock, Options::default());
    for i in 1..=3 {
        run(&j, "c", i, 300);
    }
    let path = j.dir().join("journal.log");
    let full = fs.read(&path).unwrap();
    let dir = j.dir().to_path_buf();
    drop(j);
    for cut in 0..full.len() {
        fs.write_atomic(&path, &full[..cut]).unwrap();
        // Tolerates a cut anywhere: valid prefix of whole records survives.
        let r = recover(fs.as_ref(), &dir);
        if cut < 8 {
            assert!(r.is_ok(), "cut {cut}");
        }
        let r = r.unwrap();
        let expect = r.commits.len();
        assert!(expect <= 3);
        assert_eq!(r.valid_len + r.discarded_bytes, cut as u64);
        if cut == full.len() {
            assert_eq!(expect, 3);
        }
    }
    // Reopen after a cut in the middle of the last record: truncates physically, appends cleanly.
    let cut = full.len() - 5;
    fs.write_atomic(&path, &full[..cut]).unwrap();
    let (j2, rec) = reopen(&fs, &root, &clock);
    assert_eq!(rec.commits.len(), 2);
    assert_eq!(
        rec.unfinished.len(),
        1,
        "intent 3 survived without its commit"
    );
    assert!(rec.discarded_bytes > 0);
    j2.resolve_unfinished(rec.unfinished[0].seq, Resolution::Skip)
        .unwrap();
    run(&j2, "d", 9, 10);
    let (_j3, rec3) = reopen(&fs, &root, &clock);
    assert_eq!(
        rec3.discarded_bytes, 0,
        "tail was physically truncated before appending"
    );
    assert_eq!(rec3.commits.len(), 3);
    let _ = f;
}

#[test]
fn corrupted_crc_truncates_at_last_valid_record() {
    let (f, fs, root, clock) = mem();
    let j = new_journal(&fs, &root, &clock, Options::default());
    for i in 1..=4 {
        run(&j, "c", i, 64);
    }
    let path = j.dir().join("journal.log");
    // Find the start of the 3rd command's Intent by scanning valid prefixes.
    let full = fs.read(&path).unwrap();
    let one = {
        let (_, rec) = (0, recover(fs.as_ref(), j.dir()).unwrap());
        rec.valid_len as usize / 4
    };
    // Flip a byte in the payload of the third command (approximately record 5 of 8).
    let target = 8 + one * 2 + 12;
    f.corrupt_byte(&path, target);
    let rec = recover(fs.as_ref(), j.dir()).unwrap();
    assert_eq!(rec.tail, Stop::Corrupt);
    assert!(
        rec.commits.len() <= 2,
        "nothing at or after the corrupted record is trusted"
    );
    assert!(rec.discarded_bytes > 0);
    assert!(
        rec.commits
            .iter()
            .enumerate()
            .all(|(i, c)| c.seq == i as u64 + 1),
        "survivors are an intact prefix"
    );
    assert!(full.len() as u64 > rec.valid_len);
    // Garbage length field is rejected without allocating.
    let mut bad = fs.read(&path).unwrap();
    bad.truncate(8);
    bad.extend_from_slice(&u32::MAX.to_le_bytes());
    bad.extend_from_slice(&[0; 20]);
    fs.write_atomic(&path, &bad).unwrap();
    let rec = recover(fs.as_ref(), j.dir()).unwrap();
    assert_eq!(rec.tail, Stop::Corrupt);
    assert!(rec.commits.is_empty());
    // Bad magic is a hard error, not silent data loss.
    fs.write_atomic(&path, b"NOTAJOURNALFILE!").unwrap();
    assert!(matches!(
        recover(fs.as_ref(), j.dir()),
        Err(JournalError::Corrupt(_))
    ));
}

#[test]
fn power_loss_loses_at_most_one_second() {
    for seed in 0..40u64 {
        let (f, fs, root, clock) = mem();
        let j = new_journal(&fs, &root, &clock, Options::default());
        let mut rng = fastrand(seed);
        let loss_at = 1_000_000 + 500 + rng() % 6_000; // ms
        let mut issued: Vec<(u64, u64)> = Vec::new(); // (seq, commit time)
        let mut t = 1_000_000u64;
        while t < loss_at {
            clock.set(t);
            f.set_now(t);
            let seq = run(&j, "cmd", (issued.len() + 1) as u32, 200);
            issued.push((seq, t));
            // Flusher ticks every 50 ms.
            t += 10 + rng() % 40;
            for tick in (clock_tick_start(&clock)..=t)
                .step_by(1)
                .filter(|x| x % 50 == 0)
            {
                clock.set(tick);
                j.sync_if_due().unwrap();
            }
            clock.set(t);
        }
        f.power_loss(if seed % 2 == 0 {
            PowerLoss::DropUnsynced
        } else {
            PowerLoss::KeepPrefix(seed)
        });
        let (_j2, rec) = reopen(&fs, &root, &clock);
        let survived: Vec<u64> = rec.commits.iter().map(|c| c.seq).collect();
        // Prefix property: what survived is a prefix of what was issued.
        for (i, s) in survived.iter().enumerate() {
            assert_eq!(*s, issued[i].0, "seed {seed}");
        }
        // Everything older than interval + tick granularity must have survived.
        for (seq, ts) in &issued {
            if loss_at.saturating_sub(*ts) > 1_000 + 50 + 50 {
                assert!(
                    survived.contains(seq),
                    "seed {seed}: seq {seq} at {ts} lost with power loss at {loss_at}"
                );
            }
        }
    }
}

fn clock_tick_start(c: &ManualClock) -> u64 {
    use papyrine_journal::Clock;
    c.now_ms()
}

fn fastrand(mut x: u64) -> impl FnMut() -> u64 {
    x = x
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
        | 1;
    move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x >> 8
    }
}

#[test]
fn unsynced_data_is_really_dropped_and_group_commit_batches() {
    let (f, fs, root, clock) = mem();
    let j = new_journal(&fs, &root, &clock, Options::default());
    let before = f.sync_count();
    for i in 1..=50 {
        run(&j, "c", i, 20);
    }
    assert_eq!(f.sync_count(), before, "no fsync per record");
    assert!(!j.sync_if_due().unwrap(), "not due yet");
    clock.advance(1_000);
    assert!(j.sync_if_due().unwrap());
    assert_eq!(f.sync_count(), before + 1, "one fsync covers 50 commands");
    for i in 51..=60 {
        run(&j, "c", i, 20);
    }
    f.power_loss(PowerLoss::DropUnsynced);
    let (_j, rec) = reopen(&fs, &root, &clock);
    assert_eq!(rec.commits.len(), 50);
}

#[test]
fn interval_is_clamped() {
    let (f, fs, root, clock) = mem();
    let o = Options {
        sync_interval: Duration::from_millis(10),
        ..Options::default()
    };
    let j = new_journal(&fs, &root, &clock, o);
    run(&j, "c", 1, 20);
    clock.advance(150);
    assert!(!j.sync_if_due().unwrap(), "0.2 s floor");
    clock.advance(60);
    assert!(j.sync_if_due().unwrap());
    let o = Options {
        sync_interval: Duration::from_secs(60),
        ..Options::default()
    };
    let j = Journal::create(
        &root,
        fs.clone(),
        "doc2",
        &base(),
        o,
        Arc::new(clock.clone()),
    )
    .unwrap();
    run(&j, "c", 1, 20);
    clock.advance(5_000);
    assert!(j.sync_if_due().unwrap(), "5 s ceiling");
    let _ = f;
}

#[test]
fn large_payload_waits_for_its_own_fsync() {
    let (f, fs, root, clock) = mem();
    let j = new_journal(&fs, &root, &clock, Options::default());
    // Incompressible after-image above 1 MB.
    let mut x = 12345u64;
    let data: Vec<u8> = (0..1_500_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    let t = j.begin("insert-image", json!({}), vec![]).unwrap();
    let before = f.sync_count();
    j.commit(
        t,
        vec![AfterImage {
            id: ObjId {
                num: 7,
                generation: 0,
            },
            data: data.clone(),
        }],
        vec![],
    )
    .unwrap();
    assert!(
        f.sync_count() > before,
        "commit > 1 MB fsyncs before returning"
    );
    assert!(!j.unsynced());
    // Large external input: blob is durable, and the Intent referencing it is fsynced.
    let blob = j.put_blob(&data).unwrap();
    let t2 = j.begin("insert-pdf", json!({}), vec![blob]).unwrap();
    assert!(!j.unsynced());
    f.power_loss(PowerLoss::DropUnsynced);
    let (j2, rec) = reopen(&fs, &root, &clock);
    assert_eq!(rec.commits[0].after_images[0].data, data);
    assert_eq!(rec.unfinished.len(), 1);
    assert_eq!(rec.unfinished[0].blobs, vec![blob]);
    assert_eq!(j2.read_blob(&blob).unwrap(), data);
    let _ = t2;
}

#[test]
fn poison_command_is_quarantined_after_two_crashes() {
    let (_f, fs, root, clock) = mem();
    let params = json!({"pages": [1]});
    let j = new_journal(&fs, &root, &clock, Options::default());
    // First try: process dies mid-apply.
    let t = j.begin("poison", params.clone(), vec![]).unwrap();
    let _ = t;
    drop(j);
    let (j, rec) = reopen(&fs, &root, &clock);
    assert_eq!(rec.unfinished[0].prior_crashes, 0);
    assert_eq!(
        j.resolve_unfinished(rec.unfinished[0].seq, Resolution::Redo)
            .unwrap(),
        1
    );
    // Redo: crashes again (engine crash, same process: host resolves the in-flight one).
    let t = j.begin("poison", params.clone(), vec![]).unwrap();
    let seq = t.seq();
    assert_eq!(j.resolve_unfinished(seq, Resolution::Skip).unwrap(), 2);
    assert!(j.is_quarantined("poison", &params, &[]));
    match j.begin("poison", params.clone(), vec![]) {
        Err(JournalError::Quarantined { command, crashes }) => {
            assert_eq!(command, "poison");
            assert_eq!(crashes, 2);
        }
        other => panic!("expected quarantine, got {other:?}"),
    }
    // Different params are a different command and still run.
    run(&j, "poison", 5, 10);
    // Persisted: survives reopen.
    drop(j);
    let (j, rec) = reopen(&fs, &root, &clock);
    assert_eq!(
        rec.quarantined,
        vec![Quarantined {
            command: "poison".into(),
            crashes: 2
        }]
    );
    assert!(matches!(
        j.begin("poison", params, vec![]),
        Err(JournalError::Quarantined { .. })
    ));
}

#[test]
fn crash_counting_is_idempotent_per_intent() {
    let (_f, fs, root, clock) = mem();
    let j = new_journal(&fs, &root, &clock, Options::default());
    let _ = j.begin("p", json!({}), vec![]).unwrap();
    drop(j);
    let (j, rec) = reopen(&fs, &root, &clock);
    let seq = rec.unfinished[0].seq;
    assert_eq!(j.resolve_unfinished(seq, Resolution::Skip).unwrap(), 1);
    assert!(
        j.resolve_unfinished(seq, Resolution::Skip).is_err(),
        "already resolved"
    );
}

#[test]
fn checkpoint_truncates_and_replays_from_checkpoint() {
    let (_f, fs, root, clock) = mem();
    let o = Options {
        checkpoint_records: 10,
        ..Options::default()
    };
    let j = new_journal(&fs, &root, &clock, o);
    for i in 1..=4 {
        run(&j, "c", i, 100);
    }
    assert!(!j.checkpoint_due(), "8 records < 10");
    // 8 records so far; two more commands tip it over.
    run(&j, "c", 5, 100);
    assert!(j.checkpoint_due());
    let state = vec![7u8; 5000];
    j.checkpoint(&state).unwrap();
    assert!(!j.checkpoint_due());
    run(&j, "after", 6, 100);
    drop(j);
    let (j2, rec) = reopen(&fs, &root, &clock);
    let c = rec.checkpoint.clone().unwrap();
    assert!(c.blob_ok);
    assert_eq!(c.upto_seq, 5);
    assert_eq!(rec.commits.len(), 1);
    assert_eq!(
        rec.commits[0].seq, 6,
        "seq numbering continues across the checkpoint"
    );
    assert_eq!(j2.read_blob(&c.blob).unwrap(), state);
    // Second checkpoint replaces the first blob.
    run(&j2, "x", 7, 10);
    j2.checkpoint(&[9u8; 100]).unwrap();
    let blobs = fs.list_dir(&j2.dir().join("blobs")).unwrap();
    assert_eq!(blobs.len(), 1);
    // Refuses while a command is in flight.
    let t = j2.begin("y", json!({}), vec![]).unwrap();
    assert!(matches!(j2.checkpoint(&[1]), Err(JournalError::Busy)));
    j2.fail(t).unwrap();
}

#[test]
fn checkpoint_due_by_size() {
    let (_f, fs, root, clock) = mem();
    let o = Options {
        checkpoint_bytes: 10_000,
        ..Options::default()
    };
    let j = new_journal(&fs, &root, &clock, o);
    assert!(!j.checkpoint_due());
    let mut x = 99u64;
    let noisy: Vec<u8> = (0..12_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    let t = j.begin("big", json!({}), vec![]).unwrap();
    j.commit(
        t,
        vec![AfterImage {
            id: ObjId {
                num: 1,
                generation: 0,
            },
            data: noisy,
        }],
        vec![],
    )
    .unwrap();
    assert!(j.checkpoint_due());
}

#[test]
fn clean_save_rebase_and_discard() {
    let (_f, fs, root, clock) = mem();
    let j = new_journal(&fs, &root, &clock, Options::default());
    run(&j, "a", 1, 10);
    let nb = BaseInfo {
        size: 99,
        blake3: "ff".into(),
        ..base()
    };
    j.rebase(&nb).unwrap();
    run(&j, "b", 2, 10);
    let (j2, rec) = reopen(&fs, &root, &clock);
    assert_eq!(rec.base, nb);
    assert_eq!(rec.commits.len(), 1, "only the post-save command remains");
    assert_eq!(rec.commits[0].command, "b");
    // A log ending in Clean (crash between marker and reset) means: nothing to restore.
    let (f2, fs2, root2, clock2) = mem();
    let j3 = new_journal(&fs2, &root2, &clock2, Options::default());
    run(&j3, "a", 1, 10);
    let dir = j3.dir().to_path_buf();
    let mut log = fs2.read(&dir.join("journal.log")).unwrap();
    log.extend_from_slice(&clean_bytes());
    fs2.write_atomic(&dir.join("journal.log"), &log).unwrap();
    let r = recover(fs2.as_ref(), &dir).unwrap();
    assert!(r.clean && r.commits.is_empty());
    let _ = f2;
    // Discard removes everything.
    j2.discard().unwrap();
    assert!(root.scan().is_empty());
    assert!(matches!(
        j2.begin("z", json!({}), vec![]),
        Err(JournalError::Closed)
    ));
}

fn clean_bytes() -> Vec<u8> {
    // [len=9][crc][kind=4][ts u64]
    let mut payload = vec![4u8];
    payload.extend_from_slice(&5u64.to_le_bytes());
    let mut out = (payload.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&crc32c::crc32c(&payload).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

#[test]
fn append_failure_rolls_back_torn_record() {
    let (f, fs, root, clock) = mem();
    let j = new_journal(&fs, &root, &clock, Options::default());
    run(&j, "a", 1, 10);
    f.fail_next_append_torn();
    assert!(j.begin("b", json!({}), vec![]).is_err());
    // Journal still usable: the half-written record was truncated away.
    run(&j, "c", 3, 10);
    let r = recover(fs.as_ref(), j.dir()).unwrap();
    assert_eq!(r.discarded_bytes, 0);
    assert_eq!(r.commits.len(), 2);
    // If rollback also fails the journal refuses further writes instead of corrupting.
    f.fail_next_append_torn();
    f.set_fail_truncate(true);
    assert!(j.begin("d", json!({}), vec![]).is_err());
    assert!(matches!(
        j.begin("e", json!({}), vec![]),
        Err(JournalError::Broken(_))
    ));
}

#[test]
fn fsync_failure_is_sticky() {
    let (f, fs, root, clock) = mem();
    let j = new_journal(&fs, &root, &clock, Options::default());
    run(&j, "a", 1, 10);
    f.set_fail_sync(true);
    clock.advance(2_000);
    assert!(j.sync_if_due().is_err());
    assert!(matches!(
        j.begin("b", json!({}), vec![]),
        Err(JournalError::Broken(_))
    ));
}

#[test]
fn engine_never_receives_a_command_without_a_written_intent() {
    let dir = tempfile::tempdir().unwrap();
    let fs: Arc<dyn Fs> = Arc::new(RealFs);
    let root = RecoveryRoot::new(fs.clone(), dir.path().join("recovery"));
    let j = Journal::create(
        &root,
        fs.clone(),
        "d",
        &base(),
        Options::default(),
        Arc::new(SystemClock),
    )
    .unwrap();
    let (tx, rx) = mpsc::channel::<Logged<String>>();
    let jdir = j.dir().to_path_buf();
    let jengine = j.clone();
    let engine = std::thread::spawn(move || {
        let mut got = 0;
        while let Ok(Logged { token, msg }) = rx.recv() {
            // Engine side check: read the log from disk *as the engine receives the command*.
            let rec = recover(&RealFs, &jdir).unwrap();
            let seen = rec
                .unfinished
                .iter()
                .any(|u| u.seq == token.seq() && u.command == msg);
            assert!(
                seen,
                "engine received {msg} (seq {}) before its Intent was in the log",
                token.seq()
            );
            jengine.commit(token, vec![img(1, 8, 0)], vec![]).unwrap();
            got += 1;
        }
        got
    });
    let d = Dispatcher::new(j.clone(), tx);
    // The engine commits asynchronously; serialise like the real host actor does.
    let mut sent = 0;
    for i in 0..200 {
        loop {
            match d.dispatch(
                &format!("cmd{i}"),
                json!({"i": i}),
                vec![],
                format!("cmd{i}"),
            ) {
                Ok(_) => {
                    sent += 1;
                    break;
                }
                Err(DispatchError::Journal(JournalError::Busy)) => std::thread::yield_now(),
                Err(e) => panic!("{e}"),
            }
        }
    }
    drop(d);
    assert_eq!(engine.join().unwrap(), sent);
}

#[test]
fn failed_intent_write_forwards_nothing() {
    let (f, fs, root, clock) = mem();
    let j = new_journal(&fs, &root, &clock, Options::default());
    let (tx, rx) = mpsc::channel::<Logged<u32>>();
    let d = Dispatcher::new(j.clone(), tx);
    f.fail_next_append_torn();
    assert!(matches!(
        d.dispatch("x", json!({}), vec![], 1),
        Err(DispatchError::Journal(_))
    ));
    assert!(
        rx.try_recv().is_err(),
        "engine got a command whose Intent failed to write"
    );
    // Quarantined commands are refused before anything is forwarded as well.
    for _ in 0..2 {
        let t = j.begin("poison", json!({}), vec![]).unwrap();
        j.resolve_unfinished(t.seq(), Resolution::Skip).unwrap();
    }
    assert!(matches!(
        d.dispatch("poison", json!({}), vec![], 2),
        Err(DispatchError::Journal(JournalError::Quarantined { .. }))
    ));
    assert!(rx.try_recv().is_err());
    // Engine gone: intent closed cleanly, not counted as crash.
    drop(rx);
    assert!(matches!(
        d.dispatch("y", json!({}), vec![], 3),
        Err(DispatchError::EngineGone)
    ));
    run(&j, "z", 1, 10);
}

#[test]
fn prune_old_dirs_and_scan() {
    let (f, fs, root, clock) = mem();
    let day = 86_400_000u64;
    f.set_now(100 * day);
    let _old = Journal::create(
        &root,
        fs.clone(),
        "old",
        &base(),
        Options::default(),
        Arc::new(clock.clone()),
    )
    .unwrap();
    f.set_now(125 * day);
    let _new = Journal::create(
        &root,
        fs.clone(),
        "new",
        &base(),
        Options::default(),
        Arc::new(clock.clone()),
    )
    .unwrap();
    assert_eq!(root.scan().len(), 2);
    let pruned = root.prune(131 * day, PRUNE_AFTER_DAYS);
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].doc_id, "old");
    assert_eq!(pruned[0].original_path.as_deref(), Some("/tmp/x.pdf"));
    assert_eq!(root.scan().len(), 1);
    assert!(root.doc_dir("../evil").is_err());
}

#[test]
fn backup_exclusion_requested() {
    let (f, fs, root, clock) = mem();
    let _j = new_journal(&fs, &root, &clock, Options::default());
    assert_eq!(f.excluded(), vec![Path::new("/rec").to_path_buf()]);
    let p = backup_exclude_plist();
    assert_eq!(&p[..8], b"bplist00");
    assert_eq!(p.len(), 61);
}

#[cfg(unix)]
#[test]
fn real_fs_permissions_are_owner_only_and_flusher_syncs() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let fs: Arc<dyn Fs> = Arc::new(RealFs);
    let root = RecoveryRoot::new(fs.clone(), dir.path().join("recovery"));
    let o = Options {
        sync_interval: Duration::from_millis(200),
        ..Options::default()
    };
    let j = Journal::create(&root, fs.clone(), "d", &base(), o, Arc::new(SystemClock)).unwrap();
    let blob = j.put_blob(b"hello").unwrap();
    run(&j, "a", 1, 10);
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&j.dir().join("journal.log")), 0o600);
    assert_eq!(mode(&j.dir().join("base.json")), 0o600);
    assert_eq!(
        mode(&j.dir().join("blobs").join(hash_hex(&blob.hash))),
        0o600
    );
    assert_eq!(mode(j.dir()), 0o700);
    let _fl = j.spawn_flusher();
    assert!(j.unsynced());
    for _ in 0..40 {
        if !j.unsynced() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !j.unsynced(),
        "background flusher group-committed within the interval"
    );
}
