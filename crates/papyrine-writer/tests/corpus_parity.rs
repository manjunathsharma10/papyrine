//! Serializer parity over the corpus: random edits, incremental sections, re-parse with qpdf,
//! compare every dirty object (and the untouched ones) with the in-memory handles.

mod common;

use common::*;
use papyrine_cos::{Document, OpenOptions, Secret};
use papyrine_writer::ChainState;

fn open_corpus(f: &CorpusFile) -> Option<(Vec<u8>, Document)> {
    let bytes = std::fs::read(&f.path).ok()?;
    let opts = OpenOptions {
        password: f.password.as_deref().map(Secret::from),
        attempt_recovery: false,
        ..OpenOptions::default()
    };
    let doc = Document::open_bytes(bytes.clone(), &opts).ok()?;
    doc.repair_log().is_empty().then_some((bytes, doc))
}

#[test]
fn serializer_parity_on_corpus() {
    let files = corpus_files(6 << 20);
    if files.is_empty() {
        eprintln!("corpus not fetched; skipping");
        return;
    }
    let n = env_usize("PAPYRINE_WRITER_CORPUS_N", 150);
    let (mut tested, mut skipped, mut edits_total) = (0, 0, 0usize);
    let mut seed = 0x9E3779B97F4A7C15u64;
    for f in sample(&files, n * 3) {
        if tested >= n {
            break;
        }
        let Some((original, doc)) = open_corpus(&f) else {
            skipped += 1;
            continue;
        };
        let Ok(base_chain) = ChainState::scan(original.as_slice()) else {
            skipped += 1;
            continue;
        };
        let ids = doc.object_ids().unwrap();
        if ids.is_empty() {
            skipped += 1;
            continue;
        }
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mut rng = Rng(seed | 1);
        let pw = f.password.as_deref();
        let before = if ids.len() <= 4000 {
            Some(all_fingerprints(&doc))
        } else {
            None
        };
        // Lazily detected damage (e.g. a stream without /Length) makes qpdf's view unstable
        // between reads; those files belong to the repair path, not to this test.
        if !doc.repair_log().is_empty() {
            skipped += 1;
            continue;
        }

        let mut cur = original.clone();
        let mut cur_doc = doc;
        let mut touched = std::collections::BTreeSet::new();
        for round in 0..2 {
            let ids_now = cur_doc.object_ids().unwrap();
            let count = 1 + rng.below(8);
            let dirty = random_edits(&cur_doc, &mut rng, &ids_now, count);
            edits_total += dirty.len();
            touched.extend(dirty.iter().copied());
            let (out, s) = incremental(&cur_doc, &cur, &dirty, &[]);
            assert_eq!(&out[..cur.len()], &cur[..]);
            let name = f.path.display();
            let re = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                assert_parity(&cur_doc, out.clone(), pw, &dirty)
            }))
            .unwrap_or_else(|e| {
                let _ = std::fs::write("/tmp/papyrine-writer-failed.pdf", &out);
                eprintln!(
                    "FAILED on {name} round {round}, output kept at /tmp/papyrine-writer-failed.pdf"
                );
                std::panic::resume_unwind(e)
            });
            assert_eq!(s.chain.sections, base_chain.sections + 1 + round);
            cur = out;
            cur_doc = re;
        }
        if let Some(before) = before {
            for (id, fp) in &before {
                let now = cur_doc.fingerprint(&cur_doc.object(*id).unwrap()).unwrap();
                if touched.contains(id) || references_any(&now.repr, &touched) {
                    continue; // a key can appear once a dangling reference becomes real
                }
                assert_eq!(
                    normalized(fp.clone()),
                    normalized(now),
                    "{}: untouched {id}",
                    f.path.display()
                );
            }
        }
        // The result is a structurally valid PDF according to qpdf's CLI.
        let dir = tempfile::tempdir().unwrap();
        let p = write_tmp(dir.path(), "o.pdf", &cur);
        let (code, text) = qpdf_check(&p, pw);
        if code == 2 {
            // Only our fault if the untouched file passed.
            let (orig_code, _) = qpdf_check(&f.path, pw);
            assert_eq!(orig_code, 2, "{}: qpdf --check\n{text}", f.path.display());
        }
        tested += 1;
    }
    eprintln!("corpus parity: {tested} files, {edits_total} dirty objects, {skipped} skipped");
    assert!(tested >= n.min(50), "only {tested} usable corpus files");
}

/// Does the serialized object mention `n g R` for any id in `set`?
fn references_any(repr: &[u8], set: &std::collections::BTreeSet<papyrine_cos::ObjId>) -> bool {
    let t = String::from_utf8_lossy(repr);
    let toks: Vec<&str> = t.split_whitespace().collect();
    toks.windows(3).any(|w| {
        w[2].trim_end_matches(['>', ']']) == "R"
            && matches!((w[0].parse::<u32>(), w[1].parse::<u16>()), (Ok(n), Ok(g))
                if set.contains(&papyrine_cos::ObjId::new(n, g)))
    })
}
