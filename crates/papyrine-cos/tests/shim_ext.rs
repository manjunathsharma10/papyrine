//! Tests for the v0.1 shim extensions: lazy stream data, AcroForm, flattening, page copying,
//! outlines, page labels, embedded files, name/number trees, progress and cancellation.

use papyrine_cos::{
    Document, Error, FieldKind, FlattenOptions, LabelStyle, ObjId, OpenOptions, PageLabelRange,
    PageLabels, WriteOptions,
};
use std::cell::Cell;
use std::rc::Rc;

/// Assemble a classic-xref PDF. `objs[i]` is the body of object `i + 1`; catalog must be object 1.
fn pdf(objs: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offs = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    let n = objs.len() + 1;
    out.extend_from_slice(format!("xref\n0 {n}\n0000000000 65535 f \n").as_bytes());
    for o in &offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {n} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

fn stream(dict: &str, data: &str) -> String {
    format!(
        "<< {dict} /Length {} >>\nstream\n{data}\nendstream",
        data.len()
    )
}

fn open(bytes: Vec<u8>) -> Document {
    Document::open_bytes(bytes, &OpenOptions::default()).expect("open")
}

fn reopen(doc: &Document) -> Document {
    let out = doc
        .write(&WriteOptions::default())
        .expect("write")
        .into_vec();
    open(out)
}

/// One page: text field `name`, checkbox `agree`, and `grp.child` (parent/kid text field).
fn form_pdf() -> Vec<u8> {
    let font = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string();
    let on = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 20 20]",
        "0 0 20 20 re f",
    );
    let off = stream("/Type /XObject /Subtype /Form /BBox [0 0 20 20]", "");
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 6 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
         /Resources << /Font << /Helv 5 0 R >> >> /Annots [7 0 R 8 0 R 10 0 R] >>"
            .into(),
        stream("", "BT /Helv 12 Tf 72 700 Td (Form) Tj ET"),
        font,
        "<< /Fields [7 0 R 8 0 R 9 0 R] /DA (/Helv 0 Tf 0 g) \
         /DR << /Font << /Helv 5 0 R >> >> >>"
            .into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [72 600 272 620] \
         /DA (/Helv 12 Tf 0 g) /F 4 /P 3 0 R /MK << >> >>"
            .into(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off \
         /Rect [72 560 92 580] /F 4 /P 3 0 R /AP << /N << /Yes 11 0 R /Off 12 0 R >> >> >>"
            .into(),
        "<< /T (grp) /Kids [10 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (child) /Parent 9 0 R \
         /Rect [72 500 272 520] /DA (/Helv 12 Tf 0 g) /F 4 /P 3 0 R >>"
            .into(),
        on,
        off,
    ])
}

fn plain_pages(n: usize) -> Vec<u8> {
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")),
    ];
    for _ in 0..n {
        objs.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".into());
    }
    pdf(&objs)
}

// ---------------------------------------------------------------------------------------------
// Lazy stream data

#[test]
fn lazy_stream_provider_is_pulled_at_write_time() {
    let doc = Document::new_empty().unwrap();
    let s = doc.new_stream(b"").unwrap();
    let calls = Rc::new(Cell::new(0));
    let c = calls.clone();
    s.stream_replace_lazy(
        move || {
            c.set(c.get() + 1);
            Ok(b"lazy payload".to_vec())
        },
        None,
        None,
    )
    .unwrap();
    assert_eq!(calls.get(), 0, "provider must not run eagerly");
    doc.root().unwrap().dict_set("Data", &s).unwrap();
    let out = doc
        .write(&WriteOptions {
            stream_data: papyrine_cos::StreamMode::Uncompress,
            ..Default::default()
        })
        .unwrap()
        .into_vec();
    assert!(calls.get() >= 1);
    assert!(out.windows(12).any(|w| w == b"lazy payload"));
    let again = open(out);
    let data = again.root().unwrap().dict_get("Data").unwrap();
    assert_eq!(
        data.stream_decoded(papyrine_cos::DecodeLevel::All)
            .unwrap()
            .as_slice(),
        b"lazy payload"
    );
}

