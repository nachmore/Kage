// Windows process management

use anyhow::Result;
use log::{info, warn};
use std::process::Command;
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Check if a PID belongs to a kage-related process (kage-cli, node, etc.)
/// Returns the process name if found, None if the process doesn't exist or isn't ours.
pub fn get_process_name_impl(pid: u32) -> Option<String> {
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::Foundation::CloseHandle;
    use windows::core::PWSTR;

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 260];
        let mut size = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            windows::Win32::System::Threading::PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        if result.is_ok() && size > 0 {
            let path = String::from_utf16_lossy(&buf[..size as usize]);
            // Extract just the filename from the full path
            let name = path.rsplit('\\').next().unwrap_or(&path).to_lowercase();
            Some(name)
        } else {
            None
        }
    }
}

pub fn kill_process_impl(pid: u32) -> bool {
    match Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                info!("taskkill PID {} failed: {}", pid, stderr.trim());
            }
            output.status.success()
        }
        Err(e) => {
            warn!("Failed to run taskkill for PID {}: {}", pid, e);
            false
        }
    }
}

pub fn configure_spawn_impl(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
    info!("Windows: Setting CREATE_NO_WINDOW flag");
}

const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;

/// Spawn a process that is detached from our Job Object so it survives
/// when Kage exits. Use this for user-facing launches (apps,
/// URLs, explorer, system commands) — NOT for internal child processes
/// like kage-cli or TTS servers that should die with us.
pub fn spawn_detached_impl(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    cmd.creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW);
    cmd.spawn()
}

pub fn install_signal_handlers_impl<F>(cleanup_fn: F) -> Result<()>
where
    F: Fn() + Send + 'static,
{
    ctrlc::set_handler(move || {
        info!("Received Ctrl+C signal");
        cleanup_fn();
        std::process::exit(0);
    })?;
    
    info!("✅ Ctrl+C handler installed");
    Ok(())
}

/// Set the current thread's description (name) via Win32 SetThreadDescription.
/// This makes the thread identifiable in debuggers and in our thread dump command.
pub fn set_thread_name(name: &str) {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadDescription};
    use windows::core::PCWSTR;

    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SetThreadDescription(GetCurrentThread(), PCWSTR(wide.as_ptr()));
    }
}


/// Create a Windows Job Object with KILL_ON_JOB_CLOSE and assign this
/// process to it. Any child process we spawn (TTS server, ACP CLI,
/// etc.) is implicitly added to the same job and gets killed when
/// this process exits — even on crash, force-kill from Task Manager,
/// or debugger detach. Without this, children can outlive their
/// parent and show up as orphaned processes.
///
/// The returned handle must stay alive for the lifetime of the
/// process. HANDLE is Copy with no Drop impl so it doesn't auto-close
/// when the local goes out of scope — which is what we want. The OS
/// holds the Job Object open (and its kill-on-close policy active)
/// as long as at least one handle remains unclosed.
///
/// Errors are logged but never propagated: if we can't create the
/// job, the app still runs; we just lose the orphan-cleanup guarantee.
pub fn install_kill_on_exit_job() {
    use windows::Win32::System::JobObjects::*;
    use windows::Win32::System::Threading::GetCurrentProcess;
    use windows::core::PCWSTR;

    unsafe {
        let job = match CreateJobObjectW(None, PCWSTR::null()) {
            Ok(j) => j,
            Err(e) => {
                warn!("Failed to create Job Object: {}", e);
                return;
            }
        };

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;

        let set_ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set_ok.is_err() {
            warn!("Failed to configure Job Object");
            return;
        }

        let current = GetCurrentProcess();
        match AssignProcessToJobObject(job, current) {
            Ok(_) => info!("✅ Job Object created — child processes will be killed on exit"),
            Err(e) => warn!("Failed to assign process to Job Object: {}", e),
        }
        // Deliberately leak the handle — see doc comment.
        let _ = job;
    }
}
