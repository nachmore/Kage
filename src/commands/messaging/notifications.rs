//! ACP notification handler: streaming chunk fan-out + flush thread,
//! permission-request routing, and standard-ACP slash-command discovery.

use super::*;

mod slash_commands;
use slash_commands::parse_standard_acp_commands;

/// The handler dispatches all ACP notifications to the appropriate Tauri events.
pub fn setup_notification_handler(
    client: std::sync::Arc<crate::acp_client::AcpClient>,
    app: &tauri::AppHandle,
    state_config: std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    slash_commands: std::sync::Arc<std::sync::Mutex<Vec<crate::state::SlashCommand>>>,
    pending_permission: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, crate::state::PendingPermission>>,
    >,
) {
    let app_handle = app.clone();
    let config = state_config;
    let slash_cmds = slash_commands;
    let pending_perm = pending_permission;
    let compacting = client.compacting.clone();
    let client_for_handler = client.clone();

    // Map toolCallId → first tool name (e.g. "write") from the initial tool_call update.
    // The permission request arrives later with a descriptive title (e.g. "Creating hello.txt")
    // but we want to track policies by the actual tool name.
    // Also remembers each call's ACP tool kind ("execute" for shell
    // commands) so a permission request can be matched to a trusted kind.
    let tool_names: ToolNameMap =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    // Per-session pending deltas waiting to be flushed to the UI as
    // batched message_chunk events. The notification handler appends here;
    // a dedicated thread below drains the map every CHUNK_FLUSH_INTERVAL_MS
    // and emits one event per non-empty bucket. With token-level streaming
    // we used to fire one Tauri emit per token (hundreds-to-thousands per
    // response, each costing a JSON serialize + IPC bridge + frontend
    // handler invocation), and the WebView2 emitter has no backpressure —
    // bursts pile up in Tauri's internal queue. Coalescing into ~60 fps
    // batches drops the IPC roundtrip count by 1-2 orders of magnitude
    // without changing the on-screen feel of streaming. The batcher lives
    // on the client so send paths can flush a session's tail before its
    // terminal event (see `flush_session_chunks`).
    let pending_chunks = client.chunk_batcher.clone();
    spawn_chunk_flush_thread(app_handle.clone(), pending_chunks.clone());

    // Throttle config saves for last_seen updates — at most once per 60s
    let last_config_save: std::sync::Arc<std::sync::Mutex<std::time::Instant>> =
        std::sync::Arc::new(std::sync::Mutex::new(
            std::time::Instant::now() - std::time::Duration::from_secs(60),
        ));

    client.set_notification_handler(move |notification: serde_json::Value| {
        let method = notification.get("method").and_then(|m| m.as_str()).unwrap_or("");

        if method == "session/request_permission" {
            handle_permission_notification(
                &notification, &app_handle, &config, &client_for_handler, &pending_perm, &tool_names, &last_config_save,
            );
            return;
        }

        // Synthetic, internally-dispatched (not off the wire): the recovery
        // ladder swapped session ids mid-turn. Fan out to every streaming-aware
        // window so any pinned to the old id adopts the new one and keeps
        // waiting — the recovered response streams under the new id moments
        // later. See AcpClient::notify_session_migrated.
        if method == "_kage/session_migrated" {
            if let Some(params) = notification.get("params") {
                // Deliver the failed attempt's tail first: once windows
                // re-pin, a late old-id chunk would re-badge the dead
                // session as streaming with nothing left to complete it.
                for key in ["oldSessionId", "newSessionId"] {
                    if let Some(sid) = params.get(key).and_then(|v| v.as_str()) {
                        flush_session_chunks(&app_handle, &client_for_handler, sid);
                    }
                }
                crate::event_targets::emit_streaming_audience(
                    &app_handle,
                    "session_migrated",
                    params,
                );
            }
            return;
        }

        // Synthetic: the reader thread saw the agent's stream close (EOF or
        // error). Broadcast so every window can drop its "connected" state
        // immediately rather than looking healthy until the next send fails.
        if method == "_kage/agent_disconnected" {
            let _ = app_handle.emit(events::AGENT_DISCONNECTED, ());
            // A background agent call in flight ends `agent_disconnected`.
            crate::ephemeral_session::notify_disconnected();
            return;
        }

        if method == "session/update" {
            // Every session/update carries the session id it belongs to.
            // We forward chunks and tool_call events to *all* windows
            // tagged with the session id; each window's frontend filters
            // by `acceptSessionId`, so a window only renders updates for
            // its own pinned session. Backgrounded sessions still
            // accumulate their bytes into the per-session bucket (used
            // by auto_steering and sub-agent reads) but their windows
            // see no chunks because no window has them pinned.
            let update_session_id = notification
                .get("params")
                .and_then(|p| p.get("sessionId"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());

            // Drop conversation-history replay. A `session/load` makes
            // kiro-cli re-emit every prior turn as a burst of session/update
            // notifications before the load response returns; those are
            // history, not live output. Without this gate they dump the old
            // conversation into the floating window and poison the streaming
            // accumulators (auto_steering / sub-agent reads). The gate is
            // per session id — set for the duration of the load request in
            // `load_existing_session` — so an overlapping load of one
            // session neither swallows live chunks streaming on another nor
            // unmasks a peer load's still-running replay when it finishes
            // first.
            if update_session_id
                .as_deref()
                .is_some_and(|sid| client_for_handler.is_loading_session(sid))
            {
                return;
            }

            if let Some(update) = notification.get("params").and_then(|p| p.get("update")) {
                if let Some(kind) = update.get("sessionUpdate").and_then(|v| v.as_str()) {
                    if kind == "agent_message_chunk" {
                        if let Some(text) = update.get("content").and_then(|c| c.get("text")).and_then(|t| t.as_str()) {
                            let Some(sid) = update_session_id else {
                                // No session id on the chunk — can't route. Drop.
                                return;
                            };
                            let emitted_owned: Option<String> = client_for_handler
                                .accumulate_chunk(&sid, text)
                                .map(|s| s.to_string());

                            // Background prompts (auto-steering, titler) are
                            // read back from the accumulator, never shown.
                            // Neither is a strict session's reply, even a
                            // late chunk after its prompt returned.
                            let background = client_for_handler.is_background_prompt_session(&sid)
                                || crate::ephemeral_session::is_strict(&sid);
                            if let Some(emitted) = emitted_owned {
                                if !emitted.is_empty() && !background {
                                    // Append to the per-session pending
                                    // buffer. The flush thread emits
                                    // `message_chunk` events with this
                                    // text every CHUNK_FLUSH_INTERVAL_MS,
                                    // one per non-empty session bucket.
                                    // Frontend filters by sessionId.
                                    pending_chunks.push(sid, &emitted);
                                }
                            }
                        }
                        return;
                    }
                    if kind == "available_commands_update" {
                        // Standard-ACP command discovery (Claude Code, and any
                        // non-vendor agent). Distinct from Kiro's
                        // `_kiro.dev/commands/available` vendor notification
                        // handled below — same destination (the shared slash
                        // command list), different wire shape. Claude sends
                        // `availableCommands: [{name, description, input}]` and
                        // expects the command to run as a normal prompt, so we
                        // tag each with dispatch="prompt".
                        if let Some(cmds) = update
                            .get("availableCommands")
                            .and_then(|c| c.as_array())
                        {
                            let parsed = parse_standard_acp_commands(cmds);
                            // Augment with the active agent's curated built-in
                            // catalog — some adapters (Claude) advertise far
                            // fewer commands than the CLI supports. Advertised
                            // entries win on name collision; built-ins fill
                            // gaps. See crate::agent_commands.
                            let agent_kind = config
                                .lock()
                                .ok()
                                .map(|c| crate::agent_presets::detect(&c))
                                .unwrap_or(crate::agent_presets::AgentKind::Kiro);
                            let builtin =
                                crate::agent_commands::builtin_commands(agent_kind);
                            let merged =
                                crate::agent_commands::merge_commands(parsed, builtin);
                            info!(
                                "Received {} standard-ACP slash commands ({} after built-in merge)",
                                cmds.len(),
                                merged.len()
                            );
                            if let Ok(mut slot) = slash_cmds.lock() {
                                *slot = merged;
                            }
                            crate::event_targets::emit_to_floating(
                                &app_handle,
                                "slash_commands_available",
                                &notification,
                            );
                        }
                        return;
                    }
                    if kind == "tool_call" || kind == "tool_call_update" {
                        // Kage's own `run_outside_sandbox` calls: remember
                        // their commands, so a request arriving over the host
                        // bridge can be matched to this session (a strict
                        // session's too, so the bridge can refuse it).
                        {
                            let first = update
                                .get("toolCallId")
                                .and_then(|v| v.as_str())
                                .and_then(|id| {
                                    tool_names.lock().ok().and_then(|n| n.get(id).cloned())
                                });
                            crate::host_commands::observe_tool_call(
                                update_session_id.as_deref(),
                                update,
                                first.as_ref().map(|(title, _)| title.as_str()),
                                first.as_ref().and_then(|(_, kind)| kind.as_deref()),
                            );
                        }
                        // No tool may run on a strict session (a background
                        // agent call). An agent set to run tools without
                        // asking only shows it here: stop the turn, tell the
                        // call's owner, and show nothing.
                        if let Some(sid) = update_session_id
                            .as_deref()
                            .filter(|sid| crate::ephemeral_session::is_strict(sid))
                        {
                            crate::ephemeral_session::note_tool_attempt(&client_for_handler, sid);
                            return;
                        }
                        // A kind can arrive on an update that carries no
                        // title; attach it to the call's existing entry.
                        if let (Some(call_id), None, Some(k)) = (
                            update.get("toolCallId").and_then(|v| v.as_str()),
                            update.get("title").and_then(|v| v.as_str()),
                            update
                                .get("kind")
                                .and_then(|v| v.as_str())
                                .filter(|k| !k.is_empty()),
                        ) {
                            if let Ok(mut names) = tool_names.lock() {
                                if let Some(entry) = names.get_mut(call_id) {
                                    if entry.1.is_none() {
                                        entry.1 = Some(k.to_string());
                                    }
                                }
                            }
                        }
                        // Track the first title for each toolCallId — that's the real tool name
                        if let (Some(call_id), Some(title)) = (
                            update.get("toolCallId").and_then(|v| v.as_str()),
                            update.get("title").and_then(|v| v.as_str()),
                        ) {
                            let update_kind = update
                                .get("kind")
                                .and_then(|v| v.as_str())
                                .filter(|k| !k.is_empty())
                                .map(str::to_string);
                            if let Ok(mut names) = tool_names.lock() {
                                let was_new = !names.contains_key(call_id);
                                let entry = names
                                    .entry(call_id.to_string())
                                    .or_insert_with(|| (title.to_string(), None));
                                // Title is fixed by the first update; the kind
                                // may only arrive on a later one.
                                if entry.1.is_none() {
                                    entry.1 = update_kind;
                                }
                                // Cap memory: tool call IDs are UUID-ish and we
                                // never prune on session-end. A long-running
                                // session can leak megabytes here. Keep the
                                // 4096 most recent — well above the working set
                                // for any realistic conversation.
                                const MAX_TOOL_NAMES: usize = 4096;
                                if was_new && names.len() > MAX_TOOL_NAMES {
                                    // HashMap iteration order is not insertion
                                    // order, so this is "drop arbitrary 25%"
                                    // rather than strict LRU. A dropped entry
                                    // makes a later permission request fall
                                    // back to its descriptive title, which
                                    // misses the user's per-tool policy — so
                                    // never drop the call just inserted, whose
                                    // permission request is the likeliest next.
                                    let drop_n = names.len() - MAX_TOOL_NAMES * 3 / 4;
                                    let to_drop: Vec<String> = names
                                        .keys()
                                        .filter(|k| k.as_str() != call_id)
                                        .take(drop_n)
                                        .cloned()
                                        .collect();
                                    for k in to_drop {
                                        names.remove(&k);
                                    }
                                }
                            }
                        }
                        // Forward to streaming-aware windows; frontend
                        // filters by sessionId in the payload. Skipped for
                        // background prompts so their tool use stays invisible.
                        let background = update_session_id
                            .as_deref()
                            .is_some_and(|sid| client_for_handler.is_background_prompt_session(sid));
                        if !background {
                            crate::event_targets::emit_streaming_audience(
                                &app_handle,
                                events::TOOL_CALL_UPDATE,
                                &notification,
                            );
                        }
                        return;
                    }
                }
            }
            return;
        }

        // Vendor extension dispatch. Two ACP vendor namespaces are
        // recognised — `_kage.dev/*` and `_kiro.dev/*` — with an
        // identical extension surface. Match by suffix and pin the
        // agent's preferred prefix on the AcpClient (used by outgoing
        // requests). See `acp_client::vendor_method_suffix`.
        if let Some(suffix) = crate::acp_client::vendor_method_suffix(method) {
            client_for_handler.observe_vendor_prefix(method);
            match suffix {
                "commands/available" => {
                    if let Some(commands) = notification.get("params")
                        .and_then(|p| p.get("commands"))
                        .and_then(|c| c.as_array())
                    {
                        if let Ok(parsed) = serde_json::from_value::<Vec<crate::state::SlashCommand>>(
                            serde_json::Value::Array(commands.clone()),
                        ) {
                            info!("Received {} slash commands from ACP", parsed.len());
                            if let Ok(mut cmds) = slash_cmds.lock() {
                                *cmds = parsed;
                            }
                        }
                    }
                    crate::event_targets::emit_to_floating(
                        &app_handle,
                        "slash_commands_available",
                        &notification,
                    );
                }
                "metadata" => {
                    crate::event_targets::emit_to_chat_hosts(
                        &app_handle,
                        "context_metadata",
                        &notification,
                    );
                }
                "compaction/status" => {
                    if let Some(status) = notification.get("params")
                        .and_then(|p| p.get("status"))
                        .and_then(|s| s.get("type"))
                        .and_then(|t| t.as_str())
                    {
                        let (lock, cvar) = &*compacting;
                        let mut is_compacting = lock.lock_or_recover();
                        match status {
                            "started" => {
                                info!("Compaction started — gating outgoing prompts");
                                *is_compacting = true;
                            }
                            "completed" => {
                                info!("Compaction completed — releasing prompt gate");
                                *is_compacting = false;
                                cvar.notify_all();
                            }
                            // A compaction that ends without "completed"
                            // would otherwise hold every prompt for the full
                            // 60s gate timeout. Only explicit terminal words
                            // release: an unknown progress-style status must
                            // not open the gate mid-compaction.
                            "failed" | "error" | "cancelled" | "canceled" | "aborted" => {
                                warn!("Compaction ended with status {:?} — releasing prompt gate", status);
                                *is_compacting = false;
                                cvar.notify_all();
                            }
                            _ => {}
                        }
                    }
                    crate::event_targets::emit_streaming_audience(
                        &app_handle,
                        events::COMPACTION_STATUS,
                        &notification,
                    );
                }
                "error/rate_limit" => {
                    let params = notification.get("params");
                    let message = params
                        .and_then(|p| p.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("Rate limit exceeded. Please wait a moment before trying again.");
                    let session_id = params
                        .and_then(|p| p.get("sessionId"))
                        .and_then(|s| s.as_str());
                    warn!("Rate limit hit (session {:?}): {}", session_id, message);
                    // MESSAGE_ERROR carries no session id, so every listener
                    // treats it as terminal for its own turn. Route it to the
                    // window that owns the rate-limited prompt rather than
                    // tearing down unrelated in-flight answers.
                    let background = session_id
                        .is_some_and(|sid| client_for_handler.is_background_prompt_session(sid));
                    let ui_state = app_handle.try_state::<UiState>();
                    let originator = session_id.and_then(|sid| {
                        ui_state
                            .as_ref()?
                            .pending_prompt_originators
                            .lock()
                            .ok()?
                            .get(sid)
                            .cloned()
                    });
                    // Fallback when the originator tag is gone (a recovery
                    // resend under a migrated id, or a peer's epilogue
                    // clearing a shared session's tag): the windows pinned
                    // to the session are the ones waiting on it.
                    let pinned: Vec<String> = match (session_id, ui_state.as_ref()) {
                        (Some(sid), Some(ui)) => ui
                            .window_sessions
                            .lock()
                            .map(|m| {
                                m.iter()
                                    .filter(|(_, s)| s.as_str() == sid)
                                    .map(|(label, _)| label.clone())
                                    .collect()
                            })
                            .unwrap_or_default(),
                        _ => Vec::new(),
                    };
                    match rate_limit_route(session_id.is_some(), background, originator, pinned) {
                        RateLimitRoute::LogOnly => {}
                        RateLimitRoute::Windows(labels) => {
                            for label in labels {
                                let _ = app_handle.emit_to(
                                    tauri::EventTarget::webview_window(label.as_str()),
                                    events::MESSAGE_ERROR,
                                    message,
                                );
                            }
                        }
                        RateLimitRoute::Broadcast => {
                            crate::event_targets::emit_streaming_audience(
                                &app_handle,
                                events::MESSAGE_ERROR,
                                &message,
                            );
                        }
                    }
                }
                _ => {
                    // Unknown vendor extension — forward to streaming-aware
                    // windows as a generic tool_call_update, mirroring previous
                    // behaviour.
                    crate::event_targets::emit_streaming_audience(
                        &app_handle,
                        events::TOOL_CALL_UPDATE,
                        &notification,
                    );
                }
            }
            return;
        }

        info!("Unhandled notification: {}", method);
    });
}

/// How often the chunk-flush thread wakes up to drain the pending-chunks
/// map and emit batched `message_chunk` events. ~60 fps — slower than
/// human chunk-perception, fast enough that streaming text doesn't feel
/// stuttery. Anything below 10ms invites IPC backpressure with no
/// user-visible benefit; above ~33ms (30 fps) the streaming starts to
/// feel laggy.
const CHUNK_FLUSH_INTERVAL_MS: u64 = 16;

/// How many consecutive failing flush cycles we tolerate before treating
/// the failure as app shutdown and exiting the thread. `emit_filter` can
/// `Err` transiently (a `chat-<uuid>` webview torn down mid-dispatch);
/// exiting on the first error permanently killed streaming for the rest
/// of the process lifetime while `pending_chunks` grew unbounded. At the
/// 16ms cadence 64 cycles ≈ 1s of solid failure — real shutdown never
/// recovers, a torn-down webview clears in a cycle or two.
const CHUNK_FLUSH_MAX_CONSECUTIVE_FAILURES: u32 = 64;

/// Background thread that drains `pending_chunks` every
/// CHUNK_FLUSH_INTERVAL_MS and emits one `message_chunk` event per non-
/// empty session bucket. Replaces the pre-fix one-emit-per-token path,
/// which fired hundreds-to-thousands of IPC roundtrips per response.
///
/// The thread runs for the AcpClient's lifetime — it's a single OS thread
/// (`acp-chunk-flush`) doing a HashMap drain + 0..N emits per cycle, so
/// the always-on cost is negligible. Exit is by `app_handle.emit` returning
/// errors for CHUNK_FLUSH_MAX_CONSECUTIVE_FAILURES consecutive cycles
/// (app shutdown); isolated failures are retried — the batcher re-queues
/// undelivered text, bounded by `chunk_batcher::MAX_PENDING_BYTES`.
fn spawn_chunk_flush_thread(
    app_handle: tauri::AppHandle,
    pending: std::sync::Arc<crate::chunk_batcher::ChunkBatcher>,
) {
    let _ = std::thread::Builder::new()
        .name("acp-chunk-flush".into())
        .spawn(move || {
            let interval = std::time::Duration::from_millis(CHUNK_FLUSH_INTERVAL_MS);
            let mut consecutive_failures: u32 = 0;
            loop {
                std::thread::sleep(interval);
                let alive = pending.drain_and_emit(|session_id, text| {
                    emit_chunk_batch(&app_handle, session_id, text)
                });
                if alive {
                    consecutive_failures = 0;
                    continue;
                }
                consecutive_failures += 1;
                if consecutive_failures == 1 {
                    // English-only log (see I18N contract).
                    log::warn!("chunk-flush emit failed; retrying (transient webview teardown?)");
                }
                if consecutive_failures >= CHUNK_FLUSH_MAX_CONSECUTIVE_FAILURES {
                    log::warn!(
                        "chunk-flush emit failed {} consecutive cycles — assuming shutdown, exiting",
                        consecutive_failures
                    );
                    return;
                }
            }
        });
}

/// Emit one batched `message_chunk` for `session_id`.
///
/// Streaming-audience target — chat hosts + floating + settings — so the
/// per-frame chunk doesn't fan out to every webview that happens to
/// subscribe to anything else. Calls `emit_filter` directly (rather than
/// the helper) because the chunk-flush thread relies on the emit's Err to
/// detect shutdown; the helper swallows errors at debug-log level.
fn emit_chunk_batch<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    session_id: &str,
    text: &str,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "text": text,
        "sessionId": session_id,
    });
    app_handle
        .emit_filter(events::MESSAGE_CHUNK, &payload, |t| match t {
            tauri::EventTarget::Window { label }
            | tauri::EventTarget::Webview { label }
            | tauri::EventTarget::WebviewWindow { label }
            | tauri::EventTarget::AnyLabel { label } => {
                window_labels::is_session_host_label(label)
                    || label == window_labels::FLOATING
                    || label == window_labels::SETTINGS
            }
            _ => false,
        })
        .map_err(|e| format!("{}", e))
}

