//! Windows: AppContainer + Job object (restricted token + low integrity as a
//! degraded fallback).
//!
//! Windows restrictions are fixed when the process is created, so the *parent*
//! must use [`spawn_restricted`]; [`apply`] (called in the child through
//! `apply_child_sandbox`) verifies that the child really is confined and adds
//! process mitigation policies that can only be set from inside.
//!
//! **AppContainer** (default). The child's token is a Low-integrity
//! AppContainer token with no capabilities. That is what denies, with no
//! further setup, every object whose ACL does not name the container: the
//! user's profile and files, the registry, the network (no `internetClient`,
//! and loopback is isolated), and other processes. System DLLs stay loadable
//! because the OS directories grant `ALL APPLICATION PACKAGES`. The host
//! grants the container read access to the allowlisted directories
//! ([`Profile::read_dirs`]) and full access to the per-child temp directory
//! (created by [`create_child_dir`]).
//!
//! **Job object** (both modes): at most one process (no children), the
//! profile's memory cap, UI restrictions, and kill-on-close so the host
//! dying takes the child with it. `PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY`
//! additionally makes `CreateProcess` fail inside the child.
//!
//! **Restricted-token fallback** (only if the profile has `require_enforced`
//! false and AppContainer creation fails): privileges dropped, deny-only
//! groups, restricting SIDs, Untrusted integrity. It protects files but
//! *cannot* block sockets; the child reports that in `Report::degraded`.

use crate::{Error, Profile, Report, Result};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::*;

const SE_GROUP_LOGON_ID: u32 = 0xC000_0000;
const SE_GROUP_INTEGRITY: u32 = 0x20;
const PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES: usize = 0x0002_0009;
const PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY: usize = 0x0002_000E;
const PROCESS_CREATION_CHILD_PROCESS_RESTRICTED: u32 = 1;

/// Name of the AppContainer; the SID is derived from it, no profile is created.
const APPCONTAINER_NAME: &str = "Papyrine.Sandbox.Child";

/// Integrity level of the restricted-token fallback (Untrusted).
const FALLBACK_INTEGRITY_RID: u32 = 0;

/// `PROCESS_MITIGATION_CHILD_PROCESS_POLICY` (not exposed by windows-sys).
#[repr(C)]
struct ChildProcessPolicy {
    flags: u32,
}

fn last_err(what: &str) -> io::Error {
    let e = io::Error::last_os_error();
    io::Error::new(e.kind(), format!("{what}: {e}"))
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn own(h: HANDLE) -> OwnedHandle {
    // SAFETY: caller passes a freshly created handle it owns.
    unsafe { OwnedHandle::from_raw_handle(h as RawHandle) }
}

// ------------------------------------------------------------ sid helpers

enum SidAlloc {
    LocalAlloc,
    FreeSid,
}

struct Sid(*mut c_void, SidAlloc);

impl Sid {
    fn from_str(s: &str) -> io::Result<Sid> {
        let w = wide(OsStr::new(s));
        let mut p: *mut c_void = null_mut();
        // SAFETY: valid NUL-terminated wide string and out pointer.
        if unsafe { ConvertStringSidToSidW(w.as_ptr(), &mut p) } == 0 {
            return Err(last_err("ConvertStringSidToSidW"));
        }
        Ok(Sid(p, SidAlloc::LocalAlloc))
    }

    fn app_container() -> io::Result<Sid> {
        let name = wide(OsStr::new(APPCONTAINER_NAME));
        let mut p: *mut c_void = null_mut();
        // SAFETY: valid name and out pointer.
        let hr = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut p) };
        if hr < 0 {
            return Err(io::Error::other(format!(
                "DeriveAppContainerSidFromAppContainerName: HRESULT {hr:#x}"
            )));
        }
        Ok(Sid(p, SidAlloc::FreeSid))
    }

    fn to_string(&self) -> io::Result<String> {
        let mut p: *mut u16 = null_mut();
        // SAFETY: valid sid; out pointer.
        if unsafe { ConvertSidToStringSidW(self.0, &mut p) } == 0 {
            return Err(last_err("ConvertSidToStringSidW"));
        }
        let mut n = 0;
        // SAFETY: NUL-terminated wide string from the OS.
        while unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(
            // SAFETY: n valid elements.
            unsafe { std::slice::from_raw_parts(p, n) },
        );
        // SAFETY: LocalAlloc'd by the API.
        unsafe { LocalFree(p.cast()) };
        Ok(s)
    }
}

