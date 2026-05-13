/**
 * Thin client-side wrapper around the `telemetry_track` Tauri command.
 *
 * Every call goes through the Rust layer, which re-checks the user's
 * consent before touching the Aptabase plugin. That means calling
 * `trackEvent()` from a window that hasn't read config yet, or from a
 * codepath where consent has already been withdrawn, is still safe —
 * the event simply doesn't fire.
 *
 * # Rules for adding a new event
 *  1. Event names are snake_case, no PII in the name itself.
 *  2. Values passed as `props` must be strings or numbers (Aptabase
 *     rejects anything else). Never pass user-generated text.
 *  3. Prefer coarse, bucketed values (`"small"`, `"large"`) over raw
 *     character counts for anything that could fingerprint.
 *  4. Add new names to `KNOWN_EVENTS` so they survive grep review.
 *
 * # Fire-and-forget semantics
 * All call sites ignore the promise. Telemetry must never block user
 * actions — if the IPC hop fails (window closing, backend reloading),
 * we swallow the error and move on.
 */

/**
 * Bucket a raw message length into a coarse size label so we can see
 * usage shape (short prompts vs essays) without leaking anything
 * identifying. Raw counts would be too granular — two users with the
 * same 1,847-character prompt are probably the same person writing
 * the same message; buckets flatten that.
 *
 * Exported here so both floating/app.js and chat/app.js use the same
 * thresholds; drift between the two would produce split reports.
 */
export function messageLengthBucket(msg) {
    const n = (msg || '').length;
    if (n < 50) return 'xs';
    if (n < 200) return 'sm';
    if (n < 1000) return 'md';
    if (n < 5000) return 'lg';
    return 'xl';
}

/**
 * Allow-list of known events. Purely advisory — the Rust side will
 * still dispatch unknown names — but grepping this array is the
 * fastest way to audit everything that can ever reach Aptabase.
 */
export const KNOWN_EVENTS = Object.freeze([
    // Lifecycle (most are fired from Rust)
    'app_started',
    'app_installed',
    'app_upgraded',
    'app_daily_active',
    'app_exited',
    // Crash signal — fired from the Rust panic hook, never from JS.
    // Carries `message` (truncated panic string) and `location`
    // (file:line). See src/telemetry.rs::panic_hook.
    'panic',

    // First-run + consent
    'first_run_completed',
    'first_run_extensions_provisioned',
    'telemetry_enabled',
    'telemetry_disabled',

    // Update flow
    'update_installed',
    'update_check_failed',

    // Window surface — "what parts of the app actually get used"
    'floating_shown',
    'chat_opened',
    'settings_opened',
    'store_opened',
    'inline_assist_shown',
    'context_menu_shown',

    // Primary interactions
    'message_sent',
    'voice_input_used',
    'quick_action_used',
    'slash_command_used',
    'clipboard_history_used',
    'shortcut_triggered',
    'macro_executed',

    // Sessions
    'session_created',
    'session_resumed',

    // Configuration
    'model_changed',

    // Extensions
    'extension_installed',
    'extension_uninstalled',
    'extension_enabled_toggled',
    // Fired when a widget's circuit breaker trips after repeated render
    // failures or budget overruns. See ExtensionManager._noteWidgetFailure.
    // Payload: extension_id, widget_id, reason ('overlap'|'slow_absolute'|
    // 'slow_relative'|'throw').
    'extension_widget_disabled',
]);

/**
 * Fire an anonymous telemetry event.
 *
 * @param {string} event - one of KNOWN_EVENTS (not enforced here).
 * @param {Object=} props - optional, string/number values only.
 */
export function trackEvent(event, props) {
    try {
        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) return; // Window not yet bootstrapped
        // Normalize props: drop anything that isn't a string or number so
        // we never accidentally send objects/arrays (Aptabase rejects them
        // but we'd rather fail silently than send a malformed request).
        let cleaned = null;
        if (props && typeof props === 'object') {
            cleaned = {};
            for (const [k, v] of Object.entries(props)) {
                if (typeof v === 'string' || typeof v === 'number') {
                    cleaned[k] = v;
                }
            }
            if (Object.keys(cleaned).length === 0) cleaned = null;
        }
        // Fire-and-forget. We intentionally don't await — telemetry must
        // not be on any critical path.
        invoke('telemetry_track', { event, props: cleaned }).catch(() => {});
    } catch {
        // Swallow — telemetry is never worth surfacing to the user.
    }
}

/**
 * Convenience helper: fire an event only once per window session, keyed
 * by event name. Useful for "settings_opened" type events where we want
 * to count unique opens per launch rather than every focus change.
 */
const _fired = new Set();
export function trackEventOnce(event, props) {
    if (_fired.has(event)) return;
    _fired.add(event);
    trackEvent(event, props);
}