/// Emit `session_id`'s still-batched chunks now. Call before any terminal
/// event for a turn (`message_complete`, `message_error`, `session_reset`,
/// `session_migrated`): the reader thread queues every chunk before the
/// prompt response wakes the sender, but the flush thread may be mid-sleep,
/// and a chunk landing after the terminal event is dropped by the window
/// (or re-badges the session as streaming in its peers).
pub(super) fn flush_session_chunks<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    client: &crate::acp_client::AcpClient,
    session_id: &str,
) {
    let delivered = client.chunk_batcher.flush_session(session_id, |sid, text| {
        emit_chunk_batch(app_handle, sid, text)
    });
    if !delivered {
        log::debug!("flush_session_chunks({session_id}) emit failed; left for flush thread");
    }
}

/// Where an `error/rate_limit` notification's MESSAGE_ERROR goes.
#[derive(Debug, PartialEq, Eq)]
enum RateLimitRoute {
    /// Background prompt (titler, auto-steering) or a prompt with no
    /// owning or pinned window: nobody is waiting on it, so don't end
    /// anyone's turn.
    LogOnly,
    /// The window that issued the prompt or, when its originator tag is
    /// gone, the windows pinned to the session.
    Windows(Vec<String>),
    /// Agent sent no session id — can't tell whose turn it is.
    Broadcast,
}

