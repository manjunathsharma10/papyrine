//! Annotations created by other producers (Acrobat and friends, from the corpus) survive edits
//! to other annotations byte-for-byte: unchanged objects are never touched, an incremental
//! save appends only the changed objects, and the rest of the page renders identically.

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use common::*;
use papyrine_annotate::*;
use papyrine_cos::{Document, ObjId, Object, ObjectKind};
use papyrine_ops::{ChangeSummary, ObjectImage, TRAILER};
use papyrine_writer::{ChainState, SectionRequest, write_section};

const FILES: &[&str] = &[
    "pdfjs/test/pdfs/annotation-highlight.pdf",
    "pdfjs/test/pdfs/annotation-underline.pdf",
    "pdfjs/test/pdfs/annotation-strikeout.pdf",
    "pdfjs/test/pdfs/annotation-squiggly.pdf",
    "pdfjs/test/pdfs/annotation-freetext.pdf",
    "pdfjs/test/pdfs/annotation-caret-ink.pdf",
    "pdfjs/test/pdfs/annotation-line.pdf",
    "pdfjs/test/pdfs/annotation-square-circle.pdf",
    "pdfjs/test/pdfs/annotation-polyline-polygon.pdf",
    "pdfjs/test/pdfs/annotation-stamp.pdf",
    "pdfjs/test/pdfs/annotation-border-styles.pdf",
    "pdfjs/test/pdfs/annotation-link-text-popup.pdf",
    "pdfjs/test/pdfs/annotation-text-without-popup.pdf",
    "pdfjs/test/pdfs/annotation-fileattachment.pdf",
    "pdfjs/test/pdfs/tracemonkey_with_annotations.pdf",
    "pdfjs/test/pdfs/tracemonkey_with_editable_annotations.pdf",
    "pdfjs/test/pdfs/rc_annotation.pdf",
    "pdfjs/test/pdfs/annotation-tx.pdf",
    "pdfium/testing/resources/annots.pdf",
    "pdfium/testing/resources/links_highlights_annots.pdf",
    "pdfium/testing/resources/annotation_highlight_square_with_ap.pdf",
    "pdfium/testing/resources/annotation_highlight_rollover_ap.pdf",
    "pdfium/testing/resources/annotation_ink_multiple.pdf",
    "pdfium/testing/resources/annotation_stamp_with_ap.pdf",
    "pdfium/testing/resources/line_annot.pdf",
    "pdfium/testing/resources/ink_annot.pdf",
    "pdfium/testing/resources/freetext_annotation_without_da.pdf",
    "pdfplumber/tests/pdfs/annotations-rotated-90.pdf",
    "pdfplumber/tests/pdfs/annotations-rotated-180.pdf",
    "pdfplumber/tests/pdfs/annotations-rotated-270.pdf",
    "pdfplumber/tests/pdfs/annotations-unicode-issues.pdf",
    "pdfbox/pdfbox/src/test/resources/org/apache/pdfbox/pdmodel/interactive/annotation/AnnotationTypes.pdf",
    "pdfbox/pdfbox/src/test/resources/org/apache/pdfbox/pdmodel/interactive/annotation/Annotations.pdf",
];

fn corpus_file(rel: &str) -> Option<Vec<u8>> {
    let p: PathBuf = [env!("CARGO_MANIFEST_DIR"), "../../corpus/cache/files", rel]
        .iter()
        .collect();
    std::fs::read(p).ok()
}

/// The visible box: the CropBox if there is one (inherited), else the MediaBox.
fn media_box(page: &Object) -> [f64; 4] {
    inherited_box(page, "CropBox")
        .unwrap_or_else(|| inherited_box(page, "MediaBox").unwrap_or([0.0, 0.0, 612.0, 792.0]))
}

