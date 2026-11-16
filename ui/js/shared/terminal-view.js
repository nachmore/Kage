/**
 * A small terminal emulator view (xterm.js) for a program Kage runs in a
 * pseudo-terminal the user types into — today only the agent sign-in
 * (agent-sign-in.js). Generic: it knows nothing about what runs; the
 * caller wires its callbacks to the backend's terminal commands.
 *
 * xterm.js is vendored (ui-vendor/setup.js → vendor/lib/xterm/) and
 * loaded lazily on first use, so no window pays for it at startup.
 *
 * Keys typed into the terminal stay in it: they don't bubble to the
 * window's own shortcuts (Esc hiding the floating window, ⌘, for Settings),
 * because the program needs them (Esc, arrows, Ctrl+C). Once input is off
 * (the program ended) the terminal lets go: keys reach the window again,
 * xterm no longer swallows Tab, and it gives up the keyboard focus.
 */

const XTERM_DIR = 'vendor/lib/xterm/';
const XTERM_SCRIPTS = ['xterm.js', 'addon-fit.js', 'addon-web-links.js'];
const RESIZE_DEBOUNCE_MS = 80;

let loadPromise = null;
let factoryForTests = null;

/** Let tests supply a fake terminal: `fn(options) → terminal`. */
export function _setTerminalFactoryForTests(fn) {
    factoryForTests = fn || null;
    loadPromise = null;
}

function xtermLoaded() {
    return typeof window.Terminal === 'function';
}

/**
 * Load xterm.js and its addons once (scripts in order, plus its CSS).
 * Resolves to whether the terminal can be used; a failed load can be
 * retried by calling again.
 */
export function ensureXterm() {
    if (factoryForTests || xtermLoaded()) return Promise.resolve(true);
    if (loadPromise) return loadPromise;
    loadPromise = new Promise((resolve) => {
        if (!document.querySelector('link[data-kage-xterm]')) {
            const link = document.createElement('link');
            link.rel = 'stylesheet';
            link.href = `${XTERM_DIR}xterm.css`;
            link.dataset.kageXterm = '1';
            document.head.appendChild(link);
        }
        const scripts = XTERM_SCRIPTS.map((name) => {
            const script = document.createElement('script');
            script.src = XTERM_DIR + name;
            // Dynamic scripts run in insertion order only when not async:
            // the addons need window.Terminal's bundle first.
            script.async = false;
            script.dataset.kageXterm = '1';
            return script;
        });
        let pending = scripts.length;
        let failed = false;
        for (const script of scripts) {
            script.onload = () => {
                pending -= 1;
                if (pending === 0 && !failed) resolve(xtermLoaded());
            };
            script.onerror = () => {
                if (failed) return;
                failed = true;
                for (const s of scripts) s.remove();
                loadPromise = null;
                resolve(false);
            };
            document.head.appendChild(script);
        }
    });
    return loadPromise;
}

/** The value of CSS custom property `name` as the container sees it. */
function cssVar(node, name, fallback) {
    try {
        const value = getComputedStyle(node).getPropertyValue(name).trim();
        return value || fallback;
    } catch {
        return fallback;
    }
}

/** xterm's theme from Kage's tokens (shared-kage-tokens.css). */
export function terminalTheme(node) {
    return {
        background: 'rgba(0, 0, 0, 0)',
        foreground: cssVar(node, '--kage-term-text', '#d4d4d4'),
        cursor: cssVar(node, '--kage-accent', '#8b5cf6'),
        cursorAccent: cssVar(node, '--kage-term-bg-strong', '#1e1e1e'),
        selectionBackground: cssVar(node, '--kage-term-mark-bg', 'rgba(139, 92, 246, 0.35)'),
        red: cssVar(node, '--kage-term-error', '#f87171'),
        green: cssVar(node, '--kage-term-success', '#4ade80'),
    };
}

function newTerminal(options) {
    if (factoryForTests) return factoryForTests(options);
    return new window.Terminal(options);
}

/**
 * Open a terminal in `container`.
 *   - `rows`: its height in lines (the width follows the container);
 *   - `onData(text)`: the user typed or pasted `text`;
 *   - `onResize(cols, rows)`: the view's size changed (debounced);
 *   - `onLink(uri)`: the user clicked a link the program printed;
 *   - `ariaLabel`: what screen readers call it.
 * Returns `{ write, focus, setInputEnabled, fit, size, dispose }`;
 * `write(text, onWritten)` calls `onWritten` once xterm has processed
 * `text` (or at once if the view is gone), which is when the caller
 * acknowledges it to the backend.
 * Call `ensureXterm()` first.
 */
