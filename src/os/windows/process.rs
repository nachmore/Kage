// Windows process management

use anyhow::Result;
use log::{info, warn};
use std::process::Command;
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Check if a PID belongs to a kiro-related process (kiro-cli, node, etc.)
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