fn inherited_box(page: &Object, key: &str) -> Option<[f64; 4]> {
    let mut cur = page.clone();
    for _ in 0..32 {
        let mb = cur.dict_get(key).unwrap();
        if mb.kind().unwrap() == ObjectKind::Array && mb.array_len().unwrap() == 4 {
            let v: Vec<f64> = mb
                .array_items()
                .unwrap()
                .iter()
                .map(|o| o.as_f64().unwrap_or(0.0))
                .collect();
            return Some([
                v[0].min(v[2]),
                v[1].min(v[3]),
                v[0].max(v[2]),
                v[1].max(v[3]),
            ]);
        }
        cur = cur.dict_get("Parent").unwrap();
        if cur.kind().unwrap() != ObjectKind::Dictionary {
            break;
        }
    }
    None
}

/// First page with a non-empty /Annots array.
fn annotated_page(doc: &Document) -> Option<usize> {
    (0..doc.page_count().ok()?).find(|&i| {
        let a = doc.page(i).unwrap().dict_get("Annots").unwrap();
        a.kind().unwrap() == ObjectKind::Array && a.array_len().unwrap() > 0
    })
}

fn rect_of(doc: &Document, id: ObjId) -> Option<[f64; 4]> {
    let r = doc.object(id).ok()?.dict_get("Rect").ok()?;
    let v: Vec<f64> = r
        .array_items()
        .ok()?
        .iter()
        .filter_map(|o| o.as_f64().ok())
        .collect();
    (v.len() == 4).then(|| {
        [
            v[0].min(v[2]),
            v[1].min(v[3]),
            v[0].max(v[2]),
            v[1].max(v[3]),
        ]
    })
}

#[derive(Default)]
struct Stats {
    files: usize,
    annots: usize,
    subtypes: BTreeSet<String>,
}

