//! Shared memory regions for pixels and snapshot sections.
//!
//! A [`SharedRegion`] is an anonymous shared mapping plus the handle that
//! lets a peer map the same pages: a `memfd` on Linux, an unlinked temp file
//! on other Unix systems, a pagefile-backed section object on Windows. It
//! serializes as (handle, length) and maps itself on deserialization, so
//! messages carry regions as ordinary fields.
//!
//! Ownership discipline lives in the protocol (a region has one writer at a
//! time, the one that was asked to fill it); the accessors copy, so no Rust
//! reference to concurrently-written memory is handed out by default.

use crate::handle::Handle;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io;

#[derive(Debug)]
pub struct SharedRegion {
    map: Mapping,
    handle: Handle,
}

// SAFETY: the mapping is plain shared memory; synchronisation between
// processes is the protocol's job and accessors copy.
unsafe impl Send for SharedRegion {}
unsafe impl Sync for SharedRegion {}

impl SharedRegion {
    /// Create a zero-filled region of `len` bytes (at least 1).
    pub fn create(len: usize) -> io::Result<Self> {
        let len = len.max(1);
        let handle = create_backing(len)?;
        let map = Mapping::map(&handle, len)?;
        Ok(Self { map, handle })
    }

    /// Map an existing region received from a peer.
    pub fn from_handle(handle: Handle, len: usize) -> io::Result<Self> {
        let map = Mapping::map(&handle, len.max(1))?;
        Ok(Self { map, handle })
    }

    pub fn len(&self) -> usize {
        self.map.len
    }

    pub fn is_empty(&self) -> bool {
        self.map.len == 0
    }

    /// Copy `data` to `offset`. Panics if out of range.
    pub fn write_at(&self, offset: usize, data: &[u8]) {
        assert!(
            offset
                .checked_add(data.len())
                .is_some_and(|e| e <= self.map.len)
        );
        // SAFETY: bounds checked above; the mapping lives as long as `self`.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), self.map.ptr.add(offset), data.len())
        };
    }

    /// Copy bytes at `offset` into `out`. Panics if out of range.
    pub fn read_at(&self, offset: usize, out: &mut [u8]) {
        assert!(
            offset
                .checked_add(out.len())
                .is_some_and(|e| e <= self.map.len)
        );
        // SAFETY: bounds checked above.
        unsafe {
            std::ptr::copy_nonoverlapping(self.map.ptr.add(offset), out.as_mut_ptr(), out.len())
        };
    }

    pub fn to_vec(&self, offset: usize, len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len];
        self.read_at(offset, &mut v);
        v
    }

    /// Borrow the bytes without copying.
    ///
    /// # Safety
    /// The peer must not write the range while the slice is alive; the
    /// protocol guarantees this once the response that completes the write
    /// has been received.
    pub unsafe fn slice(&self, offset: usize, len: usize) -> &[u8] {
        assert!(offset.checked_add(len).is_some_and(|e| e <= self.map.len));
        // SAFETY: bounds checked; exclusivity is the caller's contract.
        unsafe { std::slice::from_raw_parts(self.map.ptr.add(offset), len) }
    }

    /// Another process-visible handle to the same pages.
    pub fn try_clone_handle(&self) -> io::Result<Handle> {
        self.handle.try_clone()
    }
}

impl Serialize for SharedRegion {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (&self.handle, self.map.len as u64).serialize(s)
    }
}

impl<'de> Deserialize<'de> for SharedRegion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (handle, len) = <(Handle, u64)>::deserialize(d)?;
        let len = usize::try_from(len).map_err(serde::de::Error::custom)?;
        SharedRegion::from_handle(handle, len).map_err(serde::de::Error::custom)
    }
}

#[cfg(unix)]
mod os {
    use super::*;
    use std::os::fd::{AsRawFd, OwnedFd};

    #[derive(Debug)]
    pub struct Mapping {
        pub ptr: *mut u8,
        pub len: usize,
    }

    impl Mapping {
        pub fn map(h: &Handle, len: usize) -> io::Result<Self> {
            // SAFETY: valid fd, MAP_SHARED read/write mapping of `len` bytes.
            let p = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    h.0.as_raw_fd(),
                    0,
                )
            };
            if p == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { ptr: p.cast(), len })
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: unmapping exactly what mmap returned.
            unsafe { libc::munmap(self.ptr.cast(), self.len) };
        }
    }

    pub fn create_backing(len: usize) -> io::Result<Handle> {
        let fd = backing_fd()?;
        // SAFETY: valid fd; sets the file size (sparse, zero-filled).
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Handle::from_os(fd))
    }

    #[cfg(target_os = "linux")]
    fn backing_fd() -> io::Result<OwnedFd> {
        // SAFETY: NUL-terminated name, valid flags.
        let fd = unsafe { libc::memfd_create(c"papyrine-shm".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd is a fresh descriptor we own.
        Ok(unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) })
    }

    /// Unlinked temp file (works inside a sandbox that may write only its temp dir).
    #[cfg(not(target_os = "linux"))]
    fn backing_fd() -> io::Result<OwnedFd> {
        use std::os::unix::fs::OpenOptionsExt;
        let dir = std::env::temp_dir();
        for i in 0..64u32 {
            let name = dir.join(format!(
                ".papyrine-shm-{}-{}-{i}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)
            ));
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC)
                .open(&name)
            {
                Ok(f) => {
                    let _ = std::fs::remove_file(&name);
                    return Ok(f.into());
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other(
            "could not create shared memory backing file",
        ))
    }
}

