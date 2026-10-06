//! Writes a page with every annotation type to $PAPYRINE_GALLERY (for eyeballing in a viewer).

mod common;

use common::*;
use papyrine_annotate::*;

#[test]
fn gallery() {
    let Some(out) = std::env::var_os("PAPYRINE_GALLERY") else {
        return;
    };
    let doc = open(build_pdf(1));
    let mut h = history();
    let p = || AnnotProps::default().with_author("Ada");
    let cmds: Vec<AddAnnotation> = vec![
        AddAnnotation::highlight(0, vec![Quad::from_rect(20.0, 350.0, 120.0, 380.0)], p()),
        AddAnnotation::underline(
            0,
            vec![Quad::from_rect(20.0, 300.0, 120.0, 330.0)],
            p().with_width(1.5),
        ),
        AddAnnotation::strike_out(
            0,
            vec![Quad::from_rect(140.0, 300.0, 240.0, 330.0)],
            p().with_width(1.5),
        ),
        AddAnnotation::squiggly(
            0,
            vec![Quad::from_rect(260.0, 300.0, 360.0, 330.0)],
            p().with_width(1.0),
        ),
        AddAnnotation::sticky_note(0, 20.0, 280.0, p()),
        AddAnnotation::new(
            0,
            Geometry::Note {
                pos: [50.0, 280.0],
                icon: "Comment".into(),
            },
            p(),
        ),
        AddAnnotation::new(
            0,
            Geometry::Note {
                pos: [80.0, 280.0],
                icon: "Help".into(),
            },
            p(),
        ),
        AddAnnotation::new(
            0,
            Geometry::Note {
                pos: [110.0, 280.0],
                icon: "Key".into(),
            },
            p(),
        ),
        AddAnnotation::new(
            0,
            Geometry::Note {
                pos: [140.0, 280.0],
                icon: "Insert".into(),
            },
            p(),
        ),
        AddAnnotation::text_box(
            0,
            [180.0, 200.0, 380.0, 280.0],
            TextStyle {
                size: 12.0,
                ..TextStyle::default()
            },
            "A text box with Привет мир, Γειά σου κόσμε and fi ligatures AVATAR.",
            p().with_width(1.0).with_fill(Color::rgb(1.0, 1.0, 0.85)),
        ),
        AddAnnotation::pen(
            0,
            vec![vec![
                [20.0, 200.0],
                [40.0, 240.0],
                [60.0, 210.0],
                [80.0, 250.0],
                [100.0, 200.0],
            ]],
            p().with_width(3.0).with_color(Color::rgb(0.0, 0.2, 0.9)),
        ),
        AddAnnotation::rectangle(
            0,
            [20.0, 120.0, 100.0, 180.0],
            p().with_width(3.0).with_fill(Color::rgb(0.8, 1.0, 0.8)),
        ),
        AddAnnotation::oval(0, [120.0, 120.0, 200.0, 180.0], p().with_width(3.0)),
        AddAnnotation::line(0, [220.0, 130.0], [300.0, 180.0], p().with_width(2.0)),
        AddAnnotation::arrow(0, [320.0, 130.0], [380.0, 180.0], p().with_width(2.0)),
    ];
    for c in cmds {
        run(&doc, &mut h, c);
    }
    std::fs::write(
        std::path::Path::new(&out).join("gallery.pdf"),
        write_bytes(&doc),
    )
    .unwrap();
}
