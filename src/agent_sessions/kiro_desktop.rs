//! Kiro Desktop (IDE) session provider — reads:
//!   1. `<base>/workspace-sessions/<encoded-workspace>/<sessionId>.json`
//!   2. `<base>/<32-char-hex-hash>/*.chat`
//!
//! both under the Kiro Desktop globalStorage directory. The two formats
//! coexist on disk for historical reasons; the provider merges them into
//! one combined session list, deduping `.chat` files by workflow id.
//!
//! # Caching
//!
//! Listing scans potentially many files on every call; without a cache,
//! opening the chat window stalls for users with deep history. Two
//! independent maps (workspace-sessions + chat-files) keyed by absolute
//! file path with `(mtime, size)` fingerprints. Stale-file eviction runs
//! inside each scan, so deletes are picked up without a file watcher.
//! Cache is provider-internal — kiro-cli doesn't need one (sqlite caches
//! itself), and a generic "cache around list_sessions" wrapper would
//! force a one-size invalidation policy on providers that don't share
//! it.

use super::{
    clip_title, file_mtime_ms, rfc3339_from_epoch_ms, rfc3339_from_system_time, AgentMessage,
    AgentSession, AgentSessionProvider, SessionLocator,
};
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use log::info;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const PROVIDER_ID: &str = "kiro-desktop";
const PROVIDER_LABEL: &str = "Kiro IDE & CLI";

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileFingerprint {
    mtime: SystemTime,
    size: u64,
}

fn fingerprint(md: &std::fs::Metadata) -> Option<FileFingerprint> {
    let mtime = md.modified().ok()?;
    Some(FileFingerprint {
        mtime,
        size: md.len(),
    })
}

#[derive(Clone)]
struct CachedSession {
    fp: FileFingerprint,
    session: AgentSession,
}

#[derive(Default)]
pub struct KiroDesktopProvider {
    workspace_sessions: Mutex<HashMap<PathBuf, CachedSession>>,
    chat_files: Mutex<HashMap<PathBuf, CachedSession>>,
}

impl KiroDesktopProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn data_dir() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            dirs::config_dir().map(|d| {
                d.join("Kiro")
                    .join("User")
                    .join("globalStorage")
                    .join("kiro.kiroagent")
            })
        }
        #[cfg(target_os = "macos")]
        {
            dirs::home_dir().map(|d| {
                d.join("Library")
                    .join("Application Support")
                    .join("Kiro")
                    .join("User")
                    .join("globalStorage")
                    .join("kiro.kiroagent")
            })
        }
        #[cfg(target_os = "linux")]
        {
            std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .or_else(|| dirs::home_dir().map(|d| d.join(".config")))
                .map(|d| {
                    d.join("Kiro")
                        .join("User")
                        .join("globalStorage")
                        .join("kiro.kiroagent")
                })
        }
    }
}

/// Three locator shapes — one per loadable source. Differentiated by the
/// `kind` discriminator the frontend sets when it builds the locator.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
enum KiroDesktopLocator {
    /// `workspace-sessions/<encoded>/<sessionId>.json`
    #[serde(rename = "workspace_session")]
    WorkspaceSession {
        workspace_encoded: String,
        session_id: String,
    },
    /// `<hash>/<file>.chat` — full-conversation .chat file format
    #[serde(rename = "chat_file")]
    ChatFile { file_path: String },
}

