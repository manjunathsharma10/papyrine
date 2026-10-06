//! Per-command behaviour: apply -> exact undo/redo -> write -> reopen -> qpdf --check -> render.

mod common;

use common::*;
use papyrine_cos::{DecodeLevel, Document, Object, ObjectKind};
use papyrine_fill::form::{Field, FormTree, RawValue};
use papyrine_fill::*;

fn field(doc: &Document, name: &str) -> Field {
    FormTree::load(doc)
        .unwrap()
        .by_name(name)
        .unwrap_or_else(|| panic!("field {name}"))
        .clone()
}

fn ap_stream(w: &Object, state: Option<&str>) -> Option<Object> {
    let ap = w.dict_get("AP").ok()?;
    if ap.kind().ok()? != ObjectKind::Dictionary {
        return None;
    }
    let n = ap.dict_get("N").ok()?;
    match (n.kind().ok()?, state) {
        (ObjectKind::Stream, None) => Some(n),
        (ObjectKind::Dictionary, Some(s)) => {
            let v = n.dict_get(s).ok()?;
            (v.kind().ok()? == ObjectKind::Stream).then_some(v)
        }
        _ => None,
    }
}

fn ap_text(w: &Object, state: Option<&str>) -> String {
    let s = ap_stream(w, state).expect("appearance stream");
    String::from_utf8_lossy(
        s.stream_decoded(DecodeLevel::Generalized)
            .unwrap()
            .as_slice(),
    )
    .into_owned()
}

fn value_text(f: &Field) -> String {
    f.value.as_text().unwrap_or("").to_string()
}

#[test]
fn sample_form_loads() {
    let doc = open(sample_form());
    let names: Vec<String> = list_fields(&doc)
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    for n in [
        "name", "comb", "notes", "pwd", "agree", "bare", "pick", "country", "langs", "qty",
        "total", "date",
    ] {
        assert!(names.contains(&n.to_string()), "{n} in {names:?}");
    }
    let st = form_status(&doc).unwrap();
    assert!(st.has_form && !st.dynamic_xfa);
    assert!(st.skipped_scripts.is_empty(), "{:?}", st.skipped_scripts);
    assert!(st.native_scripts >= 8);
}

#[test]
fn set_text_round_trip_undo_and_render() {
    let doc = open(sample_form());
    let mut h = history();
    run(&doc, &mut h, SetTextValue::new("name", "Alice (Smith)"));
    for d in [&doc, &roundtrip(&doc)] {
        let f = field(d, "name");
        assert_eq!(value_text(&f), "Alice (Smith)");
        let c = ap_text(&f.widgets[0].obj, None);
        assert!(c.contains("/Tx BMC"), "{c}");
        assert!(c.contains("/Helv 12 Tf"), "{c}");
        assert!(c.contains("(Alice \\(Smith\\))Tj"), "{c}");
    }
    let bytes = write(&doc);
    qpdf_check(&bytes, "set_text");
    let t = render_page(&bytes, 0, 1584);
    let scale = 1584.0 / 792.0;
    assert!(
        ink_in(&t, scale, 792.0, [52.0, 702.0, 248.0, 718.0]) > 150,
        "text ink in the field"
    );
}

#[test]
fn comb_multiline_password_and_alignment() {
    let doc = open(sample_form());
    let mut h = history();
    run(&doc, &mut h, SetTextValue::new("comb", "AB12"));
    let c = ap_text(&field(&doc, "comb").widgets[0].obj, None);
    assert_eq!(c.matches(")Tj").count(), 4, "{c}");
    let long = "The quick brown fox jumps over the lazy dog. ".repeat(4);
    run(&doc, &mut h, SetTextValue::new("notes", long.trim()));
    let c = ap_text(&field(&doc, "notes").widgets[0].obj, None);
    assert!(c.matches(")Tj").count() >= 3, "wrapped lines: {c}");
    run(
        &doc,
        &mut h,
        SetTextValue::new("notes", "line one\nline two"),
    );
    assert_eq!(value_text(&field(&doc, "notes")), "line one\rline two");
    run(&doc, &mut h, SetTextValue::new("pwd", "secret"));
    let c = ap_text(&field(&doc, "pwd").widgets[0].obj, None);
    assert!(c.contains("(******)Tj"), "{c}");
    assert_eq!(value_text(&field(&doc, "pwd")), "secret");
    run(&doc, &mut h, SetTextValue::new("centered", "mid"));
    run(&doc, &mut h, SetTextValue::new("right", "end"));
    let x_of = |name: &str| -> f64 {
        let c = ap_text(&field(&doc, name).widgets[0].obj, None);
        let td = c.lines().find(|l| l.ends_with(" Td")).unwrap().to_string();
        td.split_whitespace().next().unwrap().parse().unwrap()
    };
    let (cx, rx) = (x_of("centered"), x_of("right"));
    assert!(cx > 70.0 && cx < 130.0, "centered x {cx}");
    assert!(rx > 150.0, "right-aligned x {rx}");
    qpdf_check(&write(&doc), "comb/multiline");
}

