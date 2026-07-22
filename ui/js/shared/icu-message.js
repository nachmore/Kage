// Minimal ICU MessageFormat subset — simple substitution, `plural`
// (CLDR categories via Intl.PluralRules + `=N` exact arms + `#`), and
// `select`. Hand-rolled rather than pulling in ~30KB of
// `intl-messageformat`; the supported subset is small and stable.
//
// Single source of truth for BOTH the host windows (via
// `shared/i18n.js` → formatMessage/t) and the extension sandbox
// runtime (`extension-sandbox/runtime.js` imports it, and the sandbox
// host inlines this file into the iframe srcdoc — see
// RUNTIME_SOURCE_PATHS in extension-sandbox-host.js). Because this
// file is concatenated into a classic script for the sandbox, it must
// stay dependency-free: no imports, single-line export syntax only.

/**
 * Expand an ICU template. `opts.escape` (default false): when true,
 * every interpolated variable is HTML-escaped before substitution —
 * the template itself is never escaped.
 */
export function formatIcu(template, vars, locale, opts) {
    if (typeof template !== 'string' || !template.includes('{')) return template || '';
    return _expand(template, vars || {}, locale || 'en', !!opts?.escape);
}

function _expand(template, vars, locale, escape) {
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
        out += _expandPlaceholder(inner, vars, locale, escape);
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

function _expandPlaceholder(inner, vars, locale, escape) {
    // inner is what was between the outer braces, e.g.:
    //   "name"                                 → simple sub
    //   "count, plural, one {1 chat} other {# chats}"
    //   "role, select, admin {Admin} other {User}"
    const parts = _splitTopLevel(inner, ',');
    if (parts.length === 1) {
        // Simple substitution.
        const name = inner.trim();
        const v = vars[name];
        if (v === undefined || v === null) return `{${name}}`;
        return escape ? _escapeHtml(v) : String(v);
    }
    const [varName, kind, ...rest] = parts.map((s) => s.trim());
    const body = rest.join(', ').trim();
    if (kind === 'plural') {
        return _expandPlural(varName, body, vars, locale, escape);
    }
    if (kind === 'select') {
        return _expandSelect(varName, body, vars, locale, escape);
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

// Per-locale Intl.PluralRules cache. Constructing an Intl formatter is
// comparatively expensive and t() runs hot during streaming renders.
const _pluralRulesCache = new Map();

function _pluralRulesFor(locale) {
    let pr = _pluralRulesCache.get(locale);
    if (!pr) {
        pr = new Intl.PluralRules(locale);
        _pluralRulesCache.set(locale, pr);
    }
    return pr;
}

function _expandPlural(varName, body, vars, locale, escape) {
    const count = vars[varName];
    const arms = _parseArms(body);
    // Exact-match `=N` arms win over CLDR categories.
    const exactKey = `=${count}`;
    let arm = arms.get(exactKey);
    if (arm === undefined) {
        let category = 'other';
        try {
            category = _pluralRulesFor(locale).select(Number(count));
        } catch {
            // Bad locale or non-numeric count — fall through to `other`.
        }
        arm = arms.get(category) || arms.get('other') || '';
    }
    // Inside the arm, `#` expands to the count and nested `{name}` substitutes.
    // The count itself is numeric, so escaping it is a no-op — but we go
    // through the same path for consistency.
    const countStr = escape ? _escapeHtml(count) : String(count);
    return _expand(arm.replaceAll('#', countStr), vars, locale, escape);
}

function _expandSelect(varName, body, vars, locale, escape) {
    const value = String(vars[varName] ?? '');
    const arms = _parseArms(body);
    const arm = arms.get(value) || arms.get('other') || '';
    return _expand(arm, vars, locale, escape);
}

function _escapeHtml(value) {
    return String(value)
        .replaceAll('&', '&amp;')
        .replaceAll('<', '&lt;')
        .replaceAll('>', '&gt;')
        .replaceAll('"', '&quot;')
        .replaceAll("'", '&#39;');
}
