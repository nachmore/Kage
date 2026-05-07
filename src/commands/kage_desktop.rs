//! Commands for reading Kage Desktop (IDE) chat sessions.
//!
//! Loads conversations from .chat files in the Kage Desktop data directory.
//! Uses workspace-sessions for the session index (titles, dates) and
//! .chat files for the full conversation content.
//!
//! # Caching
//!
//! Session enumeration functions (`kage_desktop_sessions` and
//! `kage_desktop_chat_sessions`) parse a potentially large number of JSON
//! files on every call. Without a cache, opening the chat window stalls for
//! users with many sessions. We cache parsed results per file, keyed by
//! `(mtime, size)`. Cache lookups are purely stat-based, so we pick up new
//! and modified sessions without needing a file watcher on an external
//! directory. See `KageDesktopCache`.

use crate::error::AppError;
use crate::lock_ext::LockExt;
use log::info;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Per-file cache for session metadata
// ---------------------------------------------------------------------------

/// Fingerprint identifying a particular version of a file on disk. If either
/// the modification time or the size changes we treat the cached parse as
/// stale and re-read the file.
#[derive(Clone, Copy, Eq, PartialEq)]
struct FileFingerprint {
    mtime: SystemTime,
    size: u64,
}

fn fingerprint(md: &std::fs::Metadata) -> Option<FileFingerprint> {
    let mtime = md.modified().ok()?;
    Some(FileFingerprint { mtime, size: md.len() })
}

/// Cached parse result for a single session file.
#[derive(Clone)]
struct CachedSession {
    fp: FileFingerprint,
    session: KageDesktopSession,
}

/// In-memory cache for parsed Kage Desktop sessions. Two independent maps —
/// one for the JSON `workspace-sessions/*` files and one for the `.chat`
/// files under the hash directories — because they produce the same output
/// shape but come from different scan roots.
///
/// Cache entries are keyed by absolute file path. Stale-file eviction
/// happens inside each scan: files not seen in the current walk are
/// dropped. This keeps the cache from growing unbounded as the user deletes
/// old sessions.
#[derive(Default)]
pub struct KageDesktopCache {
    workspace_sessions: Mutex<HashMap<PathBuf, CachedSession>>,
    chat_files: Mutex<HashMap<PathBuf, CachedSession>>,
}

impl KageDesktopCache {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Short type alias used across Tauri command signatures.
pub type KageDesktopCacheHandle = Arc<KageDesktopCache>;

/// Get the Kage Desktop globalStorage directory.
fn kage_desktop_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    { dirs::config_dir().map(|d| d.join("Kage").join("User").join("globalStorage").join("kage.kageagent")) }
    #[cfg(target_os = "macos")]
    { dirs::home_dir().map(|d| d.join("Library").join("Application Support").join("Kage").join("User").join("globalStorage").join("kage.kageagent")) }
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CONFIG_HOME").ok().map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|d| d.join(".config")))
            .map(|d| d.join("Kage").join("User").join("globalStorage").join("kage.kageagent"))
    }
}

#[derive(Debug, Serialize)]
pub struct KageDesktopWorkspace {
    pub name: String,
    pub encoded: String,
    pub session_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct KageDesktopSession {
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
    /// Workflow ID for deduplication (multiple .chat files can belong to the same workflow)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct KageDesktopMessage {
    pub role: String,
    pub content: String,
}

fn base64_decode_path(encoded: &str) -> Option<String> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded).ok()
        .and_then(|b| String::from_utf8(b).ok())
}

#[tauri::command]
pub async fn kage_desktop_available() -> bool {
    kage_desktop_data_dir().map(|d| d.exists()).unwrap_or(false)
}

#[tauri::command]
pub async fn kage_desktop_workspaces() -> Result<Vec<KageDesktopWorkspace>, AppError> {
    let base = kage_desktop_data_dir().ok_or("Kage Desktop not found")?;
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
            workspaces.push(KageDesktopWorkspace { name, encoded, session_count });
        }
    }
    workspaces.sort_by(|a, b| b.session_count.cmp(&a.session_count));
    Ok(workspaces)
}

