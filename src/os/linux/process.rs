// Linux process management

use anyhow::Result;
use log::info;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use signal_hook::consts::signal::*;
use signal_hook::iterator::Signals;
use std::process::Command;

// Moved to kage-core; re-exported for `super::process::*` callers.
pub use kage_core::os::linux::process::{get_process_name_impl, spawn_detached_impl};

pub fn kill_process_impl(pid: u32) -> bool {
    // Try SIGTERM first
    if kill(Pid::from_raw(pid as i32), Signal::SIGTERM).is_ok() {
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Force kill if still alive
        if kill(Pid::from_raw(pid as i32), Signal::SIGKILL).is_ok() {
            return true;
        }
    }
    false
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
    info!("Linux: Setting up process detachment");
}

/// No-op on Linux — there's no Windows-style Job Object that auto-kills
/// children on parent exit. Orphans become children of init / systemd
/// and either get reaped or run until they exit on their own. We set
/// process groups via setsid in `configure_spawn_impl`. Kept as a
/// function rather than an `#[cfg]` at the call site so the
/// cross-platform `os::process::install_kill_on_exit_job` is a clean
/// one-liner.
pub fn install_kill_on_exit_job_impl() {}

/// No-op companion to `install_kill_on_exit_job_impl` — see Windows
/// impl for what this does there.
pub fn release_kill_on_exit_job_impl() {}

/// Linux uses WebKitGTK via Tauri; there's no user-data-dir lock
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
            Signals::new(&[SIGTERM, SIGINT, SIGQUIT]).expect("Failed to register signal handlers");

        for sig in signals.forever() {
            info!("Received signal: {:?}", sig);
            cleanup_fn();
            std::process::exit(0);
        }
    });

    info!("✅ Signal handlers installed (SIGTERM, SIGINT, SIGQUIT)");
    Ok(())
}
