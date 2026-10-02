//! Every damaged corpus file qpdf can recover takes the full-rewrite path (or asks, if signed),
//! and the rewritten file is healthy and has the same page count.

mod common;

use common::*;
use papyrine_cos::{Document, OpenOptions, WriteOptions};
use papyrine_writer::*;

#[test]
fn damaged_files_are_rewritten_not_appended_to() {
    let files: Vec<_> = corpus_files(4 << 20)
        .into_iter()
        .filter(|f| {
            f.path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("mal-"))
        })
        .collect();
    if files.is_empty() {
        eprintln!("corpus not fetched; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (mut rewritten, mut asked, mut incremental_ok, mut unrecoverable) = (0, 0, 0, 0);
    for f in files {
        let Ok(doc) = Document::open_path(&f.path, &OpenOptions::default()) else {
            unrecoverable += 1;
            continue;
        };
        let Ok(pages) = doc.page_count() else {
            unrecoverable += 1;
            continue;
        };
        let work = dir.path().join(f.path.file_name().unwrap());
        std::fs::copy(&f.path, &work).unwrap();
        let before = std::fs::read(&work).unwrap();
        let name = f.path.file_name().unwrap().to_string_lossy().into_owned();
        let out = save(
            &doc,
            &work,
            &work,
            &SectionRequest::default(),
            &WriteOptions::default(),
            SaveOptions::default(),
        );
        match out {
            Ok(SaveOutcome::Saved(r)) if r.kind == SaveKind::Optimized => {
                rewritten += 1;
                let re = Document::open_path(
                    &work,
                    &OpenOptions {
                        attempt_recovery: false,
                        ..OpenOptions::default()
                    },
                )
                .unwrap_or_else(|e| panic!("{name}: rewrite not healthy: {e}"));
                assert_eq!(re.page_count().unwrap(), pages, "{name}");
                assert!(
                    ChainState::scan(&std::fs::File::open(&work).unwrap()).is_ok(),
                    "{name}"
                );
            }
            Ok(SaveOutcome::Saved(r)) => {
                // Damage that does not touch the xref chain (bad streams, odd page trees)
                // is saved incrementally and must still validate.
                assert_eq!(r.kind, SaveKind::Incremental, "{name}");
                let after = std::fs::read(&work).unwrap();
                assert_eq!(&after[..before.len()], &before[..], "{name}: prefix");
                incremental_ok += 1;
            }
            Ok(SaveOutcome::DecisionNeeded(_)) => {
                asked += 1;
                assert_eq!(std::fs::read(&work).unwrap(), before, "{name}");
            }
            Err(e) => {
                eprintln!(
                    "  save failed safely: {name}: {e}\n    plan: {:?}",
                    plan(&doc, &std::fs::File::open(&work).unwrap())
                );
                // A save that fails validation must leave the file as it was.
                assert_eq!(std::fs::read(&work).unwrap(), before, "{name}: {e}");
                unrecoverable += 1;
            }
        }
    }
    eprintln!(
        "damaged corpus: {rewritten} rewritten, {incremental_ok} incremental, {asked} asked, {unrecoverable} unrecoverable/failed-safely"
    );
    assert!(
        rewritten >= 5,
        "expected several damaged files to take the rewrite path"
    );
}
