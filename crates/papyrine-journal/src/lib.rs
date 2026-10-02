//! Host-side write-ahead journal (ARCHITECTURE section 7, ADR-012).
//!
//! The host appends an `Intent` before the engine sees a command and a `Commit` with
//! after-images when the engine answers. Independent of qpdf: after-images are opaque bytes.
//!
//! Layout of `<recovery>/<doc-id>/`: `base.json`, `journal.log`, `blobs/<blake3>`,
//! `quarantine.json`. Checkpoints are stored as content-addressed blobs and referenced by a
//! `Checkpoint` record (so a crash while swapping logs never loses the previous checkpoint).

mod dir;
mod error;
mod fs;
mod gate;
mod journal;
mod record;
mod recover;
pub mod testing;

pub use dir::{BaseInfo, CRASH_LIMIT, Found, PRUNE_AFTER_DAYS, Pruned, Quarantined, RecoveryRoot};
pub use error::{JournalError, Result};
pub use fs::{DirEntry, Fs, FsFile, RealFs, Stat, backup_exclude_plist};
pub use gate::{DispatchError, Dispatcher, Logged};
pub use journal::{Clock, Flusher, IntentToken, Journal, Options, Resolution, SystemClock};
pub use record::{AfterImage, BlobRef, Hash, MAGIC, ObjId, Record, Stop, hash_hex};
pub use recover::{CheckpointInfo, CommittedCommand, Recovery, UnfinishedCommand, recover};
