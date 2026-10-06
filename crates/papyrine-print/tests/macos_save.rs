//! macOS back end through PDFKit + NSPrintOperation with the "save" job disposition: no panel,
//! no progress window, nothing on screen. The saved PDF is rendered and compared with the layout
//! math. `harness = false` so everything runs on the real main thread, which AppKit needs.

#[cfg(target_os = "macos")]
mod common;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use super::common::*;
    use papyrine_core::Rect;
    use papyrine_print::*;
    use std::cell::RefCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::PathBuf;

    fn out_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("papyrine-print-mac-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{name}.pdf"))
    }

    fn save(src: &[u8], name: &str, mut opts: PrintOptions) -> (Vec<u8>, PrintOutcome, Vec<Stage>) {
        let path = out_path(name);
        let _ = std::fs::remove_file(&path);
        opts.destination = Destination::SaveToFile { path: path.clone() };
        let stages = RefCell::new(Vec::new());
        let outcome = print(
            src,
            None,
            &opts,
            &WindowHandle::none(),
            &|p| stages.borrow_mut().push(p.stage),
            &CancelToken::new(),
        )
        .expect("print");
        let bytes = std::fs::read(&path).expect("saved pdf");
        (bytes, outcome, stages.into_inner())
    }

    const INK: Rect = Rect {
        x0: 20.0,
        y0: 30.0,
        x1: 140.0,
        y1: 70.0,
    };

    fn ink_pages(dims: &[(f64, f64, i32)]) -> Vec<u8> {
        pages(
            &dims
                .iter()
                .map(|&(w, h, rotate)| PageDef {
                    w,
                    h,
                    rotate,
                    content: black_rect(INK.x0, INK.y0, INK.width(), INK.height()),
                })
                .collect::<Vec<_>>(),
        )
    }

    fn page_count_and_subset() {
        let src = numbered(6);
        let (out, outcome, stages) = save(
            &src,
            "subset",
            PrintOptions {
                pages: PageSelection::Ranges {
                    text: "2-3,6".into(),
                },
                paper: Some(Sheet::new(612.0, 792.0)),
                ..PrintOptions::default()
            },
        );
        assert_eq!(outcome, PrintOutcome::Printed { pages: 3 });
        assert_eq!(open(&out).page_count(), 3, "saved page count");
        assert!(stages.contains(&Stage::Preparing) && stages.contains(&Stage::Sending));
        assert_eq!(stages.last(), Some(&Stage::Done));
        assert!(qpdf_check(&out));
    }

    fn compare_with_layout_math() {
        let dims = [
            (400.0, 300.0, 0),
            (612.0, 792.0, 90),
            (300.0, 200.0, 0),
            (1224.0, 1584.0, 0),
        ];
        let src = ink_pages(&dims);
        let a4 = Sheet::new(595.0, 842.0);
        let sheet = papyrine_print::macos::system_sheet(&a4);
        for scaling in [Scaling::Fit, Scaling::ShrinkOversized, Scaling::ActualSize] {
            for auto_rotate in [false, true] {
                let layout = Layout {
                    scaling,
                    auto_rotate,
                    center: true,
                };
                let name = format!("layout-{scaling:?}-{auto_rotate}");
                let (out, _, _) = save(
                    &src,
                    &name,
                    PrintOptions {
                        layout,
                        paper: Some(a4),
                        ..PrintOptions::default()
                    },
                );
                let sizes = page_sizes(&out);
                assert_eq!(sizes.len(), dims.len(), "{name}: pages");
                for (i, &(w, h, rot)) in dims.iter().enumerate() {
                    let pl = place_page(
                        Rect::new(0.0, 0.0, w, h),
                        rot,
                        sheet.printable_area(),
                        &layout,
                    );
                    // The device clips to the imageable area.
                    let want = INK.transform(&pl.matrix).intersect(&sheet.printable_area());
                    let t = render(&out, i);
                    let Some(want) = want.filter(|r| r.width() > 3.0 && r.height() > 3.0) else {
                        assert!(
                            ink_bbox(&t).is_none_or(|b| b.width() < 4.0 || b.height() < 4.0),
                            "{name} p{i}: ink should be clipped away"
                        );
                        continue;
                    };
                    // PDFKit pages are the paper size (possibly turned) with ink placed in it.
                    let got = ink_bbox(&t).unwrap_or_else(|| panic!("{name} page {i}: blank"));
                    assert_eq!(
                        (sizes[i].0.round(), sizes[i].1.round()),
                        (595.0, 842.0),
                        "{name} p{i} size"
                    );
                    assert_rect_close(got, want, 2.5, &format!("{name} page {i}"));
                }
            }
        }
    }

    fn as_image_save() {
        let src = annotated();
        let (out, outcome, _) = save(
            &src,
            "image",
            PrintOptions {
                as_image: true,
                image_dpi: 150,
                pages: PageSelection::Current { index: 1 },
                paper: Some(Sheet::new(612.0, 792.0)),
                ..PrintOptions::default()
            },
        );
        assert_eq!(outcome, PrintOutcome::Printed { pages: 1 });
        let t = render(&out, 0);
        assert_eq!(
            at(&t, 100.0, 100.0),
            RED,
            "annotation survives rasterization"
        );
        assert!(
            dark_in(&t, Rect::new(50.0, 200.0, 250.0, 240.0)) > 20,
            "form value"
        );
    }

    fn annotations_in_saved_job() {
        let src = annotated();
        for comments in [true, false] {
            let (out, _, _) = save(
                &src,
                &format!("annots-{comments}"),
                PrintOptions {
                    print_comments: comments,
                    pages: PageSelection::Current { index: 1 },
                    paper: Some(Sheet::new(612.0, 792.0)),
                    ..PrintOptions::default()
                },
            );
            let t = render(&out, 0);
            let red = at(&t, 100.0, 100.0) == RED;
            assert_eq!(red, comments, "comments={comments}");
            assert_ne!(at(&t, 250.0, 100.0), [0, 0, 255], "non-printing annotation");
        }
    }

    fn cancel_before_send() {
        let token = CancelToken::new();
        token.cancel();
        let path = out_path("cancelled");
        let _ = std::fs::remove_file(&path);
        let opts = PrintOptions {
            destination: Destination::SaveToFile { path: path.clone() },
            ..PrintOptions::default()
        };
        let r = print(
            &numbered(2),
            None,
            &opts,
            &WindowHandle::none(),
            &|_| {},
            &token,
        )
        .unwrap();
        assert_eq!(r, PrintOutcome::Cancelled);
        assert!(!path.exists());
    }

    fn unknown_printer() {
        let opts = PrintOptions {
            destination: Destination::Printer {
                name: "No Such Printer 42".into(),
            },
            ..PrintOptions::default()
        };
        let r = print(
            &numbered(1),
            None,
            &opts,
            &WindowHandle::none(),
            &|_| {},
            &CancelToken::new(),
        );
        assert_eq!(r, Err(Error::NoSuchPrinter("No Such Printer 42".into())));
    }

    pub fn run() {
        let cases: [(&str, fn()); 6] = [
            ("page_count_and_subset", page_count_and_subset),
            ("compare_with_layout_math", compare_with_layout_math),
            ("as_image_save", as_image_save),
            ("annotations_in_saved_job", annotations_in_saved_job),
            ("cancel_before_send", cancel_before_send),
            ("unknown_printer", unknown_printer),
        ];
        let mut failed = 0;
        for (name, f) in cases {
            match catch_unwind(AssertUnwindSafe(f)) {
                Ok(()) => println!("test {name} ... ok"),
                Err(_) => {
                    println!("test {name} ... FAILED");
                    failed += 1;
                }
            }
        }
        println!(
            "\nmacos_save: {} passed, {failed} failed",
            cases.len() - failed
        );
        if failed > 0 {
            std::process::exit(1);
        }
    }
}
