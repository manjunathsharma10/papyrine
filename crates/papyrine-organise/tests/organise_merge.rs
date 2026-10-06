//! Merge on generated documents: structure after the merge, round trip and the interop oracles.

#[path = "../../papyrine-ops/tests/organise_support/mod.rs"]
mod organise_support;

use organise_support::*;
use papyrine_organise::*;

fn input(spec: &Spec) -> MergeInput {
    MergeInput::new(format!("{}.pdf", spec.tag), open(build(spec)))
}

/// Write, check the oracles, reopen and compare against the live summary.
fn check_output(doc: &papyrine_cos::Document) -> Summary {
    let live = summarize(doc);
    let bytes = write(doc);
    interop(&bytes, &live.texts);
    let re = open(bytes);
    assert!(re.repair_log().is_empty(), "written merge needed repair");
    assert_eq!(
        summarize(&re),
        live,
        "summary changed across write + reopen"
    );
    live
}

#[test]
fn merge_keeps_everything_per_file() {
    let a = Spec::rich("A", 4);
    let b = Spec::rich("B", 3);
    let mut c = Spec::rich("A", 2); // same tag as the first: fields and destinations collide
    c.hier = true;
    let r = merge(
        vec![input(&a), input(&b), input(&c)],
        &MergeOptions::default(),
        None,
    )
    .unwrap();
    let s = check_output(&r.doc);
    assert_eq!(s.pages, 9);
    assert_eq!(
        s.texts,
        [
            "A P:1", "A P:2", "A P:3", "A P:4", "B P:1", "B P:2", "B P:3", "A P:1", "A P:2"
        ]
        .map(|t| format!("T:{t}"))
    );
    assert_eq!(
        r.reports
            .iter()
            .map(|x| (x.first_page, x.pages))
            .collect::<Vec<_>>(),
        [(0, 4), (4, 3), (7, 2)]
    );
    // Fields: unique names, own values, on the right pages.
    let names: Vec<&str> = s.fields.iter().map(|f| f.0.as_str()).collect();
    let mut dedup = names.clone();
    dedup.sort_unstable();
    dedup.dedup();
    assert_eq!(dedup.len(), names.len(), "duplicate field names: {names:?}");
    assert_eq!(names.len(), 9);
    for (name, value, pages) in &s.fields {
        let page = pages[0].unwrap();
        let (tag, i) = match page {
            0..=3 => ("A", page + 1),
            4..=6 => ("B", page - 3),
            _ => ("A", page - 6),
        };
        assert_eq!(value, &field_value(tag, i), "field {name} on page {page}");
    }
    assert!(names.contains(&"fld1") && names.contains(&"fld1_2") && names.contains(&"form1.fld1"));
    // One top-level bookmark per file, then that file's own bookmarks under it.
    let tops: Vec<(&str, Option<usize>)> = s
        .outline
        .iter()
        .filter(|o| o.0 == 0)
        .map(|o| (o.1.as_str(), o.2))
        .collect();
    assert_eq!(
        tops,
        [("A.pdf", Some(0)), ("B.pdf", Some(4)), ("A.pdf", Some(7))]
    );
    let child = |t: &str| s.outline.iter().find(|o| o.1 == t && o.0 > 0).unwrap().2;
    assert_eq!(child("B Chapter 1"), Some(4));
    assert_eq!(child("B Section 2"), Some(5));
    assert_eq!(child("B Chapter 3"), Some(6));
    let a_items: Vec<_> = s
        .outline
        .iter()
        .filter(|o| o.1.starts_with("A ") && o.0 > 0)
        .collect();
    assert_eq!(a_items.len(), 4 + 2);
    // Links go to the right page of their own file (the fixture links page i to i+1, wrapping).
    assert_eq!(s.links[0], [Some(1), Some(1)]);
    assert_eq!(s.links[3], [Some(0), Some(0)]);
    assert_eq!(s.links[6], [Some(4), Some(4)]);
    assert_eq!(s.links[7], [Some(8), Some(8)]);
    assert_eq!(s.links[8], [Some(7), Some(7)]);
    // Named destinations: the second "A" file's names were renamed and lead to its own pages.
    assert_eq!(s.dests["A.dest1"], Some(0));
    assert_eq!(s.dests["A.dest1_2"], Some(7));
    assert_eq!(s.dests["A.dest2_2"], Some(8));
    assert_eq!(s.dests["B.dest3"], Some(6));
    assert_eq!(s.dests.len(), 4 + 3 + 2);
    assert_eq!(r.reports[2].dests_renamed.len(), 2);
    // Page labels stay with their pages (roman x2 then "A-5..." in every source).
    assert_eq!(
        s.labels,
        ["i", "ii", "A-5", "A-6", "i", "ii", "A-5", "i", "ii"]
    );
}

