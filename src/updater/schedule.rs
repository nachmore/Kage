use super::state::{clear_ready, has_active_agent_jobs, is_any_user_window_visible};
use super::{
    persist_install_source, persist_resume_marker, plugin_check, plugin_download_and_install,
    relaunch_and_exit, InstallSource, UpdaterState,
};
use crate::config::Config;
use crate::lock_ext::LockExt;
use log::{error, info, warn};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Longest a silent install waits on continuous agent work. Bounds the damage
/// if an in-flight marker ever leaks (e.g. a send path that never clears it).
const MAX_BUSY_DEFER: Duration = Duration::from_secs(4 * 3600);

pub fn start_update_loop(
    updater: Arc<UpdaterState>,
    config: Arc<std::sync::Mutex<Config>>,
    app: tauri::AppHandle,
    sessions: Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    acp_client: Arc<crate::acp_client::AcpClient>,
) {
    spawn_check_loop(updater.clone(), config.clone(), app.clone());
    spawn_idle_loop(updater, config, app, sessions, acp_client);
}

fn spawn_check_loop(
    updater: Arc<UpdaterState>,
    config: Arc<std::sync::Mutex<Config>>,
    app: tauri::AppHandle,
) {
    tauri::async_runtime::spawn(async move {
        crate::os::set_current_thread_name("updater-check");
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        let mut first = true;
        loop {
            let (enabled, check, channel) = {
                let cfg = config.lock_or_recover();
                let check = cfg.updates.auto_check
                    && (first
                        || cfg.updates.last_check_time.as_ref().is_none_or(|time| {
                            chrono::DateTime::parse_from_rfc3339(time)
                                .map(|date| {
                                    chrono::Utc::now().signed_duration_since(date).num_hours() >= 24
                                })
                                .unwrap_or(true)
                        }));
                (cfg.updates.auto_check, check, cfg.updates.channel)
            };
            first = false;
            if enabled && check {
                match plugin_check(&app, channel).await {
                    Ok(Some(update)) => {
                        let version = update.version.clone();
                        if let Ok(mut value) = updater.available_version.lock() {
                            *value = Some(version.clone());
                        }
                        if let Ok(mut slot) = updater.pending_update.lock() {
                            *slot = Some(update);
                        }
                        updater.update_ready.store(true, Ordering::SeqCst);
                        crate::event_targets::emit_update_audience(
                            &app,
                            crate::events::UPDATE_AVAILABLE,
                            &version,
                        );
                        stamp_last_check_time(&config);
                    }
                    Ok(None) => stamp_last_check_time(&config),
                    Err(error) => {
                        warn!("Update check failed: {error}");
                        crate::telemetry::track(
                            &app,
                            "update_check_failed",
                            Some(
                                serde_json::json!({"reason": check_error_reason(&error), "channel": channel.as_str()}),
                            ),
                        );
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });
}

fn spawn_idle_loop(
    updater: Arc<UpdaterState>,
    config: Arc<std::sync::Mutex<Config>>,
    app: tauri::AppHandle,
    sessions: Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    acp_client: Arc<crate::acp_client::AcpClient>,
) {
    tauri::async_runtime::spawn(async move {
        crate::os::set_current_thread_name("updater-idle");
        let mut busy_since: Option<Instant> = None;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            if is_any_user_window_visible(&app) {
                updater.touch_activity();
            }
            // Hidden windows don't mean nothing is happening: the floating
            // window hides on blur while a long agent turn keeps running, and
            // relaunching would kill it. Agent work counts as activity, so the
            // 5-minute idle window restarts once the turn finishes.
            let busy = is_agent_busy(&acp_client);
            if defer_for_agent_work(busy, &mut busy_since, Instant::now()) {
                updater.touch_activity();
                continue;
            }
            if !updater.update_ready.load(Ordering::SeqCst)
                || !updater.is_idle()
                || is_any_user_window_visible(&app)
                || !config.lock_or_recover().updates.silent_update
            {
                continue;
            }
            let Some(update) = updater.pending_update.lock_or_recover().take() else {
                clear_ready(&updater);
                continue;
            };
            if let Ok(mut cfg) = config.try_lock() {
                cfg.updates.last_updated_version = updater
                    .available_version
                    .lock()
                    .ok()
                    .and_then(|value| value.clone());
                let _ = cfg.save();
            }
            let session = sessions.lock().ok().and_then(|map| {
                map.get(crate::window_labels::FLOATING)
                    .cloned()
                    .or_else(|| map.get(crate::window_labels::MAIN).cloned())
            });
            persist_resume_marker(session.as_deref());
            persist_install_source(InstallSource::Idle);
            match plugin_download_and_install(&app, update).await {
                Ok(()) => {
                    info!("Update installed; relaunching");
                    relaunch_and_exit(&app);
                }
                Err(error) => {
                    error!("Failed to install update: {error}");
                    clear_ready(&updater);
                }
            }
        }
    });
}

/// Whether any prompt (user turn or background steering/title) is in
/// flight, or a multi-step agent job (automation plan) is running.
fn is_agent_busy(client: &crate::acp_client::AcpClient) -> bool {
    has_active_agent_jobs()
        || !client.in_flight_prompts.lock_or_recover().is_empty()
        || !client
            .background_prompt_sessions
            .lock_or_recover()
            .is_empty()
}

/// Decide whether to hold off a silent install because the agent is busy.
/// Tracks when the current busy stretch began so a leaked in-flight marker
/// can't block silent updates forever (see `MAX_BUSY_DEFER`).
fn defer_for_agent_work(busy: bool, busy_since: &mut Option<Instant>, now: Instant) -> bool {
    if !busy {
        *busy_since = None;
        return false;
    }
    let since = *busy_since.get_or_insert(now);
    now.duration_since(since) < MAX_BUSY_DEFER
}

fn check_error_reason(error: &anyhow::Error) -> &'static str {
    let message = error.to_string().to_lowercase();
    if message.contains("signature") || message.contains("verify") {
        "signature"
    } else if message.contains("no endpoint") || message.contains("not configured") {
        "config"
    } else if message.contains("404") || message.contains("not found") {
        "not_found"
    } else if ["dns", "connect", "network", "timeout"]
        .iter()
        .any(|term| message.contains(term))
    {
        "network"
    } else {
        "other"
    }
}

fn stamp_last_check_time(config: &Arc<std::sync::Mutex<Config>>) {
    let snapshot = match config.try_lock() {
        Ok(mut cfg) => {
            cfg.updates.last_check_time = Some(chrono::Utc::now().to_rfc3339());
            cfg.clone()
        }
        Err(_) => return,
    };
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = snapshot.save() {
            warn!("Failed to save config (update check stamp): {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defers_while_busy_and_resets_when_idle() {
        let start = Instant::now();
        let mut since = None;
        assert!(defer_for_agent_work(true, &mut since, start));
        assert_eq!(since, Some(start));
        assert!(defer_for_agent_work(
            true,
            &mut since,
            start + Duration::from_secs(600)
        ));
        assert!(!defer_for_agent_work(false, &mut since, start));
        assert_eq!(since, None);
    }

    #[test]
    fn stops_deferring_after_ceiling() {
        let start = Instant::now();
        let mut since = None;
        assert!(defer_for_agent_work(true, &mut since, start));
        assert!(!defer_for_agent_work(
            true,
            &mut since,
            start + MAX_BUSY_DEFER
        ));
    }
}
