//! macOS back end: PDFKit's `printOperation(for:scalingMode:autoRotate:)` run through
//! `NSPrintOperation` (vector output, native dialog and preview).
//!
//! Everything here must run on the main thread; the host dispatches there (Tauri
//! `run_on_main_thread`). Non-dialog destinations never show a panel, which is how the tests run.

use crate::layout::Scaling;
use crate::{
    CancelToken, Destination, Error, PrintOptions, PrintOutcome, PrintPdf, PrintProgress, Result,
    Sheet, Stage, WindowHandle, report,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSPrintInfo, NSPrintJobSavingURL, NSPrintSaveJob, NSPrinter};
use objc2_foundation::{NSCopying, NSData, NSMutableDictionary, NSSize, NSString, NSURL};
use objc2_pdf_kit::{PDFDocument, PDFPrintScalingMode};

fn scaling_mode(s: Scaling) -> PDFPrintScalingMode {
    match s {
        Scaling::Fit => PDFPrintScalingMode::PageScaleToFit,
        Scaling::ShrinkOversized => PDFPrintScalingMode::PageScaleDownToFit,
        Scaling::ActualSize => PDFPrintScalingMode::PageScaleNone,
    }
}

/// Only the paper size is ours to choose: PDFKit scales into the *printer's* imageable area, so
/// the margins of a [`Sheet`] are ignored here (see [`system_sheet`]).
fn apply_sheet(info: &NSPrintInfo, sheet: &Sheet) {
    info.setPaperSize(NSSize::new(sheet.width, sheet.height));
}

/// `paper` with the margins macOS will actually use for it (the imageable area of the default
/// printer, or of the built-in PDF printer when none is configured). This is the `Sheet` the
/// layout math must be given to predict PDFKit's output.
pub fn system_sheet(paper: &Sheet) -> Sheet {
    let info = NSPrintInfo::new();
    apply_sheet(&info, paper);
    let b = info.imageablePageBounds();
    let size = info.paperSize();
    Sheet {
        width: size.width,
        height: size.height,
        margin_left: b.origin.x,
        margin_bottom: b.origin.y,
        margin_right: size.width - b.origin.x - b.size.width,
        margin_top: size.height - b.origin.y - b.size.height,
    }
}

pub fn print(
    pdf: &PrintPdf,
    opts: &PrintOptions,
    _window: &WindowHandle,
    progress: &dyn Fn(PrintProgress),
    cancel: &CancelToken,
) -> Result<PrintOutcome> {
    let mtm = MainThreadMarker::new().ok_or(Error::NotMainThread)?;
    let data = NSData::with_bytes(&pdf.bytes);
    // SAFETY: `data` is a valid NSData; PDFKit copies what it needs.
    let doc = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }
        .ok_or_else(|| Error::Platform("PDFKit rejected the print PDF".into()))?;

    let interactive = matches!(opts.destination, Destination::Dialog);
    let info: Retained<NSPrintInfo> = if interactive {
        // A private copy of the user's defaults so we never mutate the shared object.
        // SAFETY: `dictionary` returns the live attribute dictionary of `sharedPrintInfo`;
        // `initWithDictionary` copies it.
        unsafe {
            let shared = NSPrintInfo::sharedPrintInfo();
            NSPrintInfo::initWithDictionary(NSPrintInfo::alloc(), &shared.dictionary())
        }
    } else {
        NSPrintInfo::new()
    };
    match &opts.destination {
        Destination::Dialog => {}
        Destination::Printer { name } => {
            let printer = NSPrinter::printerWithName(&NSString::from_str(name))
                .ok_or_else(|| Error::NoSuchPrinter(name.clone()))?;
            info.setPrinter(&printer);
            apply_sheet(&info, &opts.paper.unwrap_or(Sheet::LETTER));
        }
        Destination::SaveToFile { path } => {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            // SAFETY: the dictionary is the print info's own; key and value types match what
            // AppKit documents for `NSPrintJobSavingURL` (an NSURL).
            unsafe {
                info.setJobDisposition(NSPrintSaveJob);
                let dict: Retained<NSMutableDictionary<NSString, AnyObject>> = info.dictionary();
                dict.setObject_forKey(
                    &*url,
                    ProtocolObject::<dyn NSCopying>::from_ref(NSPrintJobSavingURL),
                );
            }
            apply_sheet(&info, &opts.paper.unwrap_or(Sheet::LETTER));
        }
    }

    // SAFETY: main thread (marker), valid document and print info.
    let op = unsafe {
        doc.printOperationForPrintInfo_scalingMode_autoRotate(
            Some(&info),
            scaling_mode(opts.layout.scaling),
            opts.layout.auto_rotate,
            mtm,
        )
    }
    .ok_or_else(|| Error::Platform("PDFKit could not create a print operation".into()))?;
    op.setJobTitle(Some(&NSString::from_str(&opts.job_name)));
    op.setShowsPrintPanel(interactive);
    op.setShowsProgressPanel(interactive);

    let pages = pdf.page_count as u64;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    report(
        progress,
        if interactive {
            Stage::AwaitingDialog
        } else {
            Stage::Sending
        },
        0,
        pages,
    );
    // Blocks (application modal) until the user prints or cancels; the progress panel is
    // AppKit's own.
    let ok = op.runOperation();
    if !ok {
        return match &opts.destination {
            Destination::Dialog => Ok(PrintOutcome::Cancelled),
            _ => Err(Error::JobFailed("NSPrintOperation reported failure".into())),
        };
    }
    if let Destination::SaveToFile { path } = &opts.destination
        && !path.exists()
    {
        return Err(Error::JobFailed(format!(
            "no output was written to {}",
            path.display()
        )));
    }
    Ok(PrintOutcome::Printed {
        pages: pdf.page_count,
    })
}