#[test]
fn lazy_stream_provider_error_fails_the_write() {
    let doc = Document::new_empty().unwrap();
    let s = doc.new_stream(b"").unwrap();
    s.stream_replace_lazy(|| Err("backing store vanished".into()), None, None)
        .unwrap();
    doc.root().unwrap().dict_set("Data", &s).unwrap();
    let err = doc
        .write(&WriteOptions::default())
        .err()
        .expect("must fail");
    assert!(err.to_string().contains("backing store vanished"), "{err}");
    // The document is still usable.
    assert!(doc.write(&WriteOptions::default()).is_err());
    s.stream_replace(b"ok", None, None).unwrap();
    doc.write(&WriteOptions::default()).unwrap();
}

#[test]
fn lazy_stream_provider_panic_is_an_error_not_a_crash() {
    let doc = Document::new_empty().unwrap();
    let s = doc.new_stream(b"").unwrap();
    s.stream_replace_lazy(|| panic!("boom"), None, None)
        .unwrap();
    doc.root().unwrap().dict_set("Data", &s).unwrap();
    assert!(doc.write(&WriteOptions::default()).is_err());
}

// ---------------------------------------------------------------------------------------------
// AcroForm

#[test]
fn lists_form_fields_with_qualified_names() {
    let doc = open(form_pdf());
    assert!(doc.has_acroform().unwrap());
    let fields = doc.form_fields().unwrap();
    let names: Vec<_> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["name", "agree", "grp.child"]);
    let name = &fields[0];
    assert_eq!(name.kind, FieldKind::Text);
    assert_eq!(name.field_type, "/Tx");
    assert_eq!(name.widgets.len(), 1);
    assert_eq!(name.widgets[0].page, Some(0));
    assert_eq!(name.widgets[0].rect, [72.0, 600.0, 272.0, 620.0]);
    assert!(name.default_appearance.contains("Helv"));
    let agree = &fields[1];
    assert_eq!(agree.kind, FieldKind::Checkbox);
    assert!(!agree.checked);
    assert_eq!(agree.states, ["Yes"]);
    assert_eq!(fields[2].partial_name, "child");
    assert_eq!(
        doc.form_field("grp.child").unwrap().unwrap().id,
        fields[2].id
    );
    assert!(doc.form_field("nope").unwrap().is_none());
}

#[test]
fn set_text_value_generates_appearance_and_survives_a_write() {
    let doc = open(form_pdf());
    let f = doc.form_field("name").unwrap().unwrap();
    doc.set_form_field_value(f.id, "Hello PDF", true).unwrap();
    assert!(!doc.need_appearances().unwrap());

    let check = |d: &Document| {
        let f = d.form_field("name").unwrap().unwrap();
        assert_eq!(f.value, "Hello PDF");
        let w = d.object(f.widgets[0].id.unwrap()).unwrap();
        let n = w.dict_get("AP").unwrap().dict_get("N").unwrap();
        let ap = n.stream_decoded(papyrine_cos::DecodeLevel::All).unwrap();
        let text = String::from_utf8_lossy(ap.as_slice()).into_owned();
        assert!(text.contains("Hello PDF"), "appearance: {text}");
    };
    check(&doc);
    check(&reopen(&doc));
}

#[test]
fn set_checkbox_updates_state() {
    let doc = open(form_pdf());
    let f = doc.form_field("agree").unwrap().unwrap();
    doc.set_form_checkbox(f.id, true).unwrap();
    let now = doc.form_field("agree").unwrap().unwrap();
    assert!(now.checked);
    assert_eq!(now.value, "Yes");
    let w = doc.object(now.widgets[0].id.unwrap()).unwrap();
    assert_eq!(w.dict_get("AS").unwrap().name().unwrap(), b"Yes");
    doc.set_form_checkbox(f.id, false).unwrap();
    assert!(!doc.form_field("agree").unwrap().unwrap().checked);
}

