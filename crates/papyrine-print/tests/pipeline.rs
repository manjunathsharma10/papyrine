//! The shared print-PDF pipeline: page subsets, annotation filtering, form values, damaged and
//! encrypted input, imposition against the layout math, and "print as image".

mod common;

use common::*;
use papyrine_core::{CancelToken, Rect};
use papyrine_cos::{
    Document, EncryptionMode, EncryptionRevision, EncryptionSpec, Permissions, PrintPermission,
    WriteOptions,
};
use papyrine_print::{
    Error, Layout, PageSelection, PrintOptions, Scaling, Sheet, build_print_pdf, impose, place_page,
};

fn build(bytes: &[u8], opts: &PrintOptions) -> papyrine_print::PrintPdf {
    build_print_pdf(bytes, None, opts, &CancelToken::new(), &|_, _, _| {}).expect("print pdf")
}

fn opts(pages: PageSelection) -> PrintOptions {
    PrintOptions {
        pages,
        ..PrintOptions::default()
    }
}

#[test]
fn page_subsets_keep_the_right_pages_in_order() {
    let src = numbered(6);
    let all = build(&src, &opts(PageSelection::All));
    assert_eq!(all.page_count, 6);
    assert_eq!(page_sizes(&all.bytes).len(), 6);
    assert!(qpdf_check(&all.bytes));

    let cur = build(&src, &opts(PageSelection::Current { index: 3 }));
    assert_eq!(cur.source_pages, vec![3]);
    assert_eq!(page_sizes(&cur.bytes), vec![(330.0, 400.0)]);

    let r = build(
        &src,
        &opts(PageSelection::Ranges {
            text: "2-3, 6".into(),
        }),
    );
    assert_eq!(r.source_pages, vec![1, 2, 5]);
    assert_eq!(
        page_sizes(&r.bytes),
        vec![(310.0, 400.0), (320.0, 400.0), (350.0, 400.0)]
    );
    assert!(qpdf_check(&r.bytes));

    let bad = build_print_pdf(
        &src,
        None,
        &opts(PageSelection::Ranges { text: "9".into() }),
        &CancelToken::new(),
        &|_, _, _| {},
    );
    assert_eq!(
        bad.unwrap_err(),
        Error::PageOutOfRange { page: 9, count: 6 }
    );
}

#[test]
fn annotations_follow_the_print_flag_and_comments_option() {
    let src = annotated();
    let page = 1; // second page of the source, first of the subset
    let sel = PageSelection::Current { index: page };

    let with = build(
        &src,
        &PrintOptions {
            print_comments: true,
            ..opts(sel.clone())
        },
    );
    let t = render(&with.bytes, 0);
    assert_eq!(at(&t, 100.0, 100.0), RED, "printable comment drawn");
    assert_eq!(
        at(&t, 250.0, 100.0),
        [255, 255, 255],
        "no /Print flag: not drawn"
    );
    assert_eq!(at(&t, 400.0, 100.0), [255, 255, 255], "hidden: not drawn");
    assert!(with.pages_with_dropped_annotations >= 1);
    assert!(qpdf_check(&with.bytes));

    let without = build(
        &src,
        &PrintOptions {
            print_comments: false,
            ..opts(sel)
        },
    );
    let t = render(&without.bytes, 0);
    assert_eq!(at(&t, 100.0, 100.0), [255, 255, 255], "comments off");
    // Page content survives either way (the page contents are shared with page 1).
    assert_eq!(at(&t, 450.0, 650.0), [0, 0, 0]);
}

#[test]
fn form_values_print_when_the_widget_prints() {
    let src = annotated();
    for comments in [true, false] {
        let p = build(
            &src,
            &PrintOptions {
                print_comments: comments,
                ..opts(PageSelection::Current { index: 1 })
            },
        );
        let t = render(&p.bytes, 0);
        assert!(
            dark_in(&t, Rect::new(50.0, 200.0, 250.0, 240.0)) > 20,
            "value text missing (comments={comments})"
        );
        assert_eq!(
            dark_in(&t, Rect::new(300.0, 200.0, 500.0, 240.0)),
            0,
            "non-printing widget leaked"
        );
    }
    // Nothing interactive survives into the print PDF.
    let p = build(&src, &opts(PageSelection::All));
    let d = Document::open_bytes(p.bytes.clone(), &Default::default()).unwrap();
    assert!(!d.has_acroform().unwrap());
    for page in d.pages().unwrap() {
        assert!(!page.dict_has("Annots").unwrap());
    }
}

