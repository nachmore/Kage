// macOS clipboard operations

use log::info;
use std::process::Command;

/// `kVK_ANSI_C` / `kVK_ANSI_V` from <HIToolbox/Events.h>. Not exposed by
/// any of our crates so we hard-code them. Stable across macOS releases.
const KEYCODE_C: u16 = 0x08;
const KEYCODE_V: u16 = 0x09;

/// How long `finish` waits for the target app to answer the synthetic
/// Cmd+C before concluding nothing was selected. Matches Windows.
const COPY_TIMEOUT_MS: u64 = 300;

/// Two-phase capture, mirroring the Windows design: `begin` snapshots the
/// clipboard + `NSPasteboard.changeCount` and posts Cmd+C (no process
/// spawns, ~1ms, so it's safe on the hotkey callback thread); `finish`
/// polls changeCount so it returns as soon as the copy lands instead of
/// sleeping blind.
pub struct SelectionCaptureToken {
    original_clipboard: Option<String>,
    change_count_before: Option<isize>,
}

pub fn begin_selection_capture_impl() -> SelectionCaptureToken {
    let original_clipboard = read_clipboard_impl();
    let change_count_before = pasteboard::change_count();
    if let Err(reason) = post_command_keystroke(KEYCODE_C) {
        info!("[selection] Cmd+C synthesis failed: {reason}");
    }
    SelectionCaptureToken {
        original_clipboard,
        change_count_before,
    }
}

pub fn finish_selection_capture_impl(token: SelectionCaptureToken) -> Option<String> {
    let SelectionCaptureToken {
        original_clipboard,
        change_count_before,
    } = token;
    // No changeCount means NSPasteboard was unreachable — we can't tell a
    // fresh copy from stale contents, so don't guess.
    let before = change_count_before?;
    if !wait_for_pasteboard_change(before, COPY_TIMEOUT_MS) {
        return None;
    }
    // changeCount bumps on clearContents, before the app has finished
    // writing its types — give it a beat, like the Windows path does.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let new_text = read_clipboard_impl();
    // Only text can be put back through this API. A non-text original
    // (None — e.g. a copied image) is left alone rather than clobbered
    // with an empty string; the captured selection stays in its place.
    if let Some(orig) = &original_clipboard {
        write_clipboard_impl(orig);
    }
    // changeCount moved, so a copy happened — return the text even when it
    // equals the previous clipboard (user re-selected the same text).
    let captured = non_empty_trimmed(new_text)?;
    info!("[selection] Captured {} chars", captured.len());
    Some(captured)
}

fn non_empty_trimmed(text: Option<String>) -> Option<String> {
    let text = text?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn wait_for_pasteboard_change(before: isize, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
        if pasteboard::change_count().is_some_and(|c| c != before) {
            return true;
        }
    }
    false
}

/// Thin NSPasteboard access via the raw objc runtime (the objc2-app-kit
/// `NSPasteboard` feature isn't enabled). Reading through the pasteboard
/// rather than `pbpaste` gives us `None` for non-text contents (pbpaste
/// prints "" for an image, which the capture path would then "restore"
/// over the image) and is locale-independent.
mod pasteboard {
    use objc2::msg_send;
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSString;

    fn general() -> Option<Retained<AnyObject>> {
        let cls = AnyClass::get(c"NSPasteboard")?;
        // SAFETY: +[NSPasteboard generalPasteboard] takes no arguments and
        // returns an NSPasteboard instance.
        unsafe { msg_send![cls, generalPasteboard] }
    }

    pub fn change_count() -> Option<isize> {
        autoreleasepool(|_| {
            let pb = general()?;
            // SAFETY: -changeCount returns NSInteger.
            let count: isize = unsafe { msg_send![&*pb, changeCount] };
            Some(count)
        })
    }

    pub fn read_text() -> Option<String> {
        autoreleasepool(|_| {
            let pb = general()?;
            let ty = NSString::from_str("public.utf8-plain-text");
            // SAFETY: -stringForType: takes an NSPasteboardType (NSString)
            // and returns a nullable NSString.
            let text: Option<Retained<NSString>> = unsafe { msg_send![&*pb, stringForType: &*ty] };
            text.map(|s| s.to_string())
        })
    }
}

/// Simulate a Cmd+V paste keystroke into the foreground window via
/// CGEvent. Requires Accessibility permission (macOS 10.15+); if the
/// permission isn't granted the CGEventPost calls silently no-op and
/// nothing is pasted. We still warn once so a missing permission is
/// visible in the logs.
pub fn simulate_paste_impl() {
    if let Err(reason) = post_command_keystroke(KEYCODE_V) {
        warn_paste_once(reason);
    }
}

/// Post a Cmd+<key> down/up pair via CGEvent. We modify only with the
/// Command flag — set explicitly so modifiers still physically held from
/// the hotkey (e.g. Shift) don't turn it into a different shortcut.
fn post_command_keystroke(keycode: u16) -> Result<(), &'static str> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // CombinedSessionState posts events as if they came from the user —
    // the same channel keystrokes normally travel on. HIDSystemState is
    // for lower-level synthetic input that bypasses session-level
    // modifications; we don't need that.
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|()| "CGEventSourceCreate returned NULL")?;

    let down = CGEvent::new_keyboard_event(source.clone(), keycode, true)
        .map_err(|()| "CGEventCreateKeyboardEvent(keydown) returned NULL")?;
    down.set_flags(CGEventFlags::CGEventFlagCommand);

    let up = CGEvent::new_keyboard_event(source, keycode, false)
        .map_err(|()| "CGEventCreateKeyboardEvent(keyup) returned NULL")?;
    up.set_flags(CGEventFlags::CGEventFlagCommand);

    // Post to the HID event tap — the earliest point in the input pipeline,
    // which means the target window sees the synthetic event identically
    // to a real keystroke. Without Accessibility permission these calls
    // complete successfully but the OS drops the events; there's no API
    // to detect that case at post time.
    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);
    Ok(())
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

/// Text on the clipboard, or `None` when it holds no plain text (image,
/// file, …).
pub fn read_clipboard_impl() -> Option<String> {
    pasteboard::read_text()
}

pub fn write_clipboard_impl(text: &str) -> bool {
    use std::io::Write;
    // pbcopy picks its text encoding from LANG, which a Finder-launched
    // .app doesn't have — without it non-ASCII text gets mangled.
    let Ok(mut child) = Command::new("pbcopy")
        .env("LANG", "en_US.UTF-8")
        .stdin(std::process::Stdio::piped())
        .spawn()
    else {
        return false;
    };
    if let Some(ref mut stdin) = child.stdin {
        if stdin.write_all(text.as_bytes()).is_err() {
            let _ = child.wait();
            return false;
        }
    }
    // pbcopy commits the clipboard on a clean exit; a non-zero status means
    // the write didn't land.
    matches!(child.wait(), Ok(status) if status.success())
}

pub fn capture_selection_impl() -> Option<String> {
    finish_selection_capture_impl(begin_selection_capture_impl())
}

#[cfg(test)]
mod tests {
    use super::non_empty_trimmed;

    #[test]
    fn non_empty_trimmed_drops_blank_and_missing_text() {
        assert_eq!(non_empty_trimmed(None), None);
        assert_eq!(non_empty_trimmed(Some("  \n".into())), None);
        assert_eq!(non_empty_trimmed(Some(" hi \n".into())), Some("hi".into()));
    }
}