#[test]
fn need_appearances_toggle_and_regenerate() {
    let doc = open(form_pdf());
    assert!(!doc.need_appearances().unwrap());
    let f = doc.form_field("name").unwrap().unwrap();
    doc.set_form_field_value(f.id, "later", false).unwrap();
    doc.set_need_appearances(true).unwrap();
    assert!(doc.need_appearances().unwrap());
    doc.generate_form_appearances().unwrap();
    assert!(!doc.need_appearances().unwrap());
    let w = doc.object(f.widgets[0].id.unwrap()).unwrap();
    let n = w.dict_get("AP").unwrap().dict_get("N").unwrap();
    let ap = n.stream_decoded(papyrine_cos::DecodeLevel::All).unwrap();
    assert!(String::from_utf8_lossy(ap.as_slice()).contains("later"));
    // No AcroForm: harmless no-ops.
    let plain = open(plain_pages(1));
    plain.set_need_appearances(true).unwrap();
    assert!(!plain.need_appearances().unwrap());
    assert!(plain.form_fields().unwrap().is_empty());
}

#[test]
fn rejects_unsettable_field_kinds() {
    let doc = open(form_pdf());
    let bogus = doc.page(0).unwrap().id().unwrap();
    assert!(doc.set_form_field_value(bogus, "x", true).is_err());
}

#[test]
fn flatten_bakes_widgets_into_content() {
    let doc = open(form_pdf());
    let f = doc.form_field("name").unwrap().unwrap();
    doc.set_form_field_value(f.id, "Flat text", true).unwrap();
    // Widgets without any appearance stream cannot be flattened by qpdf and are left in place,
    // so give every widget one first.
    doc.generate_form_appearances().unwrap();
    doc.flatten_annotations(FlattenOptions::default()).unwrap();
    assert!(!doc.has_acroform().unwrap());
    assert!(doc.form_fields().unwrap().is_empty());
    let page = doc.page(0).unwrap();
    let annots = page.dict_get("Annots").unwrap();
    assert!(
        annots.array_len().map_or(true, |n| n == 0),
        "annots left: {annots:?}"
    );
    let again = reopen(&doc);
    assert!(again.form_fields().unwrap().is_empty());
    // The appearance became a form XObject painted from the page content.
    let res = again.page(0).unwrap().dict_get("Resources").unwrap();
    assert!(res.dict_has("XObject").unwrap());
}