impl AgentSessionProvider for KiroDesktopProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn label(&self) -> &'static str {
        PROVIDER_LABEL
    }

    fn is_available(&self) -> bool {
        Self::data_dir().map(|d| d.exists()).unwrap_or(false)
    }

    fn list_sessions(&self, limit: usize) -> Result<Vec<AgentSession>, AppError> {
        let base = Self::data_dir().ok_or_else(|| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.dir_unavailable",
                &[("reason", "Kiro Desktop data directory could not be located")],
            )
        })?;
        if !base.exists() {
            return Ok(Vec::new());
        }

        // Scan both sources, merge, sort, truncate.
        let ws_dir = base.join("workspace-sessions");
        let mut sessions = self.scan_workspace_sessions(&ws_dir, None, limit * 2)?;
        let chat_sessions = self.scan_chat_sessions(&base, limit * 2)?;
        sessions.extend(chat_sessions);
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions.truncate(limit);
        Ok(sessions)
    }

    fn load_session(&self, locator: &SessionLocator) -> Result<Vec<AgentMessage>, AppError> {
        let loc: KiroDesktopLocator = serde_json::from_value(locator.clone()).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.parse_failed",
                &[("reason", &e.to_string())],
            )
        })?;

        match loc {
            KiroDesktopLocator::WorkspaceSession {
                workspace_encoded,
                session_id,
            } => self.load_workspace_session(&workspace_encoded, &session_id),
            KiroDesktopLocator::ChatFile { file_path } => self.load_chat_file(&file_path),
        }
    }

    fn check_session_updated(
        &self,
        locator: &SessionLocator,
        since_ms: i64,
    ) -> Result<Option<i64>, AppError> {
        let loc: KiroDesktopLocator = serde_json::from_value(locator.clone()).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.parse_failed",
                &[("reason", &e.to_string())],
            )
        })?;

        let path = match loc {
            KiroDesktopLocator::WorkspaceSession {
                workspace_encoded,
                session_id,
            } => {
                let base = Self::data_dir().ok_or_else(|| {
                    AppError::keyed(
                        ErrorKind::Internal,
                        "errors.session.dir_unavailable",
                        &[("reason", "Kiro Desktop data directory could not be located")],
                    )
                })?;
                base.join("workspace-sessions")
                    .join(workspace_encoded)
                    .join(format!("{}.json", session_id))
            }
            KiroDesktopLocator::ChatFile { file_path } => PathBuf::from(file_path),
        };

        let Some(current) = file_mtime_ms(&path) else {
            return Ok(None);
        };
        if current > since_ms {
            Ok(Some(current))
        } else {
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Listing — workspace-sessions
// ---------------------------------------------------------------------------

impl KiroDesktopProvider {
    fn scan_workspace_sessions(
        &self,
        ws_dir: &Path,
        workspace_encoded: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AgentSession>, AppError> {
        if !ws_dir.exists() {
            return Ok(Vec::new());
        }

        let dirs_to_scan: Vec<(String, PathBuf)> = if let Some(enc) = workspace_encoded {
            vec![(enc.to_string(), ws_dir.join(enc))]
        } else {
            std::fs::read_dir(ws_dir)
                .map_err(|e| {
                    AppError::keyed(
                        ErrorKind::Internal,
                        "errors.session.read_failed",
                        &[("reason", &e.to_string())],
                    )
                })?
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| (e.file_name().to_string_lossy().to_string(), e.path()))
                .collect()
        };

        let mut seen_files: Vec<(PathBuf, FileFingerprint, String, String)> = Vec::new();
        for (encoded, dir) in &dirs_to_scan {
            let ws_name = base64_decode_path(encoded).unwrap_or_else(|| encoded.clone());
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e != "json").unwrap_or(true) {
                    continue;
                }
                let Ok(md) = entry.metadata() else { continue };
                let Some(fp) = fingerprint(&md) else { continue };
                seen_files.push((path, fp, ws_name.clone(), encoded.clone()));
            }
        }

        let mut sessions: Vec<AgentSession> = Vec::with_capacity(seen_files.len());
        let mut misses: Vec<(PathBuf, FileFingerprint, String, String)> = Vec::new();
        let seen_keys: std::collections::HashSet<PathBuf> =
            seen_files.iter().map(|(p, _, _, _)| p.clone()).collect();
        {
            let mut guard = self.workspace_sessions.lock_or_recover();
            guard.retain(|k, _| seen_keys.contains(k));
            for (path, fp, ws_name, encoded) in seen_files {
                match guard.get(&path) {
                    Some(cached) if cached.fp == fp => sessions.push(cached.session.clone()),
                    _ => misses.push((path, fp, ws_name, encoded)),
                }
            }
        }

        let mut fresh: Vec<(PathBuf, CachedSession)> = Vec::with_capacity(misses.len());
        for (path, fp, ws_name, encoded) in misses {
            let Some(session) = parse_workspace_session(&path, &ws_name, &encoded) else {
                continue;
            };
            fresh.push((
                path,
                CachedSession {
                    fp,
                    session: session.clone(),
                },
            ));
            sessions.push(session);
        }

        if !fresh.is_empty() {
            let mut guard = self.workspace_sessions.lock_or_recover();
            for (path, entry) in fresh {
                guard.insert(path, entry);
            }
        }

        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions.truncate(limit);
        Ok(sessions)
    }
}

