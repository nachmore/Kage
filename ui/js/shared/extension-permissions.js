/**
 * Extension permission system — single source of truth.
 *
 * Every extension runs in an iframe sandbox. Each Tauri IPC command the
 * extension might call is mapped to exactly one capability (or `null` =
 * never callable from an extension). The sandbox host consults this
 * table for every `invoke` request arriving over the bridge and rejects
 * anything the extension's manifest didn't declare.
 *
 * This is the authoritative enforcement point — the extension runs in a
 * sandboxed iframe with a null origin, so it has no `window.__TAURI__`
 * and no way to bypass the bridge.
 */

/**
 * Command → capability map.
 *
 * - A string means "this command requires that capability".
 * - `null` means "never callable from an extension" (always blocked).
 * - Commands missing from this map are treated as blocked by default (fail closed).
 */
export const COMMAND_CAPABILITIES = Object.freeze({
    // --- storage: the extension's own sandboxed data + config ---------------
    get_config: 'storage',                   // read-only access; scrubbed in host
    get_extension_config: 'storage',
    save_extension_config: 'storage',
    save_extension_data: 'storage',
    load_extension_data: 'storage',
    delete_extension_data: 'storage',
    save_frecency: 'storage',
    load_frecency: 'storage',

    // --- clipboard ---------------------------------------------------------
    read_clipboard: 'clipboard',
    get_clipboard_history: 'clipboard',
    paste_clipboard_item: 'clipboard',

    // --- shell: opening things externally ----------------------------------
    open_url: 'shell',
    open_path: 'shell',
    launch_app_by_name: 'shell',
    fetch_favicon: 'shell',
    fetch_link_metadata: 'shell',

    // --- filesystem: folder/file discovery ---------------------------------
    pick_folder: 'filesystem',
    scan_folder: 'filesystem',
    execute_folder_plan: 'filesystem',
    get_common_folders: 'filesystem',
    search_files: 'filesystem',
    resolve_directories: 'filesystem',

    // --- window: Kage window chrome ----------------------------------------
    resize_floating_window: 'window',
    set_floating_opacity: 'window',
    start_drag_window: 'window',
    save_window_position: 'window',
    save_chat_window_geometry: 'window',
    apply_chat_window_size: 'window',

    // --- windows: other apps' windows --------------------------------------
    list_open_windows: 'windows',
    focus_open_window: 'windows',
    get_process_name: 'windows',
    get_source_window: 'windows',
    get_app_icon: 'windows',

    // --- notifications -----------------------------------------------------
    notify_frontend_ready: 'notifications',

    // --- calendar ----------------------------------------------------------
    get_calendar_events: 'calendar',
    get_calendar_events_for_date: 'calendar',

    // --- session (chat sessions) -------------------------------------------
    list_sessions: 'session',
    load_session: 'session',
    get_current_session_id: 'session',
    get_floating_session_id: 'session',
    get_sessions_directory: 'session',

    // --- agent (LLM communication) -----------------------------------------
    send_message_streaming: 'agent',
    cancel_generation: 'agent',
    send_steering_message: 'agent',
    send_extension_tool_steering: 'agent',
    extension_tool_response: 'agent',
    open_chat_with_message: 'agent',
    get_available_models: 'agent',
    get_slash_commands: 'agent',

    // --- activity tracker --------------------------------------------------
    start_activity_tracker: 'activity',
    stop_activity_tracker: 'activity',
    get_activity_report: 'activity',
    is_activity_tracker_running: 'activity',

    // --- automation signals ------------------------------------------------
    emit_automation_signal: 'automation',
    list_automation_signals: 'automation',
    get_power_status: 'automation',

    // --- tts --------------------------------------------------------------
    pocket_tts_test: 'tts',
    pocket_tts_voices: 'tts',

    // --- explicitly forbidden (always blocked for extensions) --------------
    save_config: null,
    quit_app: null,
    restart_app: null,
    execute_system_command: null,
    install_extension_from_path: null,
    uninstall_extension: null,
    install_bundled_package: null,
    remove_tool_permission: null,
    update_tool_policy: null,
    send_permission_response: null,
    read_extension_file: null,
    open_devtools: null,
    dump_thread_info: null,
    app_log_write: null,
    app_log_get_entries: null,
    app_log_clear: null,
    app_log_get_dir: null,
    save_mcp_config: null,
    get_mcp_config: null,
    get_mcp_json_path: null,
    set_startup_enabled: null,
    set_computer_control_enabled: null,
    check_for_update: null,
    download_and_install_update: null,
    clear_update_flag: null,
    pocket_tts_install: null,
    pocket_tts_cancel_install: null,
    pocket_tts_start: null,
    pocket_tts_stop: null,
    execute_automation_plan: null,
    execute_macro: null,
    execute_shortcut: null,
    inline_assist_apply: null,
    send_inline_assist: null,
    show_inline_assist: null,
    complete_first_run: null,
    capture_hotkey_combo: null,
    cancel_hotkey_capture: null,
    try_register_hotkey: null,
    reconnect_acp: null,
    switch_acp_session: null,
    rename_session: null,
    delete_session: null,
    reveal_session_file: null,
});

