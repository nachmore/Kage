use crate::error::AppError;
use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, FeatureServices};
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
    /// Map of message_id → ISO timestamp (extracted from turn metadata)
    #[serde(default)]
    pub message_timestamps: HashMap<String, String>,
    /// Map of message_id → turn duration in seconds
    #[serde(default)]
    pub message_durations: HashMap<String, f64>,
}

/// Resolve the sessions directory from config.
/// Priority: 1) explicit sessions_directory, 2) agent preset, 3) probe common paths
fn get_sessions_dir_from_config(config: &crate::config::Config) -> Result<PathBuf, String> {
    crate::agent_presets::resolve_sessions_dir(config)
        .ok_or_else(|| "Failed to get home directory".to_string())
}

/// Fallback for callers without config access — probes common paths
fn get_sessions_dir() -> Result<PathBuf, String> {
    crate::agent_presets::default_sessions_dir()
        .ok_or_else(|| "Failed to get home directory".to_string())
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
                                // Skip timestamp injections — not meaningful titles
                                if trimmed.starts_with("[Current time:") {
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
    use std::io::{BufRead, BufReader};

    let mut messages = Vec::new();

    let file = match fs::File::open(jsonl_path) {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to open JSONL {:?}: {}", jsonl_path, e);
            return messages;
        }
    };

    let reader = BufReader::new(file);

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to read JSONL line: {}", e);
                continue;
            }
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let val: serde_json::Value = match serde_json::from_str(&line) {
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

/// Cached session list, invalidated by the file watcher or explicit mutations.
pub struct SessionCache {
    pub sessions: Vec<SessionSummary>,
}

/// Start a background file watcher on the sessions directory.
/// When files change, invalidates the session cache and emits a Tauri event
/// so the frontend can refresh the session list.
pub fn start_session_watcher(
    session_cache: std::sync::Arc<std::sync::Mutex<Option<SessionCache>>>,
    app_handle: tauri::AppHandle,
) {
    use notify::{Watcher, RecursiveMode, Event, EventKind};
    use tauri::Emitter;

    let sessions_dir = match crate::agent_presets::default_sessions_dir() {
        Some(dir) => dir,
        None => {
            log::warn!("Cannot start session watcher: no home directory");
            return;
        }
    };

    if !sessions_dir.exists() {
        // Create the directory so the watcher has something to watch
        let _ = fs::create_dir_all(&sessions_dir);
    }

    std::thread::Builder::new().name("session-watcher".into()).spawn(move || {
        // Debounce: ignore events within 2s of the last invalidation
        let last_invalidation = std::sync::Mutex::new(std::time::Instant::now()
            - std::time::Duration::from_secs(10));

        let cache = session_cache;
        let app = app_handle;

        let mut watcher = match notify::recommended_watcher(
            move |res: Result<Event, notify::Error>| {
                let event = match res {
                    Ok(e) => e,
                    Err(e) => {
                        log::warn!("Session watcher error: {}", e);
                        return;
                    }
                };

                // Only care about creates, removes, and modifications to .json/.jsonl files
                let dominated = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
                );
                if !dominated {
                    return;
                }

                let dominated_ext = event.paths.iter().any(|p| {
                    p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e == "json" || e == "jsonl")
                        .unwrap_or(false)
                });
                if !dominated_ext {
                    return;
                }

                // Debounce
                {
                    let mut last = last_invalidation.lock_or_recover();
                    if last.elapsed() < std::time::Duration::from_secs(2) {
                        return;
                    }
                    *last = std::time::Instant::now();
                }

                log::info!("Session directory changed, invalidating cache");
                if let Ok(mut c) = cache.lock() {
                    *c = None;
                }
                let _ = app.emit("sessions_changed", ());
            },
        ) {
            Ok(w) => w,
            Err(e) => {
                log::error!("Failed to create session watcher: {}", e);
                return;
            }
        };

        if let Err(e) = watcher.watch(&sessions_dir, RecursiveMode::NonRecursive) {
            log::error!("Failed to watch sessions directory {:?}: {}", sessions_dir, e);
            return;
        }

        log::info!("Session watcher started on {:?}", sessions_dir);

        // Keep the thread alive — the watcher is dropped when this thread exits
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }).expect("Failed to spawn session-watcher thread");
}

#[tauri::command]
pub async fn list_sessions(
    limit: Option<usize>,
    offset: Option<usize>,
    force: Option<bool>,
    features: State<'_, FeatureServices>,
) -> Result<Vec<SessionSummary>, AppError> {
    let force = force.unwrap_or(false);

    // Serve from cache unless invalidated by the file watcher or a force refresh
    if !force {
        let cache = features.session_cache.lock_or_recover();
        if let Some(ref cached) = *cache {
            let sessions = paginate(&cached.sessions, limit, offset);
            info!("Found {} sessions (returning {} from cache, offset {})",
                cached.sessions.len(), sessions.len(), offset.unwrap_or(0));
            return Ok(sessions);
        }
    }

    // Scan and cache
    let config = features.config.lock_or_recover().clone();
    let all_sessions = scan_sessions_with_config(&config)?;
    let total = all_sessions.len();

    // Store in cache
    {
        let mut cache = features.session_cache.lock_or_recover();
        *cache = Some(SessionCache {
            sessions: all_sessions.clone(),
        });
    }

    let sessions = paginate(&all_sessions, limit, offset);
    info!("Found {} sessions (returning {}, offset {})", total, sessions.len(), offset.unwrap_or(0));
    Ok(sessions)
}

