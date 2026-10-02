//! Linux: `PR_SET_NO_NEW_PRIVS`, Landlock filesystem allowlist, seccomp-bpf.
//!
//! Order matters: no_new_privs first (required for unprivileged seccomp and
//! Landlock), then Landlock (needs `landlock_*` syscalls the filter would not
//! block, but doing it first keeps the filter free of exceptions), then
//! seccomp, synchronised to every thread.
//!
//! The seccomp filter is a **denylist of dangerous syscall classes** (sockets,
//! exec, fork, ptrace, mount, namespaces, bpf, io_uring, signals to other
//! processes ...), returning `EPERM` so a refused call surfaces as an ordinary
//! error rather than a SIGSYS death. Narrowing it into a true allowlist is
//! tracked as a follow-up once the full syscall profile of PDFium and qpdf is
//! measured under the corpus.

use crate::{Error, Profile, Report, Result};
use landlock::{
    ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetStatus,
};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, apply_filter_all_threads,
};
use std::collections::BTreeMap;
use std::path::Path;

/// System locations every sandboxed process may read (dynamic loader, libc,
/// locale and font data, CPU topology for thread-pool sizing).
const SYSTEM_READ: &[&str] = &[
    "/usr",
    "/lib",
    "/lib64",
    "/etc/ld.so.cache",
    "/etc/ld.so.conf",
    "/etc/ld.so.conf.d",
    "/etc/localtime",
    "/etc/fonts",
    "/etc/nsswitch.conf",
    "/proc/self",
    "/proc/cpuinfo",
    "/proc/meminfo",
    "/proc/stat",
    "/proc/sys/kernel/osrelease",
    "/proc/sys/vm/overcommit_memory",
    "/sys/devices/system/cpu",
    "/sys/fs/cgroup",
    "/dev/null",
    "/dev/zero",
    "/dev/urandom",
    "/dev/random",
];

pub(crate) fn apply(p: &Profile) -> Result<Report> {
    let mut report = Report::default();

    // SAFETY: plain prctl with integer arguments.
    let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if rc != 0 {
        return Err(Error::Setup(format!(
            "PR_SET_NO_NEW_PRIVS: {}",
            std::io::Error::last_os_error()
        )));
    }
    report.mechanisms.push("no_new_privs".into());

    match landlock_fs(p) {
        Ok(RulesetStatus::FullyEnforced) => report.mechanisms.push("landlock".into()),
        Ok(RulesetStatus::PartiallyEnforced) => {
            // Older ABI than requested but the core read/write/exec rules hold.
            report.mechanisms.push("landlock(partial)".into());
        }
        Ok(RulesetStatus::NotEnforced) => {
            degrade(p, &mut report, "landlock is not enforced by this kernel")?
        }
        Err(e) => degrade(p, &mut report, &format!("landlock: {e}"))?,
    }

    for prog in seccomp_programs()? {
        apply_filter_all_threads(&prog).map_err(|e| Error::Setup(format!("seccomp: {e}")))?;
    }
    report.mechanisms.push("seccomp-bpf".into());
    Ok(report)
}

fn degrade(p: &Profile, report: &mut Report, why: &str) -> Result<()> {
    if p.require_enforced {
        Err(Error::NotEnforced(why.into()))
    } else {
        report.degraded.push(why.into());
        Ok(())
    }
}