#[test]
fn text_limits_and_read_only() {
    let doc = open(sample_form());
    let mut h = history();
    let e = h
        .execute(&doc, Box::new(SetTextValue::new("comb", "123456789")))
        .unwrap_err();
    assert!(e.to_string().contains("MaxLen"), "{e}");
    let e = h
        .execute(&doc, Box::new(SetTextValue::new("nope", "x")))
        .unwrap_err();
    assert!(e.to_string().contains("no such field"), "{e}");
    let e = h
        .execute(&doc, Box::new(SetTextValue::new("agree", "x")))
        .unwrap_err();
    assert!(e.to_string().contains("not a text field"), "{e}");
    assert!(!h.can_undo(), "failed commands leave no history");
}

#[test]
fn number_format_validation_and_calculation_end_to_end() {
    let doc = open(sample_form());
    let mut h = history();
    // Keystroke validation refuses letters on commit.
    let pf = preflight_text(&doc, &"qty".into(), "12a").unwrap();
    assert!(!pf.accepted && !pf.alerts.is_empty());
    let e = h
        .execute(&doc, Box::new(SetTextValue::new("qty", "12a")))
        .unwrap_err();
    assert!(e.to_string().contains("rejected"), "{e}");
    // Validate: price must be 0..=1000.
    let e = h
        .execute(&doc, Box::new(SetTextValue::new("price", "2000")))
        .unwrap_err();
    assert!(e.to_string().contains("less than or equal to 1000"), "{e}");

    let pf = preflight_text(&doc, &"qty".into(), "1234.5").unwrap();
    assert!(pf.accepted);
    assert_eq!(pf.display, "1,234.50");
    assert_eq!(pf.calculated.len(), 1);
    assert_eq!(pf.calculated[0].2, "1,234.50");

    run(&doc, &mut h, SetTextValue::new("qty", "1234.5"));
    run(&doc, &mut h, SetTextValue::new("price", "10"));
    for d in [&doc, &roundtrip(&doc)] {
        let q = field(d, "qty");
        assert_eq!(value_text(&q), "1234.5", "raw value stays unformatted");
        assert!(ap_text(&q.widgets[0].obj, None).contains("(1,234.50)Tj"));
        let p = field(d, "price");
        assert!(ap_text(&p.widgets[0].obj, None).contains("($10.00)Tj"));
        let t = field(d, "total");
        assert_eq!(value_text(&t), "1244.5", "calculated in /CO order");
        assert!(ap_text(&t.widgets[0].obj, None).contains("(1,244.50)Tj"));
    }
    // Date format on the display only.
    run(&doc, &mut h, SetTextValue::new("date", "03/05/2024"));
    let d = field(&doc, "date");
    assert_eq!(value_text(&d), "03/05/2024");
    assert!(
        ap_text(&d.widgets[0].obj, None).contains("(5 Mar 2024)Tj"),
        "{}",
        ap_text(&d.widgets[0].obj, None)
    );
    let e = h
        .execute(&doc, Box::new(SetTextValue::new("date", "13/45/2024")))
        .unwrap_err();
    assert!(e.to_string().contains("rejected"), "{e}");
    qpdf_check(&write(&doc), "af");
}

