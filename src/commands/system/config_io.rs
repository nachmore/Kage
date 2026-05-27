//! Configuration persistence and surrounding small command surfaces:
//! - main `get_config` / `save_config` round-trip
//! - frecency + shortcut history files (small JSON sidecars)
//! - tool permission policy editing (with audit log integration)
//! - dev/terminator mode flag readers
//! - cross-device backup export / import bundle commands
//! - crash recovery banner state
//! - generic write-text-file pass-through used by the dialog plugin

use crate::config::Config;
use crate::error::AppError;
use crate::lock_ext::LockExt;
use crate::state::{FeatureServices, UiState};
use log::{error, info};
use std::fs;
use tauri::{Emitter, State};

#[tauri::command]
pub async fn get_config(features: State<'_, FeatureServices>) -> Result<Config, AppError> {
    let config = features.config.lock_or_recover();
    Ok(config.clone())
}

#[tauri::command]
pub async fn save_config(
    config: Config,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    info!("Saving configuration");

    // Swap the in-memory state and persist under the same lock. Pre-fix the
    // sequence was disk-write → lock → swap, which raced with concurrent
    // saves (e.g. handle_permission_notification updating last_seen): the
    // disk write could capture a stale view, get clobbered by an in-flight
    // permission save, then the in-memory swap would proceed against an
    // even staler picture. Lock-mutate-save-drop is the only correct order.
    let new_terminator = config.tool_permissions.terminator_mode;
    let new_log_buffer_size = config.system.log_buffer_size;
    let prior_terminator = {
        let mut state_config = features.config.lock_or_recover();
        let prior = state_config.tool_permissions.terminator_mode;
        *state_config = config;
        // Normalize the update channel — unknown values collapse to
        // "stable". This guards against a frontend bug, a stale config
        // migration, or an end-user hand-editing config.json to a
        // channel we don't understand.
        state_config.updates.channel =
            crate::updater::normalize_channel(&state_config.updates.channel).to_string();
        state_config.save().map_err(|e| {
            error!("Failed to save config: {}", e);
            format!("Failed to save configuration: {}", e)
        })?;
        prior
    };

    // Update app log buffer size if changed
    crate::app_log::set_max_size(new_log_buffer_size);

    if prior_terminator != new_terminator {
        crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(
            crate::permission_audit::AuditEvent::TerminatorModeChanged {
                enabled: new_terminator,
            },
        ));
    }

    info!("Configuration saved successfully");

    if let Err(e) = app.emit("config_updated", ()) {
        error!("Failed to emit config_updated event: {}", e);
    }

    Ok(())
}

#[tauri::command]
pub async fn save_frecency(data: String) -> Result<(), AppError> {
    let path = dirs::config_dir()
        .ok_or("No config dir")?
        .join("kage")
        .join("search-frecency.json");
    Ok(std::fs::write(&path, &data).map_err(|e| format!("Failed to save frecency: {}", e))?)
}

#[tauri::command]
pub async fn load_frecency() -> Result<String, AppError> {
    let path = dirs::config_dir()
        .ok_or("No config dir")?
        .join("kage")
        .join("search-frecency.json");
    match std::fs::read_to_string(&path) {
        Ok(data) => Ok(data),
        Err(_) => Ok("{}".to_string()),
    }
}

const MAX_SHORTCUT_HISTORY: usize = 20;

fn shortcut_history_path() -> Result<std::path::PathBuf, String> {
    Ok(dirs::config_dir()
        .ok_or("No config dir")?
        .join("kage")
        .join("shortcut-history.json"))
}

/// Record a shortcut execution with arguments for history recall.
#[tauri::command]
pub async fn record_shortcut_usage(trigger: String, args: String) -> Result<(), AppError> {
    let args = args.trim().to_string();
    if args.is_empty() {
        return Ok(());
    }

    let path = shortcut_history_path()?;
    let mut history: serde_json::Map<String, serde_json::Value> = if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        serde_json::Map::new()
    };

    let entry = serde_json::json!({
        "args": args,
        "at": chrono::Utc::now().to_rfc3339()
    });

    let entries = history
        .entry(trigger)
        .or_insert_with(|| serde_json::json!([]));
    if let Some(arr) = entries.as_array_mut() {
        // Remove duplicate if same args already exist
        arr.retain(|e| {
            e.get("args").and_then(|a| a.as_str()) != Some(entry["args"].as_str().unwrap_or(""))
        });
        // Prepend new entry
        arr.insert(0, entry);
        // Cap at MAX_SHORTCUT_HISTORY
        arr.truncate(MAX_SHORTCUT_HISTORY);
    }

    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    Ok(fs::write(
        &path,
        serde_json::to_string_pretty(&history).unwrap_or_default(),
    )
    .map_err(|e| format!("Failed to save shortcut history: {}", e))?)
}