#[test]
fn damaged_file_prints_as_it_displays() {
    let mut src = numbered(3);
    // Break startxref/trailer region: qpdf must reconstruct.
    src.truncate(src.len() - 20);
    let p = build(&src, &opts(PageSelection::All));
    assert_eq!(p.page_count, 3);
    assert!(qpdf_check(&p.bytes));
}

fn encrypted(print: PrintPermission, user: &str) -> Vec<u8> {
    let d = Document::open_bytes(numbered(2), &Default::default()).unwrap();
    let mut spec = EncryptionSpec::new(EncryptionRevision::R6, user, "owner-secret");
    spec.permissions = Permissions {
        print,
        ..Permissions::default()
    };
    d.write(&WriteOptions {
        encryption: EncryptionMode::Encrypt(spec),
        ..WriteOptions::default()
    })
    .unwrap()
    .into_vec()
}

#[test]
fn encryption_password_and_print_permission() {
    // Needs a password.
    let e = encrypted(PrintPermission::Full, "pw");
    let r = build_print_pdf(
        &e,
        None,
        &opts(PageSelection::All),
        &CancelToken::new(),
        &|_, _, _| {},
    );
    assert_eq!(r.unwrap_err(), Error::PasswordRequired);
    let ok = build_print_pdf(
        &e,
        Some("pw"),
        &opts(PageSelection::All),
        &CancelToken::new(),
        &|_, _, _| {},
    )
    .unwrap();
    assert_eq!(ok.page_count, 2);
    // The print PDF itself is not encrypted.
    assert!(
        Document::open_bytes(ok.bytes.clone(), &Default::default())
            .unwrap()
            .encryption()
            .unwrap()
            .is_none()
    );

    // Printing forbidden for the user password; the owner password overrides.
    let denied = encrypted(PrintPermission::None, "");
    let r = build_print_pdf(
        &denied,
        None,
        &opts(PageSelection::All),
        &CancelToken::new(),
        &|_, _, _| {},
    );
    assert_eq!(r.unwrap_err(), Error::PrintNotPermitted);
    let owner = build_print_pdf(
        &denied,
        Some("owner-secret"),
        &opts(PageSelection::All),
        &CancelToken::new(),
        &|_, _, _| {},
    );
    assert_eq!(owner.unwrap().page_count, 2);
}

#[test]
fn cancel_is_honoured_and_progress_is_monotonic() {
    let src = numbered(20);
    let token = CancelToken::new();
    token.cancel();
    let r = build_print_pdf(&src, None, &opts(PageSelection::All), &token, &|_, _, _| {});
    assert_eq!(r.unwrap_err(), Error::Cancelled);

    let seen = std::cell::RefCell::new(Vec::new());
    build_print_pdf(
        &src,
        None,
        &opts(PageSelection::All),
        &CancelToken::new(),
        &|_, d, t| {
            seen.borrow_mut().push((d, t));
        },
    )
    .unwrap();
    let seen = seen.into_inner();
    assert!(seen.len() >= 20);
    assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0));
    assert_eq!(seen.last().map(|p| p.0 == p.1), Some(true));
}

