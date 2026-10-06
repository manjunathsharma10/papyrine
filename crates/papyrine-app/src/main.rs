//! The multi-role executable used for the engine and renderer children, the CLI, and the
//! process-level tests. The Tauri host links `papyrine-app` and calls
//! [`papyrine_app::run_if_child`] first thing in `main`, so the same binary serves every role.

fn main() -> std::process::ExitCode {
    papyrine_app::main_entry()
}
