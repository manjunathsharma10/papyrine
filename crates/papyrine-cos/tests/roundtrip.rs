mod common;

use common::*;
use papyrine_cos::*;

fn open(data: Vec<u8>) -> Document {
    Document::open_bytes(data, &OpenOptions::default()).expect("open")
}

#[test]
fn open_and_read() {
    let doc = open(build_pdf(3));
    assert_eq!(doc.page_count().unwrap(), 3);
    assert_eq!(doc.pdf_version().unwrap(), "1.4");
    assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
    assert!(doc.encryption().unwrap().is_none());
    assert!(doc.encryption_key().unwrap().is_none());
    assert!(!doc.is_linearized().unwrap());

    let root = doc.root().unwrap();
    assert_eq!(root.kind().unwrap(), ObjectKind::Dictionary);
    assert_eq!(root.dict_get("Type").unwrap().name().unwrap(), b"Catalog");
    assert_eq!(
        doc.trailer().unwrap().dict_get("Root").unwrap().id(),
        Some(ObjId::new(1, 0))
    );
    assert_eq!(doc.object_count().unwrap(), 3 + 2 * 3);

    let page = doc.page(1).unwrap();
    assert_eq!(page.id(), Some(ObjId::new(6, 0)));
    let mb = page.dict_get("MediaBox").unwrap();
    let nums: Vec<f64> = mb
        .array_items()
        .unwrap()
        .iter()
        .map(|o| o.as_f64().unwrap())
        .collect();
    assert_eq!(nums, [0.0, 0.0, 612.0, 792.0]);
    assert_eq!(decoded_content(&doc, 1), content_for(2).into_bytes());
    assert_eq!(doc.find_page(&page).unwrap(), 1);

    let info = doc.trailer().unwrap().dict_get("Info").unwrap();
    assert_eq!(
        info.dict_get("Title").unwrap().string().unwrap(),
        b"Test document"
    );
    let mut keys = info.dict_keys().unwrap();
    keys.sort();
    assert_eq!(keys, [b"Producer".to_vec(), b"Title".to_vec()]);
    assert_eq!(
        doc.object(ObjId::new(3, 0)).unwrap().id(),
        Some(ObjId::new(3, 0))
    );
    let ids = doc.object_ids().unwrap();
    assert_eq!(ids.len(), 9);
}

#[test]
fn modify_write_reopen() {
    let doc = open(build_pdf(2));
    let page = doc.page(0).unwrap();
    page.dict_set("Rotate", &doc.new_int(90)).unwrap();
    page.dict_set("Marker", &doc.new_name("Hello").unwrap())
        .unwrap();
    page.dict_remove("MediaBox").unwrap();
    assert!(!page.dict_has("MediaBox").unwrap());
    let mb = doc.parse_object("[0 0 200 300.5]").unwrap();
    page.dict_set("MediaBox", &mb).unwrap();

    // Replace stream data (uncompressed), and add an annotation-like indirect object.
    let contents = page.dict_get("Contents").unwrap();
    contents
        .stream_replace(b"q 1 0 0 1 0 0 cm Q\n", None, None)
        .unwrap();
    let annot = doc
        .make_indirect(
            &doc.parse_object("<< /Type /Annot /Subtype /Text /Contents (note) >>")
                .unwrap(),
        )
        .unwrap();
    let annots = doc.new_array();
    annots.array_push(&annot).unwrap();
    page.dict_set("Annots", &annots).unwrap();

    let out = doc
        .write(&WriteOptions {
            static_id: true,
            ..Default::default()
        })
        .unwrap();
    assert!(out.bytes().starts_with(b"%PDF-"));
    assert!(!out.renumbering().is_empty());
    let again = open(out.into_vec());
    assert!(again.repair_log().is_empty(), "{:?}", again.repair_log());
    assert_eq!(again.page_count().unwrap(), 2);
    let p = again.page(0).unwrap();
    assert_eq!(p.dict_get("Rotate").unwrap().as_int().unwrap(), 90);
    assert_eq!(p.dict_get("Marker").unwrap().name().unwrap(), b"Hello");
    let mb: Vec<f64> = p
        .dict_get("MediaBox")
        .unwrap()
        .array_items()
        .unwrap()
        .iter()
        .map(|o| o.as_f64().unwrap())
        .collect();
    assert_eq!(mb, [0.0, 0.0, 200.0, 300.5]);
    assert_eq!(decoded_content(&again, 0), b"q 1 0 0 1 0 0 cm Q\n");
    let a = p.dict_get("Annots").unwrap().array_get(0).unwrap();
    assert_eq!(a.dict_get("Subtype").unwrap().name().unwrap(), b"Text");
    assert_eq!(a.dict_get("Contents").unwrap().string().unwrap(), b"note");
    // Second page untouched.
    assert_eq!(decoded_content(&again, 1), content_for(2).into_bytes());
}

