/**
 * Frontend i18n — catalog loading, key lookup, ICU plural / select expansion.
 *
 * # Architecture
 *
 * The catalog is fetched once per window via `invoke('get_i18n_catalog')`,
 * which returns the active locale's `messages.json` plus the English fallback
 * (because some keys may be missing from non-English catalogs while
 * translations seed). The Rust side is the source of truth for which language
 * is active — we don't re-read `config.ui.language` from JS, we trust the
 * payload Rust hands us.
 *
 * The bundle is small (~50KB JSON, ~10KB after gzip) so we load it eagerly
 * during window startup. After that every `t(key)` call is a synchronous
 * Map lookup with no allocations beyond the formatted string.
 *
 * On `config_updated` we re-fetch the catalog so a language change in
 * Settings → Appearance reaches every window without a restart, then dispatch
 * a `kage:i18n-changed` CustomEvent on `document` so renderers can re-render.
 *
 * # ICU MessageFormat subset
 *
 * Supported:
 *   - `{name}` — simple substitution
 *   - `{count, plural, one {…} other {…}}` — CLDR plural categories
 *   - `{role, select, admin {…} user {…} other {…}}` — discrete switch
 *   - `#` inside a plural arm expands to the count value
 *
 * Not supported (escape hatch: emit two separate keys):
 *   - Nested formatters
 *   - Number / date formatting (`{n, number, percent}`)
 *   - selectordinal
 *
 * Plural rules use `Intl.PluralRules` so we get correct behaviour for every
 * language in our catalog set, including languages with `few` / `many` /
 * `two` categories (Russian, Polish, Arabic, Welsh, …).
 *
 * # RTL
 *
 * The catalog payload includes `rtl: bool`. We set `<html dir="rtl">` and
 * `document.body.classList.add('rtl')` when active, so CSS can target
 * `[dir=rtl]` selectors. Per-input direction detection (the existing
 * `rtl.js`) is orthogonal — it handles the case where a user types
 * Arabic into an LTR UI and we want the textarea to flip locally.
 *
 * # Static markup
 *
 * Elements with `data-i18n="key.name"` get their `textContent` localised
 * automatically by `applyStaticTranslations()`. For attributes there are
 * sister attrs:
 *   - `data-i18n-title` → `title`
 *   - `data-i18n-placeholder` → `placeholder`
 *   - `data-i18n-aria-label` → `aria-label`
 *   - `data-i18n-alt` → `alt`
 *
 * Substitutions for static markup come from a sibling `data-i18n-args`
 * attribute holding JSON: `data-i18n-args='{"name": "World"}'`.
 */

import { formatIcu } from './icu-message.js';

let _catalog = null;
let _fallback = null;
let _meta = { language: 'en', system_language: '', rtl: false, machine_translated: false };
let _loadPromise = null;

/**
 * Initialise the i18n module. Call once per window during startup, before
 * any `t()` lookup.
 */
export async function initI18n(invoke) {
    if (_loadPromise) return _loadPromise;
    _loadPromise = (async () => {
        try {
            const payload = await invoke('get_i18n_catalog');
            _applyPayload(payload);
        } catch (e) {
            console.error('[i18n] get_i18n_catalog failed; falling back to English', e);
            // Hard fallback: a degenerate catalog with the literal keys.
            // This keeps the UI from going blank if the IPC ever breaks.
            _catalog = {};
            _fallback = {};
            _meta = { language: 'en', system_language: '', rtl: false, machine_translated: false };
        }
        _applyDocumentDir();
    })();

    // Listen for `config_updated` so a Settings → Language change reflows
    // every window. We re-fetch the catalog (the active language may have
    // changed) and re-broadcast `kage:i18n-changed`.
    const listen = window?.__TAURI__?.event?.listen;
    if (typeof listen === 'function') {
        listen('config_updated', async () => {
            try {
                const payload = await invoke('get_i18n_catalog');
                const langChanged = payload?.language !== _meta.language;
                _applyPayload(payload);
                _applyDocumentDir();
                if (langChanged) {
                    document.dispatchEvent(new CustomEvent('kage:i18n-changed'));
                    applyStaticTranslations(document);
                }
            } catch {
                /* ignore — next manual reload will pick up the new catalog */
            }
        }).catch(() => {
            /* ignore listener install failure — we still have the initial load */
        });
    }

    return _loadPromise;
}

function _applyPayload(payload) {
    _catalog = payload?.catalog || {};
    _fallback = payload?.fallback || {};
    _meta = {
        language: payload?.language || 'en',
        system_language: payload?.system_language || '',
        rtl: !!payload?.rtl,
        machine_translated: !!payload?.machine_translated,
    };
}

function _applyDocumentDir() {
    const dir = _meta.rtl ? 'rtl' : 'ltr';
    document.documentElement.setAttribute('dir', dir);
    document.documentElement.setAttribute('lang', _meta.language);
    if (document.body) {
        document.body.classList.toggle('rtl', _meta.rtl);
    }
}

