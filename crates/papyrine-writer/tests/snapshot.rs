//! The snapshot-section producer (ARCHITECTURE 4.6): `base || section_1 || ... || section_k` must
//! open in PDFium through the multi-buffer reader and show the edits; a merged section built
//! against the base must show the same thing.

mod common;

use common::*;
use papyrine_cos::{EncryptionRevision, ObjectStreams};
use papyrine_render as render;
use papyrine_writer::*;

#[test]
fn pdfium_sees_sections_and_merged_sections() {
    let Ok(lib) = render::Library::global() else {
        eprintln!("PDFium not available; skipping");
        return;
    };
    use EncryptionRevision::*;
    for (name, streams, enc) in [
        ("table", ObjectStreams::Disable, None),
        ("xref-stream", ObjectStreams::Generate, None),
        ("rc4-128", ObjectStreams::Disable, Some(R3)),
        ("aes-128", ObjectStreams::Generate, Some(R4 { aes: true })),
        ("aes-256", ObjectStreams::Disable, Some(R6)),
    ] {
        let pw = enc.map(|_| USER);
        let base = make_pdf(3, streams, enc);
        let doc = open(base.clone(), pw);
        let chain0 = ChainState::scan(base.as_slice()).unwrap();

        let mut rd =
            render::Document::open(&lib, render::bytes_from_vec(base.clone()), &[], pw).unwrap();
        assert_eq!(rd.page_rotation(0).unwrap(), 0);

        // Commit 1: rotate page 1.
        let p0 = doc.page(0).unwrap();
        p0.dict_set("Rotate", &doc.new_int(90)).unwrap();
        let s1 = write_section(
            &doc,
            &chain0,
            &SectionRequest {
                dirty: vec![p0.id().unwrap()],
                ..Default::default()
            },
        )
        .unwrap();
        rd.reopen(&[render::bytes_from_vec(s1.bytes.clone())])
            .unwrap();
        assert_eq!(rd.page_rotation(0).unwrap(), 90, "{name}");

        // Commit 2, chained on the first section's state.
        let p1 = doc.page(1).unwrap();
        p1.dict_set("Rotate", &doc.new_int(180)).unwrap();
        let s2 = write_section(
            &doc,
            &s1.chain,
            &SectionRequest {
                dirty: vec![p1.id().unwrap()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(s2.base_len, base.len() as u64 + s1.bytes.len() as u64);
        rd.reopen(&[
            render::bytes_from_vec(s1.bytes.clone()),
            render::bytes_from_vec(s2.bytes.clone()),
        ])
        .unwrap();
        assert_eq!(
            (
                rd.page_rotation(0).unwrap(),
                rd.page_rotation(1).unwrap(),
                rd.page_rotation(2).unwrap()
            ),
            (90, 180, 0),
            "{name}"
        );

        // L1 merge: one section against the base replaces both.
        let merged = write_section(
            &doc,
            &chain0,
            &SectionRequest {
                dirty: vec![p0.id().unwrap(), p1.id().unwrap()],
                ..Default::default()
            },
        )
        .unwrap();
        rd.reopen(&[render::bytes_from_vec(merged.bytes.clone())])
            .unwrap();
        assert_eq!(
            (rd.page_rotation(0).unwrap(), rd.page_rotation(1).unwrap()),
            (90, 180),
            "{name}"
        );
        assert!(merged.bytes.len() < s1.bytes.len() + s2.bytes.len() + 400);

        // And qpdf agrees with the concatenation.
        let mut all = base.clone();
        all.extend_from_slice(&merged.bytes);
        let re = open(all, pw);
        assert!(re.repair_log().is_empty(), "{name}");
        assert_eq!(
            re.page(1)
                .unwrap()
                .dict_get("Rotate")
                .unwrap()
                .as_int()
                .unwrap(),
            180
        );
    }
}
