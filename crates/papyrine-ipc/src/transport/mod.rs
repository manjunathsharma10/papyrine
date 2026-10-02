//! Length-prefixed message framing over a Unix socket (macOS, Linux) or a pair
//! of anonymous pipes (Windows), with out-of-band handle passing.
//!
//! Why not `ipc-channel`: it needs a Mach bootstrap server lookup on macOS
//! (which a deny-default Seatbelt profile has to open up), spawns a router
//! thread per receiver and offers no Windows story for child-to-host handles
//! under a restricted token. The transport here is inherited at spawn, needs
//! no names, and works unchanged inside every sandbox.
//!
//! Frame: `u32 LE payload length`, `u8 handle count`, [Windows only:
//! `count * u64` handle values valid in the *receiver*], payload.

use crate::handle::{MAX_HANDLES_PER_MESSAGE, collect_outgoing, provide_incoming};
use serde::{Serialize, de::DeserializeOwned};
use std::io;
use std::sync::Mutex;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
pub(crate) mod windows;

#[cfg(unix)]
use unix as os;
#[cfg(windows)]
use windows as os;

/// Largest accepted frame. Bulk data (pixels, sections) goes through shared
/// memory; inline payloads beyond this are a bug or an attack.
pub const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    #[error("message encoding error: {0}")]
    Encode(postcard::Error),
    #[error("message decoding error: {0}")]
    Decode(postcard::Error),
    #[error("frame of {0} bytes exceeds the limit")]
    TooLarge(usize),
    #[error("peer closed the connection")]
    Closed,
}

pub(crate) use os::{RawReceiver, RawSender};
#[cfg(unix)]
pub(crate) use unix::{endpoint_from_fd, socketpair};

pub(crate) use os::endpoint_from_env;

/// Thread-safe send half; whole frames are written under a lock.
pub struct Sender {
    raw: Mutex<RawSender>,
}

/// Receive half (one reader at a time).
pub struct Receiver {
    raw: RawReceiver,
}

/// Both halves of one connection.
pub struct Endpoint {
    pub tx: Sender,
    pub rx: Receiver,
}

impl Sender {
    pub fn send<T: Serialize>(&self, msg: &T) -> Result<(), TransportError> {
        let (bytes, handles) = collect_outgoing(|| postcard::to_stdvec(msg));
        let bytes = bytes.map_err(TransportError::Encode)?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(TransportError::TooLarge(bytes.len()));
        }
        debug_assert!(handles.len() <= MAX_HANDLES_PER_MESSAGE);
        let mut raw = self.raw.lock().unwrap_or_else(|p| p.into_inner());
        raw.send_frame(&bytes, handles)?;
        Ok(())
    }
}

impl Receiver {
    /// Next message, or `Ok(None)` when the peer closed cleanly between frames.
    pub fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>, TransportError> {
        let Some((bytes, handles)) = self.raw.recv_frame()? else {
            return Ok(None);
        };
        let v = provide_incoming(handles, || postcard::from_bytes::<T>(&bytes))
            .map_err(TransportError::Decode)?;
        Ok(Some(v))
    }
}

impl Endpoint {
    pub(crate) fn from_raw(tx: RawSender, rx: RawReceiver) -> Self {
        Endpoint {
            tx: Sender {
                raw: Mutex::new(tx),
            },
            rx: Receiver { raw: rx },
        }
    }

    /// Two connected endpoints inside this process (tests, in-process engines).
    pub fn pair() -> io::Result<(Endpoint, Endpoint)> {
        os::pair()
    }
}

pub(crate) fn check_len(len: usize) -> io::Result<usize> {
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds the limit"),
        ));
    }
    Ok(len)
}
