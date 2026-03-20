// Cross-platform process management utilities

use anyhow::Result;
use std::process::Command;

/// Get the process name for a PID, if it exists.
pub fn get_process_name(pid: u32) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::process::get_process_name_impl(pid)
    }

    #[cfg(not(target_os = "windows"))]
    {
        // On Unix, read /proc/{pid}/comm or use ps
        let output = Command::new("ps")
            .args(&["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .ok()?;
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if name.is_empty() { None } else { Some(name) }
    }
}

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

/// Spawn a process detached from the assistant's Job Object (Windows) so it
/// survives when the assistant exits. On other platforms, this is a plain spawn.
/// Use for user-facing launches (apps, URLs, explorer, system commands).
pub fn spawn_detached(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::process::spawn_detached_impl(cmd)
    }

    #[cfg(not(target_os = "windows"))]
    {
        cmd.spawn()
    }
}
