//! Unix transport: one `AF_UNIX`/`SOCK_STREAM` socket, `SCM_RIGHTS` for handles.

use super::{Endpoint, check_len};
use crate::handle::{Handle, MAX_HANDLES_PER_MESSAGE};
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

pub struct RawSender {
    fd: OwnedFd,
}

pub struct RawReceiver {
    fd: OwnedFd,
}

#[cfg(target_os = "linux")]
const SEND_FLAGS: libc::c_int = libc::MSG_NOSIGNAL;
#[cfg(not(target_os = "linux"))]
const SEND_FLAGS: libc::c_int = 0;

fn cvt(r: libc::ssize_t) -> io::Result<usize> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r as usize)
    }
}

fn set_cloexec(fd: RawFd) {
    // SAFETY: fcntl on a descriptor we own.
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFD);
        if fl >= 0 {
            libc::fcntl(fd, libc::F_SETFD, fl | libc::FD_CLOEXEC);
        }
    }
}

/// A connected stream socket pair with both ends close-on-exec.
pub fn socketpair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: fds has room for two descriptors.
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for &fd in &fds {
        set_cloexec(fd);
        #[cfg(target_vendor = "apple")]
        // SAFETY: setsockopt with a c_int value on our socket.
        unsafe {
            let one: libc::c_int = 1;
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_NOSIGPIPE,
                (&one as *const libc::c_int).cast(),
                mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }
    }
    // SAFETY: both are fresh descriptors we own.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Endpoint over an already-connected socket (inherited by a child).
pub fn endpoint_from_fd(fd: OwnedFd) -> io::Result<Endpoint> {
    let dup = fd.try_clone()?;
    Ok(Endpoint::from_raw(
        RawSender { fd },
        RawReceiver { fd: dup },
    ))
}

pub fn pair() -> io::Result<(Endpoint, Endpoint)> {
    let (a, b) = socketpair()?;
    Ok((endpoint_from_fd(a)?, endpoint_from_fd(b)?))
}

impl RawSender {
    pub fn send_frame(&mut self, payload: &[u8], handles: Vec<Handle>) -> io::Result<()> {
        check_len(payload.len())?;
        let mut buf = Vec::with_capacity(5 + payload.len());
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.push(handles.len() as u8);
        buf.extend_from_slice(payload);

        let fds: Vec<RawFd> = handles.iter().map(|h| h.0.as_raw_fd()).collect();
        let mut sent = 0usize;
        // The descriptors ride with the first bytes of the frame.
        if !fds.is_empty() {
            sent = sendmsg_with_fds(self.fd.as_raw_fd(), &buf, &fds)?;
        }
        while sent < buf.len() {
            // SAFETY: pointer/length come from a live slice.
            let r = unsafe {
                libc::send(
                    self.fd.as_raw_fd(),
                    buf[sent..].as_ptr().cast(),
                    buf.len() - sent,
                    SEND_FLAGS,
                )
            };
            match cvt(r) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => sent += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

fn sendmsg_with_fds(sock: RawFd, buf: &[u8], fds: &[RawFd]) -> io::Result<usize> {
    let fd_bytes = mem::size_of_val(fds);
    // SAFETY: CMSG_SPACE is a pure size computation.
    let space = unsafe { libc::CMSG_SPACE(fd_bytes as u32) } as usize;
    let mut cbuf = vec![0u8; space];
    let mut iov = libc::iovec {
        iov_base: buf.as_ptr() as *mut libc::c_void,
        iov_len: buf.len(),
    };
    // SAFETY: zeroed msghdr is a valid empty header; fields are filled below.
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cbuf.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: msg_control points at `space` bytes, enough for one cmsg.
    unsafe {
        let c = libc::CMSG_FIRSTHDR(&msg);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(fd_bytes as u32) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr().cast::<u8>(), libc::CMSG_DATA(c), fd_bytes);
    }
    loop {
        // SAFETY: msg and the buffers it references outlive the call.
        match cvt(unsafe { libc::sendmsg(sock, &msg, SEND_FLAGS) }) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

impl RawReceiver {
    pub fn recv_frame(&mut self) -> io::Result<Option<(Vec<u8>, Vec<Handle>)>> {
        let mut head = [0u8; 5];
        let mut handles: Vec<Handle> = Vec::new();
        let mut got = 0;
        while got < head.len() {
            let n = self.recvmsg(&mut head[got..], &mut handles)?;
            if n == 0 {
                if got == 0 && handles.is_empty() {
                    return Ok(None);
                }
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            got += n;
        }
        let len = check_len(u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize)?;
        let declared = head[4] as usize;
        if declared > MAX_HANDLES_PER_MESSAGE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many handles",
            ));
        }
        let mut payload = vec![0u8; len];
        let mut got = 0;
        while got < len {
            let n = self.recvmsg(&mut payload[got..], &mut handles)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            got += n;
        }
        if handles.len() != declared {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected {declared} handles, received {}", handles.len()),
            ));
        }
        Ok(Some((payload, handles)))
    }

    fn recvmsg(&self, buf: &mut [u8], handles: &mut Vec<Handle>) -> io::Result<usize> {
        // SAFETY: pure size computation.
        let space =
            unsafe { libc::CMSG_SPACE((MAX_HANDLES_PER_MESSAGE * mem::size_of::<RawFd>()) as u32) }
                as usize;
        let mut cbuf = vec![0u8; space];
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: buf.len(),
        };
        // SAFETY: zeroed msghdr, then filled.
        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_CMSG_CLOEXEC;
        #[cfg(not(target_os = "linux"))]
        let flags = 0;
        let n = loop {
            // SAFETY: msg references live buffers.
            match cvt(unsafe { libc::recvmsg(self.fd.as_raw_fd(), &mut msg, flags) }) {
                Ok(n) => break n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        };
        // SAFETY: walking the control messages the kernel wrote into cbuf.
        unsafe {
            let mut c = libc::CMSG_FIRSTHDR(&msg);
            while !c.is_null() {
                if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                    let data = libc::CMSG_DATA(c);
                    let bytes = (*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                    for i in 0..bytes / mem::size_of::<RawFd>() {
                        let mut raw: RawFd = 0;
                        std::ptr::copy_nonoverlapping(
                            data.add(i * mem::size_of::<RawFd>()),
                            (&mut raw as *mut RawFd).cast::<u8>(),
                            mem::size_of::<RawFd>(),
                        );
                        set_cloexec(raw);
                        handles.push(Handle::from_os(OwnedFd::from_raw_fd(raw)));
                    }
                }
                c = libc::CMSG_NXTHDR(&msg, c);
            }
        }
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control data truncated",
            ));
        }
        Ok(n)
    }
}

/// Take the inherited descriptor named by `PAPYRINE_IPC_FD`.
pub fn endpoint_from_env() -> io::Result<Endpoint> {
    let v = std::env::var("PAPYRINE_IPC_FD")
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "PAPYRINE_IPC_FD not set"))?;
    let raw: RawFd = v
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad PAPYRINE_IPC_FD"))?;
    // SAFETY: the parent passed this descriptor to us for exclusive use.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    set_cloexec(raw);
    endpoint_from_fd(fd)
}
