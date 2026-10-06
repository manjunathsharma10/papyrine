//! The document actor: a thread that owns one [`DocState`].
//!
//! `papyrine_cos::Document` is `!Send`, so it is created on the actor thread and never leaves
//! it. Requests are closures run against the state, one at a time, in arrival order.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::thread::JoinHandle;

use crate::proto::{DocSummary, EngineError, Result};
use crate::state::DocState;

type Job = Box<dyn FnOnce(&mut DocState) + Send>;

pub struct Actor {
    tx: Option<mpsc::Sender<Job>>,
    handle: Option<JoinHandle<()>>,
}

impl Actor {
    /// Start the thread, build the state on it and wait for the open summary.
    pub fn spawn(
        make: impl FnOnce() -> Result<(DocState, DocSummary)> + Send + 'static,
    ) -> Result<(Actor, DocSummary)> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<DocSummary>>();
        let handle = std::thread::Builder::new()
            .name("papyrine-doc-actor".into())
            .spawn(move || {
                let made = catch_unwind(AssertUnwindSafe(make));
                let (mut st, summary) = match made {
                    Ok(Ok(v)) => v,
                    Ok(Err(e)) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                    Err(_) => {
                        let _ = ready_tx.send(Err(EngineError::Internal(
                            "panic while opening the document".into(),
                        )));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(summary));
                while let Ok(job) = rx.recv() {
                    // A panic that escapes a request leaves the document in an unknown state.
                    if catch_unwind(AssertUnwindSafe(|| job(&mut st))).is_err() {
                        st.mark_poisoned();
                    }
                }
            })?;
        let summary = ready_rx
            .recv()
            .map_err(|_| EngineError::Internal("document actor failed to start".into()))??;
        Ok((
            Actor {
                tx: Some(tx),
                handle: Some(handle),
            },
            summary,
        ))
    }

    /// Run `f` on the actor and wait for its result.
    pub fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut DocState) -> R + Send + 'static,
    ) -> Result<R> {
        let (rtx, rrx) = mpsc::channel();
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| EngineError::Internal("document actor is closed".into()))?;
        tx.send(Box::new(move |st| {
            let _ = rtx.send(f(st));
        }))
        .map_err(|_| EngineError::Internal("document actor is gone".into()))?;
        rrx.recv()
            .map_err(|_| EngineError::Internal("a panic escaped the document actor".into()))
    }
}

impl Drop for Actor {
    fn drop(&mut self) {
        self.tx = None; // ends the loop after the queued jobs
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
