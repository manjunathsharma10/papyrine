//! Headless host for the dev bridge: the real broker, engine and renderer, no window.
//!
//! `papyrine-bridge --data-dir DIR [--port N] [--no-sandbox]` prints one line,
//! `PAPYRINE_BRIDGE ws://127.0.0.1:PORT/TOKEN`, and serves until killed. The Playwright
//! crash tests kill it with SIGKILL and start it again on the same data directory.

use std::path::PathBuf;
use std::sync::Arc;

use papyrine_host::bridge::{BridgeSink, ScriptedDialogs, start};
use papyrine_host::broker::{Broker, Config};
use papyrine_host::supervisor::ExeLauncher;

fn main() {
    if let Some(code) = papyrine_host::children::run_child_if_requested() {
        std::process::exit(code);
    }
    let mut data_dir = None;
    let mut port = 0u16;
    let mut sandbox = true;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data-dir" => data_dir = it.next().map(PathBuf::from),
            "--port" => port = it.next().and_then(|p| p.parse().ok()).unwrap_or(0),
            "--no-sandbox" => sandbox = false,
            other => {
                eprintln!("papyrine-bridge: unknown argument {other:?}");
                std::process::exit(2);
            }
        }
    }
    let data_dir = data_dir.unwrap_or_else(|| std::env::temp_dir().join("papyrine-bridge"));
    let mut launcher = ExeLauncher {
        sandbox,
        ..ExeLauncher::default()
    };
    if let Some(d) = papyrine_host::children::dev_pdfium_dir() {
        launcher = launcher.with_pdfium(d);
    }
    let sink = Arc::new(BridgeSink::default());
    let dialogs = Arc::new(ScriptedDialogs::default());
    let mut cfg = Config::new(
        data_dir.clone(),
        data_dir.join("cache"),
        Arc::new(launcher),
        sink.clone(),
    );
    cfg.dialogs = dialogs.clone();
    let broker = Broker::new(cfg);
    broker.warm_up();
    broker.after_first_paint();
    let bridge = match start(broker, sink, dialogs, port) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("papyrine-bridge: cannot listen: {e}");
            std::process::exit(1);
        }
    };
    println!("PAPYRINE_BRIDGE {}", bridge.url());
    use std::io::Write;
    let _ = std::io::stdout().flush();
    loop {
        std::thread::park();
    }
}
