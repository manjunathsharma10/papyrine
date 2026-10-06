//! The 100-file merge acceptance test: bookmarks, form fields (names, values, widget pages),
//! named destinations and page labels all survive a merge of 100 files, on generated documents
//! and on 100 real corpus files that have both bookmarks and forms.

#[path = "../../papyrine-ops/tests/organise_support/mod.rs"]
mod organise_support;

use std::path::{Path, PathBuf};

use organise_support::*;
use papyrine_cos::{Document, FieldKind, FormField, OpenOptions};
use papyrine_organise::*;

type OutlineRow = (usize, String, Option<usize>);
type FileGroup = (String, Option<usize>, Vec<OutlineRow>);

struct Expect {
    name: String,
    pages: usize,
    nav: Nav,
    labels: Vec<String>,
    has_labels: bool,
}

fn expect_of(name: &str, doc: &Document) -> Expect {
    let pages = doc.page_count().unwrap();
    let pl = doc.page_labels().unwrap();
    Expect {
        name: name.into(),
        pages,
        nav: nav(doc),
        labels: (0..pages).map(|i| pl.label_for(i)).collect(),
        has_labels: !pl.ranges.is_empty(),
    }
}

fn is_subsequence<T: PartialEq>(needle: &[T], hay: &[T]) -> bool {
    let mut it = hay.iter();
    needle.iter().all(|n| it.any(|h| h == n))
}

/// Check the merged document against what every input said before the merge. Returns
/// `(bookmarks expected, bookmarks found, retargeted headings, fields checked, dests checked)`.
fn verify(expects: &[Expect], merged: &Document) -> (usize, usize, usize, usize, usize) {
    let m = nav(merged);
    let total: usize = expects.iter().map(|e| e.pages).sum();
    assert_eq!(merged.page_count().unwrap(), total);
    let mut offs = vec![];
    let mut acc = 0;
    for e in expects {
        offs.push(acc);
        acc += e.pages;
    }
    let file_of = |p: usize| offs.partition_point(|&o| o <= p) - 1;

    // Bookmarks: one top-level entry per file leading to its first page, the file's own
    // bookmarks (those with a page target) below it in order with pages shifted.
    let mut groups: Vec<FileGroup> = vec![];
    for (d, t, p) in &m.outline {
        if *d == 0 {
            groups.push((t.clone(), *p, vec![]));
        } else {
            groups
                .last_mut()
                .expect("child before any file entry")
                .2
                .push((*d - 1, t.clone(), *p));
        }
    }
    assert_eq!(
        groups.len(),
        expects.len(),
        "one top-level bookmark per file"
    );
    let (mut want_total, mut got_total, mut retargeted) = (0, 0, 0);
    for (k, (g, e)) in groups.iter().zip(expects).enumerate() {
        assert_eq!(g.0, e.name, "file entry title");
        assert_eq!(
            g.1,
            Some(offs[k]),
            "file entry leads to the file's first page"
        );
        let want: Vec<OutlineRow> = e
            .nav
            .outline
            .iter()
            .filter(|o| o.2.is_some())
            .map(|(d, t, p)| (*d, t.clone(), p.map(|p| p + offs[k])))
            .collect();
        let got: Vec<OutlineRow> = g.2.iter().filter(|o| o.2.is_some()).cloned().collect();
        assert!(
            is_subsequence(&want, &got),
            "{}: bookmarks differ\n want {want:?}\n got  {got:?}",
            e.name
        );
        for (_, _, p) in &got {
            let p = p.unwrap();
            assert!(
                p >= offs[k] && p < offs[k] + e.pages,
                "{}: bookmark leaves its file",
                e.name
            );
        }
        want_total += want.len();
        got_total += got.len();
        retargeted += got.len() - want.len();
        assert!(
            g.2.len() <= e.nav.outline.len(),
            "{}: bookmarks appeared from nowhere",
            e.name
        );
    }

    // Fields: per file the same values on the same (shifted) pages, unique names overall.
    let mut per_file: Vec<Vec<(String, Vec<usize>)>> = vec![vec![]; expects.len()];
    // A name may repeat inside one file (real forms have unnamed parents), never across files.
    let mut owner: std::collections::BTreeMap<String, usize> = Default::default();
    for (name, value, pages) in &m.fields {
        let pages: Vec<usize> = pages.iter().flatten().copied().collect();
        let Some(&first) = pages.first() else {
            continue;
        };
        let k = file_of(first);
        assert_eq!(
            *owner.entry(name.clone()).or_insert(k),
            k,
            "field name {name} is shared by two files"
        );
        assert!(
            pages.iter().all(|&p| file_of(p) == k),
            "field {name} spans files"
        );
        let mut local: Vec<usize> = pages.iter().map(|p| p - offs[k]).collect();
        local.sort_unstable();
        per_file[k].push((value.clone(), local));
    }
    let mut checked_fields = 0;
    for (k, e) in expects.iter().enumerate() {
        let mut want: Vec<(String, Vec<usize>)> = e
            .nav
            .fields
            .iter()
            .filter_map(|(_, v, pages)| {
                let mut p: Vec<usize> = pages.iter().flatten().copied().collect();
                p.sort_unstable();
                (!p.is_empty()).then(|| (v.clone(), p))
            })
            .collect();
        want.sort();
        per_file[k].sort();
        assert_eq!(per_file[k], want, "{}: fields differ", e.name);
        checked_fields += want.len();
    }

    // Named destinations: per file the same pages (names may have been renamed).
    let mut dests_per_file: Vec<Vec<usize>> = vec![vec![]; expects.len()];
    for p in m.dests.values().flatten() {
        let k = file_of(*p);
        dests_per_file[k].push(p - offs[k]);
    }
    let mut checked_dests = 0;
    for (k, e) in expects.iter().enumerate() {
        let mut want: Vec<usize> = e.nav.dests.values().flatten().copied().collect();
        want.sort_unstable();
        dests_per_file[k].sort_unstable();
        assert_eq!(
            dests_per_file[k], want,
            "{}: named destinations differ",
            e.name
        );
        checked_dests += want.len();
    }
    // Every destination name that exists leads to a page (nothing dangling was copied).
    assert!(
        m.dests.values().all(Option::is_some),
        "dangling named destination in the merge"
    );

    // Page labels: every page keeps its source label.
    if expects.iter().any(|e| e.has_labels) {
        let pl = merged.page_labels().unwrap();
        for (k, e) in expects.iter().enumerate() {
            for i in 0..e.pages {
                assert_eq!(
                    pl.label_for(offs[k] + i),
                    e.labels[i],
                    "{} page {i}",
                    e.name
                );
            }
        }
    }
    (
        want_total,
        got_total,
        retargeted,
        checked_fields,
        checked_dests,
    )
}