fn parse_workspace_session(path: &Path, ws_name: &str, encoded: &str) -> Option<AgentSession> {
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let content = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    let history_count = json
        .get("history")
        .and_then(|h| h.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    if history_count == 0 {
        return None;
    }

    let title_raw = json
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("Untitled");
    let title = clip_title(title_raw, 80);

    let session_type = json
        .get("sessionType")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let model = json
        .get("selectedModel")
        .and_then(|m| m.as_str())
        .or_else(|| json.get("defaultModelTitle").and_then(|m| m.as_str()))
        .unwrap_or("")
        .to_string();

    let updated_at = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(rfc3339_from_system_time)
        .unwrap_or_default();

    Some(AgentSession {
        provider_id: PROVIDER_ID.to_string(),
        session_id: id,
        title,
        updated_at,
        message_count: history_count,
        container: Some(ws_name.to_string()),
        locator: json!({
            "kind": "workspace_session",
            "workspace_encoded": encoded,
            "session_id": path.file_stem().and_then(|s| s.to_str()).unwrap_or(""),
        }),
        extras: json!({
            "workspace": ws_name,
            "workspace_encoded": encoded,
            "session_type": session_type,
            "model": model,
            "file_path": path.to_string_lossy(),
        }),
    })
}

// ---------------------------------------------------------------------------
// Listing — .chat files
// ---------------------------------------------------------------------------

impl KiroDesktopProvider {
    fn scan_chat_sessions(&self, base: &Path, limit: usize) -> Result<Vec<AgentSession>, AppError> {
        let mut seen_files: Vec<(PathBuf, FileFingerprint, String)> = Vec::new();
        let entries = std::fs::read_dir(base).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.read_failed",
                &[("reason", &e.to_string())],
            )
        })?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.len() != 32 || !name.chars().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            let dir = entry.path();
            let Ok(files) = std::fs::read_dir(&dir) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().map(|e| e != "chat").unwrap_or(true) {
                    continue;
                }
                let Ok(md) = file.metadata() else { continue };
                let Some(fp) = fingerprint(&md) else { continue };
                seen_files.push((path, fp, name.clone()));
            }
        }

        let mut sessions: Vec<AgentSession> = Vec::with_capacity(seen_files.len());
        let mut misses: Vec<(PathBuf, FileFingerprint, String)> = Vec::new();
        let seen_keys: std::collections::HashSet<PathBuf> =
            seen_files.iter().map(|(p, _, _)| p.clone()).collect();
        {
            let mut guard = self.chat_files.lock_or_recover();
            guard.retain(|k, _| seen_keys.contains(k));
            for (path, fp, hash) in seen_files {
                match guard.get(&path) {
                    Some(cached) if cached.fp == fp => sessions.push(cached.session.clone()),
                    _ => misses.push((path, fp, hash)),
                }
            }
        }

        let mut fresh: Vec<(PathBuf, CachedSession)> = Vec::with_capacity(misses.len());
        for (path, fp, hash) in misses {
            let Some(session) = parse_chat_session(&path, &hash) else {
                continue;
            };
            fresh.push((
                path,
                CachedSession {
                    fp,
                    session: session.clone(),
                },
            ));
            sessions.push(session);
        }

        if !fresh.is_empty() {
            let mut guard = self.chat_files.lock_or_recover();
            for (path, entry) in fresh {
                guard.insert(path, entry);
            }
        }

        // Dedupe by workflow id (multiple .chat files can belong to the
        // same workflow; the latest one has the full conversation).
        let mut by_workflow: HashMap<String, AgentSession> = HashMap::new();
        for s in sessions {
            let workflow_id = s
                .extras
                .get("workflow_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let workspace = s
                .extras
                .get("workspace")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let key = match workflow_id {
                Some(ref wid) => format!("{}:{}", workspace, wid),
                None => format!("{}:{}", workspace, s.title),
            };
            match by_workflow.get(&key) {
                Some(existing) if s.message_count <= existing.message_count => {}
                _ => {
                    by_workflow.insert(key, s);
                }
            }
        }

        let mut deduped: Vec<AgentSession> = by_workflow.into_values().collect();
        deduped.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        deduped.truncate(limit);
        Ok(deduped)
    }
}

