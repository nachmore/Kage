// macOS clipboard operations

use log::info;
use std::process::Command;

pub fn read_clipboard_impl() -> Option<String> {
    Command::new("pbpaste").output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

pub fn write_clipboard_impl(text: &str) {
    use std::io::Write;
    if let Ok(mut child) = Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn() {
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
        .args(["-e", "tell application \"System Events\" to keystroke \"c\" using command down"])
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
