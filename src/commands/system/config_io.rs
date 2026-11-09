//! Configuration persistence and surrounding small command surfaces:
//! - main `get_config` / `save_config` round-trip
//! - frecency + shortcut history files (small JSON sidecars)
//! - tool permission policy editing (with audit log integration)
//! - dev/terminator mode flag readers
//! - cross-device backup export / import bundle commands
//! - crash recovery banner state
//! - generic write-text-file pass-through used by the dialog plugin

use crate::config::Config;
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, FeatureServices, UiState};
use log::{error, info};
use std::fs;
use tauri::{Emitter, State};

#[tauri::command]
pub async fn get_config(features: State<'_, FeatureServices>) -> Result<Config, AppError> {
    let config = features.config.lock_or_recover();
    Ok(config.clone())
}

#[tauri::command]
pub async fn save_config<R: tauri::Runtime>(
    config: Config,
    features: State<'_, FeatureServices>,
    acp: State<'_, AcpHandles>,
    app: tauri::AppHandle<R>,
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
    let new_language = config.ui.language.clone();
    let new_acp_mode = config.acp.active_mode();
    let new_login_env = config.acp.agent.import_login_shell_env;
    let (prior_terminator, prior_acp_mode, sites_changed) = {
        let mut state_config = features.config.lock_or_recover();
        let prior = state_config.tool_permissions.terminator_mode;
        let prior_acp = state_config.acp.active_mode();
        let sites_changed = state_config.internal_sites != config.internal_sites;
        let mut config = config;
        preserve_backend_owned_fields(&mut config, &state_config);
        *state_config = config;
        // The update channel is a typed enum with `#[serde(other)]` →
        // Stable, so an unknown wire value (stale config, hand-edit,
        // future variant) collapses to Stable at deserialise time. No
        // explicit normalisation needed here.
        state_config.save().map_err(|e| {
            error!("Failed to save config: {}", e);
            format!("Failed to save configuration: {}", e)
        })?;
        (prior, prior_acp, sites_changed)
    };

    // Cached previews were resolved under the old internal-site profiles
    // (e.g. a login page cached as a successful preview); drop them so
    // the new rules apply to links already on screen.
    if sites_changed {
        tauri::async_runtime::spawn_blocking(|| {
            if let Err(e) = crate::link_metadata_cache::clear() {
                log::warn!("Failed to clear link metadata cache: {}", e);
            }
        });
    }

    apply_runtime_config(
        &app,
        &acp,
        &prior_acp_mode,
        &new_acp_mode,
        new_language,
        new_login_env,
    );

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

    if let Err(e) = app.emit(
        crate::events::CONFIG_UPDATED,
        crate::commands::i18n::ConfigUpdatedI18n::current(),
    ) {
        error!("Failed to emit config_updated event: {}", e);
    }

    Ok(())
}

/// Apply the parts of a config swap that live outside the `Config` struct.
/// Shared by `save_config` and `import_config_bundle` so an import takes
/// effect without a restart. Must run BEFORE the caller emits
/// `config_updated`.
fn apply_runtime_config<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    acp: &AcpHandles,
    prior_acp_mode: &crate::config::AcpMode,
    new_acp_mode: &crate::config::AcpMode,
    new_language: Option<String>,
    import_login_shell_env: bool,
) {
    // Before any respawn below: the next agent spawn (this reconnect, or
    // the next lazy one) picks the toggle up without a restart.
    crate::os::login_env::set_enabled(import_login_shell_env);

    // If the active agent connection changed, hot-swap the transport mode
    // and reconnect so the user doesn't have to restart the app. On the
    // blocking pool: `set_mode` waits for an in-flight connect to finish
    // (the transport serialises them), and `connect` spawns the agent —
    // which can wait for the login-shell probe — or dials it (Remote: up to
    // six 5 s attempts). None of that may park this async worker.
    if new_acp_mode != prior_acp_mode {
        info!("Active ACP connection changed, reconnecting");
        let (new_connection_mode, _) = crate::startup::acp_mode_for(new_acp_mode);
        let client = acp.client.clone();
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            client.set_mode(new_connection_mode);
            if let Err(e) = client.connect() {
                log::warn!("ACP reconnect after config change failed: {}", e);
                // set_mode tore down the old connection, and its reader was
                // superseded, so nothing else reports the drop. Tell windows
                // now rather than leaving a stale "connected" header. Only on
                // failure: a successful swap must not flip headers.
                if let Err(e) = app.emit(crate::events::AGENT_DISCONNECTED, ()) {
                    error!("Failed to emit agent_disconnected event: {}", e);
                }
            }
        });
    }

    // Apply the language override before `config_updated` goes out: every
    // window's `i18n.js` listener immediately calls `get_i18n_catalog`, and
    // if the active locale were still the old one those windows would
    // refetch the OLD catalog and conclude nothing changed. `None` / empty
    // means "follow system".
    let preferred = new_language
        .filter(|s| !s.trim().is_empty())
        .or_else(sys_locale::get_locale);
    crate::i18n::set_language(preferred.as_deref().unwrap_or("en"));
}

