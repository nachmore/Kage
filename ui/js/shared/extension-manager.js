/**
 * Extension Manager — discovers extensions and runs their search /
 * tool / trigger providers inside sandboxed iframes.
 *
 * Security model (see docs/SECURITY_MODEL.md):
 *   - Every extension's search/tool/trigger provider code runs inside a
 *     sandboxed iframe with no access to window.__TAURI__ or the parent
 *     DOM. All Tauri IPC goes through the ExtensionSandboxHost bridge,
 *     which enforces capability permissions authoritatively.
 *   - Extensions are granted capabilities at install time by the user.
 *     The manifest declares which capabilities the extension requests;
 *     the grant is stored in config and consulted here at load time.
 *   - Contribution points that still need host DOM access (settings
 *     modules, widgets, toolbar buttons, message formatters, custom
 *     renderResult) remain in the trusted path for now. The sandbox
 *     roll-out is tracked in docs/EXTENSIONS.md and will land in a
 *     follow-up.
 */

/**
 * Allow-list of vendor libraries an extension may declare in its
 * `sandboxVendor` manifest array. Keys are the names extensions use;
 * values are workspace-relative URLs that the host fetches.
 *
 * Extensions CAN'T name arbitrary paths — they can only opt into
 * libraries we've pre-approved. Add entries here with care: every
 * declared vendor lib is loaded into the sandbox iframe of any
 * extension that names it, and runs with the same (null-origin)
 * privileges as the extension itself.
 */
const SANDBOX_VENDOR_ALLOWLIST = {
    // mathjs — sets `window.math`. Used by the built-in math extension.
    math: 'vendor/lib/math.js',
};

// --- Widget refresh-budget caps ---------------------------------------
// A widget that misbehaves can degrade the entire host process: a slow
// `renderWidget()` ties up the sandbox RPC channel, a hard-failing one
// burns CPU on every interval, and an extension that re-mounts on every
// config change can multiply both. The constants below define the
// circuit breaker that contains the blast radius.

/** Hard floor on declared refresh interval. Anything below this is
 *  clamped up — protects against a typo that says `1` (interpreted as
 *  1ms) instead of `1000`. */
const WIDGET_MIN_INTERVAL_MS = 1_000;

/** Hard ceiling — 24h. A widget that wants to refresh less often than
 *  this should just refresh manually on action. */
const WIDGET_MAX_INTERVAL_MS = 24 * 3_600 * 1_000;

/** A render that runs longer than this is flagged as slow regardless of
 *  the declared interval. Even a 5-minute-cadence widget shouldn't be
 *  blocking the UI for 5 seconds per tick — that's a bug, not a budget
 *  question. The RPC layer already times out at 10s, so the practical
 *  ceiling is somewhere in (3s, 10s); 5s is comfortably inside both. */
const WIDGET_SLOW_RENDER_MS = 5_000;

/** A render counts as a "soft failure" if it took longer than
 *  `interval * SLOW_RENDER_RATIO`. We can't accumulate work past the
 *  next tick boundary, so a render that took 70%+ of its own interval
 *  is one bad scheduling beat away from overlapping with itself. */
const WIDGET_SLOW_RENDER_RATIO = 0.7;

/** Trip the breaker after N consecutive failures. Three is a balance
 *  between "transient blip ≠ disable" and "broken extension keeps
 *  burning CPU forever". */
const WIDGET_FAILURE_TRIP_THRESHOLD = 3;

import { normalizePermissions } from './extension-permissions.js';
import { ExtensionSandboxPool } from './extension-sandbox-host.js';
import { sanitizeExtensionHtml, findExtActions } from './extension-html-sanitizer.js';
import { activeLanguage as hostLanguage, isRtl as hostIsRtl, t, tHtml } from './i18n.js';

// --- Extension i18n helpers --------------------------------------------------
//
// Each extension can ship `_locales/<lang>/messages.json` files following the
// same shape as the host catalog. The host fetches the active locale's
// catalog plus the EN fallback at extension load time; both are passed to
// the sandbox runtime so its `context.i18n.t(key, vars)` proxy can render
// without a per-call IPC roundtrip. The catalog is read via the
// `read_extension_locale` Tauri command, which validates the extension id
// and stays inside the user install dir.

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
 * One-shot helper for callers outside the manager that need a localized
 * manifest — e.g. the install-time permission prompt, which shows the
 * extension's name and description before the manager has loaded the
 * extension. Reads `_locales/<lang>/messages.json` via
 * `read_extension_locale` and applies tokens to a copy of the manifest.
 *
 * Returns the original manifest unchanged if anything fails — the prompt
 * is allowed to fall back to displaying raw `__MSG_*__` tokens rather
 * than blocking the install on a locale read.
 */
