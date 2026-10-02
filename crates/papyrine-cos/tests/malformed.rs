mod common;

use common::*;
use papyrine_cos::*;

fn open(data: Vec<u8>) -> Result<Document> {
    Document::open_bytes(data, &OpenOptions::default())
}

/// Walk everything reachable; must never panic whatever the input.
fn exercise(doc: &Document) {
    let _ = doc.repair_log();
    let _ = doc.object_count();
    let _ = doc.pdf_version();
    if let Ok(n) = doc.page_count() {
        for i in 0..n.min(20) {
            if let Ok(p) = doc.page(i) {
                let _ = p.unparse_with(false);
                if let Ok(c) = p.dict_get("Contents")
                    && c.kind().ok() == Some(ObjectKind::Stream)
                {
                    let _ = c.stream_decoded(DecodeLevel::Generalized);
                }
            }
        }
    }
    if let Ok(ids) = doc.object_ids() {
        for id in ids.into_iter().take(200) {
            if let Ok(o) = doc.object(id) {
                let _ = doc.fingerprint(&o);
            }
        }
    }
    let _ = doc.write(&WriteOptions {
        object_streams: ObjectStreams::Generate,
        ..Default::default()
    });
}

fn assert_repaired(data: Vec<u8>, pages: usize, expect: &[&str]) -> RepairLog {
    let doc = open(data).unwrap_or_else(|e| panic!("should recover: {e}"));
    let log = doc.repair_log();
    assert!(!log.is_empty(), "expected logged repairs");
    for needle in expect {
        assert!(
            log.mentions(needle),
            "log lacks {needle:?}: {:#?}",
            log.entries()
        );
    }
    assert_eq!(doc.page_count().unwrap(), pages);
    exercise(&doc);
    // The repaired document writes out a clean file.
    let out = doc.write(&WriteOptions::default()).unwrap().into_vec();
    let clean = open(out).unwrap();
    assert!(clean.repair_log().is_empty(), "{:?}", clean.repair_log());
    assert_eq!(clean.page_count().unwrap(), pages);
    log
}

#[test]
fn broken_startxref_offset() {
    let good = build_pdf(3);
    let sx = rfind(&good, b"startxref\n").unwrap() + "startxref\n".len();
    let end = sx + find(&good[sx..], b"\n").unwrap();
    let mut bad = good[..sx].to_vec();
    bad.extend_from_slice(b"999999");
    bad.extend_from_slice(&good[end..]);
    let log = assert_repaired(bad, 3, &["xref"]);
    assert!(
        log.entries()
            .iter()
            .any(|e| e.offset.is_some() || e.object.is_some() || !e.locator.is_empty())
    );
}

#[test]
fn xref_table_garbage() {
    let good = build_pdf(2);
    let bad = replace_first(&good, b"0000000015 00000 n", b"00000xxxxx 00000 n");
    // Whatever offset we broke, reading either works or repairs; it must not panic.
    if let Ok(doc) = open(bad) {
        exercise(&doc);
        assert_eq!(doc.page_count().unwrap(), 2);
    }
    let bad = replace_first(&good, b"xref\n", b"xrff\n");
    assert_repaired(bad, 2, &[]);
}

#[test]
fn missing_xref_and_trailer_keyword() {
    let good = build_pdf(2);
    let cut = find(&good, b"xref\n").unwrap();
    let mut bad = good[..cut].to_vec();
    bad.extend_from_slice(b"%%EOF\n");
    // No trailer at all: qpdf reconstructs one from the catalog or gives up with Damaged.
    match open(bad) {
        Ok(doc) => {
            assert!(!doc.repair_log().is_empty());
            exercise(&doc);
        }
        Err(Error::Damaged { .. }) => {}
        Err(e) => panic!("unexpected error {e:?}"),
    }
}

#[test]
fn missing_endobj() {
    let good = build_pdf(2);
    // Remove the endobj of the Pages object.
    let p = find(&good, b"/Type /Pages").unwrap();
    let e = p + find(&good[p..], b"endobj\n").unwrap();
    let mut bad = good[..e].to_vec();
    bad.extend_from_slice(&good[e + "endobj\n".len()..]);
    let doc = open(bad).expect("open");
    // qpdf tolerates this; if it noticed, it logged; the pages must still be readable.
    assert_eq!(doc.page_count().unwrap(), 2);
    exercise(&doc);
}

#[test]
fn bad_stream_length() {
    let good = build_pdf(2);
    let c = content_for(1);
    let from = format!("/Length {}", c.len());
    for wrong in ["3", "9999", "-5", "(x)"] {
        let bad = replace_first(
            &good,
            from.as_bytes(),
            format!("/Length {wrong}").as_bytes(),
        );
        let doc = open(bad).unwrap_or_else(|e| panic!("/Length {wrong}: {e}"));
        let data = decoded_content(&doc, 0);
        assert_eq!(
            data,
            c.clone().into_bytes(),
            "/Length {wrong}: stream recovered"
        );
        let log = doc.repair_log();
        assert!(
            log.mentions("length") || log.mentions("endstream"),
            "/Length {wrong}: {:?}",
            log.entries()
        );
        exercise(&doc);
    }
}

#[test]
fn indirect_length_pointing_nowhere() {
    let good = build_pdf(1);
    let c = content_for(1);
    let from = format!("<< /Length {} >>", c.len());
    let bad = replace_first(&good, from.as_bytes(), b"<< /Length 99 0 R >>");
    let doc = open(bad).expect("open");
    assert_eq!(decoded_content(&doc, 0), c.into_bytes());
}

