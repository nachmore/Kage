//! Targeted-emit helpers — fan events out only to the windows that
//! subscribe.
//!
//! # Why this exists
//!
//! `app.emit("foo", ...)` looks like it sends to one place but, under
//! the hood, Tauri's manager hands every webview that has a
//! `listen("foo", ...)` subscriber a `WebviewMessage::EvaluateScript`
//! to dispatch the event JS-side. For events with multiple subscribers
//! (e.g. `permission_request`, observed in chat + floating + settings)
//! that means N synchronous round-trips through wry's event loop per
//! emit — and crucially, every one of those `eval_script` calls is
//! a place where a wedged WebView2 host can surface
//! `0x8007139F (ERROR_INVALID_STATE)`.
//!
//! Most of our events have exactly one audience (chat hosts, floating,
//! inline-assist, settings). Emitting via `emit_filter` so the manager
//! skips webviews outside that audience does three things:
//!
//!   1. Cuts the per-emit work to exactly the subscribers that need
//!      the event. Hot paths like `sessions_changed` (file watcher
//!      debounce, ~once per directory change) and the streaming
//!      `MESSAGE_CHUNK` pump no longer poke webviews that aren't
//!      listening anyway.
//!   2. Gives the wedge-recovery path (see `webview_recovery`) a clean
//!      way to quarantine a single wedged window: future emits stop
//!      hitting it on *every* unrelated event, only on the ones the
//!      wedged window genuinely subscribed to.
//!   3. Documents intent at every call site — the audience is in the
//!      function name, not buried in JS listener wiring on the other
//!      side of an IPC boundary.
//!
//! # Audiences
//!
//! Each helper here corresponds to a subscriber set the frontend
//! audit found (see `ui/js/<window>/...` for the listeners). When
//! adding a new event, pick the helper whose subscriber set matches —
//! NOT `Emitter::emit` — unless the event genuinely fans out to every
//! window (only `CONFIG_UPDATED` qualifies today).

use crate::window_labels;
use serde::Serialize;
use tauri::{Emitter, EventTarget, Runtime};

/// Emit to chat-host windows: the primary `main` window and any
/// `chat-<uuid>` peer windows. Used for events that drive the chat
/// session UI: `sessions_changed`, `context_metadata`, streaming
/// chunks, tool-call updates.
pub fn emit_to_chat_hosts<R, S>(app: &impl Emitter<R>, event: &str, payload: &S)
where
    R: Runtime,
    S: Serialize + Clone,
{
    if let Err(e) = app.emit_filter(event, payload, |t| match t {
        EventTarget::Window { label }
        | EventTarget::Webview { label }
        | EventTarget::WebviewWindow { label }
        | EventTarget::AnyLabel { label } => window_labels::is_session_host_label(label),
        _ => false,
    }) {
        log::debug!("emit_to_chat_hosts({event}) failed: {e}");
    }
}

/// Emit to the floating launcher only.
pub fn emit_to_floating<R, S>(app: &impl Emitter<R>, event: &str, payload: &S)
where
    R: Runtime,
    S: Serialize + Clone,
{
    emit_to_label(app, event, payload, window_labels::FLOATING);
}

/// Emit to the inline-assist popup only.
pub fn emit_to_inline_assist<R, S>(app: &impl Emitter<R>, event: &str, payload: &S)
where
    R: Runtime,
    S: Serialize + Clone,
{
    emit_to_label(app, event, payload, window_labels::INLINE_ASSIST);
}

/// Emit to the settings window only. The settings window is on-demand,
/// so the emit is a no-op when it's never been opened — the `emit_filter`
/// short-circuits per webview, so nothing happens for absent labels.
pub fn emit_to_settings<R, S>(app: &impl Emitter<R>, event: &str, payload: &S)
where
    R: Runtime,
    S: Serialize + Clone,
{
    emit_to_label(app, event, payload, window_labels::SETTINGS);
}

