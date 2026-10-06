//! Extract and split: pages, bookmarks, fields, labels and file naming, with the oracles run on
//! every file that is written.

#[path = "../../papyrine-ops/tests/organise_support/mod.rs"]
mod organise_support;

use std::path::Path;

use organise_support::*;
use papyrine_organise::*;

fn source() -> papyrine_cos::Document {
    open(build(&Spec::rich("S", 8)))
}

fn check_bytes(bytes: Vec<u8>) -> Summary {
    let d = open(bytes.clone());
    assert!(d.repair_log().is_empty(), "output needed repair");
    let s = summarize(&d);
    interop(&bytes, &s.texts);
    s
}

fn check_file(path: &Path) -> Summary {
    check_bytes(std::fs::read(path).unwrap())
}

#[test]
fn extract_one_file() {
    let src = source();
    let doc = extract(&src, &[6, 1, 2], &ExtractOptions::default()).unwrap();
    let s = check_bytes(write(&doc));
    assert_eq!(s.texts, ["T:S P:7", "T:S P:2", "T:S P:3"]);
    // Fields only for the pages that came; values and widget pages follow.
    let f: Vec<(&str, &str, Vec<Option<usize>>)> = s
        .fields
        .iter()
        .map(|f| (f.0.as_str(), f.1.as_str(), f.2.clone()))
        .collect();
    assert_eq!(
        f,
        [
            ("fld7", "S-v7", vec![Some(0)]),
            ("fld2", "S-v2", vec![Some(1)]),
            ("fld3", "S-v3", vec![Some(2)]),
        ]
    );
    // Bookmarks that lead to these pages (parents of kept ones lead to their first section).
    let titles: Vec<(usize, &str, Option<usize>)> =
        s.outline.iter().map(|o| (o.0, o.1.as_str(), o.2)).collect();
    assert!(titles.contains(&(0, "S Chapter 7", Some(0))), "{titles:?}");
    assert!(titles.contains(&(1, "S Section 2", Some(1))), "{titles:?}");
    assert!(titles.contains(&(0, "S Chapter 3", Some(2))), "{titles:?}");
    assert!(!titles.iter().any(|t| t.1 == "S Chapter 5"));
    // Named destinations and the links that use them: links to pages outside the extract
    // are gone; page 2's links led to page 3, which is page index 2 here.
    assert_eq!(
        s.dests.keys().cloned().collect::<Vec<_>>(),
        ["S.dest2", "S.dest3", "S.dest7"]
    );
    assert_eq!(s.links[1], [Some(2), Some(2)]);
    assert!(
        s.links[0].is_empty(),
        "page 7's links led to page 8: {:?}",
        s.links[0]
    );
    assert!(s.links[2].is_empty());
    // Labels stay with the pages: page 7 = A-9 (source: i, ii, then A-5...), 2 = ii, 3 = A-5.
    assert_eq!(s.labels, ["A-9", "ii", "A-5"]);
    // Source info comes along.
    let info = doc.trailer().unwrap().dict_get("Info").unwrap();
    assert_eq!(info.dict_get("Title").unwrap().string().unwrap(), b"S");
}

#[test]
fn extract_validates() {
    let src = source();
    assert!(extract(&src, &[], &ExtractOptions::default()).is_err());
    assert!(extract(&src, &[8], &ExtractOptions::default()).is_err());
    assert!(split_every(&src, 0, &ExtractOptions::default()).is_err());
    assert!(split_ranges(&src, &[(0, 3), (3, 5)], &ExtractOptions::default()).is_err());
    assert!(split_ranges(&src, &[(4, 2)], &ExtractOptions::default()).is_err());
    assert!(split_ranges(&src, &[(0, 8)], &ExtractOptions::default()).is_err());
}