#[tauri::command]
pub async fn kage_desktop_sessions(
    workspace_encoded: Option<String>,
    limit: Option<usize>,
    features: tauri::State<'_, crate::state::FeatureServices>,
) -> Result<Vec<KageDesktopSession>, AppError> {
    let base = kage_desktop_data_dir().ok_or("Kage Desktop not found")?;
    let ws_dir = base.join("workspace-sessions");
    let limit = limit.unwrap_or(50);
    let cache = features.kage_desktop_cache.clone();

    // Run the scan + JSON parse on the blocking pool — both are file I/O
    // heavy and synchronous.
    tokio::task::spawn_blocking(move || {
        scan_workspace_sessions(&ws_dir, workspace_encoded.as_deref(), limit, &cache)
    })
    .await
    .map_err(|e| format!("Scan task failed: {}", e))?
}

/// Blocking scan of `workspace-sessions/*/*.json`. Returns already-sorted,
/// already-truncated results. Uses the per-file cache to skip re-parsing
/// files whose (mtime, size) fingerprint matches a previous scan.
fn scan_workspace_sessions(
    ws_dir: &Path,
    workspace_encoded: Option<&str>,
    limit: usize,
    cache: &KageDesktopCache,
) -> Result<Vec<KageDesktopSession>, AppError> {
    if !ws_dir.exists() { return Ok(Vec::new()); }

    let dirs_to_scan: Vec<(String, PathBuf)> = if let Some(enc) = workspace_encoded {
        vec![(enc.to_string(), ws_dir.join(enc))]
    } else {
        std::fs::read_dir(ws_dir).map_err(|e| e.to_string())?
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| (e.file_name().to_string_lossy().to_string(), e.path()))
            .collect()
    };

    // Collect all (path, fingerprint, workspace name, encoded) tuples first so
    // we can consult the cache without holding its lock across file I/O.
    let mut seen_files: Vec<(PathBuf, FileFingerprint, String, String)> = Vec::new();
    for (encoded, dir) in &dirs_to_scan {
        let ws_name = base64_decode_path(encoded).unwrap_or_else(|| encoded.clone());
        let Ok(entries) = std::fs::read_dir(dir) else { continue };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e != "json").unwrap_or(true) { continue; }
            let Ok(md) = entry.metadata() else { continue };
            let Some(fp) = fingerprint(&md) else { continue };
            seen_files.push((path, fp, ws_name.clone(), encoded.clone()));
        }
    }

    // Fast path: check cache hits under a single short lock.
    let mut sessions: Vec<KageDesktopSession> = Vec::with_capacity(seen_files.len());
    let mut misses: Vec<(PathBuf, FileFingerprint, String, String)> = Vec::new();
    let seen_keys: std::collections::HashSet<PathBuf> =
        seen_files.iter().map(|(p, _, _, _)| p.clone()).collect();
    {
        let mut guard = cache.workspace_sessions.lock_or_recover();
        // Evict entries for files that no longer exist.
        guard.retain(|k, _| seen_keys.contains(k));
        for (path, fp, ws_name, encoded) in seen_files {
            match guard.get(&path) {
                Some(cached) if cached.fp == fp => sessions.push(cached.session.clone()),
                _ => misses.push((path, fp, ws_name, encoded)),
            }
        }
    }

    // Slow path: parse missed/changed files without holding the cache lock.
    let mut fresh: Vec<(PathBuf, CachedSession)> = Vec::with_capacity(misses.len());
    for (path, fp, ws_name, encoded) in misses {
        let Some(session) = parse_workspace_session(&path, &ws_name, &encoded) else { continue };
        fresh.push((path, CachedSession { fp, session: session.clone() }));
        sessions.push(session);
    }

    // Re-insert fresh parses under a single lock scope.
    if !fresh.is_empty() {
        let mut guard = cache.workspace_sessions.lock_or_recover();
        for (path, entry) in fresh {
            guard.insert(path, entry);
        }
    }

    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions.truncate(limit);
    Ok(sessions)
}

