//! Views checked against in-code generated PDFs whose expected values are known by construction.

mod common;

use common::*;
use papyrine_core::geom::{Point, Rect};
use papyrine_cos::{ObjId, OpenOptions};
use papyrine_model::*;
use std::rc::Rc;

fn id(n: u32) -> ObjId {
    ObjId::new(n, 0)
}

#[test]
fn page_tree_inheritance_boxes_rotation() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Count 4 /Kids [3 0 R 4 0 R] /MediaBox [0 0 500 700] /Rotate 90 \
         /Resources << /ProcSet [/PDF] >> >>"
            .into(),
        "<< /Type /Pages /Parent 2 0 R /Count 2 /Kids [5 0 R 6 0 R] /CropBox [10 10 400 600] /Rotate 180 >>".into(),
        "<< /Type /Pages /Parent 2 0 R /Count 2 /Kids [7 0 R 8 0 R] >>".into(),
        "<< /Type /Page /Parent 3 0 R >>".into(),
        "<< /Type /Page /Parent 3 0 R /MediaBox [0 0 200 300] /Rotate 450 /UserUnit 2 \
         /TrimBox [-50 -50 100 100] /ArtBox [150 250 50 50] /Tabs /S >>"
            .into(),
        "<< /Type /Page /Parent 4 0 R /Contents 9 0 R /Annots [10 0 R] >>".into(),
        "<< /Type /Page /Parent 4 0 R /MediaBox [0 0 (x) 5] /Rotate 45 /Resources << /Font << >> >> >>".into(),
        stream("", "q Q"),
        "<< /Type /Annot /Subtype /Text /Rect [0 0 1 1] >>".into(),
    ];
    let m = model(build(&objs, ""));
    assert_eq!(m.page_count().unwrap(), 4);
    assert_eq!(m.page_index_of(id(7)).unwrap(), Some(2));

    let p0 = m.page(0).unwrap();
    assert_eq!(p0.id, id(5));
    assert_eq!(p0.boxes.media, Rect::new(0.0, 0.0, 500.0, 700.0));
    assert_eq!(p0.boxes.crop, Rect::new(10.0, 10.0, 400.0, 600.0));
    assert_eq!(
        p0.boxes.declared_crop,
        Some(Rect::new(10.0, 10.0, 400.0, 600.0))
    );
    assert_eq!(p0.boxes.trim, p0.boxes.crop);
    assert_eq!(p0.rotate, 180);
    assert_eq!(p0.display_size().w, 390.0);
    assert!(p0.resources.is_some());
    assert!(!p0.has_contents);

    let p1 = m.page(1).unwrap();
    assert_eq!(p1.boxes.media, Rect::new(0.0, 0.0, 200.0, 300.0));
    // the CropBox is inherited from node 3 and clipped to the page's own MediaBox
    assert_eq!(p1.boxes.crop, Rect::new(10.0, 10.0, 200.0, 300.0));
    assert_eq!(p1.rotate, 90);
    assert_eq!(p1.user_unit, 2.0);
    assert_eq!(p1.boxes.trim, Rect::new(0.0, 0.0, 100.0, 100.0)); // clipped to media
    assert_eq!(p1.boxes.art, Rect::new(50.0, 50.0, 150.0, 250.0)); // corners normalized
    assert_eq!(p1.tab_order.as_deref(), Some("S"));
    let ds = p1.display_size();
    assert_eq!((ds.w, ds.h), (580.0, 380.0)); // 190x290 crop rotated 90, times 2

    let p2 = m.page(2).unwrap();
    assert_eq!(p2.rotate, 90);
    assert!(p2.has_contents);
    assert_eq!(p2.annotation_count, 1);
    assert_eq!(p2.boxes.media, Rect::new(0.0, 0.0, 500.0, 700.0));
    assert!(p2.resources.is_some());

    let p3 = m.page(3).unwrap();
    assert_eq!(p3.boxes.media, DEFAULT_MEDIA_BOX);
    assert_eq!(p3.boxes.declared_media, None);
    assert_eq!(p3.rotate, 0);
    assert!(!m.diagnostics().is_empty());

    assert!(m.page(4).is_err());
    assert!(m.page_object(9).is_err());
}

#[test]
fn page_labels_all_styles() {
    let objs = simple_doc(
        6,
        "/PageLabels << /Nums [0 << /S /r >> 2 << /S /D /St 5 /P (A-) >> 4 << /P (Cover) >> 5 << /S /a >>] >>",
        &[],
    );
    let m = model(build(&objs, ""));
    let l = m.page_labels().unwrap();
    assert!(l.is_defined());
    let got: Vec<String> = (0..6).map(|i| l.label(i)).collect();
    assert_eq!(got, ["i", "ii", "A-5", "A-6", "Cover", "a"]);
    assert_eq!(l.ranges.len(), 4);

    let m = model(build(&simple_doc(2, "", &[]), ""));
    assert!(!m.page_labels().unwrap().is_defined());
    assert_eq!(m.page_labels().unwrap().label(1), "2");
}

