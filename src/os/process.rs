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

/// On Windows: enumerate `msedgewebview2.exe` processes whose command
/// line points at the given WebView2 user data folder, and kill them.
/// Returns the number killed.
///
/// Used by the startup path to clean up orphan WebView2 children left
/// behind by a previous kage instance that didn't shut down cleanly.
/// WebView2 enforces single-writer semantics on its user data directory;
/// an orphan child holding the lock makes the next launch fail with a
/// "frontend never became ready" timeout.
///
/// No-op on macOS and Linux — neither WKWebView nor WebKitGTK has the
/// same user-data-dir lock contention pattern.
pub fn kill_orphan_kage_webview_processes(user_data_dir: &std::path::Path) -> usize {
    crate::os::platform::process::kill_orphan_kage_webview_processes_impl(user_data_dir)
}
