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
    get_config: 'storage', // read-only access; scrubbed in host
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

    // --- network: outbound HTTP from the Rust runtime (CORS-bypassing) -----
    // These commands fire HTTP GETs from the Rust process, NOT the
    // sandboxed webview. They bypass CORS, send no `Origin` header
    // tied to the extension, and can reach sites that would refuse a
    // browser fetch (intranet endpoints, sites that block embedding,
    // etc.). Fundamentally different from `shell` (hand a URL to the
    // OS to open in the user's browser), so it gets its own capability.
    fetch_favicon: 'network',
    fetch_link_metadata: 'network',
    // Cache management commands stay settings-only — extensions shouldn't
    // wipe a shared cache or probe its size out of band.
    link_metadata_clear_cache: null,
    link_metadata_cache_stats: null,

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
    get_window_icons: 'windows',
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
    get_window_session: 'session',
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
    welcome_provision_extensions: null,
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

    // --- telemetry (user privacy) — controls Aptabase analytics. Never
    // callable from extensions; the install ID and opt-in state are
    // user-facing settings only the host chrome should touch.
    telemetry_track: null,
    get_telemetry_info: null,
    set_telemetry_enabled: null,
    reset_telemetry_install_id: null,

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
    trigger_welcome_banner: null,
    capture_hotkey_combo: null,
    cancel_hotkey_capture: null,
    try_register_hotkey: null,
    reconnect_acp: null,
    switch_acp_session: null,
    rename_session: null,
    delete_session: null,
    reveal_session_file: null,

    // --- internal status flags (Kage UI-only — no extension use case) ------
    check_connection: null,
    is_dev_mode: null,
    is_terminator_mode: null,
    is_first_run: null,
    was_just_updated: null,
    get_computer_control_enabled: null,
    get_startup_enabled: null,
    get_app_info: null,
    get_os_dark_mode: null,

    // --- agent backend introspection (Kage chrome, not extension business) -
    detect_agents: null,
    list_agent_presets: null,
    validate_agent_connection: null,
    probe_connection_version: null,
    check_npm_available: null,
    install_acp_wrapper: null,
    agent_session_providers: null,
    agent_list_sessions: null,
    agent_load_session: null,
    agent_check_session_updated: null,
    kage_desktop_delete_session: null,
    kage_desktop_open_folder: null,
    kage_desktop_workspaces: null,

    // --- Kage window chrome — opening/closing Kage's own windows ------------
    open_chat_window: null,
    open_settings_window: null,
    open_welcome_window: null,
    open_store_window: null,
    open_auto_steering_file: null,
    // --- Steering line editor + Ollama wizard — settings window only.
    // None of these should be reachable from extensions; the steering
    // docs are user-private and the Ollama wizard mutates the active
    // agent connection.
    read_steering_lines: null,
    write_steering_lines: null,
    import_steering_lines: null,
    ollama_probe: null,
    ollama_list_models: null,
    ollama_codex_spawn_command: null,
    // Per-app context rules (App Modes). Settings + the host send-
    // path lookup need this; extensions never should — a malicious
    // extension could otherwise probe what apps the user has rules
    // for, which leaks app fingerprints.
    match_context_rule: null,
    // --- Cross-device backup — settings window only. Reads + writes
    // every byte of user config; unconditionally off-limits to
    // extensions.
    export_config_default_filename: null,
    export_config_bundle: null,
    import_config_bundle: null,
    // Generic "write text to a user-picked path". Backs the chat
    // markdown export and any future save-as flows. Off-limits to
    // extensions because it can write anywhere (path comes from a
    // dialog the host owns); arbitrary write is not in any extension
    // capability.
    write_text_file: null,
    // Crash recovery. Surfaces a "Kage crashed last session" banner
    // and acknowledges it. Settings/about diagnostic surface — host
    // chrome only.
    get_recent_crash: null,
    dismiss_recent_crash: null,
    show_context_menu: null,
    test_floating_window: null,
    handle_floating_input: null,
    set_window_session: null,
    clear_window_session: null,
    open_new_chat_window: null,
    close_chat_window: null,
    list_chat_windows: null,
    touch_floating_activity: null,
    get_last_selection: null,

    // --- extension management itself (must never be re-entrant) -------------
    list_extensions: null,
    list_themes: null,
    list_command_packs: null,
    load_theme_colors: null,
    set_extension_enabled: null,
    commit_extension_install: null,
    remove_extension_grant: null,
    check_extension_updates: null,
    store_get_catalog: null,
    store_get_detail: null,
    store_install: null,
    save_store_url: null,

    // --- permission system internals ----------------------------------------
    get_permission_audit_log: null,
    get_permission_audit_log_path: null,
    clear_permission_audit_log: null,
    dismiss_pending_permission: null,
    has_pending_permission: null,
    check_extension_tool_permission: null,

    // --- agent / steering / screen ------------------------------------------
    // get_user_info exposes home dir + username — capability-gate later if
    // an extension actually needs it. get_screen_context returns the active
    // window + screenshot which is more sensitive than read_clipboard.
    get_user_info: null,
    get_screen_context: null,
    get_steering_content: null,
    get_auto_steering_path: null,
    execute_slash_command: null,
    get_slash_command_options: null,

    // --- updater / TTS install state ----------------------------------------
    fetch_changelog: null,
    get_update_urls: null,
    pocket_tts_check_install: null,

    // --- shortcut frecency (used by Kage's own search) ----------------------
    record_shortcut_usage: null,
    get_shortcut_history: null,
});