/// Get history entries for a specific shortcut trigger.
#[tauri::command]
pub async fn get_shortcut_history(trigger: String) -> Result<Vec<serde_json::Value>, AppError> {
    let path = shortcut_history_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }

    let history: serde_json::Map<String, serde_json::Value> = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    Ok(history
        .get(&trigger)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}

#[tauri::command]
pub async fn update_tool_policy(
    tool_title: String,
    policy: String,
    grant_type: Option<String>,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let gt = grant_type.unwrap_or_else(|| "once".to_string());
    info!(
        "Updating tool policy: {} -> {} (grant: {})",
        tool_title, policy, gt
    );
    let mut config = features.config.lock_or_recover();
    let timestamp = chrono::Utc::now().to_rfc3339();

    // Capture the prior state so the audit log can say "you changed
    // from allow/always to deny" etc. We emit BEFORE the struct is
    // mutated so a crash mid-write doesn't leave us logging the
    // wrong transition.
    let prior = config
        .tool_permissions
        .tools
        .iter()
        .find(|t| t.title == tool_title)
        .map(|t| (t.policy.clone(), t.grant_type.clone()));

    if let Some(tool) = config
        .tool_permissions
        .tools
        .iter_mut()
        .find(|t| t.title == tool_title)
    {
        tool.policy = policy.clone();
        tool.grant_type = gt.clone();
        tool.granted_at = timestamp;
    }
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    drop(config);

    // Log the transition. "allow" is a grant; "deny" or "ask" is a
    // revoke-style change when the prior policy was "allow".
    let event = match policy.as_str() {
        "allow" => crate::permission_audit::AuditEvent::Granted {
            tool: tool_title,
            grant_type: gt,
            session_id: None,
            args_preview: None,
        },
        _ => {
            if let Some((prior_policy, prior_gt)) = prior.filter(|(p, _)| p == "allow") {
                crate::permission_audit::AuditEvent::Revoked {
                    tool: tool_title,
                    prior_policy,
                    prior_grant_type: Some(prior_gt),
                }
            } else {
                // Transitioning from ask→deny or ask→ask; not interesting.
                return Ok(());
            }
        }
    };
    crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(event));
    Ok(())
}

