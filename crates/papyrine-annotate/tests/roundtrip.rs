mod common;

use common::*;
use papyrine_annotate::*;
use papyrine_cos::Document;
use papyrine_model::{AnnotSubtype, Annotation, annot_flags};

fn quad() -> Vec<Quad> {
    vec![Quad::from_rect(20.0, 350.0, 120.0, 380.0)]
}

/// Apply, write, reopen and check with `check`; then undo/redo exactness.
fn exercise(cmd: AddAnnotation, check: impl Fn(&Annotation)) -> Document {
    let doc = open(build_pdf(2));
    let mut h = history();
    let before = snapshot(&doc);
    let summary = run(&doc, &mut h, cmd);
    assert_eq!(summary.affected_pages, vec![0]);
    let after = snapshot(&doc);
    let items = model_annots(roundtrip(&doc), 0);
    assert_eq!(items.len(), 1);
    check(&items[0]);
    qpdf_check(&write_bytes(&doc)).unwrap();
    h.undo(&doc).unwrap().expect("undo");
    assert_matches(&doc, &before, "after undo");
    assert!(model_annots(roundtrip(&doc), 0).is_empty());
    h.redo(&doc).unwrap().expect("redo");
    assert_matches(&doc, &after, "after redo");
    let items = model_annots(roundtrip(&doc), 0);
    check(&items[0]);
    doc
}

fn props() -> AnnotProps {
    AnnotProps::default()
        .with_author("Ada")
        .with_subject("Review")
        .with_contents("check this")
}

fn common(a: &Annotation) {
    assert_eq!(a.title.as_deref(), Some("Ada"));
    assert_eq!(a.subject.as_deref(), Some("Review"));
    assert_eq!(a.contents.as_deref(), Some("check this"));
    assert!(a.creation_date.is_some() && a.modified.is_some());
    assert!(a.has_flag(annot_flags::PRINT));
    assert!(a.has_appearance);
    assert!(a.name.as_deref().is_some_and(|n| n.len() == 36));
}

#[test]
fn highlight_round_trip() {
    exercise(
        AddAnnotation::highlight(
            0,
            quad(),
            props()
                .with_color(Color::rgb(1.0, 0.5, 0.0))
                .with_opacity(0.5),
        ),
        |a| {
            common(a);
            assert_eq!(a.subtype, AnnotSubtype::Highlight);
            assert_eq!(a.color.as_deref(), Some(&[1.0, 0.5, 0.0][..]));
            assert!((a.opacity - 0.5).abs() < 1e-9);
            assert_eq!(a.quad_points.len(), 1);
            assert_eq!(a.quad_points[0][0].x, 20.0);
            assert_eq!(a.quad_points[0][0].y, 380.0);
            assert_eq!(a.quad_points[0][3].x, 120.0);
            assert_eq!(a.quad_points[0][3].y, 350.0);
            assert_eq!(
                (a.rect.x0, a.rect.y0, a.rect.x1, a.rect.y1),
                (20.0, 350.0, 120.0, 380.0)
            );
        },
    );
}

#[test]
fn underline_strikeout_squiggly_round_trip() {
    for (cmd, st) in [
        (
            AddAnnotation::underline(0, quad(), props().with_width(2.0)),
            AnnotSubtype::Underline,
        ),
        (
            AddAnnotation::strike_out(0, quad(), props()),
            AnnotSubtype::StrikeOut,
        ),
        (
            AddAnnotation::squiggly(0, quad(), props().with_color(Color::rgb(0.0, 0.0, 1.0))),
            AnnotSubtype::Squiggly,
        ),
    ] {
        exercise(cmd, |a| {
            common(a);
            assert_eq!(a.subtype, st);
            assert_eq!(a.quad_points.len(), 1);
            assert!(
                a.rect.x0 < 20.0 && a.rect.x1 > 120.0,
                "rect covers the line width"
            );
        });
    }
}

#[test]
fn sticky_note_round_trip() {
    exercise(
        AddAnnotation::sticky_note(
            0,
            100.0,
            300.0,
            props().with_color(Color::rgb(1.0, 0.9, 0.2)),
        ),
        |a| {
            common(a);
            assert_eq!(a.subtype, AnnotSubtype::Text);
            assert_eq!(a.icon_name.as_deref(), Some("Note"));
            assert_eq!(a.open, Some(false));
            assert_eq!((a.rect.x0, a.rect.y1), (100.0, 300.0));
            assert_eq!((a.rect.width(), a.rect.height()), (20.0, 20.0));
            assert!(a.has_flag(annot_flags::NO_ZOOM) && a.has_flag(annot_flags::NO_ROTATE));
        },
    );
}

