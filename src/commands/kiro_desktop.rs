//! Commands for reading Kiro Desktop (IDE) chat sessions.
//!
//! Loads conversations from .chat files in the Kiro Desktop data directory.
//! Uses workspace-sessions for the session index (titles, dates) and
//! .chat files for the full conversation content.

use log::info;
use serde::Serialize;
use std::path::PathBuf;

/// Get the Kiro Desktop globalStorage directory.
fn kiro_desktop_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    { dirs::config_dir().map(|d| d.join("Kiro").join("User").join("globalStorage").join("kiro.kiroagent")) }
    #[cfg(target_os = "macos")]
    { dirs::home_dir().map(|d| d.join("Library").join("Application Support").join("Kiro").join("User").join("globalStorage").join("kiro.kiroagent")) }
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CONFIG_HOME").ok().map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|d| d.join(".config")))
            .map(|d| d.join("Kiro").join("User").join("globalStorage").join("kiro.kiroagent"))
    }
}

#[derive(Debug, Serialize)]
pub struct KiroDesktopWorkspace {
    pub name: String,
    pub encoded: String,
    pub session_count: usize,
}

#[derive(Debug, Serialize)]
pub struct KiroDesktopSession {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub workspace_encoded: String,
    pub updated_at: String,
    pub message_count: usize,
    pub session_type: String,
    pub model: String,
    /// Path to the session file for deletion
    pub file_path: String,
}

#[derive(Debug, Serialize)]
pub struct KiroDesktopMessage {
    pub role: String,
    pub content: String,
}

fn base64_decode_path(encoded: &str) -> Option<String> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded).ok()
        .and_then(|b| String::from_utf8(b).ok())
}

#[tauri::command]
pub async fn kiro_desktop_available() -> bool {
    kiro_desktop_data_dir().map(|d| d.exists()).unwrap_or(false)
}

#[tauri::command]
pub async fn kiro_desktop_workspaces() -> Result<Vec<KiroDesktopWorkspace>, String> {
    let base = kiro_desktop_data_dir().ok_or("Kiro Desktop not found")?;
    let ws_dir = base.join("workspace-sessions");
    if !ws_dir.exists() { return Ok(Vec::new()); }

    let mut workspaces = Vec::new();
    for entry in std::fs::read_dir(&ws_dir).map_err(|e| e.to_string())?.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
        let encoded = entry.file_name().to_string_lossy().to_string();
        let name = base64_decode_path(&encoded).unwrap_or_else(|| encoded.clone());
        let session_count = std::fs::read_dir(entry.path())
            .map(|rd| rd.flatten().filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false)).count())
            .unwrap_or(0);
        if session_count > 0 {
            workspaces.push(KiroDesktopWorkspace { name, encoded, session_count });
        }
    }
    workspaces.sort_by(|a, b| b.session_count.cmp(&a.session_count));
    Ok(workspaces)
}

#[tauri::command]
pub async fn kiro_desktop_sessions(
    workspace_encoded: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<KiroDesktopSession>, String> {
    let base = kiro_desktop_data_dir().ok_or("Kiro Desktop not found")?;
    let ws_dir = base.join("workspace-sessions");
    let limit = limit.unwrap_or(50);

    let mut sessions = Vec::new();

    let dirs_to_scan: Vec<(String, PathBuf)> = if let Some(ref enc) = workspace_encoded {
        vec![(enc.clone(), ws_dir.join(enc))]
    } else {
        std::fs::read_dir(&ws_dir).map_err(|e| e.to_string())?
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| (e.file_name().to_string_lossy().to_string(), e.path()))
            .collect()
    };

    for (encoded, dir) in &dirs_to_scan {
        let ws_name = base64_decode_path(encoded).unwrap_or_else(|| encoded.clone());
        let Ok(entries) = std::fs::read_dir(dir) else { continue };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e != "json").unwrap_or(true) { continue; }

            let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();

            // Quick parse — just get title and history count
            let content = match std::fs::read_to_string(&path) { Ok(c) => c, Err(_) => continue };
            let json: serde_json::Value = match serde_json::from_str(&content) { Ok(v) => v, Err(_) => continue };

            let history_count = json.get("history").and_then(|h| h.as_array()).map(|a| a.len()).unwrap_or(0);
            // Skip empty sessions
            if history_count == 0 { continue; }

            let title = json.get("title").and_then(|t| t.as_str()).unwrap_or("Untitled").to_string();
            let title = if title.len() > 80 { format!("{}...", &title[..77]) } else { title };

            let session_type = json.get("sessionType").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let model = json.get("selectedModel").and_then(|m| m.as_str())
                .or_else(|| json.get("defaultModelTitle").and_then(|m| m.as_str()))
                .unwrap_or("").to_string();

            let updated_at = entry.metadata().and_then(|m| m.modified()).ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                    .map(|dt| dt.to_rfc3339()).unwrap_or_default())
                .unwrap_or_default();

            sessions.push(KiroDesktopSession {
                id, title, workspace: ws_name.clone(), workspace_encoded: encoded.clone(),
                updated_at, message_count: history_count, session_type, model,
                file_path: path.to_string_lossy().to_string(),
            });
        }
    }

    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions.truncate(limit);
    Ok(sessions)
}

