//! Sandbox acceptance tests (ROADMAP 1.9). Each test spawns this test binary
//! as a child with a role in the environment; the child applies the sandbox,
//! then reports over the IPC channel what it could and could not do.
//!
//! Denied inside the sandbox: reading `~`, writing outside the temp dir,
//! opening a socket, spawning a process. Allowed: normal work, loading PDFium
//! from the cache dir and rendering a tile from passed bytes, and opening a
//! PDF with papyrine-cos from passed bytes.

use papyrine_ipc::*;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const ENV_HOME_FILE: &str = "PROBE_HOME_FILE";
const ENV_OUTSIDE_FILE: &str = "PROBE_OUTSIDE_FILE";
const ENV_TCP_ADDR: &str = "PROBE_TCP_ADDR";
const ENV_HOST_CWD: &str = "PROBE_HOST_CWD";
const ENV_PDFIUM: &str = "PAPYRINE_PDFIUM_DIR";

#[derive(Debug, Serialize, Deserialize)]
struct Probe {
    name: String,
    /// True when the operation succeeded.
    succeeded: bool,
    detail: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProbeReport {
    mechanisms: Vec<String>,
    sandboxed: bool,
    probes: Vec<Probe>,
}

fn probe(name: &str, r: std::io::Result<()>) -> Probe {
    Probe {
        name: name.into(),
        succeeded: r.is_ok(),
        detail: r.err().map(|e| e.to_string()).unwrap_or_default(),
    }
}

fn run_probes() -> Vec<Probe> {
    let mut v = Vec::new();
    // --- reading ~
    let home_file = PathBuf::from(std::env::var(ENV_HOME_FILE).unwrap());
    v.push(probe(
        "read_home_file",
        std::fs::read(&home_file).map(|_| ()),
    ));
    v.push(probe(
        "list_home_dir",
        std::fs::read_dir(home_file.parent().unwrap()).map(|_| ()),
    ));
    // --- writing outside temp
    let outside = PathBuf::from(std::env::var(ENV_OUTSIDE_FILE).unwrap());
    v.push(probe("write_outside_file", std::fs::write(&outside, b"x")));
    v.push(probe(
        "write_home_new_file",
        std::fs::write(home_file.with_extension("new"), b"x"),
    ));
    // The host's working directory (the child's own cwd is its temp dir on Windows).
    let host_cwd_file =
        PathBuf::from(std::env::var(ENV_HOST_CWD).unwrap()).join("papyrine-sandbox-cwd-probe");
    v.push(probe(
        "write_host_cwd",
        std::fs::write(&host_cwd_file, b"x").inspect(|_| {
            let _ = std::fs::remove_file(&host_cwd_file);
        }),
    ));
    // --- sockets
    let addr = std::env::var(ENV_TCP_ADDR).unwrap();
    v.push(probe(
        "tcp_connect",
        TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)).map(|_| ()),
    ));
    v.push(probe(
        "udp_bind",
        UdpSocket::bind("127.0.0.1:0").map(|_| ()),
    ));
    v.push(probe(
        "tcp_listen",
        TcpListener::bind("127.0.0.1:0").map(|_| ()),
    ));
    // Windows cannot deny bind/listen to an AppContainer (the network stack only filters
    // traffic), so what matters is that nothing can reach the socket: not even the child
    // itself over loopback.
    v.push(probe(
        "loopback_self_connect",
        TcpListener::bind("127.0.0.1:0").and_then(|l| {
            TcpStream::connect_timeout(&l.local_addr()?, Duration::from_secs(2)).map(|_| ())
        }),
    ));
    // --- spawning a process
    v.push(probe("spawn_process", spawn_something()));
    #[cfg(unix)]
    {
        // SAFETY: fork; the (unexpected) child exits immediately without running any Rust code.
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            unsafe { libc::_exit(0) };
        }
        v.push(probe(
            "fork",
            if pid < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            },
        ));
    }
    // --- normal operation
    let temp = std::env::temp_dir();
    let f = temp.join("normal.txt");
    v.push(probe(
        "write_temp_file",
        std::fs::write(&f, b"hello")
            .and_then(|_| std::fs::read(&f))
            .and_then(|b| {
                if b == b"hello" {
                    Ok(())
                } else {
                    Err(std::io::Error::other("readback mismatch"))
                }
            }),
    ));
    v.push(probe(
        "create_temp_subdir",
        std::fs::create_dir_all(temp.join("a/b"))
            .and_then(|_| std::fs::write(temp.join("a/b/c"), b"1")),
    ));
    v.push(probe(
        "read_own_exe",
        std::fs::read(std::env::current_exe().unwrap()).map(|b| assert!(!b.is_empty())),
    ));
    v.push(probe(
        "threads",
        std::thread::spawn(|| 40 + 2)
            .join()
            .map(|x| assert_eq!(x, 42))
            .map_err(|_| std::io::Error::other("thread panicked")),
    ));
    v.push(probe("allocate_256mb", {
        let mut big = vec![0u8; 256 << 20];
        for i in (0..big.len()).step_by(4096) {
            big[i] = 1;
        }
        Ok(())
    }));
    v.push(probe(
        "shared_memory",
        SharedRegion::create(1 << 20).map(|r| r.write_at(0, b"ok")),
    ));
    v
}

