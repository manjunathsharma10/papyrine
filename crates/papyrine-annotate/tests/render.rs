//! Pixel checks: the appearance is where the geometry says it should be, in PDFium and (as a
//! test-only oracle) Poppler.

mod common;

use common::*;
use papyrine_annotate::*;

/// Render the single-page document with every available renderer.
fn render_all(doc: &papyrine_cos::Document) -> Vec<(&'static str, Raster)> {
    let bytes = write_bytes(doc);
    qpdf_check(&bytes).unwrap();
    let mut v = vec![("pdfium", pdfium_render(bytes.clone(), 0))];
    if let Some(r) = pdftoppm(&bytes, 0) {
        v.push(("poppler", r));
    }
    v
}

fn with(cmd: AddAnnotation) -> papyrine_cos::Document {
    let doc = open(build_pdf(1));
    let mut h = history();
    run(&doc, &mut h, cmd);
    doc
}

fn expect(r: &Raster, who: &str, x: f64, y: f64, want: [u8; 3], tol: i32) {
    let got = r.at(x, y);
    assert!(
        is_near(got, want, tol),
        "{who}: pixel at ({x},{y}) is {got:?}, expected {want:?}"
    );
}

fn count(r: &Raster, x0: f64, y0: f64, x1: f64, y1: f64, f: impl Fn([u8; 3]) -> bool) -> usize {
    let mut n = 0;
    for y in (y0 as i64)..(y1 as i64) {
        for x in (x0 as i64)..(x1 as i64) {
            if f(r.at(x as f64 + 0.5, y as f64 + 0.5)) {
                n += 1;
            }
        }
    }
    n
}

fn dark(p: [u8; 3]) -> bool {
    p.iter().map(|&c| u32::from(c)).sum::<u32>() < 200
}

fn white(p: [u8; 3]) -> bool {
    p == WHITE
}

#[test]
fn baseline_page_has_text_and_nothing_else() {
    let doc = open(build_pdf(1));
    for (who, r) in render_all(&doc) {
        assert!(
            count(&r, 20.0, 355.0, 160.0, 385.0, dark) > 20,
            "{who}: text drawn"
        );
        assert_eq!(count(&r, 0.0, 0.0, 400.0, 300.0, |p| !white(p)), 0, "{who}");
    }
}

#[test]
fn highlight_is_where_the_quad_is_and_multiplies() {
    let doc = with(AddAnnotation::highlight(
        0,
        vec![Quad::from_rect(20.0, 350.0, 120.0, 380.0)],
        AnnotProps::default(),
    ));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 115.0, 352.0, [255, 255, 0], 6);
        expect(&r, who, 115.0, 340.0, WHITE, 0);
        expect(&r, who, 125.0, 352.0, WHITE, 0);
        // Multiply keeps the black text visible through the yellow.
        assert!(
            count(&r, 20.0, 355.0, 120.0, 380.0, dark) > 20,
            "{who}: text under highlight"
        );
    }
    // With 50% opacity the yellow is blended with the page.
    let doc = with(AddAnnotation::highlight(
        0,
        vec![Quad::from_rect(200.0, 200.0, 300.0, 230.0)],
        AnnotProps::default().with_opacity(0.5),
    ));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 250.0, 215.0, [255, 255, 128], 8);
    }
}

#[test]
fn slanted_quad_highlight() {
    // A rotated line of text: corners in Acrobat order.
    let q = Quad([
        [200.0, 250.0],
        [300.0, 280.0],
        [190.0, 220.0],
        [290.0, 250.0],
    ]);
    let doc = with(AddAnnotation::highlight(0, vec![q], AnnotProps::default()));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 245.0, 250.0, [255, 255, 0], 6);
        expect(&r, who, 200.0, 280.0, WHITE, 0);
        expect(&r, who, 290.0, 225.0, WHITE, 0);
    }
}