fn outline_doc() -> Vec<String> {
    let mut o = simple_doc(
        2,
        "/Names << /Dests << /Names [(chap1) [3 0 R /XYZ 10 20 1.5] (chap2) << /D [4 0 R /FitH 100] >>] >> >> \
         /Dests << /oldstyle [4 0 R /Fit] >> /Outlines 5 0 R /OpenAction [4 0 R /Fit]",
        &[],
    );
    o.extend([
        "<< /Type /Outlines /First 6 0 R /Last 8 0 R /Count 3 >>".to_string(), // 5
        "<< /Title (Chapter 1) /Parent 5 0 R /Dest (chap1) /Next 7 0 R /First 9 0 R /Last 10 0 R /Count 2 >>".into(), // 6
        "<< /Title <FEFF00430061006600E9> /Parent 5 0 R /A << /S /URI /URI (http://example.com/x) >> /Prev 6 0 R /Next 8 0 R /C [1 0 0] /F 3 >>".into(), // 7
        "<< /Title (Last) /Parent 5 0 R /A << /S /GoTo /D (chap2) >> /Prev 7 0 R >>".into(), // 8
        "<< /Title (Sub) /Parent 6 0 R /Dest [4 0 R /FitR 0 0 50 60] /Next 10 0 R >>".into(), // 9
        "<< /Title (Loop) /Parent 6 0 R /Dest /oldstyle /Next 9 0 R >>".into(), // 10
    ]);
    o
}

#[test]
fn outlines_named_dests_actions_cycles() {
    let m = model(build(&outline_doc(), ""));
    let ol = m.outlines().unwrap();
    assert!(ol.truncated, "the /Next cycle must be detected");
    assert_eq!(ol.items.len(), 3);
    let c1 = &ol.items[0];
    assert_eq!(c1.title, "Chapter 1");
    assert!(c1.open);
    assert_eq!(c1.count, 2);
    let d = c1.dest.as_ref().unwrap();
    assert_eq!(d.name.as_deref(), Some("chap1"));
    assert_eq!(d.local_page(), Some(0));
    assert_eq!(
        d.fit,
        Fit::Xyz {
            left: Some(10.0),
            top: Some(20.0),
            zoom: Some(1.5)
        }
    );
    assert_eq!(c1.children.len(), 2);
    assert_eq!(
        c1.children[0].dest.as_ref().unwrap().fit,
        Fit::FitR(Rect::new(0.0, 0.0, 50.0, 60.0))
    );
    let old = c1.children[1].dest.as_ref().unwrap();
    assert_eq!((old.local_page(), &old.fit), (Some(1), &Fit::Fit));

    let c2 = &ol.items[1];
    assert_eq!(c2.title, "Caf\u{e9}");
    assert_eq!(c2.color, Some([1.0, 0.0, 0.0]));
    assert!(c2.bold && c2.italic);
    assert!(!c2.open);
    assert_eq!(
        c2.action.as_ref().unwrap().external_targets()[0].kind,
        ExternalKind::Uri
    );
    assert!(c2.action.as_ref().unwrap().needs_prompt());
    assert_eq!(c2.dest, None);

    let c3 = &ol.items[2];
    let d3 = c3.dest.as_ref().expect("dest taken from the GoTo action");
    assert_eq!(d3.local_page(), Some(1));
    assert_eq!(d3.fit, Fit::FitH { top: Some(100.0) });
    assert_eq!(ol.total(), 5);

    let cat = m.catalog_info().unwrap();
    match cat.open_action.as_ref().unwrap() {
        OpenAction::Destination(d) => assert_eq!(d.local_page(), Some(1)),
        other => panic!("{other:?}"),
    }
    let names = m.named_destinations().unwrap();
    let mut n: Vec<_> = names.iter().map(|d| d.name.clone().unwrap()).collect();
    n.sort();
    assert_eq!(n, ["chap1", "chap2", "oldstyle"]);
}

