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

pub(super) fn is_any_user_window_visible(app: &tauri::AppHandle) -> bool {
    use tauri::Manager;

    [
        crate::window_labels::FLOATING,
        crate::window_labels::MAIN,
        crate::window_labels::SETTINGS,
    ]
    .iter()
    .any(|label| {
        app.get_webview_window(label)
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false)
    })
}

pub(super) fn clear_ready(state: &UpdaterState) {
    state.update_ready.store(false, Ordering::SeqCst);
}
