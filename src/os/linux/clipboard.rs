// Linux clipboard operations

use log::info;
use std::process::Command;

/// Linux doesn't expose a sequence-number-based clipboard change API
/// the way Windows does, so we approximate two-phase capture by doing
/// the whole thing synchronously in `begin` and stashing the result on
/// the token. `finish` just unwraps it.
pub struct SelectionCaptureToken {
    selection: Option<String>,
}

pub fn begin_selection_capture_impl() -> SelectionCaptureToken {
    SelectionCaptureToken { selection: capture_selection_impl() }
}

pub fn finish_selection_capture_impl(token: SelectionCaptureToken) -> Option<String> {
    token.selection
}

/// Simulate a Ctrl+V paste keystroke into the foreground window.
/// Stub today — a real implementation would use xdotool or wtype on
/// Wayland. Logs once per process so the missing behaviour is visible.
pub fn simulate_paste_impl() {
    use std::sync::OnceLock;
    static WARNED: OnceLock<()> = OnceLock::new();
    WARNED.get_or_init(|| {
        log::warn!("simulate_paste: Linux implementation not yet available — paste keystroke skipped");
    });
}

pub fn read_clipboard_impl() -> Option<String> {
    Command::new("xclip").args(["-selection", "clipboard", "-o"]).output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

pub fn write_clipboard_impl(text: &str) {
    use std::io::Write;
    if let Ok(mut child) = Command::new("xclip")
        .args(["-selection", "clipboard"])
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

    // Simulate Ctrl+C via xdotool
    let _ = Command::new("xdotool")
        .args(["key", "ctrl+c"])
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
