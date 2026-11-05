// macOS window enumeration using CGWindowList (fast, no Accessibility TCC).
//
// Uses CGWindowListCopyWindowInfo to enumerate windows. This is the same
// API used by `get_foreground_window_info()` for title extraction. Window
// titles require Screen Recording permission (macOS 10.15+); without it we
// still get process names and PIDs, just empty titles.
//
// Minimized windows: CGWindowList can't tell a minimized window from the
// other off-screen layer-0 windows (other Spaces, Cmd+H-hidden apps,
// background native tabs, and dozens of invisible helper windows) — they
// all just lack `kCGWindowIsOnscreen`. The only reliable signal is AX
// `kAXMinimizedAttribute`, which needs Accessibility. So:
//   - untrusted: on-screen windows only, every entry `minimized: false`
//     (the list as it has always been; `AXIsProcessTrusted` never prompts);
//   - trusted: all windows, then AX is asked about off-screen windows that
//     could add an entry (see `ax_candidate_pids`), on this thread, with a
//     short per-message timeout and an overall budget so a hung app can't
//     stall the list. Minimized windows are appended after the on-screen
//     ones and flagged; other off-screen windows are still dropped.
//
// Handles pack the CG window number with the owner PID (see `pack_handle`).
// Focus activates the app via NSRunningApplication.activateWithOptions,
// which only needs the PID — no Accessibility permission. When Kage is
// already Accessibility-trusted it also restores (if minimized) and raises
// the exact window via AX, so picking one of several windows of the same
// app lands on that window; without the permission it degrades to
// app-level activation (no prompt).

use crate::os::window_list::WindowInfo;
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::window::{
    kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionAll,
    kCGWindowListOptionOnScreenOnly, CGWindowListCopyWindowInfo,
};
use log::debug;
use std::collections::HashSet;
use std::process::Command;
use std::time::{Duration, Instant};

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    // Private but long-stable HIServices SPI (used by yabai, AltTab,
    // Hammerspoon) mapping an AX window to its CGWindowID — there is no
    // public equivalent.
    fn _AXUIElementGetWindow(
        element: accessibility_sys::AXUIElementRef,
        out: *mut u32,
    ) -> accessibility_sys::AXError;
}

/// Per-message AX timeout while probing for minimized windows. Responsive
/// apps answer far sooner; the default (6 s) would let one hung app freeze
/// the window switcher.
const AX_MESSAGE_TIMEOUT_SECS: f32 = 0.1;

/// Stop asking further apps once the minimized-window probe has run this
/// long; their off-screen windows are dropped, as when untrusted.
const AX_PROBE_BUDGET: Duration = Duration::from_millis(150);

/// Low bits of a handle hold the owner PID; the CG window number sits above.
/// macOS PIDs are capped at 99_999 (PID_MAX), well inside 20 bits, and the
/// packed value stays far below 2^53 so it survives the JS Number round
/// trip. Window number 0 (kCGNullWindowID) means "app only", so untitled
/// per-app entries keep handle == PID — except a minimized-only app's
/// entry, which carries its window so focusing it can restore that window.
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
    // Plain AXIsProcessTrusted never prompts. Without Accessibility there's
    // no way to pick the minimized windows out of the off-screen ones, so
    // don't pay for listing those (all ~280 windows of a busy desktop: ~5
    // ms; just the on-screen ones: ~1 ms).
    let trusted = unsafe { accessibility_sys::AXIsProcessTrusted() };
    let entries = cg_layer0_windows(!trusted);
    let minimized = if trusted {
        minimized_window_numbers(&entries)
    } else {
        HashSet::new()
    };
    let windows = merge_windows(&entries, &minimized);

    debug!(
        "[window-walker] CGWindowList returned {} windows ({} minimized)",
        windows.len(),
        windows.iter().filter(|w| w.minimized).count()
    );

    windows
}

/// One layer-0 CGWindowList entry, reduced to what the list needs.
#[derive(Debug)]
struct CgWindow {
    pid: u64,
    window_number: u32,
    process_name: String,
    /// Empty without Screen Recording.
    title: String,
    on_screen: bool,
}

