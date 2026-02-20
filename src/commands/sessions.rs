use log::{error, info};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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

/// Extract a title from the JSONL — use the first user prompt text
fn extract_title_from_jsonl(jsonl_path: &std::path::Path) -> String {
    if let Ok(content) = fs::read_to_string(jsonl_path) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
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

        // Only process .json files (skip .jsonl and .lock)
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        // The session_id is the file stem (uuid)
        let session_id = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(data) => {
                    let created_at = data
                        .get("created_at")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let updated_at = data
                        .get("updated_at")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&created_at)
                        .to_string();

                    // Get title from the JSONL file (first user prompt)
                    let jsonl_path = path.with_extension("jsonl");
                    let title = extract_title_from_jsonl(&jsonl_path);

                    sessions.push(SessionSummary {
                        session_id,
                        title,
                        created_at,
                        updated_at,
                    });
                }
                Err(e) => {
                    error!("Failed to parse session JSON {:?}: {}", path, e);
                }
            },
            Err(e) => {
                error!("Failed to read session file {:?}: {}", path, e);
            }
        }
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