/// Overwrite backend-owned fields in an incoming JS config snapshot with
/// the authoritative in-memory values.
///
/// Capability grants are only ever written by the install/uninstall
/// commands (`commit_extension_install`, `remove_extension_grant`,
/// welcome provisioning) — never by JS. But `save_config` carries a FULL
/// `Config` snapshot fetched by some window at some earlier time, so a
/// window holding a snapshot from before a grant was recorded would
/// silently wipe it on its next unrelated save (the "every extension
/// runs with zero capabilities" lost-update bug). Keep the backend's map
/// regardless of what the snapshot says, and log when they diverge so
/// the offending window is visible in the log.
///
/// `extension_states` is deliberately NOT protected: the settings window
/// legitimately toggles it through `save_config`.
pub(crate) fn preserve_backend_owned_fields(incoming: &mut Config, authoritative: &Config) {
    if incoming.extension_grants != authoritative.extension_grants {
        log::warn!(
            "save_config: incoming snapshot carries {} extension grant(s) but the backend has {}; \
             keeping the backend's (grants are backend-owned and JS must never rewrite them)",
            incoming.extension_grants.len(),
            authoritative.extension_grants.len()
        );
        incoming.extension_grants = authoritative.extension_grants.clone();
    }
    // Background AI (switch, consent, allowance) changes only through
    // `set_extension_agent_access`, uninstall and grant removal. A window
    // that fetched its snapshot before the user switched it off must not
    // switch it back on with an unrelated save (EXTENSION_AGENT_CALLS
    // §5.13).
    if incoming.extension_agent_access != authoritative.extension_agent_access {
        log::warn!(
            "save_config: ignoring an extension_agent_access change in the incoming snapshot \
             ({} entries, backend has {}; backend-owned; use set_extension_agent_access)",
            incoming.extension_agent_access.len(),
            authoritative.extension_agent_access.len()
        );
        incoming.extension_agent_access = authoritative.extension_agent_access.clone();
    }
    // Category-wide tool trust ("trust all CLI commands") is changed only
    // through the audited `set_tool_kind_trusted`; a stale or buggy window
    // snapshot must not grant or revoke it.
    if incoming.tool_permissions.trusted_kinds != authoritative.tool_permissions.trusted_kinds {
        log::warn!(
            "save_config: ignoring a trusted_kinds change in the incoming snapshot \
             (backend-owned; use set_tool_kind_trusted)"
        );
        incoming.tool_permissions.trusted_kinds =
            authoritative.tool_permissions.trusted_kinds.clone();
    }
    // Same for "trust all tools from <server>": only the audited
    // `set_mcp_server_trusted` grants or revokes it.
    if incoming.tool_permissions.trusted_mcp_servers
        != authoritative.tool_permissions.trusted_mcp_servers
    {
        log::warn!(
            "save_config: ignoring a trusted_mcp_servers change in the incoming snapshot \
             (backend-owned; use set_mcp_server_trusted)"
        );
        incoming.tool_permissions.trusted_mcp_servers =
            authoritative.tool_permissions.trusted_mcp_servers.clone();
    }
    // Rules for commands an agent runs outside its sandbox, and the
    // allow-all switch: only the prompt's "Always allow" and the audited
    // Settings commands change them.
    if incoming.tool_permissions.host_command_rules
        != authoritative.tool_permissions.host_command_rules
        || incoming.tool_permissions.host_commands_allow_all
            != authoritative.tool_permissions.host_commands_allow_all
    {
        log::warn!(
            "save_config: ignoring a change to the outside-the-sandbox command rules in the \
             incoming snapshot (backend-owned; use remove_host_command_rule / \
             set_host_commands_allow_all)"
        );
        incoming.tool_permissions.host_command_rules =
            authoritative.tool_permissions.host_command_rules.clone();
        incoming.tool_permissions.host_commands_allow_all =
            authoritative.tool_permissions.host_commands_allow_all;
    }
}

/// Keep this machine's category-wide tool trust ("trust all commands",
/// "trust all tools from <server>") over an imported config's. A backup is
/// a plain, possibly hand-edited file: restoring it must neither switch
/// that trust on without consent here nor silently drop what the user
/// trusted on this machine.
pub(crate) fn keep_local_tool_trust(imported: &mut Config, local: &Config) {
    imported.tool_permissions.trusted_kinds = local.tool_permissions.trusted_kinds.clone();
    imported.tool_permissions.trusted_mcp_servers =
        local.tool_permissions.trusted_mcp_servers.clone();
    // Same for commands an agent runs outside its sandbox: a backup can't
    // add a rule or switch allow-all on here, nor take this machine's away.
    imported.tool_permissions.host_command_rules =
        local.tool_permissions.host_command_rules.clone();
    imported.tool_permissions.host_commands_allow_all =
        local.tool_permissions.host_commands_allow_all;
}