fn rate_limit_route(
    has_session_id: bool,
    background: bool,
    originator: Option<String>,
    pinned: Vec<String>,
) -> RateLimitRoute {
    if !has_session_id {
        return RateLimitRoute::Broadcast;
    }
    if background {
        return RateLimitRoute::LogOnly;
    }
    match originator {
        Some(label) => RateLimitRoute::Windows(vec![label]),
        None if !pinned.is_empty() => RateLimitRoute::Windows(pinned),
        None => RateLimitRoute::LogOnly,
    }
}

/// toolCallId → (first title, ACP tool kind if the agent sent one).
type ToolNameMap =
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, (String, Option<String>)>>>;

/// The ACP tool kind of a permission request: the request's own
/// `toolCall.kind`, else the kind recorded from that call's earlier
/// `tool_call` / `tool_call_update`. Only an explicit kind counts — ACP
/// defines a missing kind as "other", and guessing from the arguments is
/// unsafe: kiro-cli's own file-write tool sends `{"command": "create"}`,
/// so a `rawInput.command` heuristic would have let command trust approve
/// file writes.
fn classify_tool_kind(
    tool_call: Option<&serde_json::Value>,
    recorded: Option<&str>,
) -> Option<String> {
    tool_call
        .and_then(|tc| tc.get("kind"))
        .and_then(|k| k.as_str())
        .filter(|k| !k.is_empty())
        .or(recorded.filter(|k| !k.is_empty()))
        .map(str::to_string)
}