/// Layer-0 (regular app) windows in front-to-back order, Kage's own
/// excluded. `on_screen_only` lists just the windows on the current Space;
/// otherwise every window, with `on_screen` from `kCGWindowIsOnscreen`.
/// CGWindowListCopyWindowInfo doesn't need Accessibility; titles need
/// Screen Recording and are empty strings without it.
fn cg_layer0_windows(on_screen_only: bool) -> Vec<CgWindow> {
    let options = if on_screen_only {
        kCGWindowListOptionOnScreenOnly
    } else {
        kCGWindowListOptionAll
    } | kCGWindowListExcludeDesktopElements;

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
    let key_on_screen = CFString::from_static_string("kCGWindowIsOnscreen");

    let mut windows = Vec::new();
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

        let window_number = info
            .find(&key_number)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);

        // Optional key, present (true) only for on-screen windows.
        let on_screen = on_screen_only
            || info
                .find(&key_on_screen)
                .and_then(|v| v.downcast::<CFBoolean>())
                .is_some_and(bool::from);

        windows.push(CgWindow {
            pid,
            window_number,
            process_name,
            title,
            on_screen,
        });
    }
    windows
}

/// Build the list from `cg_layer0_windows` output: on-screen windows in
/// z-order, then the off-screen ones in `minimized` (CG window numbers),
/// flagged. Every other off-screen window is dropped.
///
/// A titled window gets its own entry. Untitled windows (Screen Recording
/// not granted) collapse to one entry per app, titled with the process
/// name — so an app with any on-screen window shows as not minimized, and
/// one whose windows are all minimized shows as minimized.
fn merge_windows(entries: &[CgWindow], minimized: &HashSet<u32>) -> Vec<WindowInfo> {
    let mut windows = Vec::new();
    let mut seen_pids_no_title: HashSet<u64> = HashSet::new();
    let shown = entries.iter().filter(|w| w.on_screen).map(|w| (w, false));
    let hidden = entries
        .iter()
        .filter(|w| !w.on_screen && w.window_number != 0 && minimized.contains(&w.window_number))
        .map(|w| (w, true));

    for (w, is_minimized) in shown.chain(hidden) {
        if w.title.is_empty() {
            if !seen_pids_no_title.insert(w.pid) {
                continue;
            }
            // On-screen: handle == PID, so focus activates the app. Minimized
            // only: point at the window so focus can restore it.
            let handle = if is_minimized {
                pack_handle(w.pid, w.window_number)
            } else {
                w.pid
            };
            windows.push(WindowInfo {
                title: w.process_name.clone(),
                process_name: w.process_name.clone(),
                handle,
                icon_base64: None,
                minimized: is_minimized,
            });
        } else {
            windows.push(WindowInfo {
                title: w.title.clone(),
                process_name: w.process_name.clone(),
                handle: pack_handle(w.pid, w.window_number),
                icon_base64: None,
                minimized: is_minimized,
            });
        }
    }
    windows
}

/// PIDs worth asking AX about, in z-order: owners of an off-screen window
/// that `merge_windows` would list if it turned out minimized. A titled
/// window always would; an untitled one only when its app has no
/// on-screen untitled entry already. Without Screen Recording that skips
/// every app with a window on screen, which is most of them.
fn ax_candidate_pids(entries: &[CgWindow]) -> Vec<u64> {
    let untitled_on_screen: HashSet<u64> = entries
        .iter()
        .filter(|w| w.on_screen && w.title.is_empty())
        .map(|w| w.pid)
        .collect();
    let mut seen = HashSet::new();
    entries
        .iter()
        .filter(|w| !w.on_screen && w.window_number != 0)
        .filter(|w| !w.title.is_empty() || !untitled_on_screen.contains(&w.pid))
        .filter(|w| seen.insert(w.pid))
        .map(|w| w.pid)
        .collect()
}

/// CG window numbers of the candidates' minimized windows, via AX
/// `kAXMinimizedAttribute`. Call only when Accessibility is trusted. Runs
/// on the calling thread, like `raise_window` — not on kage-core's AX
/// worker, which exists for Computer Control's element registry (unneeded
/// here) and whose queue can be busy with a multi-second tree walk. Gives
/// up on further apps after `AX_PROBE_BUDGET`.
fn minimized_window_numbers(entries: &[CgWindow]) -> HashSet<u32> {
    let mut found = HashSet::new();
    let pids = ax_candidate_pids(entries);
    let started = Instant::now();
    for (i, &pid) in pids.iter().enumerate() {
        if started.elapsed() >= AX_PROBE_BUDGET {
            debug!(
                "[window-walker] minimized-window probe over budget; skipped {} of {} apps",
                pids.len() - i,
                pids.len()
            );
            break;
        }
        let Ok(pid) = i32::try_from(pid) else {
            continue;
        };
        collect_minimized_windows(pid, &mut found);
    }
    found
}

