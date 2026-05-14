//! CGEventTap-based hotkey capture on macOS.
//!
//! The Windows implementation uses a separate helper process because
//! WebView2's raw-input registration disables WH_KEYBOARD_LL hooks in
//! the same process. macOS has no such constraint — a CGEventTap on a
//! dedicated thread coexists cleanly with Tauri's AppKit run loop on
//! the main thread.
//!
//! The tap runs in **listen-only** mode: events pass through to the
//! focused app unchanged. That means a user's captured key combo also
//! lands in whatever window had focus when they pressed it — fine in
//! practice because they're looking at the Settings "Record Hotkey"
//! dialog, which is modal-ish and the combo is momentary. Consuming
//! the event would require a non-listen-only tap plus Accessibility
//! permission; we intentionally avoid that trade.
//!
//! # Permissions
//!
//! Passive CGEventTaps require **Input Monitoring** (macOS 10.15+).
//! Without it, `CGEventTapCreate` returns NULL and we log a warn_once
//! and return `None`. The user must grant the permission via
//! System Settings → Privacy & Security → Input Monitoring → Kage.
//!
//! # Cancellation & timeout
//!
//! The capture thread owns a CFRunLoop; we store its raw pointer so
//! `cancel_capture_impl` can call `CFRunLoopStop` from another thread.
//! A short timer thread stops the run loop when the timeout expires.

use crate::os::hotkey::CapturedHotkey;
use accessibility_sys as ax;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::runloop::{CFRunLoop, CFRunLoopRef};
use core_foundation::string::CFString;
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField,
};
use log::{info, warn};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Shared capture state
// ---------------------------------------------------------------------------
//
// A global single-slot capture — only one `capture_hotkey` call can be
// in flight at a time. The caller controls this at the UX layer (the
// Settings "Record Hotkey" button is modal), so we don't need a richer
// queueing model.

static CAPTURING: AtomicBool = AtomicBool::new(false);

/// Raw `CFRunLoopRef` of the capture thread, stored as `usize` so it
/// can cross thread boundaries (the `core_foundation::CFRunLoop` newtype
/// is `!Send`). Zero means "no thread running".
static RUNLOOP_PTR: AtomicUsize = AtomicUsize::new(0);

static CAPTURED: Mutex<Option<CapturedHotkey>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn capture_hotkey_impl(timeout_ms: u64) -> Option<CapturedHotkey> {
    // Prompt for Accessibility if not already granted. Accessibility subsumes
    // Input Monitoring for passive CGEventTaps — once granted, the tap works
    // without a separate Input Monitoring entry. The prompt also ensures the
    // app appears in the Accessibility list in System Settings.
    //
    // NOTE: This only works when running as a proper .app bundle (i.e.
    // `cargo tauri build`). During `cargo tauri dev` the bare binary won't
    // appear in System Settings TCC lists — this is a macOS limitation for
    // non-bundled executables.
    let ax_trusted = ensure_accessibility_for_capture();
    if !ax_trusted {
        info!(
            "[HOTKEY_CAPTURE] Accessibility not yet granted — event tap may fail. \
             Grant Accessibility in System Settings and restart the app."
        );
    }

    // Refuse concurrent capture so two callers can't race on the shared
    // slot. The caller is expected to be serial anyway.
    if CAPTURING.swap(true, Ordering::SeqCst) {
        warn!("[HOTKEY_CAPTURE] capture_hotkey called while already capturing — ignored");
        return None;
    }
    *CAPTURED.lock().unwrap() = None;
    RUNLOOP_PTR.store(0, Ordering::SeqCst);

    // The actual capture runs on a dedicated OS thread — it needs its
    // own CFRunLoop and mustn't block the Tauri main thread. We
    // `.join()` to get the captured result back synchronously.
    let handle = std::thread::Builder::new()
        .name("kage-hotkey-capture".to_string())
        .spawn(move || run_capture_thread(timeout_ms))
        .expect("failed to spawn hotkey capture thread");

    let captured = handle.join().unwrap_or(None);
    CAPTURING.store(false, Ordering::SeqCst);
    captured
}