/// Emit back to the source window of a Tauri command. `WebviewWindow::emit`
/// looks targeted but actually broadcasts to every webview that has a
/// listener; `emit_to_self` is what most reply-to-caller paths actually
/// want. Logs at `debug` if the dispatch fails (tracing failures here is
/// noisy and rarely actionable — a closed window is the common cause).
pub fn emit_to_self<R, S>(window: &tauri::WebviewWindow<R>, event: &str, payload: &S)
where
    R: Runtime,
    S: Serialize + Clone,
{
    let label = window.label();
    if let Err(e) = window.emit_to(
        tauri::EventTarget::webview_window(label),
        event,
        payload.clone(),
    ) {
        log::debug!("emit_to_self({event} -> {label}) failed: {e}");
    }
}

/// Emit to the windows that consume streamed agent traffic:
/// `MESSAGE_CHUNK` / `MESSAGE_COMPLETE` / `MESSAGE_ERROR` /
/// `TOOL_CALL_UPDATE` / `COMPACTION_STATUS`.
///
/// Audience: chat hosts (`main` + `chat-*`), floating, and settings
/// (macros + shortcuts test-prompts subscribe through the shared
/// streaming-utils helpers). The frontend filters by `sessionId`
/// inside each event payload, so the per-token spray hits multiple
/// windows by design — they're each driving their own session view.
///
/// Carved out of `chat_hosts` because the streaming pump fires at
/// ~60fps for an entire response: the difference between "every
/// webview gets eval_scripted on every token" and "exactly the
/// streaming-aware webviews do" matters here more than anywhere
/// else.
pub fn emit_streaming_audience<R, S>(app: &impl Emitter<R>, event: &str, payload: &S)
where
    R: Runtime,
    S: Serialize + Clone,
{
    emit_long_lived_ui(app, event, payload, "emit_streaming_audience");
}

/// Emit to the long-lived UI windows: chat hosts (`main` + `chat-*`),
/// the floating launcher, and the settings window. This is the most
/// common audience for "user-relevant state changed" signals.
///
/// `emit_permission_audience`, `emit_streaming_audience`, and
/// `emit_update_audience` are aliases (same filter, named after
/// purpose) — the audience is shared, but call sites read more
/// clearly when the helper name announces the intent.
///
/// The frontend audit (2026-06-01) found these importers of
/// `ui/js/shared/permissions-core.js`: floating/app.js,
/// floating/permissions.js, chat/permissions.js,
/// settings/permissions.js. Welcome / store / inline-assist /
/// context-menu don't subscribe.
pub fn emit_permission_audience<R, S>(app: &impl Emitter<R>, event: &str, payload: &S)
where
    R: Runtime,
    S: Serialize + Clone,
{
    emit_long_lived_ui(app, event, payload, "emit_permission_audience");
}

/// Emit to the long-lived UI audience for update events
/// (`update_available`, etc). Same membership as
/// `emit_permission_audience`; helper exists so the call site reads
/// "update audience" instead of "permission audience" while still
/// going through one filter implementation.
pub fn emit_update_audience<R, S>(app: &impl Emitter<R>, event: &str, payload: &S)
where
    R: Runtime,
    S: Serialize + Clone,
{
    emit_long_lived_ui(app, event, payload, "emit_update_audience");
}

fn emit_long_lived_ui<R, S>(app: &impl Emitter<R>, event: &str, payload: &S, caller: &'static str)
where
    R: Runtime,
    S: Serialize + Clone,
{
    if let Err(e) = app.emit_filter(event, payload, |t| match t {
        EventTarget::Window { label }
        | EventTarget::Webview { label }
        | EventTarget::WebviewWindow { label }
        | EventTarget::AnyLabel { label } => {
            window_labels::is_session_host_label(label)
                || label == window_labels::FLOATING
                || label == window_labels::SETTINGS
        }
        _ => false,
    }) {
        log::debug!("{caller}({event}) failed: {e}");
    }
}

fn emit_to_label<R, S>(app: &impl Emitter<R>, event: &str, payload: &S, target_label: &str)
where
    R: Runtime,
    S: Serialize + Clone,
{
    let target_label = target_label.to_string();
    if let Err(e) = app.emit_filter(event, payload, |t| match t {
        EventTarget::Window { label }
        | EventTarget::Webview { label }
        | EventTarget::WebviewWindow { label }
        | EventTarget::AnyLabel { label } => label == &target_label,
        _ => false,
    }) {
        log::debug!("emit_to_label({event} -> {target_label}) failed: {e}");
    }
}