#[test]
fn text_box_round_trip() {
    let style = TextStyle {
        family: FontFamily::Serif,
        bold: true,
        size: 14.0,
        color: Color::rgb(0.0, 0.0, 1.0),
        align: Align::Center,
    };
    let doc = exercise(
        AddAnnotation::text_box(
            0,
            [50.0, 200.0, 250.0, 260.0],
            style.clone(),
            "Hello, boxed world",
            AnnotProps::default()
                .with_author("Ada")
                .with_subject("Review")
                .with_color(Color::rgb(1.0, 0.0, 0.0))
                .with_width(2.0)
                .with_fill(Color::rgb(1.0, 1.0, 0.8)),
        ),
        |a| {
            assert_eq!(a.subtype, AnnotSubtype::FreeText);
            assert_eq!(a.contents.as_deref(), Some("Hello, boxed world"));
            assert_eq!(
                a.default_appearance.as_deref(),
                Some("/TiBo 14 Tf 0 0 1 rg")
            );
            assert_eq!(a.quadding, Some(1));
            assert_eq!(a.interior_color.as_deref(), Some(&[1.0, 1.0, 0.8][..]));
            assert_eq!(a.border.as_ref().map(|b| b.width), Some(2.0));
            assert!(a.has_appearance);
        },
    );
    // /DS and /RC are written, and reading the annotation back reproduces the style.
    let a = page_annots(&doc, 0).remove(0);
    assert!(a.dict_has("DS").unwrap() && a.dict_has("RC").unwrap());
    let spec = read_spec(&a).unwrap();
    match spec.geometry {
        Geometry::TextBox { style: s, rect } => {
            assert_eq!(s, style);
            assert_eq!(rect, [50.0, 200.0, 250.0, 260.0]);
        }
        g => panic!("{g:?}"),
    }
    assert_eq!(spec.props.contents.as_deref(), Some("Hello, boxed world"));
}

#[test]
fn pen_round_trip() {
    let strokes = vec![
        vec![[10.0, 10.0], [20.0, 30.0], [40.0, 35.0], [60.0, 10.0]],
        vec![[100.0, 100.0]],
    ];
    exercise(
        AddAnnotation::pen(
            0,
            strokes.clone(),
            props()
                .with_width(3.0)
                .with_color(Color::rgb(0.0, 0.0, 0.0)),
        ),
        |a| {
            common(a);
            assert_eq!(a.subtype, AnnotSubtype::Ink);
            assert_eq!(a.ink_list.len(), 2);
            assert_eq!(a.ink_list[0].len(), 4);
            assert_eq!(a.ink_list[0][1].x, 20.0);
            assert_eq!(a.ink_list[1][0].y, 100.0);
            assert_eq!(a.border.as_ref().map(|b| b.width), Some(3.0));
            assert!(a.rect.x0 < 10.0 && a.rect.x1 > 100.0);
        },
    );
}

#[test]
fn rectangle_and_oval_round_trip() {
    for (cmd, st) in [
        (
            AddAnnotation::rectangle(
                0,
                [50.0, 50.0, 150.0, 120.0],
                props().with_width(4.0).with_fill(Color::rgb(0.0, 1.0, 0.0)),
            ),
            AnnotSubtype::Square,
        ),
        (
            AddAnnotation::oval(0, [50.0, 50.0, 150.0, 120.0], props().with_width(4.0)),
            AnnotSubtype::Circle,
        ),
    ] {
        let has_fill = st == AnnotSubtype::Square;
        exercise(cmd, |a| {
            common(a);
            assert_eq!(a.subtype, st);
            assert_eq!(
                (a.rect.x0, a.rect.y0, a.rect.x1, a.rect.y1),
                (50.0, 50.0, 150.0, 120.0)
            );
            assert_eq!(a.border.as_ref().map(|b| b.width), Some(4.0));
            assert_eq!(a.interior_color.is_some(), has_fill);
        });
    }
}

#[test]
fn line_and_arrow_round_trip() {
    exercise(
        AddAnnotation::line(0, [50.0, 50.0], [250.0, 150.0], props().with_width(2.0)),
        |a| {
            common(a);
            assert_eq!(a.subtype, AnnotSubtype::Line);
            let l = a.line.unwrap();
            assert_eq!((l[0].x, l[0].y, l[1].x, l[1].y), (50.0, 50.0, 250.0, 150.0));
            assert_eq!(a.line_endings, vec!["None", "None"]);
            assert_ne!(a.intent.as_deref(), Some("LineArrow"));
        },
    );
    exercise(
        AddAnnotation::arrow(0, [250.0, 150.0], [50.0, 50.0], props().with_width(2.0)),
        |a| {
            common(a);
            assert_eq!(a.line_endings, vec!["None", "ClosedArrow"]);
            assert_eq!(a.intent.as_deref(), Some("LineArrow"));
            assert!(a.interior_color.is_some(), "arrow head is filled");
        },
    );
}

#[test]
fn dashed_border_and_flags_round_trip() {
    let mut p = props().with_width(2.0);
    p.dash = vec![4.0, 2.0];
    p.flags = Some(4 | 64);
    exercise(
        AddAnnotation::rectangle(0, [10.0, 10.0, 60.0, 60.0], p),
        |a| {
            let b = a.border.as_ref().unwrap();
            assert_eq!(b.dash, vec![4.0, 2.0]);
            assert_eq!(b.style, papyrine_model::BorderKind::Dashed);
            assert!(a.has_flag(annot_flags::READ_ONLY));
        },
    );
}

