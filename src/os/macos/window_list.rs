// macOS window enumeration using CGWindowList (fast, no Accessibility TCC).
//
// Uses CGWindowListCopyWindowInfo to enumerate windows. This is the same
// API used by `get_foreground_window_info()` for title extraction. Window
// titles require Screen Recording permission (macOS 10.15+); without it we
// still get process names and PIDs, just empty titles.
//
// Order: only kCGWindowListOptionOnScreenOnly promises front-to-back order.
// kCGWindowListOptionAll's order is unspecified and does differ (the
// on-screen subset comes back shuffled), so on-screen entries always come
// from an OnScreenOnly call; the All call only supplies off-screen windows.
//
// Minimized windows: CGWindowList can't tell a minimized window from the
// other off-screen layer-0 windows (other Spaces, Cmd+H-hidden apps,
// background native tabs, and dozens of invisible helper windows) — they
// all just lack `kCGWindowIsOnscreen`. The only reliable signal is AX
// `kAXMinimizedAttribute`, which needs Accessibility. So:
//   - `list_windows_impl` (the cheap list the rest of Kage uses): on-screen
//     windows only, every entry `minimized: false`;
//   - `list_windows_with_minimized_impl` (`list_open_windows`): the same
//     while untrusted (`AXIsProcessTrusted` never prompts). Trusted, it also
//     lists all windows and asks AX about the off-screen ones that could add
//     an entry (see `ax_candidate_pids` / `ax_probe_order`), on this thread,
//     with a short per-message timeout and an overall budget so a hung app
//     can't stall the list. Minimized windows are appended after the
//     on-screen ones and flagged; other off-screen windows are dropped.
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
    kCGWindowListOptionOnScreenOnly, CGWindowListCopyWindowInfo, CGWindowListOption,
};
use log::debug;
use objc2_app_kit::NSApplicationActivationPolicy;
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

/// On-screen windows front to back; minimized ones aren't listed. Never
/// messages other apps.
pub fn list_windows_impl() -> Vec<WindowInfo> {
    list(false)
}

/// `list_windows_impl` plus, when Accessibility is trusted, the minimized
/// windows (flagged, after the on-screen ones). Can take up to
/// `AX_PROBE_BUDGET` plus one app's message timeout.
pub fn list_windows_with_minimized_impl() -> Vec<WindowInfo> {
    // Plain AXIsProcessTrusted never prompts. Without Accessibility there's
    // no way to pick the minimized windows out of the off-screen ones, so
    // don't pay for listing those.
    list(unsafe { accessibility_sys::AXIsProcessTrusted() })
}

fn list(with_minimized: bool) -> Vec<WindowInfo> {
    let on_screen = cg_layer0_windows(kCGWindowListOptionOnScreenOnly);
    let (off_screen, minimized) = if with_minimized {
        let off_screen = off_screen_windows(cg_layer0_windows(kCGWindowListOptionAll), &on_screen);
        let minimized = minimized_window_numbers(&on_screen, &off_screen);
        (off_screen, minimized)
    } else {
        (Vec::new(), HashSet::new())
    };
    let windows = merge_windows(&on_screen, &off_screen, &minimized);

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
}

/// Layer-0 (regular app) windows for `list_option`, in the order CG returns
/// them (front to back only for kCGWindowListOptionOnScreenOnly), Kage's
/// own excluded. CGWindowListCopyWindowInfo doesn't need Accessibility;
/// titles need Screen Recording and are empty strings without it.
fn cg_layer0_windows(list_option: CGWindowListOption) -> Vec<CgWindow> {
    let options = list_option | kCGWindowListExcludeDesktopElements;

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

        windows.push(CgWindow {
            pid,
            window_number,
            process_name,
            title,
        });
    }
    windows
}

/// The entries of an all-windows list that aren't in `on_screen`. Matched
/// by window number rather than `kCGWindowIsOnscreen`, so a window that
/// changes state between the two CG calls is never listed twice. Number 0
/// can't be matched to an AX window, so those are dropped.
fn off_screen_windows(all: Vec<CgWindow>, on_screen: &[CgWindow]) -> Vec<CgWindow> {
    let shown: HashSet<u32> = on_screen.iter().map(|w| w.window_number).collect();
    all.into_iter()
        .filter(|w| w.window_number != 0 && !shown.contains(&w.window_number))
        .collect()
}