/// Parse a single `workspace-sessions/*/<id>.json` file into a
/// `KageDesktopSession`. Returns `None` if the file is unreadable,
/// unparseable, or represents an empty session that should be skipped.
fn parse_workspace_session(path: &Path, ws_name: &str, encoded: &str) -> Option<KageDesktopSession> {
    let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();

    let content = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    let history_count = json.get("history").and_then(|h| h.as_array()).map(|a| a.len()).unwrap_or(0);
    if history_count == 0 { return None; }

    let title = json.get("title").and_then(|t| t.as_str()).unwrap_or("Untitled").to_string();
    let title = if title.len() > 80 { format!("{}...", &title[..77]) } else { title };

    let session_type = json.get("sessionType").and_then(|t| t.as_str()).unwrap_or("").to_string();
    let model = json.get("selectedModel").and_then(|m| m.as_str())
        .or_else(|| json.get("defaultModelTitle").and_then(|m| m.as_str()))
        .unwrap_or("").to_string();

    let updated_at = std::fs::metadata(path).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
            .map(|dt| dt.to_rfc3339()).unwrap_or_default())
        .unwrap_or_default();

    Some(KageDesktopSession {
        id,
        title,
        workspace: ws_name.to_string(),
        workspace_encoded: encoded.to_string(),
        updated_at,
        message_count: history_count,
        session_type,
        model,
        file_path: path.to_string_lossy().to_string(),
        workflow_id: None,
    })
}

#[tauri::command]
pub async fn kage_desktop_load_session(
    workspace_encoded: String,
    session_id: String,
) -> Result<Vec<KageDesktopMessage>, AppError> {
    let base = kage_desktop_data_dir().ok_or("Kage Desktop not found")?;
    let path = base.join("workspace-sessions").join(&workspace_encoded).join(format!("{}.json", session_id));

    if !path.exists() { return Err(format!("Session not found: {}", session_id).into()); }

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
            messages.push(KageDesktopMessage { role: "user".into(), content: text });
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
                        messages.push(KageDesktopMessage { role: "assistant".into(), content: completion.to_string() });
                        continue;
                    }
                }
                // Skip "On it." if we can't find the real response
                if text == "On it." { continue; }
            }
            if !text.is_empty() {
                messages.push(KageDesktopMessage { role: "assistant".into(), content: text });
            }
        }
    }

    // If we only got user messages (no real assistant responses), try loading from .chat files
    let has_assistant = messages.iter().any(|m| m.role == "assistant");
    if !has_assistant {
        // The workspace-sessions don't store completions.
        // Return what we have — user messages only — with a note.
        if !messages.is_empty() {
            messages.insert(0, KageDesktopMessage {
                role: "assistant".into(),
                content: "*Agent responses are not available for this session format. Only user prompts are shown.*".into(),
            });
        }
    }

    info!("Loaded Kage Desktop session {}: {} messages", session_id, messages.len());
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
pub async fn kage_desktop_delete_session(
    file_path: String,
    features: tauri::State<'_, crate::state::FeatureServices>,
) -> Result<(), AppError> {
    let path = std::path::Path::new(&file_path);
    if !path.exists() { return Err("File not found".into()); }
    // Safety: only delete .json files in the kage.kageagent directory
    let path_str = path.to_string_lossy();
    if !path_str.contains("kage.kageagent") || !path_str.ends_with(".json") {
        return Err("Invalid file path".into());
    }
    std::fs::remove_file(path).map_err(|e| format!("Delete failed: {}", e))?;

    // Evict the deleted file from both caches so the next list call doesn't
    // surface a stale entry (mtime-based invalidation alone can't catch a
    // delete — the file is gone, there's nothing to re-stat).
    {
        let cache = &features.kage_desktop_cache;
        cache.workspace_sessions.lock_or_recover().remove(path);
        cache.chat_files.lock_or_recover().remove(path);
    }

    info!("Deleted Kage Desktop session: {}", file_path);
    Ok(())
}

