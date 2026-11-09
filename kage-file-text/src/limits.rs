//! OS limits the worker puts on itself before it reads its input
//! (EXTENSION_FILE_TEXT §5.5).
//!
//! - Unix: `RLIMIT_CPU` (30 s), and `RLIMIT_CORE` 0 so a crash never
//!   writes the document's bytes into a core file.
//! - Linux also: `RLIMIT_AS` (2 GiB), `PR_SET_NO_NEW_PRIVS`, and
//!   `PR_SET_PDEATHSIG(SIGKILL)`. The death signal fires when the thread
//!   that spawned the worker exits, so the app must spawn and wait on the
//!   same thread (it does: one blocking thread per request).
//! - Windows: the worker's own Job Object with a 1 GiB process memory
//!   limit, an active-process limit of 1 (it can't start programs), and
//!   death on an unhandled exception. The error mode turns off the crash
//!   dialog. The worker is also in the app's kill-on-exit job, so it dies
//!   with the app.
//! - macOS doesn't enforce `RLIMIT_AS`; the allocator cap is the memory
//!   limit there.
//!
//! Every step is best effort: a step that fails is reported (English, on
//! stderr) and the worker goes on. The app's 30-second deadline and the
//! allocator cap still apply.

use kage_core::file_text_wire::WORKER_CPU_LIMIT;

/// Puts the limits in place. Returns a line for each step that failed.
pub fn apply() -> Vec<String> {
    imp::apply()
}

/// What is in effect, as JSON, for the debug-only `--v1-test-limits`
/// mode and its test.
pub fn report() -> serde_json::Value {
    imp::report()
}

/// The CPU limit in whole seconds: the soft limit. The hard limit is one
/// second more, so the kernel sends `SIGXCPU` first and `SIGKILL` after.
pub const CPU_SOFT_SECONDS: u64 = WORKER_CPU_LIMIT.as_secs();

#[cfg(unix)]
mod imp {
    use super::CPU_SOFT_SECONDS;
    #[cfg(target_os = "linux")]
    use kage_core::file_text_wire::WORKER_ADDRESS_SPACE_BYTES;
    use serde_json::{json, Value};

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    type Resource = libc::__rlimit_resource_t;
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    type Resource = libc::c_int;

    // `rlim_t` is `u64` on the targets Kage ships, but not on every Unix.
    #[allow(clippy::unnecessary_cast)]
    fn get(resource: Resource) -> Option<(u64, u64)> {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `lim` is a valid, writable rlimit.
        if unsafe { libc::getrlimit(resource, &mut lim) } != 0 {
            return None;
        }
        Some((lim.rlim_cur as u64, lim.rlim_max as u64))
    }

