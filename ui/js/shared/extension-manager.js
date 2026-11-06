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

import { ExtensionSandboxPool, hostFeaturesFor } from './extension-sandbox-host.js';
import { applyManifestI18n, extensionDisplay } from './extension-manager/i18n.js';
import { installExtensionDateMethods } from './extension-manager/dates.js';
import { installExtensionSourceMethods } from './extension-manager/sources.js';
import { installExtensionSearchMethods } from './extension-manager/search.js';
import { installExtensionUiMethods } from './extension-manager/ui.js';
import { resolveExtensionConfig } from './extension-config.js';
export {
    applyManifestI18n,
    extensionDisplay,
    fetchExtensionLocaleViaInvoke,
    fetchSharedSourcesViaInvoke,
    localizeManifestForPrompt,
    resolveExtensionMessage,
} from './extension-manager/i18n.js';

/** Same capabilities regardless of order (both are already normalised). */
function sameCapabilitySet(a, b) {
    const x = [...(a || [])].sort();
    const y = [...(b || [])].sort();
    return x.length === y.length && x.every((cap, i) => cap === y[i]);
}

/** Config is JSON; object key order must not turn an unrelated save into a refresh. */
function extensionConfigSignature(config) {
    return JSON.stringify(config, (_key, value) =>
        value && typeof value === 'object' && !Array.isArray(value)
            ? Object.fromEntries(
                  Object.keys(value)
                      .sort()
                      .map((key) => [key, value[key]])
              )
            : value
    );
}

/**
 * Render tool definitions (getToolDefinitions() shape) as the agent-facing
 * steering block; '' when there are none. Agent-facing, so English. Reads
 * only the fields it renders and tolerates malformed sandbox data.
 * @param {object[]} defs
 * @returns {string}
 */
