//! Protocol and transport tests, including round trips through a real child
//! process (this test binary re-executed with a role in the environment).

use papyrine_ipc::*;
use std::io::Write;
use std::time::Duration;

// ------------------------------------------------------------ child side

struct EngineStub;

fn checksum(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf29ce484222325u64, |h, &x| {
        (h ^ x as u64).wrapping_mul(0x100000001b3)
    })
}

impl Handler for EngineStub {
    type Req = EngineRequest;
    type Resp = EngineResponse;

    fn handle(
        &mut self,
        cx: &mut Ctx<'_, EngineResponse>,
        req: EngineRequest,
    ) -> Result<EngineResponse, IpcError> {
        match req {
            EngineRequest::Ping { nonce } => Ok(EngineResponse::Pong { nonce }),
            EngineRequest::Open { doc, source, .. } => {
                let bytes = source.into_bytes()?;
                let b: &[u8] = (*bytes).as_ref();
                Ok(EngineResponse::Opened(DocumentInfo {
                    doc,
                    page_count: b.len() as u32,
                    encrypted: false,
                    repaired: false,
                    title: Some(format!("{:016x}", checksum(b))),
                    warnings: vec![],
                }))
            }
            EngineRequest::Execute {
                command,
                out_section,
                blobs,
                ..
            } => {
                match command.name.as_str() {
                    "panic" => panic!("boom"),
                    "exit" => std::process::exit(7),
                    _ => {}
                }
                let section = out_section.map(|r| {
                    r.write_at(0, command.params_json.as_bytes());
                    SectionInfo {
                        generation: 1,
                        len: command.params_json.len() as u64,
                    }
                });
                Ok(EngineResponse::Executed {
                    summary: ChangeSetSummary {
                        label: command.name.clone(),
                        objects_changed: blobs.len() as u32,
                        ..Default::default()
                    },
                    after_images: vec![AfterImage {
                        obj: 4,
                        generation: 0,
                        body: Some(command.params_json.into_bytes()),
                    }],
                    section,
                })
            }
            EngineRequest::Search { query, .. } => {
                let mut total = 0;
                for i in 0..200u64 {
                    cx.check_cancelled()?;
                    std::thread::sleep(Duration::from_millis(10));
                    cx.progress(i + 1, 200, "searching");
                    if i % 10 == 9 {
                        cx.partial(EngineResponse::Hits {
                            hits: vec![SearchHit {
                                page: i as u32,
                                quads: vec![],
                                context: query.text.clone(),
                            }],
                            complete: false,
                        });
                        total += 1;
                    }
                }
                Ok(EngineResponse::Hits {
                    hits: vec![SearchHit {
                        page: total,
                        quads: vec![],
                        context: "done".into(),
                    }],
                    complete: true,
                })
            }
            EngineRequest::Write { out, .. } => {
                let need = 1000usize;
                if out.len() < need {
                    return Err(IpcError::new(ErrorCode::OutputTooSmall, need.to_string()));
                }
                out.write_at(0, &[0xAB; 1000]);
                Ok(EngineResponse::Written { len: need as u64 })
            }
            _ => Err(IpcError::new(ErrorCode::Unsupported, "stub")),
        }
    }
}

struct RendererStub {
    pool: Option<(SharedRegion, u64)>,
}

impl Handler for RendererStub {
    type Req = RenderRequest;
    type Resp = RenderResponse;

    fn handle(
        &mut self,
        _cx: &mut Ctx<'_, RenderResponse>,
        req: RenderRequest,
    ) -> Result<RenderResponse, IpcError> {
        match req {
            RenderRequest::Ping { nonce } => Ok(RenderResponse::Pong { nonce }),
            RenderRequest::SetTilePool {
                pool, slot_bytes, ..
            } => {
                self.pool = Some((pool, slot_bytes));
                Ok(RenderResponse::PoolReady)
            }
            RenderRequest::RenderTile {
                tile_x,
                tile_y,
                dest,
                ..
            } => {
                let (w, h) = (64u32, 32u32);
                let mut px = vec![0u8; (w * h * 4) as usize];
                for i in 0..(w * h) as usize {
                    px[i * 4..i * 4 + 4].copy_from_slice(&[
                        (i % 64) as u8,
                        tile_x as u8,
                        tile_y as u8,
                        255,
                    ]);
                }
                match dest {
                    PixelDest::Slot(s) => {
                        let (pool, slot_bytes) = self
                            .pool
                            .as_ref()
                            .ok_or_else(|| IpcError::invalid("no pool"))?;
                        pool.write_at(s as usize * *slot_bytes as usize, &px);
                        Ok(RenderResponse::Pixels(PixelsInfo {
                            width: w,
                            height: h,
                            stride: w * 4,
                            slot: Some(s),
                            inline: None,
                        }))
                    }
                    PixelDest::Inline => Ok(RenderResponse::Pixels(PixelsInfo {
                        width: w,
                        height: h,
                        stride: w * 4,
                        slot: None,
                        inline: Some(px),
                    })),
                }
            }
            _ => Err(IpcError::new(ErrorCode::Unsupported, "stub")),
        }
    }
}

