//! End to end through GTK and CUPS on the Linux CI runner: print to the CUPS-PDF virtual
//! printer and compare the pages it produced. Skipped unless `PAPYRINE_CUPS_PRINTER` names
//! the printer and `PAPYRINE_CUPS_OUT` the directory CUPS-PDF writes into (see
//! `.github/workflows/print.yml`; the job runs under `xvfb-run`).
#![cfg(target_os = "linux")]

mod common;

use common::*;
use papyrine_core::Rect;
use papyrine_print::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn newest_pdf(dir: &Path, after: std::time::SystemTime) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "pdf"))
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t >= after)
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

#[test]
fn cups_pdf_job_has_the_expected_pages() {
    let (Ok(name), Ok(out)) = (
        std::env::var("PAPYRINE_CUPS_PRINTER"),
        std::env::var("PAPYRINE_CUPS_OUT"),
    ) else {
        eprintln!("PAPYRINE_CUPS_PRINTER / PAPYRINE_CUPS_OUT not set; skipping");
        return;
    };
    let out = PathBuf::from(out);
    let src = pages(&[
        PageDef {
            w: 612.0,
            h: 792.0,
            rotate: 0,
            content: black_rect(100.0, 100.0, 200.0, 100.0),
        },
        PageDef {
            w: 300.0,
            h: 400.0,
            rotate: 0,
            content: black_rect(20.0, 20.0, 100.0, 50.0),
        },
        PageDef {
            w: 612.0,
            h: 792.0,
            rotate: 0,
            content: black_rect(10.0, 10.0, 10.0, 10.0),
        },
    ]);
    let sheet = Sheet::new(612.0, 792.0);
    let opts = PrintOptions {
        job_name: "papyrine-ci".into(),
        pages: PageSelection::Ranges { text: "1-2".into() },
        destination: Destination::Printer { name },
        paper: Some(sheet),
        layout: Layout {
            scaling: Scaling::ShrinkOversized,
            auto_rotate: false,
            center: true,
        },
        ..PrintOptions::default()
    };
    let started = std::time::SystemTime::now() - Duration::from_secs(2);
    let outcome = print(
        &src,
        None,
        &opts,
        &WindowHandle::none(),
        &|_| {},
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(outcome, PrintOutcome::Printed { pages: 2 });

    // CUPS-PDF writes asynchronously.
    let deadline = Instant::now() + Duration::from_secs(60);
    let file = loop {
        if let Some(f) = newest_pdf(&out, started) {
            std::thread::sleep(Duration::from_secs(1)); // let the writer finish
            break f;
        }
        assert!(
            Instant::now() < deadline,
            "no CUPS-PDF output in {}",
            out.display()
        );
        std::thread::sleep(Duration::from_millis(500));
    };
    let bytes = std::fs::read(&file).unwrap();
    let sizes = page_sizes(&bytes);
    assert_eq!(sizes.len(), 2, "pages in {}", file.display());
    // Ink positions: page 1 unchanged, page 2 centred at 100 %.
    let t0 = render(&bytes, 0);
    assert_rect_close(
        ink_bbox(&t0).unwrap(),
        Rect::new(100.0, 100.0, 300.0, 200.0),
        3.0,
        "page 1",
    );
    let t1 = render(&bytes, 1);
    let (w, h) = (t1.width as f64, t1.height as f64);
    let want = Rect::new(
        (w - 300.0) / 2.0 + 20.0,
        (h - 400.0) / 2.0 + 20.0,
        (w - 300.0) / 2.0 + 120.0,
        (h - 400.0) / 2.0 + 70.0,
    );
    assert_rect_close(ink_bbox(&t1).unwrap(), want, 3.0, "page 2");
}
