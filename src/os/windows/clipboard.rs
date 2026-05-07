// Windows clipboard operations using Win32 API.
//
// Clipboard data-exchange (OpenClipboard/GetClipboardData/GlobalAlloc/etc.)
// is hand-rolled extern blocks because pulling in the matching `windows`
// crate features would cost compile time without benefit — they're each
// called from one place and the bindings are tiny. SendInput is different:
// it lived in three copies across this file and the MCP binary, with
// hand-rolled INPUT structs that worked on x64 only by accident of padding.
// The windows-crate version is bundled with KEYBDINPUT/MOUSEINPUT/INPUT
// types that are correct on every supported architecture, so we use the
// crate version for SendInput specifically.

use log::info;
use std::ptr;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
};

extern "system" {
    fn OpenClipboard(hwnd: *mut std::ffi::c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> *mut std::ffi::c_void;
    fn SetClipboardData(format: u32, hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn GlobalAlloc(flags: u32, bytes: usize) -> *mut std::ffi::c_void;
    fn GlobalLock(hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn GlobalUnlock(hmem: *mut std::ffi::c_void) -> i32;
    fn GlobalSize(hmem: *mut std::ffi::c_void) -> usize;
    fn GlobalFree(hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn GetClipboardSequenceNumber() -> u32;
}

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;

pub fn read_clipboard_impl() -> Option<String> {
    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            CloseClipboard();
            return None;
        }
        // Cap the read at the allocated size reported by GlobalSize. Clipboard
        // data from other processes is untrusted — a missing null terminator
        // would otherwise let us walk off the end of the buffer.
        let max_u16 = {
            let bytes = GlobalSize(handle);
            if bytes == 0 {
                CloseClipboard();
                return None;
            }
            bytes / std::mem::size_of::<u16>()
        };
        let p = GlobalLock(handle) as *const u16;
        if p.is_null() {
            CloseClipboard();
            return None;
        }
        let mut len = 0usize;
        while len < max_u16 && *p.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(p, len);
        let text = String::from_utf16_lossy(slice);
        GlobalUnlock(handle);
        CloseClipboard();
        Some(text)
    }
}

pub fn write_clipboard_impl(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return;
        }
        EmptyClipboard();
        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if hmem.is_null() {
            CloseClipboard();
            return;
        }
        let dest = GlobalLock(hmem) as *mut u16;
        if dest.is_null() {
            GlobalFree(hmem);
            CloseClipboard();
            return;
        }
        ptr::copy_nonoverlapping(wide.as_ptr(), dest, wide.len());
        GlobalUnlock(hmem);
        // On success, the system owns hmem. On failure we must free it ourselves.
        if SetClipboardData(CF_UNICODETEXT, hmem).is_null() {
            GlobalFree(hmem);
        }
        CloseClipboard();
    }
}

