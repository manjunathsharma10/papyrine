//! Linux back end: `GtkPrintUnixDialog` + `GtkPrintJob` with the print PDF as the job's source
//! file, so the vector PDF goes straight to CUPS (inside Flatpak GTK uses the print portal by
//! itself). Nothing is imposed by GTK or the dialog, so after the dialog says which paper was
//! chosen we run [`Imposer`] to lay every page onto that sheet with the layout math.
//!
//! GTK is **dlopen-ed** (`libgtk-3.so.0`, the same library Tauri already links), not linked, so
//! building needs no GTK development files and a missing GTK is a normal error. The module only
//! calls plain C entry points; gtk-rs has no binding for `GtkPrintUnixDialog`.
//!
//! Threading: everything must run on the thread that runs the GTK main loop (Tauri's main
//! thread). The job's completion callback is delivered by iterating the default main context
//! from inside [`print`], so the call blocks until CUPS has accepted or rejected the job.
//! GTK has no API to cancel a job after `gtk_print_job_send`, so [`CancelToken`] is honoured
//! until the job is sent.

use crate::{
    CancelToken, Destination, Error, Imposer, PrintOptions, PrintOutcome, PrintPdf, PrintProgress,
    Result, Sheet, Stage, WindowHandle, report,
};
use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

type P = *mut c_void;

const GTK_RESPONSE_OK: c_int = -5;
const GTK_UNIT_POINTS: c_int = 1;

#[repr(C)]
struct GError {
    domain: u32,
    code: c_int,
    message: *mut c_char,
}

type PrintJobDone = unsafe extern "C" fn(job: P, data: P, err: *const GError);
type PrinterFunc = unsafe extern "C" fn(printer: P, data: P) -> c_int;
type DestroyNotify = Option<unsafe extern "C" fn(P)>;

macro_rules! api {
    ($($name:ident : $ty:ty),* $(,)?) => {
        /// The GTK/GLib entry points used here, resolved once.
        struct Api {
            _libs: Vec<Library>,
            $($name: $ty,)*
        }

        impl Api {
            fn load() -> Result<Api> {
                let open = |names: &[&str]| -> Result<Library> {
                    for n in names {
                        // SAFETY: loading GTK runs its (benign) library constructors.
                        if let Ok(l) = unsafe { Library::new(n) } {
                            return Ok(l);
                        }
                    }
                    Err(Error::Platform(format!("cannot load {}", names[0])))
                };
                let gtk = open(&["libgtk-3.so.0", "libgtk-3.so"])?;
                let gobject = open(&["libgobject-2.0.so.0"]).ok();
                let glib = open(&["libglib-2.0.so.0"]).ok();
                $(
                    let $name: $ty = {
                        let sym = concat!(stringify!($name), "\0").as_bytes();
                        let mut found = None;
                        for l in std::iter::once(&gtk).chain(gobject.iter()).chain(glib.iter()) {
                            // SAFETY: the declared type is the C prototype of this symbol.
                            if let Ok(s) = unsafe { l.get::<$ty>(sym) } {
                                found = Some(*s);
                                break;
                            }
                        }
                        found.ok_or_else(|| {
                            Error::Platform(format!("GTK is missing {}", stringify!($name)))
                        })?
                    };
                )*
                Ok(Api { _libs: [Some(gtk), gobject, glib].into_iter().flatten().collect(), $($name,)* })
            }
        }
    };
}

