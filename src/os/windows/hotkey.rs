//! Low-level keyboard hook for capturing hotkey combinations on Windows.
//!
//! Uses WH_KEYBOARD_LL to intercept key events at the OS level,
//! allowing capture of system-reserved combos like Alt+Space, Win+key, etc.

use crate::os::hotkey::CapturedHotkey;
use log::info;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

// Win32 FFI
extern "system" {
    fn SetWindowsHookExW(id_hook: i32, lpfn: KeyboardProc, hmod: usize, thread_id: u32) -> usize;
    fn UnhookWindowsHookEx(hhk: usize) -> i32;
    fn CallNextHookEx(hhk: usize, code: i32, wparam: usize, lparam: *const KbdLlHookStruct) -> isize;
    fn GetMessageW(msg: *mut Msg, hwnd: usize, filter_min: u32, filter_max: u32) -> i32;
    fn PeekMessageW(msg: *mut Msg, hwnd: usize, filter_min: u32, filter_max: u32, remove: u32) -> i32;
    fn PostThreadMessageW(thread_id: u32, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn GetCurrentThreadId() -> u32;
}

type KeyboardProc = extern "system" fn(i32, usize, *const KbdLlHookStruct) -> isize;

const WH_KEYBOARD_LL: i32 = 13;
const WM_KEYDOWN: usize = 0x0100;
const WM_SYSKEYDOWN: usize = 0x0104;
const WM_QUIT: u32 = 0x0012;
const HC_ACTION: i32 = 0;

#[repr(C)]
struct KbdLlHookStruct {
    vk_code: u32,
    scan_code: u32,
    flags: u32,
    time: u32,
    extra_info: usize,
}

#[repr(C)]
struct Msg {
    hwnd: usize,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt_x: i32,
    pt_y: i32,
}

// Virtual key codes
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;  // Left Alt
const VK_RMENU: u32 = 0xA5;  // Right Alt
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;   // Alt

// Global state for the hook callback
static CAPTURING: AtomicBool = AtomicBool::new(false);
static HOOK_HANDLE: Mutex<usize> = Mutex::new(0);
static HOOK_THREAD_ID: Mutex<u32> = Mutex::new(0);
static CAPTURED: Mutex<Option<CapturedHotkey>> = Mutex::new(None);

// Track which modifiers are currently held
static MOD_CTRL: AtomicBool = AtomicBool::new(false);
static MOD_ALT: AtomicBool = AtomicBool::new(false);
static MOD_SHIFT: AtomicBool = AtomicBool::new(false);
static MOD_WIN: AtomicBool = AtomicBool::new(false);

fn is_modifier(vk: u32) -> bool {
    matches!(vk,
        VK_LSHIFT | VK_RSHIFT | VK_SHIFT |
        VK_LCONTROL | VK_RCONTROL | VK_CONTROL |
        VK_LMENU | VK_RMENU | VK_MENU |
        VK_LWIN | VK_RWIN
    )
}

fn update_modifier_state(vk: u32, pressed: bool) {
    match vk {
        VK_LSHIFT | VK_RSHIFT | VK_SHIFT => MOD_SHIFT.store(pressed, Ordering::SeqCst),
        VK_LCONTROL | VK_RCONTROL | VK_CONTROL => MOD_CTRL.store(pressed, Ordering::SeqCst),
        VK_LMENU | VK_RMENU | VK_MENU => MOD_ALT.store(pressed, Ordering::SeqCst),
        VK_LWIN | VK_RWIN => MOD_WIN.store(pressed, Ordering::SeqCst),
        _ => {}
    }
}

fn vk_to_key_name(vk: u32) -> String {
    match vk {
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x1B => "Escape".into(),
        0x20 => "Space".into(),
        0x21 => "PageUp".into(),
        0x22 => "PageDown".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        0x30..=0x39 => format!("{}", (vk - 0x30)),  // 0-9
        0x41..=0x5A => format!("{}", (vk as u8 as char)),  // A-Z
        0x60..=0x69 => format!("Num{}", vk - 0x60),  // Numpad 0-9
        0x6A => "NumMultiply".into(),
        0x6B => "NumAdd".into(),
        0x6D => "NumSubtract".into(),
        0x6E => "NumDecimal".into(),
        0x6F => "NumDivide".into(),
        0x70..=0x7B => format!("F{}", vk - 0x6F),  // F1-F12
        0xBA => ";".into(),
        0xBB => "=".into(),
        0xBC => ",".into(),
        0xBD => "-".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xC0 => "`".into(),
        0xDB => "[".into(),
        0xDC => "\\".into(),
        0xDD => "]".into(),
        0xDE => "'".into(),
        _ => format!("VK_{:02X}", vk),
    }
}

extern "system" fn keyboard_hook_proc(code: i32, wparam: usize, lparam: *const KbdLlHookStruct) -> isize {
    if code == HC_ACTION && !lparam.is_null() && CAPTURING.load(Ordering::SeqCst) {
        let kb = unsafe { &*lparam };
        let vk = kb.vk_code;
        let is_keydown = wparam == WM_KEYDOWN || wparam == WM_SYSKEYDOWN;
        let is_keyup = !is_keydown;

        info!("[HOTKEY_CAPTURE] Hook event: vk=0x{:02X} down={}", vk, is_keydown);

        if is_modifier(vk) {
            update_modifier_state(vk, is_keydown);
            // Swallow modifier keys so they don't trigger system actions
            return 1;
        }

        if is_keydown {
            // Non-modifier key pressed — capture the combo
            let mut modifiers = Vec::new();
            if MOD_CTRL.load(Ordering::SeqCst) { modifiers.push("Ctrl".to_string()); }
            if MOD_ALT.load(Ordering::SeqCst) { modifiers.push("Alt".to_string()); }
            if MOD_SHIFT.load(Ordering::SeqCst) { modifiers.push("Shift".to_string()); }
            if MOD_WIN.load(Ordering::SeqCst) { modifiers.push("Super".to_string()); }

            let key = vk_to_key_name(vk);
            let display = if modifiers.is_empty() {
                key.clone()
            } else {
                format!("{}+{}", modifiers.join("+"), key)
            };

            info!("[HOTKEY_CAPTURE] Captured: {}", display);

            if let Ok(mut captured) = CAPTURED.lock() {
                *captured = Some(CapturedHotkey {
                    modifiers,
                    key,
                    display,
                });
            }

            // Stop capturing
            CAPTURING.store(false, Ordering::SeqCst);

            // Post WM_QUIT to break the message loop
            if let Ok(tid) = HOOK_THREAD_ID.lock() {
                if *tid != 0 {
                    unsafe { PostThreadMessageW(*tid, WM_QUIT, 0, 0); }
                }
            }

            // Swallow the key so it doesn't trigger system actions
            return 1;
        }

        if is_keyup {
            // Swallow key-up too while capturing
            return 1;
        }
    }

    unsafe { CallNextHookEx(0, code, wparam, lparam) }
}

/// Start capturing a hotkey combination.
/// Spawns a helper process to work around WebView2's raw input registration
/// which disables WH_KEYBOARD_LL hooks in the same process.
pub fn capture_hotkey(timeout_ms: u64) -> Option<CapturedHotkey> {
    // Reset state
    MOD_CTRL.store(false, Ordering::SeqCst);
    MOD_ALT.store(false, Ordering::SeqCst);
    MOD_SHIFT.store(false, Ordering::SeqCst);
    MOD_WIN.store(false, Ordering::SeqCst);
    if let Ok(mut c) = CAPTURED.lock() { *c = None; }
    CAPTURING.store(true, Ordering::SeqCst);

    // Run the hook on a dedicated OS thread with its own message pump
    let handle = std::thread::spawn(move || {
        // Create the message queue
        let mut msg = Msg {
            hwnd: 0, message: 0, wparam: 0, lparam: 0, time: 0, pt_x: 0, pt_y: 0,
        };
        unsafe { PeekMessageW(&mut msg, 0, 0, 0, 0); }

        let tid = unsafe { GetCurrentThreadId() };
        if let Ok(mut t) = HOOK_THREAD_ID.lock() { *t = tid; }

        let hook = unsafe {
            SetWindowsHookExW(WH_KEYBOARD_LL, keyboard_hook_proc, 0, 0)
        };
        if hook == 0 {
            log::error!("[HOTKEY_CAPTURE] Failed to install keyboard hook");
            CAPTURING.store(false, Ordering::SeqCst);
            return;
        }

        if let Ok(mut h) = HOOK_HANDLE.lock() { *h = hook; }
        info!("[HOTKEY_CAPTURE] Hook installed on thread {}", tid);

        // Timeout thread
        let timeout_tid = tid;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(timeout_ms));
            if CAPTURING.load(Ordering::SeqCst) {
                info!("[HOTKEY_CAPTURE] Timeout");
                CAPTURING.store(false, Ordering::SeqCst);
                unsafe { PostThreadMessageW(timeout_tid, WM_QUIT, 0, 0); }
            }
        });

        // Message loop
        loop {
            let ret = unsafe { GetMessageW(&mut msg, 0, 0, 0) };
            if ret <= 0 { break; }
        }

        unsafe { UnhookWindowsHookEx(hook); }
        if let Ok(mut h) = HOOK_HANDLE.lock() { *h = 0; }
        if let Ok(mut t) = HOOK_THREAD_ID.lock() { *t = 0; }
    });

    let _ = handle.join();

    MOD_CTRL.store(false, Ordering::SeqCst);
    MOD_ALT.store(false, Ordering::SeqCst);
    MOD_SHIFT.store(false, Ordering::SeqCst);
    MOD_WIN.store(false, Ordering::SeqCst);

    let result = CAPTURED.lock().ok().and_then(|c| c.clone());
    info!("[HOTKEY_CAPTURE] Result: {:?}", result);
    result
}

