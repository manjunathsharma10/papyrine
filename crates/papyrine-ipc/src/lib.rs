//! papyrine-ipc: the versioned protocol between the host and its sandboxed
//! engine/renderer children (docs/ARCHITECTURE.md §4.9).
//!
//! * [`proto`]: ids, handshake, request/response sets, UI-facing types
//!   (TypeScript generated into `apps/desktop/src/ipc/generated/`).
//! * [`transport`]: length-prefixed postcard frames over a Unix socket (with
//!   `SCM_RIGHTS` handle passing) or anonymous pipes (with `DuplicateHandle`).
//! * [`shm`]: shared memory regions for pixels and snapshot sections, and
//!   read-only file mappings for documents the host hands over.
//! * [`client`] / [`server`]: host-side calls, jobs, progress and
//!   cancellation; child-side request loop.
//! * [`spawn`] / [`child`]: starting a sandboxed child of the current
//!   executable and bootstrapping it.

pub mod child;
pub mod client;
pub mod handle;
pub mod proto;
pub mod server;
pub mod shm;
pub mod spawn;
pub mod transport;

pub use child::{Bootstrap, BootstrapError, bootstrap};
pub use client::{Client, Job, Pending};
pub use handle::Handle;
pub use proto::*;
pub use server::{Ctx, Handler, ServeError, ServeExit, serve};
pub use shm::{MappedFile, SharedBytes, SharedRegion};
pub use spawn::{ChildClient, ChildProcess, ChildSpec, Spawned, spawn_child};
pub use transport::{Endpoint, Receiver, Sender, TransportError};