/// The re-executed test binary becomes the child here.
#[test]
fn child_entry() {
    let Some(b) = bootstrap().expect("bootstrap") else {
        return;
    };
    match b.role {
        Role::Engine => {
            let exit = serve(b.endpoint, Role::Engine, || EngineStub).expect("serve");
            assert_eq!(exit, ServeExit::Shutdown);
        }
        Role::Renderer => {
            serve(b.endpoint, Role::Renderer, || RendererStub { pool: None }).expect("serve");
        }
        Role::Other(_) => unreachable!(),
    }
}

// ------------------------------------------------------------- host side

fn spawn<Q, R>(role: Role) -> ChildClient<Q, R>
where
    Q: serde::Serialize + Send + 'static,
    R: serde::de::DeserializeOwned + Send + 'static,
{
    let spec = ChildSpec::current_exe(role)
        .unwrap()
        .arg("child_entry")
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .unsandboxed();
    ChildClient::spawn(&spec, Duration::from_secs(30)).expect("spawn child")
}

fn engine() -> ChildClient<EngineRequest, EngineResponse> {
    spawn(Role::Engine)
}

#[test]
fn negotiation() {
    assert_eq!(negotiate((1, 3), (2, 5)), Some(3));
    assert_eq!(negotiate((1, 1), (2, 5)), None);
    assert_eq!(negotiate((2, 5), (1, 2)), Some(2));
}