/// Capture hotkey using a helper child process to work around WebView2's
/// raw input registration which blocks WH_KEYBOARD_LL in the same process.
///
/// Named `_impl` to match the cross-platform Pattern A dispatch shape
/// (`crate::os::platform::hotkey::capture_hotkey_impl`).
pub fn capture_hotkey_impl(timeout_ms: u64) -> Option<CapturedHotkey> {
    use std::process::{Command, Stdio};
    use std::io::Read;
    use std::os::windows::process::CommandExt;

    let exe = std::env::current_exe().ok()?;
    info!("[HOTKEY_CAPTURE] Spawning helper: {:?} /capture-hotkey {}", exe, timeout_ms);

    let mut child = Command::new(&exe)
        .args(["/capture-hotkey", &timeout_ms.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .spawn()
        .ok()?;

    let mut output = String::new();
    if let Some(ref mut stdout) = child.stdout {
        let _ = stdout.read_to_string(&mut output);
    }
    let _ = child.wait();

    let output = output.trim();
    info!("[HOTKEY_CAPTURE] Helper output: {}", output);

    if output.is_empty() || output == "null" {
        return None;
    }

    // Parse "Ctrl+Alt+Space" format
    let parts: Vec<&str> = output.split('+').collect();
    if parts.is_empty() { return None; }

    let key = parts.last()?.to_string();
    let modifiers: Vec<String> = parts[..parts.len()-1].iter().map(|s| s.to_string()).collect();

    Some(CapturedHotkey {
        display: output.to_string(),
        modifiers,
        key,
    })
}

/// Entry point when run as a hotkey capture helper process.
/// Installs a WH_KEYBOARD_LL hook, captures one combo, prints it to stdout, and exits.
pub fn run_capture_helper(timeout_ms: u64) {
    let result = capture_hotkey(timeout_ms);
    match result {
        Some(hk) => print!("{}", hk.display),
        None => print!("null"),
    }
}

/// Cancel an in-progress capture.
///
/// Named `_impl` to match the cross-platform Pattern A dispatch shape.
pub fn cancel_capture_impl() {
    if CAPTURING.load(Ordering::SeqCst) {
        CAPTURING.store(false, Ordering::SeqCst);
        if let Ok(tid) = HOOK_THREAD_ID.lock() {
            if *tid != 0 {
                unsafe { PostThreadMessageW(*tid, WM_QUIT, 0, 0); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Pure-helper coverage for the Windows hotkey capture module.
    //! The low-level Win32 hook itself can't be unit-tested (it runs
    //! inside a message pump), but the VK mapping and modifier
    //! classification are deterministic and worth locking in.

    use super::*;

    #[test]
    fn vk_to_key_name_letters() {
        assert_eq!(vk_to_key_name(0x41), "A");
        assert_eq!(vk_to_key_name(0x5A), "Z");
        // Middle of the range
        assert_eq!(vk_to_key_name(0x4B), "K");
    }

    #[test]
    fn vk_to_key_name_digits() {
        assert_eq!(vk_to_key_name(0x30), "0");
        assert_eq!(vk_to_key_name(0x35), "5");
        assert_eq!(vk_to_key_name(0x39), "9");
    }

    #[test]
    fn vk_to_key_name_function_keys() {
        assert_eq!(vk_to_key_name(0x70), "F1");
        assert_eq!(vk_to_key_name(0x74), "F5");
        assert_eq!(vk_to_key_name(0x7B), "F12");
    }

    #[test]
    fn vk_to_key_name_navigation_cluster() {
        assert_eq!(vk_to_key_name(0x25), "Left");
        assert_eq!(vk_to_key_name(0x26), "Up");
        assert_eq!(vk_to_key_name(0x27), "Right");
        assert_eq!(vk_to_key_name(0x28), "Down");
        assert_eq!(vk_to_key_name(0x24), "Home");
        assert_eq!(vk_to_key_name(0x23), "End");
        assert_eq!(vk_to_key_name(0x21), "PageUp");
        assert_eq!(vk_to_key_name(0x22), "PageDown");
    }

    #[test]
    fn vk_to_key_name_numpad_distinct_from_digits() {
        // Numpad keys prefixed with "Num" so config can distinguish them
        // from the main-row digits. Critical for users who bind hotkeys
        // to numpad-specific keys.
        assert_eq!(vk_to_key_name(0x60), "Num0");
        assert_eq!(vk_to_key_name(0x65), "Num5");
        assert_eq!(vk_to_key_name(0x69), "Num9");
        assert_eq!(vk_to_key_name(0x6A), "NumMultiply");
        assert_eq!(vk_to_key_name(0x6B), "NumAdd");
        assert_eq!(vk_to_key_name(0x6D), "NumSubtract");
        assert_eq!(vk_to_key_name(0x6E), "NumDecimal");
        assert_eq!(vk_to_key_name(0x6F), "NumDivide");
    }

    #[test]
    fn vk_to_key_name_punctuation() {
        assert_eq!(vk_to_key_name(0xBA), ";");
        assert_eq!(vk_to_key_name(0xBB), "=");
        assert_eq!(vk_to_key_name(0xBC), ",");
        assert_eq!(vk_to_key_name(0xBD), "-");
        assert_eq!(vk_to_key_name(0xBE), ".");
        assert_eq!(vk_to_key_name(0xBF), "/");
        assert_eq!(vk_to_key_name(0xC0), "`");
        assert_eq!(vk_to_key_name(0xDB), "[");
        assert_eq!(vk_to_key_name(0xDC), "\\");
        assert_eq!(vk_to_key_name(0xDD), "]");
        assert_eq!(vk_to_key_name(0xDE), "'");
    }

    #[test]
    fn vk_to_key_name_special_keys() {
        assert_eq!(vk_to_key_name(0x08), "Backspace");
        assert_eq!(vk_to_key_name(0x09), "Tab");
        assert_eq!(vk_to_key_name(0x0D), "Enter");
        assert_eq!(vk_to_key_name(0x1B), "Escape");
        assert_eq!(vk_to_key_name(0x20), "Space");
        assert_eq!(vk_to_key_name(0x2D), "Insert");
        assert_eq!(vk_to_key_name(0x2E), "Delete");
    }

    #[test]
    fn vk_to_key_name_unknown_falls_back_to_hex() {
        // Anything not in the lookup table should still yield a unique
        // string (VK_HH) so an unmapped key doesn't silently merge with
        // another one at the config layer.
        assert_eq!(vk_to_key_name(0xFF), "VK_FF");
        assert_eq!(vk_to_key_name(0x01), "VK_01");
        assert_eq!(vk_to_key_name(0x0A), "VK_0A");
    }

    #[test]
    fn is_modifier_covers_all_sides() {
        // Generic plus left/right variants for shift, ctrl, alt, win.
        for vk in [
            VK_LSHIFT, VK_RSHIFT, VK_SHIFT,
            VK_LCONTROL, VK_RCONTROL, VK_CONTROL,
            VK_LMENU, VK_RMENU, VK_MENU,
            VK_LWIN, VK_RWIN,
        ] {
            assert!(is_modifier(vk), "expected {:#X} to be a modifier", vk);
        }
    }

    #[test]
    fn is_modifier_rejects_non_modifiers() {
        // Regular letters, digits, arrows, function keys are NOT modifiers.
        assert!(!is_modifier(0x41)); // A
        assert!(!is_modifier(0x30)); // 0
        assert!(!is_modifier(0x70)); // F1
        assert!(!is_modifier(0x20)); // Space
        assert!(!is_modifier(0x25)); // Left arrow
    }
}