fn form_doc() -> Vec<String> {
    let mut o = simple_doc(1, "/AcroForm 4 0 R", &[]);
    // 3 = page with annots, patched below
    o[2] = page_obj(
        "/Annots [11 0 R 12 0 R 13 0 R 14 0 R 16 0 R 17 0 R 18 0 R 19 0 R 20 0 R 21 0 R 22 0 R]",
    );
    o.push(
        "<< /Fields [30 0 R 14 0 R 15 0 R 18 0 R 19 0 R 20 0 R 21 0 R 22 0 R] /DA (/Helv 0 Tf 0 g) /Q 1 \
         /NeedAppearances true /CO [13 0 R 11 0 R] /SigFlags 3 >>"
            .into(),
    ); // 4
    while o.len() < 10 {
        o.push("null".into());
    }
    let rect = "/Rect [10 10 110 30] /P 3 0 R /Subtype /Widget";
    o.extend([
        // 11 text, multiline, merged widget, inherits DA/Q override
        format!("<< /FT /Tx /T (name) /Parent 30 0 R {rect} /MaxLen 10 /Ff 4096 /V (Hello) /DV (Default) /TU (tip) /DA (/Helv 12 Tf) /Q 2 \
          /AA << /K << /S /JavaScript /JS (AFNumber_Keystroke\\(2,0,0,0,\"\",true\\)) >> /C << /S /JavaScript /JS (event.value=1;) >> >> >>"),
        // 12 password
        format!("<< /FT /Tx /T (pin) /Parent 30 0 R {rect} /Ff 8192 >>"),
        // 13 comb
        format!("<< /FT /Tx /T (comb) /Parent 30 0 R {rect} /Ff 16777216 /MaxLen 8 >>"),
        // 14 checkbox
        format!("<< /FT /Btn /T (agree) {rect} /V /Yes /AS /Yes /AP << /N << /Yes 40 0 R /Off 40 0 R >> >> >>"),
        // 15 radio parent
        "<< /FT /Btn /Ff 32768 /T (color) /V /Blue /Kids [16 0 R 17 0 R] >>".into(),
        // 16, 17 radio widgets
        format!("<< /Parent 15 0 R {rect} /AS /Off /AP << /N << /Red 40 0 R /Off 40 0 R >> >> >>"),
        format!("<< /Parent 15 0 R {rect} /AS /Blue /AP << /N << /Blue 40 0 R /Off 40 0 R >> >> >>"),
        // 18 combo
        format!("<< /FT /Ch /Ff 131072 /T (country) {rect} /Opt [(US) [(CA) (Canada)]] /V (CA) >>"),
        // 19 multi-select listbox
        format!("<< /FT /Ch /Ff 2097152 /T (multi) {rect} /Opt [(a) (b) (c)] /V [(a) (c)] /I [0 2] /TI 1 >>"),
        // 20 pushbutton
        format!("<< /FT /Btn /Ff 65536 /T (go) {rect} /MK << /CA (Submit) /BC [0 0 1] /BG [0.9] /R 90 >> \
          /BS << /W 2 /S /B >> /A << /S /SubmitForm /F << /FS /URL /F (http://example.com/s) >> /Flags 4 >> >>"),
        // 21 unsigned signature field
        format!("<< /FT /Sig /T (sig1) {rect} >>"),
        // 22 signed signature field
        format!("<< /FT /Sig /T (sig2) {rect} /V 23 0 R >>"),
        // 23 signature value
        "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 100 200 50] \
          /Contents <00112233> /Name (Signer) /Reason (Because) /M (D:20240102030405Z) >>".into(),
    ]);
    // 24..29 padding, 30 = form group, 40 = AP stream
    while o.len() < 29 {
        o.push("null".into());
    }
    o.push("<< /T (form) /Kids [11 0 R 12 0 R 13 0 R] >>".into()); // 30
    while o.len() < 39 {
        o.push("null".into());
    }
    o.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1]", "")); // 40
    o
}

