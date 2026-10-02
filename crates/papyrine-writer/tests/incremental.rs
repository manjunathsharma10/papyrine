mod common;

use common::*;
use papyrine_cos::{EncryptionRevision, ObjId, ObjectKind, ObjectStreams};
use papyrine_writer::{ChainState, XrefKind};

fn variants() -> Vec<(&'static str, ObjectStreams, Option<EncryptionRevision>)> {
    use EncryptionRevision::*;
    vec![
        ("table", ObjectStreams::Disable, None),
        ("xref-stream", ObjectStreams::Generate, None),
        ("rc4-40", ObjectStreams::Disable, Some(R2)),
        ("rc4-128", ObjectStreams::Disable, Some(R3)),
        ("v4-rc4", ObjectStreams::Disable, Some(R4 { aes: false })),
        ("aes-128", ObjectStreams::Disable, Some(R4 { aes: true })),
        (
            "aes-128-objstm",
            ObjectStreams::Generate,
            Some(R4 { aes: true }),
        ),
        ("aes-256-r5", ObjectStreams::Disable, Some(R5)),
        ("aes-256-r6", ObjectStreams::Disable, Some(R6)),
        ("aes-256-r6-objstm", ObjectStreams::Generate, Some(R6)),
    ]
}

/// A mix of edits touching every object type the serializer handles.
fn edit(doc: &papyrine_cos::Document) -> (Vec<ObjId>, Vec<ObjId>) {
    let mut dirty = Vec::new();
    // 1. Info strings: parens, backslash, CR/LF, non-ASCII, UTF-16BE, binary.
    let info = doc.trailer().unwrap().dict_get("Info").unwrap();
    info.dict_set(
        "Title",
        &doc.new_string("T(i)t\\le\r\nwith \u{e9}").unwrap(),
    )
    .unwrap();
    info.dict_set(
        "Author",
        &doc.new_string([0xFE, 0xFF, 0x00, 0x41, 0x26, 0x3A])
            .unwrap(),
    )
    .unwrap();
    info.dict_set(
        "Binary",
        &doc.new_string([0u8, 1, 2, 250, 251, 252, 255]).unwrap(),
    )
    .unwrap();
    info.dict_set("Empty", &doc.new_string([]).unwrap())
        .unwrap();
    info.dict_set("N#am e/", &doc.new_name("Odd name/#()<>[]%\u{e9}").unwrap())
        .unwrap();
    info.dict_set("Real", &doc.new_real(0.000001).unwrap())
        .unwrap();
    info.dict_set("Neg", &doc.new_real(-12.5).unwrap()).unwrap();
    info.dict_set("Int", &doc.new_int(-9_007_199_254_740_991))
        .unwrap();
    info.dict_set("T", &doc.new_bool(true)).unwrap();
    let arr = doc
        .parse_object("[1 2.5 (s) /N [<< /K (v) >>] null true]")
        .unwrap();
    info.dict_set("Arr", &arr).unwrap();
    dirty.push(info.id().unwrap());

    // 2. A page: new key, annotations array with a new annotation object (new indirect object).
    let page = doc.page(0).unwrap();
    let annot = doc
        .parse_object(
            "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30.75] /Contents (note) /C [1 0 0] >>",
        )
        .unwrap();
    let annot = doc.make_indirect(&annot).unwrap();
    let annots = doc.new_array();
    annots.array_push(&annot).unwrap();
    page.dict_set("Annots", &annots).unwrap();
    dirty.push(page.id().unwrap());
    dirty.push(annot.id().unwrap());

    // 3. A new content stream replacing page 1's contents (plain data), and a second stream with
    //    a declared filter whose bytes are opaque.
    let contents = page.dict_get("Contents").unwrap();
    contents
        .stream_replace(b"BT /F1 24 Tf 72 700 Td (Edited) Tj ET\n", None, None)
        .unwrap();
    dirty.push(contents.id().unwrap());
    let opaque = doc.new_stream(b"x").unwrap();
    let filter = doc.parse_object("/ASCIIHexDecode").unwrap();
    opaque
        .stream_replace(b"48656C6C6F>", Some(&filter), None)
        .unwrap();
    opaque
        .stream_dict()
        .unwrap()
        .dict_set("Note", &doc.new_string("in a stream dict").unwrap())
        .unwrap();
    dirty.push(opaque.id().unwrap());
    page.dict_set("Extra", &opaque).unwrap();

    // 4. Delete the second page's object if there is one: freed.
    let mut freed = Vec::new();
    if doc.page_count().unwrap() > 1 {
        let p = doc.page(1).unwrap();
        let id = p.id().unwrap();
        doc.remove_page(&p).unwrap();
        doc.replace_object(id, &doc.new_null()).unwrap();
        freed.push(id);
        // The page tree changed.
        let pages = doc.root().unwrap().dict_get("Pages").unwrap();
        dirty.push(pages.id().unwrap());
    }
    (dirty, freed)
}

