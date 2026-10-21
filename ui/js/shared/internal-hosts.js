/**
 * Is a host "internal" — something whose name shouldn't be sent to a
 * third-party service (e.g. a public favicon lookup)?
 *
 * Two signals, both generic:
 *   - Non-public names: single-label hosts (`http://wiki/`), IP literals,
 *     and reserved / conventionally private suffixes.
 *   - The URL rules of the user's Internal Sites profiles
 *     (`config.internal_sites`, see src/internal_sites.rs), synced from the
 *     config cache on first use and on every config change.
 *
 * Content rules can't be evaluated without fetching, so a host only they
 * would recognise isn't flagged here.
 */

import { getConfig, onConfigChange } from './config-cache.js';

const PRIVATE_SUFFIXES = [
    'local',
    'localhost',
    'internal',
    'intranet',
    'lan',
    'home.arpa',
    'corp',
    'test',
    'example',
    'invalid',
];

let _rules = [];
let _syncStarted = false;

function normalizeHost(host) {
    return String(host || '')
        .trim()
        .toLowerCase()
        .replace(/\.$/, '');
}

function suffixMatches(host, suffix) {
    const s = normalizeHost(suffix).replace(/^\*\./, '').replace(/^\./, '');
    if (!s) return false;
    return host === s || host.endsWith(`.${s}`);
}

/** Single-label names, IP literals and reserved suffixes. */
export function isNonPublicHost(host) {
    const h = normalizeHost(host);
    if (!h) return false;
    if (h.startsWith('[') || h.includes(':')) return true; // IPv6 literal
    if (/^\d{1,3}(\.\d{1,3}){3}$/.test(h)) return true; // IPv4 literal
    if (!h.includes('.')) return true;
    return PRIVATE_SUFFIXES.some((s) => suffixMatches(h, s));
}

/** Replace the active URL rules (enabled profiles only). */
export function setInternalSiteProfiles(profiles) {
    _rules = (Array.isArray(profiles) ? profiles : [])
        .filter((p) => p && p.enabled !== false)
        .flatMap((p) => (Array.isArray(p.url_rules) ? p.url_rules : []))
        .filter((r) => r && typeof r.pattern === 'string' && r.pattern.trim())
        .map((r) => {
            if (r.kind !== 'regex') return { kind: 'domain_suffix', pattern: r.pattern };
            try {
                return { kind: 'regex', re: new RegExp(r.pattern, 'i') };
            } catch {
                return null;
            }
        })
        .filter(Boolean);
}

function ensureSync() {
    if (_syncStarted) return;
    const invoke = globalThis.window?.__TAURI__?.core?.invoke;
    if (!invoke) return;
    _syncStarted = true;
    const load = () =>
        getConfig(invoke)
            .then((config) => setInternalSiteProfiles(config?.internal_sites))
            .catch(() => {});
    load();
    onConfigChange(load);
}

/** True when `host` matches a profile URL rule. */
export function matchesInternalSiteRule(host) {
    ensureSync();
    const h = normalizeHost(host);
    if (!h) return false;
    return _rules.some((r) =>
        r.kind === 'regex' ? r.re.test(`https://${h}/`) : suffixMatches(h, r.pattern)
    );
}

export function isInternalHost(host) {
    return isNonPublicHost(host) || matchesInternalSiteRule(host);
}
