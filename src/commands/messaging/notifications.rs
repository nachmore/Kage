//! ACP notification handler: streaming chunk fan-out + flush thread,
//! permission-request routing, and standard-ACP slash-command discovery.

use super::*;

/// Normalize standard-ACP `availableCommands` entries into our `SlashCommand`
/// shape. Standard ACP (Claude Code, etc.) sends
/// `{ name, description, input: { hint } | null }` and runs the command as a
/// normal prompt — so each is tagged `dispatch = "prompt"`. The `input.hint`
/// becomes the meta hint; presence of any `input` object marks the command as
/// taking free-text (mapped to `inputType = "text"`), absence as a plain
/// fire-and-run command.
fn parse_standard_acp_commands(cmds: &[serde_json::Value]) -> Vec<crate::state::SlashCommand> {
    cmds.iter()
        .filter_map(|c| {
            let raw_name = c.get("name").and_then(|v| v.as_str())?;
            // Normalize to a leading-slash name. Kiro's vendor commands arrive
            // as "/agent"; standard ACP sends bare "context". The frontend
            // assumes the "/" prefix (it does `name.substring(1)`), so prepend
            // it here to keep one shape across agents.
            let name = if raw_name.starts_with('/') {
                raw_name.to_string()
            } else {
                format!("/{raw_name}")
            };
            let description = c
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input = c.get("input");
            let hint = input
                .and_then(|i| i.get("hint"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            // `input: null` → plain command; `input: { ... }` → takes text.
            let input_type = match input {
                Some(v) if !v.is_null() => Some("text".to_string()),
                _ => None,
            };
            let meta = if hint.is_some() || input_type.is_some() {
                Some(crate::state::SlashCommandMeta {
                    options_method: None,
                    input_type,
                    hint,
                    local: None,
                })
            } else {
                None
            };
            Some(crate::state::SlashCommand {
                name,
                description,
                meta,
                dispatch: "prompt".to_string(),
            })
        })
        .collect()
}

/// Set up the notification handler on the ACP client.
/// This should be called once after the client is created.
/// The handler dispatches all ACP notifications to the appropriate Tauri events.
pub fn setup_notification_handler(
    client: std::sync::Arc<crate::acp_client::AcpClient>,
    app: &tauri::AppHandle,
    state_config: std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    slash_commands: std::sync::Arc<std::sync::Mutex<Vec<crate::state::SlashCommand>>>,
    pending_permission: std::sync::Arc<std::sync::Mutex<Option<crate::state::PendingPermission>>>,
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
    let tool_names: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>> =
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
    // without changing the on-screen feel of streaming.
    let pending_chunks: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, String>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
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

        if method == "session/update" {
            // Drop conversation-history replay. A `session/load` makes
            // kiro-cli re-emit every prior turn as a burst of session/update
            // notifications before the load response returns; those are
            // history, not live output. Without this gate they dump the old
            // conversation into the floating window and poison the streaming
            // accumulators (auto_steering / sub-agent reads). The flag is set
            // for the duration of the load request in `load_existing_session`.
            if client_for_handler.is_loading_session() {
                return;
            }

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

                            if let Some(emitted) = emitted_owned {
                                if !emitted.is_empty() {
                                    // Append to the per-session pending
                                    // buffer. The flush thread emits
                                    // `message_chunk` events with this
                                    // text every CHUNK_FLUSH_INTERVAL_MS,
                                    // one per non-empty session bucket.
                                    // Frontend filters by sessionId.
                                    if let Ok(mut map) = pending_chunks.lock() {
                                        map.entry(sid).or_default().push_str(&emitted);
                                    }
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
                        // Track the first title for each toolCallId — that's the real tool name
                        if let (Some(call_id), Some(title)) = (
                            update.get("toolCallId").and_then(|v| v.as_str()),
                            update.get("title").and_then(|v| v.as_str()),
                        ) {
                            if let Ok(mut names) = tool_names.lock() {
                                let was_new = !names.contains_key(call_id);
                                names.entry(call_id.to_string()).or_insert_with(|| title.to_string());
                                // Cap memory: tool call IDs are UUID-ish and we
                                // never prune on session-end. A long-running
                                // session can leak megabytes here. Keep the
                                // 4096 most recent — well above the working set
                                // for any realistic conversation.
                                const MAX_TOOL_NAMES: usize = 4096;
                                if was_new && names.len() > MAX_TOOL_NAMES {
                                    // HashMap iteration order is not insertion
                                    // order, so this is "drop arbitrary 25%"
                                    // rather than strict LRU. Acceptable: a
                                    // mis-attributed tool name in an audit log
                                    // is preferable to unbounded growth.
                                    let drop_n = names.len() - MAX_TOOL_NAMES * 3 / 4;
                                    let to_drop: Vec<String> = names
                                        .keys()
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
                        // filters by sessionId in the payload.
                        crate::event_targets::emit_streaming_audience(
                            &app_handle,
                            events::TOOL_CALL_UPDATE,
                            &notification,
                        );
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
                    let message = notification.get("params")
                        .and_then(|p| p.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("Rate limit exceeded. Please wait a moment before trying again.");
                    warn!("Rate limit hit: {}", message);
                    crate::event_targets::emit_streaming_audience(
                        &app_handle,
                        events::MESSAGE_ERROR,
                        &message,
                    );
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

/// Background thread that drains `pending_chunks` every
/// CHUNK_FLUSH_INTERVAL_MS and emits one `message_chunk` event per non-
/// empty session bucket. Replaces the pre-fix one-emit-per-token path,
/// which fired hundreds-to-thousands of IPC roundtrips per response.
///
/// The thread runs for the AcpClient's lifetime — it's a single OS thread
/// (`acp-chunk-flush`) doing a HashMap drain + 0..N emits per cycle, so
/// the always-on cost is negligible. Exit is by `app_handle.emit` returning
/// an error after the app shuts down; we log and break.
fn spawn_chunk_flush_thread(
    app_handle: tauri::AppHandle,
    pending: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
) {
    let _ = std::thread::Builder::new()
        .name("acp-chunk-flush".into())
        .spawn(move || {
            let interval = std::time::Duration::from_millis(CHUNK_FLUSH_INTERVAL_MS);
            loop {
                std::thread::sleep(interval);
                let alive =
                    crate::chunk_batcher::drain_and_emit_pending(&pending, |session_id, text| {
                        let payload = serde_json::json!({
                            "text": text,
                            "sessionId": session_id,
                        });
                        // Streaming-audience target — chat hosts + floating +
                        // settings — so the per-frame chunk doesn't fan out
                        // to every webview that happens to subscribe to
                        // anything else. We call `emit_filter` directly here
                        // (rather than the helper) because the chunk-flush
                        // thread relies on the emit returning Err at app
                        // shutdown to break the loop; the helper swallows
                        // errors at debug-log level.
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
                    });
                if !alive {
                    return;
                }
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn handle_permission_notification(
    notification: &serde_json::Value,
    app_handle: &tauri::AppHandle,
    config: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    client: &crate::acp_client::AcpClient,
    pending_perm: &std::sync::Arc<std::sync::Mutex<Option<crate::state::PendingPermission>>>,
    tool_names: &std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
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

    // Look up the real tool name from the first tool_call update
    let tool_title = if !tool_call_id.is_empty() {
        tool_names
            .lock()
            .ok()
            .and_then(|names| names.get(tool_call_id).cloned())
            .unwrap_or_else(|| fallback_title.to_string())
    } else {
        fallback_title.to_string()
    };

    let timestamp = chrono::Utc::now().to_rfc3339();

    // Hold the lock just long enough to mutate the in-memory config and
    // decide whether a disk save is due. The actual save is dispatched
    // off-thread (see below) so the ACP reader thread that's invoking
    // this notification handler is never blocked on disk I/O — pre-fix
    // a slow disk could stall ACP message parsing for tens of ms,
    // long enough for in-flight prompts to time out.
    let (policy, save_snapshot) = {
        let mut config_guard = config.lock_or_recover();
        let existing = config_guard
            .tool_permissions
            .tools
            .iter_mut()
            .find(|t| t.title == tool_title);
        let needs_save: bool;
        if let Some(tool) = existing {
            // Update last_seen in memory — throttle disk writes to at most once per 60s
            tool.last_seen = timestamp;
            let mut last_save = last_config_save.lock_or_recover();
            needs_save = last_save.elapsed() >= std::time::Duration::from_secs(60);
            if needs_save {
                *last_save = std::time::Instant::now();
            }
        } else {
            config_guard
                .tool_permissions
                .tools
                .push(crate::config::ToolPolicy {
                    title: tool_title.to_string(),
                    policy: crate::config::PolicyKind::Ask,
                    last_seen: timestamp,
                    granted_at: String::new(),
                    grant_type: crate::config::GrantType::Once,
                });
            // New tool discovered — save immediately
            needs_save = true;
            *last_config_save.lock_or_recover() = std::time::Instant::now();
        }

        let p = if config_guard.tool_permissions.terminator_mode
            || config_guard.tool_permissions.trust_all
        {
            crate::config::PolicyKind::Allow
        } else {
            config_guard
                .tool_permissions
                .tools
                .iter()
                .find(|t| t.title == tool_title)
                .map(|t| t.effective_policy())
                .unwrap_or(crate::config::PolicyKind::Ask)
        };
        // Snapshot the config for an off-thread save if one is due.
        // The clone happens inside the lock so the snapshot is
        // consistent — but the Mutex is released as the guard drops
        // at end-of-block, BEFORE the save runs.
        let snap = if needs_save {
            Some(config_guard.clone())
        } else {
            None
        };
        (p, snap)
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

    match policy {
        crate::config::PolicyKind::Allow => send_response("allow_once"),
        crate::config::PolicyKind::Deny => send_response("reject_once"),
        crate::config::PolicyKind::Ask => {
            if let Ok(mut pending) = pending_perm.lock() {
                *pending = Some(crate::state::PendingPermission {
                    request_id: notification
                        .get("id")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                });
            }

            // Route the modal back to the window that issued the
            // prompt this permission belongs to. The session id arrives
            // on every permission notification; the originator map was
            // written by `send_message_streaming` before the ACP call.
            // Falling back to "floating" preserves the historical
            // default for hotkey-driven prompts that bypass the map
            // (e.g. inline-assist).
            let session_id = notification
                .get("params")
                .and_then(|p| p.get("sessionId"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
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

            let payload = serde_json::json!({
                "notification": notification,
                "auto_approve": false,
                "toolName": tool_title,
                "source": source,
            });

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
                    let _ = floating.show();
                    let _ = floating.set_focus();
                }
            }
        }
    }
}

#[cfg(test)]
mod slash_discovery_tests {
    use super::parse_standard_acp_commands;

    #[test]
    fn normalizes_claude_available_commands() {
        // Trimmed copy of a real Claude Code available_commands_update.
        let cmds = serde_json::json!([
            { "name": "context", "description": "Show current context usage", "input": null },
            { "name": "compact", "description": "Clear history but keep a summary",
              "input": { "hint": "<optional custom summarization instructions>" } },
            { "name": "review", "description": "Review a pull request", "input": null }
        ]);
        let parsed = parse_standard_acp_commands(cmds.as_array().unwrap());
        assert_eq!(parsed.len(), 3);

        // Names get a leading slash so the frontend's substring(1) holds.
        assert_eq!(parsed[0].name, "/context");
        assert_eq!(parsed[1].name, "/compact");
        // All standard-ACP commands are prompt-dispatched.
        assert!(parsed.iter().all(|c| c.dispatch == "prompt"));

        // input:null → no meta (plain fire-and-run command).
        assert!(parsed[0].meta.is_none());
        // input:{hint} → meta carries the hint and marks free-text input.
        let compact_meta = parsed[1].meta.as_ref().unwrap();
        assert_eq!(compact_meta.input_type.as_deref(), Some("text"));
        assert_eq!(
            compact_meta.hint.as_deref(),
            Some("<optional custom summarization instructions>")
        );
    }

    #[test]
    fn already_slashed_names_are_left_alone() {
        let cmds = serde_json::json!([{ "name": "/foo", "description": "", "input": null }]);
        let parsed = parse_standard_acp_commands(cmds.as_array().unwrap());
        assert_eq!(parsed[0].name, "/foo");
    }

    #[test]
    fn entries_without_a_name_are_skipped() {
        let cmds = serde_json::json!([
            { "description": "no name here", "input": null },
            { "name": "ok", "description": "fine", "input": null }
        ]);
        let parsed = parse_standard_acp_commands(cmds.as_array().unwrap());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "/ok");
    }

    #[test]
    fn vendor_commands_default_to_vendor_dispatch() {
        // The Kiro path deserializes via serde; dispatch must default to
        // "vendor" when the wire payload omits it.
        let cmd: crate::state::SlashCommand = serde_json::from_value(serde_json::json!({
            "name": "/agent", "description": "Select an agent"
        }))
        .unwrap();
        assert_eq!(cmd.dispatch, "vendor");
    }
}
