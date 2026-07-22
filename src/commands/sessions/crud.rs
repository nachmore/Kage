//! Session CRUD: list/load/delete sessions, the directory watcher, ACP
//! session switch/create, and the per-window session pin commands. Title
//! resolution lives in [`super::titles`].

use super::*;

mod acp;

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
/// Handle returned from `start_session_watcher`. Dropping the handle
/// signals the watcher thread to exit (which drops the inner `Watcher`
/// and unsubscribes from FSEvents/inotify/ReadDirectoryChangesW). Held
/// in a process-wide static so the Tauri `RunEvent::Exit` hook can
/// drop it during clean shutdown.
pub struct SessionWatcherHandle {
    /// Closing this channel wakes the thread out of its `recv()` and
    /// makes it exit. The receiver lives on the watcher thread.
    _shutdown_tx: std::sync::mpsc::Sender<()>,
}

pub fn start_session_watcher(
    session_cache: std::sync::Arc<std::sync::Mutex<Option<SessionCache>>>,
    app_handle: tauri::AppHandle,
) -> Option<SessionWatcherHandle> {
    use notify::{Event, EventKind, RecursiveMode, Watcher};

    let sessions_dir = match crate::agent_presets::default_sessions_dir() {
        Some(dir) => dir,
        None => {
            log::warn!("Cannot start session watcher: no home directory");
            return None;
        }
    };

    if !sessions_dir.exists() {
        // Create the directory so the watcher has something to watch
        let _ = fs::create_dir_all(&sessions_dir);
    }

    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();

    std::thread::Builder::new()
        .name("session-watcher".into())
        .spawn(move || {
            // Debounce: ignore events within 2s of the last invalidation
            let last_invalidation = std::sync::Mutex::new(
                std::time::Instant::now() - std::time::Duration::from_secs(10),
            );

            let cache = session_cache;
            let app = app_handle;

            let mut watcher =
                match notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
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
                    crate::event_targets::emit_to_chat_hosts(&app, "sessions_changed", &());
                }) {
                    Ok(w) => w,
                    Err(e) => {
                        log::error!("Failed to create session watcher: {}", e);
                        return;
                    }
                };

            if let Err(e) = watcher.watch(&sessions_dir, RecursiveMode::NonRecursive) {
                log::error!(
                    "Failed to watch sessions directory {:?}: {}",
                    sessions_dir,
                    e
                );
                return;
            }

            log::info!("Session watcher started on {:?}", sessions_dir);

            // Block until the shutdown sender is dropped. Any send is
            // ignored; we only care about the channel disconnecting.
            // When the function returns, `watcher` drops and the
            // platform-specific FS subscription is unregistered cleanly
            // (Core Foundation run loop on macOS, inotify fd on Linux,
            // ReadDirectoryChangesW handle on Windows). Pre-fix the
            // thread sat in `sleep(3600)` forever, so the watcher was
            // only ever cleaned up by process death.
            match shutdown_rx.recv() {
                Ok(()) | Err(_) => {
                    log::info!("Session watcher shutting down");
                }
            }
        })
        .expect("Failed to spawn session-watcher thread");

    Some(SessionWatcherHandle {
        _shutdown_tx: shutdown_tx,
    })
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
            info!(
                "Found {} sessions (returning {} from cache, offset {})",
                cached.sessions.len(),
                sessions.len(),
                offset.unwrap_or(0)
            );
            return Ok(sessions);
        }
    }

    // Scan and cache
    let sessions_dir = resolve_sessions_dir_locked(&features.config)?;
    let all_sessions = scan_sessions_in_dir(&sessions_dir)?;
    let total = all_sessions.len();

    // Store in cache
    {
        let mut cache = features.session_cache.lock_or_recover();
        *cache = Some(SessionCache {
            sessions: all_sessions.clone(),
        });
    }

    let sessions = paginate(&all_sessions, limit, offset);
    info!(
        "Found {} sessions (returning {}, offset {})",
        total,
        sessions.len(),
        offset.unwrap_or(0)
    );
    Ok(sessions)
}

