#![allow(dead_code)]
//! Headless harness: a real [`Broker`] with real sandboxed engine and renderer children.
//! The test binary re-executes itself as the child (`child_entry`), like the ipc tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use papyrine_host::api::{CollectSink, DocumentInfo, EngineCommand, HostEvent};
use papyrine_host::broker::{Broker, Config};
use papyrine_host::supervisor::ExeLauncher;

/// The re-executed test binary becomes the child here. With
/// `PAPYRINE_TEST_CRASH_ON=<needle>` the engine exits abruptly on any command whose
/// parameters contain the needle (a deterministic "this command crashes the engine").
#[test]
fn child_entry() {
    use papyrine_engine::EngineHandler;
    use papyrine_engine::proto::{Request, Response};
    use papyrine_ipc::{Ctx, Handler, IpcError, Role, ServeExit};

    struct Crashy {
        inner: EngineHandler,
        needle: String,
    }
    impl Handler for Crashy {
        type Req = Request;
        type Resp = Response;
        fn handle(
            &mut self,
            cx: &mut Ctx<'_, Response>,
            req: Request,
        ) -> Result<Response, IpcError> {
            if let Request::Execute { command, .. } = &req
                && command.params_json.contains(&self.needle)
            {
                std::process::exit(9);
            }
            self.inner.handle(cx, req)
        }
    }

    let Some(b) = papyrine_ipc::bootstrap().expect("bootstrap") else {
        return;
    };
    match b.role {
        Role::Engine => {
            match std::env::var("PAPYRINE_TEST_CRASH_ON") {
                Ok(needle) if !needle.is_empty() => {
                    papyrine_ipc::serve(b.endpoint, Role::Engine, move || Crashy {
                        inner: EngineHandler::default(),
                        needle,
                    })
                    .expect("serve");
                }
                _ => {
                    let exit = papyrine_engine::run_engine(b.endpoint).expect("serve");
                    assert!(matches!(exit, ServeExit::Shutdown | ServeExit::HostGone));
                }
            }
            std::process::exit(0);
        }
        Role::Renderer => {
            papyrine_render::role::run_renderer(b.endpoint).expect("serve");
            std::process::exit(0);
        }
        Role::Other(_) => unreachable!(),
    }
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// A corpus file from `corpus/cache/generated`, if it has been generated here.
pub fn corpus(name: &str) -> Option<PathBuf> {
    let p = repo_root().join("corpus/cache/generated").join(name);
    p.is_file().then_some(p)
}

/// A valid classic-xref PDF with `pages` pages, each with its page number as text.
pub fn build_pdf(pages: usize) -> Vec<u8> {
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
        "<< /Producer (host-tests) /Title (Host test document) /Author (Tester) >>".into(),
    );
    for i in 0..pages {
        let content = format!(
            "BT /F1 48 Tf 72 700 Td (Page {}) Tj ET\nBT /F1 14 Tf 72 640 Td (The quick brown fox jumps over the lazy dog) Tj ET\n",
            i + 1
        );
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

pub struct Harness {
    pub broker: Broker,
    pub sink: Arc<CollectSink>,
    pub dir: tempfile::TempDir,
}

pub struct Opts {
    pub crash_on: Option<String>,
    pub sandbox: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            crash_on: None,
            sandbox: std::env::var_os("PAPYRINE_TEST_NO_SANDBOX").is_none(),
        }
    }
}

pub fn config_in(dir: &Path, sink: Arc<CollectSink>, o: &Opts) -> Config {
    let mut l = ExeLauncher {
        args: vec![
            "common::child_entry".into(),
            "--exact".into(),
            "--nocapture".into(),
            "--test-threads=1".into(),
        ],
        sandbox: o.sandbox,
        ..ExeLauncher::default()
    };
    if let Some(d) = papyrine_host::children::dev_pdfium_dir() {
        l = l.with_pdfium(d);
    }
    if let Some(n) = &o.crash_on {
        l.env.push(("PAPYRINE_TEST_CRASH_ON".into(), n.clone()));
    }
    let mut cfg = Config::new(dir.join("data"), dir.join("cache"), Arc::new(l), sink);
    cfg.watch_files = true;
    cfg
}

pub fn harness() -> Harness {
    harness_with(Opts::default())
}

pub fn harness_with(o: Opts) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(CollectSink::default());
    let broker = Broker::new(config_in(dir.path(), sink.clone(), &o));
    Harness { broker, sink, dir }
}

impl Harness {
    /// Write a generated PDF under the temp dir and return its path.
    pub fn pdf(&self, name: &str, pages: usize) -> PathBuf {
        let p = self.dir.path().join(name);
        std::fs::write(&p, build_pdf(pages)).unwrap();
        p
    }

    pub fn open(&self, p: &Path) -> DocumentInfo {
        self.broker
            .open_path(p, None)
            .unwrap_or_else(|e| panic!("open {}: {e}", p.display()))
    }

    pub fn events(&self) -> Vec<HostEvent> {
        self.sink.snapshot()
    }

    pub fn wait_event(&self, what: &str, f: impl Fn(&HostEvent) -> bool) -> HostEvent {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(e) = self.sink.snapshot().into_iter().find(|e| f(e)) {
                return e;
            }
            assert!(
                Instant::now() < end,
                "timed out waiting for {what}; got {:?}",
                self.events()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

pub fn rotate(pages: &[usize], degrees: i32) -> EngineCommand {
    EngineCommand::RotatePages {
        pages: pages.to_vec(),
        degrees,
    }
}

pub fn have_qpdf() -> bool {
    std::process::Command::new("qpdf")
        .arg("--version")
        .output()
        .is_ok()
}

/// `qpdf --check` must pass with no errors.
pub fn qpdf_check(p: &Path) {
    if !have_qpdf() {
        return;
    }
    let out = std::process::Command::new("qpdf")
        .arg("--check")
        .arg(p)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "qpdf --check failed for {}: {}{}",
        p.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// True when some pixel is not (near) white.
pub fn has_ink(t: &papyrine_host::cache::TileData) -> bool {
    let (px, _) = t.rgba.as_chunks::<4>();
    px.iter().any(|p| p[0] < 128 || p[1] < 128 || p[2] < 128)
}
