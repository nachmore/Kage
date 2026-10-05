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
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut val: serde_json::Value = match serde_json::from_str(line) {
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

        // Move subtrees out of `val` rather than cloning them — tool-result
        // lines can be megabytes, and `val` is discarded after this line.
        let mut data = val
            .get_mut("data")
            .map(serde_json::Value::take)
            .unwrap_or(serde_json::Value::Null);

        let message_id = data
            .get("message_id")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();

        let content_arr = match data.get_mut("content").map(serde_json::Value::take) {
            Some(serde_json::Value::Array(items)) => items,
            _ => Vec::new(),
        };

        let content: Vec<MessageContent> = content_arr
            .into_iter()
            .map(|mut item| {
                let item_kind = item
                    .get("kind")
                    .and_then(|k| k.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let item_data = item
                    .get_mut("data")
                    .map(serde_json::Value::take)
                    .unwrap_or(serde_json::Value::Null);
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
    /// Shared with the notify callback; the handle's `Drop` sends
    /// `Shutdown` on it so the thread exits.
    tx: std::sync::mpsc::Sender<WatcherMsg>,
}

impl Drop for SessionWatcherHandle {
    fn drop(&mut self) {
        // The notify callback holds its own Sender clone, so the channel
        // never disconnects while the watcher lives — shutdown has to be
        // an explicit message. Err just means the thread already exited.
        let _ = self.tx.send(WatcherMsg::Shutdown);
    }
}

/// Messages into the watcher thread.
enum WatcherMsg {
    /// A session file changed (already filtered by `is_session_file_path`).
    Changed,
    Shutdown,
}

/// Flush once the directory has been quiet this long.
const WATCH_QUIET: std::time::Duration = std::time::Duration::from_millis(400);
/// ...or after this long of continuous writes, so a long burst still
/// refreshes the sidebar periodically.
const WATCH_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Trailing-edge debounce for watcher events. The agent writes a turn as
/// a burst (JSONL appends, then the .json metadata); invalidating on the
/// first event and dropping the rest let the frontend's rescan race the
/// burst and cache the half-written state with nothing re-firing after.
/// Flushing after the burst means its last write is always reflected.
#[derive(Default)]
struct TrailingDebounce {
    first: Option<std::time::Instant>,
    last: Option<std::time::Instant>,
}

impl TrailingDebounce {
    fn note(&mut self, now: std::time::Instant) {
        self.first = Some(self.first.unwrap_or(now));
        self.last = Some(now);
    }

    /// Time left until the pending burst is due; `None` when idle.
    fn remaining(&self, now: std::time::Instant) -> Option<std::time::Duration> {
        let (first, last) = (self.first?, self.last?);
        let due = (last + WATCH_QUIET).min(first + WATCH_MAX_WAIT);
        Some(due.saturating_duration_since(now))
    }

    /// True (and resets) when a pending burst is due at `now`.
    fn take_due(&mut self, now: std::time::Instant) -> bool {
        if self.remaining(now).is_some_and(|left| left.is_zero()) {
            *self = Self::default();
            true
        } else {
            false
        }
    }
}

/// Whether a watcher event path is an agent session file. Dot-files are
/// Kage's own (`.title-cache.json` and its `.json.tmp.<pid>` siblings),
/// whose writers already invalidate the cache themselves — reacting to
/// them only re-triggers a full rescan in every window.
fn is_session_file_path(path: &std::path::Path) -> bool {
    let session_ext = matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("json" | "jsonl")
    );
    let hidden = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'));
    session_ext && !hidden
}

pub fn start_session_watcher(
    session_cache: std::sync::Arc<std::sync::Mutex<Option<SessionCache>>>,
    app_handle: tauri::AppHandle,
) -> Option<SessionWatcherHandle> {
    use notify::{Event, EventKind, RecursiveMode, Watcher};
    use std::sync::mpsc::RecvTimeoutError;

    // Watch the same directory list_sessions scans (honours the active
    // connection's `sessions_directory` override), not just the default.
    let sessions_dir = match app_handle
        .try_state::<FeatureServices>()
        .and_then(|features| resolve_sessions_dir_locked(&features.config).ok())
        .or_else(crate::agent_presets::default_sessions_dir)
    {
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

    let (tx, rx) = std::sync::mpsc::channel::<WatcherMsg>();
    let event_tx = tx.clone();

    std::thread::Builder::new()
        .name("session-watcher".into())
        .spawn(move || {
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

                    // Only care about creates, removes, and modifications to session files
                    let dominated = matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
                    );
                    if !dominated {
                        return;
                    }
                    if !event
                        .paths
                        .iter()
                        .any(|p| is_session_file_path(p.as_path()))
                    {
                        return;
                    }
                    // Err means the thread is shutting down.
                    let _ = event_tx.send(WatcherMsg::Changed);
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

            // Block for events (no polling while idle), debounce bursts,
            // and exit on Shutdown. When the closure returns, `watcher`
            // drops and the platform-specific FS subscription is
            // unregistered cleanly (Core Foundation run loop on macOS,
            // inotify fd on Linux, ReadDirectoryChangesW handle on
            // Windows).
            let mut debounce = TrailingDebounce::default();
            loop {
                let msg = match debounce.remaining(std::time::Instant::now()) {
                    None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    Some(wait) => rx.recv_timeout(wait),
                };
                match msg {
                    Ok(WatcherMsg::Changed) => debounce.note(std::time::Instant::now()),
                    Ok(WatcherMsg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                if debounce.take_due(std::time::Instant::now()) {
                    log::info!("Session directory changed, invalidating cache");
                    invalidate_session_cache(&cache);
                    crate::event_targets::emit_to_chat_hosts(&app, "sessions_changed", &());
                }
            }
            log::info!("Session watcher shutting down");
        })
        .expect("Failed to spawn session-watcher thread");

    Some(SessionWatcherHandle { tx })
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
        if let Some(sessions) = cached_page(&features.session_cache, limit, offset) {
            return Ok(sessions);
        }
    }

    // The scan is blocking FS work (read_dir, a stat per session, JSONL
    // reads on title-cache misses) — keep it off the async runtime.
    let session_cache = features.session_cache.clone();
    let config = features.config.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<SessionSummary>, AppError> {
        // Single-flight: every chat-host window refreshes on the same
        // event, so concurrent callers queue here and all but the first
        // are served from the cache that scan just filled.
        let _scan = SCAN_LOCK.lock_or_recover();
        if !force {
            if let Some(sessions) = cached_page(&session_cache, limit, offset) {
                return Ok(sessions);
            }
        }

        let epoch = CACHE_EPOCH.load(std::sync::atomic::Ordering::SeqCst);
        let sessions_dir = resolve_sessions_dir_locked(&config)?;
        let all_sessions = scan_sessions_in_dir(&sessions_dir)?;
        let total = all_sessions.len();
        let sessions = paginate(&all_sessions, limit, offset);
        {
            // Only cache if nothing invalidated mid-scan: otherwise this
            // pre-change listing would be served to the callers the
            // invalidation's event is about to send here.
            let mut cache = session_cache.lock_or_recover();
            if CACHE_EPOCH.load(std::sync::atomic::Ordering::SeqCst) == epoch {
                *cache = Some(SessionCache {
                    sessions: all_sessions,
                });
            }
        }

        info!(
            "Found {} sessions (returning {}, offset {})",
            total,
            sessions.len(),
            offset.unwrap_or(0)
        );
        Ok(sessions)
    })
    .await
    .map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.task.failed",
            &[("reason", &e.to_string())],
        )
    })?
}