#[tauri::command]
pub async fn save_frecency(data: String) -> Result<(), AppError> {
    let path = crate::paths::config_dir()
        .ok_or("No config dir")?
        .join("kage")
        .join("search-frecency.json");
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // Atomic: a torn file reads back as garbage and the next save would
    // replace the user's whole ranking history with a fresh one.
    Ok(
        crate::atomic_write::write(&path, &data, crate::atomic_write::Perms::Private)
            .map_err(|e| format!("Failed to save frecency: {}", e))?,
    )
}

#[tauri::command]
pub async fn load_frecency() -> Result<String, AppError> {
    let path = crate::paths::config_dir()
        .ok_or("No config dir")?
        .join("kage")
        .join("search-frecency.json");
    match std::fs::read_to_string(&path) {
        Ok(data) => Ok(data),
        Err(_) => Ok("{}".to_string()),
    }
}

/// Length of the per-install frecency key, in bytes.
const FRECENCY_KEY_LEN: usize = 32;

/// The per-install secret the launcher keys its frecency hashes with
/// (base64). `search-frecency.json` stores HMACs of result ids and query
/// prefixes rather than the values themselves: extension result ids can
/// carry private text (bookmark names, meeting subjects, locations), and a
/// keyed hash keeps ranking working without writing that text to disk.
/// The key lives in its own file so the frecency file alone can't be
/// dictionary-checked. Created on first use; a malformed key is replaced,
/// which only resets ranking.
#[tauri::command]
pub async fn get_frecency_key() -> Result<String, AppError> {
    let path = crate::paths::config_dir()
        .ok_or("No config dir")?
        .join("kage")
        .join("search-frecency.key");
    Ok(
        tauri::async_runtime::spawn_blocking(move || load_or_create_frecency_key(&path))
            .await
            .map_err(|e| format!("Frecency key task failed: {}", e))??,
    )
}

fn load_or_create_frecency_key(path: &std::path::Path) -> Result<String, String> {
    use base64::Engine;
    let engine = base64::engine::general_purpose::STANDARD;
    if let Ok(existing) = fs::read_to_string(path) {
        let trimmed = existing.trim();
        if engine
            .decode(trimmed)
            .is_ok_and(|bytes| bytes.len() == FRECENCY_KEY_LEN)
        {
            return Ok(trimmed.to_string());
        }
        log::warn!("Frecency key file is malformed; replacing it");
    }
    use rand::TryRngCore;
    let mut key = [0u8; FRECENCY_KEY_LEN];
    rand::rngs::OsRng
        .try_fill_bytes(&mut key)
        .map_err(|e| format!("OS RNG (frecency key): {}", e))?;
    let encoded = engine.encode(key);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Failed to create config dir: {}", e))?;
    }
    write_private_file(path, encoded.as_bytes())
        .map_err(|e| format!("Failed to save frecency key: {}", e))?;
    Ok(encoded)
}

/// Write `bytes` to `path`, readable only by the current user on Unix.
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes)
    }
}

const MAX_SHORTCUT_HISTORY: usize = 20;

fn shortcut_history_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::paths::config_dir()
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
    // Atomic and owner-only: the history holds the text the user typed,
    // and a torn file would parse as empty and lose all of it on the next
    // save (the read above falls back to an empty map).
    Ok(crate::atomic_write::write(
        &path,
        serde_json::to_string_pretty(&history).unwrap_or_default(),
        crate::atomic_write::Perms::Private,
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
    policy: crate::config::PolicyKind,
    grant_type: Option<crate::config::GrantType>,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let gt = grant_type.unwrap_or_default();
    info!(
        "Updating tool policy: {} -> {} (grant: {})",
        tool_title,
        policy.as_str(),
        gt.as_str()
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
        .map(|t| (t.policy, t.grant_type));

    if let Some(tool) = config
        .tool_permissions
        .tools
        .iter_mut()
        .find(|t| t.title == tool_title)
    {
        tool.policy = policy;
        tool.grant_type = gt;
        tool.granted_at = timestamp;
    }
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    drop(config);

    // Log the transition. Allow is a grant; anything else is a
    // revoke-style change when the prior policy was Allow.
    let event = match policy {
        crate::config::PolicyKind::Allow => crate::permission_audit::AuditEvent::Granted {
            tool: tool_title,
            grant_type: gt,
            session_id: None,
            args_preview: None,
        },
        _ => {
            if let Some((prior_policy, prior_gt)) =
                prior.filter(|(p, _)| *p == crate::config::PolicyKind::Allow)
            {
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
        .map(|t| (t.policy, t.grant_type));

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
// Export writes one of two kinds of backup: everything (config, steering
// docs, extension data) encrypted with a passphrase, or an unencrypted
// file of allowlisted settings only. Import is the inverse — unwraps,
// merges with this machine's values, sanitises away device-local fields.
// See `src/config_export.rs` for the layout and the merge rules.

/// Suggested filename for the export dialog. Frontend uses this so
/// the date stamp matches Local time on the user's machine without
/// the JS having to format dates itself.
#[tauri::command]
pub async fn export_config_default_filename(encrypted: bool) -> String {
    crate::config_export::default_filename(if encrypted {
        crate::config_export::ExportMode::Encrypted
    } else {
        crate::config_export::ExportMode::SettingsOnly
    })
}

/// Installed extensions and themes, whose manifests say which of their
/// data is sensitive, each tagged with the kind of folder it was found in.
/// Blocking (reads every manifest).
fn installed_items(
    states: &std::collections::HashMap<String, bool>,
) -> Vec<crate::extensions::InstalledItem> {
    // Every folder, even two claiming one id: each one's declarations count.
    ["extension", "theme"]
        .iter()
        .flat_map(|kind| crate::extensions::discover_all_items(kind, states))
        .collect()
}

/// What an unencrypted settings-only backup would leave out (steering
/// docs, shortcuts, agent setups, extension data, …), in plain words, and
/// how many settings it would hold. Labels only, never values. The Backup
/// UI lists it before the user saves one.
#[tauri::command]
pub async fn preview_config_export(
    features: State<'_, FeatureServices>,
) -> Result<crate::config_export::ExportPreview, AppError> {
    let config = {
        let cfg = features.config.lock_or_recover();
        cfg.clone()
    };
    tauri::async_runtime::spawn_blocking(move || {
        let installed = installed_items(&config.extension_states);
        crate::config_export::preview(&config, &installed)
    })
    .await
    .map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.config.export_task_failed",
            &[("message", &e.to_string())],
        )
    })?
    .map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.config.export_preview_failed",
            &[("message", &e.to_string())],
        )
    })
}

