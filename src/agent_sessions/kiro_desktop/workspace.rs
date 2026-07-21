use super::{content, KiroDesktopProvider, PROVIDER_ID};
use crate::agent_sessions::{
    clip_title, rfc3339_from_system_time, AgentMessage, AgentSession, CachedSession,
    FileFingerprint,
};
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use log::info;
use serde_json::json;
use std::path::{Path, PathBuf};

type SessionFile = (PathBuf, FileFingerprint, String, String);

struct CacheLookup {
    sessions: Vec<AgentSession>,
    misses: Vec<SessionFile>,
}

pub(super) fn scan_sessions(
    provider: &KiroDesktopProvider,
    ws_dir: &Path,
    workspace_encoded: Option<&str>,
    limit: usize,
) -> Result<Vec<AgentSession>, AppError> {
    if !ws_dir.exists() {
        return Ok(Vec::new());
    }

    let directories = workspace_directories(ws_dir, workspace_encoded)?;
    let mut files = Vec::new();
    for (encoded, directory) in directories {
        let workspace = decode_path(&encoded).unwrap_or_else(|| encoded.clone());
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let Some(fingerprint) = crate::agent_sessions::fingerprint(&metadata) else {
                continue;
            };
            files.push((path, fingerprint, workspace.clone(), encoded.clone()));
        }
    }

    let keys = files.iter().map(|(path, ..)| path.clone()).collect();
    let CacheLookup {
        mut sessions,
        misses,
    } = cached_or_missing(provider, files, keys);
    let mut fresh = Vec::with_capacity(misses.len());
    for (path, fingerprint, workspace, encoded) in misses {
        let Some(session) = parse_session(&path, &workspace, &encoded) else {
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
        let mut cache = provider.workspace_sessions.lock_or_recover();
        cache.extend(fresh);
    }
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    sessions.truncate(limit);
    Ok(sessions)
}

fn workspace_directories(
    ws_dir: &Path,
    encoded: Option<&str>,
) -> Result<Vec<(String, PathBuf)>, AppError> {
    if let Some(encoded) = encoded {
        return Ok(vec![(encoded.to_string(), ws_dir.join(encoded))]);
    }
    Ok(std::fs::read_dir(ws_dir)
        .map_err(read_error)?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().to_string(),
                entry.path(),
            )
        })
        .collect())
}

fn cached_or_missing(
    provider: &KiroDesktopProvider,
    files: Vec<SessionFile>,
    keys: std::collections::HashSet<PathBuf>,
) -> CacheLookup {
    let mut sessions = Vec::with_capacity(files.len());
    let mut misses = Vec::new();
    let mut cache = provider.workspace_sessions.lock_or_recover();
    cache.retain(|path, _| keys.contains(path));
    for (path, fingerprint, workspace, encoded) in files {
        match cache.get(&path) {
            Some(cached) if cached.fp == fingerprint => sessions.push(cached.session.clone()),
            _ => misses.push((path, fingerprint, workspace, encoded)),
        }
    }
    CacheLookup { sessions, misses }
}

fn parse_session(path: &Path, workspace: &str, encoded: &str) -> Option<AgentSession> {
    let id = path.file_stem()?.to_str()?.to_string();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let message_count = json.get("history")?.as_array()?.len();
    if message_count == 0 {
        return None;
    }
    let title = clip_title(
        json.get("title")
            .and_then(|value| value.as_str())
            .unwrap_or("Untitled"),
        80,
    );
    let updated_at = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(rfc3339_from_system_time)
        .unwrap_or_default();
    Some(AgentSession {
        provider_id: PROVIDER_ID.to_string(),
        session_id: id.clone(),
        title,
        updated_at,
        message_count,
        container: Some(workspace.to_string()),
        locator: json!({"kind": "workspace_session", "workspace_encoded": encoded, "session_id": id}),
        extras: json!({
            "workspace": workspace,
            "workspace_encoded": encoded,
            "session_type": json.get("sessionType").and_then(|value| value.as_str()).unwrap_or(""),
            "model": json.get("selectedModel").and_then(|value| value.as_str())
                .or_else(|| json.get("defaultModelTitle").and_then(|value| value.as_str()))
                .unwrap_or(""),
            "file_path": path.to_string_lossy(),
        }),
    })
}