#[test]
fn underline_strikeout_squiggly_draw_their_lines() {
    let quad = vec![Quad::from_rect(200.0, 300.0, 300.0, 330.0)];
    let thick = AnnotProps::default()
        .with_width(4.0)
        .with_color(Color::rgb(0.0, 0.0, 1.0));
    // Underline: just above the bottom edge (y = 300 + w).
    let doc = with(AddAnnotation::underline(0, quad.clone(), thick.clone()));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 250.0, 304.0, [0, 0, 255], 30);
        expect(&r, who, 250.0, 315.0, WHITE, 0);
        expect(&r, who, 250.0, 295.0, WHITE, 0);
        expect(&r, who, 320.0, 304.0, WHITE, 0);
    }
    // Strike-out: through the middle.
    let doc = with(AddAnnotation::strike_out(0, quad.clone(), thick.clone()));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 250.0, 315.0, [0, 0, 255], 30);
        expect(&r, who, 250.0, 304.0, WHITE, 0);
    }
    // Squiggly: ink along the bottom, none above the text height.
    let doc = with(AddAnnotation::squiggly(0, quad, thick.with_width(1.5)));
    for (who, r) in render_all(&doc) {
        assert!(
            count(&r, 200.0, 300.0, 300.0, 308.0, |p| !white(p)) > 60,
            "{who}"
        );
        assert_eq!(
            count(&r, 200.0, 312.0, 300.0, 330.0, |p| !white(p)),
            0,
            "{who}"
        );
    }
}

#[test]
fn sticky_note_icon_sits_below_its_anchor() {
    let doc = with(AddAnnotation::sticky_note(
        0,
        100.0,
        300.0,
        AnnotProps::default().with_color(Color::rgb(1.0, 0.9, 0.2)),
    ));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 103.5, 296.5, [255, 230, 51], 25);
        expect(&r, who, 99.0, 290.0, WHITE, 0);
        expect(&r, who, 110.0, 301.5, WHITE, 0);
        assert!(
            count(&r, 100.0, 280.0, 120.0, 300.0, |p| !white(p)) > 150,
            "{who}"
        );
    }
    for icon in ["Comment", "Help", "Key", "Insert", "Paragraph"] {
        let doc = with(AddAnnotation::new(
            0,
            Geometry::Note {
                pos: [100.0, 300.0],
                icon: icon.into(),
            },
            AnnotProps::default(),
        ));
        for (who, r) in render_all(&doc) {
            assert!(
                count(&r, 100.0, 280.0, 120.0, 300.0, |p| !white(p)) > 40,
                "{who} {icon}"
            );
            assert_eq!(
                count(&r, 120.5, 270.0, 150.0, 310.0, |p| !white(p)),
                0,
                "{who} {icon}"
            );
        }
    }
}

#[test]
fn text_box_draws_fill_border_and_text() {
    let doc = with(AddAnnotation::text_box(
        0,
        [50.0, 200.0, 250.0, 260.0],
        TextStyle {
            size: 16.0,
            color: Color::rgb(0.0, 0.0, 1.0),
            ..TextStyle::default()
        },
        "Hello boxed text",
        AnnotProps::default()
            .with_color(Color::rgb(1.0, 0.0, 0.0))
            .with_width(4.0)
            .with_fill(Color::rgb(1.0, 1.0, 0.8)),
    ));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 240.0, 207.0, [255, 255, 204], 4);
        expect(&r, who, 52.0, 230.0, [255, 0, 0], 20);
        expect(&r, who, 48.0, 230.0, WHITE, 0);
        let blue = |p: [u8; 3]| p[2] > 150 && p[0] < 100 && p[1] < 100;
        assert!(
            count(&r, 56.0, 225.0, 240.0, 256.0, blue) > 60,
            "{who}: blue text"
        );
        // The text starts at the top-left padding, not elsewhere.
        assert_eq!(
            count(&r, 56.0, 205.0, 240.0, 224.0, blue),
            0,
            "{who}: single line only"
        );
    }
}

