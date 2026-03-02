/**
 * Extension Manager — discovers, loads, and coordinates extensions.
 * Loads bundled extensions from ui/extensions/ and user-installed extensions
 * via the Rust backend (read_extension_file command).
 */

const BUNDLED_EXT_PATH = 'extensions';

export class ExtensionManager {
    constructor(invoke) {
        this.invoke = invoke;
        /** @type {Map<string, LoadedExtension>} */
        this.extensions = new Map();
        this._configCache = null;
    }

    /**
     * Discover and load all enabled extensions (bundled + user-installed).
     */
    async initialize() {
        try {
            this._configCache = await this.invoke('get_config');
        } catch (e) {
            console.error('ExtensionManager: failed to load config', e);
            this._configCache = {};
        }

        // 1. Load bundled extensions from ui/extensions/bundled.json
        try {
            const resp = await fetch(`${BUNDLED_EXT_PATH}/bundled.json`);
            if (resp.ok) {
                const bundledList = await resp.json();
                for (const entry of bundledList) {
                    try {
                        await this._loadBundledExtension(entry.id);
                    } catch (e) {
                        console.warn(`Failed to load bundled extension '${entry.id}':`, e);
                    }
                }
            }
        } catch (e) {
            console.warn('Failed to load bundled.json:', e);
        }

        // 2. Load user-installed extensions from backend
        try {
            const userExts = await this.invoke('list_extensions');
            for (const item of userExts) {
                if (!item.enabled) continue;
                if (this.extensions.has(item.manifest.id)) continue; // bundled takes precedence
                try {
                    await this._loadUserExtension(item);
                } catch (e) {
                    console.warn(`Failed to load user extension '${item.manifest.id}':`, e);
                }
            }
        } catch (e) {
            console.warn('Failed to list user extensions:', e);
        }

        console.log(`ExtensionManager: ${this.extensions.size} extensions loaded`);
    }

    async _loadBundledExtension(id) {
        const basePath = `${BUNDLED_EXT_PATH}/${id}`;
        const manifestResp = await fetch(`${basePath}/manifest.json`);
        if (!manifestResp.ok) return;
        const manifest = await manifestResp.json();

        const states = this._configCache?.extension_states || {};
        if (states[id] === false) return;

        const ext = { manifest, basePath, searchProvider: null, userInstalled: false };

        if (manifest.contributes?.searchProvider) {
            try {
                const mod = await import(`../${basePath}/${manifest.contributes.searchProvider}`);
                ext.searchProvider = new mod.default();
                ext.searchProvider.initialize({ invoke: this.invoke, config: this._getExtensionConfig(id, manifest) });
            } catch (e) {
                console.warn(`Failed to load search provider for '${id}':`, e);
            }
        }

        this.extensions.set(id, ext);
    }

    async _loadUserExtension(item) {
        const id = item.manifest.id;
        const manifest = item.manifest;

        const states = this._configCache?.extension_states || {};
        if (states[id] === false) return;

        const ext = { manifest, basePath: null, searchProvider: null, userInstalled: true };

        // Load search provider via read_extension_file
        if (manifest.contributes?.searchProvider) {
            try {
                const jsCode = await this.invoke('read_extension_file', {
                    extensionId: id,
                    kind: 'extension',
                    filePath: manifest.contributes.searchProvider.replace('./', ''),
                });
                // Create a blob URL and dynamically import it
                const blob = new Blob([jsCode], { type: 'application/javascript' });
                const blobUrl = URL.createObjectURL(blob);
                const mod = await import(blobUrl);
                URL.revokeObjectURL(blobUrl);
                ext.searchProvider = new mod.default();
                ext.searchProvider.initialize({ invoke: this.invoke, config: this._getExtensionConfig(id, manifest) });
            } catch (e) {
                console.warn(`Failed to load search provider for user extension '${id}':`, e);
            }
        }

        this.extensions.set(id, ext);
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

    matchAll(query) {
        const results = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.searchProvider) continue;
            if (!this._isEnabled(id)) continue;
            try {
                const matches = ext.searchProvider.match(query);
                for (const m of matches) {
                    m._extensionId = id; // stamp with owning extension
                    results.push(m);
                }
            } catch (e) {
                console.warn(`Search error in '${id}':`, e);
            }
        }
        return results;
    }

    async matchAllAsync(query) {
        const results = [];
        const promises = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.searchProvider?.matchAsync) continue;
            if (!this._isEnabled(id)) continue;
            promises.push(
                ext.searchProvider.matchAsync(query)
                    .then(matches => {
                        for (const m of matches) {
                            m._extensionId = id;
                            results.push(m);
                        }
                    })
                    .catch(e => console.warn(`Async search error in '${id}':`, e))
            );
        }
        await Promise.all(promises);
        return results;
    }

    executeResult(result) {
        const id = result._extensionId;
        if (id) {
            const ext = this.extensions.get(id);
            if (ext?.searchProvider) {
                try { return ext.searchProvider.execute(result); } catch {}
            }
        }
        return null;
    }

    renderResult(result, element) {
        const id = result._extensionId;
        if (id) {
            const ext = this.extensions.get(id);
            if (ext?.searchProvider?.renderResult) {
                try { ext.searchProvider.renderResult(result, element); return true; } catch {}
            }
        }
        return false;
    }

    async onConfigUpdate() {
        try {
            this._configCache = await this.invoke('get_config');
        } catch { return; }
        for (const [id, ext] of this.extensions) {
            if (!ext.searchProvider?.onConfigUpdate) continue;
            try {
                ext.searchProvider.onConfigUpdate(this._getExtensionConfig(id, ext.manifest));
            } catch (e) {
                console.warn(`Config update error in '${id}':`, e);
            }
        }
    }

    getLoadedExtensions() {
        return Array.from(this.extensions.values()).map(ext => ext.manifest);
    }
}