impl Drop for Sid {
    fn drop(&mut self) {
        match self.1 {
            // SAFETY: allocated by ConvertStringSidToSidW (LocalAlloc).
            SidAlloc::LocalAlloc => unsafe {
                LocalFree(self.0);
            },
            // SAFETY: allocated by DeriveAppContainerSid... (FreeSid).
            SidAlloc::FreeSid => unsafe {
                FreeSid(self.0);
            },
        }
    }
}

// ------------------------------------------------------------ token (fallback)

/// Groups kept enabled in the fallback token (everything else becomes deny-only).
const KEEP_GROUPS: &[&str] = &["S-1-1-0", "S-1-5-32-545", "S-1-5-11", "S-1-5-4", "S-1-2-0"];
/// Restricting SIDs: the object must also grant access to one of these.
const RESTRICTING: &[&str] = &["S-1-1-0", "S-1-5-32-545", "S-1-5-11", "S-1-5-12"];

fn token_info(tok: HANDLE, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u8>> {
    let mut len = 0u32;
    // SAFETY: size query.
    unsafe { GetTokenInformation(tok, class, null_mut(), 0, &mut len) };
    let mut buf = vec![0u8; len as usize];
    // SAFETY: buffer has `len` bytes.
    if unsafe { GetTokenInformation(tok, class, buf.as_mut_ptr().cast(), len, &mut len) } == 0 {
        return Err(last_err("GetTokenInformation"));
    }
    Ok(buf)
}

fn make_fallback_token() -> io::Result<OwnedHandle> {
    let mut cur: HANDLE = null_mut();
    // SAFETY: current process pseudo handle; out pointer.
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            &mut cur,
        )
    } == 0
    {
        return Err(last_err("OpenProcessToken"));
    }
    let cur = own(cur);

    // Deny-only list: every group not in KEEP_GROUPS, plus the user SID.
    let keep: Vec<Sid> = KEEP_GROUPS
        .iter()
        .map(|s| Sid::from_str(s))
        .collect::<io::Result<_>>()?;
    let groups = token_info(cur.as_raw_handle() as HANDLE, TokenGroups)?;
    // SAFETY: buffer holds a TOKEN_GROUPS.
    let tg = unsafe { &*(groups.as_ptr() as *const TOKEN_GROUPS) };
    let mut disable: Vec<SID_AND_ATTRIBUTES> = Vec::new();
    for i in 0..tg.GroupCount as usize {
        // SAFETY: i < GroupCount; the array is inline in the buffer.
        let g = unsafe { *tg.Groups.as_ptr().add(i) };
        if g.Attributes & SE_GROUP_LOGON_ID == SE_GROUP_LOGON_ID {
            continue; // keep the logon SID so the session's object namespace stays reachable
        }
        // SAFETY: valid sids.
        let keep_it = keep.iter().any(|k| unsafe { EqualSid(k.0, g.Sid) } != 0);
        if !keep_it {
            disable.push(SID_AND_ATTRIBUTES {
                Sid: g.Sid,
                Attributes: 0,
            });
        }
    }
    let user = token_info(cur.as_raw_handle() as HANDLE, TokenUser)?;
    // SAFETY: buffer holds a TOKEN_USER.
    let tu = unsafe { &*(user.as_ptr() as *const TOKEN_USER) };
    disable.push(SID_AND_ATTRIBUTES {
        Sid: tu.User.Sid,
        Attributes: 0,
    });

    let restricting: Vec<Sid> = RESTRICTING
        .iter()
        .map(|s| Sid::from_str(s))
        .collect::<io::Result<_>>()?;
    let restrict: Vec<SID_AND_ATTRIBUTES> = restricting
        .iter()
        .map(|s| SID_AND_ATTRIBUTES {
            Sid: s.0,
            Attributes: 0,
        })
        .collect();

    let mut out: HANDLE = null_mut();
    // SAFETY: arrays live through the call.
    if unsafe {
        CreateRestrictedToken(
            cur.as_raw_handle() as HANDLE,
            DISABLE_MAX_PRIVILEGE,
            disable.len() as u32,
            disable.as_ptr(),
            0,
            null(),
            restrict.len() as u32,
            restrict.as_ptr(),
            &mut out,
        )
    } == 0
    {
        return Err(last_err("CreateRestrictedToken"));
    }
    let restricted = own(out);

    let il = Sid::from_str(&format!("S-1-16-{FALLBACK_INTEGRITY_RID}"))?;
    let label = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES {
            Sid: il.0,
            Attributes: SE_GROUP_INTEGRITY,
        },
    };
    // SAFETY: label and the SID behind it live through the call.
    if unsafe {
        SetTokenInformation(
            restricted.as_raw_handle() as HANDLE,
            TokenIntegrityLevel,
            (&label as *const TOKEN_MANDATORY_LABEL).cast(),
            std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 + GetLengthSid(il.0),
        )
    } == 0
    {
        return Err(last_err("SetTokenInformation(IntegrityLevel)"));
    }
    Ok(restricted)
}