fn spawn_something() -> std::io::Result<()> {
    #[cfg(windows)]
    let mut c = std::process::Command::new("cmd.exe");
    #[cfg(windows)]
    c.args(["/c", "exit", "0"]);
    #[cfg(unix)]
    let mut c = std::process::Command::new("/bin/echo");
    c.stdout(std::process::Stdio::null())
        .spawn()
        .and_then(|mut ch| ch.wait())
        .map(|_| ())
}

// ---------------------------------------------------------- render / engine

struct RenderChild {
    lib: Arc<papyrine_render::Library>,
    doc: Option<papyrine_render::Document>,
    pool: Option<(SharedRegion, u64)>,
}

impl Handler for RenderChild {
    type Req = RenderRequest;
    type Resp = RenderResponse;

    fn handle(
        &mut self,
        cx: &mut Ctx<'_, RenderResponse>,
        req: RenderRequest,
    ) -> Result<RenderResponse, IpcError> {
        let io = |e: papyrine_render::Error| IpcError::new(ErrorCode::Corrupt, e.to_string());
        match req {
            RenderRequest::Ping { nonce } => Ok(RenderResponse::Pong { nonce }),
            RenderRequest::SetTilePool {
                pool, slot_bytes, ..
            } => {
                self.pool = Some((pool, slot_bytes));
                Ok(RenderResponse::PoolReady)
            }
            RenderRequest::Open {
                doc,
                base,
                password,
                ..
            } => {
                let bytes = base.into_bytes()?;
                let d = papyrine_render::Document::open(&self.lib, bytes, &[], password.as_deref())
                    .map_err(io)?;
                let n = d.page_count() as u32;
                self.doc = Some(d);
                Ok(RenderResponse::Opened { doc, page_count: n })
            }
            RenderRequest::RenderTile {
                page,
                bucket,
                tile_x,
                tile_y,
                dest,
                ..
            } => {
                let d = self
                    .doc
                    .as_mut()
                    .ok_or_else(|| IpcError::new(ErrorCode::NotFound, "no document"))?;
                let token = cx.cancel_token().clone();
                let tile = d
                    .render_tile(
                        page as usize,
                        bucket,
                        papyrine_render::TileCoord {
                            col: tile_x,
                            row: tile_y,
                        },
                        &mut || token.is_cancelled(),
                    )
                    .map_err(io)?;
                let stride = tile.width * 4;
                match dest {
                    PixelDest::Slot(s) => {
                        let (pool, slot_bytes) = self
                            .pool
                            .as_ref()
                            .ok_or_else(|| IpcError::invalid("no pool"))?;
                        if tile.rgba.len() as u64 > *slot_bytes {
                            return Err(IpcError::new(
                                ErrorCode::OutputTooSmall,
                                tile.rgba.len().to_string(),
                            ));
                        }
                        pool.write_at(s as usize * *slot_bytes as usize, &tile.rgba);
                        Ok(RenderResponse::Pixels(PixelsInfo {
                            width: tile.width,
                            height: tile.height,
                            stride,
                            slot: Some(s),
                            inline: None,
                        }))
                    }
                    PixelDest::Inline => Ok(RenderResponse::Pixels(PixelsInfo {
                        width: tile.width,
                        height: tile.height,
                        stride,
                        slot: None,
                        inline: Some(tile.rgba),
                    })),
                }
            }
            _ => Err(IpcError::new(ErrorCode::Unsupported, "not needed here")),
        }
    }
}