/**
 * Full set of valid capability names, with user-facing metadata.
 * The icon/label/description fields drive the install-time permission
 * prompt and the settings UI badge row.
 */
export const CAPABILITIES = Object.freeze({
    storage: {
        icon: '💾',
        label: 'Storage',
        description: 'Read and write this extension\u2019s own sandboxed data and config.',
    },
    clipboard: {
        icon: '📋',
        label: 'Clipboard',
        description: 'Read your clipboard contents and history.',
    },
    shell: {
        icon: '🌐',
        label: 'Shell',
        description: 'Open URLs, file paths, and launch other apps on your behalf.',
    },
    network: {
        icon: '📡',
        label: 'Network access',
        description:
            'Fetch URLs from outside your browser sandbox (e.g. link previews, favicons). Can reach sites your browser would refuse, including internal/intranet pages and sites that block cross-origin requests.',
    },
    filesystem: { icon: '📂', label: 'Filesystem', description: 'Scan folders and search files.' },
    window: {
        icon: '🪟',
        label: 'Kage windows',
        description: 'Resize, move, and adjust Kage\u2019s own windows.',
    },
    windows: {
        icon: '🧿',
        label: 'Open windows',
        description: 'List and focus other apps\u2019 windows.',
    },
    notifications: {
        icon: '🔔',
        label: 'Notifications',
        description: 'Show system notifications.',
    },
    calendar: {
        icon: '📅',
        label: 'Calendar',
        description: 'Read calendar events from your system.',
    },
    session: {
        icon: '💬',
        label: 'Chat sessions',
        description: 'List and read chat sessions with the agent.',
    },
    agent: {
        icon: '🤖',
        label: 'AI agent',
        description: 'Send messages to the AI agent and cancel generations.',
    },
    activity: {
        icon: '📊',
        label: 'Activity',
        description: 'Start/stop the activity tracker and read app-usage statistics.',
    },
    automation: {
        icon: '⚡',
        label: 'Automation',
        description: 'Emit signals that can trigger automations.',
    },
    tts: { icon: '🔈', label: 'Text-to-speech', description: 'Use text-to-speech voices.' },
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
    if (!Object.hasOwn(COMMAND_CAPABILITIES, command)) {
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
