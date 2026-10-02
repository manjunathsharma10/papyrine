//! Structural write-ahead guarantee: the only way to hand a command to the engine side is
//! `Dispatcher::dispatch`, which writes the Intent first. The engine receives `Logged<T>`,
//! which cannot be built without an `IntentToken`.

use crate::error::Result;
use crate::journal::{IntentToken, Journal};
use crate::record::BlobRef;
use std::sync::mpsc::Sender;

/// A command message that is known to have its Intent in the journal.
#[derive(Debug)]
pub struct Logged<T> {
    pub token: IntentToken,
    pub msg: T,
}

/// Host-side forwarder to the engine channel.
pub struct Dispatcher<T> {
    journal: Journal,
    tx: Sender<Logged<T>>,
}

#[derive(Debug)]
pub enum DispatchError {
    /// Intent could not be written (or command is quarantined); nothing was sent.
    Journal(crate::error::JournalError),
    /// Intent was written but the engine is gone; the intent has been closed as failed.
    EngineGone,
}

impl<T> Dispatcher<T> {
    pub fn new(journal: Journal, tx: Sender<Logged<T>>) -> Self {
        Self { journal, tx }
    }

    /// Write the Intent, then (and only then) forward `msg`. Returns the seq.
    pub fn dispatch(
        &self,
        command: &str,
        params: serde_json::Value,
        blobs: Vec<BlobRef>,
        msg: T,
    ) -> std::result::Result<u64, DispatchError> {
        let token = self
            .journal
            .begin(command, params, blobs)
            .map_err(DispatchError::Journal)?;
        let seq = token.seq();
        match self.tx.send(Logged { token, msg }) {
            Ok(()) => Ok(seq),
            Err(e) => {
                // The engine never saw it: close the intent as a clean failure.
                let _ = self.journal.fail(e.0.token);
                Err(DispatchError::EngineGone)
            }
        }
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Journal(e) => write!(f, "{e}"),
            Self::EngineGone => write!(f, "engine is gone"),
        }
    }
}
impl std::error::Error for DispatchError {}

#[allow(dead_code)]
fn _assert_result(_: Result<()>) {}