fn paginate(
    sessions: &[SessionSummary],
    limit: Option<usize>,
    offset: Option<usize>,
) -> Vec<SessionSummary> {
    let offset = offset.unwrap_or(0);
    let iter = sessions.iter().skip(offset);
    match limit {
        Some(limit) => iter.take(limit).cloned().collect(),
        None => iter.cloned().collect(),
    }
}

fn scan_sessions_in_dir(sessions_dir: &PathBuf) -> Result<Vec<SessionSummary>, String> {
    if !sessions_dir.exists() {
        info!("Sessions directory does not exist yet: {:?}", sessions_dir);
        return Ok(vec![]);
    }

    let mut sessions: Vec<SessionSummary> = Vec::new();
    let title_cache = load_title_cache();
    // Entries extracted this scan. Kept separate from the snapshot and
    // merged under TITLE_CACHE_LOCK at the end — writing the whole
    // snapshot back would revert any entry a concurrent writer (rename,
    // AI summariser) persisted while we were scanning JSONLs.
    let mut new_entries: HashMap<String, TitleEntry> = HashMap::new();

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
                let updated = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| {
                        chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let created = meta
                    .created()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| {
                        chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                (created, updated)
            }
            Err(_) => (String::new(), String::new()),
        };

        // Use cached title if available, otherwise extract and cache.
        // Extracted entries are flagged as `Extracted` so the AI
        // summarizer (in `session_titler`) is permitted to upgrade them
        // later; `Manual` and `Ai` entries are off-limits to that path.
        // On cache miss, try recovering a prior AI summary from the
        // JSONL first — preserves titles across cache loss / sync /
        // upgrade without re-paying the agent for them.
        let title = if let Some(cached) = title_cache.get(&session_id) {
            cached.title.clone()
        } else {
            let jsonl_path = path.with_extension("jsonl");
            if let Some(recovered) = extract_ai_title_from_jsonl(&jsonl_path) {
                new_entries.insert(
                    session_id.clone(),
                    TitleEntry {
                        title: recovered.clone(),
                        source: TitleSource::Ai,
                    },
                );
                recovered
            } else {
                let extracted = extract_title_from_jsonl(&jsonl_path);
                if extracted != "New Chat" {
                    new_entries.insert(
                        session_id.clone(),
                        TitleEntry {
                            title: extracted.clone(),
                            source: TitleSource::Extracted,
                        },
                    );
                }
                extracted
            }
        };

        sessions.push(SessionSummary {
            session_id,
            title,
            created_at,
            updated_at,
        });
    }

    // Persist newly extracted entries. Re-load under the lock and merge
    // (entry API — an entry that appeared while we were scanning, e.g. a
    // user rename, wins over our extract) rather than writing back the
    // pre-scan snapshot.
    if !new_entries.is_empty() {
        let _guard = TITLE_CACHE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut cache = load_title_cache();
        for (id, entry) in new_entries {
            cache.entry(id).or_insert(entry);
        }
        save_title_cache(&cache);
    }

    // Sort by updated_at descending (most recent first)
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    Ok(sessions)
}

#[tauri::command]
pub async fn load_session(
    session_id: String,
    features: State<'_, FeatureServices>,
) -> Result<SessionData, AppError> {
    let sessions_dir = resolve_sessions_dir_locked(&features.config)?;
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
                    let end_ts = turn
                        .get("end_timestamp")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    if end_ts.is_empty() {
                        continue;
                    }

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
pub async fn get_sessions_directory(
    features: State<'_, FeatureServices>,
) -> Result<String, AppError> {
    let dir = resolve_sessions_dir_locked(&features.config)?;
    Ok(dir.to_string_lossy().to_string())
}

/// Open the session's JSON file in the system file explorer
#[tauri::command]
pub async fn reveal_session_file(
    session_id: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let sessions_dir = resolve_sessions_dir_locked(&features.config)?;
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
pub async fn delete_session<R: tauri::Runtime>(
    session_id: String,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    let sessions_dir = resolve_sessions_dir_locked(&features.config)?;

    for ext in &["json", "jsonl", "lock"] {
        let path = sessions_dir.join(format!("{}.{}", session_id, ext));
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("Failed to delete {}: {}", ext, e))?;
        }
    }

    // Remove from title cache (load→modify→save, so serialize with the
    // other title-cache writers).
    {
        let _guard = TITLE_CACHE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut cache = load_title_cache();
        if cache.remove(&session_id).is_some() {
            save_title_cache(&cache);
        }
    }

    // Invalidate session list cache
    {
        let mut cache = features.session_cache.lock_or_recover();
        *cache = None;
    }

    // Tell chat-host windows (main + chat-*): this session is gone.
    // Windows pinned to it clear their chat area and show a "no
    // longer exists" notice; others just refresh their sidebar list.
    crate::event_targets::emit_to_chat_hosts(
        &app,
        "session_changed",
        &serde_json::json!({
            "id": session_id,
            "kind": "deleted",
        }),
    );

    info!("Deleted session: {}", session_id);
    Ok(())
}