#[tauri::command]
pub async fn kage_desktop_open_folder(file_path: String) -> Result<(), AppError> {
    let path = std::path::Path::new(&file_path);
    let dir = path.parent().ok_or("No parent directory")?;
    Ok(crate::os::shell::open_path(&dir.to_string_lossy()).map_err(|e| e.to_string())?)
}

/// Load a .chat file directly (older format with full conversations).
#[tauri::command]
pub async fn kage_desktop_load_chat_file(file_path: String) -> Result<Vec<KageDesktopMessage>, AppError> {
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
            messages.push(KageDesktopMessage {
                role: normalized_role.to_string(),
                content: extracted,
            });
            continue;
        }

        messages.push(KageDesktopMessage {
            role: normalized_role.to_string(),
            content: content.to_string(),
        });
    }

    info!("Loaded .chat file {}: {} messages", file_path, messages.len());
    Ok(messages)
}

/// List .chat files from hash directories as additional sessions.
#[tauri::command]
pub async fn kage_desktop_chat_sessions(
    limit: Option<usize>,
    features: tauri::State<'_, crate::state::FeatureServices>,
) -> Result<Vec<KageDesktopSession>, AppError> {
    let base = kage_desktop_data_dir().ok_or("Kage Desktop not found")?;
    let limit = limit.unwrap_or(50);
    let cache = features.kage_desktop_cache.clone();

    tokio::task::spawn_blocking(move || scan_chat_sessions(&base, limit, &cache))
        .await
        .map_err(|e| format!("Scan task failed: {}", e))?
}

/// Blocking scan of `<base>/<hash>/*.chat`. Uses the per-file cache to
/// avoid re-reading + re-parsing files whose (mtime, size) fingerprint
/// matches a previous scan.
fn scan_chat_sessions(
    base: &Path,
    limit: usize,
    cache: &KageDesktopCache,
) -> Result<Vec<KageDesktopSession>, AppError> {
    // Walk the hash directories and build the list of `.chat` files we
    // currently see on disk.
    let mut seen_files: Vec<(PathBuf, FileFingerprint, String)> = Vec::new();
    let entries = std::fs::read_dir(base).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.len() != 32 || !name.chars().all(|c| c.is_ascii_hexdigit()) { continue; }

        let dir = entry.path();
        let Ok(files) = std::fs::read_dir(&dir) else { continue };

        for file in files.flatten() {
            let path = file.path();
            if path.extension().map(|e| e != "chat").unwrap_or(true) { continue; }
            let Ok(md) = file.metadata() else { continue };
            let Some(fp) = fingerprint(&md) else { continue };
            seen_files.push((path, fp, name.clone()));
        }
    }

    // Check cache hits under a short lock.
    let mut sessions: Vec<KageDesktopSession> = Vec::with_capacity(seen_files.len());
    let mut misses: Vec<(PathBuf, FileFingerprint, String)> = Vec::new();
    let seen_keys: std::collections::HashSet<PathBuf> =
        seen_files.iter().map(|(p, _, _)| p.clone()).collect();
    {
        let mut guard = cache.chat_files.lock_or_recover();
        guard.retain(|k, _| seen_keys.contains(k));
        for (path, fp, hash) in seen_files {
            match guard.get(&path) {
                Some(cached) if cached.fp == fp => sessions.push(cached.session.clone()),
                _ => misses.push((path, fp, hash)),
            }
        }
    }

    // Parse missed files without holding the cache lock.
    let mut fresh: Vec<(PathBuf, CachedSession)> = Vec::with_capacity(misses.len());
    for (path, fp, hash) in misses {
        let Some(session) = parse_chat_session(&path, &hash) else { continue };
        fresh.push((path, CachedSession { fp, session: session.clone() }));
        sessions.push(session);
    }

    if !fresh.is_empty() {
        let mut guard = cache.chat_files.lock_or_recover();
        for (path, entry) in fresh {
            guard.insert(path, entry);
        }
    }

    // Group by workflow — keep only the latest .chat file per workflow
    // (the last file has the full accumulated conversation)
    let mut by_workflow: std::collections::HashMap<String, KageDesktopSession> = std::collections::HashMap::new();

    for s in sessions {
        // Use workflowId for dedup when available, fall back to workspace:title
        let key = match s.workflow_id {
            Some(ref wid) => format!("{}:{}", s.workspace, wid),
            None => format!("{}:{}", s.workspace, s.title),
        };
        if let Some(existing) = by_workflow.get(&key) {
            // Keep the one with more messages (later in the conversation)
            if s.message_count > existing.message_count {
                by_workflow.insert(key, s);
            }
        } else {
            by_workflow.insert(key, s);
        }
    }

    let mut deduped: Vec<KageDesktopSession> = by_workflow.into_values().collect();
    deduped.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    deduped.truncate(limit);
    Ok(deduped)
}

