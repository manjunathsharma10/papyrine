mod common;

use common::*;
use papyrine_cos::{Document, EncryptionRevision, ObjectStreams, OpenOptions, Secret, WriteOptions};
use papyrine_writer::*;

fn damage_startxref(pdf: &[u8]) -> Vec<u8> {
    let s = String::from_utf8_lossy(pdf).into_owned();
    let i = s.rfind("startxref").unwrap();
    let mut out = s[..i].as_bytes().to_vec();
    out.extend_from_slice(b"startxref\n7\n%%EOF\n");
    out
}

/// A PDF carrying a signature-shaped dictionary (`/ByteRange` + `/Contents`).
fn signed_looking() -> Vec<u8> {
    let doc = Document::new_empty().unwrap();
    let page = doc.parse_object("<< /Type /Page /MediaBox [0 0 100 100] /Resources << >> >>").unwrap();
    let page = doc.make_indirect(&page).unwrap();
    doc.add_page(&page, false).unwrap();
    let sig = doc
        .parse_object("<< /Type /Sig /Filter /Adobe.PPKLite /ByteRange [0 10 20 30] /Contents <0011> >>")
        .unwrap();
    let sig = doc.make_indirect(&sig).unwrap();
    doc.root().unwrap().dict_set("SigHolder", &sig).unwrap();
    doc.write(&WriteOptions {
        object_streams: ObjectStreams::Disable,
        ..WriteOptions::default()
    })
    .unwrap()
    .into_vec()
}

#[test]
fn healthy_file_saves_incrementally() {
    let pdf = make_pdf(2, ObjectStreams::Disable, None);
    let doc = open(pdf.clone(), None);
    assert!(!has_signatures(&doc).unwrap());
    assert!(matches!(plan(&doc, pdf.as_slice()).unwrap(), SavePlan::Incremental { suggest: None }));
}

#[test]
fn repaired_unsigned_file_gets_a_full_rewrite() {
    let pdf = damage_startxref(&make_pdf(2, ObjectStreams::Disable, None));
    let doc = open(pdf.clone(), None);
    assert!(xref_was_repaired(&doc.repair_log()), "{:?}", doc.repair_log());
    assert_eq!(
        plan(&doc, pdf.as_slice()).unwrap(),
        SavePlan::FullOptimized(RepairReason::XrefReconstructed)
    );

    let dir = tempfile::tempdir().unwrap();
    let path = write_tmp(dir.path(), "damaged.pdf", &pdf);
    let out = save(
        &doc,
        &path,
        &path,
        &SectionRequest::default(),
        &WriteOptions::default(),
        SaveOptions::default(),
    )
    .unwrap();
    let SaveOutcome::Saved(report) = out else { panic!("expected a save") };
    assert_eq!(report.kind, SaveKind::Optimized);
    assert!(report.renumbering.is_some());
    // The file is healthy now.
    let re = Document::open_path(&path, &OpenOptions { attempt_recovery: false, ..OpenOptions::default() }).unwrap();
    assert_eq!(re.page_count().unwrap(), 2);
    assert!(ChainState::scan(&std::fs::File::open(&path).unwrap()).is_ok());
}

#[test]
fn repaired_signed_file_asks_the_user_and_writes_nothing() {
    let pdf = damage_startxref(&signed_looking());
    let doc = open(pdf.clone(), None);
    assert!(has_signatures(&doc).unwrap());
    let plan_ = plan(&doc, pdf.as_slice()).unwrap();
    assert_eq!(
        plan_,
        SavePlan::DecisionNeeded(Decision::SignedAndDamaged(RepairReason::XrefReconstructed))
    );
    let dir = tempfile::tempdir().unwrap();
    let path = write_tmp(dir.path(), "signed-damaged.pdf", &pdf);
    let out = save(
        &doc,
        &path,
        &path,
        &SectionRequest::default(),
        &WriteOptions::default(),
        SaveOptions::default(),
    )
    .unwrap();
    assert!(matches!(out, SaveOutcome::DecisionNeeded(_)));
    assert_eq!(std::fs::read(&path).unwrap(), pdf, "nothing may be written before the user decides");
}

#[test]
fn healthy_signed_file_still_saves_incrementally() {
    let pdf = signed_looking();
    let doc = open(pdf.clone(), None);
    assert!(has_signatures(&doc).unwrap());
    assert!(matches!(plan(&doc, pdf.as_slice()).unwrap(), SavePlan::Incremental { .. }));
}