    /// Lowers a limit to at most `soft` / `hard`. Never raises one.
    fn lower(resource: Resource, soft: u64, hard: u64, name: &str) -> Result<(), String> {
        let (_, current_hard) = get(resource).ok_or_else(|| format!("getrlimit({name}) failed"))?;
        // RLIM_INFINITY is the largest value, so `min` keeps it right.
        let new_hard = hard.min(current_hard);
        let new_soft = soft.min(new_hard);
        let lim = libc::rlimit {
            rlim_cur: new_soft as libc::rlim_t,
            rlim_max: new_hard as libc::rlim_t,
        };
        // SAFETY: `lim` is a valid rlimit.
        if unsafe { libc::setrlimit(resource, &lim) } != 0 {
            return Err(format!(
                "setrlimit({name}) failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    pub fn apply() -> Vec<String> {
        let mut problems = Vec::new();
        let mut note = |r: Result<(), String>| {
            if let Err(e) = r {
                problems.push(e);
            }
        };
        note(lower(
            libc::RLIMIT_CPU,
            CPU_SOFT_SECONDS,
            CPU_SOFT_SECONDS + 1,
            "RLIMIT_CPU",
        ));
        note(lower(libc::RLIMIT_CORE, 0, 0, "RLIMIT_CORE"));
        #[cfg(target_os = "linux")]
        {
            note(lower(
                libc::RLIMIT_AS,
                WORKER_ADDRESS_SPACE_BYTES,
                WORKER_ADDRESS_SPACE_BYTES,
                "RLIMIT_AS",
            ));
            note(linux::no_new_privs());
            note(linux::die_with_parent());
        }
        problems
    }

    pub fn report() -> Value {
        let pair = |r: Option<(u64, u64)>| match r {
            Some((soft, hard)) => json!([soft, hard]),
            None => Value::Null,
        };
        #[allow(unused_mut)]
        let mut out = json!({
            "cpu": pair(get(libc::RLIMIT_CPU)),
            "core": pair(get(libc::RLIMIT_CORE)),
        });
        #[cfg(target_os = "linux")]
        {
            out["address_space"] = pair(get(libc::RLIMIT_AS));
            out["no_new_privs"] = json!(linux::get(libc::PR_GET_NO_NEW_PRIVS));
            out["death_signal"] = json!(linux::death_signal());
        }
        out
    }

    #[cfg(target_os = "linux")]
    mod linux {
        pub fn get(option: libc::c_int) -> i64 {
            // SAFETY: the PR_GET_* options read and return an int.
            unsafe { libc::prctl(option, 0, 0, 0, 0) as i64 }
        }

        pub fn death_signal() -> i64 {
            let mut sig: libc::c_int = 0;
            // SAFETY: PR_GET_PDEATHSIG writes an int through the pointer.
            let rc = unsafe {
                libc::prctl(
                    libc::PR_GET_PDEATHSIG,
                    &mut sig as *mut libc::c_int as libc::c_ulong,
                    0,
                    0,
                    0,
                )
            };
            if rc == 0 {
                sig as i64
            } else {
                -1
            }
        }

        pub fn no_new_privs() -> Result<(), String> {
            // SAFETY: plain prctl call with integer arguments.
            let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
            if rc != 0 {
                return Err(format!(
                    "PR_SET_NO_NEW_PRIVS failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        }

        pub fn die_with_parent() -> Result<(), String> {
            // SAFETY: getppid can't fail.
            let parent = unsafe { libc::getppid() };
            // SAFETY: plain prctl call with integer arguments.
            let rc = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) };
            if rc != 0 {
                return Err(format!(
                    "PR_SET_PDEATHSIG failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // The parent may have died before the signal was armed. Then
            // nobody reads the answer: stop now.
            // SAFETY: getppid can't fail.
            if unsafe { libc::getppid() } != parent {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
mod imp {
    use kage_core::file_text_wire::WORKER_JOB_MEMORY_BYTES;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows::core::{BOOL, PCWSTR};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Diagnostics::Debug::{
        SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    /// The job's handle, kept open for the life of the process (as a
    /// number: `HANDLE` isn't `Sync`).
    static JOB: AtomicUsize = AtomicUsize::new(0);

    const INFO_SIZE: u32 = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;

    pub fn apply() -> Vec<String> {
        let mut problems = Vec::new();
        // SAFETY: SetErrorMode only changes this process's error mode.
        unsafe {
            SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
        }
        if let Err(e) = own_job() {
            problems.push(e);
        }
        problems
    }

    fn own_job() -> Result<(), String> {
        // SAFETY: no security attributes, no name.
        let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|e| format!("CreateJobObjectW failed: {e}"))?;
        JOB.store(job.0 as usize, Ordering::Relaxed);
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        info.BasicLimitInformation.ActiveProcessLimit = 1;
        info.ProcessMemoryLimit = WORKER_JOB_MEMORY_BYTES as usize;
        // SAFETY: `info` is the struct this information class expects,
        // and the size matches it.
        unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                INFO_SIZE,
            )
        }
        .map_err(|e| format!("SetInformationJobObject failed: {e}"))?;
        // SAFETY: both handles are valid; the pseudo handle needs no close.
        unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) }
            .map_err(|e| format!("AssignProcessToJobObject failed: {e}"))?;
        Ok(())
    }

    pub fn report() -> Value {
        let raw = JOB.load(Ordering::Relaxed);
        if raw == 0 {
            return json!({ "job": Value::Null });
        }
        let job = HANDLE(raw as *mut core::ffi::c_void);
        let mut in_job = BOOL(0);
        // SAFETY: valid handles and an out pointer to a BOOL.
        let in_job = unsafe { IsProcessInJob(GetCurrentProcess(), Some(job), &mut in_job) }
            .map(|_| in_job.as_bool())
            .unwrap_or(false);
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // SAFETY: `info` is the struct this information class fills.
        let queried = unsafe {
            QueryInformationJobObject(
                Some(job),
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut core::ffi::c_void,
                INFO_SIZE,
                None,
            )
        }
        .is_ok();
        if !queried {
            return json!({ "job": { "in_job": in_job } });
        }
        let flags = info.BasicLimitInformation.LimitFlags;
        json!({
            "job": {
                "in_job": in_job,
                "process_memory": info.ProcessMemoryLimit as u64,
                "active_processes": info.BasicLimitInformation.ActiveProcessLimit,
                "die_on_unhandled_exception":
                    (flags & JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION).0 != 0,
            }
        })
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub fn apply() -> Vec<String> {
        vec!["no OS limits on this platform".to_string()]
    }

    pub fn report() -> serde_json::Value {
        serde_json::Value::Null
    }
}