// ------------------------------------------------------------ job

fn make_job(profile: &Profile) -> io::Result<OwnedHandle> {
    // SAFETY: anonymous job.
    let h = unsafe { CreateJobObjectW(null(), null()) };
    if h.is_null() {
        return Err(last_err("CreateJobObjectW"));
    }
    let job = own(h);
    // SAFETY: zeroed struct is a valid starting point.
    let mut ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    ext.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    ext.BasicLimitInformation.ActiveProcessLimit = 1;
    if let Some(limit) = profile.memory_limit {
        ext.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        ext.ProcessMemoryLimit = limit as usize;
    }
    // SAFETY: struct matches the class.
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle() as HANDLE,
            JobObjectExtendedLimitInformation,
            (&ext as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(last_err("SetInformationJobObject(limits)"));
    }
    let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_EXITWINDOWS
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    };
    // SAFETY: struct matches the class.
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle() as HANDLE,
            JobObjectBasicUIRestrictions,
            (&ui as *const JOBOBJECT_BASIC_UI_RESTRICTIONS).cast(),
            std::mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
        )
    } == 0
    {
        return Err(last_err("SetInformationJobObject(ui)"));
    }
    Ok(job)
}

// ------------------------------------------------------------ ACL grants

static GRANTED: Mutex<Option<HashSet<(PathBuf, String)>>> = Mutex::new(None);

