//! The Tauri commands behind the prompt ("Run once", "Always allow commands
//! starting with …", "Deny") and Settings → Tool permissions ("Commands
//! outside the agent's sandbox"). None of them is reachable from
//! extensions (`extension-permissions.js` maps each to `null`).

use super::approvals::{Answer, AnswerError};
use super::rules;
use super::server::AuditStep;
use crate::error::{AppError, ErrorKind};
use crate::events;
use crate::lock_ext::LockExt;
use crate::state::{ChildProcesses, FeatureServices};
use crate::window_labels;
use log::{info, warn};
use tauri::{Emitter, State, WebviewWindow};

fn err(kind: ErrorKind, key: &str) -> AppError {
    AppError::keyed(kind, key, &[])
}

fn not_pending(e: AnswerError) -> AppError {
    match e {
        AnswerError::NotPending => err(ErrorKind::InvalidInput, "errors.host_command.not_pending"),
        AnswerError::WrongWindow => {
            err(ErrorKind::NotPermitted, "errors.host_command.wrong_window")
        }
    }
}

fn save_failed(e: impl std::fmt::Display) -> AppError {
    AppError::keyed(
        ErrorKind::Internal,
        "errors.config.save_failed",
        &[("reason", &e.to_string())],
    )
}

/// Answer the prompt for request `request_id`: `run_once`, `always` (with
/// the rule's `prefix`, which must cover this command) or `deny`. Only the
/// window the prompt was shown in may answer it, once. A command with
/// hidden characters can't be run from here at all (the prompt offers Copy
/// only); one the guard flagged, or that no rule could cover, can't get
/// "Always allow".
///
/// `request_id` and `decision` stay required: the mock-app sweep dispatches
/// every command with empty args, which must fail at deserialization.
#[tauri::command]
pub async fn answer_host_command<R: tauri::Runtime>(
    request_id: String,
    decision: String,
    prefix: Option<String>,
    window: WebviewWindow<R>,
    app: tauri::AppHandle<R>,
    procs: State<'_, ChildProcesses>,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let label = window.label().to_string();
    if !crate::commands::system::user_shell::caller_allowed(&label) {
        return Err(err(
            ErrorKind::NotPermitted,
            "errors.host_command.wrong_window",
        ));
    }
    let approvals = &procs.host_commands.approvals;
    let info = approvals
        .info_for(&request_id, &label)
        .map_err(not_pending)?;
    let answer = match decision.as_str() {
        "deny" => Answer::Deny,
        "run_once" | "always" if !info.plain => {
            return Err(err(
                ErrorKind::NotPermitted,
                "errors.host_command.hidden_chars",
            ));
        }
        "run_once" => Answer::RunOnce,
        "always" => {
            if !info.can_always {
                return Err(err(
                    ErrorKind::NotPermitted,
                    "errors.host_command.cannot_always",
                ));
            }
            let prefix = rules::normalize_prefix(prefix.as_deref().unwrap_or(""))
                .map_err(|_| err(ErrorKind::InvalidInput, "errors.host_command.bad_prefix"))?;
            if !rules::rule_matches(&prefix, &info.command) {
                return Err(err(
                    ErrorKind::InvalidInput,
                    "errors.host_command.prefix_mismatch",
                ));
            }
            add_rule(&features, &prefix)?;
            AuditStep {
                action: "rule_added".into(),
                request_id: Some(request_id.clone()),
                rule: Some(prefix.clone()),
                ..AuditStep::default()
            }
            .write();
            if let Err(e) = app.emit(events::CONFIG_UPDATED, ()) {
                warn!("Failed to emit config_updated: {e}");
            }
            Answer::Always(prefix)
        }
        _ => {
            return Err(err(
                ErrorKind::InvalidInput,
                "errors.host_command.bad_decision",
            ))
        }
    };
    approvals
        .answer(&request_id, &label, answer)
        .map_err(not_pending)
}

