//! The renderer role in a real child process: handle passing, shared-memory
//! tiles and sections, the sandbox, and restart after a crash. The test binary
//! re-executes itself with a role in the environment (as the ipc tests do).

use papyrine_ipc::*;
use papyrine_render::role::run_renderer;
use papyrine_render::testgen::*;
use papyrine_render::{Library, mem};
use std::io::Write;
use std::time::Duration;

type CC = ChildClient<RenderRequest, RenderResponse>;

/// The re-executed test binary becomes the renderer here.
#[test]
fn child_entry() {
    if std::env::var(spawn::ENV_ROLE).is_err() {
        return;
    }
    mem::reexec_with_child_env().expect("re-exec with allocator env");
    let _ = Library::global(); // load PDFium before the sandbox closes the filesystem
    let Some(b) = bootstrap().expect("bootstrap") else {
        return;
    };
    assert_eq!(b.role, Role::Renderer);
    run_renderer(b.endpoint).expect("serve");
}

fn spawn_child(sandbox: bool) -> CC {
    let mut spec = ChildSpec::current_exe(Role::Renderer)
        .unwrap()
        .arg("child_entry")
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1");
    if !sandbox {
        spec = spec.unsandboxed();
    }
    CC::spawn(&spec, Duration::from_secs(60)).expect("spawn child")
}

fn write_tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("papyrine-render-proc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::File::create(&p).unwrap().write_all(bytes).unwrap();
    p
}

fn file_source(p: &std::path::Path) -> DocSource {
    DocSource::File {
        handle: Handle::from_file(std::fs::File::open(p).unwrap()),
        len: std::fs::metadata(p).unwrap().len(),
    }
}

fn first_tile(cc: &CC, pool_mirror: &SharedRegion, slot: u32) -> [u8; 4] {
    let r = cc
        .client
        .call_blocking(RenderRequest::RenderTile {
            doc: DocId(1),
            page: 0,
            bucket: 0,
            tile_x: 0,
            tile_y: 0,
            dest: PixelDest::Slot(slot),
        })
        .unwrap();
    let RenderResponse::Pixels(p) = r else {
        panic!("{r:?}")
    };
    assert_eq!((p.width, p.height), (512, 512));
    let off = slot as usize * (1 << 20) + (500 * 512 + 200) * 4;
    let v = pool_mirror.to_vec(off, 4);
    [v[0], v[1], v[2], v[3]]
}

fn run_scenario(sandbox: bool) {
    let built = build(&[PageSpec::letter(hello_content())]);
    let base = write_tmp("base.pdf", &built.bytes);
    let cc = spawn_child(sandbox);
    let pool = SharedRegion::create(4 << 20).unwrap();
    let mirror = SharedRegion::from_handle(pool.try_clone_handle().unwrap(), 4 << 20).unwrap();
    cc.client
        .call_blocking(RenderRequest::SetTilePool {
            pool,
            slot_bytes: 1 << 20,
            slots: 4,
        })
        .unwrap();
    let r = cc
        .client
        .call_blocking(RenderRequest::Open {
            doc: DocId(1),
            base: file_source(&base),
            sections: vec![],
            password: None,
        })
        .unwrap();
    assert!(matches!(r, RenderResponse::Opened { page_count: 1, .. }));
    assert_eq!(first_tile(&cc, &mirror, 0), [0, 0, 0, 255]);

    // An edit arrives as a section in shared memory; the tile changes.
    let mut chain = Chain::new(&built);
    let sec = chain.section(&[(built.content_ids[0], stream_body("", &red_content()))]);
    let region = SharedRegion::create(sec.len()).unwrap();
    region.write_at(0, &sec);
    let r = cc
        .client
        .call_blocking(RenderRequest::Reopen {
            doc: DocId(1),
            sections: vec![DocSource::Shared {
                region,
                len: sec.len() as u64,
            }],
        })
        .unwrap();
    assert!(matches!(r, RenderResponse::Opened { page_count: 1, .. }));
    assert_eq!(first_tile(&cc, &mirror, 1), [255, 0, 0, 255]);

    // Crash restart: kill the child, see ChildCrashed, start a new one, carry on.
    let ChildClient {
        client,
        mut process,
    } = cc;
    process.kill().unwrap();
    let e = client
        .call_blocking(RenderRequest::Ping { nonce: 1 })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::ChildCrashed);
    drop(process);

    let cc2 = spawn_child(sandbox);
    let pool = SharedRegion::create(4 << 20).unwrap();
    let mirror = SharedRegion::from_handle(pool.try_clone_handle().unwrap(), 4 << 20).unwrap();
    cc2.client
        .call_blocking(RenderRequest::SetTilePool {
            pool,
            slot_bytes: 1 << 20,
            slots: 4,
        })
        .unwrap();
    cc2.client
        .call_blocking(RenderRequest::Open {
            doc: DocId(1),
            base: file_source(&base),
            sections: vec![],
            password: None,
        })
        .unwrap();
    assert_eq!(first_tile(&cc2, &mirror, 0), [0, 0, 0, 255]);
    let code = cc2.shutdown(Duration::from_secs(10)).unwrap();
    assert_eq!(code, Some(0));
}

#[test]
fn child_process_unsandboxed() {
    run_scenario(false);
}

#[test]
fn child_process_sandboxed() {
    run_scenario(true);
}