/// Add the CG window numbers of `pid`'s minimized windows to `out`. Every
/// AX read messages the target app, so each element gets a short timeout,
/// and an app that stops answering is abandoned rather than paying that
/// timeout once per window.
fn collect_minimized_windows(pid: i32, out: &mut HashSet<u32>) {
    use accessibility_sys as ax;
    use core_foundation::base::{CFRelease, CFTypeRef};
    use std::ffi::c_void;

    let app = unsafe { ax::AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return;
    }
    unsafe { ax::AXUIElementSetMessagingTimeout(app, AX_MESSAGE_TIMEOUT_SECS) };

    let attr = CFString::from_static_string(ax::kAXWindowsAttribute);
    let mut value: CFTypeRef = std::ptr::null();
    let err =
        unsafe { ax::AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut value) };
    if err == ax::kAXErrorSuccess && !value.is_null() {
        // Copy rule: owns the array; the window refs below borrow from it.
        let value = unsafe { CFType::wrap_under_create_rule(value) };
        if let Some(windows) = value.downcast::<CFArray<*const c_void>>() {
            for item in windows.iter() {
                let win = *item as ax::AXUIElementRef;
                if win.is_null() {
                    continue;
                }
                unsafe { ax::AXUIElementSetMessagingTimeout(win, AX_MESSAGE_TIMEOUT_SECS) };
                match ax_bool_attr(win, ax::kAXMinimizedAttribute) {
                    Ok(true) => {
                        let mut wid: u32 = 0;
                        if unsafe { _AXUIElementGetWindow(win, &mut wid) } == ax::kAXErrorSuccess
                            && wid != 0
                        {
                            out.insert(wid);
                        }
                    }
                    Ok(false) => {}
                    Err(ax::kAXErrorCannotComplete) => break,
                    // E.g. kAXErrorAttributeUnsupported on an odd window.
                    Err(_) => {}
                }
            }
        }
    }
    unsafe { CFRelease(app as CFTypeRef) };
}