#[test]
fn merge_options_and_selections() {
    let a = Spec::rich("A", 4);
    let b = Spec::rich("B", 4);
    let ins = vec![
        input(&a).pages(vec![3, 0]), // reordered selection
        input(&b).pages(vec![1, 2]),
    ];
    let opts = MergeOptions {
        outline: OutlineMode::Merged,
        title: Some("Bundle".into()),
        ..MergeOptions::default()
    };
    let r = merge(ins, &opts, None).unwrap();
    let s = check_output(&r.doc);
    assert_eq!(s.texts, ["T:A P:4", "T:A P:1", "T:B P:2", "T:B P:3"]);
    // Merged mode: no per-file entries, only bookmarks that lead to chosen pages.
    let titles: Vec<&str> = s.outline.iter().map(|o| o.1.as_str()).collect();
    assert!(titles.contains(&"A Section 4") && titles.contains(&"A Chapter 1"));
    assert!(!titles.contains(&"A.pdf"));
    assert!(
        !titles.contains(&"A Chapter 3")
            || s.outline
                .iter()
                .any(|o| o.1 == "A Chapter 3" && o.2 == Some(0))
    );
    let sec4 = s.outline.iter().find(|o| o.1 == "A Section 4").unwrap();
    assert_eq!(sec4.2, Some(0));
    // The A links to pages not selected are gone; page 1 of A points to A page 2 (dropped).
    assert!(s.links[0].is_empty() || s.links[0].iter().all(Option::is_some));
    // The result carries the requested title.
    let info = r.doc.trailer().unwrap().dict_get("Info").unwrap();
    assert_eq!(info.dict_get("Title").unwrap().string().unwrap(), b"Bundle");
    // Labels follow selected pages: A p4 = "A-6", A p1 = "i", B p2 = "ii", B p3 = "A-5".
    assert_eq!(s.labels, ["A-6", "i", "ii", "A-5"]);
    // No outline at all.
    let r = merge(
        vec![input(&a), input(&b)],
        &MergeOptions {
            outline: OutlineMode::None,
            page_labels: false,
            ..MergeOptions::default()
        },
        None,
    )
    .unwrap();
    let s = check_output(&r.doc);
    assert!(s.outline.is_empty());
    assert_eq!(s.labels, (1..=8).map(|i| i.to_string()).collect::<Vec<_>>());
}

#[test]
fn merge_errors_and_cancel() {
    assert!(merge(vec![], &MergeOptions::default(), None).is_err());
    let a = Spec::plain("A", 2);
    let e = merge(
        vec![input(&a).pages(vec![5])],
        &MergeOptions::default(),
        None,
    );
    assert!(matches!(e, Err(Error::Invalid(_))));
    let mut calls = 0;
    let mut cancel_after_one = |p: &Progress| {
        calls += 1;
        p.done < 1
    };
    let e = merge(
        vec![input(&a), input(&a), input(&a)],
        &MergeOptions::default(),
        Some(&mut cancel_after_one),
    );
    assert!(matches!(e, Err(Error::Cancelled)));
}

#[test]
fn merged_form_fields_stay_editable() {
    // After a merge the fields are real fields of the new document: set a value on one and
    // read it back after a write.
    let a = Spec::rich("A", 2);
    let b = Spec::rich("B", 2);
    let r = merge(vec![input(&a), input(&b)], &MergeOptions::default(), None).unwrap();
    let f = r
        .doc
        .form_field("fld2_2")
        .unwrap()
        .expect("renamed field exists");
    assert_eq!(f.value, "B-v2");
    r.doc.set_form_field_value(f.id, "edited", true).unwrap();
    let re = roundtrip(&r.doc);
    assert_eq!(re.form_field("fld2_2").unwrap().unwrap().value, "edited");
    assert_eq!(re.form_field("fld2").unwrap().unwrap().value, "A-v2");
}