api! {
    gtk_init_check: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char) -> c_int,
    gtk_print_unix_dialog_new: unsafe extern "C" fn(*const c_char, P) -> P,
    gtk_print_unix_dialog_set_embed_page_setup: unsafe extern "C" fn(P, c_int),
    gtk_print_unix_dialog_set_page_setup: unsafe extern "C" fn(P, P),
    gtk_print_unix_dialog_get_page_setup: unsafe extern "C" fn(P) -> P,
    gtk_print_unix_dialog_get_settings: unsafe extern "C" fn(P) -> P,
    gtk_print_unix_dialog_get_selected_printer: unsafe extern "C" fn(P) -> P,
    gtk_dialog_run: unsafe extern "C" fn(P) -> c_int,
    gtk_widget_destroy: unsafe extern "C" fn(P),
    gtk_page_setup_new: unsafe extern "C" fn() -> P,
    gtk_page_setup_set_paper_size: unsafe extern "C" fn(P, P),
    gtk_page_setup_set_top_margin: unsafe extern "C" fn(P, f64, c_int),
    gtk_page_setup_set_bottom_margin: unsafe extern "C" fn(P, f64, c_int),
    gtk_page_setup_set_left_margin: unsafe extern "C" fn(P, f64, c_int),
    gtk_page_setup_set_right_margin: unsafe extern "C" fn(P, f64, c_int),
    gtk_page_setup_get_paper_width: unsafe extern "C" fn(P, c_int) -> f64,
    gtk_page_setup_get_paper_height: unsafe extern "C" fn(P, c_int) -> f64,
    gtk_page_setup_get_top_margin: unsafe extern "C" fn(P, c_int) -> f64,
    gtk_page_setup_get_bottom_margin: unsafe extern "C" fn(P, c_int) -> f64,
    gtk_page_setup_get_left_margin: unsafe extern "C" fn(P, c_int) -> f64,
    gtk_page_setup_get_right_margin: unsafe extern "C" fn(P, c_int) -> f64,
    gtk_paper_size_new_custom: unsafe extern "C" fn(*const c_char, *const c_char, f64, f64, c_int) -> P,
    gtk_paper_size_free: unsafe extern "C" fn(P),
    gtk_print_settings_new: unsafe extern "C" fn() -> P,
    gtk_print_settings_set: unsafe extern "C" fn(P, *const c_char, *const c_char),
    gtk_print_job_new: unsafe extern "C" fn(*const c_char, P, P, P) -> P,
    gtk_print_job_set_source_file: unsafe extern "C" fn(P, *const c_char, *mut *mut GError) -> c_int,
    gtk_print_job_send: unsafe extern "C" fn(P, PrintJobDone, P, DestroyNotify),
    gtk_enumerate_printers: unsafe extern "C" fn(PrinterFunc, P, DestroyNotify, c_int),
    gtk_printer_get_name: unsafe extern "C" fn(P) -> *const c_char,
    g_object_ref: unsafe extern "C" fn(P) -> P,
    g_object_unref: unsafe extern "C" fn(P),
    g_main_context_iteration: unsafe extern "C" fn(P, c_int) -> c_int,
    g_error_free: unsafe extern "C" fn(*mut GError),
}

/// An owned GObject reference, released on drop.
struct Obj<'a> {
    api: &'a Api,
    ptr: P,
}

impl<'a> Obj<'a> {
    /// Take ownership of a reference we were given (`transfer full`).
    fn owned(api: &'a Api, ptr: P) -> Option<Self> {
        (!ptr.is_null()).then_some(Obj { api, ptr })
    }

    /// Add a reference to a borrowed object (`transfer none`).
    fn shared(api: &'a Api, ptr: P) -> Option<Self> {
        // SAFETY: `ptr` is a live GObject.
        (!ptr.is_null()).then(|| Obj {
            api,
            ptr: unsafe { (api.g_object_ref)(ptr) },
        })
    }
}

impl Drop for Obj<'_> {
    fn drop(&mut self) {
        // SAFETY: we own one reference.
        unsafe { (self.api.g_object_unref)(self.ptr) }
    }
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).expect("no NULs left")
}

/// Paper and margins of a `GtkPageSetup`, in points.
fn sheet_of(api: &Api, setup: P) -> Sheet {
    // SAFETY: `setup` is a live GtkPageSetup.
    unsafe {
        Sheet {
            width: (api.gtk_page_setup_get_paper_width)(setup, GTK_UNIT_POINTS),
            height: (api.gtk_page_setup_get_paper_height)(setup, GTK_UNIT_POINTS),
            margin_top: (api.gtk_page_setup_get_top_margin)(setup, GTK_UNIT_POINTS),
            margin_right: (api.gtk_page_setup_get_right_margin)(setup, GTK_UNIT_POINTS),
            margin_bottom: (api.gtk_page_setup_get_bottom_margin)(setup, GTK_UNIT_POINTS),
            margin_left: (api.gtk_page_setup_get_left_margin)(setup, GTK_UNIT_POINTS),
        }
    }
}

