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

let _catalog = null;
let _fallback = null;
let _meta = { language: 'en', rtl: false, machine_translated: false };
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
            _meta = { language: 'en', rtl: false, machine_translated: false };
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
 */
export function t(key, vars) {
    const template = _catalog?.[key]?.message || _fallback?.[key]?.message || key;
    return formatMessage(template, vars || {}, _meta.language);
}

/**
 * Lower-level: format an ICU template against the given vars. Exported for
 * callers (extensions) that want to format their own templates without
 * going through the host catalog.
 */
export function formatMessage(template, vars, locale) {
    if (typeof template !== 'string') return '';
    if (!template.includes('{')) return template;
    return _expand(template, vars, locale || _meta.language || 'en');
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
            const value = t(key, vars);
            if (mode === 'attribute') {
                el.setAttribute(attr, value);
            } else if (mode === 'innerHTML') {
                el.innerHTML = value;
            } else {
                el.textContent = value;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ICU MessageFormat subset.
// ---------------------------------------------------------------------------
//
// We hand-roll this rather than pull in ~30KB of `intl-messageformat`. The
// supported subset is small and stable (plural / select / simple sub).
// Tests cover Arabic / Polish plural categories so we know `Intl.PluralRules`
// integration is correct.

function _expand(template, vars, locale) {
    let out = '';
    let i = 0;
    while (i < template.length) {
        const ch = template[i];
        if (ch !== '{') {
            out += ch;
            i++;
            continue;
        }
        const close = _findMatchingBrace(template, i);
        if (close < 0) {
            // Malformed — bail and emit the rest verbatim.
            out += template.slice(i);
            break;
        }
        const inner = template.slice(i + 1, close);
        out += _expandPlaceholder(inner, vars, locale);
        i = close + 1;
    }
    return out;
}

function _findMatchingBrace(s, start) {
    let depth = 0;
    for (let i = start; i < s.length; i++) {
        if (s[i] === '{') depth++;
        else if (s[i] === '}') {
            depth--;
            if (depth === 0) return i;
        }
    }
    return -1;
}

function _expandPlaceholder(inner, vars, locale) {
    // inner is what was between the outer braces, e.g.:
    //   "name"                                 → simple sub
    //   "count, plural, one {1 chat} other {# chats}"
    //   "role, select, admin {Admin} other {User}"
    const firstComma = _splitTopLevel(inner, ',');
    if (firstComma.length === 1) {
        // Simple substitution.
        const name = inner.trim();
        const v = vars[name];
        return v === undefined || v === null ? `{${name}}` : String(v);
    }
    const [varName, kind, ...rest] = firstComma.map((s) => s.trim());
    const body = rest.join(', ').trim();
    if (kind === 'plural') {
        return _expandPlural(varName, body, vars, locale);
    }
    if (kind === 'select') {
        return _expandSelect(varName, body, vars, locale);
    }
    // Unknown formatter — preserve the source so the dev sees something is wrong.
    return `{${inner}}`;
}

/**
 * Split `s` on `sep` but only at depth 0 — braces nest.
 */
function _splitTopLevel(s, sep) {
    const out = [];
    let depth = 0;
    let buf = '';
    for (const ch of s) {
        if (ch === '{') depth++;
        else if (ch === '}') depth--;
        if (depth === 0 && ch === sep) {
            out.push(buf);
            buf = '';
        } else {
            buf += ch;
        }
    }
    out.push(buf);
    return out;
}

function _parseArms(body) {
    // body is like: `one {1 chat} other {# chats}` or `=0 {none} other {…}`
    // We walk `key {arm-body}` pairs; arm-body may contain nested braces.
    const arms = new Map();
    let i = 0;
    while (i < body.length) {
        // Skip whitespace.
        while (i < body.length && /\s/.test(body[i])) i++;
        if (i >= body.length) break;
        // Read key up to `{`.
        const keyEnd = body.indexOf('{', i);
        if (keyEnd < 0) break;
        const key = body.slice(i, keyEnd).trim();
        const close = _findMatchingBrace(body, keyEnd);
        if (close < 0) break;
        arms.set(key, body.slice(keyEnd + 1, close));
        i = close + 1;
    }
    return arms;
}

function _expandPlural(varName, body, vars, locale) {
    const count = vars[varName];
    const arms = _parseArms(body);
    // Exact-match `=N` arms win over CLDR categories.
    const exactKey = `=${count}`;
    let arm = arms.get(exactKey);
    if (arm === undefined) {
        let category = 'other';
        try {
            const pr = new Intl.PluralRules(locale);
            category = pr.select(Number(count));
        } catch {
            // Bad locale or non-numeric count — fall through to `other`.
        }
        arm = arms.get(category) || arms.get('other') || '';
    }
    // Inside the arm, `#` expands to the count and nested `{name}` substitutes.
    return _expand(arm.replaceAll('#', String(count)), vars, locale);
}

function _expandSelect(varName, body, vars, locale) {
    const value = String(vars[varName] ?? '');
    const arms = _parseArms(body);
    const arm = arms.get(value) || arms.get('other') || '';
    return _expand(arm, vars, locale);
}