#[tauri::command]
pub async fn kiro_desktop_load_session(
    workspace_encoded: String,
    session_id: String,
) -> Result<Vec<KiroDesktopMessage>, String> {
    let base = kiro_desktop_data_dir().ok_or("Kiro Desktop not found")?;
    let path = base.join("workspace-sessions").join(&workspace_encoded).join(format!("{}.json", session_id));

    if !path.exists() { return Err(format!("Session not found: {}", session_id)); }

    let content = std::fs::read_to_string(&path).map_err(|e| format!("Read: {}", e))?;
    let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| format!("Parse: {}", e))?;

    let history = json.get("history").and_then(|h| h.as_array()).ok_or("No history")?;
    let mut messages = Vec::new();

    for entry in history {
        let msg = match entry.get("message") { Some(m) => m, None => continue };
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("unknown");

        // Extract user message text from content blocks
        if role == "user" {
            let text = extract_text_content(msg.get("content"));
            // Skip system prompts and steering
            if text.is_empty() || is_system_message(&text) { continue; }
            messages.push(KiroDesktopMessage { role: "user".into(), content: text });
        }

        // The actual agent response is in promptLogs.completion
        // But it's often empty in workspace-sessions. Fall back to the assistant message.
        if role == "assistant" {
            let text = extract_text_content(msg.get("content"));
            if text.is_empty() || text == "On it." {
                // Try to get the real response from promptLogs
                if let Some(logs) = entry.get("promptLogs") {
                    let completion = logs.get("completion").and_then(|c| c.as_str()).unwrap_or("");
                    if !completion.is_empty() {
                        messages.push(KiroDesktopMessage { role: "assistant".into(), content: completion.to_string() });
                        continue;
                    }
                }
                // Skip "On it." if we can't find the real response
                if text == "On it." { continue; }
            }
            if !text.is_empty() {
                messages.push(KiroDesktopMessage { role: "assistant".into(), content: text });
            }
        }
    }

    // If we only got user messages (no real assistant responses), try loading from .chat files
    let has_assistant = messages.iter().any(|m| m.role == "assistant");
    if !has_assistant {
        // The workspace-sessions don't store completions.
        // Return what we have — user messages only — with a note.
        if !messages.is_empty() {
            messages.insert(0, KiroDesktopMessage {
                role: "assistant".into(),
                content: "*Agent responses are not available for this session format. Only user prompts are shown.*".into(),
            });
        }
    }

    info!("Loaded Kiro Desktop session {}: {} messages", session_id, messages.len());
    Ok(messages)
}

