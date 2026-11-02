//! CGEventTap-based hotkey capture on macOS.
//!
//! **The Settings / welcome hotkey picker does not use this on macOS.** It
//! records combos from DOM `keydown` events in its own (focused) webview —
//! see `ui/js/shared/hotkey-capture.js` and `begin_hotkey_recording` — which
//! needs no TCC permission at all. This tap is kept for the
//! `capture_hotkey_combo` command (cross-platform API surface), and is only
//! attempted when Kage is *already* trusted for Accessibility.
//!
//! The Windows implementation uses a separate helper process because
//! WebView2's raw-input registration disables WH_KEYBOARD_LL hooks in
//! the same process. macOS has no such constraint — a CGEventTap on a
//! dedicated thread coexists cleanly with Tauri's AppKit run loop on
//! the main thread.
//!
//! The tap runs in **listen-only** mode: events pass through to the
//! focused app unchanged.
//!
//! # Permissions — never prompt from here
//!
//! A keyboard CGEventTap needs Accessibility or Input Monitoring. This
//! module used to call `AXIsProcessTrustedWithOptions(prompt: true)` on
//! every capture, which is what produced the "approve Kage in System
//! Settings" dialog even for users who had approved it: TCC pins grants
//! for ad-hoc-signed builds to the exact binary hash, so after an update
//! the old grant still *shows* as enabled while the new binary is
//! untrusted. Prompting is now the job of the explicit Permissions pane
//! in Settings; capture only checks (`AXIsProcessTrusted`, no prompt) and
//! returns `None` straight away when untrusted, instead of opening a tap
//! that would silently see no events until the 10 s timeout.
//!
//! # Cancellation & timeout
//!
//! The capture thread owns a CFRunLoop; we store its raw pointer so
//! `cancel_capture_impl` can call `CFRunLoopStop` from another thread.
//! A short timer thread stops the run loop when the timeout expires.

use super::keycodes::{keycode_to_name, KEYCODE_ESCAPE};
use crate::os::hotkey::CapturedHotkey;
use accessibility_sys as ax;
use core_foundation::runloop::{CFRunLoop, CFRunLoopRef};
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField,
};
use log::{info, warn};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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

/// Bumped per capture. Each capture's detached timeout thread only acts
/// while the generation is still its own — otherwise a timer left over
/// from a capture that finished early (key pressed) would stop the *next*
/// capture's run loop and cut it short.
static GENERATION: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn capture_hotkey_impl(timeout_ms: u64) -> Option<CapturedHotkey> {
    // Check — never prompt. See the module docs for why prompting here was
    // wrong. Without trust the tap would be created but receive no key
    // events, so bail immediately rather than hanging for `timeout_ms`.
    if !unsafe { ax::AXIsProcessTrusted() } {
        warn_permission_once();
        return None;
    }

    // Refuse concurrent capture so two callers can't race on the shared
    // slot. The caller is expected to be serial anyway.
    if CAPTURING.swap(true, Ordering::SeqCst) {
        warn!("[HOTKEY_CAPTURE] capture_hotkey called while already capturing — ignored");
        return None;
    }
    *CAPTURED.lock().unwrap() = None;
    RUNLOOP_PTR.store(0, Ordering::SeqCst);
    // Bumped before the capture thread publishes its run loop, so a timer
    // that observes the new pointer also observes the new generation.
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    // The actual capture runs on a dedicated OS thread — it needs its
    // own CFRunLoop and mustn't block the Tauri main thread. We
    // `.join()` to get the captured result back synchronously.
    let handle = std::thread::Builder::new()
        .name("kage-hotkey-capture".to_string())
        .spawn(move || run_capture_thread(timeout_ms, generation))
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

fn run_capture_thread(timeout_ms: u64, generation: u64) -> Option<CapturedHotkey> {
    // Store the thread's run loop so cancel/timeout can stop it.
    let run_loop = CFRunLoop::get_current();
    // SAFETY: `CFRunLoop` is a `!Send` wrapper over a thread-safe CF pointer.
    // Reading the raw ref here and stashing it as `usize` is fine so long as
    // the only cross-thread op we do is `CFRunLoopStop`, which Apple
    // documents as thread-safe.
    let run_loop_ptr = unsafe { run_loop_raw(&run_loop) };
    RUNLOOP_PTR.store(run_loop_ptr as usize, Ordering::SeqCst);

    // A cancel that landed before the pointer was published had nothing
    // to stop; honour it now instead of entering a run loop that nothing
    // would ever stop.
    if !CAPTURING.load(Ordering::SeqCst) {
        RUNLOOP_PTR.store(0, Ordering::SeqCst);
        return None;
    }

    // Timeout thread — stops the run loop when time's up. Separate from
    // the caller-visible cancel path so a user who lets the timer run
    // out still gets `None` back cleanly.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(timeout_ms));
        // Pointer first, then generation — see the ordering note in
        // capture_hotkey_impl.
        let ptr = RUNLOOP_PTR.load(Ordering::SeqCst);
        if ptr == 0 || GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        // Still gated on CAPTURING: once a capture has completed (key
        // pressed / Escape / cancel) the capture thread is on its way out
        // and its CFRunLoop may be gone by the time we'd touch `ptr`.
        // Nothing is left to stop in those paths anyway — the tap callback
        // and `cancel_capture_impl` stop the loop themselves, and a stop
        // issued before the loop starts is honoured on entry.
        if !CAPTURING.swap(false, Ordering::SeqCst) {
            return;
        }
        info!("[HOTKEY_CAPTURE] Timeout after {}ms", timeout_ms);
        unsafe { cf_run_loop_stop(ptr as CFRunLoopRef) };
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
            "[HOTKEY_CAPTURE] Kage is not trusted for Accessibility (or the event tap failed), \
             so the CGEventTap capture path is unavailable. The Settings hotkey picker records \
             in-webview on macOS and does not need this; see Settings → Permissions to repair."
        );
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_any_modifier_detects_each_flag() {
        assert!(has_any_modifier(CGEventFlags::CGEventFlagCommand));
        assert!(has_any_modifier(CGEventFlags::CGEventFlagShift));
        assert!(has_any_modifier(CGEventFlags::CGEventFlagAlternate));
        assert!(has_any_modifier(CGEventFlags::CGEventFlagControl));
        assert!(!has_any_modifier(CGEventFlags::empty()));
    }
}
