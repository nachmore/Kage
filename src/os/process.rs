// Cross-platform process management utilities

use anyhow::Result;
use std::process::Command;

/// Kill a process by PID
pub fn kill_process(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::process::kill_process_impl(pid)
    }
    
    #[cfg(not(target_os = "windows"))]
    {
        crate::os::platform::process::kill_process_impl(pid)
    }
}

/// Configure a Command for platform-specific process spawning
/// This sets flags like hiding console windows on Windows or detaching on Unix
pub fn configure_process_spawn(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::process::configure_spawn_impl(cmd);
    }
    
    #[cfg(unix)]
    {
        crate::os::platform::process::configure_spawn_impl(cmd);
    }
}

/// Install signal handlers for graceful shutdown
pub fn install_signal_handlers<F>(cleanup_fn: F) -> Result<()>
where
    F: Fn() + Send + 'static,
{
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::process::install_signal_handlers_impl(cleanup_fn)
    }
    
    #[cfg(not(target_os = "windows"))]
    {
        crate::os::platform::process::install_signal_handlers_impl(cleanup_fn)
    }
}