/// Build the backup bytes and write them to the user-picked path.
/// Returns the byte count so the UI can confirm a successful write.
/// We do the disk write here rather than handing bytes back over IPC
/// because a config + extension-data bundle can be tens of MB and
/// the round-trip is wasteful.
///
/// `mode` is `"encrypted"` (the default: everything, which needs a
/// passphrase that isn't blank) or `"settings_only"` (allowlisted settings,
/// unencrypted, no passphrase). There is no unencrypted full backup.
#[tauri::command]
pub async fn export_config_bundle(
    path: String,
    mode: Option<crate::config_export::ExportMode>,
    passphrase: Option<String>,
    features: State<'_, FeatureServices>,
) -> Result<u64, AppError> {
    let config = {
        let cfg = features.config.lock_or_recover();
        cfg.clone()
    };
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        let installed = installed_items(&config.extension_states);
        crate::config_export::export(
            &config,
            mode.unwrap_or_default(),
            passphrase.as_deref(),
            &installed,
        )
    })
    .await
    .map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.config.export_task_failed",
            &[("message", &e.to_string())],
        )
    })?
    .map_err(export_error)?;

    let len = bytes.len() as u64;
    std::fs::write(&path, &bytes).map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.config.export_write_failed",
            &[("path", &path), ("message", &e.to_string())],
        )
    })?;
    Ok(len)
}

/// The AppError for a failed `config_export::export`.
fn export_error(e: anyhow::Error) -> AppError {
    use crate::config_export::{BackupTooLarge, BlankPassphrase, PassphraseNeeded};
    if e.downcast_ref::<PassphraseNeeded>().is_some() {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.config.export_passphrase_required",
            &[],
        )
    } else if e.downcast_ref::<BlankPassphrase>().is_some() {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.config.export_passphrase_blank",
            &[],
        )
    } else if e.downcast_ref::<BackupTooLarge>().is_some() {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.config.export_too_large",
            &[
                ("max_mb", &megabytes(crate::config_export::MAX_ENTRY_BYTES)),
                (
                    "total_mb",
                    &megabytes(crate::config_export::MAX_TOTAL_BYTES),
                ),
            ],
        )
    } else {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.config.export_build_failed",
            &[("message", &e.to_string())],
        )
    }
}

fn megabytes(bytes: u64) -> String {
    (bytes / (1024 * 1024)).to_string()
}