/// Imposition places the ink exactly where the layout math says, for every scaling mode,
/// rotation and the auto-rotate switch.
#[test]
fn imposition_matches_the_layout_math() {
    // Asymmetric ink so rotation errors show: a 120x40 bar near the bottom-left of the box.
    let ink = Rect::new(20.0, 30.0, 140.0, 70.0);
    let defs = [
        (400.0, 300.0, 0),   // landscape on a portrait sheet
        (612.0, 792.0, 90),  // portrait with /Rotate 90 (displays landscape)
        (300.0, 200.0, 180), // small, upside down
        (1224.0, 1584.0, 0), // oversized
    ]
    .map(|(w, h, rotate)| PageDef {
        w,
        h,
        rotate,
        content: black_rect(ink.x0, ink.y0, ink.width(), ink.height()),
    });
    let src = pages(&defs);
    let print = build(&src, &opts(PageSelection::All));
    let sheet = Sheet::new(595.0, 842.0).with_margins(36.0, 24.0, 36.0, 24.0);
    for scaling in [Scaling::Fit, Scaling::ShrinkOversized, Scaling::ActualSize] {
        for auto_rotate in [false, true] {
            for center in [true, false] {
                let layout = Layout {
                    scaling,
                    auto_rotate,
                    center,
                };
                let out = impose(&print.bytes, &sheet, &layout, &CancelToken::new()).unwrap();
                assert!(qpdf_check(&out));
                for (i, d) in defs.iter().enumerate() {
                    assert_eq!(page_sizes(&out)[i], (595.0, 842.0));
                    let pl = place_page(
                        Rect::new(0.0, 0.0, d.w, d.h),
                        d.rotate,
                        sheet.printable_area(),
                        &layout,
                    );
                    // The sheet clips (the printable area is the device's job).
                    let want = ink.transform(&pl.matrix).intersect(&Rect::new(
                        0.0,
                        0.0,
                        sheet.width,
                        sheet.height,
                    ));
                    let t = render(&out, i);
                    let Some(want) = want.filter(|r| r.width() > 2.0 && r.height() > 2.0) else {
                        assert!(ink_bbox(&t).is_none(), "ink should be off the sheet");
                        continue;
                    };
                    let got = ink_bbox(&t).expect("ink");
                    assert_rect_close(
                        got,
                        want,
                        1.6,
                        &format!("{scaling:?} auto={auto_rotate} center={center} page {i}"),
                    );
                }
            }
        }
    }
}

#[test]
fn imposition_clips_to_the_page_box() {
    // Ink far outside the crop box must not appear on the sheet.
    let objs = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>".to_string()),
        (2, "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_string()),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /CropBox [0 0 300 300] /Contents 4 0 R /Resources << >> >>".to_string(),
        ),
        (4, stream("", "0 g 10 10 50 50 re f 320 10 80 80 re f")),
    ];
    let print = build(&assemble(&objs, 1), &opts(PageSelection::All));
    let out = impose(
        &print.bytes,
        &Sheet::new(612.0, 792.0),
        &Layout {
            scaling: Scaling::ActualSize,
            auto_rotate: false,
            center: false,
        },
        &CancelToken::new(),
    )
    .unwrap();
    let t = render(&out, 0);
    let bb = ink_bbox(&t).unwrap();
    assert!(bb.x1 < 100.0, "unclipped ink: {bb:?}");
}

#[test]
fn print_as_image_matches_the_vector_render() {
    let src = annotated();
    let vector = build(&src, &opts(PageSelection::All));
    let raster = build(
        &src,
        &PrintOptions {
            as_image: true,
            image_dpi: 144,
            ..opts(PageSelection::All)
        },
    );
    assert_eq!(raster.page_count, 2);
    assert_eq!(page_sizes(&raster.bytes), page_sizes(&vector.bytes));
    assert!(qpdf_check(&raster.bytes));
    // Every page is one image and no text/vector content survives.
    let d = Document::open_bytes(raster.bytes.clone(), &Default::default()).unwrap();
    let c = d.page(1).unwrap().dict_get("Contents").unwrap();
    let body = c.stream_decoded(papyrine_cos::DecodeLevel::All).unwrap();
    assert!(String::from_utf8_lossy(body.as_slice()).contains("/Im0 Do"));

    for i in 0..2 {
        let (a, b) = (render(&vector.bytes, i), render(&raster.bytes, i));
        assert_eq!((a.width, a.height), (b.width, b.height));
        let diff: u64 = a
            .rgba
            .iter()
            .zip(&b.rgba)
            .enumerate()
            .filter(|(i, _)| i % 4 != 3)
            .map(|(_, (p, q))| p.abs_diff(*q) as u64)
            .sum();
        let mean = diff as f64 / (a.width as f64 * a.height as f64 * 3.0);
        assert!(mean < 1.5, "page {i}: mean abs diff {mean}");
    }
}