/// Parse a single `.chat` file (partial JSON is tolerated and falls back to
/// filesystem metadata for timestamps). Returns `None` if the file is
/// empty or can't be opened at all.
fn parse_chat_session(path: &Path, hash_dir: &str) -> Option<KageDesktopSession> {
    let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();

    // Only read the first 50KB — enough for metadata and first few messages
    let content = read_file_head(path, 50_000)?;

    // Try to parse — may be truncated, so wrap in a recovery
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => {
            // Truncated JSON — fall back to file metadata for date
            let updated_at = std::fs::metadata(path).and_then(|m| m.modified()).ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                    .map(|dt| dt.to_rfc3339()).unwrap_or_default())
                .unwrap_or_default();
            return Some(KageDesktopSession {
                id,
                title: "Untitled".into(),
                workspace: hash_dir.to_string(),
                workspace_encoded: hash_dir.to_string(),
                updated_at,
                message_count: 0,
                session_type: "chat".into(),
                model: String::new(),
                file_path: path.to_string_lossy().to_string(),
                workflow_id: None,
            });
        }
    };

    let chat = json.get("chat").and_then(|c| c.as_array())?;
    if chat.is_empty() { return None; }

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
            let t = t.replace('\n', " ").replace('\r', " ");
            if c.len() > 80 { format!("{}...", t.trim()) } else { t.trim().to_string() }
        })
        .unwrap_or_else(|| "Untitled".to_string());

    let message_count = chat.len();
    let model = json.get("metadata").and_then(|m| m.get("modelId")).and_then(|m| m.as_str()).unwrap_or("").to_string();
    let workflow_id = json.get("metadata").and_then(|m| m.get("workflowId")).and_then(|w| w.as_str()).map(|s| s.to_string());

    let start_time = json.get("metadata").and_then(|m| m.get("startTime")).and_then(|t| t.as_i64()).unwrap_or(0);
    let end_time = json.get("metadata").and_then(|m| m.get("endTime")).and_then(|t| t.as_i64()).unwrap_or(start_time);
    let updated_at = if end_time > 0 {
        chrono::DateTime::from_timestamp(end_time / 1000, 0)
            .map(|dt| dt.to_rfc3339()).unwrap_or_default()
    } else if start_time > 0 {
        chrono::DateTime::from_timestamp(start_time / 1000, 0)
            .map(|dt| dt.to_rfc3339()).unwrap_or_default()
    } else {
        std::fs::metadata(path).and_then(|m| m.modified()).ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default())
            .unwrap_or_default()
    };

    Some(KageDesktopSession {
        id,
        title,
        workspace: hash_dir.to_string(),
        workspace_encoded: hash_dir.to_string(),
        updated_at,
        message_count,
        session_type: "chat".to_string(),
        model,
        file_path: path.to_string_lossy().to_string(),
        workflow_id,
    })
}

// ---------------------------------------------------------------------------
// Kage CLI session support (SQLite)
// ---------------------------------------------------------------------------