#[test]
fn write_is_deterministic_with_static_id() {
    let opts = WriteOptions {
        static_id: true,
        ..Default::default()
    };
    let a = open(build_pdf(2)).write(&opts).unwrap().into_vec();
    let b = open(build_pdf(2)).write(&opts).unwrap().into_vec();
    assert_eq!(a, b);
}

#[test]
fn writer_modes() {
    let src = build_pdf(5);
    let base = WriteOptions {
        static_id: true,
        ..Default::default()
    };

    let streams = open(src.clone())
        .write(&WriteOptions {
            object_streams: ObjectStreams::Generate,
            ..base.clone()
        })
        .unwrap();
    assert!(find(streams.bytes(), b"/ObjStm").is_some());
    let d = open(streams.into_vec());
    assert_eq!(d.page_count().unwrap(), 5);
    assert!(d.pdf_version().unwrap().as_str() >= "1.5");

    let plain = open(src.clone())
        .write(&WriteOptions {
            object_streams: ObjectStreams::Disable,
            ..base.clone()
        })
        .unwrap();
    assert!(find(plain.bytes(), b"/ObjStm").is_none());

    let lin = open(src.clone())
        .write(&WriteOptions {
            linearize: true,
            ..base.clone()
        })
        .unwrap();
    assert!(find(lin.bytes(), b"/Linearized").is_some());
    let d = open(lin.into_vec());
    assert!(d.is_linearized().unwrap());
    assert_eq!(d.page_count().unwrap(), 5);

    let qdf = open(src.clone())
        .write(&WriteOptions {
            qdf: true,
            stream_data: StreamMode::Uncompress,
            ..base.clone()
        })
        .unwrap();
    assert!(find(qdf.bytes(), b"%QDF-1.0").is_some());

    let v = open(src)
        .write(&WriteOptions {
            min_version: Some("1.7".into()),
            ..base
        })
        .unwrap();
    assert!(v.bytes().starts_with(b"%PDF-1.7"));
}