export function formatToolSteeringBlock(defs) {
    const list = (Array.isArray(defs) ? defs : []).filter(
        (d) => d && Array.isArray(d.tools) && d.tools.some(isToolEntry)
    );
    if (list.length === 0) return '';

    let block = '<extension_tools>\n';
    block += "You have access to local extension tools that run instantly on the user's machine.\n";
    block +=
        'This list is complete and replaces any extension tool list given earlier in this conversation.\n';
    block += 'To call one, emit a JSON block with this exact format:\n\n';
    block += '```extension_tool_call\n';
    block += '{"extension": "<extension_id>", "tool": "<tool_name>", "params": {<parameters>}}\n';
    block += '```\n\n';
    block +=
        'IMPORTANT: After emitting the tool call block, STOP generating and wait for the result.\n';
    block += 'The result will be provided as a follow-up message. Then continue your response.\n';
    block += 'Only call ONE tool at a time. Do not call multiple tools in a single message.\n\n';
    block += 'Available extension tools:\n\n';

    for (const def of list) {
        block += `Extension: ${def.extensionId} (${def.extensionIcon} ${def.extensionName})\n`;
        for (const tool of def.tools.filter(isToolEntry)) {
            block += `  - ${safeText(tool.name)}: ${safeText(tool.description)}\n`;
            const params =
                tool.parameters !== null && typeof tool.parameters === 'object'
                    ? Object.entries(tool.parameters)
                    : [];
            if (params.length > 0) {
                const paramDescs = params.map(([k, v]) => describeParam(k, v)).join('; ');
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

function isToolEntry(tool) {
    return tool !== null && typeof tool === 'object';
}

/** Steering text for one parameter declaration (a sandbox may send junk). */
function describeParam(name, spec) {
    const v = spec !== null && typeof spec === 'object' ? spec : {};
    const def = v.default !== undefined ? `, default: ${safeText(v.default)}` : '';
    const desc = v.description ? ` — ${safeText(v.description)}` : '';
    return `${name} (${safeText(v.type)}${def})${desc}`;
}

/**
 * Text for a sandbox-supplied value. Never throws: a structured-clone reply
 * can carry objects whose toString/valueOf aren't callable, which make
 * String() and template literals throw.
 */
function safeText(value) {
    if (typeof value === 'string') return value;
    try {
        return String(value);
    } catch {
        return '';
    }
}

/**
 * How many consecutive getTools() failures still report an extension's last
 * known tools (see getToolDefinitions).
 */
const MAX_REUSED_TOOL_FAILURES = 5;

export class ExtensionManager {
    /**
     * @param {Function} invoke - the raw Tauri invoke (the host, not the
     *   extension, will use it after permission checks).
     * @param {object} [opts]
     * @param {string[]} [opts.hostFeatures] - host UI features this window
     *   implements (e.g. `HOST_FEATURE_DATE_PICKER` in the floating
     *   window). Each extension is offered the subset its capabilities
     *   allow — see `hostFeaturesFor` in extension-sandbox-host.js.
     */
    constructor(invoke, opts = {}) {
        this.invoke = invoke;
        this._hostFeatures = Array.isArray(opts.hostFeatures) ? opts.hostFeatures.slice() : [];
        /** @type {Map<string, LoadedExtension>} */
        this.extensions = new Map();
        this._configCache = null;
        this._pool = new ExtensionSandboxPool(invoke);
        /** Synchronous snapshot of tool definitions; refreshed by getToolDefinitions(). */
        this._toolDefsCache = [];
        /** Steering text of `_toolDefsCache`, to tell a real change from a refresh. */
        this._toolDefsSignature = '';
        /** Bumped per getToolDefinitions() call; see there. */
        this._toolDefsGen = 0;
        this._toolDefsAppliedGen = 0;
        /** @type {Set<(defs: object[]) => void>} */
        this._toolDefsListeners = new Set();
        /** Extension id → consecutive getTools() failures. */
        this._toolFailures = new Map();
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
        // Source/locale reads can span config saves. The signature must describe
        // exactly the snapshot used to boot this sandbox.
        const config = this._getExtensionConfig(id, manifest);
        ext.configSignature = extensionConfigSignature(config);
        if (this._hasSandboxedProvider(sources)) {
            try {
                ext.sandbox = await this._pool.load({
                    extensionId: id,
                    capabilities,
                    config,
                    sources,
                    sharedSources,
                    vendorSources,
                    i18nCatalog: i18n.catalog,
                    i18nFallback: i18n.fallback,
                    i18nLanguage: i18n.language,
                    i18nRtl: i18n.rtl,
                    // Generic mascot activities the manifest declared. The
                    // sandbox host rejects any hint the extension didn't
                    // declare here.
                    mascotActivities: manifest.contributes?.mascotActivities,
                    // Host UI features this window offers, narrowed to what
                    // the granted capabilities allow (date picker ⇒ calendar).
                    hostFeatures: hostFeaturesFor(this._hostFeatures, capabilities),
                });
                ext.sandbox.onReformatRequest = () => this.requestReformat(id);
            } catch (e) {
                console.warn(`Sandbox boot failed for extension '${id}':`, e);
            }
        }

        await this._loadExtensionCss(id, manifest);

        this.extensions.set(id, ext);
        // Saves during sandbox boot or CSS loading couldn't reach this entry.
        // Once registered, reconcile it before returning to the caller.
        const currentConfig = this._getExtensionConfig(id, manifest);
        const currentSignature = extensionConfigSignature(currentConfig);
        if (currentSignature !== ext.configSignature) {
            ext.configSignature = currentSignature;
            await ext.sandbox?.updateConfig(currentConfig);
        }
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
        const previousConfig = this._configCache;
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
        const changedExtensions = new Set();
        for (const [id, ext] of this.extensions) {
            const config = this._getExtensionConfig(id, ext.manifest);
            const signature = extensionConfigSignature(config);
            const previousSignature =
                ext.configSignature ??
                extensionConfigSignature(resolveExtensionConfig(previousConfig, id, ext.manifest));
            if (signature !== previousSignature) changedExtensions.add(id);
            ext.configSignature = signature;
            if (ext.sandbox) {
                // A disabled extension keeps its sandbox loaded (so re-enable
                // is instant) but must not keep a mascot activity lease: its
                // widgets are unmounted, so nothing would re-assert it.
                if (!this._isEnabled(id)) ext.sandbox.clearMascotActivity?.();
                if (changedExtensions.has(id)) await ext.sandbox.updateConfig(config);
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
                const controller = this._widgetInstances?.get(key);
                if (controller) {
                    if (changedExtensions.has(id)) {
                        this._refreshWidgetSchedule(controller, true).catch((e) =>
                            console.warn(`Failed to refresh widget '${key}':`, e)
                        );
                    }
                    continue;
                }
                this._mountWidget(id, ext, w).catch((e) =>
                    console.warn(`Failed to remount widget '${key}':`, e)
                );
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
        const gen = ++this._toolDefsGen;
        const result = [];
        // Snapshot: a reload during the awaits below re-inserts an id at the
        // end of the live Map, which a live iteration would visit twice.
        for (const [id, ext] of [...this.extensions]) {
            if (!ext.sandbox?.hasTools) continue;
            if (!this._isEnabled(id)) continue;
            try {
                const tools = await ext.sandbox.call('getTools', {});
                this._toolFailures.delete(id);
                if (Array.isArray(tools) && tools.length > 0) {
                    const display = extensionDisplay(ext, id);
                    result.push({
                        extensionId: id,
                        extensionName: display.name,
                        extensionIcon: display.icon || '🧩',
                        tools,
                    });
                }
            } catch (e) {
                console.warn(`getTools() in '${id}' failed:`, e);
                // A failed call (timeout, sandbox busy) doesn't mean the
                // tools are gone. If the extension is still the same loaded,
                // enabled one, keep what it last reported, so a hiccup
                // doesn't tell the agent its tools were removed. Not
                // forever: a provider that keeps failing loses its tools.
                const failures = (this._toolFailures.get(id) || 0) + 1;
                this._toolFailures.set(id, failures);
                const previous = this._toolDefsCache.find((d) => d.extensionId === id);
                if (
                    previous &&
                    failures <= MAX_REUSED_TOOL_FAILURES &&
                    this.extensions.get(id) === ext &&
                    this._isEnabled(id)
                ) {
                    result.push(previous);
                }
            }
        }
        // Stable order, so the steering text depends on the tools only,
        // not on which extension was (re)loaded last.
        result.sort((a, b) =>
            a.extensionId < b.extensionId ? -1 : a.extensionId > b.extensionId ? 1 : 0
        );
        // Calls overlap (a config update, a reload, and a tool call can all
        // ask at once). Only a newer call than the last one applied may
        // replace the cache, so a slow stale result can't flip it back.
        if (gen > this._toolDefsAppliedGen) {
            this._toolDefsAppliedGen = gen;
            this._toolDefsCache = result;
            try {
                this._noteToolDefinitions(result);
            } catch (e) {
                // Never fail the caller (tool calls await this) over a
                // change notification.
                console.warn('Tool-definitions change check failed:', e);
            }
        }
        return result;
    }

    /**
     * Subscribe to changes in the tool definitions the agent is told about —
     * a setting that turns an extension's agent tools on, an extension
     * enabled, disabled, installed or removed, or getTools() simply
     * answering differently. Compared by their steering text, so it fires
     * only when that text would change, never for a refresh that found the
     * same tools.
     * @param {(defs: object[]) => void} listener
     * @returns {() => void} unsubscribe
     */
    onToolDefinitionsChanged(listener) {
        this._toolDefsListeners.add(listener);
        return () => this._toolDefsListeners.delete(listener);
    }

    _noteToolDefinitions(defs) {
        // The rendered text, not JSON: it covers exactly what the agent
        // sees, and can't throw on the cycles or BigInts a sandbox's
        // structured-clone reply may carry.
        const signature = formatToolSteeringBlock(defs);
        if (signature === this._toolDefsSignature) return;
        this._toolDefsSignature = signature;
        for (const listener of [...this._toolDefsListeners]) {
            try {
                listener(defs);
            } catch (e) {
                console.warn('Tool-definitions listener failed:', e);
            }
        }
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

        let timer;
        const timeoutPromise = new Promise((_, reject) => {
            timer = setTimeout(
                () =>
                    reject(
                        new Error(`Extension tool timed out (${Math.round(timeoutMs / 1000)}s)`)
                    ),
                timeoutMs
            );
        });
        try {
            return await Promise.race([
                // The RPC layer's own 10s default would otherwise fire first
                // for tools declaring longer timeouts. Give it a little slack
                // so the race above reports the friendlier tool-level error.
                ext.sandbox.call(
                    'executeTool',
                    { toolName, params },
                    { timeoutMs: timeoutMs + 1000 }
                ),
                timeoutPromise,
            ]);
        } catch (e) {
            return { error: e?.message || String(e) };
        } finally {
            clearTimeout(timer);
        }
    }

    /**
     * Steering text describing available extension tools to the LLM.
     * Resolves to the empty string if no extensions expose tools.
     * @param {object[]} [defs] - render these instead of fetching fresh
     *   ones (e.g. `getToolDefinitionsCached()`).
     */
    async buildToolSteeringBlock(defs) {
        return formatToolSteeringBlock(defs ?? (await this.getToolDefinitions()));
    }

    // --- Trigger providers -------------------------------------------------

    async getTriggerDefinitions() {
        const defs = [];
        for (const [id, ext] of this.extensions) {
            if (!ext.sandbox?.hasTriggers) continue;
            try {
                const triggers = await ext.sandbox.call('getTriggers', {});
                if (Array.isArray(triggers) && triggers.length > 0) {
                    const display = extensionDisplay(ext, id);
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
                    const sameVersion = existing.manifest?.version === item.manifest.version;
                    // Capabilities are baked into the sandbox at load, so a
                    // grant that changed at the same version (welcome flow,
                    // re-approval) needs a reload too — otherwise the window
                    // keeps blocking calls the user has since approved until
                    // the app restarts.
                    const sameCaps = sameCapabilitySet(
                        existing.capabilities,
                        this._computeGrantedCapabilities(item.manifest).capabilities
                    );
                    if (sameVersion && sameCaps) continue;
                    console.log(
                        sameVersion
                            ? `ExtensionManager: capabilities changed for '${item.manifest.id}', reloading`
                            : `ExtensionManager: updating '${item.manifest.id}' from ${existing.manifest?.version} to ${item.manifest.version}`
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

        // The roster may have changed (install, uninstall, enable, update):
        // refresh the tool definitions so the cache is current and
        // onToolDefinitionsChanged listeners (agent steering) hear about it.
        try {
            await this.getToolDefinitions();
        } catch (e) {
            console.warn('Tool-defs refresh after reload failed:', e);
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
        if (this._widgetVisibilityListener) {
            document.removeEventListener('visibilitychange', this._widgetVisibilityListener);
            this._widgetVisibilityListener = null;
        }
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
installExtensionDateMethods(ExtensionManager);