#[test]
fn corpus_annotations_survive_edits_of_others() {
    let mut stats = Stats::default();
    let lib = papyrine_render::Library::global().expect("libpdfium");
    for rel in FILES {
        let Some(bytes) = corpus_file(rel) else {
            assert!(!tools_required(), "{rel} missing from the corpus cache");
            eprintln!("skipping {rel}: not in the corpus cache");
            continue;
        };
        let Ok(doc) = Document::open_bytes(bytes.clone(), &papyrine_cos::OpenOptions::default())
        else {
            eprintln!("skipping {rel}: does not open");
            continue;
        };
        let Some(pi) = annotated_page(&doc) else {
            eprintln!("skipping {rel}: no annotations");
            continue;
        };
        // Compare against the file exactly as qpdf read it (a damaged file may have been repaired).
        let page = doc.page(pi).unwrap();
        let originals: Vec<Object> = page.dict_get("Annots").unwrap().array_items().unwrap();
        let orig_ids: Vec<ObjId> = originals.iter().filter_map(Object::id).collect();
        for a in &originals {
            if let Ok(s) = a.dict_get("Subtype").and_then(|s| s.name()) {
                stats
                    .subtypes
                    .insert(String::from_utf8_lossy(&s).into_owned());
            }
        }
        stats.files += 1;
        stats.annots += originals.len();
        let before = snapshot(&doc);
        let mb = media_box(&page);
        let (w, h) = (mb[2] - mb[0], mb[3] - mb[1]);
        let base_raster = {
            let mut d = papyrine_render::Document::open(
                &lib,
                papyrine_render::bytes_from_vec(bytes.clone()),
                &[],
                None,
            );
            d.as_mut().ok().map(|d| {
                let (pw, ph) = d.page_size(pi).unwrap();
                let t = d
                    .render_preview(pi, pw.max(ph) as u32, &mut papyrine_render::never_cancel)
                    .unwrap();
                (pw, ph, t)
            })
        };

        let mut hist = history();
        let mut summaries: Vec<ChangeSummary> = Vec::new();
        let mut ours: Vec<ObjId> = Vec::new();
        let holder_ids = {
            let mut s: BTreeSet<ObjId> = BTreeSet::new();
            s.insert(page.id().unwrap());
            let an = page.dict_get("Annots").unwrap();
            if let Some(id) = an.id() {
                s.insert(id);
            }
            s
        };
        let check_untouched = |doc: &Document, ours: &[ObjId], sum: &ChangeSummary, what: &str| {
            for id in &sum.touched {
                assert!(
                    holder_ids.contains(id) || ours.contains(id),
                    "{rel}: {what} touched {id}, which belongs to another annotation"
                );
            }
            for (id, img) in &before {
                if holder_ids.contains(id) || ours.contains(id) || *id == TRAILER {
                    continue;
                }
                let now = ObjectImage::capture(doc, *id).unwrap();
                assert_eq!(&now, img, "{rel}: {what} changed object {id}");
            }
        };

        let x0 = mb[0] + w * 0.1;
        let y0 = mb[1] + h * 0.1;
        let steps: Vec<Box<dyn papyrine_ops::Command>> = vec![
            Box::new(AddAnnotation::highlight(
                pi,
                vec![Quad::from_rect(x0, y0, x0 + 80.0, y0 + 14.0)],
                AnnotProps::default().with_author("T"),
            )),
            Box::new(AddAnnotation::text_box(
                pi,
                [x0, y0 + 30.0, x0 + 150.0, y0 + 70.0],
                TextStyle::default(),
                "preserved?",
                AnnotProps::default().with_width(1.0),
            )),
            Box::new(AddAnnotation::pen(
                pi,
                vec![vec![
                    [x0, y0 + 90.0],
                    [x0 + 50.0, y0 + 120.0],
                    [x0 + 100.0, y0 + 90.0],
                ]],
                AnnotProps::default(),
            )),
            Box::new(AddAnnotation::sticky_note(
                pi,
                x0 + 200.0,
                y0 + 40.0,
                AnnotProps::default(),
            )),
        ];
        let mut mine: Vec<ObjId> = Vec::new();
        for (i, c) in steps.into_iter().enumerate() {
            let s = hist
                .execute(&doc, c)
                .unwrap_or_else(|e| panic!("{rel}: step {i}: {e}"));
            ours.extend(&s.created);
            mine.push(s.created[0]);
            check_untouched(&doc, &ours, &s, &format!("add #{i}"));
            summaries.push(s);
        }
        // Edit and reply to our own annotations.
        let upd = hist
            .execute(
                &doc,
                Box::new(UpdateAnnotation::new(
                    pi,
                    AnnotRef::id(mine[0]),
                    PropsPatch::new(AnnotProps::default().with_color(Color::rgb(0.0, 1.0, 0.0))),
                )),
            )
            .unwrap();
        check_untouched(&doc, &ours, &upd, "update");
        summaries.push(upd);
        let rep = hist
            .execute(
                &doc,
                Box::new(AddReply::new(
                    pi,
                    AnnotRef::id(mine[3]),
                    "ok",
                    AnnotProps::default(),
                )),
            )
            .unwrap();
        ours.extend(&rep.created);
        check_untouched(&doc, &ours, &rep, "reply");
        summaries.push(rep);
        let er = hist
            .execute(
                &doc,
                Box::new(EraseInk::new(
                    pi,
                    AnnotRef::id(mine[2]),
                    vec![[x0 + 50.0, y0 + 100.0]],
                    8.0,
                )),
            )
            .unwrap();
        check_untouched(&doc, &ours, &er, "erase");
        summaries.push(er);

        // Incremental save: only touched and created objects are written.
        let mut dirty: BTreeSet<ObjId> = BTreeSet::new();
        for s in &summaries {
            dirty.extend(s.touched.iter().copied().filter(|i| i.num != 0));
            dirty.extend(s.created.iter().copied());
        }
        let chain = ChainState::scan(bytes.as_slice()).unwrap();
        let sec = write_section(
            &doc,
            &chain,
            &SectionRequest {
                dirty: dirty.iter().copied().collect(),
                freed: Vec::new(),
                new_id: Some([7; 16]),
            },
        )
        .unwrap();
        for (id, _) in &sec.objects {
            assert!(
                !orig_ids.contains(id) || holder_ids.contains(id),
                "{rel}: incremental section rewrote annotation object {id}"
            );
        }
        let mut full = bytes.clone();
        full.extend_from_slice(&sec.bytes);
        assert_eq!(&full[..bytes.len()], &bytes[..], "original bytes untouched");
        let reopened =
            Document::open_bytes(full.clone(), &papyrine_cos::OpenOptions::default()).unwrap();
        let now_annots = reopened
            .page(pi)
            .unwrap()
            .dict_get("Annots")
            .unwrap()
            .array_items()
            .unwrap();
        // Originals first, in order, then ours.
        for (i, o) in originals.iter().enumerate() {
            let a = now_annots[i].unparse_with(true).unwrap();
            let b = o.unparse_with(true).unwrap_or_default();
            if o.is_indirect() {
                // Resolved text of the file's own annotation is unchanged.
                let orig_doc =
                    Document::open_bytes(bytes.clone(), &papyrine_cos::OpenOptions::default())
                        .unwrap();
                let oo = orig_doc
                    .page(pi)
                    .unwrap()
                    .dict_get("Annots")
                    .unwrap()
                    .array_get(i)
                    .unwrap();
                assert_eq!(
                    a,
                    oo.unparse_with(true).unwrap(),
                    "{rel}: annotation {i} changed"
                );
            }
            let _ = b;
        }
        assert!(now_annots.len() >= originals.len() + 4);

        // The rest of the page renders identically.
        if let Some((pw, ph, base)) = &base_raster {
            let mut d = papyrine_render::Document::open(
                &lib,
                papyrine_render::bytes_from_vec(bytes.clone()),
                &[papyrine_render::bytes_from_vec(sec.bytes.clone())],
                None,
            )
            .unwrap();
            let t = d
                .render_preview(pi, pw.max(*ph) as u32, &mut papyrine_render::never_cancel)
                .unwrap();
            assert_eq!((t.width, t.height), (base.width, base.height));
            // Where we added things (page-space rects) pixels may change; compare the rest by
            // checking that every differing pixel lies inside the union of our rects (mapped
            // through the page's unrotated bounding box, only valid for unrotated pages).
            let rot = doc
                .page(pi)
                .unwrap()
                .dict_get("Rotate")
                .map(|r| r.as_int().unwrap_or(0))
                .unwrap_or(0);
            if rot % 360 == 0 {
                let rects: Vec<[f64; 4]> = mine
                    .iter()
                    .chain(ours.iter())
                    .filter_map(|id| rect_of(&doc, *id))
                    .collect();
                let k = f64::from(t.width) / f64::from(*pw);
                let mut outside = 0usize;
                for py in 0..t.height {
                    for px in 0..t.width {
                        if t.pixel(px, py) == base.pixel(px, py) {
                            continue;
                        }
                        let (x, y) = (mb[0] + f64::from(px) / k, mb[3] - f64::from(py) / k);
                        if !rects.iter().any(|r| {
                            x >= r[0] - 3.0 && x <= r[2] + 3.0 && y >= r[1] - 3.0 && y <= r[3] + 3.0
                        }) {
                            outside += 1;
                        }
                    }
                }
                if outside != 0 {
                    eprintln!("page {pw}x{ph} mb {mb:?} rects {rects:?}");
                    let mut bb = [u32::MAX, u32::MAX, 0, 0];
                    for py in 0..t.height {
                        for px in 0..t.width {
                            if t.pixel(px, py) != base.pixel(px, py) {
                                bb = [bb[0].min(px), bb[1].min(py), bb[2].max(px), bb[3].max(py)];
                            }
                        }
                    }
                    eprintln!("diff bbox px {bb:?}");
                }
                assert_eq!(
                    outside, 0,
                    "{rel}: pixels changed outside the new annotations"
                );
            }
        }

        // Editing and deleting one of the file's own annotations leaves its neighbours alone.
        if let Some(&victim) = orig_ids.first()
            && let Some(sub) = doc
                .object(victim)
                .unwrap()
                .dict_get("Subtype")
                .ok()
                .and_then(|s| s.name().ok())
            && sub != b"Popup"
            && sub != b"Widget"
        {
            let snap = snapshot(&doc);
            let del = hist
                .execute(
                    &doc,
                    Box::new(DeleteAnnotations::new(pi, vec![AnnotRef::id(victim)])),
                )
                .unwrap();
            let removed: BTreeSet<ObjId> = del.touched.iter().copied().collect();
            assert!(
                removed.is_subset(&holder_ids),
                "{rel}: delete rewrote {removed:?}"
            );
            for (id, img) in &snap {
                if holder_ids.contains(id) {
                    continue;
                }
                assert_eq!(
                    &ObjectImage::capture(&doc, *id).unwrap(),
                    img,
                    "{rel}: delete changed {id}"
                );
            }
            hist.undo(&doc).unwrap().unwrap();
            assert_matches(&doc, &snap, "undo of the delete");
        }
        eprintln!("ok {rel}: {} original annotations", originals.len());
    }
    eprintln!(
        "{} files, {} annotations, subtypes {:?}",
        stats.files, stats.annots, stats.subtypes
    );
    assert!(
        stats.files >= 20,
        "only {} corpus files exercised",
        stats.files
    );
    for st in [
        "Highlight",
        "Underline",
        "StrikeOut",
        "Squiggly",
        "FreeText",
        "Ink",
        "Line",
        "Square",
        "Circle",
        "Text",
        "Stamp",
        "Link",
        "Popup",
        "Polygon",
    ] {
        assert!(
            stats.subtypes.contains(st),
            "no {st} annotation in the exercised files"
        );
    }
}

