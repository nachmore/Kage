//! Steering message construction (built-in + user + auto) and the
//! line-editor IO commands the Personalization settings page uses.
//! Plus `match_context_rule`, the App-Modes lookup that gets spliced
//! into the prompt when the floating window's foreground app matches
//! a configured context rule.

use crate::config::Config;
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::state::FeatureServices;
use log::{error, info, warn};
use std::fs;
use tauri::{Emitter, State};

/// Re-export steering constants from auto_steering (the canonical location).
pub use crate::auto_steering::{BUILTIN_STEERING, STEERING_MSG_PREFIX};

/// The config fields `assemble_steering_parts` needs. Take this by
/// value so the caller can extract it under the config lock and drop
/// the guard before the disk reads happen.
#[derive(Debug, Clone)]
pub struct SteeringInputs {
    pub user_steering_path: Option<String>,
    pub auto_steering_enabled: bool,
}

impl SteeringInputs {
    pub fn from_config(config: &Config) -> Self {
        let assistant = &config.acp.agent;
        Self {
            user_steering_path: assistant.user_steering_path.clone(),
            auto_steering_enabled: assistant.auto_steering_enabled,
        }
    }
}

/// Assemble the full steering content from builtin + user + auto sources.
/// Returns the joined parts (without the STEERING_MSG_PREFIX wrapper).
/// Callers are responsible for adding the prefix and any instructions.
///
/// Takes plain inputs so the config lock can be released before this
/// runs — the user / auto steering files are blocking disk reads, and
/// holding the global config Mutex across them would block every
/// concurrent `lock_or_recover()` site for the duration of the read.
pub fn assemble_steering_parts(inputs: &SteeringInputs) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();

    // Built-in steering (always first)
    parts.push(BUILTIN_STEERING.to_string());

    // Current date and time — so the LLM knows the actual date for relative queries
    let now = chrono::Local::now();
    parts.push(format!(
        "<current_datetime>\nCurrent date and time: {}\nTimezone: {}\n</current_datetime>",
        now.format("%A, %B %e, %Y %I:%M %p"),
        now.format("%Z"),
    ));

    // User-written steering doc
    if let Some(ref path) = inputs.user_steering_path {
        if !path.is_empty() {
            match fs::read_to_string(path) {
                Ok(content) if !content.trim().is_empty() => {
                    info!("Loaded user steering doc from: {}", path);
                    parts.push(content);
                }
                Ok(_) => {}
                Err(e) => error!("Failed to read user steering doc {}: {}", path, e),
            }
        }
    }

    // Auto-generated steering doc
    if inputs.auto_steering_enabled {
        match Config::get_auto_steering_path() {
            Ok(auto_path) => {
                if auto_path.exists() {
                    match fs::read_to_string(&auto_path) {
                        Ok(content) if !content.trim().is_empty() => {
                            parts.push(content);
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => error!("Failed to get auto steering path: {}", e),
        }
    }

    parts
}

/// Format assembled steering parts into a complete steering message
/// with the prefix and ack instruction.
pub fn format_steering_message(parts: &[String]) -> String {
    format!(
        "{} {}\n\n---\n\n<instructions>Respond with only \"ack\" to confirm receipt. Do not summarize or comment on the content above.</instructions>",
        STEERING_MSG_PREFIX,
        parts.join("\n\n---\n\n")
    )
}

/// Build the combined steering content from user and auto-generated docs.
/// User steering takes precedence (placed first).
/// Returns None if no steering content is available.
#[tauri::command]
pub async fn get_steering_content(
    features: State<'_, FeatureServices>,
) -> Result<Option<String>, AppError> {
    // Snapshot the relevant fields under the lock; release before
    // reading user / auto steering files from disk.
    let inputs = SteeringInputs::from_config(&features.config.lock_or_recover());
    let parts = assemble_steering_parts(&inputs);
    Ok(Some(format_steering_message(&parts)))
}

/// Open the auto-generated steering document in the default editor.
/// Creates the file with a header comment if it doesn't exist yet.
#[tauri::command]
pub async fn open_auto_steering_file() -> Result<String, AppError> {
    let auto_path = Config::get_auto_steering_path()
        .map_err(|e| format!("Failed to get auto steering path: {}", e))?;

    // Create with header if it doesn't exist
    if !auto_path.exists() {
        if let Some(parent) = auto_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
        }
        let header = "<!-- AUTO-GENERATED STEERING DOCUMENT\n     Any manual changes will be overridden the next time this document is generated.\n     To add your own persistent instructions, use a User Steering Document instead. -->\n\n";
        fs::write(&auto_path, header)
            .map_err(|e| format!("Failed to create auto steering file: {}", e))?;
    }

    let path_str = auto_path
        .to_str()
        .ok_or_else(|| "Invalid path encoding".to_string())?
        .to_string();

    // Open in default editor
    crate::os::open_in_editor(&path_str).map_err(|e| format!("Failed to open file: {}", e))?;

    Ok(path_str)
}

/// Get the path to the auto-generated steering document
#[tauri::command]
pub async fn get_auto_steering_path() -> Result<String, AppError> {
    Ok(Config::get_auto_steering_path()
        .map_err(|e| format!("Failed to get path: {}", e))
        .and_then(|p| {
            p.to_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "Invalid path encoding".to_string())
        })?)
}

// --- Line-editor IO for the steering documents ----------------------
//
// The Personalization settings page edits the auto and user steering
// docs as line lists. Read returns the resolved path so the UI can
// render "we'll save this to …" + a Reveal-in-Explorer affordance.
// Write updates `user_steering_path` in config the first time a user
// saves a non-empty doc with no path configured — that pins the
// default location into the user's settings so future installs find
// it.

fn parse_steering_kind(kind: &str) -> Result<crate::steering_io::SteeringKind, AppError> {
    crate::steering_io::SteeringKind::parse(kind).ok_or_else(|| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.steering.unknown_kind",
            &[("kind", kind)],
        )
    })
}