pub(super) fn load_session(
    workspace_encoded: &str,
    session_id: &str,
) -> Result<Vec<AgentMessage>, AppError> {
    let path = KiroDesktopProvider::data_dir()
        .ok_or_else(dir_unavailable)?
        .join("workspace-sessions")
        .join(workspace_encoded)
        .join(format!("{session_id}.json"));
    if !path.exists() {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.session.not_found",
            &[],
        ));
    }
    let content = std::fs::read_to_string(&path).map_err(read_error)?;
    let json: serde_json::Value = serde_json::from_str(&content).map_err(parse_error)?;
    let history = json
        .get("history")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.parse_failed",
                &[("reason", "session file has no `history` field")],
            )
        })?;

    let mut messages = Vec::new();
    for entry in history {
        let Some(message) = entry.get("message") else {
            continue;
        };
        match message
            .get("role")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
        {
            "user" => push_user(&mut messages, message),
            "assistant" => push_assistant(&mut messages, entry, message),
            _ => {}
        }
    }
    if !messages
        .iter()
        .any(|message: &AgentMessage| message.role == "assistant")
        && !messages.is_empty()
    {
        messages.insert(0, agent_message("assistant", "*Agent responses are not available for this session format. Only user prompts are shown.*".to_string()));
    }
    info!(
        "Loaded Kiro Desktop session {session_id}: {} messages",
        messages.len()
    );
    Ok(messages)
}

fn push_user(messages: &mut Vec<AgentMessage>, message: &serde_json::Value) {
    let text = content::extract_text_content(message.get("content"));
    if !text.is_empty() && !content::is_system_message(&text) {
        messages.push(agent_message("user", text));
    }
}

fn push_assistant(
    messages: &mut Vec<AgentMessage>,
    entry: &serde_json::Value,
    message: &serde_json::Value,
) {
    let text = content::extract_text_content(message.get("content"));
    if text.is_empty() || text == "On it." {
        if let Some(completion) = entry
            .pointer("/promptLogs/completion")
            .and_then(|value| value.as_str())
            .filter(|text| !text.is_empty())
        {
            messages.push(agent_message("assistant", completion.to_string()));
            return;
        }
    }
    if !text.is_empty() && text != "On it." {
        messages.push(agent_message("assistant", text));
    }
}

fn agent_message(role: &str, content: String) -> AgentMessage {
    AgentMessage {
        role: role.into(),
        content,
        extras: serde_json::Value::Null,
    }
}

fn decode_path(encoded: &str) -> Option<String> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

#[derive(Debug, serde::Serialize)]
pub struct KiroDesktopWorkspace {
    pub name: String,
    pub encoded: String,
    pub session_count: usize,
}

pub fn list_workspaces() -> Result<Vec<KiroDesktopWorkspace>, AppError> {
    let base = KiroDesktopProvider::data_dir().ok_or_else(dir_unavailable)?;
    let ws_dir = base.join("workspace-sessions");
    if !ws_dir.exists() {
        return Ok(Vec::new());
    }
    let mut workspaces = std::fs::read_dir(ws_dir)
        .map_err(read_error)?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let encoded = entry.file_name().to_string_lossy().to_string();
            let session_count = std::fs::read_dir(entry.path())
                .ok()?
                .flatten()
                .filter(|file| {
                    file.path()
                        .extension()
                        .is_some_and(|extension| extension == "json")
                })
                .count();
            (session_count > 0).then(|| KiroDesktopWorkspace {
                name: decode_path(&encoded).unwrap_or_else(|| encoded.clone()),
                encoded,
                session_count,
            })
        })
        .collect::<Vec<_>>();
    workspaces.sort_by_key(|workspace| std::cmp::Reverse(workspace.session_count));
    Ok(workspaces)
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

fn dir_unavailable() -> AppError {
    AppError::keyed(
        ErrorKind::Internal,
        "errors.session.dir_unavailable",
        &[("reason", "Kiro Desktop data directory could not be located")],
    )
}
