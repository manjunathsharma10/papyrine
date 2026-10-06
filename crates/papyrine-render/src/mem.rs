//! Process memory accounting and relief for the renderer (ARCHITECTURE §1.2).
//!
//! * [`footprint`] is the number the large-document gate counts: macOS
//!   `phys_footprint`, Linux anonymous resident pages, Windows private bytes.
//! * [`relief`] asks the C allocator to hand free pages back to the OS
//!   (`malloc_trim` on glibc). On macOS it is a no-op in practice: libmalloc
//!   caches freed large blocks and only gives them back when the process
//!   started with [`CHILD_ENV`] (see below).
//!
//! **macOS allocator cache.** Measured on macOS 27: with libmalloc defaults a
//! freed 26 MB block stays dirty in the process for good and
//! `malloc_zone_pressure_relief` returns 0, so PDFium's transient image-decode
//! buffers pin about 200 MB. `MallocSpaceEfficient=1` read at process start
//! makes libmalloc return them at `free`. The variable is read once, before
//! `main`, so the host has to put it in the renderer child's environment
//! ([`CHILD_ENV`], or [`reexec_with_child_env`] as the first call of the
//! renderer role).

/// Environment the renderer child must start with, as `(name, value)` pairs.
/// Empty where the platform allocator already returns memory promptly.
#[cfg(target_os = "macos")]
pub const CHILD_ENV: &[(&str, &str)] = &[("MallocSpaceEfficient", "1")];
#[cfg(not(target_os = "macos"))]
pub const CHILD_ENV: &[(&str, &str)] = &[];

/// True when every variable of [`CHILD_ENV`] is set in this process.
pub fn child_env_active() -> bool {
    CHILD_ENV
        .iter()
        .all(|(k, v)| std::env::var(k).is_ok_and(|x| x == *v))
}

/// If [`CHILD_ENV`] is missing, re-execute the current executable with it set
/// (same arguments, same inherited descriptors). Returns `Ok(())` when nothing
/// had to be done; on success after a re-exec it never returns.
///
/// Call it first thing in the renderer role, before the sandbox is applied
/// (a sandbox that denies `exec` would make it fail; the error is returned and
/// the renderer then runs with the allocator cache, over budget).
#[cfg(unix)]
pub fn reexec_with_child_env() -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    if child_env_active() {
        return Ok(());
    }
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.args(std::env::args_os().skip(1));
    for (k, v) in CHILD_ENV {
        cmd.env(k, v);
    }
    Err(cmd.exec())
}

/// [`reexec_with_child_env`] only when this process was started as the renderer
/// child (`PAPYRINE_ROLE=renderer`), so a multi-role executable can call it
/// unconditionally as the first line of `main`, before `bootstrap()`.
pub fn reexec_if_renderer() -> std::io::Result<()> {
    match std::env::var("PAPYRINE_ROLE").as_deref() {
        Ok("renderer") => reexec_with_child_env(),
        _ => Ok(()),
    }
}

#[cfg(not(unix))]
pub fn reexec_with_child_env() -> std::io::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
mod os {
    #[repr(C)]
    struct TaskVmInfo {
        virtual_size: u64,
        region_count: i32,
        page_size: i32,
        resident_size: u64,
        resident_size_peak: u64,
        device: u64,
        device_peak: u64,
        internal: u64,
        internal_peak: u64,
        external: u64,
        external_peak: u64,
        reusable: u64,
        reusable_peak: u64,
        purgeable_volatile_pmap: u64,
        purgeable_volatile_resident: u64,
        purgeable_volatile_virtual: u64,
        compressed: u64,
        compressed_peak: u64,
        compressed_lifetime: u64,
        phys_footprint: u64,
        _rest: [u64; 32],
    }
    unsafe extern "C" {
        fn mach_task_self() -> u32;
        fn task_info(task: u32, flavor: u32, info: *mut TaskVmInfo, count: *mut u32) -> i32;
        fn malloc_zone_pressure_relief(zone: *mut core::ffi::c_void, goal: usize) -> usize;
    }
    const TASK_VM_INFO: u32 = 22;

    pub fn footprint() -> u64 {
        // SAFETY: plain Mach call into a zeroed, correctly sized out struct.
        unsafe {
            let mut i: TaskVmInfo = std::mem::zeroed();
            let mut count = (std::mem::size_of::<TaskVmInfo>() / 4) as u32;
            if task_info(mach_task_self(), TASK_VM_INFO, &mut i, &mut count) == 0 {
                i.phys_footprint
            } else {
                0
            }
        }
    }