#[test]
fn text_box_wraps_and_aligns() {
    let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu";
    let mk = |align| {
        with(AddAnnotation::text_box(
            0,
            [20.0, 100.0, 180.0, 200.0],
            TextStyle {
                size: 12.0,
                align,
                ..TextStyle::default()
            },
            long,
            AnnotProps::default(),
        ))
    };
    for (who, r) in render_all(&mk(Align::Left)) {
        // Several lines, all inside the box, nothing beyond its right edge.
        let rows = (100..200)
            .filter(|y| count(&r, 20.0, f64::from(*y), 180.0, f64::from(*y) + 1.0, dark) > 0)
            .count();
        assert!(rows > 25, "{who}: {rows} ink rows");
        assert_eq!(
            count(&r, 182.0, 100.0, 400.0, 200.0, |p| !white(p)),
            0,
            "{who}"
        );
    }
    let ink_x = |r: &Raster| {
        let xs: Vec<i64> = (20..180)
            .filter(|x| count(r, *x as f64, 100.0, *x as f64 + 1.0, 200.0, dark) > 0)
            .collect();
        (*xs.first().unwrap(), *xs.last().unwrap())
    };
    let right = mk(Align::Right);
    let center = mk(Align::Center);
    for ((who, rr), (_, rc)) in render_all(&right).iter().zip(render_all(&center).iter()) {
        let (_, rmax) = ink_x(rr);
        assert!(rmax >= 170, "{who}: right aligned ends at {rmax}");
        let (cmin, cmax) = ink_x(rc);
        assert!(
            cmin > 22 && cmax < 178 || (cmin + cmax) / 2 > 80,
            "{who}: {cmin}..{cmax}"
        );
    }
}

#[test]
fn pen_strokes_and_dots() {
    let doc = with(AddAnnotation::pen(
        0,
        vec![
            vec![[50.0, 100.0], [100.0, 100.0], [150.0, 100.0]],
            vec![[300.0, 300.0]],
        ],
        AnnotProps::default()
            .with_width(6.0)
            .with_color(Color::rgb(0.0, 0.0, 1.0)),
    ));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 100.0, 100.0, [0, 0, 255], 20);
        expect(&r, who, 100.0, 108.0, WHITE, 0);
        expect(&r, who, 300.0, 300.0, [0, 0, 255], 40);
        expect(&r, who, 45.0, 100.0, WHITE, 60);
    }
}

#[test]
fn rectangle_and_oval() {
    let p = AnnotProps::default()
        .with_width(4.0)
        .with_color(Color::rgb(1.0, 0.0, 0.0))
        .with_fill(Color::rgb(0.0, 1.0, 0.0));
    let doc = with(AddAnnotation::rectangle(
        0,
        [50.0, 50.0, 150.0, 120.0],
        p.clone(),
    ));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 100.0, 85.0, [0, 255, 0], 4);
        expect(&r, who, 51.5, 85.0, [255, 0, 0], 4);
        expect(&r, who, 148.5, 85.0, [255, 0, 0], 4);
        expect(&r, who, 49.0, 85.0, WHITE, 0);
        expect(&r, who, 151.0, 85.0, WHITE, 0);
        expect(&r, who, 100.0, 121.0, WHITE, 0);
    }
    let doc = with(AddAnnotation::oval(0, [50.0, 50.0, 150.0, 120.0], p));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 100.0, 85.0, [0, 255, 0], 4);
        expect(&r, who, 52.0, 85.0, [255, 0, 0], 30);
        expect(&r, who, 55.0, 55.0, WHITE, 0);
        expect(&r, who, 145.0, 115.0, WHITE, 0);
        expect(&r, who, 100.0, 118.0, [255, 0, 0], 30);
    }
}

