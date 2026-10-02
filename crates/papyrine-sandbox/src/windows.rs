//! Windows: restricted primary token + low integrity level + Job object.
//!
//! Windows restrictions are fixed when the process is created, so the *parent*
//! must use [`spawn_restricted`]; [`apply`] (called in the child through
//! `apply_child_sandbox`) verifies that the child really is confined and adds
//! process mitigation policies that can only be set from inside.
//!
//! What the confinement gives (see the integration tests):
//! * restricted token: all privileges dropped, every group but a short list
//!   deny-only, restricting SIDs `Everyone`/`Users`/`Authenticated Users`/
//!   `RESTRICTED`, so objects whose ACL names only the user (the profile, `~`)
//!   are unreadable even though the user SID is still in the token;
//! * low integrity: no writes to anything labelled medium or higher;
//! * Job object: one process at most (no children), memory cap, UI limits,
//!   kill-on-close (the host dying takes the child with it).
//!
//! Network access: the Untrusted integrity level is what refuses `socket()`
//! (no access to the AFD device); the integration test pins that behaviour.

use crate::{Error, Profile, Report, Result};
use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::*;

const SE_GROUP_LOGON_ID: u32 = 0xC000_0000;
const SE_GROUP_INTEGRITY: u32 = 0x20;

/// `PROCESS_MITIGATION_CHILD_PROCESS_POLICY` (not exposed by windows-sys).
#[repr(C)]
struct ChildProcessPolicy {
    flags: u32,
}

/// The integrity level children run at. `Untrusted` (RID 0) cannot open the
/// AFD endpoint, which is what refuses `socket()`; `Low` is 0x1000.
const CHILD_INTEGRITY_RID: u32 = 0;

/// Experiment switches while the Windows profile is being tuned on CI
/// (`PAPYRINE_WIN_VARIANT=lowil,nouilimit,norestrict,nodisable`). Removed once
/// the profile is settled.
fn variant(name: &str) -> bool {
    std::env::var("PAPYRINE_WIN_VARIANT").is_ok_and(|v| v.split(',').any(|x| x == name))
}

fn integrity_rid_for_children() -> u32 {
    if variant("lowil") {
        0x1000
    } else {
        CHILD_INTEGRITY_RID
    }
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

struct Sid(*mut c_void);
impl Sid {
    fn from_str(s: &str) -> io::Result<Sid> {
        let w = wide(OsStr::new(s));
        let mut p: *mut c_void = null_mut();
        // SAFETY: valid NUL-terminated wide string and out pointer.
        if unsafe { ConvertStringSidToSidW(w.as_ptr(), &mut p) } == 0 {
            return Err(last_err("ConvertStringSidToSidW"));
        }
        Ok(Sid(p))
    }
}
impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSidToSidW (LocalAlloc).
        unsafe { LocalFree(self.0) };
    }
}

// ------------------------------------------------------------ token

/// Groups kept enabled in the child token (everything else becomes deny-only).
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

fn make_child_token() -> io::Result<OwnedHandle> {
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

    // Integrity level.
    let il = Sid::from_str(&format!("S-1-16-{}", integrity_rid_for_children()))?;
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
    if variant("noproclimit") {
        ext.BasicLimitInformation.LimitFlags &= !JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    }
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
    if variant("nouilimit") {
        return Ok(job);
    }
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

pub fn spawn_restricted(req: &SpawnRequest<'_>) -> io::Result<RestrictedChild> {
    let profile = req.profile;
    let token = make_child_token()?;
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
    let (hin, hout, herr) = unsafe {
        (
            GetStdHandle(STD_INPUT_HANDLE),
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
    let _ = hin;

    let mut size = 0usize;
    // SAFETY: size query (expected to "fail" with insufficient buffer).
    unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size) };
    let mut attr_buf = vec![0u8; size];
    let attrs = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    // SAFETY: buffer of the size requested above.
    if unsafe { InitializeProcThreadAttributeList(attrs, 1, 0, &mut size) } == 0 {
        return Err(last_err("InitializeProcThreadAttributeList"));
    }
    // SAFETY: `list` outlives the CreateProcess call.
    if unsafe {
        UpdateProcThreadAttribute(
            attrs,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            list.as_ptr() as *const c_void,
            list.len() * std::mem::size_of::<HANDLE>(),
            null_mut(),
            null(),
        )
    } == 0
    {
        return Err(last_err("UpdateProcThreadAttribute"));
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
    // SAFETY: every pointer references live, NUL-terminated buffers.
    let ok = unsafe {
        CreateProcessAsUserW(
            token.as_raw_handle() as HANDLE,
            exe.as_ptr(),
            cmd.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                // No console: a console subsystem child would need conhost.exe, which the
                // one-process job limit forbids (it fails with 0xC0000142).
                | DETACHED_PROCESS,
            env.as_ptr().cast(),
            cwd.as_ptr(),
            &si.StartupInfo,
            &mut pi,
        )
    };
    let err = (ok == 0).then(|| last_err("CreateProcessAsUserW"));
    // SAFETY: list was initialised above.
    unsafe { DeleteProcThreadAttributeList(attrs) };
    if let Some(e) = err {
        return Err(e);
    }
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
    })
}

// ------------------------------------------------------- child directory

/// Create a directory the confined child can write: the owner and SYSTEM keep
/// full control, `Everyone` (which the restricted token's restricting SIDs
/// include) gets full access, and the integrity label is Untrusted so
/// no-write-up does not block the child.
pub fn create_child_dir(path: &Path) -> io::Result<()> {
    let sddl = wide(OsStr::new(&format!(
        "D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;OW)(A;OICI;FA;;;WD)S:(ML;OICI;NW;;;S-1-16-{})",
        integrity_rid_for_children()
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

    match integrity_rid() {
        Ok(rid) if rid <= 0x1000 => report.mechanisms.push(format!("integrity-{rid:#x}")),
        Ok(rid) => return degrade(p, report, &format!("integrity level {rid:#x} is above Low")),
        Err(e) => return Err(Error::Io(e)),
    }

    // Mitigations that may only be requested by the process itself.
    set_policy(
        ProcessChildProcessPolicy,
        &ChildProcessPolicy { flags: 1 }, // bit 0: NoChildProcessCreation
    )
    .map(|_| report.mechanisms.push("no-child-process".into()))
    .unwrap_or_else(|e| report.degraded.push(format!("child-process policy: {e}")));
    if report.degraded.is_empty() {
        Ok(report)
    } else {
        degrade(p, report.clone(), &report.degraded.join("; "))
    }
}

fn degrade(p: &Profile, mut report: Report, why: &str) -> Result<Report> {
    if p.require_enforced {
        Err(Error::NotEnforced(why.into()))
    } else {
        report.degraded.push(why.into());
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

fn integrity_rid() -> io::Result<u32> {
    let mut tok: HANDLE = null_mut();
    // SAFETY: current process; out pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) } == 0 {
        return Err(last_err("OpenProcessToken"));
    }
    let tok = own(tok);
    let buf = token_info(tok.as_raw_handle() as HANDLE, TokenIntegrityLevel)?;
    // SAFETY: buffer holds a TOKEN_MANDATORY_LABEL.
    let label = unsafe { &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL) };
    // SAFETY: valid SID; the last sub-authority is the RID.
    let n = unsafe { *GetSidSubAuthorityCount(label.Label.Sid) };
    // SAFETY: n-1 is a valid index.
    let rid = unsafe { *GetSidSubAuthority(label.Label.Sid, (n - 1) as u32) };
    Ok(rid)
}