    pub fn relief() {
        // SAFETY: a null zone means "all zones".
        unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
    }

    /// `rusage_info_v4`: a 16-byte uuid then 35 u64 fields; index 7 is
    /// `ri_phys_footprint`, index 28 `ri_lifetime_max_phys_footprint`.
    #[repr(C)]
    struct RusageV4 {
        uuid: [u8; 16],
        f: [u64; 35],
    }
    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut core::ffi::c_void) -> i32;
    }

    pub fn footprint_of(pid: u32) -> Option<(u64, u64)> {
        // SAFETY: zeroed out struct at least as large as rusage_info_v4.
        unsafe {
            let mut r: RusageV4 = std::mem::zeroed();
            if proc_pid_rusage(pid as i32, 4, &mut r as *mut _ as *mut _) != 0 {
                return None;
            }
            Some((r.f[7], r.f[28]))
        }
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod os {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }

    pub fn footprint() -> u64 {
        // statm: size resident shared ... (pages). Anonymous = resident - shared.
        let Ok(s) = std::fs::read_to_string("/proc/self/statm") else {
            return 0;
        };
        let mut f = s.split_whitespace().skip(1).map(|v| v.parse::<u64>().ok());
        let (Some(Some(res)), Some(Some(shared))) = (f.next(), f.next()) else {
            return 0;
        };
        res.saturating_sub(shared) * 4096
    }

    pub fn relief() {
        // SAFETY: glibc call with no preconditions.
        unsafe { malloc_trim(0) };
    }

    pub fn footprint_of(pid: u32) -> Option<(u64, u64)> {
        super::linux_footprint_of(pid)
    }
}

#[cfg(all(target_os = "linux", not(target_env = "gnu")))]
mod os {
    pub fn footprint() -> u64 {
        let Ok(s) = std::fs::read_to_string("/proc/self/statm") else {
            return 0;
        };
        let mut f = s.split_whitespace().skip(1).map(|v| v.parse::<u64>().ok());
        let (Some(Some(res)), Some(Some(shared))) = (f.next(), f.next()) else {
            return 0;
        };
        res.saturating_sub(shared) * 4096
    }

    pub fn relief() {}

    pub fn footprint_of(pid: u32) -> Option<(u64, u64)> {
        super::linux_footprint_of(pid)
    }
}

#[cfg(windows)]
mod os {
    #[repr(C)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(
            process: isize,
            counters: *mut ProcessMemoryCountersEx,
            cb: u32,
        ) -> i32;
    }

    pub fn footprint() -> u64 {
        // SAFETY: zeroed out struct of the size we pass; pseudo-handle needs no close.
        unsafe {
            let mut c: ProcessMemoryCountersEx = std::mem::zeroed();
            c.cb = std::mem::size_of::<ProcessMemoryCountersEx>() as u32;
            if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
                c.private_usage as u64
            } else {
                0
            }
        }
    }

    pub fn relief() {}

    pub fn footprint_of(_pid: u32) -> Option<(u64, u64)> {
        None
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod os {
    pub fn footprint() -> u64 {
        0
    }
    pub fn relief() {}
    pub fn footprint_of(_pid: u32) -> Option<(u64, u64)> {
        None
    }
}

/// Anonymous resident bytes of process `pid` from `/proc` (the peak is the
/// current value: callers that need a peak sample).
#[cfg(target_os = "linux")]
fn linux_footprint_of(pid: u32) -> Option<(u64, u64)> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let mut f = s.split_whitespace().skip(1).map(|v| v.parse::<u64>().ok());
    let (res, shared) = (f.next()??, f.next()??);
    let v = res.saturating_sub(shared) * 4096;
    Some((v, v))
}

/// Current memory footprint of this process in bytes (0 if unavailable).
pub fn footprint() -> u64 {
    os::footprint()
}

/// Footprint of another process owned by the same user, as
/// `(current, lifetime peak)` bytes. The peak is exact on macOS and equals the
/// current value on Linux (sample it); `None` where unsupported (Windows).
pub fn footprint_of(pid: u32) -> Option<(u64, u64)> {
    os::footprint_of(pid)
}

/// Give free heap pages back to the OS where the allocator allows it.
pub fn relief() {
    os::relief()
}