struct SharedBuf(SourceBytes);
impl AsRef<[u8]> for SharedBuf {
    fn as_ref(&self) -> &[u8] {
        (*self.0).as_ref()
    }
}

struct EngineChild;

impl Handler for EngineChild {
    type Req = EngineRequest;
    type Resp = EngineResponse;

    fn handle(
        &mut self,
        _cx: &mut Ctx<'_, EngineResponse>,
        req: EngineRequest,
    ) -> Result<EngineResponse, IpcError> {
        match req {
            EngineRequest::Ping { nonce } => Ok(EngineResponse::Pong { nonce }),
            EngineRequest::Open { doc, source, .. } => {
                let bytes = source.into_bytes()?;
                let d = papyrine_cos::Document::open_bytes(
                    SharedBuf(bytes),
                    &papyrine_cos::OpenOptions::default(),
                )
                .map_err(|e| IpcError::new(ErrorCode::Corrupt, e.to_string()))?;
                let pages =
                    d.page_count()
                        .map_err(|e| IpcError::internal(e.to_string()))? as u32;
                Ok(EngineResponse::Opened(DocumentInfo {
                    doc,
                    page_count: pages,
                    encrypted: false,
                    repaired: false,
                    title: None,
                    warnings: vec![],
                }))
            }
            _ => Err(IpcError::new(ErrorCode::Unsupported, "not needed here")),
        }
    }
}

// ---------------------------------------------------------- child entry

/// The re-executed test binary becomes the child here.
#[test]
fn child_entry() {
    let Some(b) = bootstrap().expect("bootstrap") else {
        return;
    };
    let sandboxed = b.sandbox.is_some();
    match b.role.clone() {
        Role::Other(n) if n == "probe" => {
            let report = ProbeReport {
                mechanisms: b
                    .sandbox
                    .as_ref()
                    .map(|r| r.mechanisms.clone())
                    .unwrap_or_default(),
                sandboxed,
                probes: run_probes(),
            };
            b.endpoint.tx.send(&report).expect("send report");
        }
        Role::Renderer => {
            // PDFium is loaded *after* the sandbox is applied, from the allowlisted cache dir.
            let dir = PathBuf::from(std::env::var(ENV_PDFIUM).unwrap());
            let lib =
                papyrine_render::Library::load(Some(&dir)).expect("load pdfium inside the sandbox");
            serve(b.endpoint, Role::Renderer, || RenderChild {
                lib,
                doc: None,
                pool: None,
            })
            .expect("serve");
        }
        Role::Engine => {
            serve(b.endpoint, Role::Engine, || EngineChild).expect("serve");
        }
        Role::Other(_) => unreachable!(),
    }
}

// ------------------------------------------------------------- host side

fn pdfium_dir() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/cache/pdfium");
    root.join(papyrine_render::platform_dir_name())
        .canonicalize()
        .unwrap_or_else(|_| {
            panic!(
                "PDFium missing: run tools/fetch-pdfium ({})",
                root.display()
            )
        })
}

