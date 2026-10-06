//! Papyrine host (ARCHITECTURE §2): the trusted process that owns the window, the
//! sandboxed engine and renderer children, the journal, the caches and every file
//! write. The `broker` module has no Tauri dependency, so it is tested headless; the
//! Tauri app and the dev bridge are adapters over it.

pub mod api;
pub mod broker;
pub mod cache;
pub mod children;
pub mod error;
pub mod qoi;
pub mod recent;
pub mod recovery;
pub mod sched;
pub mod session;
pub mod supervisor;
pub mod util;
pub mod watch;

pub use broker::{Broker, Config, Dialogs, NoDialogs};
