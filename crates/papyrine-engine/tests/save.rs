mod common;

use common::*;
use papyrine_cos::{Document, OpenOptions};
use papyrine_engine::proto::*;
use papyrine_ipc::{DocId, DocSource};
use serde_json::json;

fn ready(r: Response) -> SavePayload {
    match r {
        Response::SaveReady(p) => *p,
        r => panic!("expected SaveReady, got {r:?}"),
    }
}

fn committed(e: &mut Eng, doc: u64, token: u64, saved: Option<Vec<u8>>) -> SaveFinished {
    match e.call(Request::SaveCommitted {
        doc: DocId(doc),
        token,
        saved: saved.map(DocSource::Bytes),
    }) {
        Response::SaveFinished(f) => f,
        r => panic!("unexpected {r:?}"),
    }
}

fn page_rotations(bytes: &[u8]) -> Vec<u16> {
    let owned: Vec<u8> = bytes.into();
    let m = papyrine_model::Model::open_bytes(owned, &OpenOptions::default()).unwrap();
    let n = m.page_count().unwrap();
    (0..n).map(|i| m.page(i).unwrap().rotate).collect()
}

#[test]
fn incremental_saves_append_only_what_changed() {
    let mut e = Eng::new();
    let orig = build_pdf(4);
    e.open(1, orig.clone());
    e.exec(1, "rotate_pages", json!({"pages": [0], "delta": 90}));
    e.exec(
        1,
        "set_info_field",
        json!({"field": "Title", "value": "Saved"}),
    );
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    assert_eq!(p.kind, SaveKindDto::Incremental);
    assert!(!p.unchanged && !p.history_will_reset);
    assert_eq!(p.base_len, orig.len() as u64);
    let add1 = read_delivery(&p.bytes);
    assert_eq!(p.total_len, (orig.len() + add1.len()) as u64);
    let mut file1 = orig.clone();
    file1.extend_from_slice(&add1);
    assert_eq!(
        &file1[..orig.len()],
        &orig[..],
        "original bytes stay an exact prefix"
    );
    if have_qpdf() {
        qpdf_check(&file1, e.dir.path()).unwrap();
    }
    assert_eq!(page_rotations(&file1), vec![90, 0, 0, 0]);
    let f = committed(&mut e, 1, p.token, None);
    assert!(!f.history_reset);

    // Nothing changed: nothing to write.
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    assert!(p.unchanged);
    assert_eq!(p.base_len, file1.len() as u64);
    committed(&mut e, 1, p.token, None);

    // Only the new change goes into the second section, chained onto file1.
    e.exec(1, "rotate_pages", json!({"pages": [2], "delta": 180}));
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    assert_eq!(p.base_len, file1.len() as u64);
    let add2 = read_delivery(&p.bytes);
    assert!(
        add2.len() < add1.len() + 200,
        "second section should be small"
    );
    let mut file2 = file1.clone();
    file2.extend_from_slice(&add2);
    if have_qpdf() {
        qpdf_check(&file2, e.dir.path()).unwrap();
    }
    assert_eq!(page_rotations(&file2), vec![90, 0, 180, 0]);
    committed(&mut e, 1, p.token, None);

    // A change made while a save was in flight is not marked saved.
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    assert!(p.unchanged);
    e.exec(1, "rotate_pages", json!({"pages": [3], "delta": 90}));
    committed(&mut e, 1, p.token, None);
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    assert!(!p.unchanged, "the later change must still be dirty");
    let mut file3 = file2.clone();
    file3.extend_from_slice(&read_delivery(&p.bytes));
    assert_eq!(page_rotations(&file3), vec![90, 0, 180, 90]);
}

#[test]
fn undone_creations_are_not_saved() {
    let mut e = Eng::new();
    let orig = build_pdf(2);
    e.open(1, orig.clone());
    e.exec(
        1,
        "insert_blank_page",
        json!({"at": 1, "width": 100, "height": 100}),
    );
    e.undo(1);
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    let add = read_delivery(&p.bytes);
    let mut f = orig;
    f.extend_from_slice(&add);
    let d = Document::open_bytes(f, &OpenOptions::default()).unwrap();
    assert_eq!(d.page_count().unwrap(), 2);
    assert!(
        !String::from_utf8_lossy(&add).contains("/Type /Page\n"),
        "the undone page object should not be written"
    );
}

