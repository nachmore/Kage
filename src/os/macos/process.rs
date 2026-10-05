// macOS process management

use anyhow::Result;
use log::info;
use nix::errno::Errno;
use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use signal_hook::consts::signal::*;
use signal_hook::iterator::Signals;
use std::process::Command;

// Moved to kage-core; re-exported for `super::process::*` callers.
pub use kage_core::os::macos::process::{get_process_name_impl, spawn_detached_impl};

/// SIGTERM, wait up to 500ms for exit, then SIGKILL. Returns true once the
/// target is gone — including the common case where it exits cleanly on
/// SIGTERM (the SIGKILL then gets ESRCH, which used to be reported as a
/// failure).
///
/// Agents are spawned as session/group leaders (`setsid` in
/// `configure_spawn_impl`), so when `pid` leads its own group the whole
/// group is signalled and the agent's MCP children go with it. Callers
/// must have already confirmed `pid` is ours (PID-reuse check).
pub fn kill_process_impl(pid: u32) -> bool {
    // 0 / negative would address groups or every process — never valid here.
    let raw = match i32::try_from(pid) {
        Ok(r) if r > 0 => r,
        _ => return false,
    };
    let target = Pid::from_raw(raw);
    // SAFETY: getpgid only reads process-table state; -1 on error (no such
    // process) simply means "not a group leader".
    let leads_group = unsafe { libc::getpgid(raw) } == raw;
    let send = |sig: Option<Signal>| {
        if leads_group {
            killpg(target, sig)
        } else {
            kill(target, sig)
        }
    };

    if send(Some(Signal::SIGTERM)).is_err() {
        return false;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
        // Signal 0 = existence probe. (An unreaped child of ours still
        // probes as alive, so that case falls through to SIGKILL.)
        if send(None) == Err(Errno::ESRCH) {
            return true;
        }
    }
    matches!(send(Some(Signal::SIGKILL)), Ok(()) | Err(Errno::ESRCH))
}

pub fn configure_spawn_impl(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            // Create new process group
            libc::setsid();
            Ok(())
        });
    }
    info!("macOS: Setting up process detachment");
}

/// No-op on macOS — there's no Windows-style Job Object that auto-kills
/// children on parent exit. macOS handles orphan reaping via launchd
/// (and we set process groups via setsid in `configure_spawn_impl`).
/// Kept as a function rather than an `#[cfg]` at the call site so the
/// cross-platform `os::process::install_kill_on_exit_job` is a clean
/// one-liner.
pub fn install_kill_on_exit_job_impl() {}

/// No-op companion to `install_kill_on_exit_job_impl` — see Windows
/// impl for what this does there.
pub fn release_kill_on_exit_job_impl() {}

/// macOS uses WKWebView via Tauri; there's no user-data-dir lock
/// contention pattern that requires foreign process cleanup. No-op.
pub fn cleanup_stale_processes_impl(_marker_dir: &std::path::Path) -> usize {
    0
}

pub fn install_signal_handlers_impl<F>(cleanup_fn: F) -> Result<()>
where
    F: Fn() + Send + 'static,
{
    std::thread::spawn(move || {
        let mut signals =
            Signals::new([SIGTERM, SIGINT, SIGQUIT]).expect("Failed to register signal handlers");

        // Handle first signal then exit.
        if let Some(sig) = signals.forever().next() {
            info!("Received signal: {:?}", sig);
            cleanup_fn();
            std::process::exit(0);
        }
    });

    info!("✅ Signal handlers installed (SIGTERM, SIGINT, SIGQUIT)");
    Ok(())
}