fn parse_chat_session(path: &Path, hash_dir: &str) -> Option<AgentSession> {
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    // Only read the first 50 KB — enough for metadata and a few messages.
    let content = read_file_head(path, 50_000)?;

    // Truncated JSON: fall back to mtime-based metadata.
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => {
            let updated_at = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .map(rfc3339_from_system_time)
                .unwrap_or_default();
            return Some(AgentSession {
                provider_id: PROVIDER_ID.to_string(),
                session_id: id,
                title: "Untitled".to_string(),
                updated_at,
                message_count: 0,
                container: Some(hash_dir.to_string()),
                locator: json!({
                    "kind": "chat_file",
                    "file_path": path.to_string_lossy(),
                }),
                extras: json!({
                    "workspace": hash_dir,
                    "session_type": "chat",
                    "file_path": path.to_string_lossy(),
                }),
            });
        }
    };

    let chat = json.get("chat").and_then(|c| c.as_array())?;
    if chat.is_empty() {
        return None;
    }

    let title = chat
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("human"))
        .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
        .filter_map(|c| {
            let extracted = extract_user_text_from_chat(c);
            if extracted.is_empty() {
                None
            } else {
                Some(extracted)
            }
        })
        .next()
        .map(|c| clip_title(&c, 80))
        .unwrap_or_else(|| "Untitled".to_string());

    let message_count = chat.len();
    let model = json
        .get("metadata")
        .and_then(|m| m.get("modelId"))
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let workflow_id = json
        .get("metadata")
        .and_then(|m| m.get("workflowId"))
        .and_then(|w| w.as_str())
        .map(|s| s.to_string());

    let start_time = json
        .get("metadata")
        .and_then(|m| m.get("startTime"))
        .and_then(|t| t.as_i64())
        .unwrap_or(0);
    let end_time = json
        .get("metadata")
        .and_then(|m| m.get("endTime"))
        .and_then(|t| t.as_i64())
        .unwrap_or(start_time);
    let updated_at = if end_time > 0 {
        rfc3339_from_epoch_ms(end_time)
    } else if start_time > 0 {
        rfc3339_from_epoch_ms(start_time)
    } else {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .map(rfc3339_from_system_time)
            .unwrap_or_default()
    };

    let mut extras = json!({
        "workspace": hash_dir,
        "session_type": "chat",
        "model": model,
        "file_path": path.to_string_lossy(),
    });
    if let Some(wid) = workflow_id {
        extras
            .as_object_mut()
            .expect("just built")
            .insert("workflow_id".to_string(), serde_json::Value::String(wid));
    }

    Some(AgentSession {
        provider_id: PROVIDER_ID.to_string(),
        session_id: id,
        title,
        updated_at,
        message_count,
        container: Some(hash_dir.to_string()),
        locator: json!({
            "kind": "chat_file",
            "file_path": path.to_string_lossy(),
        }),
        extras,
    })
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