/// Adopt or create a session for the calling window.
///
/// The command wrapper remains in this facade so Tauri exports its generated
/// command symbol through the existing `commands::sessions` re-export.
#[tauri::command]
pub async fn switch_acp_session<R: tauri::Runtime>(
    session_id: Option<String>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    ui: State<'_, crate::state::UiState>,
    window: tauri::WebviewWindow<R>,
    app: tauri::AppHandle<R>,
) -> Result<String, AppError> {
    acp::switch_acp_session(session_id, acp, features, ui, window, app).await
}

/// Peek the in-flight turn on `session_id`: the user prompt that started
/// it and the response text streamed so far. Non-consuming — the backend
/// accumulator keeps filling and its usual take-at-completion readers are
/// unaffected. Used by the chat window when the user switches INTO a
/// session that is mid-stream: disk only has completed turns, so both the
/// user's own message and the partial response come from here and the
/// live chunk stream continues from that point. `text` is empty when
/// nothing is in flight (or the turn just completed and the bucket was
/// evicted); `prompt` is null for turns not started by a user prompt
/// (steering, titling).
#[tauri::command]
pub async fn get_session_stream_snapshot(
    session_id: String,
    acp: State<'_, AcpHandles>,
) -> Result<serde_json::Value, AppError> {
    Ok(serde_json::json!({
        "prompt": acp.client.peek_in_flight_prompt(&session_id),
        "text": acp.client.peek_session_accumulator(&session_id),
    }))
}

/// Read the session id pinned to a window. Frontends call this on
/// boot to discover their own pinned session, and call it for other
/// windows when implementing handoff (e.g. floating "expand to chat"
/// looks up `main`'s session).
#[tauri::command]
pub async fn get_window_session(
    label: String,
    ui: State<'_, crate::state::UiState>,
) -> Result<Option<String>, AppError> {
    let map = ui
        .window_sessions
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    Ok(map.get(&label).cloned())
}

/// Pin a session to a window. The frontend writes here on every adopt
/// (boot, switch, new) so the backend's quit-time hook, updater
/// resume-marker, and permission router can all look up "what session
/// does window X own?" without guessing.
///
/// Also updates the window title to reflect the session's first user
/// prompt (or "New Chat" when the session is empty). The single
/// authoritative path for "this window now shows this session" lives
/// here so frontends never have to coordinate `set_title` manually.
#[tauri::command]
pub async fn set_window_session<R: tauri::Runtime>(
    label: String,
    session_id: String,
    ui: State<'_, crate::state::UiState>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    {
        let mut map = ui
            .window_sessions
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        map.insert(label.clone(), session_id.clone());
    }
    update_window_title(
        &app,
        &features.config,
        &features.session_cache,
        &label,
        &session_id,
    );
    Ok(())
}

/// Drop a window's pinned session. Called when a window closes.
#[tauri::command]
pub async fn clear_window_session(
    label: String,
    ui: State<'_, crate::state::UiState>,
) -> Result<(), AppError> {
    let mut map = ui
        .window_sessions
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    map.remove(&label);
    Ok(())
}