/// A `GtkPageSetup` for a given sheet (used when no dialog chooses the paper).
fn page_setup_for<'a>(api: &'a Api, sheet: &Sheet) -> Result<Obj<'a>> {
    // SAFETY: plain constructors; every object created is released or owned by `Obj`.
    unsafe {
        let setup = Obj::owned(api, (api.gtk_page_setup_new)())
            .ok_or_else(|| Error::Platform("gtk_page_setup_new failed".into()))?;
        let name = cstr("papyrine-custom");
        let paper = (api.gtk_paper_size_new_custom)(
            name.as_ptr(),
            name.as_ptr(),
            sheet.width,
            sheet.height,
            GTK_UNIT_POINTS,
        );
        (api.gtk_page_setup_set_paper_size)(setup.ptr, paper);
        (api.gtk_paper_size_free)(paper);
        (api.gtk_page_setup_set_top_margin)(setup.ptr, sheet.margin_top, GTK_UNIT_POINTS);
        (api.gtk_page_setup_set_right_margin)(setup.ptr, sheet.margin_right, GTK_UNIT_POINTS);
        (api.gtk_page_setup_set_bottom_margin)(setup.ptr, sheet.margin_bottom, GTK_UNIT_POINTS);
        (api.gtk_page_setup_set_left_margin)(setup.ptr, sheet.margin_left, GTK_UNIT_POINTS);
        Ok(setup)
    }
}

struct Found<'a> {
    api: &'a Api,
    want: CString,
    hit: P,
}

