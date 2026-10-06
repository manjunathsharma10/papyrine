//! Native printing for Papyrine (ARCHITECTURE section 11.2, ADR-025, ADR-028).
//!
//! Printing never uses the webview. The flow is:
//!
//! 1. [`build_print_pdf`] (engine side, where qpdf parses the document) produces a normalized
//!    **print PDF**: the page subset, annotations filtered by the `/Print` flag and the "print
//!    comments" option, form appearances baked in, optionally rasterized ("print as image").
//! 2. [`print_prepared`] (host side, main thread) hands that PDF to the OS:
//!    * macOS: `PDFDocument.printOperation` through `NSPrintOperation` (vector, native dialog);
//!    * Linux: `GtkPrintUnixDialog` + `GtkPrintJob` with the PDF as source file (to CUPS);
//!    * Windows: not in v0.1 (ADR-028); returns [`Error::Unsupported`].
//! 3. [`print`] does both for callers that hold the document bytes in-process.
//!
//! Scaling, rotation and centering are the pure functions in [`layout`]; macOS delegates the
//! same choices to PDFKit's scaling modes, Linux applies them with [`impose`] once the dialog
//! has said which paper was chosen.

pub mod layout;
mod prepare;
mod range;
mod raster;

#[cfg(any(target_os = "linux", all(unix, feature = "force-gtk")))]
pub mod gtk;
#[cfg(target_os = "macos")]
pub mod macos;

pub use layout::{Layout, Placement, Scaling, Sheet, place_page};
pub use papyrine_core::CancelToken;
pub use prepare::{PrintPdf, build_print_pdf, impose};
pub use range::{PageSelection, parse_ranges};
pub use raster::rasterize;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("the document needs a password")]
    PasswordRequired,
    #[error("the document's permissions do not allow printing")]
    PrintNotPermitted,
    #[error("no pages selected")]
    EmptySelection,
    #[error("page {page} is outside the document ({count} pages)")]
    PageOutOfRange { page: usize, count: usize },
    #[error("cannot read page range {0:?}")]
    BadRange(String),
    #[error("PDF error: {0}")]
    Pdf(String),
    #[error("render error: {0}")]
    Render(String),
    #[error("printing must run on the main (UI) thread")]
    NotMainThread,
    #[error("print system error: {0}")]
    Platform(String),
    #[error("no such printer: {0}")]
    NoSuchPrinter(String),
    #[error("print job failed: {0}")]
    JobFailed(String),
    #[error("printing is not available here: {0}")]
    Unsupported(&'static str),
    #[error("cancelled")]
    Cancelled,
}

/// Where the job goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Destination {
    /// The native print dialog (the only mode a user ever sees).
    #[default]
    Dialog,
    /// No dialog: send to the named printer with default settings (tests, "quick print").
    Printer { name: String },
    /// No dialog: write the final print PDF to this path (macOS `NSPrintSaveJob`; on Linux the
    /// imposed PDF is written directly). The non-interactive path used by tests.
    SaveToFile { path: PathBuf },
}

/// What the user asked for in Papyrine's own print options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrintOptions {
    /// Shown in the print queue.
    pub job_name: String,
    pub pages: PageSelection,
    pub layout: Layout,
    /// Print comment annotations that carry the `/Print` flag. Form values always print.
    pub print_comments: bool,
    /// Rasterize with PDFium instead of sending vector PDF.
    pub as_image: bool,
    /// Resolution for [`as_image`](Self::as_image) (72..=1200).
    pub image_dpi: u32,
    pub destination: Destination,
    /// Paper to use when `destination` is not [`Destination::Dialog`] (the dialog decides
    /// otherwise). `None` is US Letter with no margins.
    pub paper: Option<Sheet>,
}

impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions {
            job_name: "Papyrine".into(),
            pages: PageSelection::All,
            layout: Layout::default(),
            print_comments: true,
            as_image: false,
            image_dpi: 300,
            destination: Destination::Dialog,
            paper: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Building the print PDF.
    Preparing,
    /// Rasterizing pages ("print as image").
    Rendering,
    /// The native dialog is up.
    AwaitingDialog,
    /// The job is being handed to the print system.
    Sending,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrintProgress {
    pub stage: Stage,
    pub done: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrintOutcome {
    /// Handed to the print system (or saved); `pages` is the number of pages in the job.
    Printed { pages: usize },
    /// The user dismissed the dialog, or the job was cancelled before it was sent.
    Cancelled,
}

/// The window the print dialog belongs to. Optional: without it the dialog is application modal.
///
/// Linux: a `GtkWindow*` (Tauri's `gtk_window()`) used as the dialog's transient parent.
/// macOS: reserved (the dialog is application modal).
#[derive(Debug, Clone, Copy)]
pub struct WindowHandle {
    ptr: *mut std::ffi::c_void,
}

impl WindowHandle {
    pub const fn none() -> Self {
        WindowHandle {
            ptr: std::ptr::null_mut(),
        }
    }

    /// # Safety
    /// `ptr` is null or a live native window of the kind described on [`WindowHandle`], and
    /// stays alive for the duration of the print call.
    pub const unsafe fn from_raw(ptr: *mut std::ffi::c_void) -> Self {
        WindowHandle { ptr }
    }

    pub fn as_raw(&self) -> *mut std::ffi::c_void {
        self.ptr
    }
}

/// Imposition runs where the document may be parsed. The default [`LocalImposer`] does it
/// in-process; a host can implement this over IPC to the engine instead.
pub trait Imposer {
    fn impose(&self, print_pdf: &[u8], sheet: &Sheet, layout: &Layout) -> Result<Vec<u8>>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LocalImposer;

impl Imposer for LocalImposer {
    fn impose(&self, print_pdf: &[u8], sheet: &Sheet, layout: &Layout) -> Result<Vec<u8>> {
        impose(print_pdf, sheet, layout, &CancelToken::new())
    }
}

fn report(progress: &dyn Fn(PrintProgress), stage: Stage, done: u64, total: u64) {
    progress(PrintProgress { stage, done, total });
}

/// Build the print PDF with progress and cancellation (see [`build_print_pdf`]).
pub fn prepare(
    doc_bytes: &[u8],
    password: Option<&str>,
    opts: &PrintOptions,
    progress: &dyn Fn(PrintProgress),
    cancel: &CancelToken,
) -> Result<PrintPdf> {
    build_print_pdf(doc_bytes, password, opts, cancel, &|stage, done, total| {
        report(progress, stage, done, total)
    })
}

/// Hand a prepared print PDF to the OS. Call on the main thread.
pub fn print_prepared(
    pdf: &PrintPdf,
    opts: &PrintOptions,
    window: &WindowHandle,
    imposer: &dyn Imposer,
    progress: &dyn Fn(PrintProgress),
    cancel: &CancelToken,
) -> Result<PrintOutcome> {
    if cancel.is_cancelled() {
        return Ok(PrintOutcome::Cancelled);
    }
    let out = platform_print(pdf, opts, window, imposer, progress, cancel);
    match out {
        Err(Error::Cancelled) => Ok(PrintOutcome::Cancelled),
        Ok(PrintOutcome::Printed { pages }) => {
            report(progress, Stage::Done, pages as u64, pages as u64);
            Ok(PrintOutcome::Printed { pages })
        }
        other => other,
    }
}

#[cfg(target_os = "macos")]
fn platform_print(
    pdf: &PrintPdf,
    opts: &PrintOptions,
    window: &WindowHandle,
    _imposer: &dyn Imposer,
    progress: &dyn Fn(PrintProgress),
    cancel: &CancelToken,
) -> Result<PrintOutcome> {
    macos::print(pdf, opts, window, progress, cancel)
}

#[cfg(target_os = "linux")]
fn platform_print(
    pdf: &PrintPdf,
    opts: &PrintOptions,
    window: &WindowHandle,
    imposer: &dyn Imposer,
    progress: &dyn Fn(PrintProgress),
    cancel: &CancelToken,
) -> Result<PrintOutcome> {
    gtk::print(pdf, opts, window, imposer, progress, cancel)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_print(
    _pdf: &PrintPdf,
    _opts: &PrintOptions,
    _window: &WindowHandle,
    _imposer: &dyn Imposer,
    _progress: &dyn Fn(PrintProgress),
    _cancel: &CancelToken,
) -> Result<PrintOutcome> {
    Err(Error::Unsupported(
        "printing on this platform ships in v0.1.x (ADR-028)",
    ))
}

/// Prepare and print in one call. `password` is for encrypted documents.
pub fn print(
    doc_bytes: &[u8],
    password: Option<&str>,
    opts: &PrintOptions,
    window: &WindowHandle,
    progress: &dyn Fn(PrintProgress),
    cancel: &CancelToken,
) -> Result<PrintOutcome> {
    let pdf = match prepare(doc_bytes, password, opts, progress, cancel) {
        Err(Error::Cancelled) => return Ok(PrintOutcome::Cancelled),
        r => r?,
    };
    print_prepared(&pdf, opts, window, &LocalImposer, progress, cancel)
}
