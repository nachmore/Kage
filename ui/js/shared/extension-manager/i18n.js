import { activeLanguage as hostLanguage, isRtl as hostIsRtl } from '../i18n.js';

function _stripCatalogMeta(catalog) {
    if (!catalog || typeof catalog !== 'object') return {};
    const out = {};
    for (const [k, v] of Object.entries(catalog)) {
        if (k.startsWith('_')) continue;
        out[k] = v;
    }
    return out;
}

/**
 * Resolve `__MSG_key__` tokens in the manifest's localizable fields against
 * the extension's catalog (with EN fallback). Returns a shallow clone so the
 * original manifest stays in wire form for places that hash / compare it.
 *
 * Chrome convention: any string-typed field that starts and ends with
 * `__MSG_` and `__` is a translation token. We only resolve `name` and
 * `description` for now — those are the user-visible ones; other manifest
 * fields are wire data.
 */
export function applyManifestI18n(manifest, catalog, fallback) {
    const out = { ...manifest };
    for (const field of ['name', 'description']) {
        const v = manifest[field];
        if (typeof v !== 'string' || !v.startsWith('__MSG_') || !v.endsWith('__')) continue;
        const key = v.slice(6, -2);
        const entry = catalog?.[key] || fallback?.[key];
        if (entry?.message) out[field] = entry.message;
    }
    return out;
}

/**
 * Resolve a single extension i18n key against a catalog (with EN fallback),
 * returning the localised message or `fallbackText` if the key is unknown.
 *
 * Unlike `applyManifestI18n` this takes a bare key (not a `__MSG_…__` token)
 * because the values come from a provider's getKeywords() — the extension
 * supplies keys directly, never raw user-facing text, so localisation isn't
 * something the author can forget. No ICU vars: keyword labels are static
 * noun phrases. Returns '' for a missing key with no fallback so callers can
 * skip empty descriptions.
 */
export function resolveExtensionMessage(key, catalog, fallback, fallbackText = '') {
    if (typeof key !== 'string' || !key) return fallbackText;
    const entry = catalog?.[key] || fallback?.[key];
    return entry?.message ?? fallbackText;
}

/**
 * Fetch an extension's `_locales/<lang>/messages.json` payload and pick
 * the right catalog for the host language (with EN fallback). Shared
 * between the manager's runtime sandbox boot and any caller outside the
 * manager — install prompt, settings window — that needs the same
 * catalog the live extensions see.
 *
 * Returns `{ catalog, fallback, language, rtl }`. Empty catalog/fallback
 * if the extension ships no `_locales/` or anything fails — the caller
 * is allowed to surface raw `__MSG_*__` tokens rather than block on the
 * locale read.
 */
export async function fetchExtensionLocaleViaInvoke(invoke, manifest) {
    try {
        const id = manifest?.id;
        if (!id) return { catalog: {}, fallback: {}, language: hostLanguage(), rtl: hostIsRtl() };
        const kind = manifest?.type || 'extension';
        return await resolveExtensionCatalog(async (code) => {
            try {
                const v = await invoke('read_extension_locale', {
                    extensionId: id,
                    kind,
                    language: code,
                });
                return v && typeof v === 'object' ? v : null;
            } catch {
                return null;
            }
        });
    } catch {
        return { catalog: {}, fallback: {}, language: hostLanguage(), rtl: hostIsRtl() };
    }
}

/**
 * One-shot helper for callers that need a localized manifest only —
 * e.g. the install-time permission prompt, which shows the extension's
 * name and description before the manager has loaded the extension.
 *
 * Returns the original manifest unchanged if anything fails.
 */
export async function localizeManifestForPrompt(invoke, manifest) {
    try {
        const i18n = await fetchExtensionLocaleViaInvoke(invoke, manifest);
        return applyManifestI18n(manifest, i18n.catalog, i18n.fallback);
    } catch {
        return manifest;
    }
}

/**
 * Walk the relative imports of `entrySources` (a `{ name → source }`
 * object), recursively pulling in every sibling module they reference
 * via `import './x.js'` / `import '../y/z.js'` etc. Returns a
 * `{ relPath → source }` map suitable for the sandbox's
 * `sharedSources` payload, or `undefined` if no relative imports are
 * found.
 *
 * Same behaviour as the manager's private `_fetchSharedSources`, but
 * exported as a free function so the settings window (which builds
 * its own per-extension sandbox in buildSandboxedSettingsModule and
 * doesn't go through the manager's _fetchProviderSources path) can
 * resolve relative imports the same way. Without this, an extension
 * whose settings.js imports a sibling like `./auth.js` (Spotify
 * does, for the OAuth helpers) fails to load with "Failed to resolve
 * module specifier" because the sandbox runtime has no blob URL
 * registered for the sibling.
 */
export async function fetchSharedSourcesViaInvoke(invoke, extensionId, entrySources) {
    const collected = new Map();
    const queue = [];

    const scan = (src) => {
        if (typeof src !== 'string') return;
        // Matches both `import X from './x.js'` and `import './x.js'`.
        const re = /\bimport\s+(?:[^'"]+?\s+from\s+)?['"](\.{1,2}\/[^'"]+?)['"]/g;
        let m;
        while ((m = re.exec(src)) !== null) {
            const rel = m[1];
            if (!collected.has(rel) && !queue.includes(rel)) queue.push(rel);
        }
    };

    for (const [, val] of Object.entries(entrySources || {})) {
        scan(val);
    }

    while (queue.length) {
        const rel = queue.shift();
        if (collected.has(rel)) continue;
        let text = null;
        try {
            text = await invoke('read_extension_file', {
                extensionId,
                kind: 'extension',
                filePath: rel.replace('./', ''),
            });
        } catch (e) {
            console.warn(`Failed to read extension file '${extensionId}/${rel}':`, e);
        }
        if (text == null) continue;
        collected.set(rel, text);
        scan(text);
    }

    if (collected.size === 0) return undefined;
    const out = {};
    for (const [k, v] of collected) out[k] = v;
    return out;
}

export async function resolveExtensionCatalog(fetchByCode) {
    const want = [hostLanguage(), 'en'];
    let catalog = {};
    let fallback = {};
    // Try to land the active language first; fall through to the
    // region-stripped form if needed.
    const tried = new Set();
    for (const code of want) {
        if (tried.has(code)) continue;
        tried.add(code);
        const c = await fetchByCode(code);
        if (c && Object.keys(c).length) {
            catalog = _stripCatalogMeta(c);
            break;
        }
        if (code.includes('-')) {
            const stem = code.split('-')[0];
            if (!tried.has(stem)) {
                tried.add(stem);
                const c2 = await fetchByCode(stem);
                if (c2 && Object.keys(c2).length) {
                    catalog = _stripCatalogMeta(c2);
                    break;
                }
            }
        }
    }
    const en = await fetchByCode('en');
    if (en) fallback = _stripCatalogMeta(en);
    return {
        catalog,
        fallback,
        language: hostLanguage(),
        rtl: hostIsRtl(),
    };
}
