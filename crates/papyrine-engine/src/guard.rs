//! Panic containment for commands.
//!
//! A panic inside `apply` must not leave the document half-edited. [`Guarded`] wraps a command:
//! on a panic it rolls back everything the `EditContext` recorded, remembers the panic text and
//! returns an error, so `History::execute` pushes nothing.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use papyrine_ops::{ChangeSet, Command, EditContext, Error, LocalizedText, Result};
use serde_json::Value;

pub type PanicSlot = Arc<Mutex<Option<String>>>;

pub struct Guarded {
    inner: Box<dyn Command>,
    slot: PanicSlot,
}

impl Guarded {
    pub fn new(inner: Box<dyn Command>, slot: PanicSlot) -> Self {
        Guarded { inner, slot }
    }

    fn record(&self, p: Box<dyn std::any::Any + Send>) -> Error {
        let msg = if let Some(s) = p.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = p.downcast_ref::<String>() {
            s.clone()
        } else {
            "panic".to_string()
        };
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg.clone());
        Error::invalid(format!("command `{}` panicked: {msg}", self.inner.name()))
    }
}

impl Command for Guarded {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn describe(&self) -> LocalizedText {
        self.inner.describe()
    }
    fn params(&self) -> Value {
        self.inner.params()
    }
    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        match catch_unwind(AssertUnwindSafe(|| self.inner.apply(cx))) {
            Ok(r) => r,
            Err(p) => {
                let e = self.record(p);
                let _ = cx.rollback();
                Err(e)
            }
        }
    }
    fn revert(&mut self, cx: &mut EditContext<'_>, cs: &ChangeSet) -> Result<()> {
        match catch_unwind(AssertUnwindSafe(|| self.inner.revert(cx, cs))) {
            Ok(r) => r,
            Err(p) => Err(self.record(p)),
        }
    }
    fn reapply(&mut self, cx: &mut EditContext<'_>, cs: &ChangeSet) -> Result<()> {
        match catch_unwind(AssertUnwindSafe(|| self.inner.reapply(cx, cs))) {
            Ok(r) => r,
            Err(p) => Err(self.record(p)),
        }
    }
}