/**
 * The active language code. Useful for telemetry, conditional CSS, etc.
 */
export function activeLanguage() {
    return _meta.language;
}

/**
 * The OS-reported locale at app startup (e.g. "en-US", "ja-JP"), regardless
 * of any user override. Returns an empty string if `sys-locale` couldn't
 * detect anything. Surfaced for the settings picker so the "System default"
 * option can hint at the *system* language rather than the active one.
 */
export function systemLanguage() {
    return _meta.system_language;
}

/**
 * `true` if the active language is right-to-left.
 */
export function isRtl() {
    return _meta.rtl;
}

/**
 * `true` if the active catalog was machine-translated. Settings shows a
 * "please report errors" banner when this is true.
 */
export function isMachineTranslated() {
    return _meta.machine_translated;
}

/**
 * Look up a translation. Falls back to English then to the literal key if
 * the key is missing. The CI drift-check guarantees no key is missing in
 * EN, so the literal-key fallback only fires for typos in the calling code.
 *
 * `vars` can be a plain object (`{ name: 'World', count: 3 }`).
 *
 * Catalog strings are trusted (we author them). `t()` does NOT escape — it
 * is safe to drop the result into HTML, BUT any user-data variable you pass
 * in `vars` will land verbatim. Use `tHtml()` if the result is going to
 * `innerHTML` and the vars might contain `<` / `&` / quotes.
 */
export function t(key, vars) {
    const template = _catalog?.[key]?.message || _fallback?.[key]?.message || key;
    return formatMessage(template, vars || {}, _meta.language);
}

/**
 * Same as `t()`, but auto-escapes every variable so the result is safe to
 * drop into `innerHTML`. The catalog string itself is trusted (we author
 * it; `_html` keys may contain intentional markup like `<a>` / `<code>`),
 * so it is NOT re-escaped — only the interpolated `vars`.
 *
 * Use this for any `t()` call whose result is concatenated into a template
 * literal that becomes innerHTML. Use plain `t()` for `confirm()`,
 * `alert()`, `textContent`, or values stored as data (e.g. a friendly
 * connection name). Mixing the two would render `&amp;` literally in
 * non-HTML contexts.
 */
export function tHtml(key, vars) {
    const template = _catalog?.[key]?.message || _fallback?.[key]?.message || key;
    return formatMessage(template, vars || {}, _meta.language, { escape: true });
}

/**
 * Lower-level: format an ICU template against the given vars. Exported for
 * callers (extensions) that want to format their own templates without
 * going through the host catalog.
 *
 * `opts.escape` (default false): when true, every interpolated variable is
 * HTML-escaped before substitution. The template itself is never escaped.
 */
export function formatMessage(template, vars, locale, opts) {
    if (typeof template !== 'string') return '';
    return formatIcu(template, vars, locale || _meta.language || 'en', opts);
}

/**
 * Apply translations to elements with `data-i18n*` attributes inside the
 * given root (defaults to the entire document). Idempotent — safe to call
 * after every DOM mutation that introduces new translatable nodes.
 */
export function applyStaticTranslations(root) {
    const r = root || document;
    const selectors = [
        ['[data-i18n]', null, 'textContent'],
        ['[data-i18n-title]', 'title', 'attribute'],
        ['[data-i18n-placeholder]', 'placeholder', 'attribute'],
        ['[data-i18n-aria-label]', 'aria-label', 'attribute'],
        ['[data-i18n-alt]', 'alt', 'attribute'],
        ['[data-i18n-html]', null, 'innerHTML'],
    ];
    for (const [sel, attr, mode] of selectors) {
        for (const el of r.querySelectorAll(sel)) {
            const key = attr
                ? el.getAttribute(`data-i18n-${attr === 'aria-label' ? 'aria-label' : attr}`)
                : el.getAttribute(mode === 'innerHTML' ? 'data-i18n-html' : 'data-i18n');
            if (!key) continue;
            const argsJson = el.getAttribute('data-i18n-args');
            let vars;
            if (argsJson) {
                try {
                    vars = JSON.parse(argsJson);
                } catch {
                    vars = {};
                }
            }
            if (mode === 'attribute') {
                el.setAttribute(attr, t(key, vars));
            } else if (mode === 'innerHTML') {
                // tHtml escapes interpolated vars (the catalog template
                // itself is trusted and may carry intentional markup) —
                // plain t() here would make the first dynamic
                // data-i18n-args value on an _html key injectable.
                el.innerHTML = tHtml(key, vars);
            } else {
                el.textContent = t(key, vars);
            }
        }
    }
}

// ICU MessageFormat machinery lives in `./icu-message.js` — one parser
// shared with the extension sandbox runtime (which inlines that file
// into the iframe srcdoc). Tests cover Arabic / Polish plural
// categories so we know `Intl.PluralRules` integration is correct.