/// Read a boolean AX attribute. `Err` carries the AXError, or
/// `kAXErrorNoValue` / `kAXErrorIllegalArgument` for a missing or
/// non-boolean value.
fn ax_bool_attr(
    elem: accessibility_sys::AXUIElementRef,
    attr: &'static str,
) -> Result<bool, accessibility_sys::AXError> {
    use accessibility_sys as ax;
    use core_foundation::base::CFTypeRef;

    let cf_attr = CFString::from_static_string(attr);
    let mut value: CFTypeRef = std::ptr::null();
    let err = unsafe {
        ax::AXUIElementCopyAttributeValue(elem, cf_attr.as_concrete_TypeRef(), &mut value)
    };
    if err != ax::kAXErrorSuccess {
        return Err(err);
    }
    if value.is_null() {
        return Err(ax::kAXErrorNoValue);
    }
    // Copy rule: the wrapper takes ownership and releases on drop.
    let value = unsafe { CFType::wrap_under_create_rule(value) };
    value
        .downcast::<CFBoolean>()
        .map(bool::from)
        .ok_or(ax::kAXErrorIllegalArgument)
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
                // unhide() brings back windows hidden via Cmd+H.
                app.unhide();

                // Restore (if minimized) and raise the chosen window before
                // activating so it's the app's main window when activation
                // lands — otherwise the app comes forward with whichever
                // window was last key. Activation alone doesn't bring a
                // window back out of the Dock.
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

/// Restore (if minimized) and raise one specific window of `pid` via
/// Accessibility. Only runs when Kage is already trusted
/// (`AXIsProcessTrusted` never prompts); returns false when untrusted or
/// the window can't be matched, and the caller falls back to plain app
/// activation.
fn raise_window(pid: i32, window_number: u32) -> bool {
    use accessibility_sys as ax;
    use core_foundation::base::{CFRelease, CFTypeRef};
    use std::ffi::c_void;

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
            if ax_bool_attr(win, ax::kAXMinimizedAttribute) == Ok(true) {
                let attr = CFString::from_static_string(ax::kAXMinimizedAttribute);
                let restored = unsafe {
                    ax::AXUIElementSetAttributeValue(
                        win,
                        attr.as_concrete_TypeRef(),
                        CFBoolean::false_value().as_CFTypeRef(),
                    )
                };
                if restored != ax::kAXErrorSuccess {
                    debug!(
                        "[window-walker] AX un-minimize failed for window {window_number}: \
                         {restored}"
                    );
                }
            }
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
pub(crate) fn frontmost_app_info() -> Option<(i32, String)> {
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
    use super::{ax_candidate_pids, merge_windows, pack_handle, unpack_handle, CgWindow};
    use std::collections::HashSet;

    fn cg(pid: u64, window_number: u32, title: &str, on_screen: bool) -> CgWindow {
        CgWindow {
            pid,
            window_number,
            process_name: format!("app{pid}"),
            title: title.into(),
            on_screen,
        }
    }

    fn set(ids: &[u32]) -> HashSet<u32> {
        ids.iter().copied().collect()
    }

    #[test]
    fn on_screen_windows_first_then_minimized_and_other_off_screen_dropped() {
        let entries = [
            cg(10, 1, "Doc A", true),
            cg(10, 2, "Doc B (minimized)", false),
            cg(20, 3, "Other Space", false),
            cg(20, 4, "Mail", true),
            cg(30, 5, "Notes (minimized)", false),
        ];
        let list = merge_windows(&entries, &set(&[2, 5]));
        let got: Vec<(&str, u64, bool)> = list
            .iter()
            .map(|w| (w.title.as_str(), w.handle, w.minimized))
            .collect();
        assert_eq!(
            got,
            [
                ("Doc A", pack_handle(10, 1), false),
                ("Mail", pack_handle(20, 4), false),
                ("Doc B (minimized)", pack_handle(10, 2), true),
                ("Notes (minimized)", pack_handle(30, 5), true),
            ]
        );
    }

    #[test]
    fn untrusted_list_is_on_screen_only_and_never_minimized() {
        // What `list_windows_impl` builds without Accessibility: an
        // on-screen-only CG list and no AX answers.
        let entries = [cg(10, 1, "Doc A", true), cg(20, 2, "", true)];
        let list = merge_windows(&entries, &HashSet::new());
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|w| !w.minimized));
    }

    #[test]
    fn minimized_number_only_flags_off_screen_windows() {
        // An on-screen window is never reported minimized, whatever AX said.
        let list = merge_windows(&[cg(10, 1, "Doc", true)], &set(&[1]));
        assert_eq!(list.len(), 1);
        assert!(!list[0].minimized);
    }

    #[test]
    fn untitled_windows_collapse_per_app() {
        // No Screen Recording: titles are empty.
        let entries = [
            cg(10, 1, "", true),
            cg(10, 2, "", false), // minimized, but app 10 is already listed
            cg(20, 3, "", false), // app 20's only windows are minimized
            cg(20, 4, "", false),
            cg(30, 5, "", false), // off-screen, not minimized
        ];
        let list = merge_windows(&entries, &set(&[2, 3, 4]));
        let got: Vec<(&str, u64, bool)> = list
            .iter()
            .map(|w| (w.title.as_str(), w.handle, w.minimized))
            .collect();
        assert_eq!(
            got,
            [
                // On-screen per-app entry keeps handle == PID.
                ("app10", 10, false),
                // Minimized-only app carries its first minimized window.
                ("app20", pack_handle(20, 3), true),
            ]
        );
    }

    #[test]
    fn ax_candidates_are_apps_whose_off_screen_windows_could_be_listed() {
        let entries = [
            cg(10, 1, "", true),
            cg(10, 2, "", false), // app 10 already has an untitled entry
            cg(20, 3, "Titled", true),
            cg(20, 4, "", false), // app 20 has no untitled entry yet
            cg(30, 5, "Other Space", false),
            cg(30, 6, "Another", false),
            cg(40, 7, "On screen", true), // nothing off-screen
            cg(50, 0, "No number", false),
            cg(10, 8, "Titled off-screen", false),
        ];
        assert_eq!(ax_candidate_pids(&entries), [20, 30, 10]);
    }

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
