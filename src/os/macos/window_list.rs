// macOS window enumeration using CGWindowList (fast, no Accessibility TCC).
//
// Uses CGWindowListCopyWindowInfo to enumerate on-screen windows. This is
// the same API used by `get_foreground_window_info()` for title extraction.
// Window titles require Screen Recording permission (macOS 10.15+); without
// it we still get process names and PIDs, just empty titles.
//
// Handles pack the CG window number with the owner PID (see `pack_handle`).
// Focus activates the app via NSRunningApplication.activateWithOptions,
// which only needs the PID — no Accessibility permission. When Kage is
// already Accessibility-trusted it also raises the exact window via AX, so
// picking one of several windows of the same app lands on that window;
// without the permission it degrades to app-level activation (no prompt).

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

/// Low bits of a handle hold the owner PID; the CG window number sits above.
/// macOS PIDs are capped at 99_999 (PID_MAX), well inside 20 bits, and the
/// packed value stays far below 2^53 so it survives the JS Number round
/// trip. Window number 0 (kCGNullWindowID) means "app only", so untitled
/// per-app entries keep handle == PID.
const PID_BITS: u32 = 20;
const PID_MASK: u64 = (1 << PID_BITS) - 1;

fn pack_handle(pid: u64, window_number: u32) -> u64 {
    (u64::from(window_number) << PID_BITS) | (pid & PID_MASK)
}

