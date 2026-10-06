//! Papyrine host binary: the Tauri window plus, when re-executed with `PAPYRINE_ROLE`, the
//! sandboxed engine or renderer child (one executable, several roles; ARCHITECTURE §2).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Linux: WebKitGTK's DMABUF renderer costs ~75 MB on the bare shell (measured, Spike 0.1;
/// ADR-026), so turn it off unless the user or packager set it. Must run before the
/// webview (and any thread) starts.
#[cfg(target_os = "linux")]
fn linux_webkit_env() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: called first thing in main(), before any other thread exists.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
}

fn main() {
    // A child must apply its sandbox before anything else happens in the process.
    if let Some(code) = papyrine_host::children::run_child_if_requested() {
        std::process::exit(code);
    }
    #[cfg(target_os = "linux")]
    linux_webkit_env();
    if std::env::var_os("PAPYRINE_TRACE").is_some() {
        println!("PAPYRINE_TRACE main_start");
    }
    papyrine_host::tauri_app::run(tauri::generate_context!());
}