#[test]
fn keystroke_filter_live() {
    let doc = open(sample_form());
    let k = keystroke_filter(&doc, &"qty".into(), "12", "x", (2, 2)).unwrap();
    assert!(!k.accepted);
    let k = keystroke_filter(&doc, &"qty".into(), "12", "3", (2, 2)).unwrap();
    assert!(k.accepted);
    let k = keystroke_filter(&doc, &"name".into(), "12", "x", (2, 2)).unwrap();
    assert!(k.accepted, "no script: everything goes");
}

#[test]
fn checkbox_toggle_with_and_without_appearances() {
    let doc = open(sample_form());
    let mut h = history();
    assert_eq!(
        field(&doc, "agree").widgets[0].appearance_state.as_deref(),
        Some("Yes")
    );
    run(&doc, &mut h, ToggleCheckbox::new("agree", None));
    let f = field(&doc, "agree");
    assert_eq!(f.value, RawValue::Name("Off".into()));
    assert_eq!(f.widgets[0].appearance_state.as_deref(), Some("Off"));
    run(&doc, &mut h, ToggleCheckbox::new("agree", Some(true)));
    let f = field(&doc, "agree");
    assert_eq!(f.value, RawValue::Name("Yes".into()));
    assert_eq!(f.widgets[0].appearance_state.as_deref(), Some("Yes"));

    // No appearances at all: generated with ZapfDingbats.
    run(&doc, &mut h, ToggleCheckbox::new("bare", Some(true)));
    for d in [&doc, &roundtrip(&doc)] {
        let f = field(d, "bare");
        assert_eq!(f.value, RawValue::Name("Yes".into()));
        assert_eq!(f.widgets[0].appearance_state.as_deref(), Some("Yes"));
        let on = ap_text(&f.widgets[0].obj, Some("Yes"));
        assert!(on.contains("/ZaDb") && on.contains("(8)Tj"), "{on}");
        let off = ap_text(&f.widgets[0].obj, Some("Off"));
        assert!(!off.contains("Tj"), "{off}");
    }
    let bytes = write(&doc);
    qpdf_check(&bytes, "checkbox");
    let t = render_page(&bytes, 0, 1584);
    assert!(ink_in(&t, 2.0, 792.0, [331.0, 601.0, 343.0, 613.0]) > 20);
}

#[test]
fn radio_group_semantics() {
    let doc = open(sample_form());
    let mut h = history();
    run(&doc, &mut h, SelectRadio::by_export("pick", "y"));
    let f = field(&doc, "pick");
    assert_eq!(f.value, RawValue::Name("y".into()));
    let st: Vec<_> = f
        .widgets
        .iter()
        .map(|w| w.appearance_state.clone().unwrap())
        .collect();
    assert_eq!(st, ["Off", "y", "Off"]);
    run(&doc, &mut h, SelectRadio::by_widget("pick", 2));
    let f = field(&doc, "pick");
    assert_eq!(f.value, RawValue::Name("z".into()));
    let st: Vec<_> = f
        .widgets
        .iter()
        .map(|w| w.appearance_state.clone().unwrap())
        .collect();
    assert_eq!(st, ["Off", "Off", "z"]);
    // Clicking the selected button again clears the group (NoToggleToOff is not set).
    run(&doc, &mut h, SelectRadio::by_widget("pick", 2));
    assert_eq!(field(&doc, "pick").value, RawValue::Name("Off".into()));
    let e = h
        .execute(&doc, Box::new(SelectRadio::by_export("pick", "q")))
        .unwrap_err();
    assert!(e.to_string().contains("no button"), "{e}");
    run(&doc, &mut h, SelectRadio::by_widget("pick", 0));
    for d in [&doc, &roundtrip(&doc)] {
        let f = field(d, "pick");
        assert_eq!(f.value, RawValue::Name("x".into()));
        assert_eq!(f.widgets[0].appearance_state.as_deref(), Some("x"));
    }
    qpdf_check(&write(&doc), "radio");
}