fn extract_text_content(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => {
            arr.iter()
                .filter(|item| item.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => String::new(),
    }
}

fn is_system_message(text: &str) -> bool {
    // Only filter out pure system prompts — not user messages with steering wrappers
    text.starts_with("<identity>") ||
    (text.starts_with("Follow these instructions") && text.len() > 5000)
}

/// Extract the actual user text from a message that may contain steering/context wrappers.
/// Extract the actual user text from a message that may contain steering/context wrappers.
/// Converts inline base64 images to markdown image syntax for rendering.
fn extract_user_text_from_chat(text: &str) -> String {
    let text = text.trim();

    // Pure system prompts
    if text.starts_with("<identity>") {
        return String::new();
    }

    let mut user_text = text.to_string();

    // The user's actual message comes AFTER all steering/rules blocks.
    // Pattern: blocks end with </user-rule>\n```\n</user-rule>\n\n\n
    // Find the last </user-rule> and take everything after it.
    if let Some(idx) = user_text.rfind("</user-rule>") {
        let after = &user_text[idx + "</user-rule>".len()..];
        let trimmed = after.trim_start_matches('`').trim_start_matches('\n').trim_start_matches('\r').trim();
        if !trimmed.is_empty() {
            user_text = trimmed.to_string();
        } else {
            return String::new();
        }
    }

    // Same for </steering-reminder>
    if let Some(idx) = user_text.rfind("</steering-reminder>") {
        let after = &user_text[idx + "</steering-reminder>".len()..];
        let trimmed = after.trim();
        if !trimmed.is_empty() {
            user_text = trimmed.to_string();
        } else {
            return String::new();
        }
    }

    // If it still starts with steering markers, it's pure steering
    if user_text.starts_with("## Included Rules") || user_text.starts_with("<steering-reminder>") {
        return String::new();
    }

    // Strip <EnvironmentContext>...</EnvironmentContext> from the end
    if let Some(idx) = user_text.find("<EnvironmentContext>") {
        user_text = user_text[..idx].trim().to_string();
    }

    if user_text.is_empty() || user_text.starts_with("<identity>") || user_text.starts_with("Follow these instructions") {
        return String::new();
    }

    user_text
}


/// Read only the first N bytes of a file (for fast metadata extraction).
fn read_file_head(path: &std::path::Path, max_bytes: usize) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len() as usize;
    if file_len <= max_bytes {
        // Small file — read it all
        let mut content = String::new();
        file.read_to_string(&mut content).ok()?;
        Some(content)
    } else {
        // Large file — read just the head
        let mut buf = vec![0u8; max_bytes];
        file.read_exact(&mut buf).ok()?;
        // Truncate to last valid UTF-8 boundary
        String::from_utf8(buf).ok().or_else(|| {
            let mut b = vec![0u8; max_bytes];
            let _ = std::fs::File::open(path).ok()?.read(&mut b).ok()?;
            Some(String::from_utf8_lossy(&b).into_owned())
        })
    }
}

#[tauri::command]
pub async fn kiro_desktop_delete_session(file_path: String) -> Result<(), String> {
    let path = std::path::Path::new(&file_path);
    if !path.exists() { return Err("File not found".into()); }
    // Safety: only delete .json files in the kiro.kiroagent directory
    let path_str = path.to_string_lossy();
    if !path_str.contains("kiro.kiroagent") || !path_str.ends_with(".json") {
        return Err("Invalid file path".into());
    }
    std::fs::remove_file(path).map_err(|e| format!("Delete failed: {}", e))?;
    info!("Deleted Kiro Desktop session: {}", file_path);
    Ok(())
}

#[tauri::command]
pub async fn kiro_desktop_open_folder(file_path: String) -> Result<(), String> {
    let path = std::path::Path::new(&file_path);
    let dir = path.parent().ok_or("No parent directory")?;
    crate::os::shell::open_path(&dir.to_string_lossy()).map_err(|e| e.to_string())
}

/// Load a .chat file directly (older format with full conversations).
#[tauri::command]
pub async fn kiro_desktop_load_chat_file(file_path: String) -> Result<Vec<KiroDesktopMessage>, String> {
    let content = std::fs::read_to_string(&file_path).map_err(|e| format!("Read: {}", e))?;
    let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| format!("Parse: {}", e))?;

    let chat = json.get("chat").and_then(|c| c.as_array()).ok_or("No chat array")?;
    let mut messages = Vec::new();

    for msg in chat {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");

        let normalized_role = match role {
            "human" => "user",
            "bot" => "assistant",
            "tool" => "tool",
            _ => continue,
        };

        // Skip empty messages
        if content.is_empty() { continue; }
        // For user messages, extract the actual text (strip steering/context wrappers)
        if normalized_role == "user" {
            if is_system_message(content) { continue; }
            let extracted = extract_user_text_from_chat(content);
            if extracted.is_empty() { continue; }
            messages.push(KiroDesktopMessage {
                role: normalized_role.to_string(),
                content: extracted,
            });
            continue;
        }

        messages.push(KiroDesktopMessage {
            role: normalized_role.to_string(),
            content: content.to_string(),
        });
    }

    info!("Loaded .chat file {}: {} messages", file_path, messages.len());
    Ok(messages)
}