#[test]
fn in_process_frames_carry_handles_and_shm() {
    let (a, mut b) = Endpoint::pair().unwrap();
    let dir = std::env::temp_dir().join(format!("papyrine-ipc-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.bin");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(b"hello handle")
        .unwrap();
    let region = SharedRegion::create(4096).unwrap();
    region.write_at(10, b"pixels");
    let msg = EngineRequest::Open {
        doc: DocId(9),
        source: DocSource::File {
            handle: Handle::from_file(std::fs::File::open(&path).unwrap()),
            len: 12,
        },
        password: Some("pw".into()),
    };
    a.tx.send(&msg).unwrap();
    a.tx.send(&EngineRequest::Write {
        doc: DocId(1),
        out: region,
        options_json: "{}".into(),
    })
    .unwrap();

    match b.rx.recv::<EngineRequest>().unwrap().unwrap() {
        EngineRequest::Open {
            doc,
            source,
            password,
        } => {
            assert_eq!(doc, DocId(9));
            assert_eq!(password.as_deref(), Some("pw"));
            let bytes = source.into_bytes().unwrap();
            assert_eq!((*bytes).as_ref(), b"hello handle");
        }
        other => panic!("{other:?}"),
    }
    match b.rx.recv::<EngineRequest>().unwrap().unwrap() {
        EngineRequest::Write { out, .. } => {
            assert_eq!(out.to_vec(10, 6), b"pixels");
            out.write_at(0, b"back");
        }
        other => panic!("{other:?}"),
    }
    drop(a);
    assert!(b.rx.recv::<EngineRequest>().unwrap().is_none(), "clean EOF");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn large_inline_message() {
    let (a, mut b) = Endpoint::pair().unwrap();
    let data: Vec<u8> = (0..8_000_000u32).map(|i| (i % 251) as u8).collect();
    let expect = checksum(&data);
    let t = std::thread::spawn(move || {
        a.tx.send(&EngineRequest::Open {
            doc: DocId(1),
            source: DocSource::Bytes(data),
            password: None,
        })
        .unwrap();
        a
    });
    match b.rx.recv::<EngineRequest>().unwrap().unwrap() {
        EngineRequest::Open {
            source: DocSource::Bytes(v),
            ..
        } => assert_eq!(checksum(&v), expect),
        other => panic!("{other:?}"),
    }
    drop(t.join().unwrap());
}

#[test]
fn oversized_frames_are_rejected() {
    let (a, _b) = Endpoint::pair().unwrap();
    let huge = vec![0u8; transport::MAX_FRAME_BYTES + 1];
    let e = a.tx.send(&EngineRequest::Open {
        doc: DocId(1),
        source: DocSource::Bytes(huge),
        password: None,
    });
    assert!(matches!(e, Err(TransportError::TooLarge(_))));
}

#[test]
fn child_ping_and_handshake() {
    let c = engine();
    assert_eq!(c.client.peer.role, Role::Engine);
    assert_eq!(c.client.version, PROTOCOL_VERSION);
    for nonce in 0..50 {
        match c
            .client
            .call_blocking(EngineRequest::Ping { nonce })
            .unwrap()
        {
            EngineResponse::Pong { nonce: n } => assert_eq!(n, nonce),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(c.shutdown(Duration::from_secs(10)).unwrap(), Some(0));
}

#[test]
fn child_opens_document_through_a_passed_handle() {
    let c = engine();
    let dir = papyrine_sandbox::ChildTempDir::create().unwrap();
    let path = dir.path().join("big.bin");
    let data: Vec<u8> = (0..3_000_000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    std::fs::write(&path, &data).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    let r = c
        .client
        .call_blocking(EngineRequest::Open {
            doc: DocId(1),
            source: DocSource::File {
                handle: file.into(),
                len: data.len() as u64,
            },
            password: None,
        })
        .unwrap();
    let EngineResponse::Opened(info) = r else {
        panic!()
    };
    assert_eq!(info.page_count, data.len() as u32);
    assert_eq!(info.title.unwrap(), format!("{:016x}", checksum(&data)));

    // Shared-memory and inline fallbacks give the same answer.
    let region = SharedRegion::create(data.len()).unwrap();
    region.write_at(0, &data);
    let r = c
        .client
        .call_blocking(EngineRequest::Open {
            doc: DocId(2),
            source: DocSource::Shared {
                region,
                len: data.len() as u64,
            },
            password: None,
        })
        .unwrap();
    let EngineResponse::Opened(info2) = r else {
        panic!()
    };
    assert_eq!(info2.title.unwrap(), format!("{:016x}", checksum(&data)));
}

#[test]
fn execute_returns_changeset_after_images_and_section() {
    let c = engine();
    let out = SharedRegion::create(1 << 16).unwrap();
    let probe = SharedRegion::from_handle(out.try_clone_handle().unwrap(), out.len()).unwrap();
    let params = r#"{"page":3,"deg":90}"#;
    let r = c
        .client
        .call_blocking(EngineRequest::Execute {
            doc: DocId(1),
            command: CommandRequest {
                name: "page.rotate".into(),
                params_json: params.into(),
            },
            blobs: vec![BlobRef::Inline(vec![1, 2, 3])],
            out_section: Some(out),
        })
        .unwrap();
    let EngineResponse::Executed {
        summary,
        after_images,
        section,
    } = r
    else {
        panic!()
    };
    assert_eq!(summary.label, "page.rotate");
    assert_eq!(summary.objects_changed, 1);
    assert_eq!(after_images[0].body.as_deref(), Some(params.as_bytes()));
    let section = section.unwrap();
    assert_eq!(probe.to_vec(0, section.len as usize), params.as_bytes());
}

#[test]
fn output_too_small_is_a_typed_error() {
    let c = engine();
    let small = SharedRegion::create(10).unwrap();
    let e = c
        .client
        .call_blocking(EngineRequest::Write {
            doc: DocId(1),
            out: small,
            options_json: "{}".into(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::OutputTooSmall);
    let big = SharedRegion::create(2000).unwrap();
    let probe = SharedRegion::from_handle(big.try_clone_handle().unwrap(), 2000).unwrap();
    let r = c
        .client
        .call_blocking(EngineRequest::Write {
            doc: DocId(1),
            out: big,
            options_json: "{}".into(),
        })
        .unwrap();
    assert!(matches!(r, EngineResponse::Written { len: 1000 }));
    assert_eq!(probe.to_vec(0, 4), [0xAB; 4]);
}

#[test]
fn job_reports_progress_partials_and_completes() {
    let c = engine();
    let job = c
        .client
        .start_job(EngineRequest::Search {
            doc: DocId(1),
            query: SearchQuery {
                text: "needle".into(),
                case_sensitive: false,
                whole_word: false,
                max_hits: 100,
            },
        })
        .unwrap();
    let last = job.progress.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(last.job, job.id);
    assert_eq!(last.total, 200);
    let r = job.wait().unwrap();
    let EngineResponse::Hits { hits, complete } = r else {
        panic!()
    };
    assert!(complete);
    assert_eq!(hits[0].page, 20);
}

#[test]
fn cancel_stops_a_job_quickly() {
    let c = engine();
    let job = c
        .client
        .start_job(EngineRequest::Search {
            doc: DocId(1),
            query: SearchQuery {
                text: "x".into(),
                case_sensitive: false,
                whole_word: false,
                max_hits: 1,
            },
        })
        .unwrap();
    job.progress.recv_timeout(Duration::from_secs(10)).unwrap();
    let t = std::time::Instant::now();
    job.cancel();
    let e = job.wait().unwrap_err();
    assert!(e.is_cancelled(), "{e:?}");
    // The full job takes ~2 s; cancellation must land within a few polls.
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "took {:?}",
        t.elapsed()
    );
    // The child is still healthy afterwards.
    assert!(matches!(
        c.client
            .call_blocking(EngineRequest::Ping { nonce: 5 })
            .unwrap(),
        EngineResponse::Pong { nonce: 5 }
    ));
}

#[test]
fn handler_panic_is_contained() {
    let c = engine();
    let cmd = |n: &str| EngineRequest::Execute {
        doc: DocId(1),
        command: CommandRequest {
            name: n.into(),
            params_json: "{}".into(),
        },
        blobs: vec![],
        out_section: None,
    };
    let e = c.client.call_blocking(cmd("panic")).unwrap_err();
    assert_eq!(e.code, ErrorCode::Internal);
    assert!(e.message.contains("boom"));
    assert!(
        c.client
            .call_blocking(EngineRequest::Ping { nonce: 1 })
            .is_ok()
    );
}

#[test]
fn child_death_fails_pending_and_later_calls() {
    let mut c = engine();
    let e = c
        .client
        .call_blocking(EngineRequest::Execute {
            doc: DocId(1),
            command: CommandRequest {
                name: "exit".into(),
                params_json: "{}".into(),
            },
            blobs: vec![],
            out_section: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::ChildCrashed);
    let e = c
        .client
        .call_blocking(EngineRequest::Ping { nonce: 1 })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::ChildCrashed);
    assert!(c.client.closed_reason().is_some());
    assert_eq!(c.process.wait().unwrap(), Some(7));
}

#[test]
fn renderer_tiles_via_shared_pool_and_inline() {
    let c: ChildClient<RenderRequest, RenderResponse> = spawn(Role::Renderer);
    let slot_bytes = 64 * 32 * 4u64;
    let pool = SharedRegion::create((slot_bytes * 4) as usize).unwrap();
    let view = SharedRegion::from_handle(pool.try_clone_handle().unwrap(), pool.len()).unwrap();
    let r = c
        .client
        .call_blocking(RenderRequest::SetTilePool {
            pool,
            slot_bytes,
            slots: 4,
        })
        .unwrap();
    assert!(matches!(r, RenderResponse::PoolReady));
    let r = c
        .client
        .call_blocking(RenderRequest::RenderTile {
            doc: DocId(1),
            page: 0,
            bucket: 0,
            tile_x: 7,
            tile_y: 9,
            dest: PixelDest::Slot(2),
        })
        .unwrap();
    let RenderResponse::Pixels(p) = r else {
        panic!()
    };
    assert_eq!((p.width, p.height, p.slot), (64, 32, Some(2)));
    let px = view.to_vec((2 * slot_bytes) as usize, 8);
    assert_eq!(px, [0, 7, 9, 255, 1, 7, 9, 255]);
    let r = c
        .client
        .call_blocking(RenderRequest::RenderTile {
            doc: DocId(1),
            page: 0,
            bucket: 0,
            tile_x: 1,
            tile_y: 2,
            dest: PixelDest::Inline,
        })
        .unwrap();
    let RenderResponse::Pixels(p) = r else {
        panic!()
    };
    assert_eq!(&p.inline.unwrap()[..4], [0, 1, 2, 255]);
}

#[test]
fn concurrent_callers_are_correlated_by_request_id() {
    let c = engine();
    let mut handles = vec![];
    for t in 0..8u64 {
        let cl = c.client.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..40u64 {
                let nonce = t * 1000 + i;
                match cl.call_blocking(EngineRequest::Ping { nonce }).unwrap() {
                    EngineResponse::Pong { nonce: n } => assert_eq!(n, nonce),
                    other => panic!("{other:?}"),
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}
