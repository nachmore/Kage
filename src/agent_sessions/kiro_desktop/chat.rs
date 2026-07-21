use super::{content, KiroDesktopProvider, PROVIDER_ID};
use crate::agent_sessions::{
    clip_title, fingerprint, rfc3339_from_epoch_ms, rfc3339_from_system_time, AgentMessage,
    AgentSession, CachedSession, FileFingerprint,
};
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use log::info;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(super) fn scan_sessions(
    provider: &KiroDesktopProvider,
    base: &Path,
    limit: usize,
) -> Result<Vec<AgentSession>, AppError> {
    let files = find_chat_files(base)?;
    let keys = files.iter().map(|(path, ..)| path.clone()).collect();
    let (mut sessions, misses) = cached_or_missing(provider, files, keys);
    let mut fresh = Vec::with_capacity(misses.len());
    for (path, fingerprint, workspace) in misses {
        let Some(session) = parse_session(&path, &workspace) else {
            continue;
        };
        fresh.push((
            path,
            CachedSession {
                fp: fingerprint,
                session: session.clone(),
            },
        ));
        sessions.push(session);
    }
    if !fresh.is_empty() {
        provider.chat_files.lock_or_recover().extend(fresh);
    }
    dedupe_sessions(sessions, limit)
}

fn find_chat_files(base: &Path) -> Result<Vec<(PathBuf, FileFingerprint, String)>, AppError> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(base).map_err(read_error)?.flatten() {
        let workspace = entry.file_name().to_string_lossy().to_string();
        if workspace.len() != 32
            || !workspace
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        for file in entries.flatten() {
            let path = file.path();
            if path.extension().is_none_or(|extension| extension != "chat") {
                continue;
            }
            let Ok(metadata) = file.metadata() else {
                continue;
            };
            let Some(fingerprint) = fingerprint(&metadata) else {
                continue;
            };
            files.push((path, fingerprint, workspace.clone()));
        }
    }
    Ok(files)
}

fn cached_or_missing(
    provider: &KiroDesktopProvider,
    files: Vec<(PathBuf, FileFingerprint, String)>,
    keys: HashSet<PathBuf>,
) -> (Vec<AgentSession>, Vec<(PathBuf, FileFingerprint, String)>) {
    let mut sessions = Vec::with_capacity(files.len());
    let mut misses = Vec::new();
    let mut cache = provider.chat_files.lock_or_recover();
    cache.retain(|path, _| keys.contains(path));
    for (path, fingerprint, workspace) in files {
        match cache.get(&path) {
            Some(cached) if cached.fp == fingerprint => sessions.push(cached.session.clone()),
            _ => misses.push((path, fingerprint, workspace)),
        }
    }
    (sessions, misses)
}

fn dedupe_sessions(
    sessions: Vec<AgentSession>,
    limit: usize,
) -> Result<Vec<AgentSession>, AppError> {
    let mut by_workflow: HashMap<String, AgentSession> = HashMap::new();
    for session in sessions {
        let workspace = session
            .extras
            .get("workspace")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let workflow = session
            .extras
            .get("workflow_id")
            .and_then(|value| value.as_str())
            .unwrap_or(&session.title);
        let key = format!("{workspace}:{workflow}");
        match by_workflow.get(&key) {
            Some(existing) if session.message_count <= existing.message_count => {}
            _ => {
                by_workflow.insert(key, session);
            }
        }
    }
    let mut sessions: Vec<_> = by_workflow.into_values().collect();
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    sessions.truncate(limit);
    Ok(sessions)
}

