/**
 * Kage App Log — structured logging for frontend code and extensions.
 *
 * Usage:
 *   import { kageLog } from './kage-log.js';
 *   kageLog.info('floating:app', 'Window opened');
 *   kageLog.error('chat:main', 'Connection failed', errorObj);
 *
 * For extensions, use createExtensionLogger(extensionId) which auto-prefixes
 * the source with "ext:<id>".
 *
 * # IPC volume
 *
 * Every `_write` call turns into a Tauri invoke, so this module is careful
 * about what it forwards. The explicit `kageLog.debug/info/warn/error` API
 * sends everything the caller asked for. The auto-intercepting
 * `interceptConsole()` used by the floating and chat windows defaults to
 * forwarding only `warn` and `error` — `console.log` and `console.debug`
 * are left in the DevTools console but not mirrored to the backend. This
 * keeps per-keystroke paths (extension search, theme reapply, etc.) from
 * flooding the backend log writer.
 */

const invoke = () => window.__TAURI__?.core?.invoke;

/**
 * Send a log entry to the Rust backend.
 */
function _write(level, source, msg) {
    const fn_ = invoke();
    if (fn_) {
        fn_('app_log_write', { level, source, msg }).catch(() => {});
    }
}

/**
 * Main log API — used by app code with explicit source tags. Always forwards
 * every call (the caller opted in explicitly).
 */
export const kageLog = {
    debug(source, ...args) { _write('debug', source, args.map(_fmt).join(' ')); },
    info(source, ...args)  { _write('info',  source, args.map(_fmt).join(' ')); },
    warn(source, ...args)  { _write('warn',  source, args.map(_fmt).join(' ')); },
    error(source, ...args) { _write('error', source, args.map(_fmt).join(' ')); },
};

/**
 * Create a logger scoped to an extension. Source is auto-set to "ext:<id>".
 * Extensions are expected to use this sparingly — see the note above.
 */
export function createExtensionLogger(extensionId) {
    const src = `ext:${extensionId}`;
    return {
        debug(...args) { _write('debug', src, args.map(_fmt).join(' ')); },
        info(...args)  { _write('info',  src, args.map(_fmt).join(' ')); },
        warn(...args)  { _write('warn',  src, args.map(_fmt).join(' ')); },
        error(...args) { _write('error', src, args.map(_fmt).join(' ')); },
    };
}

/**
 * Intercept console methods and mirror them to the app log. Original console
 * methods still fire so browser DevTools remain useful.
 *
 * By default only `warn` and `error` are forwarded — mirroring every
 * `console.log` call turns per-keystroke code paths (theme changes, search,
 * extension chatter) into a flood of IPC round-trips to Rust, which in turn
 * queues disk writes. Callers that want full capture (e.g. the settings UI
 * toggling a "verbose logging" preference) can pass `{ levels: 'all' }`.
 *
 * @param {string} source - source tag for intercepted messages (e.g. "floating")
 * @param {object} [opts]
 * @param {'warn-error'|'all'} [opts.levels='warn-error'] - which levels to mirror
 */
export function interceptConsole(source, opts = {}) {
    const levels = opts.levels === 'all' ? 'all' : 'warn-error';

    const orig = {
        log:   console.log.bind(console),
        warn:  console.warn.bind(console),
        error: console.error.bind(console),
        debug: console.debug.bind(console),
    };

    // warn/error are always forwarded — they're the actionable signals.
    console.warn = (...args) => {
        orig.warn(...args);
        _write('warn', source, args.map(_fmt).join(' '));
    };
    console.error = (...args) => {
        orig.error(...args);
        _write('error', source, args.map(_fmt).join(' '));
    };

    if (levels === 'all') {
        // Verbose mode: mirror info + debug too. Opt-in only.
        console.log = (...args) => {
            orig.log(...args);
            _write('info', source, args.map(_fmt).join(' '));
        };
        console.debug = (...args) => {
            orig.debug(...args);
            _write('debug', source, args.map(_fmt).join(' '));
        };
    }
    // In 'warn-error' mode, leave console.log and console.debug untouched so
    // DevTools-only chatter stays DevTools-only.
}

/** Format a value for log output. */
function _fmt(v) {
    if (v === null || v === undefined) return String(v);
    if (v instanceof Error) return `${v.message}\n${v.stack || ''}`;
    if (typeof v === 'object') {
        try { return JSON.stringify(v); } catch { return String(v); }
    }
    return String(v);
}
