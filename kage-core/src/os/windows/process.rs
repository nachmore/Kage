// Windows process helpers (kage-core subset — see os/process.rs).

use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;

/// Check if a PID belongs to a kage-related process (agent backend, node, etc.)
/// Returns the process name if found, None if the process doesn't exist or isn't ours.
pub fn get_process_name_impl(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

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

/// Spawn a process that is detached from our Job Object so it survives
/// when the parent exits. Use this for user-facing launches (apps,
/// URLs, explorer, system commands) — NOT for internal child processes
/// like the agent backend or TTS servers that should die with us.
pub fn spawn_detached_impl(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    cmd.creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW);
    cmd.spawn()
}
