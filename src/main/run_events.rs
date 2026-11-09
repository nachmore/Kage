use crate::{commands, telemetry};
use std::sync::{Arc, Mutex};
use tauri::Manager;

pub fn run(
    app: tauri::App,
    session_watcher: Arc<Mutex<Option<commands::sessions::SessionWatcherHandle>>>,
) {
    app.run(move |handler, event| {
        if let tauri::RunEvent::Exit = event {
            // Hand ⌘Space (or any other taken-over system shortcut) back to
            // macOS first — the disabled state outlives our process.
            // Idempotent; run_all_killers below calls it again.
            crate::system_shortcut_takeover::release_all();
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
            // The host-command bridge's socket goes on every exit path
            // (idempotent: graceful_shutdown may have stopped it already).
            if let Some(procs) = handler.try_state::<crate::state::ChildProcesses>() {
                procs.host_commands.stop();
            }
            if let Some(acp) = handler.try_state::<crate::state::AcpHandles>() {
                acp.client.disconnect();
            }
            crate::process_manager::run_all_killers();
        }
    });
}
