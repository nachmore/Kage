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

    if !output.status.success() {
        return vec![];
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut windows = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 3 {
            continue;
        }
        let process_name = parts[0].to_string();
        let title = parts[1].to_string();
        let pid: u64 = parts[2].parse().unwrap_or(0);

        // Skip our own window
        if process_name.contains("Kage") {
            continue;
        }

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
    // Fast, permissionless path: NSWorkspace.frontmostApplication gives us
    // PID + localizedName with no TCC prompt. Title extraction below requires
    // Screen Recording permission (macOS 10.15+); without it we return an
    // empty title so activity tracking still works at app granularity.
    let (pid, process_name) = frontmost_app_info()?;

    // Skip our own windows — matches the Windows impl's "contains \"Kage\"" check.
    if process_name.contains("Kage") {
        return None;
    }

    let title = window_title_for_pid(pid).unwrap_or_default();
    Some((title, process_name))
}

/// Ask NSWorkspace for the currently-frontmost running application.
/// Returns (pid, localizedName). Permissionless — no TCC prompt.
fn frontmost_app_info() -> Option<(i32, String)> {
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::NSWorkspace;

    autoreleasepool(|_pool| {
        let workspace = NSWorkspace::sharedWorkspace();
        let app = workspace.frontmostApplication()?;
        let pid = app.processIdentifier();
        let name = app.localizedName()?;
        Some((pid, name.to_string()))
    })
}

/// Look up the title of the frontmost on-screen window owned by `pid` via
/// CGWindowListCopyWindowInfo. Returns None if the title is unavailable —
/// usually because the Screen Recording TCC permission hasn't been granted
/// yet (macOS 10.15+ gates `kCGWindowName` behind it).
fn window_title_for_pid(pid: i32) -> Option<String> {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        kCGNullWindowID, kCGWindowListOptionOnScreenOnly, CGWindowListCopyWindowInfo,
    };

    // Safety: CGWindowListCopyWindowInfo returns a retained CFArrayRef or NULL.
    // core-graphics' wrapper handles the retain/release for us.
    let window_list: CFArray<CFDictionary<CFString, CFType>> = unsafe {
        let cf_array = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID);
        if cf_array.is_null() {
            return None;
        }
        CFArray::wrap_under_create_rule(cf_array as *const _)
    };

    // Windows are returned in z-order, front-most first. Find the first one
    // owned by `pid` that has both a non-empty title and layer == 0 (regular
    // app windows — filters out menu bar, dock, notifications, etc.).
    let key_pid = CFString::from_static_string("kCGWindowOwnerPID");
    let key_name = CFString::from_static_string("kCGWindowName");
    let key_layer = CFString::from_static_string("kCGWindowLayer");

    for info in window_list.iter() {
        let win_pid = info
            .find(&key_pid)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i32())
            .unwrap_or(-1);
        if win_pid != pid {
            continue;
        }

        let layer = info
            .find(&key_layer)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i32())
            .unwrap_or(i32::MAX);
        if layer != 0 {
            continue;
        }

        let title = info
            .find(&key_name)
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if !title.is_empty() {
            return Some(title);
        }
        // First matching window had an empty/missing title; keep scanning in
        // case a later window for the same PID has one (rare, but happens
        // when a modal sheet sits on top of a real titled window).
    }

    None
}