fn parse_session(path: &Path, workspace: &str) -> Option<AgentSession> {
    let id = path.file_stem()?.to_str()?.to_string();
    let content = content::read_file_head(path, 50_000)?;
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(json) => json,
        Err(_) => return Some(truncated_session(path, workspace, id)),
    };
    let chat = json.get("chat")?.as_array()?;
    if chat.is_empty() {
        return None;
    }
    let title = chat
        .iter()
        .filter(|message| message.get("role").and_then(|role| role.as_str()) == Some("human"))
        .filter_map(|message| message.get("content").and_then(|content| content.as_str()))
        .map(content::extract_user_text_from_chat)
        .find(|text| !text.is_empty())
        .map(|text| clip_title(&text, 80))
        .unwrap_or_else(|| "Untitled".to_string());
    let metadata = json.get("metadata");
    let workflow_id = metadata
        .and_then(|value| value.get("workflowId"))
        .and_then(|value| value.as_str());
    let start_time = metadata
        .and_then(|value| value.get("startTime"))
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let end_time = metadata
        .and_then(|value| value.get("endTime"))
        .and_then(|value| value.as_i64())
        .unwrap_or(start_time);
    let updated_at = timestamp_or_mtime(path, start_time, end_time);
    let mut extras = json!({
        "workspace": workspace,
        "session_type": "chat",
        "model": metadata.and_then(|value| value.get("modelId")).and_then(|value| value.as_str()).unwrap_or(""),
        "file_path": path.to_string_lossy(),
    });
    if let Some(workflow_id) = workflow_id {
        extras
            .as_object_mut()
            .expect("extras is an object")
            .insert("workflow_id".to_string(), json!(workflow_id));
    }
    Some(AgentSession {
        provider_id: PROVIDER_ID.to_string(),
        session_id: id,
        title,
        updated_at,
        message_count: chat.len(),
        container: Some(workspace.to_string()),
        locator: json!({"kind": "chat_file", "file_path": path.to_string_lossy()}),
        extras,
    })
}

fn truncated_session(path: &Path, workspace: &str, id: String) -> AgentSession {
    AgentSession {
        provider_id: PROVIDER_ID.to_string(),
        session_id: id,
        title: "Untitled".to_string(),
        updated_at: mtime(path),
        message_count: 0,
        container: Some(workspace.to_string()),
        locator: json!({"kind": "chat_file", "file_path": path.to_string_lossy()}),
        extras: json!({
            "workspace": workspace,
            "session_type": "chat",
            "file_path": path.to_string_lossy(),
        }),
    }
}

fn timestamp_or_mtime(path: &Path, start_time: i64, end_time: i64) -> String {
    if end_time > 0 {
        rfc3339_from_epoch_ms(end_time)
    } else if start_time > 0 {
        rfc3339_from_epoch_ms(start_time)
    } else {
        mtime(path)
    }
}

fn mtime(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(rfc3339_from_system_time)
        .unwrap_or_default()
}

pub(super) fn load_file(file_path: &str) -> Result<Vec<AgentMessage>, AppError> {
    let content = std::fs::read_to_string(file_path).map_err(read_error)?;
    let json: serde_json::Value = serde_json::from_str(&content).map_err(parse_error)?;
    let chat = json
        .get("chat")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.parse_failed",
                &[("reason", "session file has no `chat` array")],
            )
        })?;
    let mut messages = Vec::new();
    for message in chat {
        let role = match message.get("role").and_then(|value| value.as_str()) {
            Some("human") => "user",
            Some("bot") => "assistant",
            Some("tool") => "tool",
            _ => continue,
        };
        let Some(text) = message.get("content").and_then(|value| value.as_str()) else {
            continue;
        };
        if text.is_empty() || (role == "user" && content::is_system_message(text)) {
            continue;
        }
        let text = if role == "user" {
            content::extract_user_text_from_chat(text)
        } else {
            text.to_string()
        };
        if !text.is_empty() {
            messages.push(AgentMessage {
                role: role.to_string(),
                content: text,
                extras: serde_json::Value::Null,
            });
        }
    }
    info!("Loaded .chat file {file_path}: {} messages", messages.len());
    Ok(messages)
}

fn read_error(error: std::io::Error) -> AppError {
    AppError::keyed(
        ErrorKind::Internal,
        "errors.session.read_failed",
        &[("reason", &error.to_string())],
    )
}

fn parse_error(error: serde_json::Error) -> AppError {
    AppError::keyed(
        ErrorKind::Internal,
        "errors.session.parse_failed",
        &[("reason", &error.to_string())],
    )
}