#[test]
fn combo_and_list_boxes() {
    let doc = open(sample_form());
    let mut h = history();
    // Export value and display text both work; the display shows the option's text.
    run(&doc, &mut h, SetChoice::new("country", vec!["it".into()]));
    let f = field(&doc, "country");
    assert_eq!(value_text(&f), "it");
    assert!(ap_text(&f.widgets[0].obj, None).contains("(Italy)Tj"));
    run(
        &doc,
        &mut h,
        SetChoice::new("country", vec!["Germany".into()]),
    );
    assert_eq!(value_text(&field(&doc, "country")), "Germany");
    let e = h
        .execute(
            &doc,
            Box::new(SetChoice::new("country", vec!["Mars".into()])),
        )
        .unwrap_err();
    assert!(e.to_string().contains("not an option"), "{e}");
    run(
        &doc,
        &mut h,
        SetChoice::new("editable", vec!["anything goes".into()]),
    );
    assert!(ap_text(&field(&doc, "editable").widgets[0].obj, None).contains("(anything goes)Tj"));

    // Multi-select list: V is an array, /I the sorted indices, TI scrolls to the selection.
    run(
        &doc,
        &mut h,
        SetChoice::new("langs", vec!["Go".into(), "Ada".into(), "C".into()]),
    );
    for d in [&doc, &roundtrip(&doc)] {
        let f = field(d, "langs");
        assert_eq!(
            f.value,
            RawValue::List(vec!["Ada".into(), "C".into(), "Go".into()])
        );
        assert_eq!(f.selected_indices, vec![0, 2, 6]);
        let c = ap_text(&f.widgets[0].obj, None);
        assert!(c.contains("(Ada)Tj") && c.contains(".6 .7569"), "{c}");
    }
    run(&doc, &mut h, SetChoice::new("langs", vec![]));
    assert_eq!(field(&doc, "langs").value, RawValue::None);
    let bytes = write(&doc);
    qpdf_check(&bytes, "choice");
}

#[test]
fn reset_form_restores_defaults() {
    let doc = open(sample_form());
    let mut h = history();
    run(&doc, &mut h, SetTextValue::new("name", "Bob"));
    run(&doc, &mut h, SetTextValue::new("qty", "5"));
    run(&doc, &mut h, SelectRadio::by_export("pick", "x"));
    run(&doc, &mut h, ToggleCheckbox::new("agree", Some(false)));
    run(&doc, &mut h, ResetForm::all());
    for d in [&doc, &roundtrip(&doc)] {
        assert_eq!(value_text(&field(d, "name")), "");
        assert_eq!(value_text(&field(d, "qty")), "");
        assert_eq!(field(d, "pick").value, RawValue::Name("Off".into()));
        let f = field(d, "pick");
        assert!(
            f.widgets
                .iter()
                .all(|w| w.appearance_state.as_deref() == Some("Off"))
        );
        assert!(!ap_text(&field(d, "name").widgets[0].obj, None).contains("Tj"));
    }
    // Excluding a field keeps it.
    run(&doc, &mut h, SetTextValue::new("name", "Carol"));
    run(
        &doc,
        &mut h,
        ResetForm {
            fields: Some(vec!["name".into()]),
            exclude: true,
        },
    );
    assert_eq!(value_text(&field(&doc, "name")), "Carol");
    qpdf_check(&write(&doc), "reset");
}

#[test]
fn registry_rebuilds_every_command() {
    let mut reg = papyrine_ops::CommandRegistry::with_builtin();
    register(&mut reg);
    let doc = open(sample_form());
    let mut h = history();
    let cmds: Vec<Box<dyn papyrine_ops::Command>> = vec![
        Box::new(SetTextValue::new("name", "Zed")),
        Box::new(ToggleCheckbox::new("agree", Some(false))),
        Box::new(SelectRadio::by_export("pick", "z")),
        Box::new(SetChoice::new("country", vec!["France".into()])),
        Box::new(ResetForm::all()),
        Box::new(RegenerateAppearances::default()),
        Box::new(AddFlatText::new(0, 100.0, 300.0, "flat")),
        Box::new(AddMark::new(0, MarkKind::Check, 100.0, 100.0)),
    ];
    for c in cmds {
        let rec = papyrine_ops::describe_command(c.as_ref());
        let rebuilt = reg
            .create_from_record(&rec)
            .unwrap_or_else(|e| panic!("{rec}: {e}"));
        assert_eq!(rebuilt.params(), c.params());
        h.execute(&doc, rebuilt)
            .unwrap_or_else(|e| panic!("{rec}: {e}"));
    }
    qpdf_check(&write(&doc), "registry");
}
