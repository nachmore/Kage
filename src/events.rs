//! Tauri event names. Centralised so a typo in either the emit or the
//! listen side fails to compile (Rust) or fails a test (JS) instead of
//! silently dropping the event at runtime.
//!
//! **Casing convention: snake_case.** Several legacy events used hyphens
//! (`show-sessions`, `inline-assist-show`, `context-menu-action`) — they
//! were renamed to underscore form when these constants were introduced.
//! Add new events as snake_case; the test below enforces the rule.
//!
//! **Scope.** Only events with multiple emit sites or cross-language
//! Rust↔JS use carry a constant. Single-emit, single-listen events
//! that live entirely in Rust are already centralised by virtue of
//! having one definition and don't need a const just to have one.
//!
//! Mirror constants live in `ui/js/shared/events.js`; the alignment
//! test verifies the two lists agree.

// --- Config / app state ---------------------------------------------

/// Config was saved or imported. Every window listens and reapplies
/// theme + hotkeys + cached config from this signal.
pub const CONFIG_UPDATED: &str = "config_updated";

/// Extension list / config / install state changed. Settings page
/// re-renders; floating window invalidates its cached store list.
pub const EXTENSIONS_CHANGED: &str = "extensions_changed";

// --- Updater --------------------------------------------------------

/// New update is available — payload is the version string.
/// Both the daily-check loop and the `check_for_update` command emit.
pub const UPDATE_AVAILABLE: &str = "update_available";

/// Show a banner on the floating window. Used for both the post-update
/// celebration and the welcome banner.
pub const SHOW_FLOATING_BANNER: &str = "show_floating_banner";

// --- Streaming / agent traffic --------------------------------------

/// Streaming chunk for the active session. Routed to chat-host windows
/// (main + chat-<uuid>); the chunk_batcher emits at ~60fps.
pub const MESSAGE_CHUNK: &str = "message_chunk";

/// Final commit for an in-flight message — payload includes the
/// canonical message id.
pub const MESSAGE_COMPLETE: &str = "message_complete";

/// Failure during a send / stream — payload is a string error.
pub const MESSAGE_ERROR: &str = "message_error";

/// Tool-call status update from the agent. Frontend filters by sessionId
/// in the payload, so this fires unconditionally.
pub const TOOL_CALL_UPDATE: &str = "tool_call_update";

/// Conversation compaction progress (kage/kiro vendor extension).
pub const COMPACTION_STATUS: &str = "compaction_status";

/// A user-initiated prompt started on a session. Payload:
/// `{ sessionId, source }` where source is the originating window
/// label. Lets chat hosts show live activity (spinner/unread badges)
/// for sessions they are NOT currently viewing. Steering, titling,
/// and other hidden prompts do not emit this.
pub const SESSION_ACTIVITY: &str = "session_activity";

// --- Permissions ----------------------------------------------------

/// User dismissed a pending permission prompt — refresh the UI's
/// pending-permission slot indicator.
pub const PERMISSION_DISMISSED: &str = "permission_dismissed";

// --- Inline assist (formerly hyphen-cased) --------------------------

/// Show the inline-assist window — emitted to the inline-assist webview
/// when a hotkey fires. Renamed from `inline-assist-show`.
pub const INLINE_ASSIST_SHOW: &str = "inline_assist_show";

/// Inline assist failure (string payload).
pub const INLINE_ASSIST_ERROR: &str = "inline_assist_error";

// --- Sessions (formerly hyphen-cased) -------------------------------

/// Single-instance second-launch reactivation: bring main + sessions
/// list to the front. Renamed from `show-sessions`.
pub const SHOW_SESSIONS: &str = "show_sessions";

// --- Hotkey-loop event names ----------------------------------------

/// Floating window: enter clipboard-history mode. Fired after the
/// clipboard hotkey opens the floating window so the JS bootstrap
/// has time to initialise before switching modes.
pub const CLIPBOARD_HISTORY_MODE: &str = "clipboard_history_mode";

/// Floating window: enter voice-input mode. Same delayed pattern as
/// `CLIPBOARD_HISTORY_MODE`.
pub const VOICE_MODE: &str = "voice_mode";

/// One or more global hotkeys could not be registered with the OS (another
/// app already owns the combo, most commonly). Payload is a JSON array of
/// `{ slot, hotkey }` objects. Broadcast so any open window — but especially
/// Settings → Hotkeys — can surface a warning instead of the failure being
/// buried in the log.
pub const HOTKEY_REGISTRATION_FAILED: &str = "hotkey_registration_failed";

/// The agent backend's stream closed (EOF or error) — the process died or the
/// remote connection dropped. Emitted from the ACP reader thread's teardown so
/// windows can drop their "connected" indicator immediately instead of looking
/// healthy until the next send fails. No payload.
pub const AGENT_DISCONNECTED: &str = "agent_disconnected";

// --- Automation -----------------------------------------------------

/// Each step of an automation plan finished. Multiple emit sites
/// (success, error, retry) target the same listener.
pub const AUTOMATION_STEP_COMPLETE: &str = "automation_step_complete";

#[cfg(test)]
mod tests {
    use super::*;

    /// All event names follow snake_case. The convention check is the
    /// whole point of this module — mixing hyphens and underscores at
    /// runtime makes events silently miss listeners.
    #[test]
    fn all_event_names_are_snake_case() {
        let all = [
            CONFIG_UPDATED,
            EXTENSIONS_CHANGED,
            UPDATE_AVAILABLE,
            SHOW_FLOATING_BANNER,
            MESSAGE_CHUNK,
            MESSAGE_COMPLETE,
            MESSAGE_ERROR,
            TOOL_CALL_UPDATE,
            COMPACTION_STATUS,
            SESSION_ACTIVITY,
            PERMISSION_DISMISSED,
            INLINE_ASSIST_SHOW,
            INLINE_ASSIST_ERROR,
            SHOW_SESSIONS,
            CLIPBOARD_HISTORY_MODE,
            VOICE_MODE,
            HOTKEY_REGISTRATION_FAILED,
            AGENT_DISCONNECTED,
            AUTOMATION_STEP_COMPLETE,
        ];
        for name in all {
            assert!(
                !name.contains('-'),
                "event {:?} contains a hyphen — events use snake_case",
                name
            );
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "event {:?} must be lowercase a-z plus underscores",
                name
            );
        }
    }
}