/// The AppError for a failed `config_export::prepare_import` /
/// `apply_import`. A backup that would write outside Kage's folders, or is
/// too large, gets its own message (nothing was written), and so do the
/// passphrase cases the UI answers by asking (again).
fn import_error(e: anyhow::Error) -> AppError {
    use crate::config_export::{
        BackupTooLarge, ConfigTooLarge, ImportUnfinished, PassphraseRequired, UnsafeImportPath,
        UnsupportedConfigSchema, WrongPassphrase,
    };
    if let Some(unsafe_path) = e.downcast_ref::<UnsafeImportPath>() {
        log::warn!("import_config_bundle: refused: {}", unsafe_path);
        return AppError::keyed(
            ErrorKind::Internal,
            "errors.config.import_unsafe_path",
            &[("path", &unsafe_path.path)],
        );
    }
    if let Some(too_large) = e.downcast_ref::<BackupTooLarge>() {
        log::warn!("import_config_bundle: refused: {}", too_large);
        return AppError::keyed(
            ErrorKind::Internal,
            "errors.config.import_too_large",
            &[
                ("max_files", &crate::config_export::MAX_ENTRIES.to_string()),
                ("max_mb", &megabytes(crate::config_export::MAX_ENTRY_BYTES)),
                (
                    "total_mb",
                    &megabytes(crate::config_export::MAX_TOTAL_BYTES),
                ),
            ],
        );
    }
    if let Some(schema) = e.downcast_ref::<UnsupportedConfigSchema>() {
        log::warn!("import_config_bundle: refused: {}", schema);
        return if schema.newer {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.config.import_newer_config",
                &[],
            )
        } else {
            AppError::keyed(ErrorKind::Internal, "errors.config.import_old_config", &[])
        };
    }
    if e.downcast_ref::<ConfigTooLarge>().is_some() {
        return AppError::keyed(
            ErrorKind::Internal,
            "errors.config.import_config_too_large",
            &[("max_kb", &(Config::MAX_CONFIG_SIZE / 1024).to_string())],
        );
    }
    if e.downcast_ref::<ImportUnfinished>().is_some() {
        return AppError::keyed(ErrorKind::Internal, "errors.config.import_unfinished", &[]);
    }
    if e.downcast_ref::<PassphraseRequired>().is_some() {
        return AppError::keyed(
            ErrorKind::Internal,
            "errors.config.import_passphrase_required",
            &[],
        );
    }
    if e.downcast_ref::<WrongPassphrase>().is_some() {
        return AppError::keyed(
            ErrorKind::Internal,
            "errors.config.import_wrong_passphrase",
            &[],
        );
    }
    AppError::keyed(
        ErrorKind::Internal,
        "errors.config.import_build_failed",
        &[("message", &e.to_string())],
    )
}

/// What an import changes that the command applies after saving.
struct ImportOutcome {
    summary: crate::config_export::ImportSummary,
    prior_terminator: bool,
    prior_acp_mode: crate::config::AcpMode,
    new_log_buffer: usize,
    new_terminator: bool,
    new_language: Option<String>,
    new_acp_mode: crate::config::AcpMode,
    new_login_env: bool,
    auto_start: bool,
    /// Extensions whose data the import wrote: their instances re-read.
    data_extensions: Vec<String>,
}

