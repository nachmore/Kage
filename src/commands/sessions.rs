use crate::error::AppError;
use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, FeatureServices};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tauri::{Manager, State};

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
    pub kind: String, // "Prompt", "AssistantMessage", "ToolResults"
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

/// Lock the config, resolve the sessions dir, drop the lock. The previous
/// pattern was `let config = features.config.lock_or_recover().clone();`
/// followed by `get_sessions_dir_from_config(&config)`, which deep-cloned
/// every nested HashMap (extension grants, extension states, tool
/// permissions list, …) just to read the active connection's directory.
/// Most session commands run on a hot path (load, list, switch, delete)
/// where that overhead adds up.
fn resolve_sessions_dir_locked(
    config: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
) -> Result<PathBuf, String> {
    let guard = config.lock_or_recover();
    get_sessions_dir_from_config(&guard)
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

/// Where a session's cached title came from. Affects whether the
/// background AI summarizer is allowed to overwrite it.
///
/// Title generation runs twice per session, then stops:
///
/// 1. **First user message** → `Extracted` (or absent) becomes
///    `AiPrelim`. The opening message is often a throwaway ("hello!",
///    "test") and the resulting title reflects that ("Quick hello
///    greeting"). We treat the `AiPrelim` title as provisional.
/// 2. **Second user message** → `AiPrelim` becomes `Ai`. By now the
///    user has typed something with real intent, and the regenerated
///    summary captures the actual conversation topic.
/// 3. **Subsequent messages** → `Ai` and `Manual` are both off-limits;
///    the conversation has settled and re-summarising on every turn
///    would just waste tokens (the user can always manually rename).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleSource {
    /// Pulled from the JSONL's first user prompt — the historical
    /// behaviour. Eligible for AI re-summarization.
    Extracted,
    /// Provisional AI title generated after the first user message.
    /// Will be regenerated once on the second user message.
    AiPrelim,
    /// Final AI title generated after the second user message. Locked
    /// — will not be overwritten by the summarizer; user can still
    /// override via rename.
    Ai,
    /// User-supplied via `rename_session`. Never overwritten.
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleEntry {
    pub title: String,
    pub source: TitleSource,
}

/// Wire shape for the on-disk title cache. Accepts both the legacy
/// `{ id: "title" }` shape (treated as `Manual` — pre-summarizer
/// caches were almost always either user renames or first-prompt
/// extracts the user accepted, and assuming `Manual` is the safe
/// default since it prevents the summarizer from clobbering them) and
/// the new `{ id: { title, source } }` shape.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TitleEntryWire {
    /// Legacy: bare string entry from before TitleSource existed.
    Legacy(String),
    /// New shape with source provenance.
    Tagged(TitleEntry),
}

fn load_title_cache() -> HashMap<String, TitleEntry> {
    let raw: HashMap<String, TitleEntryWire> = get_title_cache_path()
        .ok()
        .and_then(|p| fs::read_to_string(&p).ok())
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default();
    raw.into_iter()
        .map(|(k, v)| {
            let entry = match v {
                TitleEntryWire::Legacy(title) => TitleEntry {
                    title,
                    source: TitleSource::Manual,
                },
                TitleEntryWire::Tagged(e) => e,
            };
            (k, entry)
        })
        .collect()
}

fn save_title_cache(cache: &HashMap<String, TitleEntry>) {
    if let Ok(path) = get_title_cache_path() {
        if let Ok(content) = serde_json::to_string(cache) {
            let _ = fs::write(&path, content);
        }
    }
}

/// Strip internal Kage context tags from a user-message string. Mirrors
/// `ui/js/shared/tool-utils.js::stripKageTags`: removes
/// `<_kage_ctx ...>` self-closing tags (screen-context decorations) and
/// `[_KAGE_INLINE]`-style bracket markers (inline-assist instructions).
/// These are injected by the app for the agent's benefit and must never
/// surface in user-visible titles.
fn strip_kage_tags(text: &str) -> String {
    use std::sync::LazyLock;
    // <_kage_ctx app="..." title="..."/> and similar self-closing tags.
    static KAGE_XML: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"<_kage_[^>]*/>\n?").unwrap());
    // [_KAGE_INLINE] Return ONLY... (consumes through end of line)
    static KAGE_BRACKET: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\[_KAGE_[A-Z_]*\][^\n]*\n?").unwrap());

    let stripped = KAGE_XML.replace_all(text, "");
    let stripped = KAGE_BRACKET.replace_all(&stripped, "");
    stripped.trim().to_string()
}