impl KiroDesktopProvider {
    fn load_workspace_session(
        &self,
        workspace_encoded: &str,
        session_id: &str,
    ) -> Result<Vec<AgentMessage>, AppError> {
        let base = Self::data_dir().ok_or_else(|| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.dir_unavailable",
                &[("reason", "Kiro Desktop data directory could not be located")],
            )
        })?;
        let path = base
            .join("workspace-sessions")
            .join(workspace_encoded)
            .join(format!("{}.json", session_id));

        if !path.exists() {
            return Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.session.not_found",
                &[],
            ));
        }

        let content = std::fs::read_to_string(&path).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.read_failed",
                &[("reason", &e.to_string())],
            )
        })?;
        let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.parse_failed",
                &[("reason", &e.to_string())],
            )
        })?;

        let history = json
            .get("history")
            .and_then(|h| h.as_array())
            .ok_or_else(|| {
                AppError::keyed(
                    ErrorKind::Internal,
                    "errors.session.parse_failed",
                    &[("reason", "session file has no `history` field")],
                )
            })?;
        let mut messages: Vec<AgentMessage> = Vec::new();

        for entry in history {
            let Some(msg) = entry.get("message") else {
                continue;
            };
            let role = msg
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown");

            if role == "user" {
                let text = extract_text_content(msg.get("content"));
                if text.is_empty() || is_system_message(&text) {
                    continue;
                }
                messages.push(AgentMessage {
                    role: "user".into(),
                    content: text,
                    extras: serde_json::Value::Null,
                });
            }

            if role == "assistant" {
                let text = extract_text_content(msg.get("content"));
                if text.is_empty() || text == "On it." {
                    if let Some(logs) = entry.get("promptLogs") {
                        let completion = logs
                            .get("completion")
                            .and_then(|c| c.as_str())
                            .unwrap_or("");
                        if !completion.is_empty() {
                            messages.push(AgentMessage {
                                role: "assistant".into(),
                                content: completion.to_string(),
                                extras: serde_json::Value::Null,
                            });
                            continue;
                        }
                    }
                    if text == "On it." {
                        continue;
                    }
                }
                if !text.is_empty() {
                    messages.push(AgentMessage {
                        role: "assistant".into(),
                        content: text,
                        extras: serde_json::Value::Null,
                    });
                }
            }
        }

        // No real assistant responses — explain why up front.
        let has_assistant = messages.iter().any(|m| m.role == "assistant");
        if !has_assistant && !messages.is_empty() {
            messages.insert(0, AgentMessage {
                role: "assistant".into(),
                content: "*Agent responses are not available for this session format. Only user prompts are shown.*".into(),
                extras: serde_json::Value::Null,
            });
        }

        info!(
            "Loaded Kiro Desktop session {}: {} messages",
            session_id,
            messages.len()
        );
        Ok(messages)
    }

    fn load_chat_file(&self, file_path: &str) -> Result<Vec<AgentMessage>, AppError> {
        let content = std::fs::read_to_string(file_path).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.read_failed",
                &[("reason", &e.to_string())],
            )
        })?;
        let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.parse_failed",
                &[("reason", &e.to_string())],
            )
        })?;

        let chat = json.get("chat").and_then(|c| c.as_array()).ok_or_else(|| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.parse_failed",
                &[("reason", "session file has no `chat` array")],
            )
        })?;
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

            if content.is_empty() {
                continue;
            }

            if normalized_role == "user" {
                if is_system_message(content) {
                    continue;
                }
                let extracted = extract_user_text_from_chat(content);
                if extracted.is_empty() {
                    continue;
                }
                messages.push(AgentMessage {
                    role: normalized_role.to_string(),
                    content: extracted,
                    extras: serde_json::Value::Null,
                });
                continue;
            }

            messages.push(AgentMessage {
                role: normalized_role.to_string(),
                content: content.to_string(),
                extras: serde_json::Value::Null,
            });
        }

        info!(
            "Loaded .chat file {}: {} messages",
            file_path,
            messages.len()
        );
        Ok(messages)
    }
}

// ---------------------------------------------------------------------------
// Helpers (lifted verbatim from old commands::kiro_desktop)
// ---------------------------------------------------------------------------

fn base64_decode_path(encoded: &str) -> Option<String> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
}

