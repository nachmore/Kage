/**
 * Extension Manager — discovers, loads, and coordinates extensions.
 * Dynamically imports search providers, settings modules, and widgets
 * from bundled (ui/extensions/) and user-installed extension directories.
 */

// Bundled extension base path (relative to ui/)
const BUNDLED_EXT_PATH = 'extensions';

export class ExtensionManager {
    constructor(invoke) {
        this.invoke = invoke;
        /** @type {Map<string, LoadedExtension>} */
        this.extensions = new Map();
        this._configCache = null;
    }

    /**
     * Discover and load all enabled extensions.
     * Bundled extensions are loaded from ui/extensions/<id>/manifest.json.
     * User-installed extensions are discovered via the Rust backend.
     */
    async initialize() {
        // Load config for extension states and configs
        try {
            this._configCache = await this.invoke('get_config');
        } catch (e) {
            console.error('ExtensionManager: failed to load config', e);
            this._configCache = {};
        }

        // 1. Load bundled extensions (hardcoded list — these ship with the app)
        const bundledIds = ['dev-tools', 'math', 'color-picker', 'timer'];
        for (const id of bundledIds) {
            try {
                await this._loadBundledExtension(id);
            } catch (e) {
                console.warn(`Failed to load bundled extension '${id}':`, e);
            }
        }

        // 2. Load user-installed extensions from backend
        try {
            const userExts = await this.invoke('list_extensions');
            for (const item of userExts) {
                if (!item.enabled) continue;
                if (this.extensions.has(item.manifest.id)) continue; // bundled already loaded
                // User extensions would need a different loading mechanism
                // (their files aren't in ui/extensions/). For now, skip — 
                // the store install flow will handle this in the future.
            }
        } catch (e) {
            console.warn('Failed to list user extensions:', e);
        }

        console.log(`ExtensionManager: ${this.extensions.size} extensions loaded`);
    }

    async _loadBundledExtension(id) {
        const basePath = `${BUNDLED_EXT_PATH}/${id}`;
        
        // Fetch manifest
        const manifestResp = await fetch(`${basePath}/manifest.json`);
        if (!manifestResp.ok) return;
        const manifest = await manifestResp.json();

        // Check if enabled
        const states = this._configCache?.extension_states || {};
        if (states[id] === false) {
            console.log(`Extension '${id}' is disabled, skipping`);
            return;
        }

        const ext = {
            manifest,
            basePath,
            searchProvider: null,
            settingsModuleClass: null,
            widgets: [],
        };

        // Load search provider
        if (manifest.contributes?.searchProvider) {
            try {
                const mod = await import(`../${basePath}/${manifest.contributes.searchProvider}`);
                const Provider = mod.default;
                ext.searchProvider = new Provider();
                const extConfig = this._getExtensionConfig(id, manifest);
                ext.searchProvider.initialize({ invoke: this.invoke, config: extConfig });
            } catch (e) {
                console.warn(`Failed to load search provider for '${id}':`, e);
            }
        }

        // Settings modules are loaded separately by the settings page
        // (they use <script> tags, not ES modules, due to the SettingsModule base class pattern)

        this.extensions.set(id, ext);
    }

    /**
     * Get the config for an extension, falling back to manifest defaults.
     */
    _getExtensionConfig(id, manifest) {
        const saved = this._configCache?.extensions?.[id];
        if (saved) return saved;

        // Build defaults from manifest config schema
        const defaults = {};
        if (manifest.config) {
            for (const [key, schema] of Object.entries(manifest.config)) {
                defaults[key] = schema.default;
            }
        }
        return defaults;
    }

    /**
     * Check if an extension is enabled in the current config.
     */
    _isEnabled(id) {
        const states = this._configCache?.extension_states || {};
        return states[id] !== false;
    }

    /**
     * Collect search results from all loaded extension search providers (sync).
     * @param {string} query
     * @returns {Array} search results
     */
    matchAll(query) {
        const results = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.searchProvider) continue;
            if (!this._isEnabled(id)) continue;
            try {
                const matches = ext.searchProvider.match(query);
                results.push(...matches);
            } catch (e) {
                console.warn(`Search provider error in '${id}':`, e);
            }
        }
        return results;
    }

    /**
     * Collect async search results from all loaded extension search providers.
     * @param {string} query
     * @returns {Promise<Array>} search results
     */
    async matchAllAsync(query) {
        const results = [];
        const promises = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.searchProvider?.matchAsync) continue;
            if (!this._isEnabled(id)) continue;
            promises.push(
                ext.searchProvider.matchAsync(query)
                    .then(matches => results.push(...matches))
                    .catch(e => console.warn(`Async search error in '${id}':`, e))
            );
        }
        await Promise.all(promises);
        return results;
    }

    /**
     * Execute a result's action via its extension's search provider.
     * @param {object} result - the search result object
     * @returns {object|null} action descriptor or null
     */
    executeResult(result) {
        // Find which extension owns this result by checking type prefixes
        for (const [id, ext] of this.extensions) {
            if (!ext.searchProvider) continue;
            try {
                const action = ext.searchProvider.execute(result);
                if (action) return action;
            } catch { /* not this extension's result */ }
        }
        return null;
    }

    /**
     * Check if any extension has a custom renderer for a result.
     */
    renderResult(result, element) {
        for (const [id, ext] of this.extensions) {
            if (!ext.searchProvider?.renderResult) continue;
            try {
                ext.searchProvider.renderResult(result, element);
                return true;
            } catch { /* not this extension's result */ }
        }
        return false;
    }

    /**
     * Notify all extensions of a config update.
     */
    async onConfigUpdate() {
        try {
            this._configCache = await this.invoke('get_config');
        } catch { return; }

        for (const [id, ext] of this.extensions) {
            if (!ext.searchProvider?.onConfigUpdate) continue;
            const extConfig = this._getExtensionConfig(id, ext.manifest);
            try {
                ext.searchProvider.onConfigUpdate(extConfig);
            } catch (e) {
                console.warn(`Config update error in '${id}':`, e);
            }
        }
    }

    /**
     * Get list of loaded extensions (for UI display).
     */
    getLoadedExtensions() {
        return Array.from(this.extensions.values()).map(ext => ext.manifest);
    }
}