fn paginate(sessions: &[SessionSummary], limit: Option<usize>, offset: Option<usize>) -> Vec<SessionSummary> {
    let offset = offset.unwrap_or(0);
    let iter = sessions.iter().skip(offset);
    match limit {
        Some(limit) => iter.take(limit).cloned().collect(),
        None => iter.cloned().collect(),
    }
}

/// Scan the sessions directory using auto-detected path.
fn scan_sessions_with_config(config: &crate::config::Config) -> Result<Vec<SessionSummary>, String> {
    let sessions_dir = get_sessions_dir_from_config(config)?;
    scan_sessions_in_dir(&sessions_dir)
}

fn scan_sessions_in_dir(sessions_dir: &PathBuf) -> Result<Vec<SessionSummary>, String> {

    if !sessions_dir.exists() {
        info!("Sessions directory does not exist yet: {:?}", sessions_dir);
        return Ok(vec![]);
    }

    let mut sessions: Vec<SessionSummary> = Vec::new();
    let mut title_cache = load_title_cache();
    let mut cache_dirty = false;

    let entries = fs::read_dir(sessions_dir.as_path()).map_err(|e| {
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

    Ok(sessions)
}

#[tauri::command]
pub async fn load_session(session_id: String, features: State<'_, FeatureServices>) -> Result<SessionData, AppError> {
    let config = features.config.lock_or_recover().clone();
    let sessions_dir = get_sessions_dir_from_config(&config)?;
    let json_path = sessions_dir.join(format!("{}.json", session_id));
    let jsonl_path = sessions_dir.join(format!("{}.jsonl", session_id));

    info!("Loading session: {}", session_id);

    if !json_path.exists() {
        return Err(format!("Session not found: {}", session_id).into());
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

    // Extract message timestamps and durations from turn metadata
    let mut message_timestamps: HashMap<String, String> = HashMap::new();
    let mut message_durations: HashMap<String, f64> = HashMap::new();
    if let Some(state) = metadata.get("session_state") {
        if let Some(conv) = state.get("conversation_metadata") {
            if let Some(turns) = conv.get("user_turn_metadatas").and_then(|t| t.as_array()) {
                for turn in turns {
                    let end_ts = turn.get("end_timestamp")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    if end_ts.is_empty() { continue; }

                    // Extract turn duration
                    let duration_secs = turn.get("turn_duration").map(|d| {
                        let secs = d.get("secs").and_then(|s| s.as_f64()).unwrap_or(0.0);
                        let nanos = d.get("nanos").and_then(|n| n.as_f64()).unwrap_or(0.0);
                        secs + nanos / 1_000_000_000.0
                    });

                    if let Some(ids) = turn.get("message_ids").and_then(|m| m.as_array()) {
                        for id in ids {
                            if let Some(id_str) = id.as_str() {
                                message_timestamps.insert(id_str.to_string(), end_ts.to_string());
                                if let Some(dur) = duration_secs {
                                    message_durations.insert(id_str.to_string(), dur);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(SessionData {
        session_id,
        created_at,
        updated_at,
        messages,
        message_timestamps,
        message_durations,
    })
}

/// Get the sessions directory path
#[tauri::command]
pub async fn get_sessions_directory(features: State<'_, FeatureServices>) -> Result<String, AppError> {
    let config = features.config.lock_or_recover().clone();
    let dir = get_sessions_dir_from_config(&config)?;
    Ok(dir.to_string_lossy().to_string())
}

/// Open the session's JSON file in the system file explorer
#[tauri::command]
pub async fn reveal_session_file(session_id: String, features: State<'_, FeatureServices>) -> Result<(), AppError> {
    let config = features.config.lock_or_recover().clone();
    let sessions_dir = get_sessions_dir_from_config(&config)?;
    let json_path = sessions_dir.join(format!("{}.json", session_id));

    if !json_path.exists() {
        return Err("Session file not found".to_string().into());
    }

    let path_str = json_path.to_string_lossy().to_string();

    crate::os::reveal_in_file_manager(&path_str)
        .map_err(|e| format!("Failed to reveal file: {}", e))?;

    Ok(())
}

/// Delete a session's files (.json, .jsonl, .lock)
#[tauri::command]
pub async fn delete_session(session_id: String, features: State<'_, FeatureServices>) -> Result<(), AppError> {
    let config = features.config.lock_or_recover().clone();
    let sessions_dir = get_sessions_dir_from_config(&config)?;

    for ext in &["json", "jsonl", "lock"] {
        let path = sessions_dir.join(format!("{}.{}", session_id, ext));
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("Failed to delete {}: {}", ext, e))?;
        }
    }

    // Remove from title cache
    if let Ok(cache_path) = get_title_cache_path() {
        if cache_path.exists() {
            if let Ok(content) = fs::read_to_string(&cache_path) {
                if let Ok(mut cache) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(obj) = cache.as_object_mut() {
                        obj.remove(&session_id);
                        let _ = fs::write(&cache_path, serde_json::to_string_pretty(&cache).unwrap_or_default());
                    }
                }
            }
        }
    }

    // Invalidate session list cache
    {
        let mut cache = features.session_cache.lock_or_recover();
        *cache = None;
    }

    info!("Deleted session: {}", session_id);
    Ok(())
}


/// Switch the ACP client to a different session.
/// If session_id is provided, loads that session via session/load.
/// If session_id is None, creates a new session via session/new.
/// Saves the floating session before switching so it can be restored.
#[tauri::command]
pub async fn switch_acp_session(
    session_id: Option<String>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    ui: State<'_, crate::state::UiState>,
) -> Result<String, AppError> {
    let client_guard = acp.client.clone();

    // Ensure connected
    if !client_guard.is_connected() {
        info!("Not connected, attempting to connect for session switch...");
        if let Err(e) = client_guard.connect() {
            error!("Connection failed: {}", e);
            return Err(AppError::connection_lost(format!("Failed to connect: {}", e)));
        }
    }

    // Save the current session as floating session if we don't have one yet
    {
        let mut floating = ui
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
                let config = features.config.lock_or_recover().clone();
                let sessions_dir = get_sessions_dir_from_config(&config)?;
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
                .map_err(|e| AppError::internal(format!("Failed to load session: {}", e)))
        }
        None => {
            info!("Creating new session");
            let cwd = {
                let cfg = features.config.lock_or_recover();
                cfg.acp.agent.working_directory.clone()
            };
            let (new_session_id, models_json) = client_guard
                .create_session(cwd)
                .map_err(|e| format!("Failed to create session: {}", e))?;

            // Store available models
            if let Ok(parsed) = serde_json::from_value::<Vec<crate::state::AcpModel>>(
                serde_json::Value::Array(models_json),
            ) {
                if let Ok(mut m) = acp.available_models.lock() {
                    *m = parsed;
                }
            }

            // Apply default model if configured
            let cfg = features.config.lock_or_recover();
            if let Some(ref default_model) = cfg.acp.agent.default_model {
                if !default_model.is_empty() {
                    info!("Applying default model to new session: {}", default_model);
                    let result = client_guard.send_request(
                        "_kage.dev/commands/execute",
                        serde_json::json!({
                            "sessionId": new_session_id,
                            "command": { "command": "model", "args": { "modelName": default_model } }
                        }),
                    );
                    match result {
                        Ok(_) => info!("Default model applied: {}", default_model),
                        Err(e) => error!("Failed to apply default model: {}", e),
                    }
                }
            }

            // Send steering documents to the new session
            {
                let parts = crate::commands::system::assemble_steering_parts(&cfg);
                let steering_msg = format!(
                    "{} {}",
                    crate::commands::system::STEERING_MSG_PREFIX,
                    parts.join("\n\n---\n\n")
                );
                let _ = client_guard.send_chat_streaming(&steering_msg, None);
            }

            // Invalidate session list cache (new session was created)
            {
                let mut cache = features.session_cache.lock_or_recover();
                *cache = None;
            }

            Ok(new_session_id)
        }
    }
}

/// Get the current ACP session ID
#[tauri::command]
pub async fn get_current_session_id(
    acp: State<'_, AcpHandles>,
) -> Result<Option<String>, AppError> {
    // The pre-2026-05 codebase had this guarded by try_lock to avoid blocking
    // behind in-flight prompts on the outer mutex. AcpClient is now an
    // Arc<AcpClient> with internal locks scoped to the bits that need them,
    // so this is just a read.
    Ok(acp.client.get_session_id())
}

/// Get the floating window's session ID
#[tauri::command]
pub async fn get_floating_session_id(
    ui: State<'_, crate::state::UiState>,
) -> Result<Option<String>, AppError> {
    let floating = ui
        .floating_session_id
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    Ok(floating.clone())
}

/// Restore the floating session as the active ACP session
#[tauri::command]
pub async fn restore_floating_session(
    acp: State<'_, AcpHandles>,
    ui: State<'_, crate::state::UiState>,
) -> Result<Option<String>, AppError> {
    let floating_id = {
        let floating = ui
            .floating_session_id
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        floating.clone()
    };

    if let Some(ref id) = floating_id {
        acp.client.set_session_id(Some(id.clone()));
        info!("Restored floating session: {}", id);
    }

    Ok(floating_id)
}

/// Rename a session by updating its title in the cache
#[tauri::command]
pub async fn rename_session(
    session_id: String,
    title: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("Title cannot be empty".to_string().into());
    }

    info!("Renaming session {} to: {}", session_id, title);

    let mut title_cache = load_title_cache();
    title_cache.insert(session_id, title);
    save_title_cache(&title_cache);

    // Invalidate session list cache
    {
        let mut session_cache = features.session_cache.lock_or_recover();
        *session_cache = None;
    }

    Ok(())
}