#[tauri::command]
pub async fn remove_tool_permission(
    tool_title: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();

    // Snapshot the policy we're about to drop so we can log what was
    // revoked, not just that something was.
    let prior = config
        .tool_permissions
        .tools
        .iter()
        .find(|t| t.title == tool_title)
        .map(|t| (t.policy.clone(), t.grant_type.clone()));

    config
        .tool_permissions
        .tools
        .retain(|t| t.title != tool_title);
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    drop(config);

    if let Some((prior_policy, prior_gt)) = prior {
        crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(
            crate::permission_audit::AuditEvent::Revoked {
                tool: tool_title,
                prior_policy,
                prior_grant_type: Some(prior_gt),
            },
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn is_dev_mode(ui: State<'_, UiState>) -> Result<bool, AppError> {
    Ok(ui.dev_mode)
}

#[tauri::command]
pub async fn is_terminator_mode(features: State<'_, FeatureServices>) -> Result<bool, AppError> {
    let config = features.config.lock_or_recover();
    Ok(config.tool_permissions.terminator_mode)
}

// --- Cross-device backup commands -----------------------------------
//
// Export bundles the user's current config + steering docs +
// extension data into a single zip (optionally AES-GCM-encrypted with
// a passphrase). Import is the inverse — unwraps, sanitises away
// device-local fields, and writes everything back. See
// `src/config_export.rs` for the layout + sanitisation rules.

/// Suggested filename for the export dialog. Frontend uses this so
/// the date stamp matches Local time on the user's machine without
/// the JS having to format dates itself.
#[tauri::command]
pub async fn export_config_default_filename(encrypted: bool) -> String {
    crate::config_export::default_filename(encrypted)
}

/// Build the backup bytes and write them to the user-picked path.
/// Returns the byte count so the UI can confirm a successful write.
/// We do the disk write here rather than handing bytes back over IPC
/// because a config + extension-data bundle can be tens of MB and
/// the round-trip is wasteful.
#[tauri::command]
pub async fn export_config_bundle(
    path: String,
    passphrase: Option<String>,
    features: State<'_, FeatureServices>,
) -> Result<u64, AppError> {
    let config = {
        let cfg = features.config.lock_or_recover();
        cfg.clone()
    };
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        crate::config_export::export(&config, passphrase.as_deref())
    })
    .await
    .map_err(|e| AppError::from(format!("Export task failed: {}", e)))?
    .map_err(|e| AppError::from(format!("Failed to build backup: {}", e)))?;

    let len = bytes.len() as u64;
    std::fs::write(&path, &bytes)
        .map_err(|e| AppError::from(format!("Failed to write backup to {}: {}", path, e)))?;
    Ok(len)
}

/// Apply a backup bundle from disk. Returns the import summary (counts)
/// so the UI can render a concrete success toast. The new config is
/// written to disk + the in-memory state is replaced; a `config_updated`
/// Tauri event lets every window reload theme / shortcuts / etc.
#[tauri::command]
pub async fn import_config_bundle(
    path: String,
    passphrase: Option<String>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<crate::config_export::ImportSummary, AppError> {
    let bytes = std::fs::read(&path)
        .map_err(|e| AppError::from(format!("Failed to read backup at {}: {}", path, e)))?;
    let local = {
        let cfg = features.config.lock_or_recover();
        cfg.clone()
    };

    // Disk + decryption work happens off the runtime so the dialog
    // stays responsive even with Argon2's intentionally-slow KDF.
    let (new_config, summary) = tauri::async_runtime::spawn_blocking(move || {
        crate::config_export::import(&bytes, passphrase.as_deref(), &local)
    })
    .await
    .map_err(|e| AppError::from(format!("Import task failed: {}", e)))?
    .map_err(|e| AppError::from(format!("Failed to import backup: {}", e)))?;

    // Persist + swap the in-memory state under the same lock —
    // mirrors save_config so concurrent permission saves don't race.
    let new_log_buffer = new_config.system.log_buffer_size;
    let new_terminator = new_config.tool_permissions.terminator_mode;
    let prior_terminator = {
        let mut state_config = features.config.lock_or_recover();
        let prior = state_config.tool_permissions.terminator_mode;
        *state_config = new_config;
        // Same channel-string normalisation as save_config so a hand-
        // edited backup can't trap a user on a dead channel.
        state_config.updates.channel =
            crate::updater::normalize_channel(&state_config.updates.channel).to_string();
        state_config
            .save()
            .map_err(|e| format!("Failed to persist imported config: {}", e))?;
        prior
    };

    crate::app_log::set_max_size(new_log_buffer);
    if prior_terminator != new_terminator {
        crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(
            crate::permission_audit::AuditEvent::TerminatorModeChanged {
                enabled: new_terminator,
            },
        ));
    }

    // Re-apply the OS-level startup hook so it agrees with whatever
    // value is now in config (we already sanitised it to keep the
    // local preference, so this is essentially a no-op write — we
    // run it anyway so the registry / launchd entry state can never
    // drift from the JSON).
    let auto_start = {
        let cfg = features.config.lock_or_recover();
        cfg.system.auto_start
    };
    crate::os::set_startup_enabled(auto_start);

    let _ = app.emit("config_updated", ());
    Ok(summary)
}

/// Return the most recent unseen crash summary (or null) so the
/// floating window can decide whether to surface a "Kage crashed
/// last session" banner. Returns null when there's no crash log,
/// when the latest report has already been acknowledged, or when
/// the file is too malformed to parse useful fields out of.
///
/// Acknowledging happens via `dismiss_recent_crash` — that command
/// stamps `system.last_seen_crash_timestamp` so subsequent
/// invocations of THIS command return null until a NEW crash is
/// recorded.
#[tauri::command]
pub async fn get_recent_crash(
    features: State<'_, FeatureServices>,
) -> Result<Option<crate::crash_recovery::CrashSummary>, AppError> {
    let summary = match crate::crash_recovery::read_recent_crash() {
        Some(s) => s,
        None => return Ok(None),
    };
    let last_seen = features
        .config
        .lock_or_recover()
        .system
        .last_seen_crash_timestamp
        .clone();
    if !crate::crash_recovery::is_unseen(&summary, last_seen.as_deref()) {
        return Ok(None);
    }
    Ok(Some(summary))
}

/// Mark a crash report as acknowledged. The frontend calls this when
/// the user clicks any of the banner actions (View log / Report /
/// Dismiss) so the dialog doesn't fire again on the next launch.
/// `timestamp` must match the value returned by `get_recent_crash`
/// — we don't trust an arbitrary string from the UI to overwrite the
/// stamp, but the report header is the canonical id and the UI
/// already has it.
#[tauri::command]
pub async fn dismiss_recent_crash(
    timestamp: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let mut cfg = features.config.lock_or_recover();
    cfg.system.last_seen_crash_timestamp = Some(timestamp);
    let _ = cfg.save();
    Ok(())
}

/// Write a UTF-8 string to the given path. The frontend uses this for
/// any "save text" workflow that's already routed a path through the
/// Tauri dialog plugin (chat markdown export, today; future "save as"
/// flows can land here too without the frontend needing a new
/// command). Returns the byte count so the caller can confirm a
/// successful write.
///
/// Why an in-process command rather than `tauri-plugin-fs`: the fs
/// plugin requires per-path scope grants in `capabilities/`, which
/// would force us to pre-declare every directory the user might
/// pick. The dialog plugin already returns an absolute path the user
/// just consented to, so we just write it.
#[tauri::command]
pub async fn write_text_file(path: String, contents: String) -> Result<u64, AppError> {
    let len = contents.len() as u64;
    std::fs::write(&path, contents.as_bytes())
        .map_err(|e| AppError::from(format!("Failed to write {}: {}", path, e)))?;
    Ok(len)
}