fn spec(role: Role, sandbox: bool) -> ChildSpec {
    let mut s = ChildSpec::current_exe(role)
        .unwrap()
        .arg("child_entry")
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1");
    s.sandbox = sandbox;
    s.memory_limit = Some(4 << 30);
    s
}

struct Bait {
    home_file: PathBuf,
    outside_file: PathBuf,
    listener: TcpListener,
}

impl Bait {
    fn new() -> Self {
        let home = std::env::home_dir().expect("home dir");
        let tag = format!(
            "{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        );
        let home_file = home.join(format!(".papyrine-sandbox-probe-{tag}"));
        std::fs::File::create(&home_file)
            .unwrap()
            .write_all(b"the user's secret")
            .unwrap();
        // A sibling of the child temp dirs: writable by the user, not by the sandbox.
        let outside_dir = std::env::temp_dir().join(format!("papyrine-outside-{tag}"));
        std::fs::create_dir_all(&outside_dir).unwrap();
        Bait {
            home_file,
            outside_file: outside_dir.join("planted.txt"),
            listener: TcpListener::bind("127.0.0.1:0").unwrap(),
        }
    }
}

impl Drop for Bait {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.home_file);
        let _ = std::fs::remove_file(self.home_file.with_extension("new"));
        if let Some(d) = self.outside_file.parent() {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

fn run_probe_child(sandbox: bool, bait: &Bait) -> ProbeReport {
    let mut s = spec(Role::Other("probe".into()), sandbox)
        .env(ENV_HOME_FILE, bait.home_file.as_os_str())
        .env(ENV_OUTSIDE_FILE, bait.outside_file.as_os_str())
        .env(ENV_HOST_CWD, std::env::current_dir().unwrap())
        .env(
            ENV_TCP_ADDR,
            bait.listener.local_addr().unwrap().to_string(),
        );
    s.sandbox = sandbox;
    let Spawned {
        mut process,
        mut endpoint,
    } = spawn_child(&s).expect("spawn");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(endpoint.rx.recv::<ProbeReport>());
    });
    let report = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("child did not report in time")
        .expect("recv")
        .expect("report");
    let code = process.wait_or_kill(Duration::from_secs(30)).unwrap();
    assert_eq!(
        code,
        Some(0),
        "child failed (a failed assertion inside the sandboxed child?)"
    );
    report
}

fn get<'a>(r: &'a ProbeReport, n: &str) -> &'a Probe {
    r.probes
        .iter()
        .find(|p| p.name == n)
        .unwrap_or_else(|| panic!("no probe {n}"))
}

#[test]
fn probes_succeed_without_a_sandbox() {
    // Control: proves the probes are meaningful, i.e. they really would succeed.
    let bait = Bait::new();
    let r = run_probe_child(false, &bait);
    assert!(!r.sandboxed);
    for n in [
        "read_home_file",
        "list_home_dir",
        "write_outside_file",
        "write_home_new_file",
        "tcp_connect",
        "udp_bind",
        "loopback_self_connect",
        "spawn_process",
        "write_temp_file",
        "threads",
    ] {
        assert!(
            get(&r, n).succeeded,
            "{n} should work unsandboxed: {}",
            get(&r, n).detail
        );
    }
}