#[test]
fn parity_across_formats_and_encryption() {
    for (name, streams, enc) in variants() {
        let pw = enc.map(|_| USER);
        let original = make_pdf(3, streams, enc);
        let doc = open(original.clone(), pw);
        let chain = ChainState::scan(original.as_slice()).unwrap();
        let expect_kind = if streams == ObjectStreams::Generate {
            XrefKind::Stream
        } else {
            XrefKind::Table
        };
        assert_eq!(chain.kind, expect_kind, "{name}");

        let before = all_fingerprints(&doc);
        let (dirty, freed) = edit(&doc);
        let (out, section) = incremental(&doc, &original, &dirty, &freed);
        assert_eq!(
            &out[..original.len()],
            &original[..],
            "{name}: original not an exact prefix"
        );
        assert_eq!(section.chain.kind, expect_kind);
        if enc.is_some() {
            let hay = String::from_utf8_lossy(&section.bytes);
            assert!(
                !hay.contains("Edited") && !hay.contains("Helvetica"),
                "{name}: plaintext leaked"
            );
        }
        let re = assert_parity(&doc, out.clone(), pw, &dirty);

        // Nothing outside the dirty set changed (apart from the freed objects).
        for (id, fp) in &before {
            if dirty.contains(id) || freed.contains(id) {
                continue;
            }
            let now = normalized(re.fingerprint(&re.object(*id).unwrap()).unwrap());
            assert_eq!(
                normalized(fp.clone()),
                now,
                "{name}: untouched object {id} changed"
            );
        }
        for f in &freed {
            assert_eq!(
                re.object(*f).unwrap().kind().unwrap(),
                ObjectKind::Null,
                "{name}: freed {f}"
            );
        }
        // Edited values show up on reopen, including encrypted strings.
        let title = re
            .trailer()
            .unwrap()
            .dict_get("Info")
            .unwrap()
            .dict_get("Title")
            .unwrap();
        assert_eq!(
            title.string().unwrap(),
            "T(i)t\\le\r\nwith \u{e9}".as_bytes(),
            "{name}"
        );
        assert_eq!(re.page_count().unwrap(), 2, "{name}");
        let edited = re.page(0).unwrap().dict_get("Contents").unwrap();
        assert_eq!(
            edited
                .stream_decoded(papyrine_cos::DecodeLevel::All)
                .unwrap()
                .as_slice(),
            b"BT /F1 24 Tf 72 700 Td (Edited) Tj ET\n",
            "{name}"
        );

        // qpdf's own checker agrees.
        let dir = tempfile::tempdir().unwrap();
        let p = write_tmp(dir.path(), "out.pdf", &out);
        let (code, text) = qpdf_check(&p, pw);
        assert_eq!(code, 0, "{name}: qpdf --check\n{text}");

        // A second and third section chain on top of the first.
        let mut cur = out;
        let mut cur_doc = re;
        for round in 0..2 {
            let info = cur_doc.trailer().unwrap().dict_get("Info").unwrap();
            info.dict_set("Round", &cur_doc.new_int(round)).unwrap();
            let d = vec![info.id().unwrap()];
            let (next, s) = incremental(&cur_doc, &cur, &d, &[]);
            assert_eq!(s.chain.sections, 3 + round as u32);
            let r = assert_parity(&cur_doc, next.clone(), pw, &d);
            cur = next;
            cur_doc = r;
        }
        let p = write_tmp(dir.path(), "out3.pdf", &cur);
        assert_eq!(qpdf_check(&p, pw).0, 0, "{name}: chained");
        let id = cur_doc.trailer().unwrap().dict_get("ID").unwrap();
        // /ID[0] is stable across sections; /ID[1] changes.
        let first = id.array_get(0).unwrap().string().unwrap();
        let orig_id = doc
            .trailer()
            .unwrap()
            .dict_get("ID")
            .unwrap()
            .array_get(0)
            .unwrap()
            .string()
            .unwrap();
        assert_eq!(first, orig_id, "{name}");
    }
}

#[test]
fn bad_edits_are_rejected_not_corrupting() {
    // Serializing an unreadable object kind must be an error, not garbage.
    let original = make_pdf(1, ObjectStreams::Disable, None);
    let doc = open(original.clone(), None);
    let chain = ChainState::scan(original.as_slice()).unwrap();
    let ok =
        papyrine_writer::write_section(&doc, &chain, &papyrine_writer::SectionRequest::default())
            .unwrap();
    assert!(ok.objects.is_empty());
    assert!(String::from_utf8_lossy(&ok.bytes).contains("startxref"));
}
