// Windows clipboard operations using Win32 API

use log::info;
use std::ptr;

extern "system" {
    fn OpenClipboard(hwnd: *mut std::ffi::c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> *mut std::ffi::c_void;
    fn SetClipboardData(format: u32, hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn GlobalAlloc(flags: u32, bytes: usize) -> *mut std::ffi::c_void;
    fn GlobalLock(hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn GlobalUnlock(hmem: *mut std::ffi::c_void) -> i32;
    fn GetClipboardSequenceNumber() -> u32;
    fn SendInput(count: u32, inputs: *const WinInput, size: i32) -> u32;
    fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
}

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;
const MAPVK_VK_TO_VSC: u32 = 0;
const INPUT_KEYBOARD: u32 = 1;
const KEYEVENTF_KEYUP: u32 = 0x0002;
const KEYEVENTF_SCANCODE: u32 = 0x0008;

#[repr(C)]
struct KbdInput { vk: u16, scan: u16, flags: u32, time: u32, extra: usize }

#[repr(C)]
struct WinInput { input_type: u32, ki: KbdInput, _pad: [u8; 8] }

pub fn read_clipboard_impl() -> Option<String> {
    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 { return None; }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() { CloseClipboard(); return None; }
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() { CloseClipboard(); return None; }
        let mut len = 0;
        while *ptr.add(len) != 0 { len += 1; }
        let slice = std::slice::from_raw_parts(ptr, len);
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
        if OpenClipboard(ptr::null_mut()) == 0 { return; }
        EmptyClipboard();
        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if !hmem.is_null() {
            let dest = GlobalLock(hmem) as *mut u16;
            if !dest.is_null() {
                ptr::copy_nonoverlapping(wide.as_ptr(), dest, wide.len());
                GlobalUnlock(hmem);
                SetClipboardData(CF_UNICODETEXT, hmem);
            }
        }
        CloseClipboard();
    }
}

/// Save the current clipboard, release all modifier keys, and send Ctrl+C
/// to the foreground window. Returns (original_clipboard, clipboard_seq_before_copy).
///
/// This is the shared setup for both synchronous (`capture_selection_impl`)
/// and two-phase (`begin_selection_capture` / `finish_selection_capture`) capture.
fn send_copy_keystroke() -> (Option<String>, u32) {
    fn make_key(vk: u16, up: bool) -> WinInput {
        let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
        WinInput {
            input_type: INPUT_KEYBOARD,
            ki: KbdInput { vk, scan, flags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, extra: 0 },
            _pad: [0u8; 8],
        }
    }
    fn scan_input(scan: u16, up: bool) -> WinInput {
        WinInput {
            input_type: INPUT_KEYBOARD,
            ki: KbdInput {
                vk: 0,
                scan,
                flags: KEYEVENTF_SCANCODE | if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                extra: 0,
            },
            _pad: [0u8; 8],
        }
    }
    let size = std::mem::size_of::<WinInput>() as i32;

    unsafe {
        let original_clipboard = read_clipboard_impl();
        let seq_before = GetClipboardSequenceNumber();

        // Force-release all modifiers
        let releases = [
            make_key(0x10, true), // VK_SHIFT
            make_key(0x11, true), // VK_CONTROL
            make_key(0x12, true), // VK_MENU (Alt)
            make_key(0x5B, true), // VK_LWIN
            make_key(0x5C, true), // VK_RWIN
        ];
        SendInput(releases.len() as u32, releases.as_ptr(), size);
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Send clean Ctrl+C using scan codes
        let copy_keys = [
            scan_input(0x1D, false), // Ctrl down
            scan_input(0x2E, false), // C down
            scan_input(0x2E, true),  // C up
            scan_input(0x1D, true),  // Ctrl up
        ];
        let sent = SendInput(4, copy_keys.as_ptr(), size);
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
            if !trimmed.is_empty() && new_text != original_clipboard {
                info!("[selection] Captured {} chars", trimmed.len());
                return Some(trimmed.to_string());
            }
        }
    }

    None
}

/// Phase 1: Send Ctrl+C to the foreground window and return the clipboard
/// sequence number from before the copy. This must be called while the source
/// window is still focused. Returns (original_clipboard, seq_before).
pub fn begin_selection_capture() -> (Option<String>, u32) {
    send_copy_keystroke()
}

/// Phase 2: Poll the clipboard for a change and return the captured text.
/// Can be called after the floating window is shown — the Ctrl+C was already sent.
pub fn finish_selection_capture(original_clipboard: Option<String>, seq_before: u32) -> Option<String> {
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
        if let Some(ref text) = new_text {
            let trimmed = text.trim();
            if !trimmed.is_empty() && new_text != original_clipboard {
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