#[test]
fn write_to_path_and_open_path() {
    let dir = std::env::temp_dir().join(format!("papyrine-cos-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.pdf");
    let doc = open(build_pdf(2));
    let w = doc.write_to_path(&path, &WriteOptions::default()).unwrap();
    assert!(w.bytes().is_empty());
    let again = Document::open_path(&path, &OpenOptions::default()).unwrap();
    assert_eq!(again.page_count().unwrap(), 2);
    let missing = Document::open_path(dir.join("nope.pdf"), &OpenOptions::default());
    assert!(matches!(missing, Err(Error::Io(_))), "{:?}", missing.err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn page_operations() {
    let doc = open(build_pdf(3));
    let first = doc.page(0).unwrap();
    doc.remove_page(&first).unwrap();
    assert_eq!(doc.page_count().unwrap(), 2);
    doc.add_page(&first, false).unwrap();
    assert_eq!(doc.page_count().unwrap(), 3);
    assert_eq!(doc.find_page(&first).unwrap(), 2);
    let second = doc.page(0).unwrap();
    doc.add_page_at(&second, false, &first).unwrap();
}

#[test]
fn foreign_copy_and_add_page_from_other_document() {
    let dst = Document::new_empty().unwrap();
    assert_eq!(dst.page_count().unwrap(), 0);
    let src = open(build_pdf(2));
    let p = src.page(1).unwrap();
    dst.add_page(&p, false).unwrap();
    drop(src); // objects were copied; keepalive guards the input source
    assert_eq!(dst.page_count().unwrap(), 1);
    let out = dst.write(&WriteOptions::default()).unwrap().into_vec();
    let again = open(out);
    assert_eq!(decoded_content(&again, 0), content_for(2).into_bytes());

    let other = open(build_pdf(1));
    let info = other.trailer().unwrap().dict_get("Info").unwrap();
    let copied = dst.copy_foreign(&info).unwrap();
    assert!(copied.is_indirect());
    assert_eq!(
        copied.dict_get("Title").unwrap().string().unwrap(),
        b"Test document"
    );
}

#[test]
fn new_objects_and_replace() {
    let doc = Document::new_empty().unwrap();
    let s = doc.new_stream(b"hello stream").unwrap();
    let ind = doc.make_indirect(&s).unwrap();
    let id = ind.id().expect("indirect");
    assert_eq!(
        doc.object(id).unwrap().stream_raw().unwrap().as_slice(),
        b"hello stream"
    );
    doc.replace_object(id, &doc.new_string(b"\xFF\x00bin").unwrap())
        .unwrap();
    assert_eq!(doc.object(id).unwrap().string().unwrap(), b"\xFF\x00bin");

    let f = doc.new_real(1.5).unwrap();
    assert_eq!(f.kind().unwrap(), ObjectKind::Real);
    assert_eq!(f.as_f64().unwrap(), 1.5);
    assert_eq!(f.real_text().unwrap(), "1.5");
    assert!(doc.new_bool(true).as_bool().unwrap());
    assert!(doc.new_null().is_null().unwrap());
}

#[test]
fn type_errors_are_typed_not_panics() {
    let doc = open(build_pdf(1));
    let name = doc.root().unwrap().dict_get("Type").unwrap();
    assert!(matches!(name.as_int(), Err(Error::Type(_))));
    assert!(matches!(name.array_len(), Err(Error::Type(_))));
    assert!(matches!(name.dict_get("x"), Err(Error::Type(_))));
    assert!(matches!(doc.page(7), Err(Error::Range(_))));
    let arr = doc.new_array();
    assert!(matches!(arr.array_get(0), Err(Error::Range(_))));
    assert!(doc.parse_object("<< /Unterminated").is_err());
}

#[test]
fn flate_stream_round_trip_and_raw_access() {
    let doc = open(build_pdf(1));
    let payload: Vec<u8> = (0..4000u32)
        .flat_map(|i| format!("{i} ").into_bytes())
        .collect();
    let page = doc.page(0).unwrap();
    let c = page.dict_get("Contents").unwrap();
    c.stream_replace(&payload, None, None).unwrap();
    let out = doc
        .write(&WriteOptions {
            stream_data: StreamMode::Compress,
            static_id: true,
            ..Default::default()
        })
        .unwrap()
        .into_vec();
    let again = open(out);
    let c = again.page(0).unwrap().dict_get("Contents").unwrap();
    let raw = c.stream_raw().unwrap();
    assert!(raw.as_slice().len() < payload.len(), "compressed");
    assert_eq!(
        c.stream_dict()
            .unwrap()
            .dict_get("Filter")
            .unwrap()
            .name()
            .unwrap(),
        b"FlateDecode"
    );
    assert_eq!(
        c.stream_decoded(DecodeLevel::Generalized)
            .unwrap()
            .as_slice(),
        payload.as_slice()
    );
}

#[test]
fn fingerprint_changes_with_content() {
    let doc = open(build_pdf(1));
    let c = doc.page(0).unwrap().dict_get("Contents").unwrap();
    let a = doc.fingerprint(&c).unwrap();
    assert!(a.is_stream);
    assert_eq!(a.stream_raw_len as usize, content_for(1).len());
    assert!(a.stream_sha256.is_some());
    assert_eq!(a, doc.fingerprint(&c).unwrap());
    c.stream_replace(b"different", None, None).unwrap();
    let b = doc.fingerprint(&c).unwrap();
    assert_ne!(a.stream_sha256, b.stream_sha256);
    let page = doc.page(0).unwrap();
    let pf = doc.fingerprint(&page).unwrap();
    assert!(!pf.is_stream && pf.stream_sha256.is_none());
    assert!(
        find(&pf.repr, b"/Type /Page").is_some(),
        "{}",
        String::from_utf8_lossy(&pf.repr)
    );
}

#[test]
fn renumbering_after_object_stream_rewrite() {
    let doc = open(build_pdf(2));
    // Delete a page so one page and its content become unreferenced and the rest renumber.
    let first = doc.page(0).unwrap();
    doc.remove_page(&first).unwrap();
    let out = doc
        .write(&WriteOptions {
            object_streams: ObjectStreams::Disable,
            ..Default::default()
        })
        .unwrap();
    let map = out.renumbering();
    assert!(map.contains_key(&ObjId::new(1, 0)));
    assert!(
        map.get(&ObjId::new(4, 0)).is_none(),
        "removed page must not be renumbered"
    );
    let new_second = map[&ObjId::new(6, 0)];
    let again = open(out.into_vec());
    let p = again.page(0).unwrap();
    assert_eq!(p.id(), Some(new_second));
}

#[test]
fn custom_input_source_over_borrowed_owner() {
    struct Owner(Vec<u8>);
    impl AsRef<[u8]> for Owner {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }
    let doc = Document::open_bytes(Owner(build_pdf(2)), &OpenOptions::default()).unwrap();
    // The document can be moved and objects outlive the Document handle.
    let page = doc.page(1).unwrap();
    drop(doc);
    assert_eq!(page.dict_get("Type").unwrap().name().unwrap(), b"Page");
    let arc: std::sync::Arc<[u8]> = build_pdf(1).into();
    assert_eq!(
        Document::open_bytes(arc, &OpenOptions::default())
            .unwrap()
            .page_count()
            .unwrap(),
        1
    );
    // Empty input is an error, not a crash.
    assert!(Document::open_bytes(Vec::<u8>::new(), &OpenOptions::default()).is_err());
}

#[test]
fn version_reported() {
    assert!(qpdf_version().starts_with("12."), "{}", qpdf_version());
}