#[test]
fn suggestion_thresholds() {
    let mk = |sections, first| ChainState {
        len: 0,
        startxref: 0,
        kind: XrefKind::Table,
        size: 1,
        sections,
        hybrid: false,
        ends_with_eol: true,
        first_revision_len: first,
    };
    // Exactly 50% is not enough; one byte more is.
    assert_eq!(suggestion(&mk(2, Some(1000)), 1500, None), None);
    assert!(matches!(
        suggestion(&mk(2, Some(1000)), 1501, None),
        Some(SuggestOptimize::AppendedBytes { appended: 501, original: 1000 })
    ));
    assert_eq!(suggestion(&mk(20, Some(1000)), 1000, None), None);
    assert_eq!(suggestion(&mk(21, Some(1000)), 1000, None), Some(SuggestOptimize::Sections(21)));
    // The host can supply its own baseline.
    assert!(suggestion(&mk(2, Some(1000)), 1501, Some(10_000)).is_none());
}

#[test]
fn end_to_end_incremental_saves_chain_and_raise_the_suggestion() {
    let dir = tempfile::tempdir().unwrap();
    let original = make_pdf(3, ObjectStreams::Generate, Some(EncryptionRevision::R6));
    let path = write_tmp(dir.path(), "doc.pdf", &original);
    let pw = Some(Secret::from(USER));
    let mut doc = Document::open_path(&path, &OpenOptions { password: pw.clone(), ..OpenOptions::default() }).unwrap();
    let baseline = original.len() as u64;
    let mut first_suggestion = None;
    for n in 0..23 {
        let info = doc.trailer().unwrap().dict_get("Info").unwrap();
        info.dict_set("Counter", &doc.new_int(n)).unwrap();
        info.dict_set("Title", &doc.new_string(format!("save {n}")).unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let report = save_incremental(
            &doc,
            &path,
            &path,
            &SectionRequest { dirty: vec![info.id().unwrap()], ..SectionRequest::default() },
            SaveOptions { password: pw.clone(), baseline_len: Some(baseline), ..SaveOptions::default() },
        )
        .unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(&after[..before.len()], &before[..], "save {n}: not an exact prefix");
        assert_eq!(report.kind, SaveKind::Incremental);
        assert_eq!(report.bytes_written, after.len() as u64);
        assert_eq!(report.appended, after.len() as u64 - baseline);
        if first_suggestion.is_none() && report.suggest.is_some() {
            first_suggestion = Some((n, report.suggest.clone().unwrap(), report.sections));
        }
        // Reopen from disk so the next round starts from what was actually saved.
        doc = Document::open_path(&path, &OpenOptions { password: pw.clone(), ..OpenOptions::default() }).unwrap();
        let t = doc.trailer().unwrap().dict_get("Info").unwrap().dict_get("Title").unwrap().string().unwrap();
        assert_eq!(t, format!("save {n}").as_bytes());
    }
    let (n, s, sections) = first_suggestion.expect("a suggestion after many saves");
    eprintln!("first suggestion at save {n}: {s:?} (sections {sections})");
    assert!(sections > SUGGEST_SECTION_COUNT || matches!(s, SuggestOptimize::AppendedBytes { .. }));
    assert_eq!(qpdf_check(&path, Some(USER)).0, 0);
}

#[test]
fn incremental_save_refuses_a_file_that_changed_underneath() {
    let dir = tempfile::tempdir().unwrap();
    let original = make_pdf(1, ObjectStreams::Disable, None);
    let path = write_tmp(dir.path(), "doc.pdf", &original);
    let doc = Document::open_path(&path, &OpenOptions::default()).unwrap();
    // A second writer appends to the file after our scan but before our copy: simulated by
    // using a base that differs from the target's current content.
    let other = make_pdf(2, ObjectStreams::Disable, None);
    let base = write_tmp(dir.path(), "base.pdf", &other);
    let info = doc.trailer().unwrap().dict_get("Info").unwrap();
    info.dict_set("X", &doc.new_int(1)).unwrap();
    // Base is a different, valid file: the page count validation (1 page in memory vs 2 on disk)
    // catches the mismatch and the target is untouched.
    let r = save_incremental(
        &doc,
        &base,
        &path,
        &SectionRequest { dirty: vec![info.id().unwrap()], ..SectionRequest::default() },
        SaveOptions::default(),
    );
    assert!(r.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
}
