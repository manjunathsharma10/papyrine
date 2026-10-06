//! papyrine-engine: the engine role (ARCHITECTURE sections 2, 4.4 to 4.6, 5, 6 and 7).
//!
//! * One **document actor** thread per open document owns the qpdf `Document` (inside a
//!   `papyrine_model::Model`) and the `papyrine_ops::History` ([`actor`], [`state`]).
//! * [`proto`] is the request/response set the host speaks to it over `papyrine-ipc`.
//! * Every commit returns a ChangeSet summary, serialized after-images for the host's
//!   write-ahead journal, and a new **render snapshot section** (an incremental update built by
//!   `papyrine-writer` against the render base chain), compacted L1 (merge) and L2 (new
//!   ID-preserving base) per ADR-005.
//! * [`replay`] applies journalled after-images deterministically (never re-running commands).
//! * Saves produce bytes (an appended incremental section, or a full optimized file in the
//!   engine's scratch directory); the host does the atomic replace.
//! * A panic inside a command is caught, the document is rolled back and the error returned
//!   ([`guard`]).

pub mod actor;
pub mod guard;
pub mod images;
pub mod proto;
mod query;
pub mod registry;
pub mod replay;
pub mod server;
pub mod state;
pub mod util;

pub use proto::{EngineError, Request, Response};
pub use registry::default_registry;
pub use server::{EngineHandler, run_engine};
