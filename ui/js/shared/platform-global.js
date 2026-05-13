/**
 * Platform helpers for non-module windows (settings, store, welcome).
 *
 * ES-module windows (floating, chat) use shortcuts.js instead, which
 * exports parallel helpers with identical behavior. Keep both in sync —
 * same precedent as `shared/theme-global.js` / `shared/theme.js`.
 *
 * Sets window.kagePlatform with the public API:
 *   kagePlatform.isMac()
 *   kagePlatform.cmdOrCtrlPressed(event)
 *   kagePlatform.platformKeyLabel(label)
 */
(function () {
    let _isMacCached = null;
    function isMac() {
        if (_isMacCached === null) {
            _isMacCached =
                typeof navigator !== 'undefined' &&
                typeof navigator.platform === 'string' &&
                navigator.platform.startsWith('Mac');
        }
        return _isMacCached;
    }

    // Windows / Linux detection mirrors the Rust-side `cfg(target_os = ...)`
    // gating so the UI can hide OS-specific settings panes. Derived from
    // navigator.platform (and userAgentData on Chromium) — good enough for
    // WebView2/WKWebView, which is what Tauri ships. Linux is the default
    // fallthrough so unrecognised platforms behave like Linux (the most
    // feature-limited target today).
    let _isWindowsCached = null;
    function isWindows() {
        if (_isWindowsCached === null) {
            const plat =
                typeof navigator !== 'undefined' && typeof navigator.platform === 'string'
                    ? navigator.platform
                    : '';
            _isWindowsCached = plat.startsWith('Win');
        }
        return _isWindowsCached;
    }

    let _isLinuxCached = null;
    function isLinux() {
        if (_isLinuxCached === null) {
            _isLinuxCached = !isMac() && !isWindows();
        }
        return _isLinuxCached;
    }

    function cmdOrCtrlPressed(e) {
        // Mac: both Ctrl and ⌘ work (⌘ is idiomatic; label uses ⌘ via
        // platformKeyLabel). Windows: Ctrl only — Win+key combos are OS-
        // intercepted and shouldn't hijack user bindings that leak through.
        // Linux: Ctrl only — Super+key is typically a WM/launcher binding.
        return isMac() ? e.ctrlKey || e.metaKey : e.ctrlKey;
    }

    function platformKeyLabel(label) {
        if (!isMac()) return label;
        return label
            .split('+')
            .map(function (part) {
                switch (part.trim()) {
                    case 'Ctrl':
                    case 'Cmd':
                    case 'Super':
                    case 'Meta':
                        return '\u2318'; // ⌘
                    case 'Shift':
                        return '\u21E7'; // ⇧
                    case 'Alt':
                    case 'Option':
                        return '\u2325'; // ⌥
                    case 'Enter':
                    case 'Return':
                        return '\u23CE'; // ⏎
                    case 'Backspace':
                        return '\u232B'; // ⌫
                    case 'Escape':
                    case 'Esc':
                        return '\u238B'; // ⎋
                    case 'Tab':
                        return '\u21E5'; // ⇥
                    default:
                        return part;
                }
            })
            .join('');
    }

    window.kagePlatform = {
        isMac: isMac,
        isWindows: isWindows,
        isLinux: isLinux,
        cmdOrCtrlPressed: cmdOrCtrlPressed,
        platformKeyLabel: platformKeyLabel,
    };
})();