/// Give `sid` read+execute on `path` (inherited by everything below it).
/// Idempotent per process; the OS directories already grant `ALL APPLICATION
/// PACKAGES`, so this is needed only for the build/component directories.
fn grant_read(path: &Path, sid: &Sid) -> io::Result<()> {
    let key = (path.to_path_buf(), sid.to_string()?);
    {
        let mut g = GRANTED.lock().unwrap_or_else(|p| p.into_inner());
        if !g.get_or_insert_with(HashSet::new).insert(key) {
            return Ok(());
        }
    }
    let w = wide(path.as_os_str());
    let mut old_dacl: *mut ACL = null_mut();
    let mut sd: *mut c_void = null_mut();
    // SAFETY: valid path; out pointers.
    let rc = unsafe {
        GetNamedSecurityInfoW(
            w.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut old_dacl,
            null_mut(),
            &mut sd,
        )
    };
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    let ea = EXPLICIT_ACCESS_W {
        grfAccessPermissions: 0x0012_00A9, // FILE_GENERIC_READ | FILE_GENERIC_EXECUTE
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.0.cast(),
        },
    };
    let mut new_dacl: *mut ACL = null_mut();
    // SAFETY: one valid entry; old DACL valid until LocalFree(sd).
    let rc = unsafe { SetEntriesInAclW(1, &ea, old_dacl, &mut new_dacl) };
    let result = if rc != 0 {
        Err(io::Error::from_raw_os_error(rc as i32))
    } else {
        // SAFETY: valid path and DACL.
        let rc = unsafe {
            SetNamedSecurityInfoW(
                w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                new_dacl,
                null_mut(),
            )
        };
        if rc != 0 {
            Err(io::Error::from_raw_os_error(rc as i32))
        } else {
            Ok(())
        }
    };
    // SAFETY: both allocated by the APIs above.
    unsafe {
        if !new_dacl.is_null() {
            LocalFree(new_dacl.cast());
        }
        LocalFree(sd);
    }
    result.map_err(|e| io::Error::new(e.kind(), format!("grant read on {}: {e}", path.display())))
}

// ------------------------------------------------------------ spawn

pub struct SpawnRequest<'a> {
    pub exe: &'a Path,
    /// Arguments after argv[0].
    pub args: &'a [OsString],
    /// Added to (overriding) the parent's environment.
    pub env: &'a [(OsString, OsString)],
    /// Handles the child inherits (pipe ends). They must be inheritable.
    pub inherit: &'a [HANDLE],
    pub profile: &'a Profile,
}

pub struct RestrictedChild {
    process: OwnedHandle,
    pid: u32,
    _job: OwnedHandle,
    /// `"appcontainer"` or `"restricted-token"` (degraded).
    pub mechanism: &'static str,
}

impl RestrictedChild {
    pub fn id(&self) -> u32 {
        self.pid
    }

    /// A handle with `PROCESS_DUP_HANDLE` for sending handles to the child.
    pub fn duplicate_process_handle(&self) -> io::Result<OwnedHandle> {
        self.process.try_clone()
    }

    pub fn try_wait(&self) -> io::Result<Option<u32>> {
        let mut code = 0u32;
        // SAFETY: valid process handle.
        if unsafe { GetExitCodeProcess(self.process.as_raw_handle() as HANDLE, &mut code) } == 0 {
            return Err(last_err("GetExitCodeProcess"));
        }
        Ok((code != STILL_ACTIVE as u32).then_some(code))
    }

    pub fn wait(&self) -> io::Result<u32> {
        // SAFETY: valid process handle.
        unsafe { WaitForSingleObject(self.process.as_raw_handle() as HANDLE, INFINITE) };
        self.try_wait().map(|c| c.unwrap_or(0))
    }

    pub fn kill(&self) -> io::Result<()> {
        // SAFETY: valid process handle.
        if unsafe { TerminateProcess(self.process.as_raw_handle() as HANDLE, 1) } == 0 {
            return Err(last_err("TerminateProcess"));
        }
        Ok(())
    }
}