#[cfg(windows)]
mod os {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        PAGE_READWRITE, UnmapViewOfFile,
    };

    #[derive(Debug)]
    pub struct Mapping {
        pub ptr: *mut u8,
        pub len: usize,
    }

    impl Mapping {
        pub fn map(h: &Handle, len: usize) -> io::Result<Self> {
            // SAFETY: valid section handle; maps `len` bytes read/write.
            let v =
                unsafe { MapViewOfFile(h.0.as_raw_handle() as _, FILE_MAP_ALL_ACCESS, 0, 0, len) };
            if v.Value.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                ptr: v.Value.cast(),
                len,
            })
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: unmapping the view we mapped.
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.ptr.cast(),
                })
            };
        }
    }

    pub fn create_backing(len: usize) -> io::Result<Handle> {
        let hi = ((len as u64) >> 32) as u32;
        let lo = (len as u64 & 0xFFFF_FFFF) as u32;
        // SAFETY: pagefile-backed anonymous section, default security.
        let h = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                hi,
                lo,
                std::ptr::null(),
            )
        };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh handle we own.
        let owned = unsafe { OwnedHandle::from_raw_handle(h as _) };
        Ok(Handle::from_os(owned))
    }
}

use os::{Mapping, create_backing};

/// A read-only mapping of a file the host opened for us (the brokered way for
/// a sandboxed child to read the user's document).
#[derive(Debug)]
pub struct MappedFile {
    map: os_ro::RoMapping,
}

// SAFETY: read-only mapping, no interior mutation.
unsafe impl Send for MappedFile {}
unsafe impl Sync for MappedFile {}

impl MappedFile {
    /// Map `len` bytes of the file behind `handle` (read-only, private).
    /// The host must not truncate the file while it is mapped.
    pub fn map(handle: &Handle, len: u64) -> io::Result<Self> {
        let len = usize::try_from(len).map_err(io::Error::other)?;
        Ok(Self {
            map: os_ro::RoMapping::map(handle, len)?,
        })
    }
}

impl AsRef<[u8]> for MappedFile {
    fn as_ref(&self) -> &[u8] {
        self.map.as_slice()
    }
}

/// Bytes backed by a [`SharedRegion`] that is no longer written by anyone.
#[derive(Debug)]
pub struct SharedBytes {
    pub region: SharedRegion,
    pub len: usize,
}

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        // SAFETY: the protocol hands over regions as immutable snapshots.
        unsafe { self.region.slice(0, self.len.min(self.region.len())) }
    }
}

#[cfg(unix)]
mod os_ro {
    use super::*;
    use std::os::fd::AsRawFd;

    #[derive(Debug)]
    pub struct RoMapping {
        ptr: *mut u8,
        len: usize,
    }

    impl RoMapping {
        pub fn map(h: &Handle, len: usize) -> io::Result<Self> {
            if len == 0 {
                return Ok(Self {
                    ptr: std::ptr::NonNull::dangling().as_ptr(),
                    len: 0,
                });
            }
            // SAFETY: valid fd; read-only private mapping.
            let p = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ,
                    libc::MAP_PRIVATE,
                    h.0.as_raw_fd(),
                    0,
                )
            };
            if p == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { ptr: p.cast(), len })
        }
        pub fn as_slice(&self) -> &[u8] {
            // SAFETY: mapping of `len` readable bytes (or a dangling pointer with len 0).
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    impl Drop for RoMapping {
        fn drop(&mut self) {
            if self.len != 0 {
                // SAFETY: unmapping exactly what mmap returned.
                unsafe { libc::munmap(self.ptr.cast(), self.len) };
            }
        }
    }
}

#[cfg(windows)]
mod os_ro {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        PAGE_READONLY, UnmapViewOfFile,
    };

    #[derive(Debug)]
    pub struct RoMapping {
        ptr: *mut u8,
        len: usize,
    }

    impl RoMapping {
        pub fn map(h: &Handle, len: usize) -> io::Result<Self> {
            if len == 0 {
                return Ok(Self {
                    ptr: std::ptr::NonNull::dangling().as_ptr(),
                    len: 0,
                });
            }
            // SAFETY: valid file handle; read-only section over the whole file.
            let m = unsafe {
                CreateFileMappingW(
                    h.0.as_raw_handle() as _,
                    std::ptr::null(),
                    PAGE_READONLY,
                    0,
                    0,
                    std::ptr::null(),
                )
            };
            if m.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: fresh handle; closed on drop once the view exists.
            let section = unsafe { OwnedHandle::from_raw_handle(m as _) };
            // SAFETY: valid section handle.
            let v =
                unsafe { MapViewOfFile(section.as_raw_handle() as _, FILE_MAP_READ, 0, 0, len) };
            if v.Value.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                ptr: v.Value.cast(),
                len,
            })
        }
        pub fn as_slice(&self) -> &[u8] {
            // SAFETY: view of `len` readable bytes (or dangling with len 0).
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    impl Drop for RoMapping {
        fn drop(&mut self) {
            if self.len != 0 {
                // SAFETY: unmapping the view we mapped.
                unsafe {
                    UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: self.ptr.cast(),
                    })
                };
            }
        }
    }
}