/// Serializes session-directory scans; see `list_sessions`.
static SCAN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Bumped by `invalidate_session_cache` (under the cache lock). A scan
/// that sees it change while running returns its result but doesn't
/// cache it, so with single-flight a listing that started before a
/// watcher flush / delete can't be served after it.
static CACHE_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Clear the session-list cache and fence off any scan in flight. Every
/// invalidation must go through here: a bare `*cache = None` skips the
/// epoch bump, so a scan already in flight re-caches the stale listing.
pub fn invalidate_session_cache(cache: &std::sync::Mutex<Option<SessionCache>>) {
    let mut cache = cache.lock_or_recover();
    CACHE_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    *cache = None;
}

/// The requested page from the in-memory session cache, if populated.
fn cached_page(
    cache: &std::sync::Mutex<Option<SessionCache>>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Option<Vec<SessionSummary>> {
    let cache = cache.lock_or_recover();
    let cached = cache.as_ref()?;
    let sessions = paginate(&cached.sessions, limit, offset);
    info!(
        "Found {} sessions (returning {} from cache, offset {})",
        cached.sessions.len(),
        sessions.len(),
        offset.unwrap_or(0)
    );
    Some(sessions)
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

/// A JSONL's `(mtime, len)`; `None` when the file is missing, which is
/// itself a fingerprint so a JSONL that appears later is re-extracted.
type JsonlFingerprint = Option<(Option<std::time::SystemTime>, u64)>;

fn jsonl_fingerprint(path: &std::path::Path) -> JsonlFingerprint {
    fs::metadata(path)
        .ok()
        .map(|m| (m.modified().ok(), m.len()))
}

/// Sessions whose title extraction came back "New Chat", with the JSONL
/// fingerprint it was computed from. Those never get a title-cache entry
/// (opened-but-unused sessions hold only the steering exchange), so
/// without this every scan reopens and re-parses their JSONL twice. Kept
/// in memory only, and apart from `session_cache` (which the watcher
/// wipes): an appended prompt changes the fingerprint, so a real title is
/// still picked up.
static UNTITLED_JSONL: std::sync::LazyLock<std::sync::Mutex<HashMap<String, JsonlFingerprint>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

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

    // Rebuilt every scan, so sessions that are gone or have since been
    // titled drop out and the map stays bounded.
    let prev_untitled = std::mem::take(&mut *UNTITLED_JSONL.lock_or_recover());
    let mut untitled: HashMap<String, JsonlFingerprint> = HashMap::new();

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
            let jsonl_fp = jsonl_fingerprint(&jsonl_path);
            if prev_untitled.get(&session_id) == Some(&jsonl_fp) {
                // Unchanged since it last extracted as "New Chat".
                untitled.insert(session_id.clone(), jsonl_fp);
                "New Chat".to_string()
            } else if let Some(recovered) = extract_ai_title_from_jsonl(&jsonl_path) {
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
                } else {
                    untitled.insert(session_id.clone(), jsonl_fp);
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

    *UNTITLED_JSONL.lock_or_recover() = untitled;

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
    invalidate_session_cache(&features.session_cache);

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{Duration, Instant};

    #[test]
    fn parse_jsonl_moves_payloads_and_keeps_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let lines = [
            r#"{"kind":"Prompt","data":{"message_id":"m1","content":[{"kind":"text","data":"hi"},{"data":{"x":1}}]}}"#,
            "not json",
            r#"{"data":{"content":"not an array"}}"#,
            "",
            r#"{"kind":"AssistantMessage","data":{"message_id":"m2","content":[{"kind":"text"}]}}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        let msgs = parse_jsonl(&path);
        assert_eq!(msgs.len(), 3);

        assert_eq!(msgs[0].kind, "Prompt");
        assert_eq!(msgs[0].message_id, "m1");
        assert_eq!(msgs[0].content.len(), 2);
        assert_eq!(msgs[0].content[0].kind, "text");
        assert_eq!(msgs[0].content[0].data, serde_json::json!("hi"));
        assert_eq!(msgs[0].content[1].kind, "unknown");
        assert_eq!(msgs[0].content[1].data, serde_json::json!({"x": 1}));

        assert_eq!(msgs[1].kind, "");
        assert_eq!(msgs[1].message_id, "");
        assert!(msgs[1].content.is_empty());

        assert_eq!(msgs[2].content[0].data, serde_json::Value::Null);
    }

    #[test]
    fn watcher_filter_skips_kage_dotfiles() {
        assert!(is_session_file_path(Path::new("/s/abc.json")));
        assert!(is_session_file_path(Path::new("/s/abc.jsonl")));
        assert!(!is_session_file_path(Path::new("/s/abc.lock")));
        assert!(!is_session_file_path(Path::new("/s/.title-cache.json")));
        assert!(!is_session_file_path(Path::new(
            "/s/.title-cache.json.tmp.123"
        )));
    }

    #[test]
    fn debounce_is_idle_until_an_event_is_noted() {
        let mut d = TrailingDebounce::default();
        let t0 = Instant::now();
        assert_eq!(d.remaining(t0), None);
        assert!(!d.take_due(t0));
    }

    #[test]
    fn debounce_fires_after_quiet_period_from_last_event() {
        let mut d = TrailingDebounce::default();
        let t0 = Instant::now();
        d.note(t0);
        d.note(t0 + Duration::from_millis(300));
        let mid = t0 + Duration::from_millis(500);
        assert!(!d.take_due(mid));
        assert_eq!(d.remaining(mid), Some(Duration::from_millis(200)));
        let after = t0 + Duration::from_millis(700);
        assert!(d.take_due(after));
        // Reset: idle again, nothing pending.
        assert_eq!(d.remaining(after), None);
    }

    #[test]
    fn debounce_caps_continuous_bursts_at_max_wait() {
        let mut d = TrailingDebounce::default();
        let t0 = Instant::now();
        let mut t = t0;
        while t < t0 + WATCH_MAX_WAIT {
            d.note(t);
            t += Duration::from_millis(100);
        }
        assert!(d.take_due(t0 + WATCH_MAX_WAIT));
    }

    #[test]
    fn jsonl_fingerprint_distinguishes_missing_and_appended() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        assert_eq!(jsonl_fingerprint(&path), None);
        fs::write(&path, "a").unwrap();
        let first = jsonl_fingerprint(&path);
        assert!(first.is_some());
        fs::write(&path, "ab").unwrap();
        assert_ne!(jsonl_fingerprint(&path), first);
    }
}
