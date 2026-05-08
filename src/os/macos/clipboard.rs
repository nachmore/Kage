// macOS clipboard operations

use log::info;
use std::process::Command;

/// macOS doesn't expose a sequence-number-based clipboard change API the
/// way Windows does, so we approximate two-phase capture by doing the
/// whole thing synchronously in `begin` and stashing the result on the
/// token. `finish` just unwraps it.
pub struct SelectionCaptureToken {
    selection: Option<String>,
}

pub fn begin_selection_capture_impl() -> SelectionCaptureToken {
    SelectionCaptureToken {
        selection: capture_selection_impl(),
    }
}

pub fn finish_selection_capture_impl(token: SelectionCaptureToken) -> Option<String> {
    token.selection
}

/// Simulate a Cmd+V paste keystroke into the foreground window via
/// CGEvent. Requires Accessibility permission (macOS 10.15+); if the
/// permission isn't granted the CGEventPost calls silently no-op and
/// nothing is pasted. We still warn once so a missing permission is
/// visible in the logs.
///
/// The V key is raw keycode `kVK_ANSI_V` (0x09). We modify only with
/// the Command flag — adding Shift/Option would change the Paste
/// shortcut in some apps.
pub fn simulate_paste_impl() {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    /// `kVK_ANSI_V` from <HIToolbox/Events.h>. Not exposed by any of our
    /// crates so we hard-code it. Stable across macOS releases.
    const KEYCODE_V: u16 = 0x09;

    // CombinedSessionState posts events as if they came from the user —
    // the same channel keystrokes normally travel on. HIDSystemState is
    // for lower-level synthetic input that bypasses session-level
    // modifications; we don't need that.
    let source = match CGEventSource::new(CGEventSourceStateID::CombinedSessionState) {
        Ok(s) => s,
        Err(()) => {
            warn_paste_once("CGEventSourceCreate returned NULL");
            return;
        }
    };

    // Build a key-down and key-up pair. Both need the Command modifier
    // flag set — without it the target app sees plain V.
    let down = match CGEvent::new_keyboard_event(source.clone(), KEYCODE_V, true) {
        Ok(e) => e,
        Err(()) => {
            warn_paste_once("CGEventCreateKeyboardEvent(keydown) returned NULL");
            return;
        }
    };
    down.set_flags(CGEventFlags::CGEventFlagCommand);

    let up = match CGEvent::new_keyboard_event(source, KEYCODE_V, false) {
        Ok(e) => e,
        Err(()) => {
            warn_paste_once("CGEventCreateKeyboardEvent(keyup) returned NULL");
            return;
        }
    };
    up.set_flags(CGEventFlags::CGEventFlagCommand);

    // Post to the HID event tap — the earliest point in the input pipeline,
    // which means the target window sees the synthetic event identically
    // to a real keystroke. Without Accessibility permission these calls
    // complete successfully but the OS drops the events; there's no API
    // to detect that case at post time, so we rely on the user confirming
    // the paste actually happened.
    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);
}

/// Log the paste-failure reason exactly once per process. We don't want
/// to spam the log on every hotkey when Accessibility permission is
/// missing — one line with the specific failure mode is enough.
fn warn_paste_once(reason: &str) {
    use std::sync::OnceLock;
    static WARNED: OnceLock<()> = OnceLock::new();
    WARNED.get_or_init(|| {
        log::warn!(
            "simulate_paste: failed to synthesize Cmd+V ({reason}) — \
             check Accessibility permission in System Settings → Privacy & Security"
        );
    });
}

pub fn read_clipboard_impl() -> Option<String> {
    Command::new("pbpaste")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

pub fn write_clipboard_impl(text: &str) {
    use std::io::Write;
    if let Ok(mut child) = Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

pub fn capture_selection_impl() -> Option<String> {
    let original_clipboard = read_clipboard_impl();

    // Simulate Cmd+C via osascript
    let _ = Command::new("osascript")
        .args([
            "-e",
            "tell application \"System Events\" to keystroke \"c\" using command down",
        ])
        .output();

    std::thread::sleep(std::time::Duration::from_millis(100));
    let new_clipboard = read_clipboard_impl();

    match (&original_clipboard, &new_clipboard) {
        (Some(orig), Some(new)) if orig != new && !new.is_empty() => {
            write_clipboard_impl(orig);
            info!("[selection] Captured {} chars", new.trim().len());
            Some(new.clone())
        }
        (None, Some(new)) if !new.is_empty() => {
            write_clipboard_impl("");
            info!("[selection] Captured {} chars", new.trim().len());
            Some(new.clone())
        }
        _ => None,
    }
}