#[test]
fn form_tree_fields_widgets_actions() {
    let m = model(build(&form_doc(), ""));
    let f = m.form().unwrap();
    assert!(f.has_acroform && f.need_appearances);
    assert_eq!(f.sig_flags, 3);
    assert_eq!(f.xfa, XfaKind::None);
    assert_eq!(f.default_appearance.as_deref(), Some("/Helv 0 Tf 0 g"));
    let names: Vec<&str> = f
        .terminal_fields()
        .map(|x| x.qualified_name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "form.name",
            "form.pin",
            "form.comb",
            "agree",
            "color",
            "country",
            "multi",
            "go",
            "sig1",
            "sig2"
        ]
    );
    assert_eq!(f.roots.len(), 8);
    assert!(!f.field_by_id(id(30)).unwrap().is_terminal);
    assert_eq!(f.field_by_id(id(30)).unwrap().qualified_name, "form");

    let name = f.find("form.name").unwrap();
    assert_eq!(name.kind, FieldKind::Text);
    assert!(name.is_multiline() && !name.is_password() && !name.is_comb());
    assert_eq!(name.value, FieldValue::Text("Hello".into()));
    assert_eq!(name.default_value, FieldValue::Text("Default".into()));
    assert_eq!(name.max_len, Some(10));
    assert_eq!(name.quadding, 2);
    assert_eq!(name.default_appearance.as_deref(), Some("/Helv 12 Tf"));
    assert_eq!(name.tooltip.as_deref(), Some("tip"));
    assert_eq!(name.widgets.len(), 1);
    assert_eq!(name.widgets[0].page, Some(0));
    assert_eq!(name.widgets[0].rect, Rect::new(10.0, 10.0, 110.0, 30.0));
    assert!(name.action("K").unwrap().scripts()[0].starts_with("AFNumber_Keystroke("));
    assert_eq!(name.action("C").unwrap().scripts(), ["event.value=1;"]);
    assert!(name.action("V").is_none());

    let pin = f.find("form.pin").unwrap();
    assert!(pin.is_password() && !pin.is_multiline());
    assert_eq!(pin.quadding, 1, "inherits AcroForm /Q");
    assert_eq!(pin.default_appearance.as_deref(), Some("/Helv 0 Tf 0 g"));
    assert!(f.find("form.comb").unwrap().is_comb());

    let cb = f.find("agree").unwrap();
    assert_eq!(cb.kind, FieldKind::Checkbox);
    assert_eq!(cb.value, FieldValue::Name("Yes".into()));
    assert_eq!(cb.is_checked(), Some(true));
    assert_eq!(cb.export_values, ["Yes"]);
    assert!(cb.widgets[0].is_on && cb.widgets[0].has_appearance);
    assert_eq!(cb.widgets[0].ap_states.len(), 2);

    let radio = f.find("color").unwrap();
    assert_eq!(radio.kind, FieldKind::Radio);
    assert_eq!(radio.widgets.len(), 2);
    assert_eq!(radio.export_values, ["Red", "Blue"]);
    assert_eq!(radio.value, FieldValue::Name("Blue".into()));
    assert!(!radio.widgets[0].is_on && radio.widgets[1].is_on);
    let (owner, wi) = f.owner_of_widget(id(17)).unwrap();
    assert_eq!((owner.qualified_name.as_str(), wi), ("color", 1));

    let combo = f.find("country").unwrap();
    assert_eq!(combo.kind, FieldKind::ComboBox);
    assert_eq!(
        combo.options,
        [
            ChoiceOption {
                export: "US".into(),
                display: "US".into()
            },
            ChoiceOption {
                export: "CA".into(),
                display: "Canada".into()
            }
        ]
    );
    assert!(!combo.is_editable_combo());
    let multi = f.find("multi").unwrap();
    assert_eq!(multi.kind, FieldKind::ListBox);
    assert!(multi.is_multi_select());
    assert_eq!(multi.value, FieldValue::List(vec!["a".into(), "c".into()]));
    assert_eq!(multi.selected_indices, [0, 2]);
    assert_eq!(multi.top_index, Some(1));

    let go = f.find("go").unwrap();
    assert_eq!(go.kind, FieldKind::PushButton);
    let w = &go.widgets[0];
    assert_eq!(w.caption.as_deref(), Some("Submit"));
    assert_eq!(w.border_color.as_deref(), Some(&[0.0, 0.0, 1.0][..]));
    assert_eq!(w.background.as_deref(), Some(&[0.9][..]));
    assert_eq!(w.rotation, 90);
    let b = w.border.as_ref().unwrap();
    assert_eq!((b.width, b.style), (2.0, BorderKind::Beveled));
    let t = w.action.as_ref().unwrap().external_targets();
    assert_eq!(
        t[0],
        ExternalTarget {
            kind: ExternalKind::SubmitForm,
            target: "http://example.com/s".into()
        }
    );
    match &w.action.as_ref().unwrap().kind {
        ActionKind::SubmitForm { flags, .. } => assert_eq!(*flags, 4),
        k => panic!("{k:?}"),
    }

    // calc order refers to fields 13 (comb) then 11 (name)
    let co: Vec<&str> = f
        .calc_order
        .iter()
        .map(|&i| f.fields[i].qualified_name.as_str())
        .collect();
    assert_eq!(co, ["form.comb", "form.name"]);
    assert_eq!(f.widgets_on_page(0).len(), f.widget_count());

    // signatures
    let s = m.signatures().unwrap();
    assert_eq!(s.fields.len(), 2);
    assert_eq!((s.signed_count(), s.unsigned_count()), (1, 1));
    let signed = s.fields.iter().find(|x| x.signed).unwrap();
    assert_eq!(signed.field_name, "sig2");
    assert_eq!(signed.sub_filter.as_deref(), Some("adbe.pkcs7.detached"));
    assert_eq!(signed.byte_range, [0, 100, 200, 50]);
    assert_eq!(signed.contents_len, Some(4));
    assert_eq!(signed.signer_name.as_deref(), Some("Signer"));
    assert_eq!(
        signed.signing_time.unwrap().to_iso8601(),
        "2024-01-02T03:04:05Z"
    );
    assert!(s.is_signed() && s.requires_incremental_save() && !s.certified);

    // scripts found through the form
    let sc = m.scripts().unwrap();
    assert!(sc.has_javascript());
    assert_eq!(sc.field_scripts().count(), 2);
}

