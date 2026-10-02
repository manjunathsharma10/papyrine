//! Windows transport: two anonymous pipes (one per direction); handles are
//! duplicated into the peer process and their values travel in the frame.

use super::{Endpoint, check_len};
use crate::handle::{Handle, MAX_HANDLES_PER_MESSAGE};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::Arc;
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_BROKEN_PIPE, HANDLE, HANDLE_FLAG_INHERIT,
    SetHandleInformation,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// The process that receives handles we send. `None` on the child side of a
/// restricted spawn (a sandboxed child never gets a handle to the host).
#[derive(Clone)]
pub struct Peer(Option<Arc<PeerInner>>);

enum PeerInner {
    /// Pseudo-handle for this very process (in-process pairs).
    Current,
    Process(OwnedHandle),
}

impl Peer {
    pub fn none() -> Self {
        Peer(None)
    }
    pub fn current() -> Self {
        Peer(Some(Arc::new(PeerInner::Current)))
    }
    pub fn process(h: OwnedHandle) -> Self {
        Peer(Some(Arc::new(PeerInner::Process(h))))
    }
    fn raw(&self) -> Option<HANDLE> {
        match self.0.as_deref()? {
            // SAFETY: GetCurrentProcess never fails.
            PeerInner::Current => Some(unsafe { GetCurrentProcess() }),
            PeerInner::Process(h) => Some(h.as_raw_handle() as HANDLE),
        }
    }
}

pub struct RawSender {
    file: File,
    peer: Peer,
}

pub struct RawReceiver {
    file: File,
}

/// One unidirectional pipe: (read end, write end). Both non-inheritable.
pub fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut r: HANDLE = std::ptr::null_mut();
    let mut w: HANDLE = std::ptr::null_mut();
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 0,
    };
    // SAFETY: out-pointers are valid.
    if unsafe { CreatePipe(&mut r, &mut w, &sa, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fresh handles we own.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(r as RawHandle),
            OwnedHandle::from_raw_handle(w as RawHandle),
        )
    })
}

/// Allow (or forbid) a handle to be inherited by a child created with an
/// explicit handle list.
pub fn set_inheritable(h: &OwnedHandle, on: bool) -> io::Result<()> {
    // SAFETY: valid handle.
    let ok = unsafe {
        SetHandleInformation(
            h.as_raw_handle() as HANDLE,
            HANDLE_FLAG_INHERIT,
            if on { HANDLE_FLAG_INHERIT } else { 0 },
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn endpoint_from_pipes(tx: OwnedHandle, rx: OwnedHandle, peer: Peer) -> Endpoint {
    Endpoint::from_raw(
        RawSender {
            file: File::from(tx),
            peer,
        },
        RawReceiver {
            file: File::from(rx),
        },
    )
}

pub fn pair() -> io::Result<(Endpoint, Endpoint)> {
    let (r1, w1) = pipe()?; // a -> b
    let (r2, w2) = pipe()?; // b -> a
    Ok((
        endpoint_from_pipes(w1, r2, Peer::current()),
        endpoint_from_pipes(w2, r1, Peer::current()),
    ))
}

/// Child side: handle values passed through the environment.
pub fn endpoint_from_env() -> io::Result<Endpoint> {
    let get = |k: &str| -> io::Result<usize> {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{k} not set")))
    };
    let rx = get("PAPYRINE_IPC_RX")?;
    let tx = get("PAPYRINE_IPC_TX")?;
    // SAFETY: the parent inherited these handles into this process for our exclusive use.
    let (rx, tx) = unsafe {
        (
            OwnedHandle::from_raw_handle(rx as RawHandle),
            OwnedHandle::from_raw_handle(tx as RawHandle),
        )
    };
    Ok(endpoint_from_pipes(tx, rx, Peer::none()))
}

impl RawSender {
    pub fn send_frame(&mut self, payload: &[u8], handles: Vec<Handle>) -> io::Result<()> {
        check_len(payload.len())?;
        let mut values = Vec::with_capacity(handles.len());
        if !handles.is_empty() {
            let target = self.peer.raw().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "this endpoint cannot send handles",
                )
            })?;
            for h in &handles {
                let mut dst: HANDLE = std::ptr::null_mut();
                // SAFETY: valid source handle in this process; target is the peer process.
                let ok = unsafe {
                    DuplicateHandle(
                        GetCurrentProcess(),
                        h.0.as_raw_handle() as HANDLE,
                        target,
                        &mut dst,
                        0,
                        0,
                        DUPLICATE_SAME_ACCESS,
                    )
                };
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                values.push(dst as usize as u64);
            }
        }
        let mut buf = Vec::with_capacity(5 + values.len() * 8 + payload.len());
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.push(values.len() as u8);
        for v in &values {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(payload);
        self.file.write_all(&buf)
    }
}

fn read_full(f: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => break,
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

impl RawReceiver {
    pub fn recv_frame(&mut self) -> io::Result<Option<(Vec<u8>, Vec<Handle>)>> {
        let mut head = [0u8; 5];
        let n = read_full(&mut self.file, &mut head)?;
        if n == 0 {
            return Ok(None);
        }
        if n < head.len() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let len = check_len(u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize)?;
        let count = head[4] as usize;
        if count > MAX_HANDLES_PER_MESSAGE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many handles",
            ));
        }
        let mut tab = vec![0u8; count * 8];
        if read_full(&mut self.file, &mut tab)? < tab.len() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let mut payload = vec![0u8; len];
        if read_full(&mut self.file, &mut payload)? < len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let handles = tab
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| {
                let v = u64::from_le_bytes(*c) as usize;
                // SAFETY: the sender duplicated this handle into our process for us to own.
                Handle::from_os(unsafe { OwnedHandle::from_raw_handle(v as RawHandle) })
            })
            .collect();
        Ok(Some((payload, handles)))
    }
}