/// A field that can take a typed value: a text field with a widget on a page, not read-only.
fn marker_field(doc: &Document) -> Option<FormField> {
    doc.form_fields().ok()?.into_iter().find(|f| {
        f.kind == FieldKind::Text
            && f.flags & FormField::READ_ONLY == 0
            && f.widgets.iter().any(|w| w.page.is_some())
    })
}

/// After the merge the fields are live: type into the marked one of every file, write, reopen.
fn exercise_fields(merged: &Document, n_files: usize, marked: &[bool]) {
    for (k, _) in marked.iter().enumerate().take(n_files).filter(|(_, m)| **m) {
        let want = format!("MARK{k}");
        let hits: Vec<FormField> = merged
            .form_fields()
            .unwrap()
            .into_iter()
            .filter(|f| f.value == want)
            .collect();
        assert_eq!(hits.len(), 1, "marker {want} found {} times", hits.len());
        merged
            .set_form_field_value(hits[0].id, &format!("EDIT{k}"), false)
            .unwrap();
    }
    let re = roundtrip(merged);
    let f = re.form_fields().unwrap();
    for (k, _) in marked.iter().enumerate().take(n_files).filter(|(_, m)| **m) {
        assert_eq!(
            f.iter().filter(|x| x.value == format!("EDIT{k}")).count(),
            1
        );
    }
}

fn run_hundred(files: Vec<(String, Document)>, label: &str) {
    assert_eq!(files.len(), 100);
    let mut expects = vec![];
    let mut marked = vec![];
    let mut inputs = vec![];
    for (k, (name, doc)) in files.into_iter().enumerate() {
        let m = marker_field(&doc);
        if let Some(f) = &m {
            doc.set_form_field_value(f.id, &format!("MARK{k}"), false)
                .unwrap();
        }
        marked.push(m.is_some());
        expects.push(expect_of(&name, &doc));
        inputs.push(MergeInput::new(name, doc));
    }
    let t = std::time::Instant::now();
    let r = merge(inputs, &MergeOptions::default(), None).unwrap();
    let merge_ms = t.elapsed().as_millis();
    let (bm_want, bm_got, retargeted, nfields, ndests) = verify(&expects, &r.doc);
    let t = std::time::Instant::now();
    let bytes = write(&r.doc);
    let write_ms = t.elapsed().as_millis();
    let pages = r.doc.page_count().unwrap();

    // Interop on the single output file.
    qpdf_check(&bytes).unwrap();
    if let Some(n) = poppler_pages(&bytes).unwrap() {
        assert_eq!(n, pages, "Poppler page count");
    }
    let sample: Vec<usize> = r
        .reports
        .iter()
        .flat_map(|x| [x.first_page, x.first_page + x.pages - 1])
        .collect();
    pdfium_open_render(&bytes, pages, &sample).unwrap();

    // Reopen: the same navigation structure.
    let re = open(bytes.clone());
    assert!(
        re.repair_log().is_empty(),
        "merged file needed repair: {:?}",
        re.repair_log()
    );
    verify(&expects, &re);
    let (a, b) = (nav(&re), nav(&r.doc));
    assert_eq!(
        a.outline, b.outline,
        "outline changed across write + reopen"
    );
    assert_eq!(
        a.dests, b.dests,
        "named destinations changed across write + reopen"
    );
    // qpdf lists fields in object-number order, which a rewrite renumbers: compare as sets.
    let (mut fa, mut fb) = (a.fields.clone(), b.fields.clone());
    fa.sort();
    fb.sort();
    assert_eq!(fa.len(), fb.len());
    for (x, y) in fa.iter().zip(&fb) {
        assert_eq!(x, y, "field changed across write + reopen");
    }
    exercise_fields(&r.doc, 100, &marked);
    let renamed: usize = r.reports.iter().map(|x| x.fields.renamed.len()).sum();
    println!(
        "{label}: 100 files -> {pages} pages, {} bytes; merge {merge_ms} ms, write {write_ms} ms; \
         {bm_got} bookmarks ({bm_want} expected, {retargeted} headings retargeted), {nfields} \
         fields, {ndests} named destinations verified; {renamed} top-level fields renamed",
        bytes.len()
    );
}