pub fn cancel_capture_impl() {
    CAPTURING.store(false, Ordering::SeqCst);
    // Stop the capture thread's run loop — safe to call from any thread
    // per Apple's CF documentation. `swap(0)` both reads the pointer and
    // clears it so we don't double-stop if cancel is called twice.
    let ptr = RUNLOOP_PTR.swap(0, Ordering::SeqCst);
    if ptr != 0 {
        unsafe {
            cf_run_loop_stop(ptr as CFRunLoopRef);
        }
    }
}

// ---------------------------------------------------------------------------
// Capture thread
// ---------------------------------------------------------------------------

fn run_capture_thread(timeout_ms: u64) -> Option<CapturedHotkey> {
    // Store the thread's run loop so cancel/timeout can stop it.
    let run_loop = CFRunLoop::get_current();
    // SAFETY: `CFRunLoop` is a `!Send` wrapper over a thread-safe CF pointer.
    // Reading the raw ref here and stashing it as `usize` is fine so long as
    // the only cross-thread op we do is `CFRunLoopStop`, which Apple
    // documents as thread-safe.
    let run_loop_ptr = unsafe { run_loop_raw(&run_loop) };
    RUNLOOP_PTR.store(run_loop_ptr as usize, Ordering::SeqCst);

    // Timeout thread — stops the run loop when time's up. Separate from
    // the caller-visible cancel path so a user who lets the timer run
    // out still gets `None` back cleanly.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(timeout_ms));
        let ptr = RUNLOOP_PTR.load(Ordering::SeqCst);
        if ptr != 0 && CAPTURING.load(Ordering::SeqCst) {
            info!("[HOTKEY_CAPTURE] Timeout after {}ms", timeout_ms);
            CAPTURING.store(false, Ordering::SeqCst);
            unsafe { cf_run_loop_stop(ptr as CFRunLoopRef) };
        }
    });

    let result = CGEventTap::with_enabled(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::KeyDown, CGEventType::FlagsChanged],
        |_proxy, event_type, event| {
            handle_tap_event(event_type, event);
            CallbackResult::Keep
        },
        CFRunLoop::run_current,
    );

    RUNLOOP_PTR.store(0, Ordering::SeqCst);

    if result.is_err() {
        warn_permission_once();
        return None;
    }

    CAPTURED.lock().unwrap().take()
}