#[test]
fn copy_pages_keeps_annotations_and_fields_private_per_copy() {
    let src = open(form_pdf());
    let f = src.form_field("name").unwrap().unwrap();
    src.set_form_field_value(f.id, "copied", true).unwrap();

    let dst = Document::new_empty().unwrap();
    let new = dst.copy_pages(&src, &[0, 0], usize::MAX).unwrap();
    assert_eq!(new.len(), 2);
    assert_eq!(dst.page_count().unwrap(), 2);
    assert!(dst.has_acroform().unwrap());

    let fields = dst.form_fields().unwrap();
    assert_eq!(
        fields.len(),
        6,
        "{:?}",
        fields.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    let mut names: Vec<_> = fields.iter().map(|f| f.name.clone()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 6, "colliding field names must be renamed");
    let pages: std::collections::BTreeSet<_> = fields
        .iter()
        .flat_map(|f| f.widgets.iter().map(|w| w.page))
        .collect();
    assert_eq!(pages, [Some(0), Some(1)].into());
    let annot_ids: std::collections::BTreeSet<_> = fields
        .iter()
        .flat_map(|f| f.widgets.iter().filter_map(|w| w.id))
        .collect();
    assert_eq!(annot_ids.len(), 6, "each copy has private annotations");
    assert!(fields.iter().any(|f| f.value == "copied"));

    // Survives a rewrite, and the source is untouched.
    let again = reopen(&dst);
    assert_eq!(again.form_fields().unwrap().len(), 6);
    assert_eq!(src.page_count().unwrap(), 1);
    assert_eq!(src.form_fields().unwrap().len(), 3);
}

#[test]
fn copy_pages_inserts_at_position_and_within_one_document() {
    let src = open(plain_pages(4));
    let dst = open(plain_pages(2));
    let marker = dst.page(1).unwrap();
    let new = dst.copy_pages(&src, &[3, 1], 1).unwrap();
    assert_eq!(new.len(), 2);
    assert_eq!(dst.page_count().unwrap(), 4);
    assert_eq!(dst.find_page(&dst.page(1).unwrap()).unwrap(), 1);
    assert_eq!(dst.find_page(&marker).unwrap(), 3);

    // Same document: duplicate the form page.
    let doc = open(form_pdf());
    doc.copy_pages(&doc, &[0], 1).unwrap();
    assert_eq!(doc.page_count().unwrap(), 2);
    assert_eq!(doc.form_fields().unwrap().len(), 6);
    assert!(matches!(
        dst.copy_pages(&src, &[9], 0),
        Err(Error::Range(_))
    ));
    assert_eq!(
        dst.page_count().unwrap(),
        4,
        "a failed copy changes nothing"
    );
}

// ---------------------------------------------------------------------------------------------
// Outlines, labels, embedded files

#[test]
fn reads_outlines_with_depth_pages_and_uri() {
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R /Names << /Dests 12 0 R >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Outlines /First 7 0 R /Last 9 0 R /Count 4 >>".into(),
        "<< /Title (Chapter 1) /Parent 6 0 R /Next 9 0 R /First 8 0 R /Last 8 0 R /Count -1 \
         /Dest [3 0 R /Fit] >>"
            .into(),
        "<< /Title (Section 1.1) /Parent 7 0 R /Dest [4 0 R /XYZ 0 100 null] >>".into(),
        "<< /Title (Chapter 2) /Parent 6 0 R /Prev 7 0 R /Next 10 0 R /Dest (named1) >>".into(),
        "<< /Title (Website) /Parent 6 0 R /Prev 9 0 R \
         /A << /S /URI /URI (https://example.org/) >> >>"
            .into(),
        "<< >>".into(),
        "<< /Names [(named1) [5 0 R /Fit]] >>".into(),
    ]);
    // Object 6 /Last should be 10 for a valid chain; qpdf tolerates and we assert on content.
    let doc = open(bytes);
    let o = doc.outlines().unwrap();
    let view: Vec<_> = o
        .iter()
        .map(|i| (i.title.as_str(), i.depth, i.page))
        .collect();
    assert_eq!(
        view,
        [
            ("Chapter 1", 0, Some(0)),
            ("Section 1.1", 1, Some(1)),
            ("Chapter 2", 0, Some(2)),
            ("Website", 0, None),
        ]
    );
    assert_eq!(o[0].count, -1);
    assert!(!o[0].is_open());
    assert_eq!(o[2].dest_name.as_deref(), Some("named1"));
    assert_eq!(o[3].uri.as_deref(), Some("https://example.org/"));
    assert!(open(plain_pages(1)).outlines().unwrap().is_empty());
}

#[test]
fn page_labels_round_trip_and_format() {
    let doc = open(plain_pages(8));
    assert!(doc.page_labels().unwrap().ranges.is_empty());
    let labels = PageLabels {
        ranges: vec![
            PageLabelRange {
                start_page: 0,
                style: LabelStyle::LowerRoman,
                prefix: String::new(),
                first_value: 1,
            },
            PageLabelRange {
                start_page: 3,
                style: LabelStyle::Decimal,
                prefix: "Ch\u{e9}-".into(),
                first_value: 5,
            },
            PageLabelRange {
                start_page: 6,
                style: LabelStyle::UpperAlpha,
                prefix: String::new(),
                first_value: 1,
            },
        ],
    };
    doc.set_page_labels(&labels).unwrap();
    for d in [
        doc.page_labels().unwrap(),
        reopen(&doc).page_labels().unwrap(),
    ] {
        assert_eq!(d, labels);
        assert_eq!(d.label_for(1), "ii");
        assert_eq!(d.label_for(4), "Ch\u{e9}-6");
        assert_eq!(d.label_for(7), "B");
    }
    // Replacing and clearing.
    let bad = PageLabels {
        ranges: vec![PageLabelRange {
            start_page: 2,
            style: LabelStyle::Decimal,
            prefix: String::new(),
            first_value: 1,
        }],
    };
    assert!(doc.set_page_labels(&bad).is_err());
    doc.set_page_labels(&PageLabels::default()).unwrap();
    assert!(doc.page_labels().unwrap().ranges.is_empty());
    assert!(!doc.root().unwrap().dict_has("PageLabels").unwrap());
}