unsafe extern "C" fn match_printer(printer: P, data: P) -> c_int {
    // SAFETY: `data` is the `Found` that outlives the (blocking) enumeration.
    let f = unsafe { &mut *(data as *mut Found<'_>) };
    // SAFETY: GTK hands us a live printer; the name is valid for this call.
    let name = unsafe { CStr::from_ptr((f.api.gtk_printer_get_name)(printer)) };
    if name == f.want.as_c_str() {
        // SAFETY: keep the printer alive beyond the callback.
        f.hit = unsafe { (f.api.g_object_ref)(printer) };
        return 1; // stop
    }
    0
}

fn find_printer<'a>(api: &'a Api, name: &str) -> Result<Obj<'a>> {
    let mut found = Found {
        api,
        want: cstr(name),
        hit: std::ptr::null_mut(),
    };
    // SAFETY: blocks (wait = TRUE) until every backend has reported, so `found` outlives it.
    unsafe {
        (api.gtk_enumerate_printers)(match_printer, &mut found as *mut Found<'_> as P, None, 1);
    }
    Obj::owned(api, found.hit).ok_or_else(|| Error::NoSuchPrinter(name.to_string()))
}

/// Completion state shared with the C callback.
#[derive(Default)]
struct JobState {
    done: AtomicBool,
    error: Mutex<Option<String>>,
}

unsafe extern "C" fn job_done(_job: P, data: P, err: *const GError) {
    // SAFETY: `data` is the `JobState` owned by `print`, alive until `done` is observed.
    let st = unsafe { &*(data as *const JobState) };
    if !err.is_null() {
        // SAFETY: GError.message is a NUL-terminated string.
        let msg = unsafe { CStr::from_ptr((*err).message) };
        *st.error.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(msg.to_string_lossy().into_owned());
    }
    st.done.store(true, Ordering::Release);
}

/// Removes the temporary job file when dropped.
struct TempJob(PathBuf);

impl Drop for TempJob {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn temp_job_file(bytes: &[u8]) -> Result<TempJob> {
    use std::sync::atomic::AtomicU32;
    static N: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "papyrine-print-{}-{}.pdf",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, bytes).map_err(|e| Error::Platform(format!("temp job file: {e}")))?;
    Ok(TempJob(path))
}

pub fn print(
    pdf: &PrintPdf,
    opts: &PrintOptions,
    window: &WindowHandle,
    imposer: &dyn Imposer,
    progress: &dyn Fn(PrintProgress),
    cancel: &CancelToken,
) -> Result<PrintOutcome> {
    let pages = pdf.page_count as u64;

    // No printer involved: lay out for the requested paper and write the PDF.
    if let Destination::SaveToFile { path } = &opts.destination {
        let sheet = opts.paper.unwrap_or(Sheet::LETTER);
        report(progress, Stage::Sending, 0, pages);
        let out = imposer.impose(&pdf.bytes, &sheet, &opts.layout)?;
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        std::fs::write(path, out)
            .map_err(|e| Error::JobFailed(format!("{}: {e}", path.display())))?;
        return Ok(PrintOutcome::Printed {
            pages: pdf.page_count,
        });
    }

    let api = Api::load()?;
    // SAFETY: initialises GTK for this process if the host has not (a no-op if it has).
    if unsafe { (api.gtk_init_check)(std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
        return Err(Error::Platform("GTK could not open a display".into()));
    }

    let (printer, settings, setup);
    match &opts.destination {
        Destination::Dialog => {
            report(progress, Stage::AwaitingDialog, 0, pages);
            let title = cstr(&opts.job_name);
            // SAFETY: GTK is initialised and we are on its thread; the dialog is destroyed below.
            unsafe {
                let dialog = (api.gtk_print_unix_dialog_new)(title.as_ptr(), window.as_raw());
                if dialog.is_null() {
                    return Err(Error::Platform("gtk_print_unix_dialog_new failed".into()));
                }
                (api.gtk_print_unix_dialog_set_embed_page_setup)(dialog, 1);
                if let Some(paper) = &opts.paper {
                    let s = page_setup_for(&api, paper)?;
                    (api.gtk_print_unix_dialog_set_page_setup)(dialog, s.ptr);
                }
                let response = (api.gtk_dialog_run)(dialog);
                let picked = if response == GTK_RESPONSE_OK {
                    // Settings are `transfer full`; printer and page setup are borrowed.
                    Some((
                        Obj::shared(
                            &api,
                            (api.gtk_print_unix_dialog_get_selected_printer)(dialog),
                        ),
                        Obj::owned(&api, (api.gtk_print_unix_dialog_get_settings)(dialog)),
                        Obj::shared(&api, (api.gtk_print_unix_dialog_get_page_setup)(dialog)),
                    ))
                } else {
                    None
                };
                (api.gtk_widget_destroy)(dialog);
                match picked {
                    None => return Ok(PrintOutcome::Cancelled),
                    Some((Some(p), Some(s), Some(u))) => (printer, settings, setup) = (p, s, u),
                    Some(_) => {
                        return Err(Error::Platform(
                            "the print dialog returned no printer".into(),
                        ));
                    }
                }
            }
        }
        Destination::Printer { name } => {
            printer = find_printer(&api, name)?;
            // SAFETY: plain constructor.
            settings = Obj::owned(&api, unsafe { (api.gtk_print_settings_new)() })
                .ok_or_else(|| Error::Platform("gtk_print_settings_new failed".into()))?;
            setup = page_setup_for(&api, &opts.paper.unwrap_or(Sheet::LETTER))?;
        }
        Destination::SaveToFile { .. } => unreachable!("handled above"),
    }
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }

    // Our pages are already laid out for the sheet: CUPS must not scale them again.
    let (k, v) = (cstr("cups-print-scaling"), cstr("none"));
    // SAFETY: valid settings object and C strings.
    unsafe { (api.gtk_print_settings_set)(settings.ptr, k.as_ptr(), v.as_ptr()) };

    report(progress, Stage::Sending, 0, pages);
    let sheet = sheet_of(&api, setup.ptr);
    let laid_out = imposer.impose(&pdf.bytes, &sheet, &opts.layout)?;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let file = temp_job_file(&laid_out)?;
    drop(laid_out);

    let title = cstr(&opts.job_name);
    let path = cstr(&file.0.to_string_lossy());
    let state = JobState::default();
    // SAFETY: all objects are live; `state` outlives the loop below, which runs until the
    // completion callback has fired, so GTK never touches it afterwards.
    unsafe {
        let job = Obj::owned(
            &api,
            (api.gtk_print_job_new)(title.as_ptr(), printer.ptr, settings.ptr, setup.ptr),
        )
        .ok_or_else(|| Error::Platform("gtk_print_job_new failed".into()))?;
        let mut err: *mut GError = std::ptr::null_mut();
        if (api.gtk_print_job_set_source_file)(job.ptr, path.as_ptr(), &mut err) == 0 {
            let msg = if err.is_null() {
                "cannot use the print file".to_string()
            } else {
                let m = CStr::from_ptr((*err).message)
                    .to_string_lossy()
                    .into_owned();
                (api.g_error_free)(err);
                m
            };
            return Err(Error::JobFailed(msg));
        }
        (api.gtk_print_job_send)(job.ptr, job_done, &state as *const JobState as P, None);
        while !state.done.load(Ordering::Acquire) {
            if (api.g_main_context_iteration)(std::ptr::null_mut(), 0) == 0 {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    match state.error.into_inner().unwrap_or_else(|e| e.into_inner()) {
        Some(e) => Err(Error::JobFailed(e)),
        None => Ok(PrintOutcome::Printed {
            pages: pdf.page_count,
        }),
    }
}