#[test]
fn hundred_generated_files() {
    let files: Vec<(String, Document)> = (0..100)
        .map(|k| {
            // 37 distinct tags over 100 files: field values, destination names and (flat)
            // field names collide constantly.
            let mut spec = Spec::rich(&format!("F{:02}", k % 37), 1 + k % 7);
            spec.hier = k % 3 == 0;
            spec.labels = k % 4 != 0;
            spec.dests = k % 5 != 0;
            (format!("file{k:03}"), open(build(&spec)))
        })
        .collect();
    run_hundred(files, "generated");
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cache")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "pdf") {
            out.push(p);
        }
    }
}

/// Source files must pass `qpdf --check` themselves (some corpus files are deliberately
/// damaged), so that any warning on the merged file is ours. Without the CLI nothing is filtered.
fn qpdf_cli_clean(p: &Path) -> bool {
    match std::process::Command::new("qpdf")
        .arg("--check")
        .arg(p)
        .output()
    {
        Ok(o) => o.status.code() == Some(0),
        Err(_) => true,
    }
}

/// Clean, unencrypted, modestly sized corpus files that have form fields and/or bookmarks:
/// `(both, either)`, each sorted by path.
fn corpus_candidates() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut all = vec![];
    for sub in ["js-forms", "files"] {
        walk(&corpus_root().join(sub), &mut all);
    }
    all.sort();
    let (mut both, mut either) = (vec![], vec![]);
    for p in all {
        if !std::fs::metadata(&p).is_ok_and(|m| m.len() <= 3_000_000) {
            continue;
        }
        let Ok(d) = Document::open_path(&p, &OpenOptions::default()) else {
            continue;
        };
        if !(d.repair_log().is_empty()
            && d.encryption().ok().flatten().is_none()
            && d.page_count().is_ok_and(|n| (1..=80).contains(&n)))
        {
            continue;
        }
        let forms = d.form_fields().is_ok_and(|f| !f.is_empty());
        let outline = d.outlines().is_ok_and(|o| !o.is_empty());
        if (forms || outline) && qpdf_cli_clean(&p) {
            if forms && outline {
                both.push(p);
            } else {
                either.push(p);
            }
        }
    }
    (both, either)
}

#[test]
fn hundred_corpus_files_with_outlines_and_forms() {
    let (both, either) = corpus_candidates();
    if both.len() + either.len() < 100 {
        assert!(
            std::env::var_os("PAPYRINE_REQUIRE_CORPUS").is_none(),
            "corpus has only {} usable files",
            both.len() + either.len()
        );
        eprintln!(
            "note: only {} corpus candidates (need 100); skipping",
            both.len() + either.len()
        );
        return;
    }
    // Every file with both bookmarks and forms (up to 100), topped up with evenly spaced files
    // that have one of the two.
    let mut picked: Vec<PathBuf> = both.iter().take(100).cloned().collect();
    let need = 100 - picked.len();
    if need > 0 {
        let step = either.len() as f64 / need as f64;
        picked.extend((0..need).map(|i| either[(i as f64 * step) as usize].clone()));
    }
    picked.sort();
    let files: Vec<(String, Document)> = picked
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let name = format!("{k:03}-{}", p.file_stem().unwrap().to_string_lossy());
            (
                name,
                Document::open_path(p, &OpenOptions::default()).unwrap(),
            )
        })
        .collect();
    println!(
        "corpus: {} files with bookmarks and forms + {} with one of them available; using {} + {}",
        both.len(),
        either.len(),
        both.len().min(100),
        need
    );
    run_hundred(files, "corpus");
}