#[test]
fn repaired_file_saves_as_an_optimized_rewrite_and_resets_history() {
    // Break the xref: point startxref at garbage so qpdf must rebuild it.
    let orig = damage(build_pdf(3));
    let mut e = Eng::new();
    let s = e.open(1, orig.clone());
    assert!(s.repaired, "{:?}", s.repair_log);
    let RenderBase::Repaired(f) = &s.render_base else {
        panic!("repaired base expected")
    };
    assert!(f.len > 0 && std::fs::metadata(&f.path).is_ok());
    let mut m = Mirror::from_open(orig.clone(), &s);
    for d in [90, 180] {
        let c = e.exec(1, "rotate_pages", json!({"pages": [1], "delta": d}));
        m.apply(&c.snapshot, None);
    }
    assert_snapshot_matches(&mut e, 1, &m);
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    assert_eq!(p.kind, SaveKindDto::Optimized);
    assert!(p.history_will_reset);
    let saved = read_delivery(&p.bytes);
    if have_qpdf() {
        qpdf_check(&saved, e.dir.path()).unwrap();
    }
    assert_eq!(page_rotations(&saved), vec![0, 270, 0]);
    let fin = committed(&mut e, 1, p.token, Some(saved.clone()));
    assert!(fin.history_reset && fin.notice.is_some());
    assert!(matches!(fin.snapshot.action, SnapshotAction::NewBase(None)));
    m.apply(&fin.snapshot, Some(&saved));
    // The engine now works on the saved file: history is empty, edits and saves continue.
    assert!(
        e.try_call(Request::Undo {
            doc: DocId(1),
            out_section: None
        })
        .is_err()
    );
    let c = e.exec(1, "rotate_pages", json!({"pages": [0], "delta": 90}));
    m.apply(&c.snapshot, Some(&saved));
    assert_snapshot_matches(&mut e, 1, &m);
    let u = e.undo(1);
    m.apply(&u.snapshot, Some(&saved));
    assert_snapshot_matches(&mut e, 1, &m);
    // Now a normal incremental save over the optimized file.
    e.exec(1, "rotate_pages", json!({"pages": [2], "delta": 90}));
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    assert_eq!(p.kind, SaveKindDto::Incremental);
    assert_eq!(p.base_len, saved.len() as u64);
}

fn rfind(h: &[u8], n: &[u8]) -> usize {
    h.windows(n.len()).rposition(|w| w == n).unwrap()
}

/// Break the xref pointer so qpdf has to rebuild the table.
fn damage(mut pdf: Vec<u8>) -> Vec<u8> {
    let at = rfind(&pdf, b"startxref\n");
    pdf.truncate(at);
    pdf.extend_from_slice(b"startxref\n7\n%%EOF\n");
    pdf
}

/// A PDF with a (fake) signature dictionary, added as an incremental update: enough for the
/// save policy, which looks for `/ByteRange` + `/Contents`.
fn signed_pdf() -> Vec<u8> {
    let mut out = build_pdf(2);
    let prev_at = rfind(&out, b"startxref\n") + b"startxref\n".len();
    let prev: usize = String::from_utf8_lossy(&out[prev_at..])
        .lines()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let o1 = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Sig 8 0 R >>\nendobj\n");
    let o8 = out.len();
    out.extend_from_slice(
        b"8 0 obj\n<< /Type /Sig /Filter /Adobe.PPKLite /ByteRange [0 10 20 30] /Contents <00> >>\nendobj\n",
    );
    let x = out.len();
    out.extend_from_slice(
        format!(
            "xref\n0 1\n0000000000 65535 f \n1 1\n{o1:010} 00000 n \n8 1\n{o8:010} 00000 n \ntrailer\n<< /Size 9 /Root 1 0 R /Info 3 0 R /Prev {prev} /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>] >>\nstartxref\n{x}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

#[test]
fn signed_file_incremental_save_keeps_the_original_prefix() {
    let orig = signed_pdf();
    let mut e = Eng::new();
    let s = e.open(1, orig.clone());
    assert!(!s.repaired, "{:?}", s.repair_log);
    e.exec(1, "rotate_pages", json!({"pages": [0], "delta": 90}));
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    assert_eq!(p.kind, SaveKindDto::Incremental);
    let add = read_delivery(&p.bytes);
    let mut f = orig.clone();
    f.extend_from_slice(&add);
    assert_eq!(&f[..orig.len()], &orig[..]);
    // An optimized rewrite of a signed file needs the user's say-so.
    match e.save(
        1,
        SaveMode::Optimized {
            break_signatures: false,
        },
    ) {
        Response::SaveDecision(SaveDecision::SignedRewrite) => {}
        r => panic!("unexpected {r:?}"),
    }
    let p = ready(e.save(
        1,
        SaveMode::Optimized {
            break_signatures: true,
        },
    ));
    assert_eq!(p.kind, SaveKindDto::Optimized);
}

#[test]
fn signed_and_damaged_needs_a_decision() {
    let orig = damage(signed_pdf());
    let mut e = Eng::new();
    let sum = e.open(1, orig);
    assert!(sum.repaired && sum.signatures.signed);
    match e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ) {
        Response::SaveDecision(SaveDecision::SignedAndDamaged { reason }) => {
            assert!(reason.contains("rebuilt"));
        }
        r => panic!("unexpected {r:?}"),
    }
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: true,
        },
    ));
    assert_eq!(p.kind, SaveKindDto::Optimized);
}

