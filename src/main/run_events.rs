use crate::{commands, telemetry};
use std::sync::{Arc, Mutex};
use tauri::Manager;

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
            // Exits that skip graceful_shutdown (macOS logout/restart via
            // the quit Apple Event, any `app.exit`) land here, and the
            // event loop then calls process::exit so no Drop runs. On
            // macOS/Linux there's no Job Object and the agent is a setsid
            // session leader, so kill it (and pocket-tts) explicitly.
            // Both are take-then-kill, so paths that already cleaned up
            // are no-ops.
            if let Some(acp) = handler.try_state::<crate::state::AcpHandles>() {
                acp.client.disconnect();
            }
            crate::process_manager::run_all_killers();
        }
    });
}
