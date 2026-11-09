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

    // Context menu (formerly context-menu-action). RENDER: backend → menu
    // window; ACTION: menu window → the window that asked (JS-only).
    CONTEXT_MENU_RENDER: 'context_menu_render',
    CONTEXT_MENU_ACTION: 'context_menu_action',

    // Hotkey-loop event names
    CLIPBOARD_HISTORY_MODE: 'clipboard_history_mode',
    VOICE_MODE: 'voice_mode',
    HOTKEY_REGISTRATION_FAILED: 'hotkey_registration_failed',

    // Automation
    AUTOMATION_STEP_COMPLETE: 'automation_step_complete',

    // Background agent calls (first-party windows only)
    AGENT_CALLS_ACTIVITY: 'agent_calls_activity',

    // Extension file text (the calling window only)
    FILE_TEXT_PROGRESS: 'file_text_progress',

    // Extension views: REOPEN and VISIBILITY go to one view window,
    // FOCUS_LOST and FLOATING_COMPOSE to the launcher, DATA_CHANGED to every
    // window that runs extension instances.
    EXT_VIEW_REOPEN: 'ext_view_reopen',
    EXT_VIEW_VISIBILITY: 'ext_view_visibility',
    EXT_VIEW_FOCUS_LOST: 'ext_view_focus_lost',
    FLOATING_COMPOSE: 'floating_compose',
    EXTENSION_DATA_CHANGED: 'extension_data_changed',

    // Folder sync: STATUS_CHANGED and CONFLICTS_CHANGED go to first-party
    // windows, APPLIED comes before config_updated, EXTENSION_SYNC_CHANGES
    // goes to the hosts of that extension.
    SYNC_STATUS_CHANGED: 'sync_status_changed',
    SYNC_CONFLICTS_CHANGED: 'sync_conflicts_changed',
    SYNC_APPLIED: 'sync_applied',
    EXTENSION_SYNC_CHANGES: 'extension_sync_changes',
    // Commands the user runs themselves (shell-run-view.js)
    USER_SHELL_OUTPUT: 'user_shell_output',
    // Commands an agent asks Kage to run outside its sandbox
    // (host-command-view.js)
    HOST_COMMAND_PROMPT: 'host_command_prompt',
    HOST_COMMAND_UPDATE: 'host_command_update',
};