/// (pid, window_number) from a handle built by `pack_handle`.
fn unpack_handle(handle: u64) -> (i32, u32) {
    ((handle & PID_MASK) as i32, (handle >> PID_BITS) as u32)
}

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
    let key_number = CFString::from_static_string("kCGWindowNumber");

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
            let window_number = info
                .find(&key_number)
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0);
            windows.push(WindowInfo {
                title,
                process_name,
                handle: pack_handle(pid, window_number),
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

/// Extract app icons for a list of window handles (see `pack_handle`).
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
            let (pid, _) = unpack_handle(handle);
            let app = match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
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

    let (pid, window_number) = unpack_handle(handle);

    autoreleasepool(|_pool| {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
        match app {
            Some(app) => {
                // Un-hide/un-minimize the app first so its windows are restored.
                // unhide() brings back windows hidden via Cmd+H; for minimized
                // (Dock'd) windows, activateWithOptions also restores them when
                // the app becomes frontmost.
                app.unhide();

                // Raise the chosen window before activating so it's the
                // app's main window when activation lands — otherwise the
                // app comes forward with whichever window was last key.
                if window_number != 0 && !raise_window(pid, window_number) {
                    debug!(
                        "[window-walker] AX raise unavailable for window {window_number}; \
                         activating app only"
                    );
                }

                #[allow(deprecated)]
                // macOS 14 deprecates this but the replacement isn't available yet
                let options = NSApplicationActivationOptions::ActivateIgnoringOtherApps;
                let ok = app.activateWithOptions(options);
                if ok {
                    Ok(())
                } else {
                    // Fallback to osascript for cases where activateWithOptions fails
                    // (e.g. the app doesn't support activation this way)
                    focus_via_osascript(pid)
                }
            }
            None => Err(format!("No running application with PID {}", pid)),
        }
    })
}

/// Fallback: use osascript to focus a window. Only called when NSRunningApplication
/// activation fails.
fn focus_via_osascript(pid: i32) -> Result<(), String> {
    let script = format!(
        r#"tell application "System Events"
            set targetProc to first process whose unix id is {}
            set frontmost of targetProc to true
        end tell"#,
        pid
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

/// Raise one specific window of `pid` via Accessibility. Only runs when
/// Kage is already trusted (`AXIsProcessTrusted` never prompts); returns
/// false when untrusted or the window can't be matched, and the caller
/// falls back to plain app activation.
fn raise_window(pid: i32, window_number: u32) -> bool {
    use accessibility_sys as ax;
    use core_foundation::base::{CFRelease, CFTypeRef};
    use std::ffi::c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        // Private but long-stable HIServices SPI (used by yabai, AltTab,
        // Hammerspoon) mapping an AX window to its CGWindowID — there is
        // no public equivalent.
        fn _AXUIElementGetWindow(element: ax::AXUIElementRef, out: *mut u32) -> ax::AXError;
    }

    if !unsafe { ax::AXIsProcessTrusted() } {
        return false;
    }
    let app = unsafe { ax::AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return false;
    }

    let attr = CFString::from_static_string(ax::kAXWindowsAttribute);
    let mut value: CFTypeRef = std::ptr::null();
    let err =
        unsafe { ax::AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut value) };
    let mut raised = false;
    if err == ax::kAXErrorSuccess && !value.is_null() {
        // Owns the copied array; the window refs below borrow from it.
        let windows: CFArray<*const c_void> =
            unsafe { CFArray::wrap_under_create_rule(value as _) };
        let title = title_for_window_number(window_number);
        let mut by_id = None;
        let mut by_title = None;
        for item in windows.iter() {
            let win = *item as ax::AXUIElementRef;
            if win.is_null() {
                continue;
            }
            let mut wid: u32 = 0;
            if unsafe { _AXUIElementGetWindow(win, &mut wid) } == ax::kAXErrorSuccess
                && wid == window_number
            {
                by_id = Some(win);
                break;
            }
            // Fallback when the SPI fails: first window with the same title.
            if by_title.is_none()
                && title.is_some()
                && ax_string_attr(win, ax::kAXTitleAttribute) == title
            {
                by_title = Some(win);
            }
        }
        if let Some(win) = by_id.or(by_title) {
            let action = CFString::from_static_string(ax::kAXRaiseAction);
            raised = unsafe { ax::AXUIElementPerformAction(win, action.as_concrete_TypeRef()) }
                == ax::kAXErrorSuccess;
        }
    }
    unsafe { CFRelease(app as CFTypeRef) };
    raised
}

fn ax_string_attr(elem: accessibility_sys::AXUIElementRef, attr: &'static str) -> Option<String> {
    use accessibility_sys as ax;
    use core_foundation::base::CFTypeRef;

    let cf_attr = CFString::from_static_string(attr);
    let mut value: CFTypeRef = std::ptr::null();
    let err = unsafe {
        ax::AXUIElementCopyAttributeValue(elem, cf_attr.as_concrete_TypeRef(), &mut value)
    };
    if err != ax::kAXErrorSuccess || value.is_null() {
        return None;
    }
    // Copy rule: the wrapper takes ownership and releases on drop.
    let value = unsafe { CFType::wrap_under_create_rule(value) };
    value.downcast::<CFString>().map(|s| s.to_string())
}

/// Title of one CG window by number (Screen Recording permitting).
fn title_for_window_number(window_number: u32) -> Option<String> {
    use core_graphics::window::kCGWindowListOptionIncludingWindow;

    let list: CFArray<CFDictionary<CFString, CFType>> = unsafe {
        let cf_array =
            CGWindowListCopyWindowInfo(kCGWindowListOptionIncludingWindow, window_number);
        if cf_array.is_null() {
            return None;
        }
        CFArray::wrap_under_create_rule(cf_array as *const _)
    };
    let key_name = CFString::from_static_string("kCGWindowName");
    let info = list.iter().next()?;
    info.find(&key_name)
        .and_then(|v| v.downcast::<CFString>())
        .map(|s| s.to_string())
        .filter(|t| !t.is_empty())
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

#[cfg(test)]
mod tests {
    use super::{pack_handle, unpack_handle};

    #[test]
    fn handle_round_trips_pid_and_window_number() {
        let h = pack_handle(99_999, 123_456);
        assert_eq!(unpack_handle(h), (99_999, 123_456));
        // Stays exactly representable as a JS Number.
        assert!(h < (1u64 << 53));
    }

    #[test]
    fn app_only_handle_is_the_bare_pid() {
        assert_eq!(pack_handle(4321, 0), 4321);
        assert_eq!(unpack_handle(4321), (4321, 0));
    }
}