#[test]
fn truncated_files_never_panic() {
    let good = build_pdf(4);
    let mut recovered = 0;
    let mut failed = 0;
    for cut in (0..good.len())
        .step_by(7)
        .chain([good.len() - 1, good.len() - 2])
    {
        match open(good[..cut].to_vec()) {
            Ok(doc) => {
                recovered += 1;
                exercise(&doc);
            }
            Err(Error::Damaged { .. })
            | Err(Error::Unsupported(_))
            | Err(Error::Object(_))
            | Err(Error::Pages(_)) => failed += 1,
            Err(e) => panic!("cut {cut}: unexpected error {e:?}"),
        }
    }
    assert!(
        recovered > 0 && failed > 0,
        "recovered {recovered}, failed {failed}"
    );
}

#[test]
fn truncated_mid_stream_recovers_prefix_with_log() {
    let good = build_pdf(3);
    let cut = find(&good, b"(Page 3)").unwrap() + 3;
    let doc = match open(good[..cut].to_vec()) {
        Ok(d) => d,
        Err(Error::Damaged { repairs, .. }) => {
            assert!(
                !repairs.is_empty(),
                "damaged error should carry the repair log"
            );
            return;
        }
        Err(e) => panic!("{e:?}"),
    };
    assert!(!doc.repair_log().is_empty());
    exercise(&doc);
}

#[test]
fn garbage_and_tiny_inputs_give_typed_errors() {
    for data in [
        Vec::new(),
        b"%PDF-1.4".to_vec(),
        b"not a pdf at all".to_vec(),
        vec![0u8; 4096],
        b"%PDF-1.7\n1 0 obj\n<<\nendobj\ntrailer\n<< >>\nstartxref\n0\n%%EOF".to_vec(),
        b"%PDF-1.7\nstartxref\n12345678901234567890\n%%EOF".to_vec(),
    ] {
        match open(data.clone()) {
            Ok(doc) => exercise(&doc),
            Err(
                Error::Damaged { .. } | Error::Unsupported(_) | Error::Object(_) | Error::Pages(_),
            ) => {}
            Err(e) => panic!(
                "{:?}: unexpected {e:?}",
                String::from_utf8_lossy(&data[..data.len().min(20)])
            ),
        }
    }
}

#[test]
fn no_recovery_option_fails_with_damaged() {
    let good = build_pdf(2);
    let sx = rfind(&good, b"startxref\n").unwrap() + "startxref\n".len();
    let mut bad = good[..sx].to_vec();
    bad.extend_from_slice(b"7\n%%EOF\n");
    let opts = OpenOptions {
        attempt_recovery: false,
        ..Default::default()
    };
    match Document::open_bytes(bad, &opts) {
        Err(Error::Damaged { .. }) => {}
        other => panic!("expected Damaged, got {:?}", other.err()),
    }
}

#[test]
fn deterministic_byte_mutations_never_panic() {
    let good = build_pdf(3);
    let mut rng = Lcg(0x5eed_1234);
    let (mut ok, mut err) = (0, 0);
    for _ in 0..400 {
        let mut d = good.clone();
        for _ in 0..1 + rng.below(8) {
            let i = rng.below(d.len());
            match rng.below(4) {
                0 => d[i] = rng.next() as u8,
                1 => d[i] ^= 1 << rng.below(8),
                2 => {
                    d.remove(i);
                }
                _ => d.insert(i, rng.next() as u8),
            }
        }
        match open(d) {
            Ok(doc) => {
                ok += 1;
                exercise(&doc);
            }
            Err(_) => err += 1,
        }
    }
    assert!(ok > 50, "ok {ok} err {err}");
}

#[test]
fn encrypted_and_mutated_never_panics() {
    let enc = encrypted_pdf(EncryptionSpec::new(EncryptionRevision::R6, "u", "o"));
    let mut rng = Lcg(42);
    for _ in 0..100 {
        let mut d = enc.clone();
        for _ in 0..1 + rng.below(4) {
            let i = rng.below(d.len());
            d[i] = rng.next() as u8;
        }
        if let Ok(doc) = Document::open_bytes(d, &OpenOptions::with_password("u")) {
            exercise(&doc);
        }
    }
}

#[test]
fn hostile_structures_do_not_hang_or_crash() {
    // Page tree loop and reference cycles.
    let cyc = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 3 >>\n%%EOF\n"
        .to_vec();
    if let Ok(doc) = open(cyc) {
        let _ = doc.page_count();
        exercise(&doc);
    }
    // Deeply nested arrays.
    let mut deep = b"%PDF-1.4\n1 0 obj\n".to_vec();
    deep.extend(std::iter::repeat_n(b'[', 200_000));
    deep.extend_from_slice(b"\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    if let Ok(doc) = open(deep) {
        exercise(&doc);
    }
    // Self-referential /Length.
    let bad = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n\
3 0 obj\n<< /Length 3 0 R >>\nstream\nabc\nendstream\nendobj\n\
trailer\n<< /Root 1 0 R /Size 4 >>\n%%EOF\n"
        .to_vec();
    if let Ok(doc) = open(bad) {
        exercise(&doc);
        if let Ok(o) = doc.object(ObjId::new(3, 0)) {
            let _ = o.stream_decoded(DecodeLevel::Generalized);
        }
    }
}
