/**
 * The view surface of the sandbox runtime: `context.view`, the view
 * module's lifecycle, the theme `<style>`, and the input guards a view
 * window's document gets (docs/design/EXTENSION_VIEWS.md §8, §9, §11, §13).
 *
 * Self-contained (no imports): the host inlines this file into the srcdoc
 * runtime (see RUNTIME_SOURCE_PATHS in extension-sandbox-host.js), and the
 * host's view policy (`shared/extension-view-policy.js`) imports the theme
 * helpers from here, so both sides write the same `<style>`.
 *
 * Nothing here is a security boundary. The extension's own code runs in
 * this realm and can undo any of it. The host re-checks every request.
 */

// --- Theme ------------------------------------------------------------------

/** The Kage tokens a view gets, by name (§11). Nothing else is passed. */
export const VIEW_THEME_TOKENS = Object.freeze([
    'kage-bg',
    'kage-bg-elevated',
    'kage-bg-input',
    'kage-text',
    'kage-text-bright',
    'kage-text-muted',
    'kage-border',
    'kage-border-subtle',
    'kage-accent',
    'kage-accent-hover',
    'kage-accent-active',
    'kage-accent-light',
    'kage-accent-glow',
    'kage-link',
    'kage-success',
    'kage-warning',
    'kage-danger',
    'kage-error',
    'kage-font',
    'kage-font-mono',
]);

/** Longest token value, in characters. */
export const VIEW_TOKEN_MAX_LENGTH = 200;

/** Font size used when the theme has none, in CSS pixels. */
export const VIEW_DEFAULT_FONT_SIZE = 14;