/// Build an INPUT for a virtual-key keystroke. `vk` is a Windows VK_*
/// value; the scan code is looked up via MapVirtualKeyW so apps that
/// inspect both fields see consistent state.
fn make_key(vk: u16, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Build an INPUT for a scancode keystroke (used by Ctrl+C delivery so
/// the keystroke is delivered the same way a physical key would be —
/// some apps watch the scancode path and ignore VK-only events).
fn scan_input(scan: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: KEYEVENTF_SCANCODE
                    | if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Force-release all modifier keys (both generic AND left/right-specific
/// variants). When the user physically holds e.g. left-Alt, Windows tracks
/// both VK_MENU *and* VK_LMENU as pressed. Releasing only the generic VK
/// doesn't always clear the side-specific state, so apps that check
/// GetAsyncKeyState(VK_LMENU) still see Alt as held.
fn release_all_modifiers() {
    let releases = [
        make_key(0xA0, true), // VK_LSHIFT
        make_key(0xA1, true), // VK_RSHIFT
        make_key(0x10, true), // VK_SHIFT
        make_key(0xA2, true), // VK_LCONTROL
        make_key(0xA3, true), // VK_RCONTROL
        make_key(0x11, true), // VK_CONTROL
        make_key(0xA4, true), // VK_LMENU (Left Alt)
        make_key(0xA5, true), // VK_RMENU (Right Alt)
        make_key(0x12, true), // VK_MENU  (Alt generic)
        make_key(0x5B, true), // VK_LWIN
        make_key(0x5C, true), // VK_RWIN
    ];
    unsafe {
        SendInput(&releases, std::mem::size_of::<INPUT>() as i32);
    }
    std::thread::sleep(std::time::Duration::from_millis(30));
}

/// Save the current clipboard, release all modifier keys, and send Ctrl+C
/// to the foreground window. Returns (original_clipboard, clipboard_seq_before_copy).
///
/// This is the shared setup for both synchronous (`capture_selection_impl`)
/// and two-phase (`begin_selection_capture` / `finish_selection_capture`) capture.
fn send_copy_keystroke() -> (Option<String>, u32) {
    unsafe {
        let original_clipboard = read_clipboard_impl();
        let seq_before = GetClipboardSequenceNumber();

        release_all_modifiers();

        // Send clean Ctrl+C using scan codes
        let copy_keys = [
            scan_input(0x1D, false), // Ctrl down
            scan_input(0x2E, false), // C down
            scan_input(0x2E, true),  // C up
            scan_input(0x1D, true),  // Ctrl up
        ];
        let sent = SendInput(&copy_keys, std::mem::size_of::<INPUT>() as i32);
        if sent != 4 {
            info!("[selection] SendInput failed: returned {}", sent);
        }

        (original_clipboard, seq_before)
    }
}

#[allow(dead_code)]
pub fn capture_selection_impl() -> Option<String> {
    let (original_clipboard, seq_before) = send_copy_keystroke();

    // Poll for clipboard change
    let changed = wait_for_clipboard_change(seq_before, 300);

    if changed {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let new_text = read_clipboard_impl();
        // Restore original clipboard
        if let Some(ref orig) = original_clipboard {
            write_clipboard_impl(orig);
        } else {
            write_clipboard_impl("");
        }
        if let Some(ref text) = new_text {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                info!("[selection] Captured {} chars", trimmed.len());
                return Some(trimmed.to_string());
            }
        }
    }

    None
}

/// Carries state between begin/finish selection capture. The Windows
/// flavour uses the clipboard sequence number to detect that a copy
/// actually happened (so we don't return stale clipboard contents when
/// the user had nothing selected).
pub struct SelectionCaptureToken {
    original_clipboard: Option<String>,
    seq_before: u32,
}

/// Phase 1: Send Ctrl+C to the foreground window and snapshot the
/// clipboard state. Must be called while the source window is still
/// focused.
pub fn begin_selection_capture_impl() -> SelectionCaptureToken {
    let (original_clipboard, seq_before) = send_copy_keystroke();
    SelectionCaptureToken {
        original_clipboard,
        seq_before,
    }
}

/// Phase 2: Poll the clipboard for a change and return the captured text.
/// Can be called after the floating window is shown — the Ctrl+C was already sent.
pub fn finish_selection_capture_impl(token: SelectionCaptureToken) -> Option<String> {
    let SelectionCaptureToken {
        original_clipboard,
        seq_before,
    } = token;
    let changed = wait_for_clipboard_change(seq_before, 300);

    if changed {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let new_text = read_clipboard_impl();
        // Restore original clipboard
        if let Some(ref orig) = &original_clipboard {
            write_clipboard_impl(orig);
        } else {
            write_clipboard_impl("");
        }
        // The sequence number changed, so a copy happened — return the text
        // even if it matches the previous clipboard content (user may have
        // re-selected the same text).
        if let Some(ref text) = new_text {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                info!("[selection] Captured {} chars", trimmed.len());
                return Some(trimmed.to_string());
            }
        }
    }

    None
}

/// Poll for clipboard sequence number change with timeout (ms)
fn wait_for_clipboard_change(seq_before: u32, timeout_ms: u32) -> bool {
    let steps = (timeout_ms / 10).max(1);
    for _ in 0..steps {
        std::thread::sleep(std::time::Duration::from_millis(10));
        if unsafe { GetClipboardSequenceNumber() } != seq_before {
            return true;
        }
    }
    false
}

/// Simulate Ctrl+V paste keystroke to the foreground window.
pub fn simulate_paste_impl() {
    release_all_modifiers();

    // Ctrl down (scan 0x1D), V down (scan 0x2F), V up, Ctrl up
    let paste_keys = [
        scan_input(0x1D, false), // Ctrl down
        scan_input(0x2F, false), // V down
        scan_input(0x2F, true),  // V up
        scan_input(0x1D, true),  // Ctrl up
    ];
    unsafe {
        SendInput(&paste_keys, std::mem::size_of::<INPUT>() as i32);
    }
}