fn landlock_fs(p: &Profile) -> std::result::Result<RulesetStatus, landlock::RulesetError> {
    // Best-effort ABI: the newest rights the kernel knows are used, older
    // kernels get the subset; the seccomp filter blocks sockets regardless.
    let abi = ABI::V5;
    let read = AccessFs::from_read(abi);
    let all = AccessFs::from_all(abi);

    let mut grants: Vec<(&Path, landlock::BitFlags<AccessFs>)> = Vec::new();
    for s in SYSTEM_READ {
        grants.push((Path::new(s), read));
    }
    grants.extend(p.read_dirs.iter().map(|d| (d.as_path(), read)));
    grants.extend(p.read_files.iter().map(|f| (f.as_path(), read)));
    grants.push((p.temp_dir.as_path(), all));

    let mut rs = Ruleset::default().handle_access(all)?.create()?;
    for (path, access) in grants {
        // Missing optional locations simply grant nothing.
        let Ok(fd) = PathFd::new(path) else { continue };
        // A file cannot carry directory-only rights.
        let access = if path.is_dir() {
            access
        } else {
            access & AccessFs::from_file(abi)
        };
        rs = rs.add_rule(PathBeneath::new(fd, access))?;
    }
    // /dev/null is opened for writing by many C libraries.
    if let Ok(fd) = PathFd::new("/dev/null") {
        rs = rs.add_rule(PathBeneath::new(
            fd,
            AccessFs::ReadFile | AccessFs::WriteFile,
        ))?;
    }
    Ok(rs.restrict_self()?.ruleset)
}

fn seccomp_programs() -> Result<Vec<BpfProgram>> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    let mut deny = |nr: libc::c_long| {
        rules.insert(nr, vec![]); // empty rule list = unconditional match
    };

    // Network.
    for nr in [
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
    ] {
        deny(nr);
    }
    // Process creation and replacement (clone is filtered below, by flags).
    deny(libc::SYS_execve);
    deny(libc::SYS_execveat);
    #[cfg(target_arch = "x86_64")]
    {
        deny(libc::SYS_fork);
        deny(libc::SYS_vfork);
    }
    // Attacks on other processes.
    for nr in [
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_kcmp,
        libc::SYS_kill,
        libc::SYS_rt_sigqueueinfo,
        libc::SYS_pidfd_open,
        libc::SYS_pidfd_getfd,
        libc::SYS_pidfd_send_signal,
    ] {
        deny(nr);
    }
    // Kernel attack surface a renderer has no business with.
    for nr in [
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_setns,
        libc::SYS_unshare,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_userfaultfd,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_open_by_handle_at,
        libc::SYS_name_to_handle_at,
        libc::SYS_reboot,
        libc::SYS_kexec_load,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_swapon,
        libc::SYS_swapoff,
        libc::SYS_mknodat,
        libc::SYS_fanotify_init,
        libc::SYS_inotify_init1, // the sandbox has nothing to watch
    ] {
        deny(nr);
    }
    // clone: threads (CLONE_THREAD) are fine, new processes are not.
    let no_thread = SeccompCondition::new(
        0,
        SeccompCmpArgLen::Qword,
        SeccompCmpOp::MaskedEq(libc::CLONE_THREAD as u64),
        0,
    )
    .map_err(|e| Error::Setup(format!("seccomp rule: {e}")))?;
    rules.insert(
        libc::SYS_clone,
        vec![
            SeccompRule::new(vec![no_thread])
                .map_err(|e| Error::Setup(format!("seccomp rule: {e}")))?,
        ],
    );

    let arch: seccompiler::TargetArch = std::env::consts::ARCH
        .try_into()
        .map_err(|e| Error::Setup(format!("seccomp arch: {e}")))?;
    let compile = |rules, mismatch, matched| -> Result<BpfProgram> {
        SeccompFilter::new(rules, mismatch, matched, arch)
            .map_err(|e| Error::Setup(format!("seccomp filter: {e}")))?
            .try_into()
            .map_err(|e: seccompiler::BackendError| Error::Setup(format!("seccomp compile: {e}")))
    };
    // Denied calls fail with EPERM so they surface as ordinary errors.
    let main = compile(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
    )?;
    // clone3 cannot be inspected (its flags live in a struct), so it fails with
    // ENOSYS and glibc/musl fall back to clone, which is filtered above.
    let mut c3: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    c3.insert(libc::SYS_clone3, vec![]);
    let clone3 = compile(
        c3,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::ENOSYS as u32),
    )?;
    Ok(vec![main, clone3])
}
