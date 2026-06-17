//! Session title machinery: the on-disk title cache, JSONL title
//! extraction (AI-summary recovery + first-prompt fallback), window-title
//! updates, manual rename, and the background AI summariser.

use super::*;

pub(super) fn get_title_cache_path() -> Result<PathBuf, String> {
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

pub(super) fn load_title_cache() -> HashMap<String, TitleEntry> {
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

pub(super) fn save_title_cache(cache: &HashMap<String, TitleEntry>) {
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
pub(super) fn extract_ai_title_from_jsonl(jsonl_path: &std::path::Path) -> Option<String> {
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
pub(super) fn extract_title_from_jsonl(jsonl_path: &std::path::Path) -> String {
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

/// Rename a session by updating its title in the cache
#[tauri::command]
pub async fn rename_session(
    session_id: String,
    title: String,
    features: State<'_, FeatureServices>,
    ui: State<'_, crate::state::UiState>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
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

    // Tell chat-host windows so their session list / chat header
    // re-renders. Windows not showing this session ignore the event
    // but cheaply refresh their sidebar so the renamed entry shows
    // the new title.
    crate::event_targets::emit_to_chat_hosts(
        &app,
        "session_changed",
        &serde_json::json!({
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

        // Emit session_changed so existing chat-host listeners update
        // window titles, sidebars, and chat headers without us having
        // to know about each window here.
        crate::event_targets::emit_to_chat_hosts(
            &app,
            "session_changed",
            &serde_json::json!({
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
