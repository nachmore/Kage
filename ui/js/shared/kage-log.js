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
 *
 * Callers can flip this at runtime via `setVerboseConsoleCapture(true|false)`
 * (wired to the "Log all messages" setting in About > Logging). The flag
 * is read fresh on every console call, so a toggle takes effect immediately
 * without tearing down the interception.
 */

const invoke = () => window.__TAURI__?.core?.invoke;

/**
 * Runtime flag: when true, `console.log` and `console.debug` are mirrored
 * to the app log. Kept in a module-level variable so callers can flip it
 * without reinstalling the interception.
 */
let _verboseConsoleCapture = false;

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
    debug(source, ...args) {
        _write('debug', source, args.map(_fmt).join(' '));
    },
    info(source, ...args) {
        _write('info', source, args.map(_fmt).join(' '));
    },
    warn(source, ...args) {
        _write('warn', source, args.map(_fmt).join(' '));
    },
    error(source, ...args) {
        _write('error', source, args.map(_fmt).join(' '));
    },
};

/**
 * Create a logger scoped to an extension. Source is auto-set to "ext:<id>".
 * Extensions are expected to use this sparingly — see the note above.
 */
export function createExtensionLogger(extensionId) {
    const src = `ext:${extensionId}`;
    return {
        debug(...args) {
            _write('debug', src, args.map(_fmt).join(' '));
        },
        info(...args) {
            _write('info', src, args.map(_fmt).join(' '));
        },
        warn(...args) {
            _write('warn', src, args.map(_fmt).join(' '));
        },
        error(...args) {
            _write('error', src, args.map(_fmt).join(' '));
        },
    };
}

/**
 * Intercept console methods and mirror them to the app log. Original console
 * methods still fire so browser DevTools remain useful.
 *
 * `warn` and `error` are always forwarded — they're the actionable signals.
 * `log` and `debug` are only forwarded when verbose capture is enabled
 * (see `setVerboseConsoleCapture`). The check is re-read on every call so
 * the setting takes effect live.
 *
 * @param {string} source - source tag for intercepted messages (e.g. "floating")
 * @param {object} [opts]
 * @param {boolean} [opts.verbose=false] - initial value of the verbose flag
 */
export function interceptConsole(source, opts = {}) {
    _verboseConsoleCapture = !!opts.verbose;
    installGlobalErrorCapture(source);

    const orig = {
        log: console.log.bind(console),
        warn: console.warn.bind(console),
        error: console.error.bind(console),
        debug: console.debug.bind(console),
    };

    console.warn = (...args) => {
        orig.warn(...args);
        _write('warn', source, args.map(_fmt).join(' '));
    };
    console.error = (...args) => {
        orig.error(...args);
        _write('error', source, args.map(_fmt).join(' '));
    };
    console.log = (...args) => {
        orig.log(...args);
        if (_verboseConsoleCapture) _write('info', source, args.map(_fmt).join(' '));
    };
    console.debug = (...args) => {
        orig.debug(...args);
        if (_verboseConsoleCapture) _write('debug', source, args.map(_fmt).join(' '));
    };
}

let _errorCaptureInstalled = false;

/**
 * Pipe uncaught exceptions and unhandled promise rejections to the app
 * log. Console interception alone misses these: a `ReferenceError`
 * thrown from an event handler (e.g. a missing import surfacing at
 * call time) never passes through `console.error` — it goes straight
 * to the WebView's internal console, which nothing reads in a release
 * build. These are exactly the errors that make a feature silently
 * dead, so they're the most important ones to land in app.jsonl.
 *
 * Installed automatically by `interceptConsole()`; windows with their
 * own console wiring (settings) can call it directly. Idempotent.
 *
 * @param {string} source - source tag (e.g. "floating")
 */
export function installGlobalErrorCapture(source) {
    if (_errorCaptureInstalled) return;
    _errorCaptureInstalled = true;

    window.addEventListener('error', (e) => {
        // Resource-load errors (img/script) surface here with no
        // `error` object; message them by target so they're traceable.
        if (!e.message && e.target && e.target !== window) {
            const tag = e.target.tagName?.toLowerCase() || 'resource';
            const url = e.target.src || e.target.href || '';
            _write('error', source, `uncaught: failed to load <${tag}> ${url}`);
            return;
        }
        const where = e.filename ? ` (${e.filename}:${e.lineno}:${e.colno})` : '';
        const stack = e.error?.stack ? `\n${e.error.stack}` : '';
        _write('error', source, `uncaught: ${e.message}${where}${stack}`);
    });

    window.addEventListener('unhandledrejection', (e) => {
        const reason = e.reason;
        const msg =
            reason instanceof Error ? `${reason.message}\n${reason.stack || ''}` : _fmt(reason);
        _write('error', source, `unhandledrejection: ${msg}`);
    });
}

/**
 * Flip verbose console capture on or off at runtime. When true, every
 * `console.log` and `console.debug` is mirrored to the backend app log;
 * when false (the default), only `warn` and `error` are mirrored.
 *
 * Intended to be called by the settings module when the user toggles the
 * "Log all messages" checkbox, and on startup once config is loaded.
 */
export function setVerboseConsoleCapture(enabled) {
    _verboseConsoleCapture = !!enabled;
}

/** Format a value for log output. */
function _fmt(v) {
    if (v === null || v === undefined) return String(v);
    if (v instanceof Error) return `${v.message}\n${v.stack || ''}`;
    if (typeof v === 'object') {
        try {
            return JSON.stringify(v);
        } catch {
            return String(v);
        }
    }
    return String(v);
}