#[test]
fn lists_embedded_files() {
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles 4 0 R >> >>".into(),
        "<< /Type /Pages /Kids [] /Count 0 >>".into(),
        "<< >>".into(),
        "<< /Names [(notes.txt) 5 0 R] >>".into(),
        "<< /Type /Filespec /F (notes.txt) /UF (notes.txt) /Desc (My notes) \
         /EF << /F 6 0 R /UF 6 0 R >> >>"
            .into(),
        stream(
            "/Type /EmbeddedFile /Subtype /text#2Fplain \
             /Params << /Size 11 /CreationDate (D:20260101000000Z) >>",
            "hello world",
        ),
    ]);
    let doc = open(bytes);
    let files = doc.embedded_files().unwrap();
    assert_eq!(files.len(), 1);
    let f = &files[0];
    assert_eq!(f.name, "notes.txt");
    assert_eq!(f.filename, "notes.txt");
    assert_eq!(f.description, "My notes");
    assert_eq!(f.mime_type.as_deref(), Some("text/plain"));
    assert_eq!(f.size, Some(11));
    assert_eq!(f.created.as_deref(), Some("D:20260101000000Z"));
    let data = doc
        .object(f.stream.unwrap())
        .unwrap()
        .stream_decoded(papyrine_cos::DecodeLevel::All)
        .unwrap();
    assert_eq!(data.as_slice(), b"hello world");
    assert!(open(plain_pages(1)).embedded_files().unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Trees

#[test]
fn name_tree_set_get_remove_with_splitting() {
    let doc = Document::new_empty().unwrap();
    let tree = doc.new_name_tree().unwrap();
    for i in (0..400).rev() {
        let v = doc.new_int(i);
        doc.name_tree_set(&tree, format!("key{i:04}"), &v).unwrap();
    }
    let keys = doc.name_tree_keys(&tree).unwrap();
    assert_eq!(keys.len(), 400);
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "keys sorted");
    assert_eq!(
        doc.name_tree_get(&tree, "key0123")
            .unwrap()
            .unwrap()
            .as_int()
            .unwrap(),
        123
    );
    assert!(doc.name_tree_get(&tree, "missing").unwrap().is_none());
    // Replace.
    doc.name_tree_set(&tree, "key0123", &doc.new_int(-1))
        .unwrap();
    assert_eq!(
        doc.name_tree_get(&tree, "key0123")
            .unwrap()
            .unwrap()
            .as_int()
            .unwrap(),
        -1
    );
    assert_eq!(doc.name_tree_keys(&tree).unwrap().len(), 400);
    assert!(doc.name_tree_remove(&tree, "key0123").unwrap());
    assert!(!doc.name_tree_remove(&tree, "key0123").unwrap());
    assert_eq!(doc.name_tree_keys(&tree).unwrap().len(), 399);

    // Attached under /Names, it survives a write.
    let attached = doc.catalog_name_tree("Dests", true).unwrap().unwrap();
    doc.name_tree_set(&attached, "a", &doc.new_int(1)).unwrap();
    let again = reopen(&doc);
    let t = again.catalog_name_tree("Dests", false).unwrap().unwrap();
    assert_eq!(
        again
            .name_tree_get(&t, "a")
            .unwrap()
            .unwrap()
            .as_int()
            .unwrap(),
        1
    );
    assert!(
        again
            .catalog_name_tree("EmbeddedFiles", false)
            .unwrap()
            .is_none()
    );
}