#[test]
fn xfa_static_and_dynamic() {
    let mk = |template: &str, catalog_extra: &str| {
        let mut o = simple_doc(1, &format!("/AcroForm 4 0 R {catalog_extra}"), &[]);
        o.push("<< /Fields [] /XFA [(preamble) 5 0 R (template) 6 0 R] >>".into());
        o.push(stream("", "<xdp/>"));
        o.push(stream("", template));
        build(&o, "")
    };
    let st = model(mk(
        "<template xmlns=\"x\" baseProfile=\"interactiveForms\"><subform/></template>",
        "",
    ));
    assert_eq!(st.form().unwrap().xfa, XfaKind::Static);
    let dy = model(mk(
        "<template xmlns=\"x\"><subform/></template>",
        "/NeedsRendering true",
    ));
    let f = dy.form().unwrap();
    assert_eq!(f.xfa, XfaKind::Dynamic);
    assert!(f.needs_rendering);
}

#[test]
fn annotations_all_the_fields() {
    let mut o = simple_doc(1, "", &[]);
    o[2] = page_obj(
        "/Annots [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R 9 0 R 10 0 R 11 0 R 12 0 R 13 0 R 4 0 R null << /Subtype /Square /Rect [1 2 3 4] >> 99 0 R]",
    );
    o.extend([
        // 4 text with popup
        "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (Note) /T (Alice) /Subj (S) /M (D:20230101120000Z) \
         /CreationDate (D:20221231235959-05'00') /C [1 1 0] /CA 0.5 /Popup 5 0 R /Name /Comment /F 28 /NM (id1) /Open false >>".into(),
        // 5 popup
        "<< /Type /Annot /Subtype /Popup /Rect [40 40 100 100] /Parent 4 0 R /Open true >>".into(),
        // 6 reply
        "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /IRT 4 0 R /RT /R /T (Bob) /Contents (reply) /State (Accepted) /StateModel (Review) >>".into(),
        // 7 highlight
        "<< /Type /Annot /Subtype /Highlight /Rect [0 0 100 20] /QuadPoints [0 20 100 20 0 0 100 0 0 50 100 50 0 30 100 30] /C [1 0.9 0] /IT /Highlight >>".into(),
        // 8 ink
        "<< /Type /Annot /Subtype /Ink /Rect [0 0 20 10] /InkList [[0 0 10 10 20 0] [5 5 6 6]] /BS << /W 3 >> >>".into(),
        // 9 line
        "<< /Type /Annot /Subtype /Line /Rect [0 0 100 100] /L [0 0 100 100] /LE [/OpenArrow /ClosedArrow] /IC [1 0 0] /BS << /W 1 /S /D /D [4 2] >> >>".into(),
        // 10 polygon
        "<< /Type /Annot /Subtype /Polygon /Rect [0 0 10 10] /Vertices [0 0 10 0 10 10] >>".into(),
        // 11 link
        "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Border [0 0 2 [3 1]] /A << /S /URI /URI (https://example.org) >> >>".into(),
        // 12 stamp
        "<< /Type /Annot /Subtype /Stamp /Rect [0 0 10 10] /Name /Approved /AP << /N 14 0 R >> >>".into(),
        // 13 free text
        "<< /Type /Annot /Subtype /FreeText /Rect [0 0 100 30] /DA (/Helv 10 Tf) /Q 1 /Contents (Hi) /Subtype /FreeText >>".into(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1]", ""), // 14
    ]);
    let m = model(build(&o, ""));
    let set = m.annotations(0).unwrap();
    // duplicate 4, null and the dangling ref are dropped; the direct dict stays
    assert_eq!(set.items.len(), 11);
    let a = set.by_id(id(4)).unwrap();
    assert_eq!(a.subtype, AnnotSubtype::Text);
    assert_eq!(a.contents.as_deref(), Some("Note"));
    assert_eq!(a.title.as_deref(), Some("Alice"));
    assert_eq!(a.subject.as_deref(), Some("S"));
    assert_eq!(a.name.as_deref(), Some("id1"));
    assert_eq!(a.modified.unwrap().to_unix(), 1_672_574_400);
    assert_eq!(a.creation_date.unwrap().utc_offset_minutes, Some(-300));
    assert_eq!(a.color.as_deref(), Some(&[1.0, 1.0, 0.0][..]));
    assert_eq!(a.opacity, 0.5);
    assert_eq!(a.popup, Some(id(5)));
    assert_eq!(a.icon_name.as_deref(), Some("Comment"));
    assert_eq!(a.flags, 28);
    assert!(a.has_flag(annot_flags::PRINT) && a.has_flag(annot_flags::NO_ZOOM));
    assert_eq!(a.open, Some(false));
    assert_eq!(set.popup_of(id(4)).unwrap().id, Some(id(5)));
    assert_eq!(set.popup_of(id(4)).unwrap().open, Some(true));
    let replies = set.replies_to(id(4));
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].reply_type.as_deref(), Some("R"));
    assert_eq!(replies[0].state.as_deref(), Some("Accepted"));
    assert_eq!(replies[0].state_model.as_deref(), Some("Review"));

    let h = set.by_id(id(7)).unwrap();
    assert!(h.subtype.is_text_markup());
    assert_eq!(h.quad_points.len(), 2);
    assert_eq!(h.quad_points[1][3], Point::new(100.0, 30.0));
    assert_eq!(h.intent.as_deref(), Some("Highlight"));
    let ink = set.by_id(id(8)).unwrap();
    assert_eq!(ink.ink_list.len(), 2);
    assert_eq!(ink.ink_list[0][2], Point::new(20.0, 0.0));
    assert_eq!(ink.border.as_ref().unwrap().width, 3.0);
    let line = set.by_id(id(9)).unwrap();
    assert_eq!(
        line.line,
        Some([Point::new(0.0, 0.0), Point::new(100.0, 100.0)])
    );
    assert_eq!(line.line_endings, ["OpenArrow", "ClosedArrow"]);
    assert_eq!(line.interior_color.as_deref(), Some(&[1.0, 0.0, 0.0][..]));
    let b = line.border.as_ref().unwrap();
    assert_eq!(
        (b.style, b.dash.clone()),
        (BorderKind::Dashed, vec![4.0, 2.0])
    );
    assert_eq!(set.by_id(id(10)).unwrap().vertices.len(), 3);
    let link = set.by_id(id(11)).unwrap();
    assert_eq!(link.uri(), Some("https://example.org"));
    let lb = link.border.as_ref().unwrap();
    assert_eq!((lb.width, lb.dash.clone()), (2.0, vec![3.0, 1.0]));
    let stamp = set.by_id(id(12)).unwrap();
    assert!(stamp.has_appearance);
    assert_eq!(stamp.icon_name.as_deref(), Some("Approved"));
    let ft = set.by_id(id(13)).unwrap();
    assert_eq!(ft.default_appearance.as_deref(), Some("/Helv 10 Tf"));
    assert_eq!(ft.quadding, Some(1));
    let direct = set.items.iter().find(|a| a.id.is_none()).unwrap();
    assert_eq!(direct.subtype, AnnotSubtype::Square);
    assert_eq!(direct.rect, Rect::new(1.0, 2.0, 3.0, 4.0));
    assert_eq!(m.page(0).unwrap().annotation_count, 14);
}