/**
 * Full set of valid capability names, with user-facing metadata.
 * The icon/label/description fields drive the install-time permission
 * prompt and the settings UI badge row.
 */
export const CAPABILITIES = Object.freeze({
    storage:       { icon: '💾', label: 'Storage',       description: 'Read and write this extension\u2019s own sandboxed data and config.' },
    clipboard:     { icon: '📋', label: 'Clipboard',     description: 'Read your clipboard contents and history.' },
    shell:         { icon: '🌐', label: 'Shell',         description: 'Open URLs, file paths, and launch other apps on your behalf.' },
    filesystem:    { icon: '📂', label: 'Filesystem',    description: 'Scan folders and search files.' },
    window:        { icon: '🪟', label: 'Kage windows',  description: 'Resize, move, and adjust Kage\u2019s own windows.' },
    windows:       { icon: '🧿', label: 'Open windows',  description: 'List and focus other apps\u2019 windows.' },
    notifications: { icon: '🔔', label: 'Notifications', description: 'Show system notifications.' },
    calendar:      { icon: '📅', label: 'Calendar',      description: 'Read calendar events from your system.' },
    session:       { icon: '💬', label: 'Chat sessions', description: 'List and read chat sessions with the agent.' },
    agent:         { icon: '🤖', label: 'AI agent',      description: 'Send messages to the AI agent and cancel generations.' },
    activity:      { icon: '📊', label: 'Activity',      description: 'Start/stop the activity tracker and read app-usage statistics.' },
    automation:    { icon: '⚡', label: 'Automation',    description: 'Emit signals that can trigger automations.' },
    tts:           { icon: '🔈', label: 'Text-to-speech',description: 'Use text-to-speech voices.' },
});

/** Ordered list of capability names (stable order for UI display). */
export const KNOWN_CAPABILITIES = Object.freeze(Object.keys(CAPABILITIES));

/**
 * Normalize whatever the manifest provided into a deduped list of valid
 * capabilities. Unknown capabilities are dropped (with a warning).
 * @param {unknown} raw
 * @param {string} extensionId - for log messages
 * @returns {string[]}
 */
export function normalizePermissions(raw, extensionId) {
    if (!Array.isArray(raw)) return [];
    const seen = new Set();
    const out = [];
    for (const entry of raw) {
        if (typeof entry !== 'string') continue;
        const cap = entry.trim().toLowerCase();
        if (!cap) continue;
        if (!(cap in CAPABILITIES)) {
            console.warn(`Extension '${extensionId}': unknown capability '${cap}' \u2014 ignored`);
            continue;
        }
        if (seen.has(cap)) continue;
        seen.add(cap);
        out.push(cap);
    }
    return out;
}

/**
 * Policy decision record — returned by {@link decideInvoke}.
 * @typedef {object} InvokeDecision
 * @property {boolean} allow
 * @property {string} [reason] - when !allow, human-readable rejection reason
 */

/**
 * Authoritative check: may an extension with capability set `held` call `command`?
 * Fails closed for unknown commands.
 *
 * @param {string} command
 * @param {Set<string>} held
 * @returns {InvokeDecision}
 */
export function decideInvoke(command, held) {
    if (typeof command !== 'string') {
        return { allow: false, reason: 'command name must be a string' };
    }
    if (!Object.prototype.hasOwnProperty.call(COMMAND_CAPABILITIES, command)) {
        return { allow: false, reason: `command '${command}' is not available to extensions` };
    }
    const required = COMMAND_CAPABILITIES[command];
    if (required === null) {
        return { allow: false, reason: `command '${command}' is never callable from an extension` };
    }
    if (!held.has(required)) {
        return {
            allow: false,
            reason: `missing capability '${required}' (required for '${command}'). Add it to 'permissions' in manifest.json and the user will be asked to grant it.`,
        };
    }
    return { allow: true };
}
