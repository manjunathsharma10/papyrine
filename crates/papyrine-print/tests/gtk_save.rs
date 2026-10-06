//! The GTK back end's display-free paths. Compiled on Linux, and on any unix with
//! `--features force-gtk` so the module can be exercised from a Mac.
#![cfg(any(target_os = "linux", all(unix, feature = "force-gtk")))]

mod common;

use common::*;
use papyrine_core::Rect;
use papyrine_print::gtk;
use papyrine_print::*;

fn prepared(src: &[u8], opts: &PrintOptions) -> PrintPdf {
    prepare(src, None, opts, &|_| {}, &CancelToken::new()).unwrap()
}

#[test]
fn save_to_file_lays_pages_out_on_the_requested_paper() {
    let ink = Rect::new(20.0, 30.0, 140.0, 70.0);
    let src = pages(&[
        PageDef {
            w: 400.0,
            h: 300.0,
            rotate: 0,
            content: black_rect(20.0, 30.0, 120.0, 40.0),
        },
        PageDef {
            w: 612.0,
            h: 792.0,
            rotate: 0,
            content: black_rect(20.0, 30.0, 120.0, 40.0),
        },
    ]);
    let dir = std::env::temp_dir().join(format!("papyrine-gtk-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("job.pdf");
    let sheet = Sheet::new(595.0, 842.0).with_margins(36.0, 24.0, 36.0, 24.0);
    let opts = PrintOptions {
        destination: Destination::SaveToFile { path: path.clone() },
        paper: Some(sheet),
        layout: Layout {
            scaling: Scaling::Fit,
            auto_rotate: true,
            center: true,
        },
        ..PrintOptions::default()
    };
    let pdf = prepared(&src, &opts);
    let out = gtk::print(
        &pdf,
        &opts,
        &WindowHandle::none(),
        &LocalImposer,
        &|_| {},
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(out, PrintOutcome::Printed { pages: 2 });
    let bytes = std::fs::read(&path).unwrap();
    assert!(qpdf_check(&bytes));
    assert_eq!(page_sizes(&bytes), vec![(595.0, 842.0); 2]);
    for (i, (w, h)) in [(400.0, 300.0), (612.0, 792.0)].into_iter().enumerate() {
        let pl = place_page(
            Rect::new(0.0, 0.0, w, h),
            0,
            sheet.printable_area(),
            &opts.layout,
        );
        let got = ink_bbox(&render(&bytes, i)).unwrap();
        assert_rect_close(got, ink.transform(&pl.matrix), 1.6, &format!("page {i}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_before_the_job_leaves_nothing_behind() {
    let src = numbered(1);
    let path = std::env::temp_dir().join(format!("papyrine-gtk-cancel-{}.pdf", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let opts = PrintOptions {
        destination: Destination::SaveToFile { path: path.clone() },
        ..PrintOptions::default()
    };
    let pdf = prepared(&src, &opts);
    let token = CancelToken::new();
    token.cancel();
    let r = print_prepared(
        &pdf,
        &opts,
        &WindowHandle::none(),
        &LocalImposer,
        &|_| {},
        &token,
    );
    assert_eq!(r, Ok(PrintOutcome::Cancelled));
    assert!(!path.exists());
}

/// Without GTK (a CI container, a Mac) loading fails with a typed error instead of crashing.
#[cfg(not(target_os = "linux"))]
#[test]
fn missing_gtk_is_a_typed_error() {
    let opts = PrintOptions {
        destination: Destination::Printer { name: "x".into() },
        ..PrintOptions::default()
    };
    let pdf = prepared(&numbered(1), &opts);
    let r = gtk::print(
        &pdf,
        &opts,
        &WindowHandle::none(),
        &LocalImposer,
        &|_| {},
        &CancelToken::new(),
    );
    assert!(matches!(r, Err(Error::Platform(_))), "{r:?}");
}