fn path_to_string(p: &std::path::Path) -> Result<String, AppError> {
    p.to_str().map(|s| s.to_string()).ok_or_else(|| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.steering.invalid_path_encoding",
            &[],
        )
    })
}

#[tauri::command]
pub async fn read_steering_lines(
    kind: String,
    features: State<'_, FeatureServices>,
) -> Result<serde_json::Value, AppError> {
    let kind = parse_steering_kind(&kind)?;
    let config = features.config.lock_or_recover();
    let (path, lines) = crate::steering_io::read_lines(kind, &config)
        .map_err(|e| format!("Failed to read steering doc: {}", e))?;
    Ok(serde_json::json!({
        "path": path_to_string(&path)?,
        "lines": lines,
        "exists": path.exists(),
    }))
}

#[tauri::command]
pub async fn write_steering_lines(
    kind: String,
    lines: Vec<String>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, AppError> {
    let kind = parse_steering_kind(&kind)?;

    // Capture whether the user had no explicit user_steering_path
    // configured before this write — that's the trigger for pinning
    // the default location into config so subsequent reads agree.
    let needs_path_persist = matches!(kind, crate::steering_io::SteeringKind::User) && {
        let cfg = features.config.lock_or_recover();
        cfg.acp
            .agent
            .user_steering_path
            .as_deref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
    };

    let path = {
        let config = features.config.lock_or_recover();
        crate::steering_io::write_lines(kind, &config, &lines)
            .map_err(|e| format!("Failed to write steering doc: {}", e))?
    };

    if needs_path_persist {
        let mut config = features.config.lock_or_recover();
        config.acp.agent.user_steering_path = Some(path_to_string(&path)?);
        if let Err(e) = config.save() {
            warn!("Failed to persist user_steering_path: {}", e);
        }
        // Drop the lock before emitting so listeners that re-read
        // config don't deadlock.
        drop(config);
        let _ = app.emit(crate::events::CONFIG_UPDATED, ());
    }

    Ok(serde_json::json!({
        "path": path_to_string(&path)?,
    }))
}

/// Read an arbitrary file path the user picked via the file dialog.
/// Returns its lines without touching the on-disk steering doc — the
/// frontend decides whether to merge with existing content or
/// replace, and writes via `write_steering_lines`.
#[tauri::command]
pub async fn import_steering_lines(path: String) -> Result<Vec<String>, AppError> {
    crate::steering_io::import_lines_from_path(&path).map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.steering.import_failed",
            &[("message", &e.to_string())],
        )
    })
}

// --- App Modes / context rules --------------------------------------
//
// The floating window's send path calls `match_context_rule` with the
// foreground process name (already captured for the `<_kage_ctx>`
// tag). On a hit we return both the formatted `<_kage_app_steering>`
// payload (ready to splice into the prompt) and the rule's friendly
// name so the chip in the input bar can show "🎯 VS Code mode" before
// the user sends. Returning `None` is fine — the caller skips the
// injection entirely. See `src/context_rules.rs` for the matcher.

#[derive(Debug, Clone, serde::Serialize)]
pub struct MatchedContextRule {
    pub friendly_name: String,
    pub steering_payload: String,
}

#[tauri::command]
pub async fn match_context_rule(
    executable: String,
    features: State<'_, FeatureServices>,
) -> Result<Option<MatchedContextRule>, AppError> {
    let cfg = features.config.lock_or_recover();
    let Some(rule) = crate::context_rules::first_matching(&cfg.context_rules, &executable) else {
        return Ok(None);
    };
    let Some(payload) = crate::context_rules::format_steering_payload(rule) else {
        // Rule matched but its steering body is empty — treat as a
        // no-op so the chip doesn't claim a mode that contributes
        // nothing to the prompt.
        return Ok(None);
    };
    Ok(Some(MatchedContextRule {
        friendly_name: rule.friendly_name.clone(),
        steering_payload: payload,
    }))
}
