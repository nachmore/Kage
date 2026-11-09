/**
 * Host-side rules for extension views (docs/design/EXTENSION_VIEWS.md):
 * the manifest's `contributes.views` (§4), `open_view` effects and their
 * params (§6.2), titles, theme tokens (§11), the view iframe's CSP and
 * srcdoc (§8.3, §8.4), and the small limits of view requests (§8.6).
 *
 * `normalizeViews` and `validateViewParams` mirror
 * `src/extensions/views.rs`. A fixture both test suites read
 * (`tests/fixtures/extension-views-policy.json`) keeps them equal.
 */

import { copyTextError } from './settings-schema.js';
import {
    cleanTokenValue,
    isSafeTokenValue,
    sanitizeViewTheme,
    themeStyleText,
    VIEW_REQUEST_KINDS,
    VIEW_THEME_STYLE_ID,
    VIEW_THEME_TOKENS,
} from '../extension-sandbox/runtime/view-api.js';

export {
    cleanTokenValue,
    isSafeTokenValue,
    sanitizeViewTheme,
    themeStyleText,
    VIEW_REQUEST_KINDS,
    VIEW_THEME_TOKENS,
};

// --- Limits (same values as src/extensions/views.rs) ------------------------

export const MAX_VIEWS = 4;
export const MAX_VIEW_ID_LEN = 32;
export const MAX_TITLE_CHARS = 60;
export const MAX_ICON_CHARS = 8;
export const MAX_CSS_FILES = 8;
export const MAX_CSS_FILE_BYTES = 256 * 1024;
export const MAX_CSS_TOTAL_BYTES = 1024 * 1024;
export const DEFAULT_SIZE = Object.freeze({ width: 1100, height: 760 });
export const DEFAULT_SIZE_MIN = Object.freeze({ width: 480, height: 360 });
export const DEFAULT_SIZE_MAX = Object.freeze({ width: 3840, height: 2160 });
export const DEFAULT_MIN_SIZE = Object.freeze({ width: 480, height: 360 });
export const MIN_SIZE_FLOOR = Object.freeze({ width: 320, height: 240 });
export const MAX_PARAMS_BYTES = 4096;
export const MAX_PARAMS_DEPTH = 4;

/** Longest subtitle `context.view.setTitle` may set (§8.6). */
export const MAX_SUBTITLE_CHARS = 80;
/** Longest text `context.view.compose` may put in the launcher (§16.1). */
export const MAX_COMPOSE_CHARS = 2000;
/** Longest text `context.view.copyText` may copy (§13.3). */
export const MAX_COPY_CHARS = 8192;

/** What opened a view (§9.1); same list as `ext_views::VIEW_SOURCES`. */
export const VIEW_SOURCES = Object.freeze([
    'launcher',
    'chat',
    'formatter',
    'widget',
    'toolbar',
    'settings',
    'view',
]);

/** `source` checked against {@link VIEW_SOURCES}; anything else is `launcher`. */
export function normalizeViewSource(source) {
    return typeof source === 'string' && VIEW_SOURCES.includes(source) ? source : 'launcher';
}

// --- Characters -------------------------------------------------------------

// Rust's `char::is_control` is the Cc category; `char::is_whitespace` and
// `str::trim` use White_Space. JS's own `trim` differs (it strips U+FEFF).
const CONTROL_RE = /\p{Cc}/u;
const CONTROL_G = /\p{Cc}/gu;
const EDGE_WS_G = /^\p{White_Space}+|\p{White_Space}+$/gu;
const WS_RE = /\p{White_Space}/u;

/**
 * Bidi embedding, override and isolate characters (U+202A–U+202E,
 * U+2066–U+2069), as `extensions::views::is_bidi_override`.
 */
function isBidiOverride(ch) {
    const cp = ch.codePointAt(0);
    return (cp >= 0x202a && cp <= 0x202e) || (cp >= 0x2066 && cp <= 0x2069);
}

function stripBidi(s) {
    return Array.from(s)
        .filter((ch) => !isBidiOverride(ch))
        .join('');
}

function trimWs(s) {
    return s.replace(EDGE_WS_G, '');
}

function codePoints(s) {
    return Array.from(s);
}

