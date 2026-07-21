use crate::{commands, telemetry};
use std::sync::{Arc, Mutex};

pub fn run(
    app: tauri::App,
    session_watcher: Arc<Mutex<Option<commands::sessions::SessionWatcherHandle>>>,
) {
    app.run(move |handler, event| {
        if let tauri::RunEvent::Exit = event {
            if let Ok(mut slot) = session_watcher.lock() {
                slot.take();
            }
            telemetry::record_shutdown(handler);
        }
    });
}