#[test]
fn info_catalog_xmp_scripts_security() {
    let xmp = "<?xpacket begin=''?><x:xmpmeta xmlns:x='adobe:ns:meta/'/><?xpacket end='w'?>";
    let mut o = simple_doc(
        2,
        "/Version /1.7 /Lang (en-GB) /MarkInfo << /Marked true >> /StructTreeRoot << >> /Metadata 5 0 R \
         /PageMode /UseOutlines /PageLayout /TwoColumnLeft /ViewerPreferences << /HideToolbar true /Direction /R2L /DisplayDocTitle true >> \
         /Names << /JavaScript << /Names [(init) 7 0 R] >> /EmbeddedFiles << /Names [] >> >> \
         /OpenAction 8 0 R /AA << /WC 8 0 R >> /OCProperties << >> /Extensions << /ADBE << /ExtensionLevel 8 >> >>",
        &[],
    );
    o[2] = page_obj("/AA << /O 7 0 R >>");
    o.push(stream("/Type /Metadata /Subtype /XML", xmp)); // 5
    o.push(
        "<< /Title <FEFF00480065006C006C006F263A> /Author (Jos\\351 \\200 Smith) /Creator (C) /Producer (P) \
         /CreationDate (D:20200229101112+01'00') /ModDate (garbage) /Trapped /True /Custom (val) /Num 3 >>"
            .into(),
    ); // 6
    o.push("<< /S /JavaScript /JS (app.alert\\(\"hi\"\\);) >>".into()); // 7
    o.push("<< /S /JavaScript /JS 9 0 R >>".into()); // 8
    o.push(stream("", "this.print();")); // 9
    let m = model(build_versioned("1.4", &o, "/Info 6 0 R"));
    let info = m.info().unwrap();
    assert_eq!(info.title.as_deref(), Some("Hello\u{263a}"));
    assert_eq!(info.author.as_deref(), Some("Jos\u{e9} \u{2022} Smith"));
    assert_eq!(
        info.creation_date.unwrap().to_iso8601(),
        "2020-02-29T10:11:12+01:00"
    );
    assert_eq!(info.mod_date, None);
    assert_eq!(info.mod_date_raw.as_deref(), Some("garbage"));
    assert_eq!(info.trapped.as_deref(), Some("True"));
    assert_eq!(info.custom.get("Custom").map(String::as_str), Some("val"));
    assert!(!info.custom.contains_key("Num"));

    let c = m.catalog_info().unwrap();
    assert_eq!(
        (c.header_version.as_str(), c.version.as_str()),
        ("1.4", "1.7")
    );
    assert_eq!(c.catalog_version.as_deref(), Some("1.7"));
    assert_eq!(c.language.as_deref(), Some("en-GB"));
    assert!(c.is_marked && c.has_struct_tree && c.has_xmp && c.has_embedded_files);
    assert!(c.has_optional_content && !c.has_acroform && !c.has_outlines);
    assert_eq!(c.page_mode.as_deref(), Some("UseOutlines"));
    assert_eq!(c.page_layout.as_deref(), Some("TwoColumnLeft"));
    assert_eq!(c.adobe_extension_level, Some(8));
    assert!(c.viewer_preferences.hide_toolbar && c.viewer_preferences.display_doc_title);
    assert_eq!(c.viewer_preferences.direction.as_deref(), Some("R2L"));
    assert!(matches!(c.open_action, Some(OpenAction::Action(_))));
    assert_eq!(c.catalog_actions.len(), 1);
    assert_eq!(m.xmp_packet().unwrap().unwrap(), xmp.as_bytes());

    let s = m.scripts().unwrap();
    assert_eq!(s.scripts.len(), 4);
    let doc: Vec<_> = s.document_level().collect();
    assert_eq!(doc.len(), 1);
    assert_eq!(doc[0].source, "app.alert(\"hi\");");
    assert!(
        s.scripts
            .iter()
            .any(|x| x.source == "this.print();" && x.location == ScriptLocation::OpenAction)
    );
    assert!(s.scripts.iter().any(
        |x| matches!(&x.location, ScriptLocation::Page { page: 0, trigger } if trigger == "O")
    ));
    assert!(m.has_javascript().unwrap());

    let sec = m.security().unwrap();
    assert!(!sec.encrypted && sec.effective == PermissionSummary::ALL);
}