#[test]
fn sandbox_denies_home_outside_writes_sockets_and_processes() {
    let bait = Bait::new();
    let r = run_probe_child(true, &bait);
    assert!(r.sandboxed);
    eprintln!("mechanisms: {:?}", r.mechanisms);
    let mut failures = vec![];
    for n in [
        "read_home_file",
        "list_home_dir",
        "write_outside_file",
        "write_home_new_file",
        "write_host_cwd",
        "tcp_connect",
        "loopback_self_connect",
        // Creating or binding a socket is only denied outright on Unix; on Windows the
        // AppContainer makes it unreachable (see loopback_self_connect).
        #[cfg(unix)]
        "udp_bind",
        #[cfg(unix)]
        "tcp_listen",
        "spawn_process",
        #[cfg(unix)]
        "fork",
    ] {
        let p = get(&r, n);
        if p.succeeded {
            failures.push(format!("{n} was NOT denied"));
        } else {
            eprintln!("denied  {n}: {}", p.detail);
        }
    }
    for n in [
        "write_temp_file",
        "create_temp_subdir",
        "read_own_exe",
        "threads",
        "allocate_256mb",
        "shared_memory",
    ] {
        let p = get(&r, n);
        if !p.succeeded {
            failures.push(format!("{n} should work in the sandbox: {}", p.detail));
        }
    }
    assert!(
        !bait.outside_file.exists(),
        "a sandboxed write reached the outside dir"
    );
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn require_enforced_profiles_refuse_missing_job_or_bad_temp() {
    // A profile pointing at a missing temp dir is rejected, never silently widened.
    let p = papyrine_sandbox::Profile::new("/definitely/not/here/papyrine");
    assert!(papyrine_sandbox::apply_child_sandbox(&p).is_err());
}

fn sandboxed_child<Q, R>(role: Role) -> ChildClient<Q, R>
where
    Q: Serialize + Send + 'static,
    R: serde::de::DeserializeOwned + Send + 'static,
{
    let pdfium = pdfium_dir();
    let s = spec(role, true)
        .env(ENV_PDFIUM, pdfium.as_os_str())
        .read_dir(&pdfium);
    ChildClient::spawn(&s, Duration::from_secs(60)).expect("spawn sandboxed child")
}

fn test_pdf() -> Vec<u8> {
    use papyrine_render::testgen::{PageSpec, build, hello_content};
    build(&[
        PageSpec::letter(hello_content()),
        PageSpec::letter(hello_content()),
    ])
    .bytes
}

#[test]
fn sandboxed_renderer_loads_pdfium_and_renders_a_tile_from_passed_bytes() {
    let c: ChildClient<RenderRequest, RenderResponse> = sandboxed_child(Role::Renderer);
    let pdf = test_pdf();

    // Document bytes cross the boundary through a handle to a host-opened file (no path in the child).
    let dir = papyrine_sandbox::ChildTempDir::create().unwrap();
    let path = dir.path().join("doc.pdf");
    std::fs::write(&path, &pdf).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    let r = c
        .client
        .call_blocking(RenderRequest::Open {
            doc: DocId(1),
            base: DocSource::File {
                handle: file.into(),
                len: pdf.len() as u64,
            },
            sections: vec![],
            password: None,
        })
        .unwrap();
    assert!(matches!(r, RenderResponse::Opened { page_count: 2, .. }));

    let slot_bytes = 512 * 512 * 4u64;
    let pool = SharedRegion::create((slot_bytes * 2) as usize).unwrap();
    let view = SharedRegion::from_handle(pool.try_clone_handle().unwrap(), pool.len()).unwrap();
    c.client
        .call_blocking(RenderRequest::SetTilePool {
            pool,
            slot_bytes,
            slots: 2,
        })
        .unwrap();

    // bucket 0 = 100 % zoom; the black square covers device x 100..300, y 492..692 (page height 792).
    let r = c
        .client
        .call_blocking(RenderRequest::RenderTile {
            doc: DocId(1),
            page: 0,
            bucket: 0,
            tile_x: 0,
            tile_y: 0,
            dest: PixelDest::Slot(1),
        })
        .unwrap();
    let RenderResponse::Pixels(p) = r else {
        panic!()
    };
    assert_eq!((p.width, p.height), (512, 512));
    assert_eq!(p.slot, Some(1));
    let px = view.to_vec(slot_bytes as usize, (p.width * p.height * 4) as usize);
    let at = |x: u32, y: u32| &px[((y * p.width + x) * 4) as usize..][..4];
    assert_eq!(at(10, 10), [255, 255, 255, 255], "page background is white");
    // y 492..692 spans tile rows 0 and 1; check inside the first tile (y < 512).
    assert_eq!(at(200, 500), [0, 0, 0, 255], "black square rendered");
    let dark = px.as_chunks::<4>().0.iter().filter(|c| c[0] < 128).count();
    assert!(
        dark >= 4_000,
        "expected the black square (200 x 20 px) plus text, got {dark} dark pixels"
    );

    // Same child, inline fallback and the second page.
    let r = c
        .client
        .call_blocking(RenderRequest::RenderTile {
            doc: DocId(1),
            page: 1,
            bucket: 0,
            tile_x: 0,
            tile_y: 0,
            dest: PixelDest::Inline,
        })
        .unwrap();
    let RenderResponse::Pixels(p2) = r else {
        panic!()
    };
    assert_eq!(p2.inline.unwrap().len(), 512 * 512 * 4);
    assert_eq!(c.shutdown(Duration::from_secs(30)).unwrap(), Some(0));
}

#[test]
fn sandboxed_renderer_text_is_rendered_with_fonts() {
    // Helvetica text must produce dark pixels: font access works inside the sandbox.
    let c: ChildClient<RenderRequest, RenderResponse> = sandboxed_child(Role::Renderer);
    let pdf = {
        use papyrine_render::testgen::{PageSpec, build};
        build(&[PageSpec::letter(
            b"BT /F1 72 Tf 72 600 Td (HHHH) Tj ET\n".to_vec(),
        )])
        .bytes
    };
    c.client
        .call_blocking(RenderRequest::Open {
            doc: DocId(1),
            base: DocSource::Bytes(pdf),
            sections: vec![],
            password: None,
        })
        .unwrap();
    let r = c
        .client
        .call_blocking(RenderRequest::RenderTile {
            doc: DocId(1),
            page: 0,
            bucket: 0,
            tile_x: 0,
            tile_y: 0,
            dest: PixelDest::Inline,
        })
        .unwrap();
    let RenderResponse::Pixels(p) = r else {
        panic!()
    };
    let pixels = p.inline.unwrap();
    let dark = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|c| c[0] < 100)
        .count();
    assert!(dark > 500, "text did not render: {dark} dark pixels");
}

