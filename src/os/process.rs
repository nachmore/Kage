// Cross-platform process management utilities

use anyhow::Result;
use std::process::Command;

/// Get the process name for a PID, if it exists.
pub fn get_process_name(pid: u32) -> Option<String> {
    crate::os::platform::process::get_process_name_impl(pid)
}

/// Kill a process by PID
pub fn kill_process(pid: u32) -> bool {
    crate::os::platform::process::kill_process_impl(pid)
}

/// Configure a Command for platform-specific process spawning
/// This sets flags like hiding console windows on Windows or detaching on Unix
pub fn configure_process_spawn(cmd: &mut Command) {
    crate::os::platform::process::configure_spawn_impl(cmd);
}

/// Install signal handlers for graceful shutdown
pub fn install_signal_handlers<F>(cleanup_fn: F) -> Result<()>
where
    F: Fn() + Send + 'static,
{
    crate::os::platform::process::install_signal_handlers_impl(cleanup_fn)
}

/// Spawn a process detached from Kage's Job Object (Windows) so it
/// survives when Kage exits. On other platforms, this is a plain spawn.
/// Use for user-facing launches (apps, URLs, explorer, system commands).
pub fn spawn_detached(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    crate::os::platform::process::spawn_detached_impl(cmd)
}

/// On Windows: create a Job Object that kills all child processes when
/// this process exits, even on crash. Prevents orphaned TTS servers,
/// ACP CLI processes, MCP children, etc. No-op on macOS/Linux which
/// rely on init/launchd reaping for orphan cleanup.
pub fn install_kill_on_exit_job() {
    crate::os::platform::process::install_kill_on_exit_job_impl()
}

/// On Windows: disable the Job Object's kill-on-close behaviour so
/// processes spawned in the moments before our exit (notably the
/// updater installer) survive our shutdown. No-op on macOS/Linux
/// where there's no equivalent OS-level kill mechanism. One-shot —
/// not restored after the call, since the only caller is on the
/// imminent-exit path.
pub fn release_kill_on_exit_job() {
    crate::os::platform::process::release_kill_on_exit_job_impl()
}

/// Best-effort: clean up stale processes that could prevent the next
/// launch from succeeding. Returns the number killed.
///
/// Currently implemented for Windows only, where it kills orphan
/// `msedgewebview2.exe` children whose command line points at the
/// given WebView2 user-data folder. WebView2 enforces single-writer
/// semantics on that folder; an orphan child holding the lock makes
/// the next launch fail to render a webview at all.
///
/// `marker_dir` is passed through to platform code so per-platform
/// implementations can match against the relevant resource (the
/// WebView2 user-data folder on Windows). It's accepted unconditionally
/// at this layer to keep the cross-platform call site clean even
/// though macOS / Linux ignore it today.
///
/// No-op on macOS and Linux — neither WKWebView nor WebKitGTK has the
/// same user-data-dir lock contention pattern. If a future platform
/// needs equivalent cleanup (e.g. a stuck IPC socket), add the impl
/// behind this same name.
pub fn cleanup_stale_processes(marker_dir: &std::path::Path) -> usize {
    crate::os::platform::process::cleanup_stale_processes_impl(marker_dir)
}