/// Recover a previously-AI-summarised title from a session's JSONL.
/// Looks for a `[KAGE_STEERING_IGNORE] [KAGE_TITLE]` Prompt and the
/// next AssistantMessage; the cleaned reply (via
/// `session_titler::clean_title`) is the recovered title.
///
/// Used by `list_sessions` when the title cache has no entry for a
/// session. Three cases this handles:
///   1. **Migration** — first list_sessions after this PR ships against
///      a session that previously generated `[KAGE_TITLE]` exchanges
///      (e.g. via an in-tree dev build) gets the recovered title with
///      no extra prompt cost.
///   2. **Cache loss** — `.title-cache.json` was deleted/corrupted.
///   3. **Cross-machine** — JSONLs synced from another box without
///      the cache file. Recovers titles instead of regenerating.
///
/// Walks the whole file (capped at 200 lines for safety) since the
/// title prompt may not be the first one — earlier prompts could be
/// steering, timestamp injections, etc.
fn extract_ai_title_from_jsonl(jsonl_path: &std::path::Path) -> Option<String> {
    use std::io::{BufRead, BufReader};

    let file = fs::File::open(jsonl_path).ok()?;
    let reader = BufReader::new(file);

    let title_prompt_marker = "[KAGE_STEERING_IGNORE] [KAGE_TITLE]";
    let mut saw_title_prompt = false;

    for line in reader.lines().take(200) {
        let Ok(line) = line else { continue };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = val.get("kind").and_then(|k| k.as_str()).unwrap_or("");

        if !saw_title_prompt {
            if kind != "Prompt" {
                continue;
            }
            // Look for the title-prompt marker in any text content block.
            let matched = val
                .get("data")
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter().any(|item| {
                        item.get("kind").and_then(|k| k.as_str()) == Some("text")
                            && item
                                .get("data")
                                .and_then(|d| d.as_str())
                                .is_some_and(|s| s.starts_with(title_prompt_marker))
                    })
                })
                .unwrap_or(false);
            if matched {
                saw_title_prompt = true;
            }
            continue;
        }

        // We've seen the title prompt — the next AssistantMessage is
        // the reply we want.
        if kind != "AssistantMessage" {
            continue;
        }
        let arr = val
            .get("data")
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_array())?;
        let combined: String = arr
            .iter()
            .filter_map(|item| {
                if item.get("kind").and_then(|k| k.as_str()) == Some("text") {
                    item.get("data").and_then(|d| d.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");

        return crate::session_titler::clean_title(&combined);
    }
    None
}

/// Extract a title from the JSONL — use the first user prompt text.
/// Skips steering messages, timestamp injections, and pure-tag prompts
/// (e.g. inline-assist instruction-only messages).
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
                                if trimmed.starts_with(crate::commands::system::STEERING_MSG_PREFIX)
                                {
                                    continue;
                                }
                                // Skip timestamp injections — not meaningful titles
                                if trimmed.starts_with("[Current time:") {
                                    continue;
                                }
                                // Strip injected Kage tags before clipping.
                                // If the message was *only* tags (e.g.
                                // inline-assist instruction wrappers),
                                // the post-strip string will be empty —
                                // skip and try the next prompt.
                                let stripped = strip_kage_tags(trimmed);
                                if stripped.is_empty() {
                                    continue;
                                }
                                let title: String = stripped.chars().take(60).collect();
                                if title.chars().count() < stripped.chars().count() {
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
    use tauri::Emitter;

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
                    let _ = app.emit("sessions_changed", ());
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
                title_cache.insert(
                    session_id.clone(),
                    TitleEntry {
                        title: recovered.clone(),
                        source: TitleSource::Ai,
                    },
                );
                cache_dirty = true;
                recovered
            } else {
                let extracted = extract_title_from_jsonl(&jsonl_path);
                if extracted != "New Chat" {
                    title_cache.insert(
                        session_id.clone(),
                        TitleEntry {
                            title: extracted.clone(),
                            source: TitleSource::Extracted,
                        },
                    );
                    cache_dirty = true;
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

    // Persist cache if we added new entries
    if cache_dirty {
        save_title_cache(&title_cache);
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
pub async fn delete_session(
    session_id: String,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    use tauri::Emitter;
    let sessions_dir = resolve_sessions_dir_locked(&features.config)?;

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
                        let _ = fs::write(
                            &cache_path,
                            serde_json::to_string_pretty(&cache).unwrap_or_default(),
                        );
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

    // Tell every window: this session is gone. Windows pinned to it
    // clear their chat area and show a "no longer exists" notice;
    // others just refresh their sidebar list.
    let _ = app.emit(
        "session_changed",
        serde_json::json!({
            "id": session_id,
            "kind": "deleted",
        }),
    );

    info!("Deleted session: {}", session_id);
    Ok(())
}

/// Adopt or create a session for the calling window. If `session_id`
/// is provided, loads that session via session/load. If absent, creates
/// a new session via session/new. The returned id is what the frontend
/// will pass on subsequent send/cancel/slash invokes; the backend also
/// records it in `UiState.window_sessions[window_label]`.
#[tauri::command]
pub async fn switch_acp_session(
    session_id: Option<String>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    ui: State<'_, crate::state::UiState>,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<String, AppError> {
    let client_guard = acp.client.clone();
    let window_label = window.label().to_string();

    // Ensure connected
    if !client_guard.is_connected() {
        info!("Not connected, attempting to connect for session switch...");
        if let Err(e) = client_guard.connect() {
            error!("Connection failed: {}", e);
            return Err(AppError::connection_lost(format!(
                "Failed to connect: {}",
                e
            )));
        }
    }

    match session_id {
        Some(id) => {
            info!("Switching to existing session: {}", id);

            // Read the cwd from the session's .json metadata file
            let cwd = {
                let sessions_dir = resolve_sessions_dir_locked(&features.config)?;
                let json_path = sessions_dir.join(format!("{}.json", id));
                if json_path.exists() {
                    fs::read_to_string(&json_path)
                        .ok()
                        .and_then(|content| {
                            serde_json::from_str::<serde_json::Value>(&content).ok()
                        })
                        .and_then(|data| {
                            data.get("cwd")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        })
                } else {
                    None
                }
            };

            let (loaded_id, models_json) = client_guard
                .load_existing_session(&id, cwd)
                .map_err(|e| AppError::internal(format!("Failed to load session: {}", e)))?;
            crate::telemetry::track(
                &app,
                "session_resumed",
                Some(serde_json::json!({ "source": "manual" })),
            );
            // Refresh the model dropdown if the agent included
            // availableModels in the load response. Empty list is
            // tolerated — the dropdown just keeps whatever was there
            // (typically populated when the previous session was
            // created), so the user-visible behaviour is "no
            // regression" rather than "models reset to empty".
            if !models_json.is_empty() {
                if let Ok(parsed) = serde_json::from_value::<Vec<crate::state::AcpModel>>(
                    serde_json::Value::Array(models_json),
                ) {
                    if let Ok(mut m) = acp.available_models.lock() {
                        *m = parsed;
                    }
                }
            }
            if let Ok(mut ws) = ui.window_sessions.lock() {
                ws.insert(window_label.clone(), loaded_id.clone());
            }
            update_window_title(
                &app,
                &features.config,
                &features.session_cache,
                &window_label,
                &loaded_id,
            );
            Ok(loaded_id)
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

            crate::telemetry::track(
                &app,
                "session_created",
                Some(serde_json::json!({ "source": "manual" })),
            );

            // Store available models
            if let Ok(parsed) = serde_json::from_value::<Vec<crate::state::AcpModel>>(
                serde_json::Value::Array(models_json),
            ) {
                if let Ok(mut m) = acp.available_models.lock() {
                    *m = parsed;
                }
            }

            // Apply default model if configured. Snapshot the relevant
            // fields under one lock and drop before the agent calls and
            // the steering disk reads.
            let (default_model, steering_inputs) = {
                let cfg = features.config.lock_or_recover();
                (
                    cfg.acp.agent.default_model.clone(),
                    crate::commands::system::SteeringInputs::from_config(&cfg),
                )
            };
            if let Some(ref default_model) = default_model {
                if !default_model.is_empty() {
                    info!("Applying default model to new session: {}", default_model);
                    let result = client_guard.send_request(
                        &client_guard.vendor_method("commands/execute"),
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

            // Send steering documents to the new session.
            {
                let parts = crate::commands::system::assemble_steering_parts(&steering_inputs);
                let steering_msg = format!(
                    "{} {}",
                    crate::commands::system::STEERING_MSG_PREFIX,
                    parts.join("\n\n---\n\n")
                );
                let _ = client_guard.send_chat_streaming(&new_session_id, &steering_msg, None);
            }

            // Invalidate session list cache (new session was created)
            {
                let mut cache = features.session_cache.lock_or_recover();
                *cache = None;
            }

            if let Ok(mut ws) = ui.window_sessions.lock() {
                ws.insert(window_label.clone(), new_session_id.clone());
            }
            update_window_title(
                &app,
                &features.config,
                &features.session_cache,
                &window_label,
                &new_session_id,
            );

            Ok(new_session_id)
        }
    }
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
pub async fn set_window_session(
    label: String,
    session_id: String,
    ui: State<'_, crate::state::UiState>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
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

/// Resolve the session's display title and write it to the window's
/// title bar. Chat windows (`main`, `chat-<uuid>`) get
/// `"<title> - Kage"`; floating gets `"Kage — <title>"`. Operates on
/// the `FeatureServices` Arcs directly so callers in spawn-blocking
/// closures (which can't hold a State) can invoke it too.
pub fn update_window_title(
    app: &tauri::AppHandle,
    config_arc: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    session_cache_arc: &std::sync::Arc<std::sync::Mutex<Option<SessionCache>>>,
    label: &str,
    session_id: &str,
) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let title = lookup_session_title(config_arc, session_cache_arc, session_id)
        .unwrap_or_else(|| "New Chat".to_string());
    let display_title = if label == crate::window_labels::FLOATING {
        format!("Kage — {}", title)
    } else {
        format!("{} - Kage", title)
    };
    if let Err(e) = window.set_title(&display_title) {
        log::warn!("Failed to set title for window {}: {}", label, e);
    }
}

/// Look up a session's display title. Cache hit first; on miss falls
/// back to extracting from the JSONL on disk (AI summary first, then
/// first-prompt). Returns None when the session has no extractable
/// title (fresh, empty, or missing).
fn lookup_session_title(
    config_arc: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    session_cache_arc: &std::sync::Arc<std::sync::Mutex<Option<SessionCache>>>,
    session_id: &str,
) -> Option<String> {
    if let Ok(cache) = session_cache_arc.lock() {
        if let Some(ref c) = *cache {
            if let Some(s) = c.sessions.iter().find(|s| s.session_id == session_id) {
                if !s.title.is_empty() && s.title != "New Chat" {
                    return Some(s.title.clone());
                }
            }
        }
    }

    // Cache miss or default title — extract directly from the file.
    let sessions_dir = resolve_sessions_dir_locked(config_arc).ok()?;
    let jsonl_path = sessions_dir.join(format!("{}.jsonl", session_id));
    if !jsonl_path.exists() {
        return None;
    }
    if let Some(ai_title) = extract_ai_title_from_jsonl(&jsonl_path) {
        return Some(ai_title);
    }
    let title = extract_title_from_jsonl(&jsonl_path);
    if title == "New Chat" {
        None
    } else {
        Some(title)
    }
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

/// Rename a session by updating its title in the cache
#[tauri::command]
pub async fn rename_session(
    session_id: String,
    title: String,
    features: State<'_, FeatureServices>,
    ui: State<'_, crate::state::UiState>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    use tauri::Emitter;
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("Title cannot be empty".to_string().into());
    }

    info!("Renaming session {} to: {}", session_id, title);

    // User-driven rename — flagged Manual so the AI summarizer in
    // session_titler never overwrites it.
    let mut title_cache = load_title_cache();
    title_cache.insert(
        session_id.clone(),
        TitleEntry {
            title: title.clone(),
            source: TitleSource::Manual,
        },
    );
    save_title_cache(&title_cache);

    // Invalidate session list cache
    {
        let mut session_cache = features.session_cache.lock_or_recover();
        *session_cache = None;
    }

    // Refresh window titles for any window pinned to this session.
    let labels: Vec<String> = ui
        .window_sessions
        .lock()
        .ok()
        .map(|m| {
            m.iter()
                .filter(|(_, sid)| **sid == session_id)
                .map(|(label, _)| label.clone())
                .collect()
        })
        .unwrap_or_default();
    for label in &labels {
        update_window_title(
            &app,
            &features.config,
            &features.session_cache,
            label,
            &session_id,
        );
    }

    // Broadcast so all windows can refresh their session list / chat
    // header. The frontend filters by sessionId — windows not showing
    // this session ignore the event but cheaply re-render their
    // sidebar so the renamed entry shows the new title.
    let _ = app.emit(
        "session_changed",
        serde_json::json!({
            "id": session_id,
            "kind": "renamed",
            "title": title,
        }),
    );

    Ok(())
}

/// Background AI-summariser for session titles. Called from the
/// `send_message_streaming` epilogue after the user's prompt
/// completes successfully. Two-stage:
///
/// - First call (Extracted/absent → AiPrelim): generates a
///   provisional title from whatever the conversation has so far,
///   typically just one user message. Often a throwaway like
///   "Quick hello greeting" if the user opened with "hello!".
/// - Second call (AiPrelim → Ai): regenerates after the user's
///   second message has landed. By now the actual intent is in
///   the conversation, so the resulting title is keepable.
/// - Subsequent calls (Ai/Manual): no-op. We're done.
///
/// On any title-generation failure we leave the cache in its
/// current state. If we were AiPrelim and the agent refuses on the
/// second call, the title stays prelim — we don't keep retrying
/// forever; a third user message will trip the Ai/Manual no-op.
/// The exception is handled below: an explicit upgrade-from-prelim
/// path stamps Ai when the second call returns `None`, so we don't
/// loop trying.
///
/// Spawns its own background task — caller doesn't await it.
pub fn maybe_generate_ai_title(
    app: tauri::AppHandle,
    client: std::sync::Arc<crate::acp_client::AcpClient>,
    session_cache: std::sync::Arc<std::sync::Mutex<Option<SessionCache>>>,
    session_id: String,
) {
    // Single disk read up front: snapshot the cache once and decide
    // both the gate (skip / first-pass / prelim-upgrade) and what to
    // write at the end. The previous pass loaded the title cache up to
    // three times per call (gate check + lock-prelim branch + final
    // persist), each one a fresh `read_to_string + serde_json::from_str`
    // round-trip on `~/.kiro/sessions/.title-cache.json`.
    let mut cache = load_title_cache();
    let is_prelim_upgrade = match cache.get(&session_id).map(|e| e.source) {
        Some(TitleSource::Manual) | Some(TitleSource::Ai) => return,
        Some(TitleSource::AiPrelim) => true,
        Some(TitleSource::Extracted) | None => false,
    };

    tauri::async_runtime::spawn_blocking(move || {
        if !client.is_connected() {
            return;
        }
        let title_opt = match crate::session_titler::generate_title(&client, &session_id) {
            Ok(t) => t,
            Err(e) => {
                warn!(
                    "AI title generation errored for {}: {}",
                    &session_id[..session_id.len().min(12)],
                    e
                );
                return;
            }
        };

        // Decide what to write.
        let (title_to_write, new_source) = match (title_opt, is_prelim_upgrade) {
            (Some(t), false) => (t, TitleSource::AiPrelim),
            (Some(t), true) => (t, TitleSource::Ai),
            (None, true) => {
                // Second-stage refusal/empty. Lock the prelim by
                // promoting it to Ai so we stop trying — the prelim
                // title is already good enough by definition (it
                // came from the first call's success).
                info!(
                    "AI title regeneration produced no usable title for {} — locking prelim as final",
                    &session_id[..session_id.len().min(12)]
                );
                if let Some(entry) = cache.get_mut(&session_id) {
                    entry.source = TitleSource::Ai;
                    save_title_cache(&cache);
                }
                return;
            }
            (None, false) => {
                // First-stage refusal/empty. Leave the cache alone —
                // next message will retry from scratch.
                info!(
                    "AI title generation produced no usable title for {} — leaving cache",
                    &session_id[..session_id.len().min(12)]
                );
                return;
            }
        };

        // Persist + broadcast.
        cache.insert(
            session_id.clone(),
            TitleEntry {
                title: title_to_write.clone(),
                source: new_source,
            },
        );
        save_title_cache(&cache);

        // Invalidate the in-memory session list cache so the next
        // list_sessions reads the new title.
        if let Ok(mut sc) = session_cache.lock() {
            *sc = None;
        }

        // Emit session_changed so existing PR 3 listeners update
        // window titles, sidebars, and chat headers without us
        // having to know about each window here.
        use tauri::Emitter;
        let _ = app.emit(
            "session_changed",
            serde_json::json!({
                "id": session_id,
                "kind": "renamed",
                "title": title_to_write,
                "source": "ai",
            }),
        );
        info!(
            "AI title set for {} (source={:?}): {}",
            &session_id[..session_id.len().min(12)],
            new_source,
            title_to_write
        );
    });
}
