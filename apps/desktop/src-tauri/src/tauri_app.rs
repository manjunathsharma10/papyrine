//! The Tauri adapter over the broker: window, commands, the `papyrine://` scheme, native
//! dialogs, drag and drop, OS file associations and second-instance forwarding.
//!
//! Only work that is on the first-paint path starts before the window is visible: the
//! renderer and engine children (in parallel, on their own threads). The recovery scan,
//! the file watcher and housekeeping start after the page has loaded
//! (ARCHITECTURE §2, ADR-010).

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use serde_json::Value;
use tauri::http::{Response, StatusCode, header};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

use crate::api::{EventSink, HostEvent};
use crate::broker::{Broker, Config, Dialogs};
use crate::cache::TileData;
use crate::dispatch;
use crate::error::HostErr;
use crate::sched::{PRIO_PREFETCH, PRIO_PREVIEW, PRIO_VISIBLE};
use crate::supervisor::ExeLauncher;
use crate::{children, single_instance};

/// Subsystems allowed to start before first paint.
pub const STARTUP_ALLOWLIST: &[&str] = &["window", "renderer", "engine", "ipc"];

struct AppState {
    broker: Broker,
    started: Instant,
}

/// Events are emitted through the app handle once it exists.
#[derive(Default)]
struct TauriSink(OnceLock<AppHandle>);

impl EventSink for TauriSink {
    fn emit(&self, event: HostEvent) {
        if let Some(app) = self.0.get() {
            let _ = app.emit("host-event", &event);
        }
    }
}

/// Native dialogs on the main thread (macOS requires it; elsewhere it is harmless).
struct NativeDialogs(AppHandle);

impl NativeDialogs {
    fn on_main<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
        let (tx, rx) = mpsc::channel();
        self.0
            .run_on_main_thread(move || {
                let _ = tx.send(f());
            })
            .ok()?;
        rx.recv().ok()
    }
}

impl Dialogs for NativeDialogs {
    fn open(&self) -> Vec<PathBuf> {
        self.on_main(|| {
            rfd::FileDialog::new()
                .add_filter("PDF", &["pdf"])
                .pick_files()
                .unwrap_or_default()
        })
        .unwrap_or_default()
    }

    fn save(&self, suggested: &str) -> Option<PathBuf> {
        let name = suggested.to_string();
        self.on_main(move || {
            rfd::FileDialog::new()
                .add_filter("PDF", &["pdf"])
                .set_file_name(&name)
                .save_file()
        })
        .flatten()
    }

    fn folder(&self) -> Option<PathBuf> {
        self.on_main(|| rfd::FileDialog::new().pick_folder())
            .flatten()
    }
}

fn trace_on() -> bool {
    std::env::var_os("PAPYRINE_TRACE").is_some()
}

fn epoch_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

// ------------------------------------------------------------------------ commands

#[tauri::command]
async fn host_call(
    state: State<'_, AppState>,
    method: String,
    args: Vec<Value>,
) -> Result<Value, HostErr> {
    let b = state.broker.clone();
    tauri::async_runtime::spawn_blocking(move || dispatch::call(&b, &method, &args))
        .await
        .map_err(|e| HostErr::internal(e.to_string()))?
}

