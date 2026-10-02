//! Durable-write primitives: full fsync, directory fsync, metadata carry-over, atomic rename.

use std::fs::{self, File};
use std::io;
use std::path::Path;

/// Flush to stable storage. macOS needs `F_FULLFSYNC`: plain `fsync` only reaches the drive's
/// cache there. Falls back to `fsync` when the filesystem refuses it (network mounts).
pub fn full_sync(f: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: valid open descriptor; F_FULLFSYNC takes no argument.
        if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_FULLFSYNC) } == 0 {
            return Ok(());
        }
    }
    f.sync_all()
}

/// Make a rename inside `dir` durable.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        full_sync(&File::open(dir)?)
    }
    #[cfg(not(unix))]
    {
        // Windows has no directory handle sync; ReplaceFileW with WRITE_THROUGH covers it.
        let _ = dir;
        Ok(())
    }
}

/// Copy permissions, ownership (best effort), extended attributes and ACLs from `from` (the file
/// being replaced) to `to` (the new file), before the rename.
pub fn copy_metadata(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::metadata(from)?;
    fs::set_permissions(to, meta.permissions())?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;
        let src = File::open(from)?;
        let dst = File::options().read(true).write(true).open(to)?;
        // SAFETY: valid descriptors. Ownership change fails for non-root users on foreign files:
        // that is expected and ignored.
        unsafe {
            let _ = libc::fchown(dst.as_raw_fd(), meta.uid(), meta.gid());
        }
        copy_xattrs(&src, &dst)?;
        // chown may have cleared setuid/setgid bits; restore the mode.
        fs::set_permissions(to, meta.permissions())?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn copy_xattrs(src: &File, dst: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn fcopyfile(
            from: libc::c_int,
            to: libc::c_int,
            state: *mut libc::c_void,
            flags: u32,
        ) -> libc::c_int;
    }
    const COPYFILE_ACL: u32 = 1 << 0;
    const COPYFILE_XATTR: u32 = 1 << 2;
    // SAFETY: valid descriptors, null state is allowed.
    let r = unsafe {
        fcopyfile(
            src.as_raw_fd(),
            dst.as_raw_fd(),
            std::ptr::null_mut(),
            COPYFILE_ACL | COPYFILE_XATTR,
        )
    };
    if r == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    // Filesystems without xattr support (FAT, some network mounts) are not an error.
    match e.raw_os_error() {
        Some(libc::ENOTSUP | libc::ENOSYS | libc::EINVAL) => Ok(()),
        _ => Err(e),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn copy_xattrs(src: &File, dst: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let (s, d) = (src.as_raw_fd(), dst.as_raw_fd());
    // SAFETY: standard size-probe then fill pattern on valid descriptors and owned buffers.
    unsafe {
        let n = libc::flistxattr(s, std::ptr::null_mut(), 0);
        if n <= 0 {
            return Ok(()); // none, or unsupported
        }
        let mut names = vec![0u8; n as usize];
        let n = libc::flistxattr(s, names.as_mut_ptr().cast(), names.len());
        if n <= 0 {
            return Ok(());
        }
        names.truncate(n as usize);
        for name in names.split(|&b| b == 0).filter(|n| !n.is_empty()) {
            let mut cname = name.to_vec();
            cname.push(0);
            let len = libc::fgetxattr(s, cname.as_ptr().cast(), std::ptr::null_mut(), 0);
            if len < 0 {
                continue;
            }
            let mut val = vec![0u8; len as usize];
            let len = libc::fgetxattr(s, cname.as_ptr().cast(), val.as_mut_ptr().cast(), val.len());
            if len < 0 {
                continue;
            }
            // Namespaces we may not be allowed to set (security.*, trusted.*) are skipped.
            let _ = libc::fsetxattr(
                d,
                cname.as_ptr().cast(),
                val.as_ptr().cast(),
                len as usize,
                0,
            );
        }
    }
    Ok(())
}

/// Atomically make `new` the file at `target`.
pub fn replace_file(new: &Path, target: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows_replace(new, target)
    }
    #[cfg(not(windows))]
    {
        fs::rename(new, target)
    }
}

#[cfg(windows)]
fn windows_replace(new: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACEFILE_WRITE_THROUGH,
        ReplaceFileW,
    };
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(Some(0)).collect() };
    let (n, t) = (wide(new), wide(target));
    // SAFETY: NUL-terminated UTF-16 buffers that outlive the call.
    let ok = unsafe {
        if target.exists() {
            // Keeps the target's attributes, ACLs, streams and creation time.
            ReplaceFileW(
                t.as_ptr(),
                n.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                std::ptr::null(),
                std::ptr::null(),
            )
        } else {
            MoveFileExW(
                n.as_ptr(),
                t.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