fn handle_tap_event(event_type: CGEventType, event: &core_graphics::event::CGEvent) {
    if !CAPTURING.load(Ordering::SeqCst) {
        return;
    }

    // FlagsChanged fires for modifier presses/releases — we don't capture
    // on those alone (otherwise pressing Cmd by itself would complete a
    // capture), but we DO snapshot the current flags for the next KeyDown.
    if matches!(event_type, CGEventType::FlagsChanged) {
        return;
    }

    if !matches!(event_type, CGEventType::KeyDown) {
        return;
    }

    let flags = event.get_flags();
    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;

    // Escape by itself cancels — consistent with the Settings UI's
    // "press Esc to cancel" affordance on Windows.
    if keycode == KEYCODE_ESCAPE && !has_any_modifier(flags) {
        info!("[HOTKEY_CAPTURE] Cancelled via Escape");
        CAPTURING.store(false, Ordering::SeqCst);
        let ptr = RUNLOOP_PTR.load(Ordering::SeqCst);
        if ptr != 0 {
            unsafe { cf_run_loop_stop(ptr as CFRunLoopRef) };
        }
        return;
    }

    let mut modifiers = Vec::new();
    if flags.contains(CGEventFlags::CGEventFlagControl) {
        modifiers.push("Ctrl".to_string());
    }
    if flags.contains(CGEventFlags::CGEventFlagAlternate) {
        modifiers.push("Alt".to_string()); // Option — same name as Win for shortcut reg
    }
    if flags.contains(CGEventFlags::CGEventFlagShift) {
        modifiers.push("Shift".to_string());
    }
    if flags.contains(CGEventFlags::CGEventFlagCommand) {
        modifiers.push("Super".to_string()); // Cmd — matches Windows' Super for cross-plat reg
    }

    let key = keycode_to_name(keycode);
    let display = if modifiers.is_empty() {
        key.clone()
    } else {
        format!("{}+{}", modifiers.join("+"), key)
    };

    info!("[HOTKEY_CAPTURE] Captured: {}", display);
    *CAPTURED.lock().unwrap() = Some(CapturedHotkey {
        modifiers,
        key,
        display,
    });

    // Stop the run loop — capture complete. with_enabled then returns Ok.
    CAPTURING.store(false, Ordering::SeqCst);
    let ptr = RUNLOOP_PTR.load(Ordering::SeqCst);
    if ptr != 0 {
        unsafe { cf_run_loop_stop(ptr as CFRunLoopRef) };
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn has_any_modifier(flags: CGEventFlags) -> bool {
    flags.contains(CGEventFlags::CGEventFlagControl)
        || flags.contains(CGEventFlags::CGEventFlagAlternate)
        || flags.contains(CGEventFlags::CGEventFlagShift)
        || flags.contains(CGEventFlags::CGEventFlagCommand)
}

fn warn_permission_once() {
    static WARNED: OnceLock<()> = OnceLock::new();
    WARNED.get_or_init(|| {
        log::warn!(
            "hotkey capture: CGEventTapCreate failed — Accessibility permission likely not granted. \
             Go to System Settings → Privacy & Security → Accessibility and enable Kage."
        );
    });
}

/// Check (and prompt for) Accessibility permission. Returns true if granted.
///
/// Accessibility permission subsumes Input Monitoring for passive CGEventTaps,
/// so granting Accessibility is sufficient — the user doesn't need to separately
/// find and enable the app in the Input Monitoring list.
///
/// The prompt option (`kAXTrustedCheckOptionPrompt = true`) causes macOS to show
/// the native "Kage would like to control this computer" dialog if permission
/// hasn't been granted yet, and adds the app to the Accessibility list in System
/// Settings. This works even for ad-hoc signed development builds.
fn ensure_accessibility_for_capture() -> bool {
    use core_foundation::base::TCFType;

    let dict: CFDictionary<CFString, CFBoolean> = CFDictionary::from_CFType_pairs(&[(
        unsafe { CFString::wrap_under_get_rule(ax::kAXTrustedCheckOptionPrompt) },
        CFBoolean::true_value(),
    )]);
    unsafe { ax::AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef()) }
}

/// Access the raw `CFRunLoopRef` inside the `CFRunLoop` newtype. The crate
/// doesn't expose `.as_concrete_TypeRef()` publicly in a `!Send`-safe way,
/// so we `mem::transmute` the newtype (single-pointer `repr(C)` over
/// `CFRunLoopRef`) to read the pointer. Safe because we only store it as
/// an integer and the cross-thread op is Apple-documented thread-safe.
unsafe fn run_loop_raw(rl: &CFRunLoop) -> CFRunLoopRef {
    use core_foundation::base::TCFType;
    rl.as_concrete_TypeRef()
}

unsafe fn cf_run_loop_stop(rl: CFRunLoopRef) {
    extern "C" {
        fn CFRunLoopStop(rl: CFRunLoopRef);
    }
    CFRunLoopStop(rl);
}

// ---------------------------------------------------------------------------
// Keycode → name mapping (kVK_* from HIToolbox/Events.h)
// ---------------------------------------------------------------------------

const KEYCODE_ESCAPE: u16 = 0x35;

fn keycode_to_name(code: u16) -> String {
    // Values from <HIToolbox/Events.h>. The layout groups non-contiguously
    // (letters aren't alphabetic in keycode order) so a flat match is the
    // clearest way to express it.
    match code {
        0x00 => "A".into(),
        0x01 => "S".into(),
        0x02 => "D".into(),
        0x03 => "F".into(),
        0x04 => "H".into(),
        0x05 => "G".into(),
        0x06 => "Z".into(),
        0x07 => "X".into(),
        0x08 => "C".into(),
        0x09 => "V".into(),
        0x0B => "B".into(),
        0x0C => "Q".into(),
        0x0D => "W".into(),
        0x0E => "E".into(),
        0x0F => "R".into(),
        0x10 => "Y".into(),
        0x11 => "T".into(),
        0x1F => "O".into(),
        0x20 => "U".into(),
        0x22 => "I".into(),
        0x23 => "P".into(),
        0x25 => "L".into(),
        0x26 => "J".into(),
        0x28 => "K".into(),
        0x2D => "N".into(),
        0x2E => "M".into(),

        // Digits (0x12-0x1D, non-alphabetic order too)
        0x12 => "1".into(),
        0x13 => "2".into(),
        0x14 => "3".into(),
        0x15 => "4".into(),
        0x16 => "6".into(),
        0x17 => "5".into(),
        0x19 => "9".into(),
        0x1A => "7".into(),
        0x1C => "8".into(),
        0x1D => "0".into(),

        // Editing / layout
        0x18 => "=".into(),
        0x1B => "-".into(),
        0x1E => "]".into(),
        0x21 => "[".into(),
        0x24 => "Enter".into(),
        0x27 => "'".into(),
        0x29 => ";".into(),
        0x2A => "\\".into(),
        0x2B => ",".into(),
        0x2C => "/".into(),
        0x2F => ".".into(),
        0x30 => "Tab".into(),
        0x31 => "Space".into(),
        0x32 => "`".into(),
        0x33 => "Backspace".into(),
        0x35 => "Escape".into(),

        // Arrows / navigation
        0x73 => "Home".into(),
        0x74 => "PageUp".into(),
        0x75 => "Delete".into(),
        0x77 => "End".into(),
        0x79 => "PageDown".into(),
        0x7B => "Left".into(),
        0x7C => "Right".into(),
        0x7D => "Down".into(),
        0x7E => "Up".into(),

        // Function keys (non-contiguous — painful but stable)
        0x7A => "F1".into(),
        0x78 => "F2".into(),
        0x63 => "F3".into(),
        0x76 => "F4".into(),
        0x60 => "F5".into(),
        0x61 => "F6".into(),
        0x62 => "F7".into(),
        0x64 => "F8".into(),
        0x65 => "F9".into(),
        0x6D => "F10".into(),
        0x67 => "F11".into(),
        0x6F => "F12".into(),

        // Numpad
        0x41 => "NumDecimal".into(),
        0x43 => "NumMultiply".into(),
        0x45 => "NumAdd".into(),
        0x4B => "NumDivide".into(),
        0x4E => "NumSubtract".into(),
        0x51 => "NumEquals".into(),
        0x52 => "Num0".into(),
        0x53 => "Num1".into(),
        0x54 => "Num2".into(),
        0x55 => "Num3".into(),
        0x56 => "Num4".into(),
        0x57 => "Num5".into(),
        0x58 => "Num6".into(),
        0x59 => "Num7".into(),
        0x5B => "Num8".into(),
        0x5C => "Num9".into(),

        // Unknown — surface the raw code so the user sees something meaningful
        // rather than a silent drop.
        other => format!("VK_{:02X}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keycode_letters_map_correctly() {
        assert_eq!(keycode_to_name(0x00), "A");
        assert_eq!(keycode_to_name(0x09), "V");
        assert_eq!(keycode_to_name(0x2E), "M");
    }

    #[test]
    fn keycode_digits_map_correctly() {
        assert_eq!(keycode_to_name(0x12), "1");
        assert_eq!(keycode_to_name(0x1D), "0");
    }

    #[test]
    fn keycode_arrows_map_correctly() {
        assert_eq!(keycode_to_name(0x7B), "Left");
        assert_eq!(keycode_to_name(0x7E), "Up");
    }

    #[test]
    fn keycode_function_keys_map_correctly() {
        assert_eq!(keycode_to_name(0x7A), "F1");
        assert_eq!(keycode_to_name(0x6F), "F12");
    }

    #[test]
    fn unknown_keycode_falls_back_to_vk_hex() {
        // 0xFF isn't a real kVK_* — the fallback should expose it verbatim
        // so the user sees something rather than a silent drop.
        assert_eq!(keycode_to_name(0xFF), "VK_FF");
    }

    #[test]
    fn has_any_modifier_detects_each_flag() {
        assert!(has_any_modifier(CGEventFlags::CGEventFlagCommand));
        assert!(has_any_modifier(CGEventFlags::CGEventFlagShift));
        assert!(has_any_modifier(CGEventFlags::CGEventFlagAlternate));
        assert!(has_any_modifier(CGEventFlags::CGEventFlagControl));
        assert!(!has_any_modifier(CGEventFlags::empty()));
    }
}