/// The MCP server a permission request's tool belongs to, or `None` unless
/// it can be identified unambiguously.
///
/// ACP's `ToolCall` has no MCP server field and no agent we've seen sends a
/// dedicated one, so the server comes from the title, in the forms kiro-cli
/// and Claude Code use (`config::mcp_server_from_title`). Two extra
/// conditions:
///
/// - The kind must be absent or `"other"` — what agents report for MCP
///   tools. Any other kind is a built-in tool; above all `"execute"`,
///   whose kiro-cli title is the command itself (`Running: <command>`), so
///   a shell command spelled `@builder-mcp/InternalSearch` (or one whose
///   title is cut at a newline) must never read as that server's tool.
///   Claude Code's adapter also reports its built-in Edit/Bash/Read with
///   their own kinds.
/// - Every title we have must name the same server: the policy title
///   (`tool_title`, from the call's first `tool_call`) and the request's
///   own `toolCall.title`, when it differs.
fn classify_mcp_server(
    tool_title: &str,
    request_title: Option<&str>,
    kind: Option<&str>,
) -> Option<String> {
    if !matches!(kind, None | Some("other")) {
        return None;
    }
    let server = crate::config::mcp_server_from_title(tool_title)?;
    if let Some(title) = request_title.filter(|t| !t.is_empty() && *t != tool_title) {
        if crate::config::mcp_server_from_title(title) != Some(server) {
            return None;
        }
    }
    Some(server.to_string())
}

/// Whether a permission request is for Kage's own `run_outside_sandbox`
/// tool: an MCP call (kind absent or `other`) of the server Kage registers
/// (`mcp_registration::MCP_SERVER_KEY`), and every title we have names
/// exactly that tool.
fn is_host_command_call(tool_title: &str, request_title: Option<&str>, kind: Option<&str>) -> bool {
    classify_mcp_server(tool_title, request_title, kind).as_deref()
        == Some(crate::mcp_registration::MCP_SERVER_KEY)
        && crate::host_commands::is_host_command_title(tool_title)
        && request_title
            .filter(|t| !t.is_empty() && *t != tool_title)
            .is_none_or(crate::host_commands::is_host_command_title)
}

/// The audit log's `args_preview` for an auto-approved request: the command
/// line of a shell run, the arguments of an MCP tool call (at most 2048
/// characters), nothing for other tools.
fn auto_allow_args_preview(
    raw_input: Option<&serde_json::Value>,
    request: &crate::config::ToolRequest,
) -> Option<String> {
    let clip = |s: &str| s.chars().take(2048).collect::<String>();
    if request.kind.as_deref() == Some(crate::config::TOOL_KIND_EXECUTE) {
        raw_input
            .and_then(|ri| ri.get("command"))
            .and_then(|c| c.as_str())
            .map(clip)
    } else if request.mcp_server.is_some() {
        raw_input
            .filter(|ri| !ri.is_null())
            .map(|ri| clip(&ri.to_string()))
    } else {
        None
    }
}

