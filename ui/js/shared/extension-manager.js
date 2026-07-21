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

import { ExtensionSandboxPool } from './extension-sandbox-host.js';
import { applyManifestI18n } from './extension-manager/i18n.js';
import { installExtensionSourceMethods } from './extension-manager/sources.js';
import { installExtensionSearchMethods } from './extension-manager/search.js';
import { installExtensionUiMethods } from './extension-manager/ui.js';
export {
    applyManifestI18n,
    fetchExtensionLocaleViaInvoke,
    fetchSharedSourcesViaInvoke,
    localizeManifestForPrompt,
    resolveExtensionMessage,
} from './extension-manager/i18n.js';

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
        // Keep the resolved catalog around so host-side machinery that needs
        // to localise extension-supplied i18n keys (e.g. search-keyword hint
        // labels — see getKeywordDefinitions) can reuse it without another
        // locale read.
        ext.i18n = i18n;
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

    // --- Config updates ----------------------------------------------------

    /**
     * Single-flight + trailing rerun, same shape as reload(): rapid
     * config saves (slider drags, multi-field settings apply) fire one
     * config_updated per save, and each onConfigUpdate pass costs a
     * get_config IPC plus a per-extension updateConfig postMessage
     * fan-out. Coalescing keeps exactly one pass in flight and one
     * trailing pass to pick up the final state.
     */
    async onConfigUpdate() {
        if (this._configUpdateInFlight) {
            this._configUpdatePending = true;
            return this._configUpdateInFlight;
        }
        this._configUpdateInFlight = (async () => {
            try {
                do {
                    this._configUpdatePending = false;
                    await this._onConfigUpdateOnce();
                } while (this._configUpdatePending);
            } finally {
                this._configUpdateInFlight = null;
            }
        })();
        return this._configUpdateInFlight;
    }

    async _onConfigUpdateOnce() {
        try {
            this._configCache = await this.invoke('get_config');
        } catch {
            return;
        }
        // Invalidate caches whose contents depend on extension config.
        this._customRenderCache?.clear();
        this._toolbarButtonsCache = null;
        // Keyword hints can key off a user-configured trigger, so a config
        // change may change the registered words — drop the cache.
        this._keywordDefsCache = null;
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
                    const display = ext.localizedManifest || ext.manifest;
                    result.push({
                        extensionId: id,
                        extensionName: display.name || id,
                        extensionIcon: display.icon || '🧩',
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
                    const display = ext.localizedManifest || ext.manifest;
                    defs.push({
                        extensionId: id,
                        extensionName: display.name,
                        extensionIcon: display.icon || '🔌',
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
     *
     * Serialized + coalesced. reload() is async with awaits throughout, and
     * several signals can fire it near-simultaneously (extensions_changed,
     * config_updated, settings hot-reload). Without a guard, two reloads enter
     * concurrently, both observe the pre-reload `extensions` map, and both try
     * to load the same id — producing "sandbox already loaded" errors and
     * duplicate installs. We run at most one reload at a time; any requests
     * arriving while one is in flight collapse into a single trailing rerun
     * (the roster is read fresh at the start of each run, so one rerun
     * captures everything that changed during the previous one).
     */
    async reload() {
        if (this._reloadInFlight) {
            // A reload is running; remember that state changed again and
            // return the in-flight promise. One trailing rerun suffices.
            this._reloadPending = true;
            return this._reloadInFlight;
        }
        this._reloadInFlight = (async () => {
            try {
                do {
                    this._reloadPending = false;
                    await this._reloadOnce();
                } while (this._reloadPending);
            } finally {
                this._reloadInFlight = null;
            }
        })();
        return this._reloadInFlight;
    }

    async _reloadOnce() {
        try {
            this._configCache = await this.invoke('get_config');
        } catch {
            return;
        }

        // The set of loaded extensions may change below; drop the keyword
        // hint cache so it's rebuilt from the new roster on next query.
        this._keywordDefsCache = null;

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

installExtensionSourceMethods(ExtensionManager);
installExtensionSearchMethods(ExtensionManager);
installExtensionUiMethods(ExtensionManager);