/// Get the kage-cli SQLite database path.
fn kage_cli_db_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    { std::env::var("LOCALAPPDATA").ok().map(|d| PathBuf::from(d).join("kage-cli").join("data.sqlite3")) }
    #[cfg(not(target_os = "windows"))]
    { dirs::home_dir().map(|d| d.join(".local").join("share").join("kage-cli").join("data.sqlite3")) }
}

#[tauri::command]
pub async fn kage_cli_available() -> bool {
    kage_cli_db_path().map(|p| p.exists()).unwrap_or(false)
}

#[tauri::command]
pub async fn kage_cli_sessions(limit: Option<usize>) -> Result<Vec<KageDesktopSession>, AppError> {
    let db_path = kage_cli_db_path().ok_or("kage-cli database not found")?;
    if !db_path.exists() { return Ok(Vec::new()); }

    let limit = limit.unwrap_or(50);

    // Open read-only with SQLITE_OPEN_READONLY to avoid locking issues
    let db = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ).map_err(|e| format!("SQLite open: {}", e))?;

    let mut stmt = db.prepare(
        "SELECT key, conversation_id, value, created_at, updated_at \
         FROM conversations_v2 ORDER BY updated_at DESC LIMIT ?1"
    ).map_err(|e| format!("SQLite prepare: {}", e))?;

    let mut sessions = Vec::new();
    let rows = stmt.query_map([limit as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,  // key (workspace)
            row.get::<_, String>(1)?,  // conversation_id
            row.get::<_, String>(2)?,  // value (JSON)
            row.get::<_, i64>(3)?,     // created_at
            row.get::<_, i64>(4)?,     // updated_at
        ))
    }).map_err(|e| format!("SQLite query: {}", e))?;

    for row in rows {
        let (workspace, conv_id, value_json, _created_at, updated_at) = match row {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Extract title from transcript (first user message)
        let title = extract_cli_title(&value_json);
        let message_count = count_cli_messages(&value_json);

        let updated_at_str = chrono::DateTime::from_timestamp(updated_at / 1000, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();

        sessions.push(KageDesktopSession {
            id: conv_id,
            title,
            workspace: workspace.clone(),
            workspace_encoded: workspace,
            updated_at: updated_at_str,
            message_count,
            session_type: "cli".to_string(),
            model: String::new(),
            file_path: db_path.to_string_lossy().to_string(),
            workflow_id: None,
        });
    }

    Ok(sessions)
}

fn extract_cli_title(value_json: &str) -> String {
    // Quick parse — just get the first transcript entry
    let json: serde_json::Value = match serde_json::from_str(value_json) {
        Ok(v) => v,
        Err(_) => return "Untitled".to_string(),
    };
    if let Some(transcript) = json.get("transcript").and_then(|t| t.as_array()) {
        if let Some(first) = transcript.first().and_then(|t| t.as_str()) {
            let clean = first.trim().trim_start_matches('>').trim();
            let title: String = clean.chars().take(80).collect();
            let title = title.replace('\n', " ").replace('\r', " ");
            return if clean.len() > 80 { format!("{}...", title.trim()) } else { title.trim().to_string() };
        }
    }
    "Untitled".to_string()
}

fn count_cli_messages(value_json: &str) -> usize {
    // Count transcript entries (quick without full parse)
    let json: serde_json::Value = match serde_json::from_str(value_json) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    json.get("transcript").and_then(|t| t.as_array()).map(|a| a.len()).unwrap_or(0)
}

#[tauri::command]
pub async fn kage_cli_load_session(conversation_id: String) -> Result<Vec<KageDesktopMessage>, AppError> {
    let db_path = kage_cli_db_path().ok_or("kage-cli database not found")?;

    let db = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ).map_err(|e| format!("SQLite open: {}", e))?;

    let value_json: String = db.query_row(
        "SELECT value FROM conversations_v2 WHERE conversation_id = ?1",
        [&conversation_id],
        |row| row.get(0),
    ).map_err(|e| format!("SQLite query: {}", e))?;

    let json: serde_json::Value = serde_json::from_str(&value_json)
        .map_err(|e| format!("JSON parse: {}", e))?;

    let mut messages = Vec::new();

    // Use the structured history field — it has proper roles and tool details
    if let Some(history) = json.get("history").and_then(|h| h.as_array()) {
        for entry in history {
            // Extract user message
            if let Some(user) = entry.get("user") {
                if let Some(content) = user.get("content") {
                    if let Some(prompt) = content.get("Prompt").and_then(|p| p.get("prompt")).and_then(|p| p.as_str()) {
                        if !prompt.is_empty() {
                            messages.push(KageDesktopMessage {
                                role: "user".to_string(),
                                content: prompt.to_string(),
                            });
                        }
                    }
                    // Tool results — show as tool messages
                    if let Some(tool_results) = content.get("ToolUseResults").and_then(|t| t.get("tool_use_results")).and_then(|t| t.as_array()) {
                        for tr in tool_results {
                            let tool_content = tr.get("content").and_then(|c| c.as_array())
                                .map(|arr| arr.iter().filter_map(|item| {
                                    item.get("Text").and_then(|t| t.as_str()).or_else(|| {
                                        item.get("Json").map(|_j| "").filter(|_| false) // skip JSON for now
                                    })
                                }).collect::<Vec<_>>().join("\n"))
                                .unwrap_or_default();
                            if !tool_content.is_empty() {
                                messages.push(KageDesktopMessage {
                                    role: "tool".to_string(),
                                    content: tool_content,
                                });
                            }
                        }
                    }
                }
            }

            // Extract assistant message
            if let Some(assistant) = entry.get("assistant") {
                // ToolUse — assistant is calling tools
                if let Some(tool_use) = assistant.get("ToolUse") {
                    let content = tool_use.get("content").and_then(|c| c.as_str()).unwrap_or("");
                    let tool_uses = tool_use.get("tool_uses").and_then(|t| t.as_array());

                    // Show the assistant's text (if any)
                    if !content.is_empty() {
                        messages.push(KageDesktopMessage {
                            role: "assistant".to_string(),
                            content: content.to_string(),
                        });
                    }

                    // Show tool calls with names and args
                    if let Some(tools) = tool_uses {
                        for tool in tools {
                            let name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
                            let args = tool.get("args").map(|a| serde_json::to_string_pretty(a).unwrap_or_default()).unwrap_or_default();
                            messages.push(KageDesktopMessage {
                                role: "tool".to_string(),
                                content: format!("🔧 {} {}", name, args),
                            });
                        }
                    }
                }
                // Response — final assistant message
                if let Some(response) = assistant.get("Response") {
                    let content = response.get("content").and_then(|c| c.as_str()).unwrap_or("");
                    if !content.is_empty() {
                        messages.push(KageDesktopMessage {
                            role: "assistant".to_string(),
                            content: content.to_string(),
                        });
                    }
                }
                // Message — simple assistant message
                if let Some(msg) = assistant.get("Message") {
                    let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
                    if !content.is_empty() {
                        messages.push(KageDesktopMessage {
                            role: "assistant".to_string(),
                            content: content.to_string(),
                        });
                    }
                }
            }
        }
    }

    info!("Loaded kage-cli session {}: {} messages", conversation_id, messages.len());
    Ok(messages)
}

/// Check if a kage-cli conversation has been updated since a given timestamp.
/// Returns the new updated_at if changed, or None if unchanged.
#[tauri::command]
pub async fn kage_cli_check_updated(
    conversation_id: String,
    last_updated_at: i64,
) -> Result<Option<i64>, AppError> {
    let db_path = kage_cli_db_path().ok_or("kage-cli database not found")?;

    let db = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ).map_err(|e| format!("SQLite open: {}", e))?;

    let current: i64 = db.query_row(
        "SELECT updated_at FROM conversations_v2 WHERE conversation_id = ?1",
        [&conversation_id],
        |row| row.get(0),
    ).map_err(|e| format!("SQLite query: {}", e))?;

    if current > last_updated_at {
        Ok(Some(current))
    } else {
        Ok(None)
    }
}