fn extract_text_content(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter(|item| item.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn is_system_message(text: &str) -> bool {
    text.starts_with("<identity>")
        || (text.starts_with("Follow these instructions") && text.len() > 5000)
}

/// Strip steering/context wrappers from a .chat user message, leaving
/// just the user's actual text. The .chat format embeds steering rules
/// and environment context inline; we want to display only what the
/// user typed.
fn extract_user_text_from_chat(text: &str) -> String {
    let text = text.trim();

    if text.starts_with("<identity>") {
        return String::new();
    }

    let mut user_text = text.to_string();

    if let Some(idx) = user_text.rfind("</user-rule>") {
        let after = &user_text[idx + "</user-rule>".len()..];
        let trimmed = after
            .trim_start_matches('`')
            .trim_start_matches('\n')
            .trim_start_matches('\r')
            .trim();
        if !trimmed.is_empty() {
            user_text = trimmed.to_string();
        } else {
            return String::new();
        }
    }

    if let Some(idx) = user_text.rfind("</steering-reminder>") {
        let after = &user_text[idx + "</steering-reminder>".len()..];
        let trimmed = after.trim();
        if !trimmed.is_empty() {
            user_text = trimmed.to_string();
        } else {
            return String::new();
        }
    }

    if user_text.starts_with("## Included Rules") || user_text.starts_with("<steering-reminder>") {
        return String::new();
    }

    if let Some(idx) = user_text.find("<EnvironmentContext>") {
        user_text = user_text[..idx].trim().to_string();
    }

    if user_text.is_empty()
        || user_text.starts_with("<identity>")
        || user_text.starts_with("Follow these instructions")
    {
        return String::new();
    }

    user_text
}

/// Read up to `max_bytes` from the head of a file, returning a UTF-8
/// string (lossy on the boundary if needed).
fn read_file_head(path: &Path, max_bytes: usize) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len() as usize;
    if file_len <= max_bytes {
        let mut content = String::new();
        file.read_to_string(&mut content).ok()?;
        Some(content)
    } else {
        let mut buf = vec![0u8; max_bytes];
        file.read_exact(&mut buf).ok()?;
        String::from_utf8(buf).ok().or_else(|| {
            let mut b = vec![0u8; max_bytes];
            let _ = std::fs::File::open(path).ok()?.read(&mut b).ok()?;
            Some(String::from_utf8_lossy(&b).into_owned())
        })
    }
}

// ---------------------------------------------------------------------------
// Workspace listing — used by the typed `kiro_desktop_workspaces` command
// (chrome, not part of the trait).
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
pub struct KiroDesktopWorkspace {
    pub name: String,
    pub encoded: String,
    pub session_count: usize,
}

pub fn list_workspaces() -> Result<Vec<KiroDesktopWorkspace>, AppError> {
    let base = KiroDesktopProvider::data_dir().ok_or_else(|| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.session.dir_unavailable",
            &[("reason", "Kiro Desktop installation not detected")],
        )
    })?;
    let ws_dir = base.join("workspace-sessions");
    if !ws_dir.exists() {
        return Ok(Vec::new());
    }

    let mut workspaces = Vec::new();
    for entry in std::fs::read_dir(&ws_dir)
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.read_failed",
                &[("reason", &e.to_string())],
            )
        })?
        .flatten()
    {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let encoded = entry.file_name().to_string_lossy().to_string();
        let name = base64_decode_path(&encoded).unwrap_or_else(|| encoded.clone());
        let session_count = std::fs::read_dir(entry.path())
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                    .count()
            })
            .unwrap_or(0);
        if session_count > 0 {
            workspaces.push(KiroDesktopWorkspace {
                name,
                encoded,
                session_count,
            });
        }
    }
    workspaces.sort_by_key(|w| std::cmp::Reverse(w.session_count));
    Ok(workspaces)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_user_text_strips_user_rule_block() {
        let s = "<user-rule>blah</user-rule>\n\nactual prompt";
        assert_eq!(extract_user_text_from_chat(s), "actual prompt");
    }

    #[test]
    fn extract_user_text_returns_empty_for_pure_steering() {
        assert_eq!(extract_user_text_from_chat("## Included Rules\nfoo"), "");
        assert_eq!(extract_user_text_from_chat("<identity>x</identity>"), "");
    }

    #[test]
    fn extract_user_text_strips_environment_context_suffix() {
        let s = "real text<EnvironmentContext>os=linux</EnvironmentContext>";
        assert_eq!(extract_user_text_from_chat(s), "real text");
    }

    #[test]
    fn is_system_message_only_flags_obvious_prompts() {
        assert!(is_system_message("<identity>foo"));
        assert!(!is_system_message("hello"));
        // Short "Follow these instructions" prefix is NOT a system prompt.
        assert!(!is_system_message("Follow these instructions: do X"));
    }

    #[test]
    fn extract_text_content_handles_array_of_text_blocks() {
        let v = serde_json::json!([
            {"type": "text", "text": "hello"},
            {"type": "image", "url": "..."},
            {"type": "text", "text": "world"},
        ]);
        assert_eq!(extract_text_content(Some(&v)), "hello\nworld");
    }
}
