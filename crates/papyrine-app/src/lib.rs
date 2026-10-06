//! papyrine-app: the logic of Papyrine's single multi-role executable (ARCHITECTURE section 2).
//!
//! * [`roles`]: role dispatch. A child started by [`spawn`] finds its role in the environment,
//!   applies its sandbox and runs the engine or renderer loop; `papyrine info` runs the CLI.
//! * [`spawn`]: starting sandboxed engine and renderer children by re-executing the binary.
//! * [`host`]: the host-side half of the engine protocol: write-ahead journalling around every
//!   command, atomic saves from engine-produced bytes, checkpoints, and engine restart with
//!   journal replay.
//! * [`supervisor`]: restart policy and renderer snapshot reload.
//! * [`cli`]: the v0.1.x CLI skeleton (`papyrine info [--json] FILE`).

pub mod cli;
pub mod host;
pub mod roles;
pub mod spawn;
pub mod supervisor;

pub use roles::{main_entry, run_if_child};