/// The `permission_request` event payload for a request that prompts.
fn permission_prompt_payload(
    notification: &serde_json::Value,
    request: &crate::config::ToolRequest,
    source: &str,
) -> serde_json::Value {
    serde_json::json!({
        "notification": notification,
        "auto_approve": false,
        "toolName": request.title,
        // Lets the prompt offer "Trust all commands" for shell runs.
        "toolKind": request.kind,
        // Lets the prompt offer "Trust all tools from <server>".
        "mcpServer": request.mcp_server,
        // Category trust never covers these sessions: the prompt offers no
        // category trust for them, and doesn't bulk-approve their queued
        // requests when the user trusts a category from another prompt.
        "backgroundSession": request.hidden_session,
        "source": source,
    })
}

/// How a permission request is decided.
#[derive(Debug, PartialEq, Eq)]
enum PermissionRoute {
    /// A strict session (a background agent call): refuse with this
    /// option, before any policy.
    Strict { option_id: String },
    /// Everything else: the user's tool policy decides.
    Policy(crate::config::RequestDecision),
}

/// Decide a permission request. A strict session is refused before the
/// policy is consulted, so explicit Allow rules, trusted categories and
/// terminator mode never approve a tool there, and the request adds no
/// `tool_permissions` row. Anything else goes to `observe_request`.
fn route_permission(
    perms: &mut crate::config::ToolPermissionsConfig,
    notification: &serde_json::Value,
    request: &crate::config::ToolRequest,
    strict: bool,
    timestamp: &str,
) -> PermissionRoute {
    if strict {
        return PermissionRoute::Strict {
            option_id: reject_once_option(notification),
        };
    }
    // An explicit per-tool Deny wins even under trust_all /
    // terminator_mode / a trusted category — see resolve_policy_for.
    PermissionRoute::Policy(perms.observe_request(request, timestamp))
}

/// The id of the request's `reject_once` option, as the agent named it
/// (ACP lets the agent choose option ids). `"reject_once"` when it offers
/// none. Never an allow option.
fn reject_once_option(notification: &serde_json::Value) -> String {
    notification
        .get("params")
        .and_then(|p| p.get("options"))
        .and_then(|o| o.as_array())
        .and_then(|options| {
            options.iter().find_map(|o| {
                (o.get("kind").and_then(|k| k.as_str()) == Some("reject_once"))
                    .then(|| o.get("optionId").and_then(|id| id.as_str()))
                    .flatten()
                    .filter(|id| !id.is_empty())
            })
        })
        .unwrap_or("reject_once")
        .to_string()
}