const VIEW_FONT_TOKENS = Object.freeze(['kage-font', 'kage-font-mono']);
const VIEW_HEX_COLOR_RE = /^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;
// rgb()/rgba()/hsl()/hsla() with numbers, units and separators only: no
// nested function, quote, `;`, `}` or `\`.
const VIEW_FUNC_COLOR_RE = /^(?:rgba?|hsla?)\((?:[\d.\s,%/+-]|deg|rad|grad|turn|none|e)*\)$/i;
const VIEW_NAMED_COLOR_RE = /^[a-z]{3,32}$/i;
// Letters, digits, spaces, commas, quotes and hyphens.
const VIEW_FONT_RE = /^[\p{L}\p{N} ,"'-]+$/u;

/**
 * The safe form of a theme token's value, or null when it isn't safe to
 * write into a view's `<style>`. Surrounding whitespace is trimmed and
 * inner runs collapse to one space (computed custom properties keep the
 * line breaks of the source). Colours must be hex, `rgb[a]()`, `hsl[a]()`
 * or a named colour; fonts only letters, digits, spaces, commas, quotes
 * and hyphens. A theme extension's `url(…)`, a `;` or a `}` is refused:
 * theme extensions are untrusted too.
 *
 * @param {string} name - a {@link VIEW_THEME_TOKENS} entry
 * @param {unknown} value
 * @returns {string|null}
 */
export function cleanTokenValue(name, value) {
    if (typeof name !== 'string' || !VIEW_THEME_TOKENS.includes(name)) return null;
    if (typeof value !== 'string') return null;
    const v = value.trim().replace(/\s+/g, ' ');
    if (!v || v.length > VIEW_TOKEN_MAX_LENGTH) return null;
    if (VIEW_FONT_TOKENS.includes(name)) return VIEW_FONT_RE.test(v) ? v : null;
    if (VIEW_HEX_COLOR_RE.test(v) || VIEW_FUNC_COLOR_RE.test(v) || VIEW_NAMED_COLOR_RE.test(v)) {
        return v;
    }
    return null;
}

/** Whether `value` may be written as the token `name` (see {@link cleanTokenValue}). */
export function isSafeTokenValue(name, value) {
    return cleanTokenValue(name, value) !== null;
}

function viewFontSize(n) {
    const size = Number(n);
    if (!Number.isFinite(size)) return VIEW_DEFAULT_FONT_SIZE;
    return Math.min(72, Math.max(6, Math.round(size * 10) / 10));
}

/**
 * A frozen theme snapshot with only the allowed tokens and safe values:
 * `{ scheme: 'dark'|'light', fontSize, reducedMotion, tokens }`. Unknown
 * tokens and unsafe values are dropped. This is what reaches
 * `context.view.theme`.
 *
 * @param {unknown} theme
 */
export function sanitizeViewTheme(theme) {
    const t = theme && typeof theme === 'object' ? theme : {};
    const raw = t.tokens && typeof t.tokens === 'object' ? t.tokens : {};
    const tokens = {};
    for (const name of VIEW_THEME_TOKENS) {
        if (!Object.hasOwn(raw, name)) continue;
        const v = cleanTokenValue(name, raw[name]);
        if (v !== null) tokens[name] = v;
    }
    return Object.freeze({
        scheme: t.scheme === 'light' ? 'light' : 'dark',
        fontSize: viewFontSize(t.fontSize ?? VIEW_DEFAULT_FONT_SIZE),
        reducedMotion: t.reducedMotion === true,
        tokens: Object.freeze(tokens),
    });
}

/**
 * The CSS the view document's theme `<style>` holds:
 * `:root { --kage-bg: …; …; color-scheme: dark; font-size: 14px }`.
 * Re-sanitises, so the output is safe whatever it's given.
 *
 * @param {unknown} theme
 * @returns {string}
 */
export function themeStyleText(theme) {
    const clean = sanitizeViewTheme(theme);
    const decls = [];
    for (const name of VIEW_THEME_TOKENS) {
        if (Object.hasOwn(clean.tokens, name)) decls.push(`--${name}: ${clean.tokens[name]}`);
    }
    decls.push(`color-scheme: ${clean.scheme}`);
    decls.push(`font-size: ${clean.fontSize}px`);
    return `:root { ${decls.join('; ')} }`;
}

/** Id of the theme `<style>` in the view document. */
export const VIEW_THEME_STYLE_ID = 'kage-view-theme';

/** Id of the element the runtime mounts the view into. */
export const VIEW_ROOT_ID = 'kage-view-root';

// --- Requests to the host ---------------------------------------------------

/** `view-request` kinds the host knows (§8.6). */
export const VIEW_REQUEST_KINDS = Object.freeze([
    'set-title',
    'close',
    'open-view',
    'compose',
    'copy-text',
    'open-settings',
]);

// --- WebRTC -----------------------------------------------------------------

const VIEW_WEBRTC_GLOBALS = Object.freeze([
    'RTCPeerConnection',
    'webkitRTCPeerConnection',
    'RTCDataChannel',
]);

/**
 * Remove WebRTC from a view document before the view module loads (§8.4):
 * CSP doesn't cover it. Deletes the constructors from the global object
 * and pins them to `undefined`. It also hides `contentWindow` /
 * `contentDocument` of frame elements, because `frame-src 'none'` doesn't
 * stop an `about:blank` child frame, whose globals would bring fresh
 * copies. `window[0]` still reaches such a frame: this is defence in
 * depth (the extension's hidden instances have the network anyway).
 *
 * @param {Window} win
 */
export function removeWebRtc(win) {
    for (const name of VIEW_WEBRTC_GLOBALS) {
        try {
            delete win[name];
        } catch {}
        try {
            Object.defineProperty(win, name, {
                value: undefined,
                writable: false,
                configurable: false,
                enumerable: false,
            });
        } catch {}
    }
    const hide = (proto, names) => {
        if (!proto) return;
        for (const name of names) {
            try {
                Object.defineProperty(proto, name, {
                    get: () => null,
                    configurable: false,
                    enumerable: true,
                });
            } catch {}
        }
    };
    hide(win.HTMLIFrameElement?.prototype, ['contentWindow', 'contentDocument']);
    hide(win.HTMLFrameElement?.prototype, ['contentWindow', 'contentDocument']);
    hide(win.HTMLObjectElement?.prototype, ['contentWindow', 'contentDocument']);
    for (const ctor of [win.HTMLIFrameElement, win.HTMLObjectElement, win.HTMLEmbedElement]) {
        if (!ctor?.prototype) continue;
        try {
            Object.defineProperty(ctor.prototype, 'getSVGDocument', {
                value: () => null,
                writable: false,
                configurable: false,
            });
        } catch {}
    }
}

// --- Links and keys ---------------------------------------------------------

/**
 * The URL a link in a view may open, or null: absolute `http(s)` and
 * `mailto` only. Relative links would resolve against Kage's own origin.
 *
 * @param {unknown} raw - the element's `href` attribute
 * @returns {string|null}
 */
export function viewLinkTarget(raw) {
    if (typeof raw !== 'string') return null;
    let url;
    try {
        url = new URL(raw.trim());
    } catch {
        return null;
    }
    return url.protocol === 'http:' || url.protocol === 'https:' || url.protocol === 'mailto:'
        ? url.href
        : null;
}

/**
 * Whether a keydown is ⌘W / Ctrl+W (§13.2). The key's character decides
 * when it is a Latin letter, so AZERTY's Ctrl+Z (on the W key) is undo,
 * not close. Other layouts fall back to the physical W key.
 *
 * @param {KeyboardEvent} ev
 */
export function isViewCloseShortcut(ev) {
    if (!ev || ev.altKey || ev.shiftKey || ev.repeat) return false;
    if (!ev.metaKey && !ev.ctrlKey) return false;
    const key = typeof ev.key === 'string' ? ev.key : '';
    if (/^[a-z]$/i.test(key)) return key.toLowerCase() === 'w';
    return ev.code === 'KeyW';
}

// --- The surface ------------------------------------------------------------

/**
 * Build the view surface of one sandbox.
 *
 * @param {object} deps
 * @param {string} deps.viewId
 * @param {unknown} deps.theme - the host's theme snapshot
 * @param {boolean} deps.links - the extension holds `urls`, so links open
 * @param {(msg: object) => void} deps.post - post to the host (may throw)
 * @param {(command: string, args?: object) => Promise<unknown>} deps.invoke
 * @param {(level: string, ...args: unknown[]) => void} deps.log
 * @param {(msg: object) => Error} deps.toError - builds a rejection from a response
 * @param {Window} deps.win
 * @param {Document} deps.doc
 */
export function createViewSurface({ viewId, theme, links, post, invoke, log, toError, win, doc }) {
    let current = sanitizeViewTheme(theme);
    /** The view module's instance, once it loaded. */
    let instance = null;
    let mounted = false;
    let visible = true;
    const pending = new Map();
    let nextId = 1;

    function request(kind, fields) {
        return new Promise((resolve, reject) => {
            const id = nextId++;
            pending.set(id, { resolve, reject });
            try {
                post({ type: 'view-request', id, kind, ...fields });
            } catch (e) {
                pending.delete(id);
                reject(e instanceof Error ? e : new Error(String(e)));
            }
        });
    }

    const api = {};
    Object.defineProperties(api, {
        id: { value: viewId, enumerable: true },
        theme: { get: () => current, enumerable: true },
        setTitle: {
            value: (text) => request('set-title', { text: text === undefined ? null : text }),
            enumerable: true,
        },
        close: { value: () => request('close', {}), enumerable: true },
        open: {
            value: (view, params) =>
                request('open-view', { view, params: params === undefined ? null : params }),
            enumerable: true,
        },
        compose: { value: (text) => request('compose', { text }), enumerable: true },
        copyText: { value: (text) => request('copy-text', { text }), enumerable: true },
        openSettings: { value: () => request('open-settings', {}), enumerable: true },
    });
    Object.freeze(api);

    function call(name, ...args) {
        const fn = instance?.[name];
        if (typeof fn !== 'function') return undefined;
        return fn.apply(instance, args);
    }

    function styleElement() {
        let el = doc.getElementById(VIEW_THEME_STYLE_ID);
        if (!el) {
            el = doc.createElement('style');
            el.id = VIEW_THEME_STYLE_ID;
            doc.head.appendChild(el);
        }
        return el;
    }

    function onLinkClick(ev) {
        const target = ev.target;
        const el =
            target && typeof target.closest === 'function' ? target.closest('a, area') : null;
        if (!el) return;
        const raw =
            el.getAttribute('href') ??
            el.getAttributeNS?.('http://www.w3.org/1999/xlink', 'href') ??
            null;
        if (raw === null) return;
        // The extension controls this document and can fake clicks, so a
        // link never navigates or opens anything on its own (§8.8).
        ev.preventDefault();
        if (ev.type === 'auxclick' && ev.button !== 1) return;
        const url = viewLinkTarget(raw);
        if (!url) {
            log('warn', 'view link ignored: only absolute http(s) and mailto links open');
            return;
        }
        if (!links) {
            log('warn', "view link ignored: opening links needs the 'urls' capability");
            return;
        }
        Promise.resolve(invoke('open_url', { url })).catch((e) =>
            log('warn', `view link failed to open: ${e?.message || e}`)
        );
    }

    function onKeyDown(ev) {
        if (!isViewCloseShortcut(ev)) return;
        ev.preventDefault();
        ev.stopImmediatePropagation();
        request('close', {}).catch((e) => log('warn', `close failed: ${e?.message || e}`));
    }

    return {
        api,

        /** Install the link and ⌘W / Ctrl+W guards (capture phase, on `win`). */
        installGuards() {
            win.addEventListener('click', onLinkClick, true);
            win.addEventListener('auxclick', onLinkClick, true);
            win.addEventListener('keydown', onKeyDown, true);
        },

        /** Write the theme `<style>` and the view's CSS files. */
        applyStyles(css) {
            styleElement().textContent = themeStyleText(current);
            const list = Array.isArray(css) ? css : [];
            for (const text of list) {
                if (typeof text !== 'string' || !text) continue;
                const el = doc.createElement('style');
                el.dataset.kageViewCss = '';
                el.textContent = text;
                doc.head.appendChild(el);
            }
        },

        /** Hand over the loaded view module instance. */
        attach(view) {
            instance = view;
        },

        /** `viewMount`: create the root and call `mount(root, info)` once. */
        async mount(params) {
            if (!instance) throw new Error('the view module did not load');
            if (mounted) throw new Error('the view is already mounted');
            mounted = true;
            const root = doc.createElement('div');
            root.id = VIEW_ROOT_ID;
            root.style.cssText = 'position:fixed;inset:0;overflow:hidden;';
            doc.body.appendChild(root);
            // Keys reach the view without a click once the page focuses
            // the iframe (§13.2).
            try {
                win.focus();
            } catch {}
            await call('mount', root, {
                params: params?.params ?? null,
                source: typeof params?.source === 'string' ? params.source : 'launcher',
                reloaded: params?.reloaded === true,
            });
            return true;
        },

        /** `viewReopen`: the window was opened again with new params. */
        async reopen(params) {
            if (!mounted) return false;
            await call('onOpen', {
                params: params?.params ?? null,
                source: typeof params?.source === 'string' ? params.source : 'launcher',
            });
            return true;
        },

        /** `viewVisibility`: `onShow` / `onHide`, once per change. */
        async visibility(params) {
            const next = params?.visible === true;
            if (!mounted || next === visible) {
                visible = next;
                return false;
            }
            visible = next;
            await call(next ? 'onShow' : 'onHide');
            return true;
        },

        /** `viewTheme`: replace the snapshot, the `<style>`, then `onThemeChange`. */
        async setTheme(params) {
            current = sanitizeViewTheme(params?.theme);
            styleElement().textContent = themeStyleText(current);
            if (mounted) await call('onThemeChange', current);
            return true;
        },

        /** A `view-response` from the host. */
        handleResponse(msg) {
            const entry = pending.get(msg?.id);
            if (!entry) return;
            pending.delete(msg.id);
            if (msg.error !== undefined && msg.error !== null) entry.reject(toError(msg));
            else entry.resolve(msg.result);
        },
    };
}