/// List .chat files from hash directories as additional sessions.
#[tauri::command]
pub async fn kiro_desktop_chat_sessions(limit: Option<usize>) -> Result<Vec<KiroDesktopSession>, String> {
    let base = kiro_desktop_data_dir().ok_or("Kiro Desktop not found")?;
    let limit = limit.unwrap_or(50);
    let mut sessions = Vec::new();

    // Find hash directories (32-char hex names)
    let entries = std::fs::read_dir(&base).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.len() != 32 || !name.chars().all(|c| c.is_ascii_hexdigit()) { continue; }

        let dir = entry.path();
        let Ok(files) = std::fs::read_dir(&dir) else { continue };

        for file in files.flatten() {
            let path = file.path();
            if path.extension().map(|e| e != "chat").unwrap_or(true) { continue; }

            let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();

            // Only read the first 50KB — enough for metadata and first few messages
            let content = match read_file_head(&path, 50_000) { Some(c) => c, None => continue };
            // Try to parse — may be truncated, so wrap in a recovery
            let json: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(_) => {
                    // Truncated JSON — try to get metadata from the beginning
                    // Fall back to file metadata for date
                    let updated_at = file.metadata().and_then(|m| m.modified()).ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                            .map(|dt| dt.to_rfc3339()).unwrap_or_default())
                        .unwrap_or_default();
                    sessions.push(KiroDesktopSession {
                        id, title: "Untitled".into(),
                        workspace: name.clone(), workspace_encoded: name.clone(),
                        updated_at, message_count: 0, session_type: "chat".into(),
                        model: String::new(), file_path: path.to_string_lossy().to_string(),
                    });
                    continue;
                }
            };

            let chat = match json.get("chat").and_then(|c| c.as_array()) { Some(c) => c, None => continue };
            if chat.is_empty() { continue; }

            // Find the first real user message for the title
            let title = chat.iter()
                .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("human"))
                .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
                .filter_map(|c| {
                    // Use the same extraction logic as the message renderer
                    let extracted = extract_user_text_from_chat(c);
                    if extracted.is_empty() { None } else { Some(extracted) }
                })
                .next()
                .map(|c| {
                    let t: String = c.chars().take(80).collect();
                    // Clean up any JSON artifacts or newlines
                    let t = t.replace('\n', " ").replace('\r', " ");
                    if c.len() > 80 { format!("{}...", t.trim()) } else { t.trim().to_string() }
                })
                .unwrap_or_else(|| "Untitled".to_string());

            let message_count = chat.len();
            let model = json.get("metadata").and_then(|m| m.get("modelId")).and_then(|m| m.as_str()).unwrap_or("").to_string();

            let start_time = json.get("metadata").and_then(|m| m.get("startTime")).and_then(|t| t.as_i64()).unwrap_or(0);
            let end_time = json.get("metadata").and_then(|m| m.get("endTime")).and_then(|t| t.as_i64()).unwrap_or(start_time);
            let updated_at = if end_time > 0 {
                chrono::DateTime::from_timestamp(end_time / 1000, 0)
                    .map(|dt| dt.to_rfc3339()).unwrap_or_default()
            } else if start_time > 0 {
                chrono::DateTime::from_timestamp(start_time / 1000, 0)
                    .map(|dt| dt.to_rfc3339()).unwrap_or_default()
            } else {
                file.metadata().and_then(|m| m.modified()).ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                        .map(|dt| dt.to_rfc3339()).unwrap_or_default())
                    .unwrap_or_default()
            };

            sessions.push(KiroDesktopSession {
                id, title,
                workspace: name.clone(),
                workspace_encoded: name.clone(),
                updated_at, message_count,
                session_type: "chat".to_string(),
                model,
                file_path: path.to_string_lossy().to_string(),
            });
        }
    }

    // Group by workflowId — keep only the latest .chat file per workflow
    // (the last file has the full accumulated conversation)
    let mut by_workflow: std::collections::HashMap<String, KiroDesktopSession> = std::collections::HashMap::new();

    for s in sessions {
        // Extract workflowId from the session (stored in workspace_encoded for .chat files)
        // We need to re-read it... but we already have it from the metadata parse above.
        // For now, use the file path to re-extract. TODO: store workflowId in the struct.
        // Actually, let's just deduplicate by title — sessions with the same title from the
        // same workspace are likely the same conversation at different points.
        let key = format!("{}:{}", s.workspace, s.title);
        if let Some(existing) = by_workflow.get(&key) {
            // Keep the one with more messages (later in the conversation)
            if s.message_count > existing.message_count {
                by_workflow.insert(key, s);
            }
        } else {
            by_workflow.insert(key, s);
        }
    }

    let mut deduped: Vec<KiroDesktopSession> = by_workflow.into_values().collect();
    deduped.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    deduped.truncate(limit);
    Ok(deduped)
}
