// macOS window enumeration using osascript (AppleScript)
// Uses System Events to list windows and NSWorkspace to activate apps.

use crate::os::window_list::WindowInfo;
use std::process::Command;

pub fn list_windows_impl() -> Vec<WindowInfo> {
    // Use AppleScript via osascript to get window list — avoids native framework bindings
    let script = r#"
        tell application "System Events"
            set windowList to ""
            repeat with proc in (every process whose visible is true)
                set procName to name of proc
                set procId to unix id of proc
                try
                    set bundleId to bundle identifier of proc
                on error
                    set bundleId to ""
                end try
                repeat with win in (every window of proc)
                    set winTitle to name of win
                    if winTitle is not "" then
                        set windowList to windowList & procName & "	" & winTitle & "	" & procId & "	" & bundleId & linefeed
                    end if
                end repeat
            end repeat
        end tell
        return windowList
    "#;

    let output = match Command::new("osascript").arg("-e").arg(script).output() {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    if !output.status.success() { return vec![]; }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut windows = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 3 { continue; }
        let process_name = parts[0].to_string();
        let title = parts[1].to_string();
        let pid: u64 = parts[2].parse().unwrap_or(0);

        // Skip our own window
        if process_name.contains("Kage") { continue; }

        windows.push(WindowInfo {
            title,
            process_name,
            handle: pid, // use PID as handle — we activate by PID on macOS
            icon_base64: None,
        });
    }

    windows
}

pub fn focus_window_impl(handle: u64) -> Result<(), String> {
    // Activate the application by PID and restore if minimized
    let script = format!(
        r#"tell application "System Events"
            set targetProc to first process whose unix id is {}
            -- Restore minimized windows
            repeat with win in (every window of targetProc)
                if miniaturized of win then
                    set miniaturized of win to false
                end if
            end repeat
            set frontmost of targetProc to true
        end tell"#,
        handle
    );

    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("Failed to run osascript: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("Failed to focus window: {}", stderr.trim()))
    }
}

pub fn get_foreground_window_info() -> Option<(String, String)> {
    None // TODO: implement on macOS
}