/// Apply a backup bundle from disk. Returns the import summary (counts)
/// so the UI can render a concrete success toast. The new config is
/// written to disk + the in-memory state is replaced; a `config_updated`
/// Tauri event lets every window reload theme / shortcuts / etc.
#[tauri::command]
pub async fn import_config_bundle<R: tauri::Runtime>(
    path: String,
    passphrase: Option<String>,
    features: State<'_, FeatureServices>,
    acp: State<'_, AcpHandles>,
    app: tauri::AppHandle<R>,
) -> Result<crate::config_export::ImportSummary, AppError> {
    // Capped before it's read: a huge file is refused without filling memory.
    let bytes =
        crate::config_export::read_backup_file(std::path::Path::new(&path)).map_err(|e| {
            if e.downcast_ref::<crate::config_export::BackupTooLarge>()
                .is_some()
            {
                import_error(e)
            } else {
                AppError::keyed(
                    ErrorKind::Internal,
                    "errors.config.import_read_failed",
                    &[("path", &path), ("message", &e.to_string())],
                )
            }
        })?;
    // config.json on disk is from a newer Kage and is being preserved, so
    // nothing this import changes could be saved with it.
    if Config::saves_suspended() {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.config.import_local_newer",
            &[],
        ));
    }
    let states = features.config.lock_or_recover().extension_states.clone();
    let config = features.config.clone();

    // Decryption (Argon2's intentionally slow KDF) and unpacking happen off
    // the runtime and outside the config lock. The installed manifests are
    // read here too, so what an import may switch on can be checked against
    // this machine's grants and what's actually on disk, and the summary can
    // name extensions to approve or sign in to again. Then, under the config
    // lock held from reading this machine's config until the merged one is
    // saved, the backup is merged and its files written: a save made
    // meanwhile (a permission decision, a setting) can't be lost, and if
    // saving fails the files are put back.
    let outcome =
        tauri::async_runtime::spawn_blocking(move || -> Result<ImportOutcome, AppError> {
            let items = installed_items(&states);
            let prepared = crate::config_export::prepare_import(bytes, passphrase.as_deref())
                .map_err(import_error)?;

            let mut state_config = config.lock_or_recover();
            let local = state_config.clone();
            let (mut new_config, summary, mut files) =
                crate::config_export::apply_import(prepared, &local, &items)
                    .map_err(import_error)?
                    .into_parts();
            // Grants are this machine's whole, and no extension is switched on
            // that its grant here doesn't cover: `apply_import` enforces that
            // for every kind of backup (`config_export::consent`). Like grants,
            // category-wide tool trust stays this machine's.
            keep_local_tool_trust(&mut new_config, &local);
            if let Err(e) = crate::config_export::check_config_size(&new_config) {
                files.rollback();
                return Err(import_error(e));
            }
            let outcome = ImportOutcome {
                summary,
                prior_terminator: local.tool_permissions.terminator_mode,
                prior_acp_mode: local.acp.active_mode(),
                new_log_buffer: new_config.system.log_buffer_size,
                new_terminator: new_config.tool_permissions.terminator_mode,
                new_language: new_config.ui.language.clone(),
                new_acp_mode: new_config.acp.active_mode(),
                new_login_env: new_config.acp.agent.import_login_shell_env,
                auto_start: new_config.system.auto_start,
                data_extensions: files.data_extensions().to_vec(),
            };
            *state_config = new_config;
            // Channel is a typed enum — unknown values from a hand-edited
            // backup collapsed to Stable at deserialise time.
            if let Err(e) = files.save_config(&state_config) {
                *state_config = local;
                files.rollback();
                return Err(AppError::keyed(
                    ErrorKind::Internal,
                    "errors.config.persist_failed",
                    &[("message", &e.to_string())],
                ));
            }
            if let Err(e) = files.finish() {
                // The files were put back: put the config back too.
                *state_config = local;
                let _ = state_config.save();
                return Err(AppError::keyed(
                    ErrorKind::Internal,
                    "errors.config.persist_failed",
                    &[("message", &e.to_string())],
                ));
            }
            Ok(outcome)
        })
        .await
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.config.import_task_failed",
                &[("message", &e.to_string())],
            )
        })??;
    let ImportOutcome {
        summary,
        prior_terminator,
        prior_acp_mode,
        new_log_buffer,
        new_terminator,
        new_language,
        new_acp_mode,
        new_login_env,
        auto_start,
        data_extensions,
    } = outcome;

    apply_runtime_config(
        &app,
        &acp,
        &prior_acp_mode,
        &new_acp_mode,
        new_language,
        new_login_env,
    );

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
    crate::os::set_startup_enabled(auto_start);

    let _ = app.emit(
        crate::events::CONFIG_UPDATED,
        crate::commands::i18n::ConfigUpdatedI18n::current(),
    );
    // The import is committed: the open instances of each extension whose
    // data it wrote re-read everything (EXTENSION_VIEWS §20).
    for id in &data_extensions {
        crate::extension_data_events::record_bulk(
            &app,
            id,
            crate::extension_data_events::ChangeSource::Import,
        );
    }
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
    std::fs::write(&path, contents.as_bytes()).map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.config.write_file_failed",
            &[("path", &path), ("message", &e.to_string())],
        )
    })?;
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_key_path(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kage-frecency-key-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("kage").join("search-frecency.key")
    }

    #[test]
    fn frecency_key_is_created_once_and_reused() {
        use base64::Engine;
        let path = scratch_key_path("reuse");
        let first = load_or_create_frecency_key(&path).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&first)
            .unwrap();
        assert_eq!(bytes.len(), FRECENCY_KEY_LEN);
        assert_eq!(load_or_create_frecency_key(&path).unwrap(), first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "key file must be owner-only");
        }
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn malformed_frecency_key_is_replaced() {
        let path = scratch_key_path("malformed");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not base64 / too short").unwrap();
        let key = load_or_create_frecency_key(&path).unwrap();
        assert_ne!(key, "not base64 / too short");
        assert_eq!(load_or_create_frecency_key(&path).unwrap(), key);
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    fn grant(caps: &[&str]) -> crate::config::ExtensionGrant {
        crate::config::ExtensionGrant {
            granted: caps.iter().map(|s| s.to_string()).collect(),
            approved_version: "1.0.0".into(),
            approved_at: "2026-01-01T00:00:00Z".into(),
            origin: None,
        }
    }

    #[test]
    fn import_refusing_an_unsafe_path_gets_its_own_error() {
        let refused = anyhow::Error::new(crate::config_export::UnsafeImportPath {
            path: "extension-data/ext/CON".into(),
            reason: "disallowed path component".into(),
        });
        let e = import_error(refused);
        assert_eq!(e.key, "errors.config.import_unsafe_path");
        assert_eq!(
            e.params.get("path").map(String::as_str),
            Some("extension-data/ext/CON")
        );

        for (err, key) in [
            (
                anyhow::Error::new(crate::config_export::PassphraseRequired),
                "errors.config.import_passphrase_required",
            ),
            (
                anyhow::Error::new(crate::config_export::WrongPassphrase),
                "errors.config.import_wrong_passphrase",
            ),
            (
                anyhow::Error::new(crate::config_export::BackupTooLarge {
                    what: "test".into(),
                }),
                "errors.config.import_too_large",
            ),
            (
                anyhow::Error::new(crate::config_export::UnsupportedConfigSchema {
                    stored: 9,
                    newer: true,
                }),
                "errors.config.import_newer_config",
            ),
            (
                anyhow::Error::new(crate::config_export::UnsupportedConfigSchema {
                    stored: 0,
                    newer: false,
                }),
                "errors.config.import_old_config",
            ),
            (
                anyhow::Error::new(crate::config_export::ImportUnfinished),
                "errors.config.import_unfinished",
            ),
            (
                anyhow::Error::new(crate::config_export::ConfigTooLarge),
                "errors.config.import_config_too_large",
            ),
        ] {
            assert_eq!(import_error(err).key, key);
        }
        for (err, key) in [
            (
                anyhow::Error::new(crate::config_export::BlankPassphrase),
                "errors.config.export_passphrase_blank",
            ),
            (
                anyhow::Error::new(crate::config_export::PassphraseNeeded),
                "errors.config.export_passphrase_required",
            ),
            (
                anyhow::Error::new(crate::config_export::BackupTooLarge {
                    what: "test".into(),
                }),
                "errors.config.export_too_large",
            ),
            (anyhow::anyhow!("x"), "errors.config.export_build_failed"),
        ] {
            assert_eq!(export_error(err).key, key);
        }

        let other = import_error(anyhow::anyhow!("Backup is missing config.json"));
        assert_eq!(other.key, "errors.config.import_build_failed");
        assert_eq!(
            other.params.get("message").map(String::as_str),
            Some("Backup is missing config.json")
        );
    }

    #[test]
    fn stale_snapshot_cannot_wipe_extension_grants() {
        // The lost-update bug: a window fetched config before grants were
        // written, then saved an unrelated change — its empty grants map
        // must not clobber the backend's.
        let mut authoritative = Config::default();
        authoritative
            .extension_grants
            .insert("spotify".into(), grant(&["storage", "oauth"]));

        let mut incoming = Config::default(); // stale: no grants
        preserve_backend_owned_fields(&mut incoming, &authoritative);

        assert_eq!(incoming.extension_grants.len(), 1);
        assert_eq!(
            incoming.extension_grants.get("spotify").unwrap().granted,
            vec!["storage", "oauth"]
        );
    }

    #[test]
    fn snapshot_cannot_expand_grants_either() {
        // Symmetric direction: JS must not be able to GRANT capabilities
        // by round-tripping an inflated map through save_config — grants
        // only ever change through the dedicated install commands.
        let authoritative = Config::default(); // no grants
        let mut incoming = Config::default();
        incoming
            .extension_grants
            .insert("evil".into(), grant(&["storage", "urls", "oauth"]));

        preserve_backend_owned_fields(&mut incoming, &authoritative);
        assert!(incoming.extension_grants.is_empty());
    }

    #[test]
    fn matching_grants_pass_through_untouched() {
        let mut authoritative = Config::default();
        authoritative
            .extension_grants
            .insert("todos".into(), grant(&["storage"]));
        let mut incoming = authoritative.clone();

        preserve_backend_owned_fields(&mut incoming, &authoritative);
        assert_eq!(incoming.extension_grants, authoritative.extension_grants);
    }

    fn agent_access(enabled: bool, consent: bool) -> crate::config::ExtensionAgentAccess {
        crate::config::ExtensionAgentAccess {
            enabled,
            consent: consent.then(|| crate::config::AgentConsent {
                agent_fingerprint: "ab12".into(),
                agent_label: "Work agent".into(),
                granted_at: "2026-11-09T10:00:00+00:00".into(),
            }),
            allowance: crate::config::AgentAllowance::Normal,
        }
    }

    #[test]
    fn stale_snapshot_cannot_switch_background_ai_back_on() {
        // A Settings window fetched its config while background AI was on,
        // then the user switched it off (which also clears the consent).
        // The window's next unrelated save must not switch it back on.
        let mut authoritative = Config::default();
        authoritative
            .extension_agent_access
            .insert("mindmap".into(), agent_access(false, false));
        let mut stale = Config::default();
        stale
            .extension_agent_access
            .insert("mindmap".into(), agent_access(true, true));
        stale.ui.theme = "dark".into();

        preserve_backend_owned_fields(&mut stale, &authoritative);
        assert_eq!(
            stale.extension_agent_access,
            authoritative.extension_agent_access
        );
        assert!(!stale.extension_agent_access["mindmap"].enabled);
        assert!(stale.extension_agent_access["mindmap"].consent.is_none());
        assert_eq!(stale.ui.theme, "dark", "the unrelated change still saves");

        // Turning off can also remove the entry: a stale "on" entry stays gone.
        let authoritative = Config::default();
        let mut stale = Config::default();
        stale
            .extension_agent_access
            .insert("mindmap".into(), agent_access(true, true));
        preserve_backend_owned_fields(&mut stale, &authoritative);
        assert!(stale.extension_agent_access.is_empty());
    }

    #[test]
    fn snapshot_cannot_turn_background_ai_on_or_off_either_way() {
        // JS can't grant itself access or a bigger allowance…
        let mut authoritative = Config::default();
        authoritative
            .extension_agent_access
            .insert("mindmap".into(), agent_access(true, true));
        let mut incoming = authoritative.clone();
        incoming
            .extension_agent_access
            .get_mut("mindmap")
            .unwrap()
            .allowance = crate::config::AgentAllowance::High;
        incoming
            .extension_agent_access
            .insert("other".into(), agent_access(true, true));
        preserve_backend_owned_fields(&mut incoming, &authoritative);
        assert_eq!(
            incoming.extension_agent_access,
            authoritative.extension_agent_access
        );

        // …nor drop what the user turned on after the snapshot was taken.
        let mut stale = Config::default();
        preserve_backend_owned_fields(&mut stale, &authoritative);
        assert_eq!(
            stale.extension_agent_access,
            authoritative.extension_agent_access
        );

        // A matching snapshot passes through untouched.
        let mut same = authoritative.clone();
        preserve_backend_owned_fields(&mut same, &authoritative);
        assert_eq!(
            same.extension_agent_access,
            authoritative.extension_agent_access
        );
    }

    #[test]
    fn snapshot_cannot_grant_or_revoke_command_trust() {
        // "Trust all commands" changes only through set_tool_kind_trusted.
        let authoritative = Config::default();
        let mut incoming = Config::default();
        incoming.tool_permissions.trusted_kinds = vec!["execute".into()];
        preserve_backend_owned_fields(&mut incoming, &authoritative);
        assert!(incoming.tool_permissions.trusted_kinds.is_empty());

        let mut trusted = Config::default();
        trusted.tool_permissions.trusted_kinds = vec!["execute".into()];
        let mut stale = Config::default(); // fetched before the user trusted
        preserve_backend_owned_fields(&mut stale, &trusted);
        assert_eq!(
            stale.tool_permissions.trusted_kinds,
            vec!["execute".to_string()]
        );
    }

    #[test]
    fn snapshot_cannot_grant_or_revoke_mcp_server_trust() {
        // "Trust all tools from <server>" changes only through
        // set_mcp_server_trusted.
        let authoritative = Config::default();
        let mut incoming = Config::default();
        incoming.tool_permissions.trusted_mcp_servers = vec!["builder-mcp".into()];
        preserve_backend_owned_fields(&mut incoming, &authoritative);
        assert!(incoming.tool_permissions.trusted_mcp_servers.is_empty());

        let mut trusted = Config::default();
        trusted.tool_permissions.trusted_mcp_servers = vec!["builder-mcp".into()];
        let mut stale = Config::default(); // fetched before the user trusted
        preserve_backend_owned_fields(&mut stale, &trusted);
        assert_eq!(
            stale.tool_permissions.trusted_mcp_servers,
            vec!["builder-mcp".to_string()]
        );
    }

    #[test]
    fn snapshot_and_import_cannot_change_host_command_rules() {
        let rule = crate::config::HostCommandRule {
            prefix: "brew install".into(),
            created_at: String::new(),
        };
        let authoritative = Config::default();
        let mut incoming = Config::default();
        incoming.tool_permissions.host_command_rules = vec![rule.clone()];
        incoming.tool_permissions.host_commands_allow_all = true;
        preserve_backend_owned_fields(&mut incoming, &authoritative);
        assert!(incoming.tool_permissions.host_command_rules.is_empty());
        assert!(!incoming.tool_permissions.host_commands_allow_all);

        let mut local = Config::default();
        local.tool_permissions.host_command_rules = vec![rule.clone()];
        let mut stale = Config::default();
        preserve_backend_owned_fields(&mut stale, &local);
        assert_eq!(
            stale.tool_permissions.host_command_rules,
            vec![rule.clone()]
        );

        let mut imported = Config::default();
        imported.tool_permissions.host_commands_allow_all = true;
        keep_local_tool_trust(&mut imported, &local);
        assert!(!imported.tool_permissions.host_commands_allow_all);
        assert_eq!(imported.tool_permissions.host_command_rules, vec![rule]);
    }

    #[test]
    fn import_keeps_this_machines_tool_trust() {
        // A backup trusting a server / every command must not grant that
        // here…
        let local = Config::default();
        let mut imported = Config::default();
        imported.tool_permissions.trusted_kinds = vec!["execute".into()];
        imported.tool_permissions.trusted_mcp_servers = vec!["evil-mcp".into()];
        keep_local_tool_trust(&mut imported, &local);
        assert!(imported.tool_permissions.trusted_kinds.is_empty());
        assert!(imported.tool_permissions.trusted_mcp_servers.is_empty());

        // …and a backup from before the user trusted one must not revoke it.
        let mut local = Config::default();
        local.tool_permissions.trusted_kinds = vec!["execute".into()];
        local.tool_permissions.trusted_mcp_servers = vec!["builder-mcp".into()];
        let mut imported = Config::default();
        keep_local_tool_trust(&mut imported, &local);
        assert_eq!(
            imported.tool_permissions.trusted_kinds,
            vec!["execute".to_string()]
        );
        assert_eq!(
            imported.tool_permissions.trusted_mcp_servers,
            vec!["builder-mcp".to_string()]
        );
    }
}