#[test]
fn replies_round_trip_and_delete_with_parent() {
    let doc = open(build_pdf(1));
    let mut h = history();
    let s = run(
        &doc,
        &mut h,
        AddAnnotation::sticky_note(0, 100.0, 300.0, props()),
    );
    let parent = AnnotRef::id(s.created[0]);
    let s2 = run(
        &doc,
        &mut h,
        AddReply::new(
            0,
            parent.clone(),
            "agreed",
            AnnotProps::default().with_author("Bob"),
        ),
    );
    let reply_id = s2.created[0];
    let s3 = run(
        &doc,
        &mut h,
        AddReply::new(
            0,
            AnnotRef::id(reply_id),
            "thanks",
            AnnotProps::default().with_author("Ada"),
        ),
    );
    // (Writing renumbers objects, so identify annotations by their relations.)
    let model = papyrine_model::Model::new(roundtrip(&doc));
    let set = model.annotations(0).unwrap();
    assert_eq!(set.items.len(), 3);
    let p = set.items.iter().find(|a| a.in_reply_to.is_none()).unwrap();
    let replies = set.replies_to(p.id.unwrap());
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].contents.as_deref(), Some("agreed"));
    assert_eq!(replies[0].reply_type.as_deref(), Some("R"));
    assert_eq!(replies[0].title.as_deref(), Some("Bob"));
    let nested = set.replies_to(replies[0].id.unwrap());
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0].contents.as_deref(), Some("thanks"));
    let _ = s3;

    // Deleting the parent removes the whole thread; undo brings it all back exactly.
    let before = snapshot(&doc);
    run(&doc, &mut h, DeleteAnnotations::new(0, vec![parent]));
    assert!(model_annots(roundtrip(&doc), 0).is_empty());
    h.undo(&doc).unwrap().unwrap();
    assert_matches(&doc, &before, "undo delete");
    assert_eq!(model_annots(roundtrip(&doc), 0).len(), 3);
}

#[test]
fn update_changes_properties_and_regenerates_appearance() {
    let doc = open(build_pdf(1));
    let mut h = history();
    let s = run(
        &doc,
        &mut h,
        AddAnnotation::rectangle(0, [50.0, 50.0, 150.0, 120.0], props().with_width(2.0)),
    );
    let target = AnnotRef::id(s.created[0]);
    let before = snapshot(&doc);
    let patch = PropsPatch {
        set: AnnotProps::default()
            .with_color(Color::rgb(0.0, 0.0, 1.0))
            .with_opacity(0.25)
            .with_width(6.0),
        clear: vec![PropKey::Subject],
    };
    run(
        &doc,
        &mut h,
        UpdateAnnotation::new(0, target.clone(), patch),
    );
    let a = &model_annots(roundtrip(&doc), 0)[0];
    assert_eq!(a.color.as_deref(), Some(&[0.0, 0.0, 1.0][..]));
    assert!((a.opacity - 0.25).abs() < 1e-9);
    assert_eq!(a.border.as_ref().unwrap().width, 6.0);
    assert!(a.subject.is_none());
    assert_eq!(a.contents.as_deref(), Some("check this"));
    // The appearance stream object is reused, not leaked.
    assert_eq!(
        doc.object_ids().unwrap().len(),
        before.len() - 1 /* trailer */
    );
    // Move it.
    run(
        &doc,
        &mut h,
        UpdateAnnotation::new(0, target.clone(), PropsPatch::default()).with_geometry(
            Geometry::Square {
                rect: [60.0, 60.0, 200.0, 140.0],
            },
        ),
    );
    let a = &model_annots(roundtrip(&doc), 0)[0];
    assert_eq!((a.rect.x0, a.rect.x1), (60.0, 200.0));
    h.undo(&doc).unwrap().unwrap();
    h.undo(&doc).unwrap().unwrap();
    assert_matches(&doc, &before, "undo both updates");
    // Wrong type is refused and leaves the document untouched.
    let bad =
        UpdateAnnotation::new(0, target, PropsPatch::default()).with_geometry(Geometry::Circle {
            rect: [1.0, 1.0, 20.0, 20.0],
        });
    assert!(h.execute(&doc, Box::new(bad)).is_err());
    assert_matches(&doc, &before, "failed update rolled back");
}

#[test]
fn commands_rebuild_from_params() {
    let reg = registry();
    let doc = open(build_pdf(1));
    let mut h = history();
    let c = AddAnnotation::highlight(0, quad(), props());
    let record = papyrine_ops::describe_command(&c);
    let rebuilt = reg.create_from_record(&record).unwrap();
    h.execute(&doc, rebuilt).unwrap();
    let a = &model_annots(roundtrip(&doc), 0)[0];
    // Name and dates come from params, so replay reproduces them.
    assert_eq!(a.name.as_deref(), c.props.name.as_deref());
    assert_eq!(a.modified_raw.as_deref(), c.props.modified.as_deref());
}
