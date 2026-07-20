// Tauri event names — must agree with src/events.rs.
// Convention: snake_case. The Rust test
// `events::tests::all_event_names_are_snake_case` and the JS test
// `events.test.js > matches Rust` enforce both rules. When you
// add an event, add it on BOTH sides.

export const EVT = {
    // Config / app state
    CONFIG_UPDATED: 'config_updated',
    EXTENSIONS_CHANGED: 'extensions_changed',

    // Updater
    UPDATE_AVAILABLE: 'update_available',
    SHOW_FLOATING_BANNER: 'show_floating_banner',

    // Streaming / agent traffic
    MESSAGE_CHUNK: 'message_chunk',
    MESSAGE_COMPLETE: 'message_complete',
    MESSAGE_ERROR: 'message_error',
    TOOL_CALL_UPDATE: 'tool_call_update',
    COMPACTION_STATUS: 'compaction_status',
    SESSION_ACTIVITY: 'session_activity',
    AGENT_DISCONNECTED: 'agent_disconnected',

    // Permissions
    PERMISSION_DISMISSED: 'permission_dismissed',

    // Inline assist (formerly inline-assist-show / inline_assist_error)
    INLINE_ASSIST_SHOW: 'inline_assist_show',
    INLINE_ASSIST_ERROR: 'inline_assist_error',

    // Sessions (formerly show-sessions)
    SHOW_SESSIONS: 'show_sessions',

    // Context menu (formerly context-menu-action)
    CONTEXT_MENU_ACTION: 'context_menu_action',

    // Hotkey-loop event names
    CLIPBOARD_HISTORY_MODE: 'clipboard_history_mode',
    VOICE_MODE: 'voice_mode',
    HOTKEY_REGISTRATION_FAILED: 'hotkey_registration_failed',

    // Automation
    AUTOMATION_STEP_COMPLETE: 'automation_step_complete',
};