#[test]
fn certified_document_blocks_full_rewrite() {
    let mut o = simple_doc(1, "/AcroForm 4 0 R /Perms << /DocMDP 6 0 R >>", &[]);
    o[2] = page_obj("/Annots [5 0 R]");
    o.push("<< /Fields [5 0 R] /SigFlags 3 >>".into()); // 4
    o.push("<< /FT /Sig /T (cert) /Subtype /Widget /Rect [0 0 0 0] /P 3 0 R /V 6 0 R >>".into()); // 5
    o.push(
        "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 10] /Contents <00> \
         /Reference [<< /TransformMethod /DocMDP /TransformParams << /P 1 /V /1.2 >> >>] >>"
            .into(),
    ); // 6
    let m = model(build(&o, ""));
    let s = m.signatures().unwrap();
    assert!(s.certified && s.forbids_changes() && s.requires_incremental_save());
    assert_eq!(s.certification_level, Some(1));
    assert_eq!(s.fields[0].docmdp_level, Some(1));
    assert_eq!(s.fields[0].page, Some(0));

    let plain = model(build(&simple_doc(1, "", &[]), ""));
    let s = plain.signatures().unwrap();
    assert!(!s.is_signed() && !s.requires_incremental_save());
}

#[test]
fn encryption_and_permissions_summary() {
    use papyrine_cos::{
        Document, EncryptionMode, EncryptionRevision, EncryptionSpec, PrintPermission, WriteOptions,
    };
    let plain =
        Document::open_bytes(build(&simple_doc(1, "", &[]), ""), &OpenOptions::default()).unwrap();
    let mut spec = EncryptionSpec::new(EncryptionRevision::R6, "user", "owner");
    spec.permissions.extract = false;
    spec.permissions.print = PrintPermission::LowRes;
    spec.permissions.annotate_and_form = false;
    let bytes = plain
        .write(&WriteOptions {
            encryption: EncryptionMode::Encrypt(spec),
            ..WriteOptions::default()
        })
        .unwrap()
        .into_vec();

    let as_user = Model::open_bytes(bytes.clone(), &OpenOptions::with_password("user")).unwrap();
    let sec = as_user.security().unwrap();
    assert!(sec.encrypted);
    assert_eq!(sec.algorithm.as_deref(), Some("AES-256 (R6)"));
    assert!(!sec.owner_authenticated);
    assert!(sec.effective.print && !sec.effective.print_high_quality);
    assert!(!sec.effective.copy && !sec.effective.annotate);
    assert!(sec.effective.modify_contents);
    assert_eq!(sec.effective, sec.declared);
    assert_eq!(as_user.page_count().unwrap(), 1);

    let as_owner = Model::open_bytes(bytes.clone(), &OpenOptions::with_password("owner")).unwrap();
    let sec = as_owner.security().unwrap();
    assert!(sec.owner_authenticated);
    assert_eq!(sec.effective, PermissionSummary::ALL);
    assert!(!sec.declared.copy);

    assert!(Model::open_bytes(bytes, &OpenOptions::default()).is_err());
}

