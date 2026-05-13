// Cross-platform clipboard operations.
//
// Each platform's `clipboard` submodule defines its own opaque
// `SelectionCaptureToken` type — the data needed to complete a
// two-phase capture is genuinely different per OS (Windows uses the
// clipboard sequence number; macOS/Linux just capture synchronously
// and stash the result). We re-export the platform's token here so
// callers see a uniform name.

pub use crate::os::platform::clipboard::SelectionCaptureToken;

/// Read text from the system clipboard.
pub fn read_clipboard() -> Option<String> {
    crate::os::platform::clipboard::read_clipboard_impl()
}

/// Write text to the system clipboard.
pub fn write_clipboard(text: &str) {
    crate::os::platform::clipboard::write_clipboard_impl(text);
}

/// Capture the currently selected text from the active window.
#[allow(dead_code)]
pub fn capture_selection() -> Option<String> {
    crate::os::platform::clipboard::capture_selection_impl()
}

/// Phase 1 of two-phase selection capture: send the copy keystroke to
/// the foreground window. Must be called while the source window is
/// still focused.
pub fn begin_selection_capture() -> SelectionCaptureToken {
    crate::os::platform::clipboard::begin_selection_capture_impl()
}

/// Check whether the given foreground process name is on the user's
/// "don't inject Ctrl+C" blocklist. Comparison is case-insensitive;
/// a trailing ".exe" on either side is ignored so users can enter
/// either form in settings. Returns `false` on empty inputs (fail-open
/// — capture still runs, matching prior behaviour).
pub fn is_process_blocklisted(process_name: &str, blocklist: &[String]) -> bool {
    if blocklist.is_empty() {
        return false;
    }
    let needle = strip_exe(process_name.trim()).to_ascii_lowercase();
    if needle.is_empty() {
        return false;
    }
    blocklist
        .iter()
        .any(|entry| strip_exe(entry.trim()).eq_ignore_ascii_case(&needle))
}

fn strip_exe(s: &str) -> &str {
    s.strip_suffix(".exe")
        .or_else(|| s.strip_suffix(".EXE"))
        .unwrap_or(s)
}

/// Phase 2: poll the clipboard and return the captured text.
pub fn finish_selection_capture(token: SelectionCaptureToken) -> Option<String> {
    crate::os::platform::clipboard::finish_selection_capture_impl(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocklist_is_case_insensitive() {
        let list = vec!["cmd".to_string(), "PowerShell".to_string()];
        assert!(is_process_blocklisted("CMD", &list));
        assert!(is_process_blocklisted("powershell", &list));
        assert!(!is_process_blocklisted("pwsh", &list));
    }

    #[test]
    fn blocklist_ignores_exe_suffix_on_either_side() {
        let list = vec!["cmd.exe".to_string()];
        assert!(is_process_blocklisted("cmd", &list));
        assert!(is_process_blocklisted("CMD.EXE", &list));

        let list2 = vec!["cmd".to_string()];
        assert!(is_process_blocklisted("cmd.exe", &list2));
    }

    #[test]
    fn empty_blocklist_matches_nothing() {
        assert!(!is_process_blocklisted("cmd", &[]));
    }

    #[test]
    fn empty_process_name_matches_nothing() {
        let list = vec!["cmd".to_string()];
        assert!(!is_process_blocklisted("", &list));
        assert!(!is_process_blocklisted("   ", &list));
    }

    #[test]
    fn entries_are_trimmed() {
        let list = vec!["  cmd  ".to_string()];
        assert!(is_process_blocklisted("cmd", &list));
    }
}
