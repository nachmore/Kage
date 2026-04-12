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
 * Main log API — used by app code with explicit source tags.
 */
export const kageLog = {
    debug(source, ...args) { _write('debug', source, args.map(_fmt).join(' ')); },
    info(source, ...args)  { _write('info',  source, args.map(_fmt).join(' ')); },
    warn(source, ...args)  { _write('warn',  source, args.map(_fmt).join(' ')); },
    error(source, ...args) { _write('error', source, args.map(_fmt).join(' ')); },
};

/**
 * Create a logger scoped to an extension. Source is auto-set to "ext:<id>".
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
 * Intercept console.log/warn/error and mirror to the app log.
 * Original console methods still fire (always, not just debug builds)
 * so browser DevTools remain useful.
 *
 * @param {string} source - source tag for intercepted messages (e.g. "floating", "chat")
 */
export function interceptConsole(source) {
    const orig = {
        log:   console.log.bind(console),
        warn:  console.warn.bind(console),
        error: console.error.bind(console),
        debug: console.debug.bind(console),
    };

    console.log = (...args) => {
        orig.log(...args);
        _write('info', source, args.map(_fmt).join(' '));
    };
    console.warn = (...args) => {
        orig.warn(...args);
        _write('warn', source, args.map(_fmt).join(' '));
    };
    console.error = (...args) => {
        orig.error(...args);
        _write('error', source, args.map(_fmt).join(' '));
    };
    console.debug = (...args) => {
        orig.debug(...args);
        _write('debug', source, args.map(_fmt).join(' '));
    };
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