#[test]
fn split_by_ranges_to_files() {
    let src = source();
    let dir = tempfile::tempdir().unwrap();
    let ranges = parse_split_ranges("1-3, 4, 6-8", 8).unwrap();
    let parts = split_ranges(&src, &ranges, &ExtractOptions::default()).unwrap();
    assert_eq!(parts.len(), 3);
    let set = WriteSet::new(dir.path(), "Report");
    let paths = write_parts(parts, &set, None).unwrap();
    let names: Vec<String> = paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["Report_1-3.pdf", "Report_4.pdf", "Report_6-8.pdf"]);
    let s0 = check_file(&paths[0]);
    assert_eq!(s0.texts, ["T:S P:1", "T:S P:2", "T:S P:3"]);
    assert_eq!(s0.labels, ["i", "ii", "A-5"]);
    let s1 = check_file(&paths[1]);
    assert_eq!(s1.texts, ["T:S P:4"]);
    assert_eq!(s1.fields.len(), 1);
    let s2 = check_file(&paths[2]);
    assert_eq!(s2.texts, ["T:S P:6", "T:S P:7", "T:S P:8"]);
    assert_eq!(
        s2.fields.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(),
        ["fld6", "fld7", "fld8"]
    );
    // Existing files are never overwritten by default: names get a counter.
    let parts = split_ranges(&src, &ranges, &ExtractOptions::default()).unwrap();
    let again = write_parts(parts, &set, None).unwrap();
    assert_eq!(again[0].file_name().unwrap(), "Report_1-3 (2).pdf");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 6);
}

#[test]
fn split_every_n_and_extract_each_with_templates() {
    let src = source();
    let dir = tempfile::tempdir().unwrap();
    let parts = split_every(&src, 3, &ExtractOptions::default()).unwrap();
    let set = WriteSet::new(dir.path().join("by3"), "doc").template("{name}-part{n:02}-{title}");
    let paths = write_parts(parts, &set, None).unwrap();
    let names: Vec<String> = paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    // {title} is the part's first top-level bookmark.
    assert_eq!(
        names,
        [
            "doc-part01-S Chapter 1.pdf",
            "doc-part02-S Chapter 3.pdf",
            "doc-part03-S Chapter 7.pdf"
        ]
    );
    assert_eq!(check_file(&paths[2]).pages, 2);

    let each = extract_each(&src, &[0, 4], &ExtractOptions::default()).unwrap();
    let set = WriteSet::new(dir.path().join("each"), "doc").template("{name}_page{pages:3}");
    let paths = write_parts(each, &set, None).unwrap();
    assert_eq!(paths[0].file_name().unwrap(), "doc_page001.pdf");
    assert_eq!(paths[1].file_name().unwrap(), "doc_page005.pdf");
    assert_eq!(check_file(&paths[1]).texts, ["T:S P:5"]);

    // A constant template still yields distinct files.
    let parts = split_every(&src, 4, &ExtractOptions::default()).unwrap();
    let set = WriteSet::new(dir.path().join("same"), "doc").template("same");
    let paths = write_parts(parts, &set, None).unwrap();
    assert_eq!(
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["same.pdf", "same (2).pdf"]
    );
    // Bad templates are rejected before anything is written.
    let parts = split_every(&src, 4, &ExtractOptions::default()).unwrap();
    let bad = WriteSet::new(dir.path().join("bad"), "doc").template("{nope}");
    assert!(write_parts(parts, &bad, None).is_err());
    assert!(
        !dir.path()
            .join("bad")
            .read_dir()
            .is_ok_and(|mut d| d.next().is_some())
    );
}

#[test]
fn cancel_removes_what_was_written() {
    let src = source();
    let dir = tempfile::tempdir().unwrap();
    let parts = split_every(&src, 1, &ExtractOptions::default()).unwrap();
    let set = WriteSet::new(dir.path(), "doc");
    let mut cancel = |done: usize| done >= 3;
    let r = write_parts(parts, &set, Some(&mut cancel));
    assert!(matches!(r, Err(Error::Cancelled)));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn outline_modes_for_extract() {
    let src = source();
    let none = extract(
        &src,
        &[0, 1],
        &ExtractOptions {
            outline: OutlineMode::None,
            page_labels: false,
            info: InfoPolicy::None,
            ..ExtractOptions::default()
        },
    )
    .unwrap();
    let s = check_bytes(write(&none));
    assert!(s.outline.is_empty());
    assert_eq!(s.labels, ["1", "2"]);
    assert!(
        none.trailer()
            .unwrap()
            .dict_get("Info")
            .unwrap()
            .is_null()
            .unwrap()
    );
    let per = extract(
        &src,
        &[0, 1],
        &ExtractOptions {
            outline: OutlineMode::PerFile,
            ..ExtractOptions::default()
        },
    )
    .unwrap();
    let s = check_bytes(write(&per));
    assert_eq!(s.outline[0].1, "Document");
    assert_eq!(s.outline[0].0, 0);
}