#[test]
fn number_tree_set_get_remove() {
    let doc = Document::new_empty().unwrap();
    let tree = doc.new_number_tree().unwrap();
    for i in (0..300).rev() {
        doc.number_tree_set(&tree, i * 3, &doc.new_int(i)).unwrap();
    }
    let keys = doc.number_tree_keys(&tree).unwrap();
    assert_eq!(keys.len(), 300);
    assert!(keys.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(
        doc.number_tree_get(&tree, 30)
            .unwrap()
            .unwrap()
            .as_int()
            .unwrap(),
        10
    );
    assert!(doc.number_tree_get(&tree, 31).unwrap().is_none());
    doc.number_tree_set(&tree, 30, &doc.new_int(99)).unwrap();
    assert_eq!(
        doc.number_tree_get(&tree, 30)
            .unwrap()
            .unwrap()
            .as_int()
            .unwrap(),
        99
    );
    assert!(doc.number_tree_remove(&tree, 30).unwrap());
    assert!(!doc.number_tree_remove(&tree, 30).unwrap());
    assert_eq!(doc.number_tree_keys(&tree).unwrap().len(), 299);
}

// ---------------------------------------------------------------------------------------------
// Writer progress and cancellation, page helpers

#[test]
fn write_reports_progress_and_can_be_cancelled() {
    let doc = open(plain_pages(300));
    let mut seen = Vec::new();
    let out = doc
        .write_with_progress(&WriteOptions::default(), &mut |p| {
            seen.push(p);
            true
        })
        .unwrap();
    assert!(!out.bytes().is_empty());
    assert!(!seen.is_empty());
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "monotonic: {seen:?}");
    assert_eq!(*seen.last().unwrap(), 100);

    let r = doc.write_with_progress(&WriteOptions::default(), &mut |_| false);
    assert!(r.as_ref().is_err_and(Error::is_cancelled));
    // Still usable afterwards.
    assert_eq!(reopen(&doc).page_count().unwrap(), 300);
}

#[test]
fn cancelled_file_write_removes_partial_output() {
    let dir = std::env::temp_dir().join(format!("papyrine-cos-cancel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.pdf");
    let doc = open(plain_pages(50));
    let r = doc.write_to_path_with_progress(&path, &WriteOptions::default(), &mut |_| false);
    assert!(r.as_ref().is_err_and(Error::is_cancelled));
    assert!(!path.exists());
    let mut last = 0;
    doc.write_to_path_with_progress(&path, &WriteOptions::default(), &mut |p| {
        last = p;
        true
    })
    .unwrap();
    assert_eq!(last, 100);
    assert_eq!(
        Document::open_path(&path, &OpenOptions::default())
            .unwrap()
            .page_count()
            .unwrap(),
        50
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn push_inherited_attributes_moves_them_to_pages() {
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] /Rotate 90 >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ]);
    let doc = open(bytes);
    let page = doc.page(0).unwrap();
    assert!(!page.dict_has("MediaBox").unwrap());
    doc.push_inherited_page_attributes().unwrap();
    let page = doc.page(0).unwrap();
    assert_eq!(page.dict_get("MediaBox").unwrap().array_len().unwrap(), 4);
    assert_eq!(page.dict_get("Rotate").unwrap().as_int().unwrap(), 90);
}

#[test]
fn remove_unreferenced_resources_drops_unused_entries() {
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R /F2 5 0 R >> >> >>"
            .into(),
        stream("", "BT /F1 12 Tf 10 10 Td (x) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R /F2 5 0 R >> >> >>"
            .into(),
    ]);
    let doc = open(bytes);
    let fonts = |d: &Document, i| {
        d.page(i)
            .unwrap()
            .dict_get("Resources")
            .unwrap()
            .dict_get("Font")
            .unwrap()
            .dict_keys()
            .unwrap()
    };
    assert_eq!(fonts(&doc, 0).len(), 2);
    doc.remove_unreferenced_page_resources(&doc.page(0).unwrap())
        .unwrap();
    assert_eq!(fonts(&doc, 0), [b"F1".to_vec()]);
    assert_eq!(fonts(&doc, 1).len(), 2);
    doc.remove_unreferenced_resources().unwrap();
    assert_eq!(fonts(&doc, 1), [b"F1".to_vec()]);
    let _ = ObjId::new(1, 0);
}