/// Save a rule (if it isn't there already); rolled back if it can't be
/// saved, so memory and disk agree.
fn add_rule(features: &FeatureServices, prefix: &str) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    let rules = &mut config.tool_permissions.host_command_rules;
    if rules.iter().any(|r| r.prefix == prefix) {
        return Ok(());
    }
    if rules.len() >= crate::config::MAX_HOST_COMMAND_RULES {
        return Err(err(
            ErrorKind::InvalidInput,
            "errors.host_command.too_many_rules",
        ));
    }
    rules.push(crate::config::HostCommandRule {
        prefix: prefix.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
    });
    if let Err(e) = config.save() {
        config
            .tool_permissions
            .host_command_rules
            .retain(|r| r.prefix != prefix);
        return Err(save_failed(e));
    }
    info!("Added a rule for commands outside the agent's sandbox: '{prefix}'");
    Ok(())
}

/// The prompts waiting in the calling window, for a page that reloaded to
/// show again (a prompt nobody answers is denied after five minutes).
#[tauri::command]
pub async fn pending_host_commands<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    procs: State<'_, ChildProcesses>,
) -> Result<Vec<serde_json::Value>, AppError> {
    let label = window.label();
    if !crate::commands::system::user_shell::caller_allowed(label) {
        return Ok(Vec::new());
    }
    Ok(procs.host_commands.approvals.payloads_for(label))
}

/// Remove the rule `prefix` (Settings). Resolves to whether there was one.
#[tauri::command]
pub async fn remove_host_command_rule<R: tauri::Runtime>(
    prefix: String,
    app: tauri::AppHandle<R>,
    features: State<'_, FeatureServices>,
) -> Result<bool, AppError> {
    {
        let mut config = features.config.lock_or_recover();
        let prior = config.tool_permissions.host_command_rules.clone();
        config
            .tool_permissions
            .host_command_rules
            .retain(|r| r.prefix != prefix);
        if config.tool_permissions.host_command_rules.len() == prior.len() {
            return Ok(false);
        }
        if let Err(e) = config.save() {
            config.tool_permissions.host_command_rules = prior;
            return Err(save_failed(e));
        }
    }
    info!("Removed a rule for commands outside the agent's sandbox: '{prefix}'");
    AuditStep {
        action: "rule_removed".into(),
        rule: Some(crate::permission_audit::command_preview(&prefix)),
        ..AuditStep::default()
    }
    .write();
    if let Err(e) = app.emit(events::CONFIG_UPDATED, ()) {
        warn!("Failed to emit config_updated: {e}");
    }
    Ok(true)
}

/// Turn "run every command without asking" on or off (Settings only;
/// turning it on is refused from any other window). The credential guard
/// and hidden characters still ask either way.
#[tauri::command]
pub async fn set_host_commands_allow_all<R: tauri::Runtime>(
    enabled: bool,
    window: WebviewWindow<R>,
    app: tauri::AppHandle<R>,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    if enabled && window.label() != window_labels::SETTINGS {
        return Err(err(
            ErrorKind::NotPermitted,
            "errors.host_command.allow_all_settings_only",
        ));
    }
    {
        let mut config = features.config.lock_or_recover();
        if config.tool_permissions.host_commands_allow_all == enabled {
            return Ok(());
        }
        config.tool_permissions.host_commands_allow_all = enabled;
        if let Err(e) = config.save() {
            config.tool_permissions.host_commands_allow_all = !enabled;
            return Err(save_failed(e));
        }
    }
    info!("Commands outside the agent's sandbox: allow-all {enabled}");
    AuditStep {
        action: if enabled {
            "allow_all_on"
        } else {
            "allow_all_off"
        }
        .into(),
        ..AuditStep::default()
    }
    .write();
    if let Err(e) = app.emit(events::CONFIG_UPDATED, ()) {
        warn!("Failed to emit config_updated: {e}");
    }
    Ok(())
}
