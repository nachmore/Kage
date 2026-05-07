// Linux window enumeration using wmctrl
// Falls back to xdotool if wmctrl is not available.

use crate::os::window_list::WindowInfo;
use std::process::Command;

pub fn list_windows_impl() -> Vec<WindowInfo> {
    // Try wmctrl first (most common on X11 desktops)
    if let Some(windows) = list_with_wmctrl() {
        return windows;
    }
    // Fallback to xdotool
    if let Some(windows) = list_with_xdotool() {
        return windows;
    }
    vec![]
}

fn list_with_wmctrl() -> Option<Vec<WindowInfo>> {
    // wmctrl -l -p outputs: <hwnd> <desktop> <pid> <hostname> <title>
    let output = Command::new("wmctrl").args(["-l", "-p"]).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut windows = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(5, char::is_whitespace).collect();
        if parts.len() < 5 {
            continue;
        }

        let handle_str = parts[0].trim();
        let handle = u64::from_str_radix(handle_str.trim_start_matches("0x"), 16).unwrap_or(0);
        let pid: u32 = parts[2].trim().parse().unwrap_or(0);
        let title = parts[4].trim().to_string();

        if title.is_empty() || title == "Desktop" {
            continue;
        }
        if title.contains("Kage") {
            continue;
        }

        let process_name = if pid > 0 {
            get_process_name_linux(pid)
        } else {
            String::new()
        };

        windows.push(WindowInfo {
            title,
            process_name,
            handle,
            icon_base64: None,
        });
    }

    Some(windows)
}

fn list_with_xdotool() -> Option<Vec<WindowInfo>> {
    let output = Command::new("xdotool")
        .args(["search", "--onlyvisible", "--name", ""])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut windows = Vec::new();

    for line in stdout.lines() {
        let handle: u64 = line.trim().parse().unwrap_or(0);
        if handle == 0 {
            continue;
        }

        // Get window name
        let name_output = Command::new("xdotool")
            .args(["getwindowname", &handle.to_string()])
            .output()
            .ok();
        let title = name_output
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        if title.is_empty() || title.contains("Kage") {
            continue;
        }

        // Get PID
        let pid_output = Command::new("xdotool")
            .args(["getwindowpid", &handle.to_string()])
            .output()
            .ok();
        let pid: u32 = pid_output
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(0);

        let process_name = if pid > 0 {
            get_process_name_linux(pid)
        } else {
            String::new()
        };

        windows.push(WindowInfo {
            title,
            process_name,
            handle,
            icon_base64: None,
        });
    }

    Some(windows)
}

fn get_process_name_linux(pid: u32) -> String {
    // Read /proc/<pid>/comm for the process name
    std::fs::read_to_string(format!("/proc/{}/comm", pid))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn focus_window_impl(handle: u64) -> Result<(), String> {
    // Try wmctrl first — -ia activates and restores minimized windows
    let result = Command::new("wmctrl")
        .args(["-ia", &format!("0x{:x}", handle)])
        .status();

    match result {
        Ok(status) if status.success() => return Ok(()),
        _ => {}
    }

    // Fallback to xdotool — windowactivate restores minimized windows
    let result = Command::new("xdotool")
        .args(["windowactivate", "--sync", &handle.to_string()])
        .status();

    match result {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("xdotool exited with {}", status)),
        Err(e) => Err(format!("Failed to focus window: {}", e)),
    }
}

pub fn get_foreground_window_info() -> Option<(String, String)> {
    None // TODO: implement on Linux
}
