//! Crash-test helper for tests/kill9.rs. Not a product binary.
//!
//! `killer host <recovery-root> <doc-id>`: a stand-in for the Papyrine host. It journals
//! commands and forwards them to a child "engine" process, printing progress to stdout:
//!   `E <pid>`  engine spawned      `I <seq>`  Intent written (begin returned)
//!   `C <seq>`  Commit returned     `X <seq>`  engine died mid-command, intent resolved
//! `killer engine`: reads `<seq>\n` on stdin, answers with the deterministic after-image.
//! The test SIGKILLs the host and/or the engine at random moments.

use papyrine_journal::*;
use serde_json::json;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;

include!("killer_payload.rs.inc");

fn engine() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return };
        let seq: u64 = line.trim().parse().unwrap();
        let data = payload(seq);
        let mut reply = Vec::with_capacity(data.len() + 12);
        reply.extend_from_slice(&seq.to_le_bytes());
        reply.extend_from_slice(&(data.len() as u32).to_le_bytes());
        reply.extend_from_slice(&data);
        // Write in pieces so a kill can land mid-reply; a dead host just ends the engine.
        for chunk in reply.chunks(16 * 1024) {
            if out.write_all(chunk).is_err() {
                return;
            }
        }
        if out.flush().is_err() {
            return;
        }
    }
}

struct Engine {
    child: std::process::Child,
    tx: std::process::ChildStdin,
    rx: BufReader<std::process::ChildStdout>,
}

fn spawn_engine() -> Engine {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("engine")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    println!("E {}", child.id());
    let tx = child.stdin.take().unwrap();
    let rx = BufReader::new(child.stdout.take().unwrap());
    Engine { child, tx, rx }
}

fn ask(e: &mut Engine, seq: u64) -> std::io::Result<Vec<u8>> {
    writeln!(e.tx, "{seq}")?;
    e.tx.flush()?;
    let mut head = [0u8; 12];
    e.rx.read_exact(&mut head)?;
    let got = u64::from_le_bytes(head[..8].try_into().unwrap());
    let len = u32::from_le_bytes(head[8..].try_into().unwrap()) as usize;
    assert_eq!(got, seq);
    let mut data = vec![0u8; len];
    e.rx.read_exact(&mut data)?;
    Ok(data)
}

fn host(root: &str, doc: &str) {
    let fs: Arc<dyn Fs> = Arc::new(RealFs);
    let root = RecoveryRoot::new(fs.clone(), root);
    let base = BaseInfo {
        original_path: "/nonexistent/original.pdf".into(),
        size: 1,
        mtime_ms: 0,
        blake3: "00".into(),
    };
    let opts = Options {
        sync_interval: std::time::Duration::from_millis(200),
        ..Options::default()
    };
    let j = Journal::create(&root, fs, doc, &base, opts, Arc::new(SystemClock)).unwrap();
    let _flusher = j.spawn_flusher();
    let mut eng = spawn_engine();
    let mut n = 0u64;
    loop {
        n += 1;
        let blobs = if n.is_multiple_of(40) {
            vec![j.put_blob(&payload(n + 1_000_000)).unwrap()]
        } else {
            vec![]
        };
        let tok = j.begin("cmd", json!({ "n": n }), blobs).unwrap();
        let seq = tok.seq();
        println!("I {seq}");
        match ask(&mut eng, seq) {
            Ok(data) => {
                j.commit(
                    tok,
                    vec![AfterImage {
                        id: ObjId {
                            num: seq as u32,
                            generation: 0,
                        },
                        data,
                    }],
                    vec![ObjId {
                        num: seq as u32 + 1_000_000,
                        generation: 0,
                    }],
                )
                .unwrap();
                println!("C {seq}");
            }
            Err(_) => {
                // Engine died while applying: leave the engine dead, resolve, restart.
                let _ = eng.child.wait();
                let _ = tok;
                j.resolve_unfinished(seq, Resolution::Skip).unwrap();
                println!("X {seq}");
                eng = spawn_engine();
            }
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("engine") => engine(),
        Some("host") => host(&a[2], &a[3]),
        _ => eprintln!("usage: killer host <root> <doc> | engine"),
    }
}