function isMsgToken(s) {
    return s.length > '__MSG___'.length && s.startsWith('__MSG_') && s.endsWith('__');
}

// --- Manifest ---------------------------------------------------------------

const VIEW_ID_RE = /^[a-z][a-z0-9-]{0,31}$/;

/** Whether `id` is a valid view id: `^[a-z][a-z0-9-]{0,31}$`. */
export function isValidViewId(id) {
    return typeof id === 'string' && VIEW_ID_RE.test(id);
}

/**
 * Whether `path` is a bundle-relative path to a file ending in `ext`: an
 * optional `./`, then `/`-separated segments that aren't empty, `.` or
 * `..`, and no `\`, `:`, `?`, `#`, `%` or control characters.
 */
export function isBundlePath(path, ext) {
    if (typeof path !== 'string' || typeof ext !== 'string') return false;
    const rest = path.startsWith('./') ? path.slice(2) : path;
    if (rest.length <= ext.length || !rest.endsWith(ext)) return false;
    if (CONTROL_RE.test(rest) || /[\\:?#%]/.test(rest)) return false;
    return rest.split('/').every((seg) => seg !== '' && seg !== '.' && seg !== '..');
}

function isPlainObject(v) {
    if (v === null || typeof v !== 'object' || Array.isArray(v)) return false;
    const proto = Object.getPrototypeOf(v);
    return proto === Object.prototype || proto === null;
}

function own(obj, key) {
    return Object.hasOwn(obj, key) ? obj[key] : undefined;
}

function cleanTitleText(s) {
    return trimWs(stripBidi(String(s).replace(CONTROL_G, '')));
}

function viewTitle(raw, name) {
    let title = cleanTitleText(typeof raw === 'string' ? raw : '');
    if (!title) title = cleanTitleText(typeof name === 'string' ? name : '');
    if (isMsgToken(title)) return title;
    return codePoints(title).slice(0, MAX_TITLE_CHARS).join('');
}

function viewIcon(raw, fallback) {
    const icon = trimWs(typeof raw === 'string' ? raw : '');
    const chars = codePoints(icon);
    const ok =
        chars.length >= 1 &&
        chars.length <= MAX_ICON_CHARS &&
        chars.every(
            (c) =>
                c.codePointAt(0) >= 0x80 &&
                !WS_RE.test(c) &&
                !CONTROL_RE.test(c) &&
                !isBidiOverride(c)
        );
    return ok ? icon : typeof fallback === 'string' ? fallback : '';
}

function viewCss(raw, view, context, warn) {
    if (raw === undefined || raw === null) return [];
    if (!Array.isArray(raw)) {
        warn(`Extension '${context}': view '${view}' css must be an array — ignored`);
        return [];
    }
    const out = [];
    for (const entry of raw) {
        if (out.length === MAX_CSS_FILES) {
            warn(
                `Extension '${context}': view '${view}' lists more than ${MAX_CSS_FILES} css files; the rest are ignored`
            );
            break;
        }
        if (typeof entry === 'string' && isBundlePath(entry, '.css')) {
            if (!out.includes(entry)) out.push(entry);
        } else {
            warn(
                `Extension '${context}': view '${view}' css entry ${JSON.stringify(entry)} isn't a bundle-relative .css path — ignored`
            );
        }
    }
    return out;
}

function readSize(raw, def, min, max) {
    const dim = (key, d, lo, hi) => {
        const v = isPlainObject(raw) ? own(raw, key) : undefined;
        const n =
            typeof v === 'number' && Number.isFinite(v)
                ? Math.min(Math.max(Math.round(v), 0), 0xffffffff)
                : d;
        return Math.min(Math.max(n, lo), Math.max(hi, lo));
    };
    return {
        width: dim('width', def.width, min.width, max.width),
        height: dim('height', def.height, min.height, max.height),
    };
}

function normalizeView(entry, name, icon, context, warn) {
    if (!isPlainObject(entry)) return { error: 'not an object' };
    const id = own(entry, 'id');
    if (!isValidViewId(id)) return { error: 'id must match ^[a-z][a-z0-9-]{0,31}$' };
    const module = own(entry, 'module');
    if (typeof module !== 'string' || !isBundlePath(module, '.js')) {
        return { error: 'module must be a bundle-relative .js path' };
    }
    const defaultSize = readSize(
        own(entry, 'defaultSize'),
        DEFAULT_SIZE,
        DEFAULT_SIZE_MIN,
        DEFAULT_SIZE_MAX
    );
    const minSize = readSize(own(entry, 'minSize'), DEFAULT_MIN_SIZE, MIN_SIZE_FLOOR, defaultSize);
    return {
        decl: {
            id,
            title: viewTitle(own(entry, 'title'), name),
            icon: viewIcon(own(entry, 'icon'), icon),
            module,
            css: viewCss(own(entry, 'css'), id, context, warn),
            defaultSize,
            minSize,
        },
    };
}

/**
 * Read a manifest's raw `contributes.views` (mirror of
 * `extensions::views::normalize_views`). `name` and `icon` are the
 * extension's, used when an entry's title or icon is unusable. A bad entry
 * is dropped with a warning; a bad `views` value gives `[]`.
 *
 * @param {unknown} raw
 * @param {string} name
 * @param {string} icon
 * @param {string} context - names the extension in warnings
 * @param {{warn?: ((msg: string) => void) | null}} [options] - `warn: null` is silent
 * @returns {Array<{id: string, title: string, icon: string, module: string, css: string[],
 *   defaultSize: {width: number, height: number}, minSize: {width: number, height: number}}>}
 */
export function normalizeViews(raw, name, icon, context, { warn = console.warn } = {}) {
    const say = typeof warn === 'function' ? warn : () => {};
    if (raw === undefined || raw === null) return [];
    if (!Array.isArray(raw)) {
        say(`Extension '${context}': contributes.views must be an array — ignored`);
        return [];
    }
    if (raw.length > MAX_VIEWS) {
        say(
            `Extension '${context}': contributes.views has more than ${MAX_VIEWS} entries; the rest are ignored`
        );
    }
    const out = [];
    raw.slice(0, MAX_VIEWS).forEach((entry, i) => {
        const { decl, error } = normalizeView(entry, name, icon, context, say);
        if (error) {
            say(`Extension '${context}': view ${i} is invalid (${error}) — ignored`);
        } else if (out.some((v) => v.id === decl.id)) {
            say(
                `Extension '${context}': view ${i} repeats the id ${JSON.stringify(decl.id)} — ignored`
            );
        } else {
            out.push(decl);
        }
    });
    return out;
}

/**
 * Display title of a view: control and bidi-override characters removed,
 * trimmed, at most `max` code points. Use it on a title after its
 * `__MSG_*__` token is resolved, and (with {@link MAX_SUBTITLE_CHARS}) on
 * a `setTitle` subtitle.
 *
 * @param {unknown} text
 * @param {number} [max]
 * @returns {string} empty when nothing is left
 */
export function sanitizeViewTitle(text, max = MAX_TITLE_CHARS) {
    if (typeof text !== 'string') return '';
    const limit = Number.isSafeInteger(max) && max > 0 ? max : MAX_TITLE_CHARS;
    return codePoints(cleanTitleText(text)).slice(0, limit).join('');
}

// --- open_view --------------------------------------------------------------

function refusal(kind, key, vars, reason) {
    return { ok: false, kind, key, vars, reason };
}

const textEncoder = new TextEncoder();

/** {@link paramsDepth}'s answer for a value JSON (or Rust) can't carry. */
const BAD_VALUE = -1;

function wellFormed(s) {
    return typeof s.isWellFormed === 'function' ? s.isWellFormed() : true;
}

/**
 * How deeply `v` nests objects and arrays (a scalar is 0, `{}` is 1), as
 * `extensions::views::depth`. `enclosing` counts the containers around
 * `v`. Stops past {@link MAX_PARAMS_DEPTH}, so a cycle can't loop.
 * {@link BAD_VALUE} for anything but plain objects, arrays, well-formed
 * strings, finite numbers, booleans and null.
 */
function paramsDepth(v, enclosing) {
    if (v === null || typeof v === 'boolean') return 0;
    if (typeof v === 'string') return wellFormed(v) ? 0 : BAD_VALUE;
    if (typeof v === 'number') return Number.isFinite(v) ? 0 : BAD_VALUE;
    let items;
    if (Array.isArray(v)) items = v;
    else if (isPlainObject(v)) {
        if (!Object.keys(v).every(wellFormed)) return BAD_VALUE;
        items = Object.values(v);
    } else return BAD_VALUE;
    if (enclosing + 1 > MAX_PARAMS_DEPTH) return MAX_PARAMS_DEPTH + 1;
    let deepest = 0;
    for (const item of items) {
        const d = paramsDepth(item, enclosing + 1);
        if (d === BAD_VALUE || d > MAX_PARAMS_DEPTH) return d;
        deepest = Math.max(deepest, d);
    }
    return 1 + deepest;
}

/**
 * Check `open_view` params (mirror of `extensions::views::validate_params`):
 * absent or null, or a plain JSON object at most {@link MAX_PARAMS_DEPTH}
 * deep and {@link MAX_PARAMS_BYTES} UTF-8 bytes as compact JSON. Values
 * must be plain objects, arrays, strings, finite numbers, booleans or null.
 *
 * @param {unknown} params
 * @returns {null | {ok: false, kind: string, key: string, vars: object, reason: string}}
 *   null when the params are fine
 */
export function validateViewParams(params) {
    if (params === undefined || params === null) return null;
    const invalid = (reason) =>
        refusal('invalid_input', 'errors.ext_view.params_invalid', { reason }, reason);
    if (!isPlainObject(params)) return invalid('must be a JSON object');
    const depth = paramsDepth(params, 0);
    if (depth === BAD_VALUE) {
        return invalid('only objects, arrays, strings, finite numbers, booleans and null');
    }
    if (depth > MAX_PARAMS_DEPTH)
        return invalid(`nested more than ${MAX_PARAMS_DEPTH} levels deep`);
    let json;
    try {
        json = JSON.stringify(params);
    } catch (e) {
        return invalid(String(e?.message || e));
    }
    const size = textEncoder.encode(json).length;
    if (size > MAX_PARAMS_BYTES) {
        return refusal(
            'invalid_input',
            'errors.ext_view.params_too_large',
            { size: String(size), limit: String(MAX_PARAMS_BYTES) },
            `params are ${size} bytes; the limit is ${MAX_PARAMS_BYTES}`
        );
    }
    return null;
}

/**
 * The host-side checks before `ext_view_open` (§6.2), in order:
 *   1. the window offers the `views` host feature;
 *   2. the extension holds `views` in this window's grant;
 *   3. the manifest declares the view;
 *   4. `params` pass {@link validateViewParams}.
 *
 * Rust repeats 2–4 and more (§6.2). A refusal carries the `kind` and `key`
 * Rust would use, and an English `reason` for the log line
 * `[ext <id>] open_view refused: <reason>`.
 *
 * @param {unknown} effect - `{ type: 'open_view', view, params? }`
 * @param {object} ctx
 * @param {string[]} [ctx.hostFeatures] - the sandbox's (capability-filtered) host features
 * @param {Iterable<string>} [ctx.capabilities] - the extension's grant in this window
 * @param {unknown} [ctx.views] - the manifest's `contributes.views`, raw or normalised
 * @param {string} [ctx.extensionId] - for error parameters
 * @param {string} [ctx.extensionName] - for error parameters
 * @returns {{ok: true, view: string, params: object|null}
 *   | {ok: false, kind: string, key: string, vars: object, reason: string}}
 */
export function validateOpenViewEffect(effect, ctx = {}) {
    const extension = ctx.extensionName || ctx.extensionId || '';
    if (!isPlainObject(effect) || effect.type !== 'open_view') {
        return refusal(
            'invalid_input',
            'errors.ext_view.unknown_view',
            { extension, view: '' },
            'not an open_view effect'
        );
    }
    const features = Array.isArray(ctx.hostFeatures) ? ctx.hostFeatures : [];
    if (!features.includes('views')) {
        return refusal(
            'not_permitted',
            'errors.ext_view.opener_refused',
            {},
            "this window doesn't offer the views host feature"
        );
    }
    const caps = new Set(ctx.capabilities || []);
    if (!caps.has('views')) {
        return refusal(
            'not_permitted',
            'errors.ext_view.not_granted',
            { extension },
            "the extension doesn't hold the 'views' capability"
        );
    }
    const view = effect.view;
    const declared = normalizeViews(ctx.views, '', '', ctx.extensionId || '', { warn: null });
    if (!isValidViewId(view) || !declared.some((v) => v.id === view)) {
        return refusal(
            'invalid_input',
            'errors.ext_view.unknown_view',
            { extension, view: typeof view === 'string' ? view : '' },
            `the manifest declares no view ${JSON.stringify(typeof view === 'string' ? view : String(view))}`
        );
    }
    const params = effect.params === undefined ? null : effect.params;
    const bad = validateViewParams(params);
    if (bad) return bad;
    return { ok: true, view, params };
}

/**
 * Why `text` can't be composed or copied from a view, or null: the
 * settings copy rules (plain text, no control or bidi-override
 * characters) with the view's own length limit.
 *
 * @param {unknown} text
 * @param {number} max - {@link MAX_COMPOSE_CHARS} or {@link MAX_COPY_CHARS}
 */
export function viewTextError(text, max) {
    return copyTextError(text, { max });
}

// --- The view iframe --------------------------------------------------------

/** The view iframe's `sandbox` attribute: exactly this (§8.3). */
export const VIEW_IFRAME_SANDBOX = 'allow-scripts';

/** The view iframe's `allow` attribute: explicit denials (§8.3). */
export const VIEW_IFRAME_ALLOW = [
    'camera',
    'microphone',
    'geolocation',
    'clipboard-read',
    'clipboard-write',
    'display-capture',
    'fullscreen',
    'payment',
    'usb',
    'serial',
    'hid',
    'bluetooth',
    'midi',
    'screen-wake-lock',
    'web-share',
    'autoplay',
]
    .map((feature) => `${feature} 'none'`)
    .join('; ');

/**
 * The view document's Content Security Policy (§8.4). No network: only
 * `blob:` and `data:` for images, fonts, media and fetches; no frames,
 * objects, forms or `<base>`.
 */
export const VIEW_CSP = [
    "default-src 'none'",
    "script-src 'unsafe-inline' 'unsafe-eval' blob:",
    'worker-src blob:',
    'child-src blob:',
    "style-src 'unsafe-inline' blob:",
    'img-src blob: data:',
    'font-src blob: data:',
    'media-src blob: data:',
    'connect-src blob: data:',
    "frame-src 'none'",
    "object-src 'none'",
    "base-uri 'none'",
    "form-action 'none'",
].join('; ');

/**
 * Keep an inline `<script>` body from closing its element early:
 * `</script` becomes `<\/script`, which means the same in JS strings,
 * template literals and regular expressions.
 */
export function escapeInlineScript(source) {
    return String(source).replace(/<\/script/gi, '<\\/script');
}

const LANG_RE = /^[a-z]{2,3}(?:-[a-z0-9]{1,8})*$/i;

const VIEW_BASE_CSS =
    'html,body{margin:0;padding:0;width:100%;height:100%;overflow:hidden;' +
    'background:var(--kage-bg);color:var(--kage-text);font-family:var(--kage-font);}';

/**
 * The view iframe's srcdoc: the CSP `<meta>` first, then the charset, the
 * base and theme `<style>`s, and the runtime inlined as a classic script.
 * `<html lang dir>` come from Kage's language.
 *
 * @param {object} opts
 * @param {string} opts.runtimeSource - the assembled sandbox runtime
 * @param {string} [opts.lang]
 * @param {'ltr'|'rtl'} [opts.dir]
 * @param {unknown} [opts.theme]
 */
export function buildViewSrcdoc({ runtimeSource, lang, dir, theme }) {
    const safeLang = typeof lang === 'string' && LANG_RE.test(lang) ? lang : 'en';
    const safeDir = dir === 'rtl' ? 'rtl' : 'ltr';
    return (
        `<!DOCTYPE html><html lang="${safeLang}" dir="${safeDir}"><head>` +
        `<meta http-equiv="Content-Security-Policy" content="${VIEW_CSP}">` +
        '<meta charset="UTF-8">' +
        `<style>${VIEW_BASE_CSS}</style>` +
        `<style id="${VIEW_THEME_STYLE_ID}">${themeStyleText(theme)}</style>` +
        `</head><body><script>${escapeInlineScript(runtimeSource)}</script></body></html>`
    );
}

// --- Theme ------------------------------------------------------------------

/**
 * The view theme snapshot from a computed style (§11): the allowed tokens
 * read with `getPropertyValue('--<name>')`, plus scheme, font size and
 * reduced motion. Unsafe values are dropped.
 *
 * @param {{getPropertyValue: (name: string) => string}} style
 * @param {{scheme?: string, fontSize?: number, reducedMotion?: boolean}} [extra]
 */
export function readViewTheme(style, { scheme, fontSize, reducedMotion } = {}) {
    const tokens = {};
    for (const name of VIEW_THEME_TOKENS) {
        let value = '';
        try {
            value = style?.getPropertyValue?.(`--${name}`) ?? '';
        } catch {}
        if (value) tokens[name] = value;
    }
    return sanitizeViewTheme({ scheme, fontSize, reducedMotion, tokens });
}

/** Whether two theme snapshots are the same (so `viewTheme` can be skipped). */
export function viewThemesEqual(a, b) {
    return themeStyleText(a) === themeStyleText(b) && !!a?.reducedMotion === !!b?.reducedMotion;
}

// --- Rate limits ------------------------------------------------------------

/**
 * A sliding-window limiter: `take()` is true and records the request when
 * fewer than `max` were taken in the last `windowMs`; else false.
 *
 * @param {number} max
 * @param {number} windowMs
 * @param {() => number} [now]
 */
export function createRateWindow(max, windowMs, now = () => Date.now()) {
    const times = [];
    return {
        take() {
            const t = now();
            while (times.length && times[0] <= t - windowMs) times.shift();
            if (times.length >= max) return false;
            times.push(t);
            return true;
        },
    };
}

/** Per-kind rate limits of view requests: `[max, windowMs]` (§8.6, §13.3). */
export const VIEW_REQUEST_RATES = Object.freeze({
    'set-title': Object.freeze([10, 1000]),
    'open-view': Object.freeze([1, 1000]),
    compose: Object.freeze([2, 1000]),
    'copy-text': Object.freeze([2, 1000]),
    'open-settings': Object.freeze([1, 1000]),
});

// --- Storage change events --------------------------------------------------

const STORAGE_OPS = Object.freeze(['save', 'delete', 'bulk']);
const STORAGE_SOURCES = Object.freeze(['window', 'import', 'sync', 'uninstall']);
const DATA_KEY_RE = /^[A-Za-z0-9._-]{1,128}$/;
/** A batch never has more than this many entries (the coalescer's 64, §10.1). */
const MAX_STORAGE_CHANGES = 64;

/**
 * The `storage-changed` payload for a sandbox: `{ key, op, self, source }`
 * entries with a valid data key (or `*` for bulk), a known op and source.
 * Anything else, including the writer's window label, is dropped.
 *
 * @param {unknown} changes
 * @returns {Array<{key: string, op: string, self: boolean, source: string}>}
 */
export function sanitizeStorageChanges(changes) {
    if (!Array.isArray(changes)) return [];
    const out = [];
    for (const c of changes) {
        if (!c || typeof c !== 'object') continue;
        if (!STORAGE_OPS.includes(c.op)) continue;
        const key = c.op === 'bulk' ? '*' : c.key;
        if (
            key !== '*' &&
            !(typeof key === 'string' && DATA_KEY_RE.test(key) && !key.includes('..'))
        )
            continue;
        out.push({
            key,
            op: c.op,
            self: c.self === true,
            source: STORAGE_SOURCES.includes(c.source) ? c.source : 'window',
        });
    }
    if (out.length > MAX_STORAGE_CHANGES) {
        return [{ key: '*', op: 'bulk', self: false, source: out[0].source }];
    }
    return out;
}

/** True for any `view-request` kind the host handles. */
export function isViewRequestKind(kind) {
    return typeof kind === 'string' && VIEW_REQUEST_KINDS.includes(kind);
}
