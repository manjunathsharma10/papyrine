//! OS handles that travel inside messages.
//!
//! A [`Handle`] is an owned file descriptor (Unix) or kernel `HANDLE`
//! (Windows). Serializing a message that contains handles does not put the
//! handle in the byte stream: the serializer records the handle in a
//! per-thread collector and writes its index; the transport then sends the
//! handles out of band (`SCM_RIGHTS` on Unix, `DuplicateHandle` into the peer
//! on Windows). Deserialization takes the received handles back by index.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::RefCell;
use std::fs::File;
use std::io;

#[cfg(unix)]
pub(crate) type OsHandle = std::os::fd::OwnedFd;
#[cfg(windows)]
pub(crate) type OsHandle = std::os::windows::io::OwnedHandle;

/// Upper bound on handles attached to one message.
pub const MAX_HANDLES_PER_MESSAGE: usize = 16;

#[derive(Debug)]
pub struct Handle(pub(crate) OsHandle);

impl Handle {
    pub fn from_file(f: File) -> Self {
        Handle(f.into())
    }

    /// Reinterpret the handle as a `File` (read-only files, pipes, memfds).
    pub fn into_file(self) -> File {
        File::from(self.0)
    }

    pub fn try_clone(&self) -> io::Result<Handle> {
        self.0.try_clone().map(Handle)
    }

    pub(crate) fn from_os(h: OsHandle) -> Self {
        Handle(h)
    }
}

impl From<File> for Handle {
    fn from(f: File) -> Self {
        Handle::from_file(f)
    }
}

thread_local! {
    static OUTGOING: RefCell<Option<Vec<Handle>>> = const { RefCell::new(None) };
    static INCOMING: RefCell<Option<Vec<Option<Handle>>>> = const { RefCell::new(None) };
}

/// Run `f` (a serialization) collecting every [`Handle`] it encounters.
pub(crate) fn collect_outgoing<T>(f: impl FnOnce() -> T) -> (T, Vec<Handle>) {
    OUTGOING.with(|o| *o.borrow_mut() = Some(Vec::new()));
    let out = f();
    let handles = OUTGOING.with(|o| o.borrow_mut().take()).unwrap_or_default();
    (out, handles)
}

/// Run `f` (a deserialization) with `handles` available to [`Handle`] fields.
pub(crate) fn provide_incoming<T>(handles: Vec<Handle>, f: impl FnOnce() -> T) -> T {
    INCOMING.with(|i| *i.borrow_mut() = Some(handles.into_iter().map(Some).collect()));
    let out = f();
    INCOMING.with(|i| *i.borrow_mut() = None);
    out
}

impl Serialize for Handle {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let idx = OUTGOING.with(|o| {
            let mut o = o.borrow_mut();
            let v = o.as_mut().ok_or_else(|| {
                serde::ser::Error::custom("Handle serialized outside Sender::send")
            })?;
            if v.len() >= MAX_HANDLES_PER_MESSAGE {
                return Err(serde::ser::Error::custom("too many handles in one message"));
            }
            // The message keeps ownership of its own copy; the transport sends the clone.
            let dup = self.try_clone().map_err(serde::ser::Error::custom)?;
            v.push(dup);
            Ok((v.len() - 1) as u8)
        })?;
        s.serialize_u8(idx)
    }
}

impl<'de> Deserialize<'de> for Handle {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let idx = u8::deserialize(d)? as usize;
        INCOMING.with(|i| {
            let mut i = i.borrow_mut();
            let slot = i
                .as_mut()
                .and_then(|v| v.get_mut(idx))
                .ok_or_else(|| serde::de::Error::custom("handle index out of range"))?;
            slot.take()
                .ok_or_else(|| serde::de::Error::custom("handle used twice"))
        })
    }
}