#[test]
fn invalidation_is_selective() {
    let m = model(build(&simple_doc(3, "", &[]), ""));
    let p0 = m.page(0).unwrap();
    let p1 = m.page(1).unwrap();
    let list = m.page_list().unwrap();
    assert_eq!(p0.rotate, 0);
    assert!(Rc::ptr_eq(&p0, &m.page(0).unwrap()), "cached");
    assert_eq!(m.generation(id(3)), 0);

    // Edit page 0's Rotate behind the model's back, then tell it which object changed.
    let page0 = m.page_object(0).unwrap();
    page0
        .dict_set("Rotate", &m.document().new_int(270))
        .unwrap();
    assert_eq!(m.page(0).unwrap().rotate, 0, "stale until invalidated");
    m.invalidate(&[id(3)]);
    assert_eq!(m.generation(id(3)), 1);
    assert_eq!(m.page(0).unwrap().rotate, 270);
    assert!(!Rc::ptr_eq(&p0, &m.page(0).unwrap()));
    assert!(
        Rc::ptr_eq(&p1, &m.page(1).unwrap()),
        "other pages stay cached"
    );
    assert!(
        Rc::ptr_eq(&list, &m.page_list().unwrap()),
        "leaf edits keep the page list"
    );

    // Editing the Pages node drops the list and every page.
    let pages = m.document().object(id(2)).unwrap();
    pages
        .dict_set(
            "MediaBox",
            &m.document().parse_object("[0 0 100 100]").unwrap(),
        )
        .unwrap();
    m.invalidate(&[id(2)]);
    assert!(!Rc::ptr_eq(&p1, &m.page(1).unwrap()));
    assert!(!Rc::ptr_eq(&list, &m.page_list().unwrap()));
    // page objects had their own MediaBox, so values are unchanged
    assert_eq!(
        m.page(1).unwrap().boxes.media,
        Rect::new(0.0, 0.0, 612.0, 792.0)
    );
    assert!(m.epoch() >= 2);
}

#[test]
fn page_removal_through_cos_then_invalidate_pages() {
    let m = model(build(&simple_doc(3, "", &[]), ""));
    assert_eq!(m.page_count().unwrap(), 3);
    let victim = m.page_object(1).unwrap();
    m.document().remove_page(&victim).unwrap();
    m.invalidate(&[id(2)]); // the Pages node's /Kids changed
    assert_eq!(m.page_count().unwrap(), 2);
    assert_eq!(m.page(1).unwrap().id, id(5));
    m.invalidate_pages();
    assert_eq!(m.page_count().unwrap(), 2);
}

#[test]
fn hostile_structures_do_not_hang_or_panic() {
    // /Parent loop, outline loop, form kid cycle, name tree cycle, action /Next cycle.
    let mut o = simple_doc(
        1,
        "/AcroForm 4 0 R /Outlines 5 0 R /Names << /Dests 6 0 R >> /OpenAction 7 0 R /PageLabels 6 0 R",
        &[],
    );
    o[2] = "<< /Type /Page /Parent 3 0 R /MediaBox [0 0 10 10] >>".into(); // page is its own parent
    o.push("<< /Fields [8 0 R] >>".into()); // 4
    o.push("<< /First 5 0 R /Last 5 0 R >>".into()); // 5: outline root is its own first child
    o.push("<< /Kids [6 0 R] /Names [(a) 6 0 R] /Nums [0 6 0 R] >>".into()); // 6
    o.push("<< /S /GoTo /D (a) /Next 7 0 R >>".into()); // 7
    o.push("<< /T (x) /Kids [8 0 R 9 0 R] >>".into()); // 8
    o.push("<< /T (y) /Kids [8 0 R] >>".into()); // 9
    let m = model(build(&o, ""));
    let _ = m.page(0).unwrap();
    let ol = m.outlines().unwrap();
    assert!(ol.items.len() <= 1);
    let _ = m.form().unwrap();
    let _ = m.page_labels().unwrap();
    let _ = m.named_destinations().unwrap();
    let _ = m.catalog_info().unwrap();
    let _ = m.scripts().unwrap();
    let _ = m.signatures().unwrap();
    let _ = m.annotations(0).unwrap();
    assert!(!m.diagnostics().is_empty());
}

#[test]
fn orphan_widgets_are_adopted() {
    let mut o = simple_doc(1, "/AcroForm 4 0 R", &[]);
    o[2] = page_obj("/Annots [5 0 R 6 0 R]");
    o.push("<< /Fields [5 0 R] >>".into()); // 4
    o.push("<< /Subtype /Widget /FT /Tx /T (listed) /Rect [0 0 10 10] /P 3 0 R >>".into()); // 5
    o.push("<< /Subtype /Widget /FT /Tx /T (stray) /Rect [0 0 10 10] /V (x) >>".into()); // 6
    let m = model(build(&o, ""));
    let f = m.form().unwrap();
    assert!(!f.find("listed").unwrap().orphan);
    let stray = f.find("stray").unwrap();
    assert!(stray.orphan);
    assert_eq!(stray.widgets[0].page, Some(0));
    assert_eq!(stray.value, FieldValue::Text("x".into()));
    assert_eq!(f.widgets_on_page(0).len(), 2);
}