fn quote_arg(a: &OsStr, out: &mut Vec<u16>) {
    let s: Vec<u16> = a.encode_wide().collect();
    let needs = s.is_empty()
        || s.iter()
            .any(|&c| c == b' ' as u16 || c == b'\t' as u16 || c == b'"' as u16);
    if !needs {
        out.extend(s);
        return;
    }
    out.push(b'"' as u16);
    let mut backslashes = 0;
    for &c in &s {
        if c == b'\\' as u16 {
            backslashes += 1;
        } else {
            if c == b'"' as u16 {
                out.extend(std::iter::repeat_n(b'\\' as u16, backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    out.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
    out.push(b'"' as u16);
}

fn env_block(overrides: &[(OsString, OsString)]) -> Vec<u16> {
    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    for (k, v) in overrides {
        vars.retain(|(ek, _)| !ek.eq_ignore_ascii_case(k));
        vars.push((k.clone(), v.clone()));
    }
    vars.sort_by_key(|a| a.0.to_ascii_uppercase());
    let mut blk = Vec::new();
    for (k, v) in vars {
        blk.extend(k.encode_wide());
        blk.push(b'=' as u16);
        blk.extend(v.encode_wide());
        blk.push(0);
    }
    blk.push(0);
    blk
}

/// Start `req.exe` confined according to `req.profile`.
///
/// AppContainer first; if that is impossible and the profile does not
/// `require_enforced`, the restricted-token fallback (no network denial).
pub fn spawn_restricted(req: &SpawnRequest<'_>) -> io::Result<RestrictedChild> {
    match spawn_appcontainer(req) {
        Ok(c) => Ok(c),
        Err(e) if !req.profile.require_enforced => {
            eprintln!("papyrine-sandbox: AppContainer unavailable ({e}); using restricted token");
            spawn_with_token(req)
        }
        Err(e) => Err(e),
    }
}

fn spawn_appcontainer(req: &SpawnRequest<'_>) -> io::Result<RestrictedChild> {
    let ac = Sid::app_container()?;
    // The OS directories grant ALL APPLICATION PACKAGES; ours need an explicit grant.
    for d in &req.profile.read_dirs {
        grant_read(d, &ac)?;
    }
    for f in &req.profile.read_files {
        grant_read(f, &ac)?;
    }
    if let Some(d) = req.exe.parent() {
        grant_read(d, &ac)?;
    }
    let caps = SECURITY_CAPABILITIES {
        AppContainerSid: ac.0,
        Capabilities: null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    launch(req, None, Some(&caps), "appcontainer")
}

fn spawn_with_token(req: &SpawnRequest<'_>) -> io::Result<RestrictedChild> {
    let token = make_fallback_token()?;
    launch(
        req,
        Some(token.as_raw_handle() as HANDLE),
        None,
        "restricted-token",
    )
}

fn launch(
    req: &SpawnRequest<'_>,
    token: Option<HANDLE>,
    caps: Option<&SECURITY_CAPABILITIES>,
    mechanism: &'static str,
) -> io::Result<RestrictedChild> {
    let profile = req.profile;
    let job = make_job(profile)?;

    let mut cmd: Vec<u16> = Vec::new();
    quote_arg(req.exe.as_os_str(), &mut cmd);
    for a in req.args {
        cmd.push(b' ' as u16);
        quote_arg(a, &mut cmd);
    }
    cmd.push(0);
    let env = env_block(req.env);
    let exe = wide(req.exe.as_os_str());
    let cwd = wide(profile.temp_dir.as_os_str());

    // Explicit inheritance list (plus std handles so test output stays visible).
    // SAFETY: GetStdHandle has no preconditions.
    let (hout, herr) = unsafe {
        (
            GetStdHandle(STD_OUTPUT_HANDLE),
            GetStdHandle(STD_ERROR_HANDLE),
        )
    };
    let valid = |h: HANDLE| !h.is_null() && h != INVALID_HANDLE_VALUE;
    let mut list: Vec<HANDLE> = req.inherit.to_vec();
    for h in [hout, herr] {
        if valid(h) && !list.contains(&h) {
            // Make sure the std handle can be inherited at all.
            // SAFETY: valid handle.
            unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
            list.push(h);
        }
    }

    let count: u32 = 2 + u32::from(caps.is_some());
    let mut size = 0usize;
    // SAFETY: size query (expected to "fail" with insufficient buffer).
    unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut size) };
    let mut attr_buf = vec![0u8; size];
    let attrs = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    // SAFETY: buffer of the size requested above.
    if unsafe { InitializeProcThreadAttributeList(attrs, count, 0, &mut size) } == 0 {
        return Err(last_err("InitializeProcThreadAttributeList"));
    }
    let child_policy: u32 = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;
    let result = (|| {
        let set = |attr: usize, v: *const c_void, len: usize| -> io::Result<()> {
            // SAFETY: the value outlives the CreateProcess call.
            if unsafe { UpdateProcThreadAttribute(attrs, 0, attr, v, len, null_mut(), null()) } == 0
            {
                Err(last_err("UpdateProcThreadAttribute"))
            } else {
                Ok(())
            }
        };
        set(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            list.as_ptr().cast(),
            list.len() * std::mem::size_of::<HANDLE>(),
        )?;
        set(
            PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY,
            (&child_policy as *const u32).cast(),
            std::mem::size_of::<u32>(),
        )?;
        if let Some(c) = caps {
            set(
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
                (c as *const SECURITY_CAPABILITIES).cast(),
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
            )?;
        }

        // SAFETY: zeroed STARTUPINFOEXW with cb set is valid.
        let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.lpAttributeList = attrs;
        if valid(hout) || valid(herr) {
            si.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = null_mut();
            si.StartupInfo.hStdOutput = if valid(hout) { hout } else { null_mut() };
            si.StartupInfo.hStdError = if valid(herr) { herr } else { null_mut() };
        }
        // SAFETY: zeroed PROCESS_INFORMATION is an out parameter.
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // No console: a console-subsystem child would need conhost.exe, which the
        // one-process job limit forbids (it fails with 0xC0000142).
        let flags = EXTENDED_STARTUPINFO_PRESENT
            | CREATE_SUSPENDED
            | CREATE_UNICODE_ENVIRONMENT
            | DETACHED_PROCESS;
        // SAFETY: every pointer references live, NUL-terminated buffers.
        let ok = unsafe {
            match token {
                Some(t) => CreateProcessAsUserW(
                    t,
                    exe.as_ptr(),
                    cmd.as_mut_ptr(),
                    null(),
                    null(),
                    1,
                    flags,
                    env.as_ptr().cast(),
                    cwd.as_ptr(),
                    &si.StartupInfo,
                    &mut pi,
                ),
                None => CreateProcessW(
                    exe.as_ptr(),
                    cmd.as_mut_ptr(),
                    null(),
                    null(),
                    1,
                    flags,
                    env.as_ptr().cast(),
                    cwd.as_ptr(),
                    &si.StartupInfo,
                    &mut pi,
                ),
            }
        };
        if ok == 0 {
            return Err(last_err("CreateProcess"));
        }
        Ok(pi)
    })();
    // SAFETY: list was initialised above.
    unsafe { DeleteProcThreadAttributeList(attrs) };
    let pi = result?;
    let process = own(pi.hProcess);
    let thread = own(pi.hThread);

    // SAFETY: valid job and process handles.
    if unsafe {
        AssignProcessToJobObject(
            job.as_raw_handle() as HANDLE,
            process.as_raw_handle() as HANDLE,
        )
    } == 0
    {
        let e = last_err("AssignProcessToJobObject");
        // SAFETY: valid process handle.
        unsafe { TerminateProcess(process.as_raw_handle() as HANDLE, 1) };
        return Err(e);
    }
    // SAFETY: valid thread handle of the suspended child.
    if unsafe { ResumeThread(thread.as_raw_handle() as HANDLE) } == u32::MAX {
        return Err(last_err("ResumeThread"));
    }
    Ok(RestrictedChild {
        process,
        pid: pi.dwProcessId,
        _job: job,
        mechanism,
    })
}

// ------------------------------------------------------- child directory

/// Create a directory the confined child can write: the owner and SYSTEM keep
/// full control and the AppContainer gets full access (inherited). The Low
/// integrity label stops no-write-up from blocking the Low-integrity child.
pub fn create_child_dir(path: &Path) -> io::Result<()> {
    let ac = Sid::app_container()?.to_string()?;
    let sddl = wide(OsStr::new(&format!(
        "D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;OW)(A;OICI;FA;;;{ac})S:(ML;OICI;NW;;;LW)"
    )));
    let mut sd: *mut c_void = null_mut();
    // SAFETY: valid SDDL string and out pointer.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            null_mut(),
        )
    } == 0
    {
        return Err(last_err(
            "ConvertStringSecurityDescriptorToSecurityDescriptorW",
        ));
    }
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    let w = wide(path.as_os_str());
    // SAFETY: valid path and attributes.
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::CreateDirectoryW(w.as_ptr(), &sa) };
    let err = (ok == 0).then(io::Error::last_os_error);
    // SAFETY: allocated by the conversion API.
    unsafe { LocalFree(sd) };
    match err {
        None => Ok(()),
        Some(e) => Err(e),
    }
}

// ------------------------------------------------------- in-child checks

/// Verify the confinement the parent set up and add mitigation policies.
pub(crate) fn apply(p: &Profile) -> Result<Report> {
    let mut report = Report::default();

    let mut in_job = 0;
    // SAFETY: current process, null job = any job.
    unsafe { IsProcessInJob(GetCurrentProcess(), null_mut(), &mut in_job) };
    if in_job == 0 {
        return degrade(
            p,
            report,
            "not running inside the Job object (was spawn_restricted used?)",
        );
    }
    report.mechanisms.push("job-object".into());

    match is_app_container() {
        Ok(true) => report.mechanisms.push("appcontainer".into()),
        Ok(false) => report
            .degraded
            .push("not an AppContainer process: the network is not blocked".into()),
        Err(e) => return Err(Error::Io(e)),
    }
    match integrity_rid() {
        Ok(rid) if rid <= 0x1000 => report.mechanisms.push(format!("integrity-{rid:#x}")),
        Ok(rid) => {
            return degrade(p, report, &format!("integrity level {rid:#x} is above Low"));
        }
        Err(e) => return Err(Error::Io(e)),
    }

    // Mitigations that may only be requested by the process itself.
    match set_policy(
        ProcessChildProcessPolicy,
        &ChildProcessPolicy { flags: 1 }, // bit 0: NoChildProcessCreation
    ) {
        Ok(()) => report.mechanisms.push("no-child-process".into()),
        Err(e) => report.degraded.push(format!("child-process policy: {e}")),
    }
    if report.degraded.is_empty() {
        Ok(report)
    } else {
        let why = report.degraded.join("; ");
        degrade(p, report, &why)
    }
}

fn degrade(p: &Profile, mut report: Report, why: &str) -> Result<Report> {
    if p.require_enforced {
        Err(Error::NotEnforced(why.into()))
    } else {
        if !report.degraded.iter().any(|d| d == why) {
            report.degraded.push(why.into());
        }
        Ok(report)
    }
}

fn set_policy<T>(policy: PROCESS_MITIGATION_POLICY, v: &T) -> io::Result<()> {
    use windows_sys::Win32::System::Threading::SetProcessMitigationPolicy;
    // SAFETY: v points at the structure matching `policy`.
    if unsafe {
        SetProcessMitigationPolicy(policy, (v as *const T).cast(), std::mem::size_of::<T>())
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `GetCurrentProcessToken()`: a pseudo handle that needs no access check. A
/// confined token cannot `OpenProcessToken` its own process.
fn current_token() -> HANDLE {
    -4isize as HANDLE
}

fn is_app_container() -> io::Result<bool> {
    let buf = token_info(current_token(), TokenIsAppContainer)?;
    Ok(buf.len() >= 4 && u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) != 0)
}

fn integrity_rid() -> io::Result<u32> {
    let buf = token_info(current_token(), TokenIntegrityLevel)?;
    // SAFETY: buffer holds a TOKEN_MANDATORY_LABEL.
    let label = unsafe { &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL) };
    // SAFETY: valid SID; the last sub-authority is the RID.
    let n = unsafe { *GetSidSubAuthorityCount(label.Label.Sid) };
    // SAFETY: n-1 is a valid index.
    let rid = unsafe { *GetSidSubAuthority(label.Label.Sid, (n - 1) as u32) };
    Ok(rid)
}