#[allow(clippy::too_many_arguments)]
fn handle_permission_notification(
    notification: &serde_json::Value,
    app_handle: &tauri::AppHandle,
    config: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    client: &crate::acp_client::AcpClient,
    pending_perm: &std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, crate::state::PendingPermission>>,
    >,
    tool_names: &ToolNameMap,
    last_config_save: &std::sync::Arc<std::sync::Mutex<std::time::Instant>>,
) {
    // The permission request has a descriptive title (e.g. "Creating hello.txt")
    // but we want the actual tool name (e.g. "write") from the first tool_call update.
    let tool_call_id = notification
        .get("params")
        .and_then(|p| p.get("toolCall"))
        .and_then(|tc| tc.get("toolCallId"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    let fallback_title = notification
        .get("params")
        .and_then(|p| p.get("toolCall"))
        .and_then(|tc| tc.get("title"))
        .and_then(|t| t.as_str())
        .unwrap_or("unknown");

    // Look up the real tool name (and kind) from the first tool_call update
    let recorded = if !tool_call_id.is_empty() {
        tool_names
            .lock()
            .ok()
            .and_then(|names| names.get(tool_call_id).cloned())
    } else {
        None
    };
    let tool_title = recorded
        .as_ref()
        .map(|(title, _)| title.clone())
        .unwrap_or_else(|| fallback_title.to_string());
    let tool_kind = classify_tool_kind(
        notification.get("params").and_then(|p| p.get("toolCall")),
        recorded.as_ref().and_then(|(_, kind)| kind.as_deref()),
    );
    let request_title = notification
        .get("params")
        .and_then(|p| p.get("toolCall"))
        .and_then(|tc| tc.get("title"))
        .and_then(|t| t.as_str());
    let mcp_server = classify_mcp_server(&tool_title, request_title, tool_kind.as_deref());

    let timestamp = chrono::Utc::now().to_rfc3339();

    // Category trust applies only to prompts the user can see. Background
    // prompts (inline assist, macros, script generation, titler, auto-
    // steering) run on text from other apps and their tool calls are never
    // shown — a prompt-injected selection must not get commands (or a
    // trusted MCP server's tools) run silently. `ToolRequest::hidden_session`
    // carries that into every policy decision.
    let request_session = notification
        .get("params")
        .and_then(|p| p.get("sessionId"))
        .and_then(|s| s.as_str());
    let hidden_session = request_session.is_some_and(|sid| {
        client.is_background_prompt_session(sid) || crate::ephemeral_session::is_active(sid)
    });
    let request = crate::config::ToolRequest {
        title: tool_title.clone(),
        kind: tool_kind.clone(),
        mcp_server: mcp_server.clone(),
        hidden_session,
    };
    let strict = request_session.is_some_and(crate::ephemeral_session::is_strict);

    // Kage's own `run_outside_sandbox` — exactly that server and tool — is
    // allowed here without a prompt: the real gate is at execution, where
    // Kage asks the user (or applies their rules) once the command reaches
    // it over the host bridge. Asking twice would only teach clicking
    // through. Never on a strict session (no tool runs there; the strict
    // route below refuses it), and an explicit per-tool Deny the user set
    // still wins.
    if !strict && is_host_command_call(&tool_title, request_title, tool_kind.as_deref()) {
        let denied = config
            .lock_or_recover()
            .tool_permissions
            .tools
            .iter()
            .any(|t| {
                t.title == tool_title && t.effective_policy() == crate::config::PolicyKind::Deny
            });
        if !denied {
            info!("Allowed {tool_title}: Kage asks the user when the command reaches it");
            if let Some(request_id) = notification.get("id") {
                if let Err(e) = client.send_permission_response(request_id, "allow_once") {
                    warn!("Failed to send auto permission response: {}", e);
                }
            }
            return;
        }
    }

    // Hold the lock just long enough to mutate the in-memory config and
    // decide whether a disk save is due. The actual save is dispatched
    // off-thread (see below) so the ACP reader thread that's invoking
    // this notification handler is never blocked on disk I/O — pre-fix
    // a slow disk could stall ACP message parsing for tens of ms,
    // long enough for in-flight prompts to time out.
    let (route, save_snapshot) = {
        let mut config_guard = config.lock_or_recover();
        let route = route_permission(
            &mut config_guard.tool_permissions,
            notification,
            &request,
            strict,
            &timestamp,
        );
        let row = match &route {
            PermissionRoute::Strict { .. } => crate::config::PolicyRowChange::Skipped,
            PermissionRoute::Policy(decision) => decision.row,
        };
        let needs_save = match row {
            crate::config::PolicyRowChange::Touched => {
                // last_seen changed in memory — throttle disk writes to at
                // most once per 60s.
                let mut last_save = last_config_save.lock_or_recover();
                let due = last_save.elapsed() >= std::time::Duration::from_secs(60);
                if due {
                    *last_save = std::time::Instant::now();
                }
                due
            }
            crate::config::PolicyRowChange::Added => {
                // New tool discovered — save immediately
                *last_config_save.lock_or_recover() = std::time::Instant::now();
                true
            }
            crate::config::PolicyRowChange::Skipped => false,
        };
        // Snapshot the config for an off-thread save if one is due.
        // The clone happens inside the lock so the snapshot is
        // consistent — but the Mutex is released as the guard drops
        // at end-of-block, BEFORE the save runs.
        let snap = needs_save.then(|| config_guard.clone());
        (route, snap)
    };

    // Disk save on a worker thread — never blocks the ACP reader.
    // `Config::save_to_atomic` does write+rename atomic replace, so
    // even concurrent saves leave the file in either old or new
    // state (never half-written). We don't await the result; a save
    // failure here is logged and the config will save again on the
    // next mutation that crosses the 60s throttle.
    if let Some(snap) = save_snapshot {
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(e) = snap.save() {
                warn!("Failed to save config (async): {}", e);
            }
        });
    }

    let send_response = |option_id: &str| {
        if let Some(request_id) = notification.get("id") {
            if let Err(e) = client.send_permission_response(request_id, option_id) {
                warn!("Failed to send auto permission response: {}", e);
            }
        }
    };

    // Auto-decisions (policy Allow/Deny, or a blanket-allow mode) bypass the
    // interactive send_permission_response command, which is where the audit
    // entry is normally written. Record them here too so the audit log
    // reflects EVERY tool decision, not just the ones that prompted — the log
    // is a security feature and a silent auto-approve is exactly what a user
    // reviewing it would want to see.
    let audit_session_id = notification
        .get("params")
        .and_then(|p| p.get("sessionId"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    let decision = match route {
        PermissionRoute::Strict { option_id } => {
            // A background agent call: refused before any policy, no
            // config row, no prompt. The call stops at its first tool.
            crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(
                crate::permission_audit::AuditEvent::Denied {
                    tool: tool_title.to_string(),
                    session_id: audit_session_id.clone(),
                },
            ));
            send_response(&option_id);
            if let Some(sid) = request_session {
                info!("Refused a tool request on strict session {}", sid);
                crate::ephemeral_session::note_tool_attempt(client, sid);
            }
            return;
        }
        PermissionRoute::Policy(decision) => decision,
    };
    // Auto-approved commands and MCP tools: record what actually ran (the
    // command line / the tool's arguments). Also kept with a prompted
    // request, for when trusting a category approves it from the queue.
    let args_preview = auto_allow_args_preview(
        notification
            .get("params")
            .and_then(|p| p.get("toolCall"))
            .and_then(|tc| tc.get("rawInput")),
        &request,
    );
    match decision.policy {
        crate::config::PolicyKind::Allow => {
            if let Some(reason) = &decision.category_trust {
                info!("Auto-approved a tool call (trusted {})", reason);
            }
            crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(
                crate::permission_audit::AuditEvent::Granted {
                    tool: tool_title.to_string(),
                    grant_type: crate::config::GrantType::Once,
                    session_id: audit_session_id.clone(),
                    args_preview,
                },
            ));
            send_response("allow_once");
        }
        crate::config::PolicyKind::Deny => {
            crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(
                crate::permission_audit::AuditEvent::Denied {
                    tool: tool_title.to_string(),
                    session_id: audit_session_id.clone(),
                },
            ));
            send_response("reject_once");
        }
        crate::config::PolicyKind::Ask => {
            let session_id = notification
                .get("params")
                .and_then(|p| p.get("sessionId"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());

            let request_id = notification
                .get("id")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            if let Ok(mut pending) = pending_perm.lock() {
                pending.insert(
                    crate::state::permission_key(&request_id),
                    crate::state::PendingPermission {
                        request_id,
                        session_id: session_id.clone(),
                        request: request.clone(),
                        args_preview,
                    },
                );
            }

            // Route the modal back to the window that issued the
            // prompt this permission belongs to. The session id arrives
            // on every permission notification; the originator map was
            // written by `send_message_streaming` before the ACP call.
            // Falling back to "floating" preserves the historical
            // default for hotkey-driven prompts that bypass the map
            // (e.g. inline-assist).
            let source = app_handle
                .try_state::<UiState>()
                .and_then(|state| {
                    let sid = session_id.as_deref()?;
                    state
                        .pending_prompt_originators
                        .lock()
                        .ok()
                        .and_then(|m| m.get(sid).cloned())
                })
                .unwrap_or_else(|| window_labels::FLOATING.to_string());

            let payload = permission_prompt_payload(notification, &request, &source);

            // Fan out to the windows that subscribe to permission prompts
            // (chat hosts, floating, settings). Each decides whether to show.
            crate::event_targets::emit_permission_audience(
                app_handle,
                "permission_request",
                &payload,
            );

            // If originated from floating and it's hidden, show it (case 3: background permission)
            if source == window_labels::FLOATING {
                if let Some(floating) = app_handle.get_webview_window(window_labels::FLOATING) {
                    let _ = crate::overlay_windows::present(&floating);
                }
            }
        }
    }
}

#[cfg(test)]
mod strict_permission_tests {
    use super::{reject_once_option, route_permission, PermissionRoute};
    use crate::config::{
        GrantType, PolicyKind, PolicyRowChange, ToolPermissionsConfig, ToolPolicy, ToolRequest,
    };
    use serde_json::json;

    const TS: &str = "2026-11-09T10:00:00Z";