/// Editing the file's own annotations (colour change) regenerates only that annotation and
/// its appearance stream; undo restores everything exactly. Types and states Papyrine cannot
/// regenerate are refused without touching the document.
#[test]
fn editing_foreign_annotations_touches_only_themselves() {
    let (mut edited, mut refused) = (0usize, 0usize);
    for rel in FILES {
        let Some(bytes) = corpus_file(rel) else {
            continue;
        };
        let Ok(doc) = Document::open_bytes(bytes, &papyrine_cos::OpenOptions::default()) else {
            continue;
        };
        let Some(pi) = annotated_page(&doc) else {
            continue;
        };
        let page = doc.page(pi).unwrap();
        let originals: Vec<Object> = page.dict_get("Annots").unwrap().array_items().unwrap();
        let holder = page
            .dict_get("Annots")
            .unwrap()
            .id()
            .unwrap_or(page.id().unwrap());
        for (i, a) in originals.iter().enumerate() {
            let Some(id) = a.id() else { continue };
            let sub = a.dict_get("Subtype").unwrap().name().unwrap_or_default();
            let sub = String::from_utf8_lossy(&sub).into_owned();
            let ap = a
                .dict_get("AP")
                .ok()
                .and_then(|ap| ap.dict_get("N").ok())
                .and_then(|n| n.id());
            let before = snapshot(&doc);
            let mut h = history();
            let patch =
                PropsPatch::new(AnnotProps::default().with_color(Color::rgb(0.0, 0.0, 1.0)));
            let res = h.execute(
                &doc,
                Box::new(UpdateAnnotation::new(pi, AnnotRef::id(id), patch)),
            );
            match res {
                Ok(sum) => {
                    edited += 1;
                    for t in &sum.touched {
                        assert!(
                            *t == id || Some(*t) == ap || *t == holder,
                            "{rel} annot {i} ({sub}): update touched {t}"
                        );
                    }
                    // Metadata-only types keep their appearance untouched.
                    if !catalog::is_editable_subtype(&sub) {
                        assert!(!sum.touched.iter().any(|t| Some(*t) == ap), "{rel} {sub}");
                    }
                    let out = write_bytes(&doc);
                    Document::open_bytes(out, &papyrine_cos::OpenOptions::default())
                        .unwrap_or_else(|e| {
                            panic!("{rel} annot {i} ({sub}) unreadable after edit: {e}")
                        });
                    h.undo(&doc).unwrap().unwrap();
                    assert_matches(&doc, &before, &format!("{rel} annot {i} undo"));
                }
                Err(e) => {
                    refused += 1;
                    if catalog::is_editable_subtype(&sub) {
                        eprintln!("REFUSED editable {rel} #{i} {sub}: {e}");
                    }
                    assert_matches(
                        &doc,
                        &before,
                        &format!("{rel} annot {i} ({sub}) refused: {e}"),
                    );
                }
            }
        }
    }
    eprintln!("edited {edited} foreign annotations, refused {refused}");
    assert!(edited >= 40, "only {edited} edited");
}