/// Open a PDF that arrived as bytes (webview drop or picker). Raw body; the name is in the
/// `x-papyrine-name` header (URI-encoded).
#[tauri::command]
async fn open_bytes(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<Value, HostErr> {
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else {
        return Err(HostErr::internal("open_bytes needs a raw body"));
    };
    let name = request
        .headers()
        .get("x-papyrine-name")
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .unwrap_or_else(|| "document.pdf".into());
    let data = data.clone();
    let b = state.broker.clone();
    tauri::async_runtime::spawn_blocking(move || {
        serde_json::to_value(b.open_bytes(&name, &data, None)?).map_err(HostErr::from)
    })
    .await
    .map_err(|e| HostErr::internal(e.to_string()))?
}

#[tauri::command]
fn cancel_tile(state: State<'_, AppState>, rid: u64) {
    state.broker.cancel_tile(rid);
}

#[tauri::command]
fn first_tile_painted(stage: &str, state: State<'_, AppState>) {
    if trace_on() {
        println!(
            "PAPYRINE_TRACE first_tile_painted stage={stage} epoch_ms={} since_main_ms={}",
            epoch_ms(),
            state.started.elapsed().as_millis()
        );
    }
}

#[tauri::command]
fn ui_error(message: String) {
    if trace_on() {
        println!("PAPYRINE_TRACE ui_error {message}");
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(h) = s.get(i + 1..i + 3)
            && let Ok(v) = u8::from_str_radix(h, 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// --------------------------------------------------------------------- papyrine://

fn tile_response(r: Result<Arc<TileData>, HostErr>) -> Response<Vec<u8>> {
    let base = Response::builder()
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "x-papyrine-width, x-papyrine-height, x-papyrine-error",
        )
        .header(header::CACHE_CONTROL, "no-store");
    match r {
        Ok(t) => base
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header("x-papyrine-width", t.width)
            .header("x-papyrine-height", t.height)
            .body(t.rgba.clone()),
        Err(e) => base
            .status(if e.is_cancelled() {
                StatusCode::GONE
            } else {
                StatusCode::NOT_FOUND
            })
            .header("x-papyrine-error", e.message.replace(['\r', '\n'], " "))
            .body(Vec::new()),
    }
    .expect("static response parts are valid")
}

/// `/tile/{doc}/{page}/{scale}/{tx}/{ty}?rid=N&prio=P` and `/preview/{doc}/{page}/{edge}?rid=N`.
fn handle_scheme(
    broker: &Broker,
    uri: &tauri::http::Uri,
    respond: impl FnOnce(Response<Vec<u8>>) + Send + 'static,
) {
    let query = uri.query().unwrap_or("");
    let q = |k: &str| {
        query
            .split('&')
            .find_map(|kv| kv.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
            .and_then(|v| v.parse::<u64>().ok())
    };
    let rid = q("rid").unwrap_or(0);
    let prio = match q("prio") {
        Some(1) => PRIO_PREVIEW,
        Some(2) => PRIO_PREFETCH,
        _ => PRIO_VISIBLE,
    };
    let parts: Vec<&str> = uri.path().trim_start_matches('/').split('/').collect();
    let cb = move |r: Result<Arc<TileData>, HostErr>| respond(tile_response(r));
    match parts.as_slice() {
        ["tile", doc, page, scale, tx, ty] => {
            let (Ok(page), Ok(scale), Ok(tx), Ok(ty)) =
                (page.parse(), scale.parse(), tx.parse(), ty.parse())
            else {
                return cb(Err(HostErr::internal("bad tile url")));
            };
            broker.request_tile(doc, page, scale, tx, ty, prio, rid, Box::new(cb));
        }
        ["preview", doc, page, edge] => {
            let (Ok(page), Ok(edge)) = (page.parse(), edge.parse()) else {
                return cb(Err(HostErr::internal("bad preview url")));
            };
            broker.request_preview(doc, page, edge, rid, Box::new(cb));
        }
        _ => cb(Err(HostErr::not_found("no such resource"))),
    }
}

// --------------------------------------------------------------------------- run

fn file_args() -> Vec<String> {
    std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .filter(|a| std::path::Path::new(a).is_file())
        .collect()
}

/// Run the app. `context` comes from `tauri::generate_context!()` in the binary crate.
pub fn run(context: tauri::Context<tauri::Wry>) {
    let trace = papyrine_core::startup::global();
    let sink = Arc::new(TauriSink::default());
    let sink_for_setup = sink.clone();

    let app = {
        let _span = trace.span("subsystem.window");
        tauri::Builder::default()
            .register_asynchronous_uri_scheme_protocol("papyrine", |ctx, req, responder| {
                let app = ctx.app_handle().clone();
                let uri = req.uri().clone();
                match app.try_state::<AppState>() {
                    Some(st) => handle_scheme(&st.broker, &uri, move |r| responder.respond(r)),
                    None => {
                        responder.respond(tile_response(Err(HostErr::internal("host is starting"))))
                    }
                }
            })
            .invoke_handler(tauri::generate_handler![
                host_call,
                open_bytes,
                cancel_tile,
                first_tile_painted,
                ui_error
            ])
            .setup(move |app| {
                let handle = app.handle().clone();
                let _ = sink_for_setup.0.set(handle.clone());
                let data_dir = app.path().app_data_dir()?;
                let cache_dir = app.path().app_cache_dir()?;
                let files = file_args();
                // Second launch: hand the files to the running instance and quit.
                let (listener, token) = match single_instance::acquire(&data_dir, &files)? {
                    single_instance::Instance::Forwarded => std::process::exit(0),
                    single_instance::Instance::Primary(l, t) => (l, t),
                };
                let mut launcher = ExeLauncher::default();
                if let Some(d) = children::dev_pdfium_dir() {
                    launcher = launcher.with_pdfium(d);
                }
                let mut cfg = Config::new(
                    data_dir,
                    cache_dir,
                    Arc::new(launcher),
                    sink_for_setup.clone(),
                );
                cfg.dialogs = Arc::new(NativeDialogs(handle.clone()));
                let broker = Broker::new(cfg);
                // Children start now so they are ready by the time the UI wants page 1.
                broker.warm_up();
                broker.request_open(files.iter().map(PathBuf::from).collect());
                let b2 = broker.clone();
                let h2 = handle.clone();
                single_instance::serve(listener, token, move |f| {
                    b2.request_open(f.iter().map(PathBuf::from).collect());
                    if let Some(w) = h2.get_webview_window("main") {
                        let _ = w.set_focus();
                    }
                });
                app.manage(AppState {
                    broker,
                    started: Instant::now(),
                });
                Ok(())
            })
            .on_page_load(|webview, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    let t = papyrine_core::startup::global();
                    t.mark_first_paint();
                    if trace_on() {
                        println!(
                            "PAPYRINE_TRACE first_paint subsystems={:?} violations={:?}",
                            t.subsystems_before_first_paint(),
                            t.violations(STARTUP_ALLOWLIST)
                        );
                    }
                    if let Some(st) = webview.app_handle().try_state::<AppState>() {
                        st.broker.after_first_paint();
                    }
                }
            })
            .on_window_event(|window, event| {
                let Some(st) = window.app_handle().try_state::<AppState>() else {
                    return;
                };
                match event {
                    WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
                        st.broker.request_open(paths.clone());
                    }
                    WindowEvent::Focused(true) => {
                        let b = st.broker.clone();
                        std::thread::spawn(move || b.check_external_all());
                    }
                    _ => {}
                }
            })
            .build(context)
            .expect("error while building Papyrine")
    };

    app.run(|app, event| match event {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        RunEvent::Opened { urls } => {
            if let Some(st) = app.try_state::<AppState>() {
                let paths = urls.iter().filter_map(|u| u.to_file_path().ok()).collect();
                st.broker.request_open(paths);
            }
        }
        RunEvent::Exit => {
            if let Some(st) = app.try_state::<AppState>() {
                st.broker.shutdown();
            }
            if let Ok(d) = app.path().app_data_dir() {
                single_instance::release(&d);
            }
        }
        _ => {
            let _ = app;
        }
    });
}