    /// Every way the user's policy could approve a tool on its own: an
    /// explicit Allow row, trust-all, terminator mode, trusted shell
    /// commands and a trusted MCP server.
    fn approve_everything() -> ToolPermissionsConfig {
        ToolPermissionsConfig {
            trust_all: true,
            terminator_mode: true,
            trusted_kinds: vec!["execute".into()],
            trusted_mcp_servers: vec!["notes-mcp".into()],
            tools: vec![ToolPolicy {
                title: "Running: rm -rf ~".into(),
                policy: PolicyKind::Allow,
                last_seen: "2026-11-01T00:00:00Z".into(),
                granted_at: "2026-11-01T00:00:00Z".into(),
                grant_type: GrantType::Always,
            }],
            // Commands outside the agent's sandbox: a rule and allow-all.
            host_command_rules: vec![crate::config::HostCommandRule {
                prefix: "rm".into(),
                created_at: String::new(),
            }],
            host_commands_allow_all: true,
        }
    }

    fn requests() -> Vec<ToolRequest> {
        vec![
            ToolRequest {
                title: "Running: rm -rf ~".into(),
                kind: Some("execute".into()),
                hidden_session: true,
                ..Default::default()
            },
            ToolRequest {
                title: "Running: @notes-mcp/search".into(),
                mcp_server: Some("notes-mcp".into()),
                hidden_session: true,
                ..Default::default()
            },
            ToolRequest {
                title: "a tool never seen before".into(),
                hidden_session: true,
                ..Default::default()
            },
        ]
    }

    #[test]
    fn a_strict_session_is_refused_before_any_policy() {
        let n = json!({ "id": 3, "params": { "sessionId": "strict-1" } });
        for request in requests() {
            let mut perms = approve_everything();
            let before = format!("{:?}", perms);
            let route = route_permission(&mut perms, &n, &request, true, TS);
            assert_eq!(
                route,
                PermissionRoute::Strict {
                    option_id: "reject_once".into()
                },
                "{}",
                request.title
            );
            assert_eq!(
                format!("{:?}", perms),
                before,
                "no row added or touched for {}",
                request.title
            );
        }
    }

    #[test]
    fn the_same_requests_outside_a_strict_session_follow_the_policy() {
        let n = json!({ "id": 3, "params": { "sessionId": "user-chat" } });
        let mut perms = approve_everything();
        match route_permission(&mut perms, &n, &requests()[0], false, TS) {
            PermissionRoute::Policy(d) => {
                assert_eq!(d.policy, PolicyKind::Allow);
                assert_eq!(d.row, PolicyRowChange::Touched);
            }
            other => panic!("expected the policy, got {other:?}"),
        }
        match route_permission(&mut perms, &n, &requests()[2], false, TS) {
            PermissionRoute::Policy(d) => {
                assert_eq!(d.policy, PolicyKind::Allow, "terminator mode");
                assert_eq!(d.row, PolicyRowChange::Added);
            }
            other => panic!("expected the policy, got {other:?}"),
        }
        assert_eq!(perms.tools.len(), 2);
    }

    #[test]
    fn the_refusal_uses_the_agents_own_reject_once_option() {
        let n = json!({ "params": { "options": [
            { "kind": "allow_always", "optionId": "allow_always" },
            { "kind": "allow_once", "optionId": "allow" },
            { "kind": "reject_once", "optionId": "reject" },
        ] } });
        assert_eq!(reject_once_option(&n), "reject");
    }

    #[test]
    fn the_refusal_never_picks_an_allow_option() {
        for n in [
            json!({ "params": {} }),
            json!({ "params": { "options": "nonsense" } }),
            json!({ "params": { "options": [
                { "kind": "allow_once", "optionId": "reject_once_please" },
                { "kind": "reject_once" },
                { "kind": "reject_once", "optionId": "" },
            ] } }),
        ] {
            assert_eq!(reject_once_option(&n), "reject_once", "{n}");
        }
    }
}

#[cfg(test)]
mod permission_payload_tests {
    use super::{auto_allow_args_preview, permission_prompt_payload};
    use crate::config::ToolRequest;
    use serde_json::json;

    fn mcp_request(hidden_session: bool) -> ToolRequest {
        ToolRequest {
            title: "Running: @builder-mcp/InternalSearch".into(),
            kind: None,
            mcp_server: Some("builder-mcp".into()),
            hidden_session,
        }
    }

    #[test]
    fn prompt_payload_carries_what_the_trust_ui_needs() {
        let n = json!({ "id": 7, "params": { "sessionId": "s1" } });
        let p = permission_prompt_payload(&n, &mcp_request(false), "main");
        assert_eq!(p["notification"], n);
        assert_eq!(p["auto_approve"], false);
        assert_eq!(p["toolName"], "Running: @builder-mcp/InternalSearch");
        assert_eq!(p["toolKind"], serde_json::Value::Null);
        assert_eq!(p["mcpServer"], "builder-mcp");
        assert_eq!(p["backgroundSession"], false);
        assert_eq!(p["source"], "main");

        let p = permission_prompt_payload(&n, &mcp_request(true), "floating");
        assert_eq!(p["backgroundSession"], true);
        let shell = ToolRequest {
            title: "Running: ls".into(),
            kind: Some("execute".into()),
            ..Default::default()
        };
        let p = permission_prompt_payload(&n, &shell, "main");
        assert_eq!(p["toolKind"], "execute");
        assert_eq!(p["mcpServer"], serde_json::Value::Null);
    }

    #[test]
    fn args_preview_records_commands_and_mcp_arguments_only() {
        let shell = ToolRequest {
            kind: Some("execute".into()),
            ..Default::default()
        };
        let raw = json!({ "command": "git status" });
        assert_eq!(
            auto_allow_args_preview(Some(&raw), &shell).as_deref(),
            Some("git status")
        );
        let raw = json!({ "query": "kage" });
        assert_eq!(
            auto_allow_args_preview(Some(&raw), &mcp_request(false)).as_deref(),
            Some(r#"{"query":"kage"}"#)
        );
        assert_eq!(
            auto_allow_args_preview(Some(&serde_json::Value::Null), &mcp_request(false)),
            None
        );
        let edit = ToolRequest {
            kind: Some("edit".into()),
            ..Default::default()
        };
        assert_eq!(auto_allow_args_preview(Some(&raw), &edit), None);
        let long = json!({ "command": "x".repeat(5000) });
        assert_eq!(
            auto_allow_args_preview(Some(&long), &shell).map(|s| s.chars().count()),
            Some(2048)
        );
    }
}

#[cfg(test)]
mod host_command_permission_tests {
    use super::is_host_command_call;

    #[test]
    fn kages_own_tool_is_allowed_through_by_exact_server_and_tool() {
        for t in [
            "Running: @kage-computer-control/run_outside_sandbox",
            "mcp__kage-computer-control__run_outside_sandbox",
        ] {
            assert!(is_host_command_call(t, Some(t), None), "{t}");
            assert!(is_host_command_call(t, None, Some("other")), "{t}");
        }
    }