#[test]
fn unfilled_shape_leaves_page_visible() {
    let doc = with(AddAnnotation::rectangle(
        0,
        [10.0, 340.0, 200.0, 395.0],
        AnnotProps::default().with_width(2.0),
    ));
    for (who, r) in render_all(&doc) {
        assert!(
            count(&r, 22.0, 360.0, 190.0, 380.0, dark) > 20,
            "{who}: text visible inside"
        );
    }
}

#[test]
fn line_and_arrow_heads() {
    let doc = with(AddAnnotation::line(
        0,
        [50.0, 50.0],
        [250.0, 50.0],
        AnnotProps::default().with_width(4.0),
    ));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 150.0, 50.0, [255, 0, 0], 4);
        expect(&r, who, 150.0, 58.0, WHITE, 0);
        expect(&r, who, 254.0, 50.0, WHITE, 0);
    }
    let doc = with(AddAnnotation::arrow(
        0,
        [50.0, 50.0],
        [250.0, 50.0],
        AnnotProps::default().with_width(4.0),
    ));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 150.0, 50.0, [255, 0, 0], 4);
        expect(&r, who, 236.0, 53.0, [255, 0, 0], 4);
        expect(&r, who, 236.0, 60.0, WHITE, 0);
        expect(&r, who, 248.0, 50.0, [255, 0, 0], 4);
    }
}

#[test]
fn every_line_ending_draws_something_at_the_end() {
    use LineEnding::*;
    for e in [
        Square,
        Circle,
        Diamond,
        OpenArrow,
        ClosedArrow,
        Butt,
        ROpenArrow,
        RClosedArrow,
        Slash,
    ] {
        let doc = with(AddAnnotation::new(
            0,
            Geometry::Line {
                from: [50.0, 50.0],
                to: [250.0, 50.0],
                start: e,
                end: e,
            },
            AnnotProps::default()
                .with_width(3.0)
                .with_fill(Color::rgb(0.0, 0.0, 1.0)),
        ));
        for (who, r) in render_all(&doc) {
            for x in [50.0, 250.0] {
                let n = count(&r, x - 14.0, 33.0, x + 14.0, 67.0, |p| !white(p));
                assert!(n > 40, "{who} {e:?} at {x}: {n}");
            }
        }
    }
}

#[test]
fn opacity_applies_to_shapes_and_ink() {
    let doc = with(AddAnnotation::rectangle(
        0,
        [50.0, 50.0, 150.0, 120.0],
        AnnotProps::default()
            .with_width(0.0)
            .with_fill(Color::rgb(0.0, 0.0, 0.0))
            .with_opacity(0.5),
    ));
    for (who, r) in render_all(&doc) {
        expect(&r, who, 100.0, 85.0, [128, 128, 128], 6);
    }
}

#[test]
fn rotated_page_draws_annotations_in_page_space() {
    // A /Rotate 90 page: the annotation stays attached to the page content.
    let mut bytes = build_pdf(1);
    let s = String::from_utf8_lossy(&bytes).replace("/MediaBox", "/Rotate 90 /MediaBox");
    bytes = s.into_bytes();
    // The edit above changes offsets; let qpdf-backed open repair the xref.
    let doc = open(bytes);
    let mut h = history();
    run(
        &doc,
        &mut h,
        AddAnnotation::rectangle(
            0,
            [50.0, 50.0, 150.0, 120.0],
            AnnotProps::default()
                .with_width(0.0)
                .with_fill(Color::rgb(0.0, 0.0, 1.0)),
        ),
    );
    let bytes = write_bytes(&doc);
    let r = pdfium_render(bytes, 0);
    // Page-space (100, 85) with a 90 degree clockwise rotation lands at device x = 85, y = 100
    // measured from the top-left: raster x = 85, raster y(top) = 100 -> pdf-y = 400 - 100.
    expect(&r, "pdfium", 85.0, 400.0 - 100.0, [0, 0, 255], 4);
    expect(&r, "pdfium", 300.0, 300.0, WHITE, 0);
}