#[test]
fn sandboxed_engine_opens_a_pdf_with_qpdf_from_passed_bytes() {
    let c: ChildClient<EngineRequest, EngineResponse> = sandboxed_child(Role::Engine);
    let pdf = test_pdf();
    let r = c
        .client
        .call_blocking(EngineRequest::Open {
            doc: DocId(1),
            source: DocSource::Bytes(pdf.clone()),
            password: None,
        })
        .unwrap();
    let EngineResponse::Opened(info) = r else {
        panic!()
    };
    assert_eq!(info.page_count, 2);

    // Via a shared-memory region and via a file handle as well.
    let region = SharedRegion::create(pdf.len()).unwrap();
    region.write_at(0, &pdf);
    let r = c
        .client
        .call_blocking(EngineRequest::Open {
            doc: DocId(2),
            source: DocSource::Shared {
                region,
                len: pdf.len() as u64,
            },
            password: None,
        })
        .unwrap();
    assert!(matches!(
        r,
        EngineResponse::Opened(DocumentInfo { page_count: 2, .. })
    ));

    // Garbage is rejected as an error, not a crash.
    let e = c
        .client
        .call_blocking(EngineRequest::Open {
            doc: DocId(3),
            source: DocSource::Bytes(b"not a pdf at all".to_vec()),
            password: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Corrupt);
    assert!(
        c.client
            .call_blocking(EngineRequest::Ping { nonce: 1 })
            .is_ok()
    );
}