    #[test]
    fn nothing_else_is() {
        let ours = "Running: @kage-computer-control/run_outside_sandbox";
        // A shell command spelled like it.
        assert!(!is_host_command_call(ours, Some(ours), Some("execute")));
        // Another tool of the same server, or the same tool elsewhere.
        assert!(!is_host_command_call(
            "Running: @kage-computer-control/run_script",
            None,
            None
        ));
        assert!(!is_host_command_call(
            "Running: @other-server/run_outside_sandbox",
            None,
            None
        ));
        assert!(!is_host_command_call(
            "mcp__ka-computer-control__run_outside_sandbox",
            None,
            None
        ));
        // Titles that disagree.
        assert!(!is_host_command_call(
            ours,
            Some("Running: @kage-computer-control/run_script"),
            None
        ));
    }
}

#[cfg(test)]
mod mcp_server_tests {
    use super::classify_mcp_server;

    #[test]
    fn kiro_mcp_call_names_its_server() {
        let t = "Running: @builder-mcp/InternalSearch";
        assert_eq!(
            classify_mcp_server(t, Some(t), None).as_deref(),
            Some("builder-mcp")
        );
        assert_eq!(
            classify_mcp_server(t, Some(t), Some("other")).as_deref(),
            Some("builder-mcp")
        );
        // No request title (only the recorded one) is fine too.
        assert_eq!(
            classify_mcp_server("Running: @kage-computer-control/launch_app", None, None)
                .as_deref(),
            Some("kage-computer-control")
        );
    }

    #[test]
    fn claude_mcp_call_names_its_server() {
        let t = "mcp__builder-mcp__InternalSearch";
        assert_eq!(
            classify_mcp_server(t, Some(t), Some("other")).as_deref(),
            Some("builder-mcp")
        );
    }

    #[test]
    fn a_shell_command_is_never_an_mcp_call() {
        // kiro-cli titles shell runs "Running: <command>": a command spelled
        // like an MCP tool, or cut at a newline, must not read as one.
        let t = "Running: @builder-mcp/InternalSearch";
        assert_eq!(classify_mcp_server(t, Some(t), Some("execute")), None);
        assert_eq!(
            classify_mcp_server("Running: cd x; @foo/bar", None, None),
            None
        );
        assert_eq!(
            classify_mcp_server("Running: cd x; @foo/bar", None, Some("execute")),
            None
        );
    }

    #[test]
    fn built_in_kinds_are_never_mcp_calls() {
        let t = "mcp__fs__write";
        for kind in ["edit", "read", "delete", "move", "search", "fetch", "think"] {
            assert_eq!(classify_mcp_server(t, Some(t), Some(kind)), None, "{kind}");
        }
    }

    #[test]
    fn titles_that_disagree_are_ambiguous() {
        assert_eq!(
            classify_mcp_server(
                "Running: @builder-mcp/InternalSearch",
                Some("Running: @other/InternalSearch"),
                None
            ),
            None
        );
        assert_eq!(
            classify_mcp_server(
                "Running: @builder-mcp/InternalSearch",
                Some("Running: rm -rf ~"),
                None
            ),
            None
        );
        // Same server, different tool wording: still that server.
        assert_eq!(
            classify_mcp_server(
                "Running: @builder-mcp/InternalSearch",
                Some("mcp__builder-mcp__InternalSearch"),
                None
            )
            .as_deref(),
            Some("builder-mcp")
        );
        // An empty request title adds nothing.
        assert_eq!(
            classify_mcp_server("Running: @builder-mcp/InternalSearch", Some(""), None).as_deref(),
            Some("builder-mcp")
        );
    }

    #[test]
    fn non_mcp_titles_have_no_server() {
        assert_eq!(classify_mcp_server("Searching the web", None, None), None);
        assert_eq!(classify_mcp_server("unknown", None, None), None);
        assert_eq!(classify_mcp_server("ext:todos/add", None, None), None);
    }
}

#[cfg(test)]
mod tool_kind_tests {
    use super::classify_tool_kind;
    use serde_json::json;

    #[test]
    fn request_kind_wins() {
        let tc = json!({ "kind": "execute", "title": "Running: ls" });
        assert_eq!(
            classify_tool_kind(Some(&tc), Some("other")).as_deref(),
            Some("execute")
        );
    }

    #[test]
    fn falls_back_to_the_recorded_tool_call_kind() {
        let tc = json!({ "title": "Running: ls" });
        assert_eq!(
            classify_tool_kind(Some(&tc), Some("execute")).as_deref(),
            Some("execute")
        );
        // Empty strings count as missing.
        let tc = json!({ "kind": "" });
        assert_eq!(
            classify_tool_kind(Some(&tc), Some("edit")).as_deref(),
            Some("edit")
        );
    }

    #[test]
    fn arguments_never_promote_a_tool_to_execute() {
        // kiro-cli's file-write tool: `command` names a sub-operation, and
        // ACP defines a missing kind as "other". Must not count as a shell.
        let tc = json!({ "rawInput": { "command": "create", "path": "a.rs" } });
        assert_eq!(classify_tool_kind(Some(&tc), None), None);
        let tc = json!({ "rawInput": { "command": "git status" } });
        assert_eq!(classify_tool_kind(Some(&tc), None), None);
    }

    #[test]
    fn unknown_without_any_signal() {
        assert_eq!(classify_tool_kind(None, None), None);
        let tc = json!({ "title": "Editing main.rs", "rawInput": { "path": "main.rs" } });
        assert_eq!(classify_tool_kind(Some(&tc), None), None);
    }
}

#[cfg(test)]
mod tests {
    use super::{rate_limit_route, RateLimitRoute};

    #[test]
    fn background_rate_limit_reaches_no_window() {
        // A titler/auto-steering prompt hitting the limit must not tear
        // down an unrelated answer streaming in a chat window.
        assert_eq!(
            rate_limit_route(true, true, Some("main".into()), vec!["main".into()]),
            RateLimitRoute::LogOnly
        );
    }

    #[test]
    fn rate_limit_goes_to_the_originating_window_only() {
        assert_eq!(
            rate_limit_route(
                true,
                false,
                Some("chat-1".into()),
                vec!["chat-1".into(), "main".into()]
            ),
            RateLimitRoute::Windows(vec!["chat-1".into()])
        );
    }

    #[test]
    fn untagged_rate_limit_goes_to_windows_pinned_to_the_session() {
        // Recovery resends under a migrated id and shared-session epilogues
        // can leave a visible prompt untagged; its pinned windows still
        // need to hear about the limit.
        assert_eq!(
            rate_limit_route(true, false, None, vec!["main".into()]),
            RateLimitRoute::Windows(vec!["main".into()])
        );
        assert_eq!(
            rate_limit_route(true, false, None, Vec::new()),
            RateLimitRoute::LogOnly
        );
    }

    #[test]
    fn rate_limit_without_session_id_falls_back_to_broadcast() {
        assert_eq!(
            rate_limit_route(false, false, None, Vec::new()),
            RateLimitRoute::Broadcast
        );
    }
}
