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

        const context = { invoke: this.invoke, config: this._getExtensionConfig(id, manifest) };

        if (manifest.contributes?.searchProvider) {
            try {
                const mod = await import(`../../${basePath}/${manifest.contributes.searchProvider}`);
                ext.searchProvider = new mod.default();
                ext.searchProvider.initialize(context);
            } catch (e) {
                console.warn(`Failed to load search provider for '${id}':`, e);
            }
        }

        // Load toolbar buttons
        if (manifest.contributes?.toolbarButtons) {
            try {
                const mod = await import(`../../${basePath}/${manifest.contributes.toolbarButtons}`);
                ext.toolbarProvider = new mod.default();
                ext.toolbarProvider.initialize?.(context);
            } catch (e) {
                console.warn(`Failed to load toolbar buttons for '${id}':`, e);
            }
        }

        // Load message formatters
        if (manifest.contributes?.messageFormatters) {
            try {
                const mod = await import(`../../${basePath}/${manifest.contributes.messageFormatters}`);
                ext.messageFormatter = new mod.default();
                ext.messageFormatter.initialize?.(context);
            } catch (e) {
                console.warn(`Failed to load message formatters for '${id}':`, e);
            }
        }

        // Load tool provider
        if (manifest.contributes?.toolProvider) {
            try {
                const mod = await import(`../../${basePath}/${manifest.contributes.toolProvider}`);
                ext.toolProvider = new mod.default();
                ext.toolProvider.initialize?.(context);
            } catch (e) {
                console.warn(`Failed to load tool provider for '${id}':`, e);
            }
        }

        // Load CSS
        this._loadBundledCss(id, basePath, manifest);

        this.extensions.set(id, ext);
    }

    _loadBundledCss(id, basePath, manifest) {
        const cssFiles = manifest.contributes?.css;
        if (!Array.isArray(cssFiles) || cssFiles.length === 0) return;
        for (const cssPath of cssFiles) {
            const fullPath = `${basePath}/${cssPath.replace('./', '')}`;
            if (document.querySelector(`link[data-ext-css="${id}"]`)) continue;
            const link = document.createElement('link');
            link.rel = 'stylesheet';
            link.href = fullPath;
            link.dataset.extCss = id;
            document.head.appendChild(link);
        }
    }

    async _loadUserExtension(item) {
        const id = item.manifest.id;
        const manifest = item.manifest;

        const states = this._configCache?.extension_states || {};
        if (states[id] === false) return;

        const ext = { manifest, basePath: null, searchProvider: null, userInstalled: true };

        const context = { invoke: this.invoke, config: this._getExtensionConfig(id, manifest) };

        // Load search provider via read_extension_file
        if (manifest.contributes?.searchProvider) {
            try {
                const jsCode = await this.invoke('read_extension_file', {
                    extensionId: id,
                    kind: 'extension',
                    filePath: manifest.contributes.searchProvider.replace('./', ''),
                });
                const blob = new Blob([jsCode], { type: 'application/javascript' });
                const blobUrl = URL.createObjectURL(blob);
                const mod = await import(blobUrl);
                URL.revokeObjectURL(blobUrl);
                ext.searchProvider = new mod.default();
                ext.searchProvider.initialize(context);
            } catch (e) {
                console.warn(`Failed to load search provider for user extension '${id}':`, e);
            }
        }

        // Load toolbar buttons
        if (manifest.contributes?.toolbarButtons) {
            try {
                const jsCode = await this.invoke('read_extension_file', {
                    extensionId: id, kind: 'extension',
                    filePath: manifest.contributes.toolbarButtons.replace('./', ''),
                });
                const blob = new Blob([jsCode], { type: 'application/javascript' });
                const blobUrl = URL.createObjectURL(blob);
                const mod = await import(blobUrl);
                URL.revokeObjectURL(blobUrl);
                ext.toolbarProvider = new mod.default();
                ext.toolbarProvider.initialize?.(context);
            } catch (e) {
                console.warn(`Failed to load toolbar buttons for user extension '${id}':`, e);
            }
        }

        // Load message formatters
        if (manifest.contributes?.messageFormatters) {
            try {
                const jsCode = await this.invoke('read_extension_file', {
                    extensionId: id, kind: 'extension',
                    filePath: manifest.contributes.messageFormatters.replace('./', ''),
                });
                const blob = new Blob([jsCode], { type: 'application/javascript' });
                const blobUrl = URL.createObjectURL(blob);
                const mod = await import(blobUrl);
                URL.revokeObjectURL(blobUrl);
                ext.messageFormatter = new mod.default();
                ext.messageFormatter.initialize?.(context);
            } catch (e) {
                console.warn(`Failed to load message formatters for user extension '${id}':`, e);
            }
        }

        // Load tool provider
        if (manifest.contributes?.toolProvider) {
            try {
                const jsCode = await this.invoke('read_extension_file', {
                    extensionId: id, kind: 'extension',
                    filePath: manifest.contributes.toolProvider.replace('./', ''),
                });
                const blob = new Blob([jsCode], { type: 'application/javascript' });
                const blobUrl = URL.createObjectURL(blob);
                const mod = await import(blobUrl);
                URL.revokeObjectURL(blobUrl);
                ext.toolProvider = new mod.default();
                ext.toolProvider.initialize?.(context);
            } catch (e) {
                console.warn(`Failed to load tool provider for user extension '${id}':`, e);
            }
        }

        // Load CSS
        await this._loadUserCss(id, manifest);

        this.extensions.set(id, ext);
    }

    async _loadUserCss(id, manifest) {
        const cssFiles = manifest.contributes?.css;
        if (!Array.isArray(cssFiles) || cssFiles.length === 0) return;
        for (const cssPath of cssFiles) {
            if (document.querySelector(`style[data-ext-css="${id}"]`)) continue;
            try {
                const cssCode = await this.invoke('read_extension_file', {
                    extensionId: id, kind: 'extension',
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

    matchAll(query) {
        // > prefix is reserved for built-in commands — never sent to extensions
        if (query.trim().startsWith('>')) return [];
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
        // > prefix is reserved for built-in commands — never sent to extensions
        if (query.trim().startsWith('>')) return [];
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
            const config = this._getExtensionConfig(id, ext.manifest);
            if (ext.searchProvider?.onConfigUpdate) {
                try { ext.searchProvider.onConfigUpdate(config); } catch (e) {
                    console.warn(`Config update error in '${id}':`, e);
                }
            }
            if (ext.toolbarProvider?.onConfigUpdate) {
                try { ext.toolbarProvider.onConfigUpdate(config); } catch (e) {
                    console.warn(`Toolbar config update error in '${id}':`, e);
                }
            }
            if (ext.messageFormatter?.onConfigUpdate) {
                try { ext.messageFormatter.onConfigUpdate(config); } catch (e) {
                    console.warn(`Formatter config update error in '${id}':`, e);
                }
            }
            if (ext.toolProvider?.onConfigUpdate) {
                try { ext.toolProvider.onConfigUpdate(config); } catch (e) {
                    console.warn(`Tool provider config update error in '${id}':`, e);
                }
            }
        }
    }

    /**
     * Get all toolbar button definitions from loaded extensions.
     * @returns {Array<{id, icon, tooltip, onClick, extensionId}>}
     */
    getToolbarButtons() {
        const buttons = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.toolbarProvider) continue;
            if (!this._isEnabled(id)) continue;
            try {
                const defs = ext.toolbarProvider.getButtons();
                if (Array.isArray(defs)) {
                    for (const btn of defs) {
                        buttons.push({ ...btn, extensionId: id });
                    }
                }
            } catch (e) {
                console.warn(`getButtons error in '${id}':`, e);
            }
        }
        return buttons;
    }

    /**
     * Run all message formatters on a rendered message container.
     * Called after markdown rendering is complete.
     * @param {HTMLElement} container - The rendered message content element
     * @param {object} context - { role: 'user'|'assistant', streaming: boolean }
     */
    formatMessage(container, context) {
        for (const [id, ext] of this.extensions) {
            if (!ext.messageFormatter) continue;
            if (!this._isEnabled(id)) continue;
            try {
                ext.messageFormatter.format(container, context);
            } catch (e) {
                console.warn(`Message formatter error in '${id}':`, e);
            }
        }
    }

    getLoadedExtensions() {
        return Array.from(this.extensions.values()).map(ext => ext.manifest);
    }

    /**
     * Reload extensions — discovers newly installed extensions without restarting.
     * Existing extensions are kept; only new ones are loaded.
     */
    async reload() {
        try {
            this._configCache = await this.invoke('get_config');
        } catch { return; }

        // Unload extensions that are no longer installed or were disabled
        try {
            const userExts = await this.invoke('list_extensions');
            const installedIds = new Set(userExts.map(e => e.manifest.id));
            const states = this._configCache?.extension_states || {};

            for (const [id, ext] of this.extensions) {
                // Keep bundled extensions
                if (!ext.userInstalled) continue;
                // Unload if no longer installed or disabled
                if (!installedIds.has(id) || states[id] === false) {
                    this._unloadExtension(id, ext);
                }
            }

            // Load newly installed extensions
            for (const item of userExts) {
                if (!item.enabled) continue;
                if (this.extensions.has(item.manifest.id)) continue;
                try {
                    await this._loadUserExtension(item);
                    console.log(`ExtensionManager: hot-loaded '${item.manifest.id}'`);
                } catch (e) {
                    console.warn(`Failed to hot-load extension '${item.manifest.id}':`, e);
                }
            }
        } catch (e) {
            console.warn('Failed to reload extensions:', e);
        }
    }

    /**
     * Unload a single extension — destroy providers, remove CSS, remove from map.
     */
    _unloadExtension(id, ext) {
        console.log(`ExtensionManager: unloading '${id}'`);
        try { ext.searchProvider?.destroy?.(); } catch {}
        try { ext.toolbarProvider?.destroy?.(); } catch {}
        try { ext.messageFormatter?.destroy?.(); } catch {}
        try { ext.toolProvider?.destroy?.(); } catch {}

        // Remove injected CSS
        document.querySelectorAll(`[data-ext-css="${id}"]`).forEach(el => el.remove());

        this.extensions.delete(id);
    }

    /**
     * Collect tool definitions from all enabled extensions with tool providers.
     * Returns an array of { extensionId, extensionName, extensionIcon, tools[] }
     * where each tool has { name, description, parameters }.
     */
    getToolDefinitions() {
        const result = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.toolProvider) continue;
            if (!this._isEnabled(id)) continue;
            try {
                const tools = ext.toolProvider.getTools();
                if (Array.isArray(tools) && tools.length > 0) {
                    result.push({
                        extensionId: id,
                        extensionName: ext.manifest.name || id,
                        extensionIcon: ext.manifest.icon || '🧩',
                        tools,
                    });
                }
            } catch (e) {
                console.warn(`getTools error in '${id}':`, e);
            }
        }
        return result;
    }

    /**
     * Execute an extension tool call. Returns { result, error } with a timeout.
     * Default timeout is 5s, but tools can declare a longer timeout via getToolTimeout().
     * @param {string} extensionId
     * @param {string} toolName
     * @param {object} params
     * @returns {Promise<{result?: any, error?: string}>}
     */
    async executeExtensionTool(extensionId, toolName, params = {}) {
        const ext = this.extensions.get(extensionId);
        if (!ext?.toolProvider) {
            return { error: `Extension '${extensionId}' not found or has no tool provider` };
        }
        if (!this._isEnabled(extensionId)) {
            return { error: `Extension '${extensionId}' is disabled` };
        }

        // Allow tool providers to declare custom timeouts per tool
        let timeoutMs = 5000;
        if (typeof ext.toolProvider.getToolTimeout === 'function') {
            timeoutMs = ext.toolProvider.getToolTimeout(toolName) || timeoutMs;
        }
        try {
            const resultPromise = ext.toolProvider.execute(toolName, params);
            const timeoutPromise = new Promise((_, reject) =>
                setTimeout(() => reject(new Error(`Extension tool timed out (${Math.round(timeoutMs / 1000)}s)`)), timeoutMs)
            );
            return await Promise.race([resultPromise, timeoutPromise]);
        } catch (e) {
            return { error: e.message || String(e) };
        }
    }

    /**
     * Build the steering text block describing available extension tools.
     * This is injected into the agent's system prompt so it knows what tools exist.
     */
    buildToolSteeringBlock() {
        const defs = this.getToolDefinitions();
        if (defs.length === 0) return '';

        const now = new Date();
        const dateStr = now.toLocaleDateString('en-US', { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric' });
        const timeStr = now.toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit' });

        let block = '<extension_tools>\n';
        block += `Current date and time: ${dateStr}, ${timeStr}\n\n`;
        block += 'You have access to local extension tools that run instantly on the user\'s machine.\n';
        block += 'To call one, emit a JSON block with this exact format:\n\n';
        block += '```extension_tool_call\n';
        block += '{"extension": "<extension_id>", "tool": "<tool_name>", "params": {<parameters>}}\n';
        block += '```\n\n';
        block += 'IMPORTANT: After emitting the tool call block, STOP generating and wait for the result.\n';
        block += 'The result will be provided as a follow-up message. Then continue your response.\n';
        block += 'Only call ONE tool at a time. Do not call multiple tools in a single message.\n\n';
        block += 'Available extension tools:\n\n';

        for (const def of defs) {
            block += `Extension: ${def.extensionId} (${def.extensionIcon} ${def.extensionName})\n`;
            for (const tool of def.tools) {
                block += `  - ${tool.name}: ${tool.description}\n`;
                if (tool.parameters && Object.keys(tool.parameters).length > 0) {
                    const paramDescs = Object.entries(tool.parameters)
                        .map(([k, v]) => `${k} (${v.type}${v.default !== undefined ? ', default: ' + v.default : ''})${v.description ? ' — ' + v.description : ''}`)
                        .join('; ');
                    block += `    Parameters: ${paramDescs}\n`;
                }
            }
            block += '\n';
        }

        block += '</extension_tools>';
        return block;
    }
}
