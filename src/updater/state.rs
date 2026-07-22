use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tauri_plugin_updater::Update;

/// Cached updater state shared by commands and the background scheduler.
pub struct UpdaterState {
    pub last_user_activity: std::sync::Mutex<Instant>,
    pub update_ready: AtomicBool,
    pub pending_update: std::sync::Mutex<Option<Update>>,
    pub available_version: std::sync::Mutex<Option<String>>,
}

impl Default for UpdaterState {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdaterState {
    pub fn new() -> Self {
        Self {
            last_user_activity: std::sync::Mutex::new(Instant::now()),
            update_ready: AtomicBool::new(false),
            pending_update: std::sync::Mutex::new(None),
            available_version: std::sync::Mutex::new(None),
        }
    }

    pub fn touch_activity(&self) {
        if let Ok(mut activity) = self.last_user_activity.lock() {
            *activity = Instant::now();
        }
    }

    pub fn is_idle(&self) -> bool {
        self.last_user_activity
            .lock()
            .map(|activity| activity.elapsed().as_secs() >= 300)
            .unwrap_or(false)
    }
}

/// Whether a window label counts as "the user might be looking at this"
/// for the silent-update idle gate. Covers the fixed user-facing windows
/// plus every per-session `chat-<uuid>` window — a user mid-conversation
/// in a chat window must block a silent relaunch just like one in `main`.
pub(super) fn is_user_facing_label(label: &str) -> bool {
    label == crate::window_labels::FLOATING
        || label == crate::window_labels::SETTINGS
        || crate::window_labels::is_session_host_label(label)
}

pub(super) fn is_any_user_window_visible(app: &tauri::AppHandle) -> bool {
    use tauri::Manager;

    app.webview_windows()
        .iter()
        .any(|(label, window)| is_user_facing_label(label) && window.is_visible().unwrap_or(false))
}

pub(super) fn clear_ready(state: &UpdaterState) {
    state.update_ready.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::is_user_facing_label;
    use crate::window_labels;

    #[test]
    fn fixed_user_windows_are_user_facing() {
        for label in [
            window_labels::FLOATING,
            window_labels::MAIN,
            window_labels::SETTINGS,
        ] {
            assert!(is_user_facing_label(label), "{} should gate updates", label);
        }
    }

    #[test]
    fn chat_session_windows_are_user_facing() {
        let label = window_labels::chat_label("11111111-2222-3333-4444-555555555555");
        assert!(is_user_facing_label(&label));
    }

    #[test]
    fn non_user_windows_do_not_gate() {
        // context-menu / welcome / store / inline-assist are transient or
        // programmatic surfaces; their visibility shouldn't hold updates.
        for label in [
            window_labels::CONTEXT_MENU,
            window_labels::WELCOME,
            window_labels::STORE,
            window_labels::INLINE_ASSIST,
        ] {
            assert!(
                !is_user_facing_label(label),
                "{} should not gate updates",
                label
            );
        }
    }
}