/// Build the list: `on_screen` in the order given (z-order), then the
/// `off_screen` windows whose CG number is in `minimized`, flagged. Every
/// other off-screen window is dropped.
///
/// A titled window gets its own entry. Untitled windows (Screen Recording
/// not granted) collapse to one entry per app, titled with the process
/// name — so an app with any on-screen window shows as not minimized, and
/// one whose windows are all minimized shows as minimized.
fn merge_windows(
    on_screen: &[CgWindow],
    off_screen: &[CgWindow],
    minimized: &HashSet<u32>,
) -> Vec<WindowInfo> {
    let mut windows = Vec::new();
    let mut seen_pids_no_title: HashSet<u64> = HashSet::new();
    let shown = on_screen.iter().map(|w| (w, false));
    let hidden = off_screen
        .iter()
        .filter(|w| w.window_number != 0 && minimized.contains(&w.window_number))
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

/// PIDs worth asking AX about, in `off_screen` order (which CG doesn't
/// define — see `ax_probe_order` for the probe order): owners of an
/// off-screen window that `merge_windows` would list if it turned out
/// minimized. A titled window always would; an untitled one only when its
/// app has no on-screen untitled entry already. Without Screen Recording
/// that skips every app with a window on screen, which is most of them.
fn ax_candidate_pids(on_screen: &[CgWindow], off_screen: &[CgWindow]) -> Vec<u64> {
    let untitled_on_screen: HashSet<u64> = on_screen
        .iter()
        .filter(|w| w.title.is_empty())
        .map(|w| w.pid)
        .collect();
    let mut seen = HashSet::new();
    off_screen
        .iter()
        .filter(|w| w.window_number != 0)
        .filter(|w| !w.title.is_empty() || !untitled_on_screen.contains(&w.pid))
        .filter(|w| seen.insert(w.pid))
        .map(|w| w.pid)
        .collect()
}

/// Order AX candidates so the probe budget goes to the apps that minimize
/// windows into the Dock: `.regular` first, then `.accessory` (menu-bar
/// apps, which can still open the odd window), then PIDs AppKit doesn't
/// know; stable within each group. `.prohibited` processes are skipped:
/// they're view services ("AutoFill (…)", Open and Save Panel Service, …)
/// that make up about half the candidates, own no window a user can
/// minimize, and would each cost a round trip, or a timeout if busy.
fn ax_probe_order(
    pids: Vec<u64>,
    policy_of: impl Fn(u64) -> Option<NSApplicationActivationPolicy>,
) -> Vec<u64> {
    let mut ranked: Vec<(u8, u64)> = pids
        .into_iter()
        .filter_map(|pid| {
            let rank = match policy_of(pid) {
                Some(NSApplicationActivationPolicy::Prohibited) => return None,
                Some(NSApplicationActivationPolicy::Regular) => 0,
                Some(NSApplicationActivationPolicy::Accessory) => 1,
                _ => 2,
            };
            Some((rank, pid))
        })
        .collect();
    ranked.sort_by_key(|&(rank, _)| rank); // stable
    ranked.into_iter().map(|(_, pid)| pid).collect()
}

/// `pid`'s activation policy, from AppKit's in-process running-apps table
/// (no message to the app). `None` when AppKit doesn't know the PID.
fn activation_policy(pid: u64) -> Option<NSApplicationActivationPolicy> {
    use objc2_app_kit::NSRunningApplication;

    let pid = i32::try_from(pid).ok()?;
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .map(|app| app.activationPolicy())
}

/// CG window numbers of the candidates' minimized windows, via AX
/// `kAXMinimizedAttribute`. Call only when Accessibility is trusted. Runs
/// on the calling thread, like `raise_window` — not on kage-core's AX
/// worker, which exists for Computer Control's element registry (unneeded
/// here) and whose queue can be busy with a multi-second tree walk. Gives
/// up on further apps after `AX_PROBE_BUDGET`.
fn minimized_window_numbers(on_screen: &[CgWindow], off_screen: &[CgWindow]) -> HashSet<u32> {
    use objc2::rc::autoreleasepool;

    let mut found = HashSet::new();
    let pids = autoreleasepool(|_pool| {
        ax_probe_order(ax_candidate_pids(on_screen, off_screen), activation_policy)
    });
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
    use super::{
        ax_candidate_pids, ax_probe_order, merge_windows, off_screen_windows, pack_handle,
        unpack_handle, CgWindow,
    };
    use objc2_app_kit::NSApplicationActivationPolicy;
    use std::collections::HashSet;

    fn cg(pid: u64, window_number: u32, title: &str) -> CgWindow {
        CgWindow {
            pid,
            window_number,
            process_name: format!("app{pid}"),
            title: title.into(),
        }
    }

    fn set(ids: &[u32]) -> HashSet<u32> {
        ids.iter().copied().collect()
    }

    fn summary(list: &[crate::os::window_list::WindowInfo]) -> Vec<(&str, u64, bool)> {
        list.iter()
            .map(|w| (w.title.as_str(), w.handle, w.minimized))
            .collect()
    }

    #[test]
    fn on_screen_windows_first_then_minimized_and_other_off_screen_dropped() {
        let on_screen = [cg(10, 1, "Doc A"), cg(20, 4, "Mail")];
        let off_screen = [
            cg(10, 2, "Doc B (minimized)"),
            cg(20, 3, "Other Space"),
            cg(30, 5, "Notes (minimized)"),
        ];
        let list = merge_windows(&on_screen, &off_screen, &set(&[2, 5]));
        assert_eq!(
            summary(&list),
            [
                ("Doc A", pack_handle(10, 1), false),
                ("Mail", pack_handle(20, 4), false),
                ("Doc B (minimized)", pack_handle(10, 2), true),
                ("Notes (minimized)", pack_handle(30, 5), true),
            ]
        );
    }

    #[test]
    fn on_screen_order_is_kept_as_given() {
        // The on-screen list is CG's front-to-back order; the merge must not
        // regroup it by app or window number (Window Walker ranks by index).
        let on_screen = [cg(30, 9, "Front"), cg(10, 2, "Second"), cg(30, 1, "Third")];
        let off_screen = [cg(20, 7, "Min B"), cg(10, 3, "Min A")];
        let list = merge_windows(&on_screen, &off_screen, &set(&[3, 7]));
        let titles: Vec<&str> = list.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(titles, ["Front", "Second", "Third", "Min B", "Min A"]);
    }

    #[test]
    fn off_screen_windows_are_the_all_list_minus_on_screen_numbers() {
        // The all-windows list shuffles the on-screen windows among the
        // others; they're removed by number, whatever their position.
        let on_screen = [cg(10, 1, "A"), cg(20, 2, "B")];
        let all = vec![
            cg(20, 2, "B"),
            cg(30, 5, "Hidden"),
            cg(10, 1, "A"),
            cg(40, 0, "No number"),
            cg(10, 6, "Minimized"),
        ];
        let off: Vec<u32> = off_screen_windows(all, &on_screen)
            .iter()
            .map(|w| w.window_number)
            .collect();
        assert_eq!(off, [5, 6]);
    }

    #[test]
    fn untrusted_list_is_on_screen_only_and_never_minimized() {
        // What `list` builds without the probe: no off-screen list and no
        // AX answers.
        let on_screen = [cg(10, 1, "Doc A"), cg(20, 2, "")];
        let list = merge_windows(&on_screen, &[], &HashSet::new());
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|w| !w.minimized));
    }

    #[test]
    fn minimized_number_only_flags_off_screen_windows() {
        // An on-screen window is never reported minimized, whatever AX said.
        let list = merge_windows(&[cg(10, 1, "Doc")], &[], &set(&[1]));
        assert_eq!(list.len(), 1);
        assert!(!list[0].minimized);
    }

    #[test]
    fn untitled_windows_collapse_per_app() {
        // No Screen Recording: titles are empty.
        let on_screen = [cg(10, 1, "")];
        let off_screen = [
            cg(10, 2, ""), // minimized, but app 10 is already listed
            cg(20, 3, ""), // app 20's only windows are minimized
            cg(20, 4, ""),
            cg(30, 5, ""), // off-screen, not minimized
        ];
        let list = merge_windows(&on_screen, &off_screen, &set(&[2, 3, 4]));
        assert_eq!(
            summary(&list),
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
        let on_screen = [cg(10, 1, ""), cg(20, 3, "Titled"), cg(40, 7, "On screen")];
        let off_screen = [
            cg(10, 2, ""), // app 10 already has an untitled entry
            cg(20, 4, ""), // app 20 has no untitled entry yet
            cg(30, 5, "Other Space"),
            cg(30, 6, "Another"),
            cg(50, 0, "No number"),
            cg(10, 8, "Titled off-screen"),
        ];
        assert_eq!(ax_candidate_pids(&on_screen, &off_screen), [20, 30, 10]);
    }

    #[test]
    fn ax_probe_order_puts_dock_apps_first_and_skips_view_services() {
        let policy = |pid: u64| match pid {
            1 | 5 => Some(NSApplicationActivationPolicy::Prohibited),
            2 | 6 => Some(NSApplicationActivationPolicy::Accessory),
            3 | 7 => Some(NSApplicationActivationPolicy::Regular),
            _ => None, // 4, 8: not a running application AppKit knows
        };
        assert_eq!(
            ax_probe_order(vec![1, 2, 3, 4, 5, 6, 7, 8], policy),
            [3, 7, 2, 6, 4, 8]
        );
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
