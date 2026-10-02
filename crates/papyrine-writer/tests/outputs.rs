mod common;

use common::*;
use papyrine_cos::{EncryptionRevision, ObjId, ObjectStreams, WriteOptions};
use papyrine_writer::*;

fn variants() -> Vec<(&'static str, ObjectStreams, Option<EncryptionRevision>)> {
    use EncryptionRevision::*;
    vec![
        ("table", ObjectStreams::Disable, None),
        ("objstm", ObjectStreams::Generate, None),
        ("rc4-128", ObjectStreams::Disable, Some(R3)),
        (
            "aes-128-objstm",
            ObjectStreams::Generate,
            Some(R4 { aes: true }),
        ),
        ("aes-256-r6", ObjectStreams::Disable, Some(R6)),
    ]
}

#[test]
fn full_write_preserves_every_object_id() {
    for (name, streams, enc) in variants() {
        let pw = enc.map(|_| USER);
        let original = make_pdf(4, streams, enc);
        let doc = open(original, pw);
        for xref in [XrefKind::Table, XrefKind::Stream] {
            let mut out = Vec::new();
            let report = write_full(
                &doc,
                &mut out,
                &FullWriteOptions {
                    xref,
                    ..FullWriteOptions::default()
                },
                &|| false,
            )
            .unwrap();
            assert_eq!(report.bytes, out.len() as u64);
            assert!(
                !String::from_utf8_lossy(&out).contains("/Prev"),
                "{name}: no /Prev in a full write"
            );
            let chain = ChainState::scan(out.as_slice()).unwrap();
            assert_eq!((chain.kind, chain.sections), (xref, 1), "{name}");

            let re = open(out.clone(), pw);
            assert!(re.repair_log().is_empty(), "{name}: {:?}", re.repair_log());
            assert_eq!(re.page_count().unwrap(), 4);
            // Every non-container object is present under the same id with the same content.
            let mem = all_fingerprints(&doc);
            for (id, fp) in &mem {
                let t = String::from_utf8_lossy(&fp.repr);
                if fp.is_stream && (t.contains("/Type /ObjStm") || t.contains("/Type /XRef")) {
                    continue;
                }
                let disk = re.fingerprint(&re.object(*id).unwrap()).unwrap();
                assert_eq!(
                    normalized(fp.clone()),
                    normalized(disk),
                    "{name}/{xref:?}: {id}"
                );
            }
            let dir = tempfile::tempdir().unwrap();
            let p = write_tmp(dir.path(), "full.pdf", &out);
            let (code, text) = qpdf_check(&p, pw);
            assert_eq!(code, 0, "{name}/{xref:?}\n{text}");
            // IDs are preserved: /ID is untouched.
            let a = doc.trailer().unwrap().dict_get("ID").unwrap();
            let b = re.trailer().unwrap().dict_get("ID").unwrap();
            for i in 0..2 {
                assert_eq!(
                    a.array_get(i).unwrap().string().unwrap(),
                    b.array_get(i).unwrap().string().unwrap(),
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn full_write_after_edits_then_incremental_on_top() {
    // Snapshot compaction: ID-preserving base, then sections for later commits.
    let original = make_pdf(3, ObjectStreams::Generate, Some(EncryptionRevision::R6));
    let doc = open(original, Some(USER));
    let mut base = Vec::new();
    write_full(&doc, &mut base, &FullWriteOptions::default(), &|| false).unwrap();
    let doc2 = open(base.clone(), Some(USER));
    let info = doc2.trailer().unwrap().dict_get("Info").unwrap();
    info.dict_set("Title", &doc2.new_string("after compaction").unwrap())
        .unwrap();
    let (out, _) = incremental(&doc2, &base, &[info.id().unwrap()], &[]);
    let re = assert_parity(&doc2, out, Some(USER), &[info.id().unwrap()]);
    assert_eq!(
        re.trailer()
            .unwrap()
            .dict_get("Info")
            .unwrap()
            .dict_get("Title")
            .unwrap()
            .string()
            .unwrap(),
        b"after compaction"
    );
}

#[test]
fn cancellation_stops_a_full_write() {
    let doc = open(make_pdf(5, ObjectStreams::Disable, None), None);
    let mut out = Vec::new();
    let r = write_full(&doc, &mut out, &FullWriteOptions::default(), &|| true);
    assert!(matches!(r, Err(Error::Cancelled)));
}

/// Build a hybrid-reference file by hand: a classic table whose trailer has /XRefStm pointing at
/// an xref stream that describes a page living in an object stream.
fn hybrid_pdf() -> Vec<u8> {
    let mut o = b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offs = [0usize; 7];
    let mut add = |o: &mut Vec<u8>, n: usize, body: &str| {
        offs[n] = o.len();
        o.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    };
    add(&mut o, 1, "<< /Type /Catalog /Pages 2 0 R >>");
    add(&mut o, 2, "<< /Type /Pages /Count 1 /Kids [3 0 R] >>");
    let inner = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << >> >>";
    let head = "3 0 ";
    let objstm = format!("{head}{inner}");
    add(
        &mut o,
        5,
        &format!(
            "<< /Type /ObjStm /N 1 /First {} /Length {} >>\nstream\n{objstm}\nendstream",
            head.len(),
            objstm.len()
        ),
    );
    // Xref stream (object 6): entries for objects 3 (compressed in 5, index 0), 5 and 6.
    let x6 = o.len();
    let row = |t: u8, a: u32, b: u16| {
        let mut r = vec![t];
        r.extend_from_slice(&a.to_be_bytes());
        r.extend_from_slice(&b.to_be_bytes());
        r
    };
    let mut data = row(2, 5, 0);
    data.extend(row(1, offs[5] as u32, 0));
    data.extend(row(1, x6 as u32, 0));
    let mut obj = format!(
        "6 0 obj\n<< /Type /XRef /Size 7 /W [1 4 2] /Index [3 1 5 2] /Length {} >>\nstream\n",
        data.len()
    )
    .into_bytes();
    obj.extend_from_slice(&data);
    obj.extend_from_slice(b"\nendstream\nendobj\n");
    o.extend_from_slice(&obj);
    let xref = o.len();
    // Hidden (compressed) objects are simply absent from the table; the XRefStm covers them.
    o.extend_from_slice(b"xref\n0 3\n0000000000 65535 f \n");
    for off in [offs[1], offs[2]] {
        o.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    o.extend_from_slice(format!("5 2\n{:010} 00000 n \n{x6:010} 00000 n \n", offs[5]).as_bytes());
    o.extend_from_slice(
        format!("trailer\n<< /Size 7 /Root 1 0 R /XRefStm {x6} /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>] >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    o
}

#[test]
fn hybrid_reference_file_is_extended_with_a_table_section() {
    let original = hybrid_pdf();
    let doc = open(original.clone(), None);
    assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
    assert_eq!(doc.page_count().unwrap(), 1);
    let chain = ChainState::scan(original.as_slice()).unwrap();
    assert!(chain.hybrid);
    assert_eq!(chain.kind, XrefKind::Table);

    let pages = doc.root().unwrap().dict_get("Pages").unwrap();
    pages
        .dict_set("Marker", &doc.new_string("hybrid edit").unwrap())
        .unwrap();
    // Edit the compressed page too: it must move to a top-level object.
    let page = doc.page(0).unwrap();
    page.dict_set("Rotate", &doc.new_int(90)).unwrap();
    let dirty = [pages.id().unwrap(), page.id().unwrap()];
    let (out, s) = incremental(&doc, &original, &dirty, &[]);
    assert_eq!(s.chain.kind, XrefKind::Table);
    let re = assert_parity(&doc, out.clone(), None, &dirty);
    assert_eq!(re.page_count().unwrap(), 1);
    // Editing only the pages node leaves the compressed page reachable through the old XRefStm.
    let doc = open(original.clone(), None);
    let pages = doc.root().unwrap().dict_get("Pages").unwrap();
    pages.dict_set("Marker", &doc.new_int(1)).unwrap();
    let (out, _) = incremental(&doc, &original, &[pages.id().unwrap()], &[]);
    let re = open(out.clone(), None);
    assert!(re.repair_log().is_empty());
    assert_eq!(
        re.page(0)
            .unwrap()
            .dict_get("MediaBox")
            .unwrap()
            .array_len()
            .unwrap(),
        4
    );
    let dir = tempfile::tempdir().unwrap();
    let p = write_tmp(dir.path(), "hybrid.pdf", &out);
    let (code, text) = qpdf_check(&p, None);
    assert_eq!(code, 0, "{text}");
}

#[test]
fn optimized_rewrite_returns_a_usable_renumbering() {
    let dir = tempfile::tempdir().unwrap();
    let original = make_pdf(6, ObjectStreams::Disable, None);
    let doc = open(original, None);
    let target = dir.path().join("opt.pdf");
    let report = save_optimized(
        &doc,
        &target,
        &WriteOptions {
            object_streams: papyrine_cos::ObjectStreams::Generate,
            ..WriteOptions::default()
        },
        SaveOptions::default(),
    )
    .unwrap();
    assert_eq!(report.kind, SaveKind::Optimized);
    let map = report.renumbering.unwrap();
    assert!(!map.is_empty());
    let re =
        papyrine_cos::Document::open_path(&target, &papyrine_cos::OpenOptions::default()).unwrap();
    assert_eq!(re.page_count().unwrap(), 6);
    // Each page maps to the page at the same index in the rewritten file.
    for (i, page) in doc.pages().unwrap().iter().enumerate() {
        let new = map[&page.id().unwrap()];
        assert_eq!(re.page(i).unwrap().id().unwrap(), new);
    }
    let info_new = map[&doc
        .trailer()
        .unwrap()
        .dict_get("Info")
        .unwrap()
        .id()
        .unwrap()];
    let t = re
        .object(info_new)
        .unwrap()
        .dict_get("Title")
        .unwrap()
        .string()
        .unwrap();
    assert_eq!(t, b"Original title");
    assert_eq!(qpdf_check(&target, None).0, 0);
    let _: Option<ObjId> = None;
}
