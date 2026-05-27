//! Diagnostics surface: in-memory app log, the per-thread CPU dump
//! (delegated to `os::diagnostics`), the permission audit log, and
//! telemetry pass-through. None of these touch the agent transport
//! or main event loop — they're observability glue.

use crate::error::AppError;
use crate::lock_ext::LockExt;
use crate::state::FeatureServices;
use tauri::State;

// --- App Log commands ------------------------------------------------

/// Write a log entry from the frontend.
#[tauri::command]
pub async fn app_log_write(level: String, source: String, msg: String) -> Result<(), AppError> {
    crate::app_log::log(&level, &source, &msg);
    Ok(())
}

/// Get all log entries.
#[tauri::command]
pub async fn app_log_get_entries() -> Result<Vec<crate::app_log::LogEntry>, AppError> {
    Ok(crate::app_log::get_entries())
}

/// Clear all log entries.
#[tauri::command]
pub async fn app_log_clear() -> Result<(), AppError> {
    crate::app_log::clear().map_err(|e| e.to_string())?;
    Ok(())
}

/// Get the log directory path (for "Open Logs Folder").
#[tauri::command]
pub async fn app_log_get_dir() -> Result<String, AppError> {
    Ok(crate::app_log::log_dir_string())
}

// --- Thread CPU dump -------------------------------------------------

/// Dump thread CPU usage info. Takes two snapshots 3 seconds apart so
/// we can show which threads are actively burning CPU plus cumulative
/// totals. Available via the tray menu in debug mode. The work happens
/// in `os::diagnostics::dump_thread_info` — moving the OS-specific
/// sampling out of the command layer keeps platform `#[cfg]` arms in
/// the OS abstraction where they belong.
#[tauri::command]
pub async fn dump_thread_info() -> Result<String, AppError> {
    let result = tauri::async_runtime::spawn_blocking(crate::os::diagnostics::dump_thread_info)
        .await
        .map_err(|e| AppError::from(format!("Thread dump task failed: {}", e)))?;
    Ok(result)
}

// --- Permission audit log commands ----------------------------------

/// Read the most recent `limit` entries from the permission audit log,
/// newest-first. `limit` is clamped to 1..=2000 so a misbehaving UI
/// can't ask for an enormous slice.
#[tauri::command]
pub async fn get_permission_audit_log(
    limit: Option<usize>,
) -> Result<Vec<crate::permission_audit::AuditEntry>, AppError> {
    let n = clamp_audit_limit(limit);
    Ok(crate::permission_audit::read_recent_default(n))
}

/// Default-and-clamp the audit-log query limit. 1..=2000 caps memory
/// and IPC payload size; default of 500 matches what the UI requests
/// when it doesn't pass a value. Pulled out so the bound is testable
/// without spinning up the audit-log infrastructure.
pub(crate) fn clamp_audit_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(500).clamp(1, 2000)
}

/// Clear the permission audit log. Intended for the "Clear log" button
/// in settings. Not destructive beyond the log itself — permissions
/// and grants are untouched.
#[tauri::command]
pub async fn clear_permission_audit_log() -> Result<(), AppError> {
    crate::permission_audit::clear_default()
        .map_err(|e| format!("Failed to clear audit log: {}", e))?;
    Ok(())
}

/// Return the filesystem path to the audit log so the UI can show it
/// to the user (e.g. "stored at: ~/.../permission-audit.jsonl") and
/// link to "open in file explorer".
#[tauri::command]
pub async fn get_permission_audit_log_path() -> Result<String, AppError> {
    Ok(crate::permission_audit::default_log_path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default())
}

// --- Telemetry commands ----------------------------------------------
//
// The Aptabase plugin is registered (or not) in main.rs based on the
// compile-time APTABASE_KEY. These commands let the UI surface the
// current telemetry state and change it without needing to poke at
// `config.telemetry` fields directly from the frontend.

/// Return the current telemetry snapshot for the Settings → Privacy UI.
#[tauri::command]
pub async fn get_telemetry_info(
    features: State<'_, FeatureServices>,
) -> Result<crate::telemetry::TelemetryInfo, AppError> {
    Ok(crate::telemetry::snapshot(&features.config))
}

/// Enable or disable anonymous telemetry. Applies immediately — any
/// subsequent calls to `telemetry::track()` respect the new value.
#[tauri::command]
pub async fn set_telemetry_enabled(
    app: tauri::AppHandle,
    features: State<'_, FeatureServices>,
    enabled: bool,
) -> Result<(), AppError> {
    // Track the toggle *before* applying it so an opt-out still sends a
    // single "telemetry_disabled" event — useful for measuring opt-out
    // rates. Opt-in cannot fire a pre-change event because telemetry
    // was off; we fire "telemetry_enabled" right after set_consent
    // instead.
    let was_enabled = features.config.lock_or_recover().telemetry.enabled;
    if was_enabled && !enabled {
        crate::telemetry::track(&app, "telemetry_disabled", None);
    }

    crate::telemetry::set_consent(&features.config, enabled);

    if !was_enabled && enabled {
        crate::telemetry::track(&app, "telemetry_enabled", None);
    }
    Ok(())
}

/// Generate a fresh anonymous install ID. The returned value is the new
/// ID (kept for UI display only — the plugin already uses it on the next
/// event).
#[tauri::command]
pub async fn reset_telemetry_install_id(
    features: State<'_, FeatureServices>,
) -> Result<String, AppError> {
    Ok(crate::telemetry::reset_install_id(&features.config))
}

/// Pass-through for the frontend to fire arbitrary telemetry events.
/// Gated by `telemetry.enabled` on the Rust side — calls from an opted-out
/// user become no-ops. Props are restricted to string/number values by
/// Aptabase's plugin, but we don't re-validate here because the plugin
/// already does.
///
/// Event names MUST be drawn from the known list in
/// `ui/js/shared/telemetry.js`. Unknown names are still accepted but
/// logged so accidental PII leakage through a misnamed event surfaces
/// during development.
#[tauri::command]
pub async fn telemetry_track(
    app: tauri::AppHandle,
    event: String,
    props: Option<serde_json::Value>,
) -> Result<(), AppError> {
    crate::telemetry::track(&app, &event, props);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_limit_defaults_when_unset() {
        // None → 500 (the UI's standard "show me a page" request).
        assert_eq!(clamp_audit_limit(None), 500);
    }

    #[test]
    fn audit_limit_clamps_to_safe_range() {
        // Lower bound: 0 must not collapse to 0 (read_recent on 0 returns
        // nothing useful and is just wasted IPC).
        assert_eq!(clamp_audit_limit(Some(0)), 1);
        // Within range: pass-through.
        assert_eq!(clamp_audit_limit(Some(1)), 1);
        assert_eq!(clamp_audit_limit(Some(100)), 100);
        assert_eq!(clamp_audit_limit(Some(2000)), 2000);
        // Upper bound: 2000-row payload is already ~200KB; cap there so a
        // misbehaving UI can't ask for the entire log.
        assert_eq!(clamp_audit_limit(Some(2001)), 2000);
        assert_eq!(clamp_audit_limit(Some(usize::MAX)), 2000);
    }
}