export function createTerminalView(container, opts = {}) {
    const { rows = 12, onData, onResize, onLink, ariaLabel } = opts;
    container.dir = 'ltr';
    if (ariaLabel) container.setAttribute('aria-label', ariaLabel);
    let inputEnabled = true;
    let disposed = false;

    const term = newTerminal({
        rows,
        scrollback: 1000,
        cursorBlink: true,
        fontFamily: cssVar(container, '--kage-font-mono', 'ui-monospace, Menlo, monospace'),
        fontSize: 12,
        allowTransparency: true,
        allowProposedApi: false,
        // No window manipulation or size reports back to the program.
        windowOptions: {},
        linkHandler: {
            activate: (_event, uri) => onLink?.(uri),
            allowNonHttpProtocols: false,
        },
        theme: terminalTheme(container),
    });

    let fitAddon = null;
    if (!factoryForTests && window.FitAddon?.FitAddon) {
        fitAddon = new window.FitAddon.FitAddon();
        term.loadAddon(fitAddon);
    }
    if (!factoryForTests && window.WebLinksAddon?.WebLinksAddon) {
        term.loadAddon(new window.WebLinksAddon.WebLinksAddon((_event, uri) => onLink?.(uri)));
    }

    term.open(container);
    // false: xterm leaves the key alone (no preventDefault), so with input
    // off Tab moves focus and Esc reaches the window as usual.
    term.attachCustomKeyEventHandler?.(() => inputEnabled && !disposed);
    const dataSub = term.onData?.((text) => {
        if (inputEnabled && !disposed) onData?.(text);
    });

    // Keys meant for the program never reach the window's shortcuts.
    const stopKeys = (e) => {
        if (inputEnabled && !disposed) e.stopPropagation();
    };
    container.addEventListener('keydown', stopKeys);
    container.addEventListener('keyup', stopKeys);

    let lastSize = { cols: term.cols, rows: term.rows };
    let resizeTimer = null;
    function fit() {
        if (disposed) return;
        try {
            fitAddon?.fit();
        } catch {
            /* not laid out yet */
        }
        const now = { cols: term.cols, rows: term.rows };
        if (now.cols === lastSize.cols && now.rows === lastSize.rows) return;
        lastSize = now;
        clearTimeout(resizeTimer);
        resizeTimer = setTimeout(() => {
            if (!disposed) onResize?.(now.cols, now.rows);
        }, RESIZE_DEBOUNCE_MS);
    }
    fit();

    let resizeObserver = null;
    if (typeof ResizeObserver === 'function') {
        resizeObserver = new ResizeObserver(() => fit());
        resizeObserver.observe(container);
    }

    // Theme changes: dark/light toggles body classes; custom themes set
    // variables on <html>'s style.
    const applyTheme = () => {
        if (disposed) return;
        try {
            term.options.theme = terminalTheme(container);
        } catch {
            /* a terminal that's going away */
        }
    };
    const themeObserver = new MutationObserver(applyTheme);
    if (document.body)
        themeObserver.observe(document.body, { attributes: true, attributeFilter: ['class'] });
    themeObserver.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ['style', 'class'],
    });

    return {
        write(text, onWritten) {
            if (disposed || !text) {
                onWritten?.();
                return;
            }
            if (!onWritten) {
                term.write(text);
                return;
            }
            try {
                term.write(text, onWritten);
            } catch {
                onWritten();
            }
        },
        focus() {
            if (!disposed) term.focus();
        },
        setInputEnabled(enabled) {
            inputEnabled = !!enabled;
            try {
                term.options.disableStdin = !inputEnabled;
                if (!inputEnabled) term.options.cursorBlink = false;
            } catch {
                /* ignore */
            }
            // Nothing left to type into: don't keep the keyboard.
            if (!inputEnabled && container.contains(document.activeElement)) {
                try {
                    term.blur?.();
                } catch {
                    /* ignore */
                }
                if (container.contains(document.activeElement)) document.activeElement.blur?.();
            }
        },
        fit,
        size() {
            return { cols: term.cols, rows: term.rows };
        },
        dispose() {
            if (disposed) return;
            disposed = true;
            clearTimeout(resizeTimer);
            resizeObserver?.disconnect();
            themeObserver.disconnect();
            container.removeEventListener('keydown', stopKeys);
            container.removeEventListener('keyup', stopKeys);
            try {
                dataSub?.dispose?.();
                term.dispose();
            } catch {
                /* ignore */
            }
            container.textContent = '';
        },
    };
}
