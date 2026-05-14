// macOS window enumeration using CGWindowList (fast, no Accessibility TCC).
//
// Uses CGWindowListCopyWindowInfo to enumerate on-screen windows. This is
// the same API used by `get_foreground_window_info()` for title extraction.
// Window titles require Screen Recording permission (macOS 10.15+); without
// it we still get process names and PIDs, just empty titles.
//
// Focus uses NSRunningApplication.activateWithOptions which only needs the
// PID — no Accessibility permission required for basic activation.

use crate::os::window_list::WindowInfo;
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::window::{
    kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    CGWindowListCopyWindowInfo,
};
use log::debug;
use std::collections::HashSet;
use std::process::Command;

pub fn list_windows_impl() -> Vec<WindowInfo> {
    // CGWindowListCopyWindowInfo is fast (~1-5ms) and doesn't require
    // Accessibility permission. Window titles require Screen Recording
    // permission; without it they'll be empty strings.
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;

    let window_list: CFArray<CFDictionary<CFString, CFType>> = unsafe {
        let cf_array = CGWindowListCopyWindowInfo(options, kCGNullWindowID);
        if cf_array.is_null() {
            debug!("[window-walker] CGWindowListCopyWindowInfo returned null");
            return vec![];
        }
        CFArray::wrap_under_create_rule(cf_array as *const _)
    };

    let key_pid = CFString::from_static_string("kCGWindowOwnerPID");
    let key_name = CFString::from_static_string("kCGWindowName");
    let key_layer = CFString::from_static_string("kCGWindowLayer");
    let key_owner = CFString::from_static_string("kCGWindowOwnerName");

    let mut windows = Vec::new();
    // Track PIDs we've already added a window for — show one entry per app
    // when the title is empty (Screen Recording not granted), but show all
    // titled windows when we have permission.
    let mut seen_pids_no_title: HashSet<u64> = HashSet::new();

    for info in window_list.iter() {
        // Only layer 0 = regular app windows (skip menubar, dock, etc.)
        let layer = info
            .find(&key_layer)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i32())
            .unwrap_or(i32::MAX);
        if layer != 0 {
            continue;
        }

        let pid = info
            .find(&key_pid)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(0) as u64;
        if pid == 0 {
            continue;
        }

        let process_name = info
            .find(&key_owner)
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .unwrap_or_default();

        // Skip our own windows
        if process_name.contains("Kage") || process_name.contains("kage") {
            continue;
        }

        let title = info
            .find(&key_name)
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .unwrap_or_default();

        // If we have a title, show each window individually.
        // If no title (Screen Recording not granted), show one entry per app.
        if title.is_empty() {
            if seen_pids_no_title.contains(&pid) {
                continue;
            }
            seen_pids_no_title.insert(pid);
            // Use process name as the display title when we can't get window titles
            windows.push(WindowInfo {
                title: process_name.clone(),
                process_name,
                handle: pid,
                icon_base64: None,
            });
        } else {
            windows.push(WindowInfo {
                title,
                process_name,
                handle: pid,
                icon_base64: None,
            });
        }
    }

    debug!(
        "[window-walker] CGWindowList returned {} windows",
        windows.len()
    );

    windows
}

/// Extract app icons for a list of window handles (PIDs on macOS).
/// Returns a map of handle → base64 icon.
/// Uses NSRunningApplication to get the bundle path, then NSWorkspace.iconForFile.
/// Results are cached in the cross-platform icon-by-name cache.
pub fn get_window_icons(handles: &[u64]) -> std::collections::HashMap<u64, String> {
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::NSRunningApplication;
    use std::collections::HashMap;

    let mut result: HashMap<u64, String> = HashMap::new();

    autoreleasepool(|_pool| {
        for &handle in handles {
            // Check the by-name cache first (may have been populated by a prior call)
            let app = match NSRunningApplication::runningApplicationWithProcessIdentifier(
                handle as i32,
            ) {
                Some(a) => a,
                None => continue,
            };
            let process_name = app
                .localizedName()
                .map(|n| n.to_string())
                .unwrap_or_default();

            // Fast path: already cached by process name
            if let Some(cached) = crate::os::icon::get_icon_by_process_name(&process_name) {
                result.insert(handle, cached);
                continue;
            }

            // Extract from bundle path
            let bundle_url = match app.bundleURL() {
                Some(u) => u,
                None => continue,
            };
            let path = match bundle_url.path() {
                Some(p) => p.to_string(),
                None => continue,
            };
            if let Some(icon) = crate::os::icon::extract_icon_base64(&path) {
                crate::os::icon::register_process_name_icon(&process_name, &icon);
                result.insert(handle, icon);
            }
        }
    });

    result
}

pub fn focus_window_impl(handle: u64) -> Result<(), String> {
    // Use NSRunningApplication to activate by PID — fast, no Accessibility TCC.
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::NSApplicationActivationOptions;
    use objc2_app_kit::NSRunningApplication;

    let pid = handle as i32;

    autoreleasepool(|_pool| {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
        match app {
            Some(app) => {
                // Un-hide/un-minimize the app first so its windows are restored.
                // unhide() brings back windows hidden via Cmd+H; for minimized
                // (Dock'd) windows, activateWithOptions also restores them when
                // the app becomes frontmost.
                app.unhide();

                #[allow(deprecated)]
                // macOS 14 deprecates this but the replacement isn't available yet
                let options = NSApplicationActivationOptions::ActivateIgnoringOtherApps;
                let ok = app.activateWithOptions(options);
                if ok {
                    Ok(())
                } else {
                    // Fallback to osascript for cases where activateWithOptions fails
                    // (e.g. the app doesn't support activation this way)
                    focus_via_osascript(handle)
                }
            }
            None => Err(format!("No running application with PID {}", pid)),
        }
    })
}

/// Fallback: use osascript to focus a window. Only called when NSRunningApplication
/// activation fails.
fn focus_via_osascript(handle: u64) -> Result<(), String> {
    let script = format!(
        r#"tell application "System Events"
            set targetProc to first process whose unix id is {}
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
