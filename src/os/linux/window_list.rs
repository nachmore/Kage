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
        let Some((handle, pid, title)) = parse_wmctrl_line(line) else {
            continue;
        };
        let title = title.to_string();

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

/// Split one `wmctrl -l -p` line into (handle, pid, title). wmctrl pads
/// its columns with printf widths (`0x%.8lx %2ld %-6lu %s %s`), so the
/// separators are runs of spaces — take the four leading tokens (id,
/// desktop, pid, host) by whitespace runs and keep the remainder verbatim
/// as the title, preserving any spacing inside it.
fn parse_wmctrl_line(line: &str) -> Option<(u64, u32, &str)> {
    fn next_token(s: &str) -> Option<(&str, &str)> {
        let s = s.trim_start();
        if s.is_empty() {
            return None;
        }
        Some(s.split_once(char::is_whitespace).unwrap_or((s, "")))
    }
    let (id, rest) = next_token(line)?;
    let (_desktop, rest) = next_token(rest)?;
    let (pid, rest) = next_token(rest)?;
    let (_host, rest) = next_token(rest)?;
    let handle = u64::from_str_radix(id.trim_start_matches("0x"), 16).ok()?;
    Some((handle, pid.parse().unwrap_or(0), rest.trim()))
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
    let wmctrl_ok = Command::new("wmctrl")
        .args(["-ia", &format!("0x{:x}", handle)])
        .status()
        .is_ok_and(|status| status.success());
    if wmctrl_ok {
        return Ok(());
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

pub fn get_window_icons(_handles: &[u64]) -> std::collections::HashMap<u64, String> {
    std::collections::HashMap::new() // TODO: implement on Linux
}

#[cfg(test)]
mod tests {
    use super::parse_wmctrl_line;

    #[test]
    fn parses_padded_wmctrl_columns() {
        let (handle, pid, title) =
            parse_wmctrl_line("0x03a00003  0 1234   myhost Firefox  -  Docs").unwrap();
        assert_eq!(handle, 0x03a00003);
        assert_eq!(pid, 1234);
        assert_eq!(title, "Firefox  -  Docs");
    }

    #[test]
    fn parses_sticky_window_and_missing_host() {
        let (_, pid, title) = parse_wmctrl_line("0x01e00006 -1 987    N/A Panel").unwrap();
        assert_eq!(pid, 987);
        assert_eq!(title, "Panel");
    }

    #[test]
    fn untitled_and_malformed_lines() {
        assert_eq!(
            parse_wmctrl_line("0x01e00006  0 42     host").unwrap().2,
            ""
        );
        assert!(parse_wmctrl_line("0x01e00006  0").is_none());
        assert!(parse_wmctrl_line("").is_none());
    }
}