export async function localizeManifestForPrompt(invoke, manifest) {
    try {
        const id = manifest?.id;
        if (!id) return manifest;
        const kind = manifest?.type || 'extension';
        const i18n = await _resolveExtensionCatalog(async (code) => {
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
        return applyManifestI18n(manifest, i18n.catalog, i18n.fallback);
    } catch {
        return manifest;
    }
}

async function _resolveExtensionCatalog(fetchByCode) {
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

export class ExtensionManager {
    /**
     * @param {Function} invoke - the raw Tauri invoke (the host, not the
     *   extension, will use it after permission checks).
     */
    constructor(invoke) {
        this.invoke = invoke;
        /** @type {Map<string, LoadedExtension>} */
        this.extensions = new Map();
        this._configCache = null;
        this._pool = new ExtensionSandboxPool(invoke);
        /** Synchronous snapshot of tool definitions; refreshed by getToolDefinitions(). */
        this._toolDefsCache = [];
    }

    /**
     * Discover and load all enabled extensions.
     */
    async initialize() {
        try {
            this._configCache = await this.invoke('get_config');
        } catch (e) {
            console.error('ExtensionManager: failed to load config', e);
            this._configCache = {};
        }

        try {
            const userExts = await this.invoke('list_extensions');
            for (const item of userExts) {
                if (!item.enabled) continue;
                try {
                    await this._loadExtension(item);
                } catch (e) {
                    console.warn(`Failed to load extension '${item.manifest.id}':`, e);
                }
            }
        } catch (e) {
            console.warn('Failed to list extensions:', e);
        }

        console.log(`ExtensionManager: ${this.extensions.size} extensions loaded`);

        // Prime caches so synchronous callers can read latest state
        // without incurring a round-trip per render.
        try {
            await this.getToolDefinitions();
        } catch (e) {
            console.warn('Tool-defs prime failed:', e);
        }
        try {
            await this._refreshToolbarButtons();
        } catch (e) {
            console.warn('Toolbar prime failed:', e);
        }

        // Mount widgets contributed by any loaded extensions.
        try {
            await this._mountAllWidgets();
        } catch (e) {
            console.warn('Widget mount failed:', e);
        }
    }

    async _loadExtension(item) {
        const id = item.manifest.id;
        const manifest = item.manifest;

        const states = this._configCache?.extension_states || {};
        if (states[id] === false) return;

        const capabilities = this._resolveGrantedCapabilities(manifest);
        const ext = {
            manifest,
            capabilities,
            sandbox: null,
        };

        const sources = await this._fetchProviderSources(id, manifest);
        const sharedSources = sources.sharedSources;
        delete sources.sharedSources;
        const vendorSources = await this._fetchVendorSources(manifest);
        const i18n = await this._fetchExtensionLocale(id, manifest);
        // Resolve __MSG_*__ tokens in the manifest's name/description so
        // search results, settings rows, and the store all show localised
        // labels. Mutates a copy so the original manifest stays the wire
        // form (used elsewhere for hashes / etc.).
        ext.localizedManifest = applyManifestI18n(manifest, i18n.catalog, i18n.fallback);
        if (this._hasSandboxedProvider(sources)) {
            try {
                ext.sandbox = await this._pool.load({
                    extensionId: id,
                    capabilities,
                    config: this._getExtensionConfig(id, manifest),
                    sources,
                    sharedSources,
                    vendorSources,
                    i18nCatalog: i18n.catalog,
                    i18nFallback: i18n.fallback,
                    i18nLanguage: i18n.language,
                    i18nRtl: i18n.rtl,
                });
            } catch (e) {
                console.warn(`Sandbox boot failed for extension '${id}':`, e);
            }
        }

        await this._loadExtensionCss(id, manifest);

        this.extensions.set(id, ext);
    }

    // --- Sources -----------------------------------------------------------

    _hasSandboxedProvider(sources) {
        return !!(
            sources.searchProvider ||
            sources.toolProvider ||
            sources.triggerProvider ||
            sources.toolbarProvider ||
            sources.messageFormatter ||
            (sources.widgets && Object.keys(sources.widgets).length > 0)
        );
    }

    async _fetchProviderSources(id, manifest) {
        const out = {};
        const c = manifest.contributes || {};
        if (c.searchProvider) out.searchProvider = await this._fetchText(id, c.searchProvider);
        if (c.toolProvider) out.toolProvider = await this._fetchText(id, c.toolProvider);
        if (c.triggerProvider) out.triggerProvider = await this._fetchText(id, c.triggerProvider);
        if (c.toolbarButtons) out.toolbarProvider = await this._fetchText(id, c.toolbarButtons);
        if (c.messageFormatters)
            out.messageFormatter = await this._fetchText(id, c.messageFormatters);
        if (Array.isArray(c.widgets) && c.widgets.length) {
            out.widgets = {};
            for (const w of c.widgets) {
                if (!w?.id || !w?.module) continue;
                out.widgets[w.id] = await this._fetchText(id, w.module);
            }
        }
        out.sharedSources = await this._fetchSharedSources(out, id);
        return out;
    }

    /**
     * Walk every fetched provider source looking for `import ... from './...'`
     * statements. Recursively fetch those files too so the sandbox can
     * wire them up as shared blob URLs (see runtime.js :
     * registerSharedModules).
     *
     * Note: this is a deliberately dumb regex-based discovery. It does
     * NOT handle dynamic `import()` expressions or conditional imports.
     * Extensions that need those should keep all their JS in a single
     * file or declare them explicitly later.
     *
     * @param {object} sources - the sources bag accumulated so far
     * @param {string} extensionId - extension id used for the read_extension_file IPC
     * @returns {Promise<object>} flat map of { "./rel/path.js": sourceText }
     */
    async _fetchSharedSources(sources, extensionId) {
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

        // Seed queue from the entry-point provider sources.
        for (const [kind, val] of Object.entries(sources)) {
            if (kind === 'widgets' && val && typeof val === 'object') {
                for (const s of Object.values(val)) scan(s);
            } else {
                scan(val);
            }
        }

        // Breadth-first resolve — shared modules can import each other.
        while (queue.length) {
            const rel = queue.shift();
            if (collected.has(rel)) continue;
            const text = await this._fetchText(extensionId, rel);
            if (text == null) continue;
            collected.set(rel, text);
            scan(text);
        }

        if (collected.size === 0) return undefined;
        const out = {};
        for (const [k, v] of collected) out[k] = v;
        return out;
    }

    /**
     * Fetch vendor libraries declared by the extension in
     * `manifest.sandboxVendor` (an array of allow-listed basenames).
     *
     * Vendor libs are non-ES-module UMD/IIFE bundles (like mathjs) that
     * set globals when run. The sandbox runtime injects them via a
     * `<script>` tag before loading provider code so providers can rely
     * on those globals.
     *
     * Only a small allow-list is accepted — we never load arbitrary
     * paths that the extension names, because that would be a path
     * traversal vector. Unknown names are dropped with a warning.
     *
     * @returns {Promise<Record<string,string> | undefined>}
     *   Map of allow-list name → source text, or undefined if none.
     */
    /**
     * Fetch an extension's `_locales/<lang>/messages.json` via the Tauri
     * command `read_extension_locale`, which path-validates the extension
     * id and stays inside the user install dir.
     */
    async _fetchExtensionLocale(id, manifest) {
        const kind = manifest?.type || 'extension';
        return _resolveExtensionCatalog(async (code) => {
            try {
                const v = await this.invoke('read_extension_locale', {
                    extensionId: id,
                    kind,
                    language: code,
                });
                return v && typeof v === 'object' ? v : null;
            } catch {
                return null;
            }
        });
    }

    async _fetchVendorSources(manifest) {
        const list = Array.isArray(manifest?.sandboxVendor) ? manifest.sandboxVendor : null;
        if (!list || list.length === 0) return undefined;
        const out = {};
        for (const name of list) {
            if (typeof name !== 'string') continue;
            const url = SANDBOX_VENDOR_ALLOWLIST[name];
            if (!url) {
                console.warn(
                    `Extension '${manifest.id}': unknown sandboxVendor '${name}', ignored`
                );
                continue;
            }
            try {
                const resp = await fetch(url);
                if (!resp.ok) {
                    console.warn(
                        `Failed to fetch vendor '${name}' from ${url}: HTTP ${resp.status}`
                    );
                    continue;
                }
                out[name] = await resp.text();
            } catch (e) {
                console.warn(`Failed to fetch vendor '${name}':`, e);
            }
        }
        return Object.keys(out).length ? out : undefined;
    }

    async _fetchText(id, relPath) {
        try {
            return await this.invoke('read_extension_file', {
                extensionId: id,
                kind: 'extension',
                filePath: relPath.replace('./', ''),
            });
        } catch (e) {
            console.warn(`Failed to read extension file '${id}/${relPath}':`, e);
            return null;
        }
    }

    // --- Capabilities ------------------------------------------------------

    /**
     * Compute the capabilities actually granted to this extension.
     *
     * Grant flow:
     *   1. Manifest declares requested capabilities in `permissions[]`.
     *   2. At install time the user approves that set (or uninstalls).
     *   3. We store the approved set in config under
     *      `extension_grants[<id>]` alongside the manifest version that
     *      was approved. If the manifest later requests more caps, we
     *      drop the extras until the user re-approves.
     *
     * @returns {string[]}
     */
    _resolveGrantedCapabilities(manifest) {
        const id = manifest.id;
        const requested = normalizePermissions(manifest.permissions, id);

        const grants = this._configCache?.extension_grants || {};
        const record = grants[id];
        if (!record) {
            // Extension without a grant record. This shouldn't happen if
            // install went through the permission prompt, but handle it
            // defensively: grant nothing and log loudly.
            console.warn(
                `Extension '${id}': no user grant recorded, running with no capabilities. ` +
                    `The extension may be broken until it is reinstalled.`
            );
            return [];
        }

        // Intersect: the user's grant is authoritative. If the extension
        // is updated to request new capabilities, we drop the extras
        // until the user approves them.
        const grantedSet = new Set(normalizePermissions(record.granted, id));
        return requested.filter((cap) => grantedSet.has(cap));
    }

    async _loadExtensionCss(id, manifest) {
        const cssFiles = manifest.contributes?.css;
        if (!Array.isArray(cssFiles) || cssFiles.length === 0) return;
        for (const cssPath of cssFiles) {
            if (document.querySelector(`style[data-ext-css="${id}"]`)) continue;
            try {
                const cssCode = await this.invoke('read_extension_file', {
                    extensionId: id,
                    kind: 'extension',
                    filePath: cssPath.replace('./', ''),
                });
                const style = document.createElement('style');
                style.dataset.extCss = id;
                style.textContent = cssCode;
                document.head.appendChild(style);
                console.log(`ExtensionManager: loaded CSS for '${id}'`);
            } catch (e) {
                console.warn(`Failed to load CSS for '${id}':`, e);
            }
        }
    }

    _getExtensionConfig(id, manifest) {
        const saved = this._configCache?.extensions?.[id];
        if (saved) return saved;
        const defaults = {};
        if (manifest.config) {
            for (const [key, schema] of Object.entries(manifest.config)) {
                defaults[key] = schema.default;
            }
        }
        return defaults;
    }

    _isEnabled(id) {
        const states = this._configCache?.extension_states || {};
        return states[id] !== false;
    }

    /**
     * Get the trigger keyword for an extension (if any).
     * Returns the lowercase trigger string, or null if no trigger (extension sees all queries).
     */
    _getExtensionTrigger(id, manifest) {
        const config = this._getExtensionConfig(id, manifest);
        const trigger = config?.trigger;
        if (typeof trigger === 'string' && trigger.trim()) {
            return trigger.trim().toLowerCase();
        }
        return null;
    }

    // --- Search dispatch ---------------------------------------------------

    /**
     * Synchronous match, fan out to all extensions. The sandbox RPC is
     * async, so the "sync" match() becomes async too — the caller can
     * await it or treat the promise as a stream-of-results batch.
     *
     * Returns a Promise that resolves to an array of SearchResult, each
     * stamped with `_extensionId`.
     */
    async matchAll(query) {
        if (query.trim().startsWith('>')) return []; // > prefix reserved for built-ins
        const lowerQuery = query.trim().toLowerCase();
        const promises = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.sandbox?.hasSearch) continue;
            if (!this._isEnabled(id)) continue;
            const trigger = this._getExtensionTrigger(id, ext.manifest);
            if (trigger && !lowerQuery.startsWith(trigger)) continue;
            promises.push(
                ext.sandbox
                    .call('match', { query })
                    .then((matches) => (matches || []).map((m) => ({ ...m, _extensionId: id })))
                    .catch((e) => {
                        console.warn(`match() in '${id}' failed:`, e);
                        return [];
                    })
            );
        }
        const results = await Promise.all(promises);
        return results.flat();
    }

    async matchAllAsync(query) {
        if (query.trim().startsWith('>')) return [];
        const lowerQuery = query.trim().toLowerCase();
        const promises = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.sandbox?.hasSearch) continue;
            if (!this._isEnabled(id)) continue;
            const trigger = this._getExtensionTrigger(id, ext.manifest);
            if (trigger && !lowerQuery.startsWith(trigger)) continue;
            promises.push(
                ext.sandbox
                    .call('matchAsync', { query })
                    .then((matches) => (matches || []).map((m) => ({ ...m, _extensionId: id })))
                    .catch((e) => {
                        console.warn(`matchAsync() in '${id}' failed:`, e);
                        return [];
                    })
            );
        }
        const results = await Promise.all(promises);
        return results.flat();
    }

    async executeResult(result) {
        const id = result?._extensionId;
        if (!id) return null;
        const ext = this.extensions.get(id);
        if (!ext?.sandbox?.hasSearch) return null;
        try {
            // Strip the host-only stamp before sending; the extension never needs it.
            const { _extensionId: _ignore, ...clean } = result;
            return await ext.sandbox.call('execute', { result: clean });
        } catch (e) {
            console.warn(`execute() in '${id}' failed:`, e);
            return null;
        }
    }

    /**
     * Custom render hook. Asks the sandbox for a custom HTML string;
     * host sanitizes and injects. Returns true if the extension handled
     * rendering, false to fall back to the default renderer.
     *
     * This must stay synchronous to match the existing call site, so we
     * keep a per-result cache warmed by `prefetchCustomRender()`. Use
     * that async method in the code path that produces results before
     * rendering — see search-unified.js.
     */
    renderResult(result, element) {
        const id = result?._extensionId;
        if (!id) return false;
        const ext = this.extensions.get(id);
        if (!ext?.sandbox?.hasSearch) return false;
        const cached = this._customRenderCache?.get(result.id);
        if (!cached) return false;
        // Result rows are structural — use rich sanitization so the
        // extension can lay out its own row (icon + label + buttons).
        const frag = sanitizeExtensionHtml(cached.html, 'rich');
        if (cached.className)
            element.classList.add(...cached.className.split(/\s+/).filter(Boolean));
        element.appendChild(frag);
        this._wireExtActionsFor(id, element);
        return true;
    }

    /**
     * Pre-warm the custom-render cache for a batch of results. Called
     * once per render pass before we start building suggestion DOM.
     *
     * Cache has a soft cap to prevent unbounded growth as users type
     * many different queries. When the cap is hit, we drop the oldest
     * half. Pure map-order LRU — simple and good enough for this scale.
     */
    async prefetchCustomRender(results) {
        if (!this._customRenderCache) this._customRenderCache = new Map();
        const CAP = 200;
        if (this._customRenderCache.size >= CAP) {
            const toDrop = Math.floor(CAP / 2);
            let i = 0;
            for (const key of this._customRenderCache.keys()) {
                if (i++ >= toDrop) break;
                this._customRenderCache.delete(key);
            }
        }
        const tasks = [];
        for (const r of results) {
            const id = r?._extensionId;
            if (!id) continue;
            const ext = this.extensions.get(id);
            if (!ext?.sandbox?.hasSearch) continue;
            if (!r.id) continue;
            if (this._customRenderCache.has(r.id)) {
                // Re-insert at the tail so recently-used entries survive
                // the next eviction pass.
                const v = this._customRenderCache.get(r.id);
                this._customRenderCache.delete(r.id);
                this._customRenderCache.set(r.id, v);
                continue;
            }
            // Strip host-only stamp.
            const { _extensionId: _ig, ...clean } = r;
            tasks.push(
                ext.sandbox
                    .call('renderCustom', { result: clean })
                    .then((out) => {
                        if (out && typeof out.html === 'string') {
                            this._customRenderCache.set(r.id, out);
                        }
                    })
                    .catch(() => {
                        /* null/throw → fall back to default */
                    })
            );
        }
        if (tasks.length) await Promise.all(tasks);
    }

    // --- Config updates ----------------------------------------------------

    async onConfigUpdate() {
        try {
            this._configCache = await this.invoke('get_config');
        } catch {
            return;
        }
        // Invalidate caches whose contents depend on extension config.
        this._customRenderCache?.clear();
        this._toolbarButtonsCache = null;
        for (const [id, ext] of this.extensions) {
            const config = this._getExtensionConfig(id, ext.manifest);
            if (ext.sandbox) {
                await ext.sandbox.updateConfig(config);
            }
        }
        // If enabled-states changed, unmount widgets of newly-disabled
        // extensions and (re-)mount widgets of newly-enabled ones. The
        // sandbox itself stays loaded either way so re-enable is
        // instant.
        if (this._widgetInstances) {
            for (const [key, ctrl] of this._widgetInstances) {
                if (!this._isEnabled(ctrl.extensionId)) {
                    // Unmount just this widget, not the whole extension.
                    ctrl.destroyed = true;
                    if (ctrl.timer) clearInterval(ctrl.timer);
                    try {
                        ctrl.host.remove();
                    } catch {}
                    this._widgetInstances.delete(key);
                }
            }
        }
        for (const [id, ext] of this.extensions) {
            if (!this._isEnabled(id)) continue;
            if (!Array.isArray(ext.manifest.contributes?.widgets)) continue;
            for (const w of ext.manifest.contributes.widgets) {
                if (!w?.id || !w?.slot) continue;
                const key = `${id}:${w.id}`;
                if (this._widgetInstances?.has(key)) continue; // already mounted
                try {
                    await this._mountWidget(id, ext, w);
                } catch (e) {
                    console.warn(`Failed to remount widget '${key}':`, e);
                }
            }
        }
        // Tool definitions may change when config updates (e.g. a tool
        // is enabled/disabled). Refresh the cache.
        try {
            await this.getToolDefinitions();
        } catch {}
        try {
            await this._refreshToolbarButtons();
        } catch {}
    }

    // --- Tool providers ----------------------------------------------------

    /**
     * Fetch tool definitions from all extensions that expose a tool
     * provider. The result is cached in `_toolDefsCache` so synchronous
     * call sites (`getToolDefinitionsCached`) can surface the latest
     * snapshot without hitting the sandbox on every render.
     */
    async getToolDefinitions() {
        const result = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.sandbox?.hasTools) continue;
            if (!this._isEnabled(id)) continue;
            try {
                const tools = await ext.sandbox.call('getTools', {});
                if (Array.isArray(tools) && tools.length > 0) {
                    result.push({
                        extensionId: id,
                        extensionName: ext.manifest.name || id,
                        extensionIcon: ext.manifest.icon || '🧩',
                        tools,
                    });
                }
            } catch (e) {
                console.warn(`getTools() in '${id}' failed:`, e);
            }
        }
        this._toolDefsCache = result;
        return result;
    }

    /**
     * Snapshot of the last known tool definitions. Synchronous. Callers
     * that need the freshest data should `await getToolDefinitions()`
     * first and then read this.
     */
    getToolDefinitionsCached() {
        return this._toolDefsCache || [];
    }

    async executeExtensionTool(extensionId, toolName, params = {}) {
        const ext = this.extensions.get(extensionId);
        if (!ext?.sandbox?.hasTools) {
            return { error: `Extension '${extensionId}' not found or has no tool provider` };
        }
        if (!this._isEnabled(extensionId)) {
            return { error: `Extension '${extensionId}' is disabled` };
        }

        // Tool providers can declare a custom timeout per tool. We honor it
        // up to a sensible upper bound so a misbehaving extension can't
        // stall the agent indefinitely.
        let timeoutMs = 5000;
        try {
            const declared = await ext.sandbox.call('getToolTimeout', { toolName });
            if (typeof declared === 'number' && declared > 0) {
                timeoutMs = Math.min(declared, 60_000);
            }
        } catch {
            /* non-fatal */
        }

        const timeoutPromise = new Promise((_, reject) =>
            setTimeout(
                () =>
                    reject(
                        new Error(`Extension tool timed out (${Math.round(timeoutMs / 1000)}s)`)
                    ),
                timeoutMs
            )
        );
        try {
            return await Promise.race([
                ext.sandbox.call('executeTool', { toolName, params }),
                timeoutPromise,
            ]);
        } catch (e) {
            return { error: e?.message || String(e) };
        }
    }

    /**
     * Steering text describing available extension tools to the LLM.
     * Resolves to the empty string if no extensions expose tools.
     */
    async buildToolSteeringBlock() {
        const defs = await this.getToolDefinitions();
        if (defs.length === 0) return '';

        let block = '<extension_tools>\n';
        block +=
            "You have access to local extension tools that run instantly on the user's machine.\n";
        block += 'To call one, emit a JSON block with this exact format:\n\n';
        block += '```extension_tool_call\n';
        block +=
            '{"extension": "<extension_id>", "tool": "<tool_name>", "params": {<parameters>}}\n';
        block += '```\n\n';
        block +=
            'IMPORTANT: After emitting the tool call block, STOP generating and wait for the result.\n';
        block +=
            'The result will be provided as a follow-up message. Then continue your response.\n';
        block +=
            'Only call ONE tool at a time. Do not call multiple tools in a single message.\n\n';
        block += 'Available extension tools:\n\n';

        for (const def of defs) {
            block += `Extension: ${def.extensionId} (${def.extensionIcon} ${def.extensionName})\n`;
            for (const tool of def.tools) {
                block += `  - ${tool.name}: ${tool.description}\n`;
                if (tool.parameters && Object.keys(tool.parameters).length > 0) {
                    const paramDescs = Object.entries(tool.parameters)
                        .map(
                            ([k, v]) =>
                                `${k} (${v.type}${v.default !== undefined ? ', default: ' + v.default : ''})${v.description ? ' — ' + v.description : ''}`
                        )
                        .join('; ');
                    block += `    Parameters: ${paramDescs}\n`;
                }
            }
            block += '\n';
        }

        block += '</extension_tools>\n\n';

        block += '<suggested_actions_format>\n';
        block += 'When your response presents options or asks the user what to do next, ';
        block += 'you can emit a hidden actions block at the END of your response. ';
        block +=
            'The frontend will strip this from the visible text and render the actions as clickable buttons.\n\n';
        block += '```suggested_actions\n';
        block += '[{"label": "Short button text", "prompt": "The message to send when clicked"}]\n';
        block += '```\n\n';
        block += 'Rules:\n';
        block += '- Place this block at the very end of your response, after all visible text.\n';
        block += '- Keep labels short (2-5 words). Use an emoji prefix if appropriate.\n';
        block += "- The prompt is what gets sent as the user's next message when they click.\n";
        block +=
            '- Include 2-4 actions max. Always include one that proceeds with the proposed plan.\n';
        block += '- Only use this when you are asking the user to choose between options.\n';
        block += '</suggested_actions_format>';

        return block;
    }

    // --- Trigger providers -------------------------------------------------

    async getTriggerDefinitions() {
        const defs = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.sandbox?.hasTriggers) continue;
            try {
                const triggers = await ext.sandbox.call('getTriggers', {});
                if (Array.isArray(triggers) && triggers.length > 0) {
                    defs.push({
                        extensionId: id,
                        extensionName: ext.manifest.name,
                        extensionIcon: ext.manifest.icon || '🔌',
                        triggers,
                    });
                }
            } catch (e) {
                console.warn(`getTriggers() in '${id}' failed:`, e);
            }
        }
        return defs;
    }

    // --- Lifecycle ---------------------------------------------------------

    getLoadedExtensions() {
        return Array.from(this.extensions.values()).map((ext) => ({
            ...ext.manifest,
            _capabilities: ext.capabilities,
        }));
    }

    /**
     * Hot-reload: discover newly installed extensions and tear down
     * ones that were uninstalled/disabled. Bundled ones are never
     * unloaded since they ship with the app.
     */
    async reload() {
        try {
            this._configCache = await this.invoke('get_config');
        } catch {
            return;
        }

        try {
            const userExts = await this.invoke('list_extensions');
            const installedIds = new Set(userExts.map((e) => e.manifest.id));
            const states = this._configCache?.extension_states || {};

            for (const [id] of this.extensions) {
                if (!installedIds.has(id) || states[id] === false) {
                    this._unloadExtension(id);
                }
            }

            for (const item of userExts) {
                if (!item.enabled) continue;
                const existing = this.extensions.get(item.manifest.id);
                if (existing) {
                    if (existing.manifest?.version === item.manifest.version) continue;
                    console.log(
                        `ExtensionManager: updating '${item.manifest.id}' from ${existing.manifest?.version} to ${item.manifest.version}`
                    );
                    this._unloadExtension(item.manifest.id);
                }
                try {
                    await this._loadExtension(item);
                    // Mount any widgets this extension contributes.
                    if (Array.isArray(item.manifest.contributes?.widgets)) {
                        const ext = this.extensions.get(item.manifest.id);
                        if (ext) {
                            for (const w of item.manifest.contributes.widgets) {
                                if (!w?.id || !w?.slot) continue;
                                await this._mountWidget(item.manifest.id, ext, w);
                            }
                        }
                    }
                    console.log(`ExtensionManager: hot-loaded '${item.manifest.id}'`);
                } catch (e) {
                    console.warn(`Failed to hot-load extension '${item.manifest.id}':`, e);
                }
            }
        } catch (e) {
            console.warn('Failed to reload extensions:', e);
        }
    }

    _unloadExtension(id) {
        console.log(`ExtensionManager: unloading '${id}'`);
        this._unmountWidgetsFor(id);
        this._pool.unload(id);
        document.querySelectorAll(`[data-ext-css="${id}"]`).forEach((el) => el.remove());
        this.extensions.delete(id);
    }

    // --- Widgets -----------------------------------------------------------

    /**
     * Map from slot name ("floating-bottom", "floating-status") to its
     * host container. The floating/chat windows call
     * `setWidgetSlot(slot, el)` once on startup; widgets mount into the
     * registered slots only. If a slot isn't registered, its widgets
     * simply aren't rendered in that window.
     */
    setWidgetSlot(slotName, element) {
        if (!this._widgetSlots) this._widgetSlots = new Map();
        this._widgetSlots.set(slotName, element);
        // If widgets were already mounted to a pending queue, flush them
        // now that the slot exists.
        if (this._pendingWidgetMounts?.has(slotName)) {
            const pending = this._pendingWidgetMounts.get(slotName);
            this._pendingWidgetMounts.delete(slotName);
            pending.forEach((fn) => {
                try {
                    fn();
                } catch {}
            });
        }
    }

    async _mountAllWidgets() {
        if (!this._widgetInstances) this._widgetInstances = new Map(); // key: extId + ':' + widgetId
        for (const [extId, ext] of this.extensions) {
            if (!this._isEnabled(extId)) continue;
            if (!Array.isArray(ext.manifest.contributes?.widgets)) continue;
            for (const w of ext.manifest.contributes.widgets) {
                if (!w?.id || !w?.slot) continue;
                await this._mountWidget(extId, ext, w);
            }
        }
    }

    async _mountWidget(extId, ext, widgetManifest) {
        const key = `${extId}:${widgetManifest.id}`;
        if (this._widgetInstances.has(key)) return;
        if (!ext.sandbox?.widgetIds?.includes(widgetManifest.id)) {
            // Extension declared the widget in the manifest but the
            // sandbox didn't actually load it (e.g. import error).
            console.warn(`Widget '${key}' declared but not loaded in sandbox — skipping mount`);
            return;
        }

        const host = document.createElement('div');
        host.className = `ext-widget ext-widget-${widgetManifest.slot}`;
        host.dataset.extWidgetKey = key;

        const controller = {
            extensionId: extId,
            widgetId: widgetManifest.id,
            slot: widgetManifest.slot,
            host,
            timer: null,
            destroyed: false,
            refreshIntervalMs: 0,
            // --- Refresh-budget enforcement state ----------------------
            // A widget can declare a healthy 60s interval but still
            // misbehave: throw on every tick, run >> than its declared
            // cadence, or stack overlapping renders if we don't gate.
            // The fields below implement a small circuit-breaker.

            /** True while a renderWidget RPC is in flight. We skip ticks
             *  rather than letting them stack — prevents unbounded
             *  pending-promise growth if the extension is slow. */
            renderInFlight: false,

            /** Consecutive failures (RPC throw, timeout, or render that
             *  blew the slow-render budget). When this hits the trip
             *  threshold we kill the timer and surface a paused state. */
            consecutiveFailures: 0,

            /** Set true after the breaker trips. The host shows a small
             *  "Widget paused" message with a retry link; until then we
             *  don't auto-recover. Manual retry resets the counter. */
            tripped: false,
        };
        this._widgetInstances.set(key, controller);

        // Attach to slot (or queue if slot not yet registered).
        const slotEl = this._widgetSlots?.get(widgetManifest.slot);
        if (slotEl) {
            slotEl.appendChild(host);
        } else {
            if (!this._pendingWidgetMounts) this._pendingWidgetMounts = new Map();
            const queue = this._pendingWidgetMounts.get(widgetManifest.slot) || [];
            queue.push(() => {
                const newSlot = this._widgetSlots.get(widgetManifest.slot);
                if (newSlot) newSlot.appendChild(host);
            });
            this._pendingWidgetMounts.set(widgetManifest.slot, queue);
        }

        // Fetch the extension's refresh interval and do an initial render.
        try {
            const ms = await ext.sandbox.call('getWidgetRefreshInterval', {
                widgetId: widgetManifest.id,
            });
            const n = Number(ms);
            if (Number.isFinite(n) && n > 0) {
                // Clamp to a sane range. The floor stops a widget from
                // re-rendering itself into a CPU spike (declared `1`
                // interpreted as 1ms), the ceiling keeps the schedule
                // bounded so the timer doesn't sit idle for years.
                controller.refreshIntervalMs = Math.max(
                    WIDGET_MIN_INTERVAL_MS,
                    Math.min(n, WIDGET_MAX_INTERVAL_MS)
                );
            } else {
                controller.refreshIntervalMs = 0;
            }
        } catch {
            controller.refreshIntervalMs = 0;
        }

        // Between the awaits above and the setInterval below, the
        // extension may have been unloaded (_unmountWidgetsFor flipped
        // `destroyed` and tried to clearInterval on a still-null timer).
        // Bail out here so we don't schedule an orphan interval.
        if (controller.destroyed) return;

        await this._renderWidget(controller);

        if (controller.destroyed) return;

        if (controller.refreshIntervalMs > 0) {
            controller.timer = setInterval(
                () => this._renderWidget(controller),
                controller.refreshIntervalMs
            );
        }
    }

    async _renderWidget(controller) {
        if (controller.destroyed || controller.tripped) return;
        // Skip ticks while the floating window is hidden. The host
        // signals this via window._kageFloatingHidden in app.js; the
        // widget is repainting into an off-screen webview otherwise,
        // which is wasted work for both us and the extension.
        // This check is intentionally lighter than the breaker path:
        // a skipped-while-hidden tick is not a failure (no counter
        // increment, no breaker trip). The next tick after the window
        // is shown will catch up.
        if (typeof window !== 'undefined' && window._kageFloatingHidden === true) return;
        const ext = this.extensions.get(controller.extensionId);
        if (!ext?.sandbox) return;

        // Re-entrancy guard. setInterval keeps firing even if the
        // previous render hasn't finished; left unchecked, a slow
        // widget piles up overlapping `renderWidget` RPCs and starves
        // every other RPC on the same sandbox. Skipping is preferable
        // to queueing — by the time the in-flight render returns, its
        // output is already what the next tick would draw.
        if (controller.renderInFlight) {
            this._noteWidgetFailure(controller, 'overlap');
            return;
        }
        controller.renderInFlight = true;

        const start = performance.now();
        let failureReason = null;
        try {
            const out = await ext.sandbox.call('renderWidget', { widgetId: controller.widgetId });
            // Bail if we were unmounted while the RPC was in flight —
            // writing to a detached host is harmless but the listeners
            // we'd wire up would never fire anyway.
            if (controller.destroyed) return;

            const elapsed = performance.now() - start;
            // Slow-render checks. Both bounds are hard caps:
            //   - absolute: 5s blocks the UI noticeably regardless of
            //     declared cadence. We treat that as a failure even
            //     for hourly-cadence widgets.
            //   - relative: a render eating 70%+ of its own interval
            //     is one bad scheduling beat away from overlapping
            //     with itself. Treat as a failure before we hit the
            //     overlap path above.
            if (elapsed >= WIDGET_SLOW_RENDER_MS) {
                failureReason = 'slow_absolute';
            } else if (
                controller.refreshIntervalMs > 0 &&
                elapsed >= controller.refreshIntervalMs * WIDGET_SLOW_RENDER_RATIO
            ) {
                failureReason = 'slow_relative';
            }

            if (!out || typeof out.html !== 'string') {
                // Nothing to render → hide the host so it takes up no layout.
                controller.host.innerHTML = '';
                controller.host.style.display = 'none';
            } else {
                const frag = sanitizeExtensionHtml(out.html, 'rich');
                controller.host.innerHTML = '';
                if (out.className) {
                    controller.host.className = `ext-widget ext-widget-${controller.slot} ${out.className}`;
                }
                controller.host.style.display = '';
                controller.host.appendChild(frag);

                // Wire declared action buttons. We enumerate all
                // data-ext-action elements in the widget and match by
                // action id — avoids having to escape arbitrary ids inside
                // a CSS attribute selector.
                if (Array.isArray(out.actions)) {
                    const actionMap = new Map();
                    for (const a of out.actions) {
                        if (!a?.id) continue;
                        actionMap.set(a.id, a.rpc || a.id);
                    }
                    if (actionMap.size > 0) {
                        const nodes = controller.host.querySelectorAll('[data-ext-action]');
                        for (const btn of nodes) {
                            const aid = btn.getAttribute('data-ext-action');
                            if (!actionMap.has(aid)) continue;
                            if (btn.__kageExtAction) continue;
                            btn.__kageExtAction = true;
                            const rpc = actionMap.get(aid);
                            btn.addEventListener('click', (ev) => {
                                ev.preventDefault();
                                ev.stopPropagation();
                                this._runWidgetAction(controller, rpc);
                            });
                        }
                    }
                }
            }

            if (failureReason) {
                this._noteWidgetFailure(controller, failureReason, elapsed);
            } else {
                // Successful render resets the failure counter — a single
                // good tick after two bad ones shouldn't leave us one
                // tick away from tripping. Transient blips are forgiven.
                controller.consecutiveFailures = 0;
            }
        } catch (e) {
            console.warn(
                `widget render for '${controller.extensionId}:${controller.widgetId}' failed:`,
                e
            );
            this._noteWidgetFailure(controller, 'throw');
        } finally {
            controller.renderInFlight = false;
        }
    }

    /** Increment the failure counter and trip the breaker if we've hit
     *  the threshold. `reason` is one of:
     *    - 'overlap'        — re-entrant tick skipped
     *    - 'slow_absolute'  — render exceeded WIDGET_SLOW_RENDER_MS
     *    - 'slow_relative'  — render exceeded interval * SLOW_RENDER_RATIO
     *    - 'throw'          — RPC threw or timed out
     *  Each failure increments the counter; a successful render resets
     *  it. Once tripped we stop the timer and surface a paused-state UI
     *  so the user sees what happened. */
    _noteWidgetFailure(controller, reason, elapsedMs) {
        controller.consecutiveFailures++;
        const key = `${controller.extensionId}:${controller.widgetId}`;
        console.warn(
            `[widget] ${key} failure (${reason}` +
                (typeof elapsedMs === 'number' ? `, ${Math.round(elapsedMs)}ms` : '') +
                `): ${controller.consecutiveFailures}/${WIDGET_FAILURE_TRIP_THRESHOLD}`
        );
        if (controller.consecutiveFailures < WIDGET_FAILURE_TRIP_THRESHOLD) return;

        // Trip the breaker. Stop the timer, mark the controller, and
        // render a small paused notice with a retry link. We keep the
        // host element in the DOM so the user can choose to recover —
        // unmounting would reset the breaker silently on the next
        // refresh anyway.
        controller.tripped = true;
        if (controller.timer) {
            clearInterval(controller.timer);
            controller.timer = null;
        }
        try {
            controller.host.style.display = '';
            controller.host.innerHTML = '';
            const notice = document.createElement('div');
            notice.className = 'ext-widget-paused';
            notice.style.cssText =
                'padding:8px 12px;font-size:12px;color:var(--kage-text-muted);background:var(--kage-bg-input);border-radius:4px;display:flex;align-items:center;gap:8px;';
            const extName =
                this.extensions.get(controller.extensionId)?.manifest?.name ||
                controller.extensionId;
            notice.innerHTML = tHtml('shared.extension.widget.paused_html', { name: extName });
            const retry = document.createElement('a');
            retry.href = '#';
            retry.textContent = t('shared.extension.widget.retry');
            retry.style.cssText = 'color:var(--kage-accent);text-decoration:underline;';
            retry.addEventListener('click', (ev) => {
                ev.preventDefault();
                this._retryWidget(controller);
            });
            notice.appendChild(retry);
            controller.host.appendChild(notice);
        } catch {
            // DOM may be in any state if we got here mid-render; the
            // breaker tripping is what matters, the UI hint is
            // best-effort.
        }

        // Telemetry — surface in aggregate so we can spot a problematic
        // extension across the install base. Anonymous: extension id
        // only, no widget content. Best-effort import so this file
        // doesn't hard-depend on the telemetry module being loadable.
        try {
            import('./telemetry.js')
                .then(({ trackEvent }) =>
                    trackEvent('extension_widget_disabled', {
                        extension_id: controller.extensionId,
                        widget_id: controller.widgetId,
                        reason,
                    })
                )
                .catch(() => {});
        } catch {}
    }

    /** Reset and resume a tripped widget. Single retry: if it trips
     *  again, we leave it paused — repeated retries would let a broken
     *  widget burn CPU indefinitely. The user can disable the
     *  extension if it never recovers. */
    _retryWidget(controller) {
        if (controller.destroyed) return;
        controller.consecutiveFailures = 0;
        controller.tripped = false;
        controller.renderInFlight = false;
        controller.host.innerHTML = '';
        this._renderWidget(controller).then(() => {
            if (controller.destroyed || controller.tripped) return;
            if (controller.refreshIntervalMs > 0 && !controller.timer) {
                controller.timer = setInterval(
                    () => this._renderWidget(controller),
                    controller.refreshIntervalMs
                );
            }
        });
    }

    async _runWidgetAction(controller, actionId) {
        const ext = this.extensions.get(controller.extensionId);
        if (!ext?.sandbox) return;
        try {
            const out = await ext.sandbox.call('onWidgetAction', {
                widgetId: controller.widgetId,
                actionId,
                context: {},
            });
            // If the action returns an immediate re-render request, do it.
            if (out?.rerender) {
                await this._renderWidget(controller);
            }
        } catch (e) {
            console.warn(`widget action '${actionId}' in '${controller.extensionId}' failed:`, e);
        }
    }

    _unmountWidgetsFor(extensionId) {
        if (!this._widgetInstances) return;
        for (const [key, ctrl] of this._widgetInstances) {
            if (ctrl.extensionId !== extensionId) continue;
            ctrl.destroyed = true;
            if (ctrl.timer) clearInterval(ctrl.timer);
            try {
                ctrl.host.remove();
            } catch {}
            this._widgetInstances.delete(key);
        }
    }

    /**
     * Refresh the cached list of toolbar buttons from all enabled
     * extensions that expose a toolbar provider. Call after init and on
     * config updates; readers use the synchronous `getToolbarButtons()`.
     */
    async _refreshToolbarButtons() {
        const out = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.sandbox?.hasToolbar) continue;
            if (!this._isEnabled(id)) continue;
            try {
                const defs = await ext.sandbox.call('getToolbarButtons', {});
                if (Array.isArray(defs)) {
                    for (const d of defs) {
                        if (!d?.id) continue;
                        out.push({
                            extensionId: id,
                            id: String(d.id),
                            icon: String(d.icon || '🧩'),
                            tooltip: String(d.tooltip || ''),
                        });
                    }
                }
            } catch (e) {
                console.warn(`toolbar getButtons() in '${id}' failed:`, e);
            }
        }
        this._toolbarButtonsCache = out;
        return out;
    }

    /**
     * Synchronous snapshot of toolbar buttons. The cache is primed by
     * `initialize()` and refreshed on config change.
     */
    getToolbarButtons() {
        if (!this._toolbarButtonsCache) return [];
        return this._toolbarButtonsCache.map((b) => ({
            ...b,
            // The onClick callback bridges to the sandbox RPC. The call
            // site provides the current chat context (input + messages).
            onClick: (ctx) => this.runToolbarClick(b.extensionId, b.id, ctx),
        }));
    }

    /**
     * Execute a toolbar button click. `ctx` carries the current chat
     * input and messages so the extension can make an informed decision
     * without DOM access. Returns a host effect the caller should
     * apply (set input, send message, etc.) or null.
     */
    async runToolbarClick(extensionId, buttonId, ctx = {}) {
        const ext = this.extensions.get(extensionId);
        if (!ext?.sandbox?.hasToolbar) return null;
        try {
            // Marshal ctx so extensions can't reach back into live DOM
            // via functions accidentally passed in.
            const safeCtx = {
                input: typeof ctx.input === 'string' ? ctx.input : '',
                messages: Array.isArray(ctx.messages)
                    ? ctx.messages.map((m) => ({
                          role: String(m?.role || ''),
                          content: typeof m?.content === 'string' ? m.content : '',
                      }))
                    : [],
            };
            const out = await ext.sandbox.call('onToolbarClick', {
                buttonId,
                context: safeCtx,
            });
            return out && typeof out === 'object' ? out : null;
        } catch (e) {
            console.warn(`toolbar onClick in '${extensionId}' failed:`, e);
            return null;
        }
    }

    // --- Message formatter -------------------------------------------------

    /**
     * Run all enabled extension message formatters against the rendered
     * container. Each formatter receives the container's innerHTML and
     * returns either a replacement string (sanitized and applied) or
     * null to leave the content unchanged. During streaming we skip
     * formatters that haven't opted into live formatting.
     */
    async formatMessage(container, context) {
        if (!container || !this.extensions?.size) return;
        const ctx = {
            streaming: !!context?.streaming,
            role: String(context?.role || ''),
        };
        for (const [id, ext] of this.extensions) {
            if (!ext.sandbox?.hasFormatter) continue;
            if (!this._isEnabled(id)) continue;
            // Skip streaming calls unless the extension explicitly
            // opted in. Most formatters return null during streaming,
            // so round-tripping per chunk is wasted work.
            if (ctx.streaming && !ext.sandbox.formatterOptsInStreaming) continue;
            try {
                const out = await ext.sandbox.call('formatMessage', {
                    html: container.innerHTML,
                    context: ctx,
                });
                if (out && typeof out.html === 'string') {
                    const frag = sanitizeExtensionHtml(out.html, 'rich');
                    // Replace the container's children with the sanitized
                    // fragment. We use replaceChildren so existing
                    // listeners on the container itself are preserved.
                    container.replaceChildren();
                    container.appendChild(frag);
                    // Wire any declared extension actions in the new DOM.
                    this._wireExtActionsFor(id, container);
                }
            } catch (e) {
                console.warn(`message formatter in '${id}' failed:`, e);
            }
        }
    }

    // --- Shared: wire data-ext-action buttons in sanitized HTML -----------

    _wireExtActionsFor(extensionId, root) {
        const hits = findExtActions(root);
        for (const { element, actionId } of hits) {
            // Defensive: prevent double-wiring when the same container is
            // re-formatted multiple times during streaming.
            if (element.__kageExtAction) continue;
            element.__kageExtAction = true;
            element.addEventListener('click', (ev) => {
                ev.preventDefault();
                ev.stopPropagation();
                const ext = this.extensions.get(extensionId);
                if (!ext?.sandbox) return;
                // Custom-render actions are for result-row buttons: they
                // all flow through onWidgetAction-style RPC because we
                // don't yet have a `onRenderAction` — in Commit C we route
                // them through the search provider's execute() with a
                // synthetic result carrying { action: actionId }.
                ext.sandbox
                    .call('onResultAction', {
                        actionId,
                        resultId: root.dataset?.extResultId || null,
                    })
                    .catch((e) => console.warn(`onResultAction '${actionId}' failed:`, e));
            });
        }
    }

    /**
     * Tear down all extensions and their sandboxes. Called from the
     * floating window's tauri://close-requested handler so a closed
     * webview doesn't leave widget timers ticking until process exit.
     *
     * Idempotent: safe to call twice; the second call no-ops because
     * the widget instances and sandbox pool are already empty.
     */
    destroy() {
        // Clear every widget's timer + flip the destroyed flag so any
        // in-flight render returns without writing back to the DOM.
        if (this._widgetInstances) {
            for (const [, ctrl] of this._widgetInstances) {
                ctrl.destroyed = true;
                if (ctrl.timer) {
                    clearInterval(ctrl.timer);
                    ctrl.timer = null;
                }
            }
            this._widgetInstances.clear();
        }
        // Sandbox pool tears down each iframe + rejects pending RPCs.
        try {
            this._pool?.unloadAll();
        } catch (e) {
            console.warn('ExtensionManager.destroy: pool.unloadAll failed:', e);
        }
        this.extensions.clear();
    }
}