#[test]
fn stale_save_tokens_are_refused() {
    let mut e = Eng::new();
    e.open(1, build_pdf(1));
    e.exec(1, "rotate_pages", json!({"pages": [0], "delta": 90}));
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    assert!(
        e.try_call(Request::SaveCommitted {
            doc: DocId(1),
            token: p.token + 5,
            saved: None
        })
        .is_err()
    );
    // A new Save supersedes the previous token.
    let p2 = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    assert!(
        e.try_call(Request::SaveCommitted {
            doc: DocId(1),
            token: p.token,
            saved: None
        })
        .is_err()
    );
    committed(&mut e, 1, p2.token, None);
}

#[test]
fn many_saves_raise_the_optimize_suggestion() {
    let mut e = Eng::new();
    e.open(1, build_pdf(2));
    let mut suggested = false;
    for i in 0..25 {
        e.exec(1, "rotate_pages", json!({"pages": [i % 2], "delta": 90}));
        let p = ready(e.save(
            1,
            SaveMode::Policy {
                break_signatures: false,
            },
        ));
        suggested |= p.suggest_optimize.is_some();
        committed(&mut e, 1, p.token, None);
    }
    assert!(
        suggested,
        "more than 20 revisions should suggest Save optimized"
    );
}

#[test]
fn encrypted_documents_edit_snapshot_and_save() {
    let Some(orig) = corpus("enc-aes-256-r6-user.pdf") else {
        eprintln!("corpus not generated; skipping");
        return;
    };
    let mut e = Eng::new();
    let err = e.try_open(1, orig.clone(), None).unwrap_err();
    assert_eq!(err.code, papyrine_ipc::ErrorCode::Encrypted);
    let err = e.try_open(1, orig.clone(), Some("wrong")).unwrap_err();
    assert_eq!(err.code, papyrine_ipc::ErrorCode::Encrypted);
    let s = e.try_open(1, orig.clone(), Some("user")).unwrap();
    assert!(s.security.encrypted && s.security.algorithm.is_some());
    let mut m = Mirror::from_open(orig.clone(), &s);
    m.password = Some("user".into());
    let c = e.exec(1, "rotate_pages", json!({"pages": [0], "delta": 90}));
    m.apply(&c.snapshot, None);
    assert_snapshot_matches(&mut e, 1, &m);
    let p = ready(e.save(
        1,
        SaveMode::Policy {
            break_signatures: false,
        },
    ));
    let mut f = orig.clone();
    f.extend_from_slice(&read_delivery(&p.bytes));
    let d = Document::open_bytes(f, &OpenOptions::with_password("user")).unwrap();
    assert_eq!(d.page_count().unwrap() as u32, s.page_count);
    // And an optimized rewrite keeps the encryption and reopens with the password.
    let p = ready(e.save(
        1,
        SaveMode::Optimized {
            break_signatures: false,
        },
    ));
    let saved = read_delivery(&p.bytes);
    assert!(Document::open_bytes(saved.clone(), &OpenOptions::default()).is_err());
    let fin = committed(&mut e, 1, p.token, Some(saved));
    assert!(fin.history_reset);
}
