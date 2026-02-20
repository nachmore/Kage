use crate::state::AppState;
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tauri::State;

/// Summary of a session for the sidebar list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A single message in a session conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub kind: String,       // "Prompt", "AssistantMessage", "ToolResults"
    pub message_id: String,
    pub content: Vec<MessageContent>,
}

/// Content item within a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageContent {
    pub kind: String, // "text", "toolUse", "toolResult", "json"
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Full session data returned when loading a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<SessionMessage>,
}

/// Get the sessions directory: [home]/.kiro/sessions/cli
fn get_sessions_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Failed to get home directory".to_string())?;
    Ok(home.join(".kiro").join("sessions").join("cli"))
}

fn get_title_cache_path() -> Result<PathBuf, String> {
    let dir = get_sessions_dir()?;
    Ok(dir.join(".title-cache.json"))
}

fn load_title_cache() -> HashMap<String, String> {
    get_title_cache_path()
        .ok()
        .and_then(|p| fs::read_to_string(&p).ok())
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_title_cache(cache: &HashMap<String, String>) {
    if let Ok(path) = get_title_cache_path() {
        if let Ok(content) = serde_json::to_string(cache) {
            let _ = fs::write(&path, content);
        }
    }
}

/// Extract a title from the JSONL — use the first user prompt text
/// Skips steering messages (prefixed with STEERING_MSG_PREFIX)
fn extract_title_from_jsonl(jsonl_path: &std::path::Path) -> String {
    // Read only the first few KB to find the title — JSONL files can be huge
    use std::io::{BufRead, BufReader};

    let file = match fs::File::open(jsonl_path) {
        Ok(f) => f,
        Err(_) => return "New Chat".to_string(),
    };

    let reader = BufReader::new(file);

    for line in reader.lines().take(10) {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
            if val.get("kind").and_then(|k| k.as_str()) == Some("Prompt") {
                if let Some(content_arr) = val
                    .get("data")
                    .and_then(|d| d.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for item in content_arr {
                        if item.get("kind").and_then(|k| k.as_str()) == Some("text") {
                            if let Some(text) = item.get("data").and_then(|d| d.as_str()) {
                                let trimmed = text.trim();
                                if trimmed.starts_with(crate::commands::system::STEERING_MSG_PREFIX) {
                                    continue;
                                }
                                if !trimmed.is_empty() {
                                    let title: String = trimmed.chars().take(60).collect();
                                    if title.len() < trimmed.len() {
                                        return format!("{}...", title);
                                    }
                                    return title;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    "New Chat".to_string()
}

/// Parse the JSONL file into a list of SessionMessages
fn parse_jsonl(jsonl_path: &std::path::Path) -> Vec<SessionMessage> {
    let mut messages = Vec::new();

    let content = match fs::read_to_string(jsonl_path) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to read JSONL {:?}: {}", jsonl_path, e);
            return messages;
        }
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to parse JSONL line: {}", e);
                continue;
            }
        };

        let kind = val
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();

        let data = val.get("data").cloned().unwrap_or(serde_json::Value::Null);

        let message_id = data
            .get("message_id")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();

        let content_arr = data
            .get("content")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();

        let content: Vec<MessageContent> = content_arr
            .into_iter()
            .map(|item| {
                let item_kind = item
                    .get("kind")
                    .and_then(|k| k.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let item_data = item.get("data").cloned().unwrap_or(serde_json::Value::Null);
                MessageContent {
                    kind: item_kind,
                    data: item_data,
                }
            })
            .collect();

        messages.push(SessionMessage {
            kind,
            message_id,
            content,
        });
    }

    messages
}

#[tauri::command]
pub async fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    let sessions_dir = get_sessions_dir()?;
    info!("Loading sessions from: {:?}", sessions_dir);

    if !sessions_dir.exists() {
        info!("Sessions directory does not exist yet: {:?}", sessions_dir);
        return Ok(vec![]);
    }

    let mut sessions: Vec<SessionSummary> = Vec::new();
    let mut title_cache = load_title_cache();
    let mut cache_dirty = false;

    let entries = fs::read_dir(&sessions_dir).map_err(|e| {
        error!("Failed to read sessions directory: {}", e);
        format!("Failed to read sessions directory: {}", e)
    })?;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();

        // Only process .json files (skip .jsonl, .lock, .title-cache.json)
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        // The session_id is the file stem (uuid)
        let session_id = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        // Skip the title cache file itself
        if session_id == ".title-cache" {
            continue;
        }

        // Get dates from file metadata (fast)
        let (created_at, updated_at) = match fs::metadata(&path) {
            Ok(meta) => {
                let updated = meta.modified().ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default())
                    .unwrap_or_default();
                let created = meta.created().ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default())
                    .unwrap_or_default();
                (created, updated)
            }
            Err(_) => (String::new(), String::new()),
        };

        // Use cached title if available, otherwise extract and cache
        let title = if let Some(cached) = title_cache.get(&session_id) {
            cached.clone()
        } else {
            let jsonl_path = path.with_extension("jsonl");
            let extracted = extract_title_from_jsonl(&jsonl_path);
            // Only cache non-default titles (session has actual content)
            if extracted != "New Chat" {
                title_cache.insert(session_id.clone(), extracted.clone());
                cache_dirty = true;
            }
            extracted
        };

        sessions.push(SessionSummary {
            session_id,
            title,
            created_at,
            updated_at,
        });
    }

    // Persist cache if we added new entries
    if cache_dirty {
        save_title_cache(&title_cache);
    }

    // Sort by updated_at descending (most recent first)
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    info!("Found {} sessions", sessions.len());
    Ok(sessions)
}

#[tauri::command]
pub async fn load_session(session_id: String) -> Result<SessionData, String> {
    let sessions_dir = get_sessions_dir()?;
    let json_path = sessions_dir.join(format!("{}.json", session_id));
    let jsonl_path = sessions_dir.join(format!("{}.jsonl", session_id));

    info!("Loading session: {}", session_id);

    if !json_path.exists() {
        return Err(format!("Session not found: {}", session_id));
    }

    // Read metadata from .json
    let json_content = fs::read_to_string(&json_path).map_err(|e| {
        error!("Failed to read session JSON: {}", e);
        format!("Failed to read session: {}", e)
    })?;

    let metadata: serde_json::Value = serde_json::from_str(&json_content).map_err(|e| {
        error!("Failed to parse session JSON: {}", e);
        format!("Failed to parse session: {}", e)
    })?;

    let created_at = metadata
        .get("created_at")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let updated_at = metadata
        .get("updated_at")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Read messages from .jsonl
    let messages = if jsonl_path.exists() {
        parse_jsonl(&jsonl_path)
    } else {
        vec![]
    };

    Ok(SessionData {
        session_id,
        created_at,
        updated_at,
        messages,
    })
}


/// Switch the ACP client to a different session.
/// If session_id is provided, loads that session via session/load.
/// If session_id is None, creates a new session via session/new.
/// Saves the floating session before switching so it can be restored.
#[tauri::command]
pub async fn switch_acp_session(
    session_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let client = state.acp_client.clone();
    let client_guard = client.lock().await;

    // Ensure connected
    if !client_guard.is_connected() {
        info!("Not connected, attempting to connect for session switch...");
        if let Err(e) = client_guard.connect() {
            error!("Connection failed: {}", e);
            return Err(format!("Failed to connect: {}", e));
        }
    }

    // Save the current session as floating session if we don't have one yet
    {
        let mut floating = state
            .floating_session_id
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if floating.is_none() {
            *floating = client_guard.get_session_id();
        }
    }

    match session_id {
        Some(id) => {
            info!("Switching to existing session: {}", id);

            // Read the cwd from the session's .json metadata file
            let cwd = {
                let sessions_dir = get_sessions_dir()?;
                let json_path = sessions_dir.join(format!("{}.json", id));
                if json_path.exists() {
                    fs::read_to_string(&json_path)
                        .ok()
                        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
                        .and_then(|data| {
                            data.get("cwd")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        })
                } else {
                    None
                }
            };

            client_guard
                .load_existing_session(&id, cwd)
                .map_err(|e| format!("Failed to load session: {}", e))
        }
        None => {
            info!("Creating new session");
            let cwd = {
                let cfg = state.config.lock().await;
                cfg.acp.assistant.working_directory.clone()
            };
            let (new_session_id, models_json) = client_guard
                .create_session(cwd)
                .map_err(|e| format!("Failed to create session: {}", e))?;

            // Store available models
            if let Ok(parsed) = serde_json::from_value::<Vec<crate::state::AcpModel>>(
                serde_json::Value::Array(models_json),
            ) {
                if let Ok(mut m) = state.available_models.lock() {
                    *m = parsed;
                }
            }

            // Apply default model if configured
            let cfg = state.config.lock().await;
            if let Some(ref default_model) = cfg.acp.assistant.default_model {
                if !default_model.is_empty() {
                    info!("Applying default model to new session: {}", default_model);
                    let request = crate::acp_client::AcpRequest {
                        jsonrpc: "2.0".to_string(),
                        id: serde_json::json!(4),
                        method: "_kiro.dev/commands/execute".to_string(),
                        params: serde_json::json!({
                            "sessionId": new_session_id,
                            "command": { "command": "model", "args": { "modelName": default_model } }
                        }),
                    };
                    match client_guard.send_request(&request) {
                        Ok(_) => info!("Default model applied: {}", default_model),
                        Err(e) => error!("Failed to apply default model: {}", e),
                    }
                }
            }

            // Send steering documents to the new session
            {
                let mut steering_parts: Vec<String> = Vec::new();
                steering_parts.push(crate::commands::system::BUILTIN_STEERING.to_string());

                if let Some(ref path) = cfg.acp.assistant.user_steering_path {
                    if !path.is_empty() {
                        if let Ok(content) = std::fs::read_to_string(path) {
                            if !content.trim().is_empty() {
                                steering_parts.push(content);
                            }
                        }
                    }
                }
                if cfg.acp.assistant.auto_steering_enabled {
                    if let Ok(auto_path) = crate::config::Config::get_auto_steering_path() {
                        if auto_path.exists() {
                            if let Ok(content) = std::fs::read_to_string(&auto_path) {
                                if !content.trim().is_empty() {
                                    steering_parts.push(content);
                                }
                            }
                        }
                    }
                }

                let steering_msg = format!(
                    "{} {}",
                    crate::commands::system::STEERING_MSG_PREFIX,
                    steering_parts.join("\n\n---\n\n")
                );
                let _ = client_guard.send_chat_streaming(steering_msg, None);
            }

            Ok(new_session_id)
        }
    }
}

/// Get the current ACP session ID
#[tauri::command]
pub async fn get_current_session_id(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let client = state.acp_client.lock().await;
    Ok(client.get_session_id())
}

/// Get the floating window's session ID
#[tauri::command]
pub async fn get_floating_session_id(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let floating = state
        .floating_session_id
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    Ok(floating.clone())
}

/// Restore the floating session as the active ACP session
#[tauri::command]
pub async fn restore_floating_session(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let floating_id = {
        let floating = state
            .floating_session_id
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        floating.clone()
    };

    if let Some(ref id) = floating_id {
        let client = state.acp_client.lock().await;
        client.set_session_id(Some(id.clone()));
        info!("Restored floating session: {}", id);
    }

    Ok(floating_id)
}

/// Rename a session by updating its title in the cache
#[tauri::command]
pub async fn rename_session(
    session_id: String,
    title: String,
) -> Result<(), String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("Title cannot be empty".to_string());
    }

    info!("Renaming session {} to: {}", session_id, title);

    let mut cache = load_title_cache();
    cache.insert(session_id, title);
    save_title_cache(&cache);

    Ok(())
}
