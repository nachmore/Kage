import { normalizePermissions } from '../extension-permissions.js';
import { resolveExtensionCatalog } from './i18n.js';
import { applyMixin } from '../mixin.js';

const SANDBOX_VENDOR_ALLOWLIST = {
    math: 'vendor/lib/math.js',
};

export function installExtensionSourceMethods(ExtensionManager) {
    applyMixin(ExtensionManager.prototype, {
        _hasSandboxedProvider(sources) {
            return !!(
                sources.searchProvider ||
                sources.toolProvider ||
                sources.triggerProvider ||
                sources.toolbarProvider ||
                sources.messageFormatter ||
                (sources.widgets && Object.keys(sources.widgets).length > 0)
            );
        },

        async _fetchProviderSources(id, manifest) {
            const out = {};
            const c = manifest.contributes || {};
            if (c.searchProvider) out.searchProvider = await this._fetchText(id, c.searchProvider);
            if (c.toolProvider) out.toolProvider = await this._fetchText(id, c.toolProvider);
            if (c.triggerProvider)
                out.triggerProvider = await this._fetchText(id, c.triggerProvider);
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
        },

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
         */ async _fetchSharedSources(sources, extensionId) {
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
        },

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
         */ async _fetchExtensionLocale(id, manifest) {
            const kind = manifest?.type || 'extension';
            return resolveExtensionCatalog(async (code) => {
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
        },

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
        },

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
        },

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
         */ _resolveGrantedCapabilities(manifest) {
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
        },

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
        },

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
        },

        _isEnabled(id) {
            const states = this._configCache?.extension_states || {};
            return states[id] !== false;
        },
    });
}
