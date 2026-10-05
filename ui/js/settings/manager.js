/**
 * Settings Manager
 * Coordinates all settings modules and handles save/load operations.
 */

import { onConfigChange } from '../shared/config-cache.js';
import { errLabel } from '../shared/error-message.js';
import {
    applyManifestI18n,
    fetchExtensionLocaleViaInvoke,
    fetchSharedSourcesViaInvoke,
} from '../shared/extension-manager.js';
import { ExtensionSandboxPool } from '../shared/extension-sandbox-host.js';
import { normalizePermissions } from '../shared/extension-permissions.js';
import { renderSchema } from '../shared/settings-renderer.js';
import { t } from '../shared/i18n.js';
import { escapeAttr, escapeHtml } from '../shared/tool-utils.js';
import { SettingsModule } from './base.js';
import { DIRTY_EVENT_TYPES, DirtyTracker, dirtySectionForEvent } from './dirty-tracker.js';
import { renderCapabilityBadges } from './extension-capabilities.js';
import { registerSettingsActions, setSettingsManager } from './module-registry.js';

// --- Sandboxed extension settings ------------------------------------------
//
// Extension settings run in the same iframe sandbox as search/tool/trigger
// providers. They declare their UI as a JSON schema and handle action
// button RPCs. See docs/EXTENSIONS.md for the contract.
//
// This adapter wraps an ExtensionSandbox + RenderedSettings pair behind
// the same interface the legacy `SettingsModule` base class exposed, so
// the rest of SettingsManager treats it like any other module.

export class SandboxedExtensionSettingsModule {
    constructor({ extensionId, manifest, sandbox, rendered, capabilities }) {
        this._extensionId = extensionId;
        this._extensionVersion = manifest.version || '';
        this._capabilities = capabilities;
        this._legacyPermissions = false; // enforced: no legacy path in sandbox mode
        this._sandbox = sandbox;
        this._rendered = rendered;
        this.id = `ext-${extensionId}`;
        this.title = manifest.name || extensionId;
        this.icon = manifest.icon || '📦';
        this.description = manifest.description || '';
    }

    renderContent() {
        // The rendered settings already wrote into this._rendered.container,
        // but that container is populated AFTER the manager's render pass —
        // so we return a stable placeholder div and mount into it later.
        // See the custom `_mountSandboxModules()` pass below.
        return `<div id="ext-sandbox-slot-${this._extensionId}"></div>`;
    }

    render() {
        return this.renderContent();
    }

    load(config) {
        const stored = config.extensions?.[this._extensionId] || {};
        this._rendered.load(stored);
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions[this._extensionId] = this._rendered.save();
    }

    async validate() {
        return this._rendered.validate();
    }

    initialize() {
        /* event wiring happens inside RenderedSettings */
    }

    destroy() {
        try {
            this._rendered.destroy();
        } catch {}
    }
}

/**
 * Given a manifest and its source paths, boot a sandbox for settings
 * rendering, fetch the declared schema, and build the adapter module.
 */
export async function buildSandboxedSettingsModule({
    invoke,
    pool,
    manifest,
    capabilities,
    settingsProviderSource,
    currentConfig,
}) {
    // Collect sources: only the settings provider is needed for the
    // settings window. Search/tool/trigger providers are already loaded
    // in the floating/chat windows' own sandbox pools.
    const sources = { settingsProvider: settingsProviderSource };

    // Walk relative imports in the settings provider so siblings like
    // `./auth.js` (Spotify uses this for OAuth helpers) can resolve
    // inside the sandbox. Without this the sandbox's blob-URL
    // registry has nothing for the sibling specifier and the import
    // fails with "Failed to resolve module specifier './auth.js'."
    // Skipping the call when there's no `invoke` keeps the function
    // callable from the legacy bootstrap path that doesn't have one.
    const sharedSources = invoke
        ? await fetchSharedSourcesViaInvoke(invoke, manifest.id, sources)
        : undefined;

    // Extension config values the provider should see.
    const extConfig = currentConfig?.extensions?.[manifest.id] || {};

    // Fetch the extension's _locales/ catalog and apply __MSG_*__ token
    // resolution to the manifest. Without this the section header,
    // sidebar entry, and capability description all rendered raw tokens
    // ("__MSG_manifest.name__"), and the sandbox runtime's `t()` proxy
    // returned bare keys ("settings.show_overlay.label") because the
    // catalog wasn't seeded. The extension manager applies the same
    // pair (manifest tokens + sandbox catalog) for runtime extensions;
    // settings was missing both.
    const i18n = invoke
        ? await fetchExtensionLocaleViaInvoke(invoke, manifest)
        : { catalog: {}, fallback: {}, language: 'en', rtl: false };
    const localizedManifest = applyManifestI18n(manifest, i18n.catalog, i18n.fallback);

    const sandbox = await pool.load({
        extensionId: manifest.id,
        capabilities,
        config: extConfig,
        sources,
        sharedSources,
        i18nCatalog: i18n.catalog,
        i18nFallback: i18n.fallback,
        i18nLanguage: i18n.language,
        i18nRtl: i18n.rtl,
    });

    if (!sandbox.hasSettings) {
        pool.unload(manifest.id);
        throw new Error(
            `extension '${manifest.id}' declared a settingsProvider but the sandbox didn't report one`
        );
    }

    const schema = await sandbox.call('getSettings', {});
    if (!schema || typeof schema !== 'object') {
        pool.unload(manifest.id);
        throw new Error(`extension '${manifest.id}' getSettings() returned nothing`);
    }

    const container = document.createElement('div');
    const rendered = renderSchema({
        extensionId: manifest.id,
        schema,
        container,
        sandbox,
    });

    return new SandboxedExtensionSettingsModule({
        extensionId: manifest.id,
        manifest: localizedManifest,
        sandbox,
        rendered,
        capabilities,
    });
}

export class SettingsManager {
    constructor() {
        this.modules = [];
        this.invoke = window.__TAURI__.core.invoke;
        this.appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();

        // The settings window's lifetime is short (modal-ish: opened to
        // change a thing, closed when done) and it has no streaming /
        // typed-state to lose. Reloading it is the cleanest way to pick
        // up a language change — full re-render cycle for the manager
        // would also need to re-mount every sandboxed extension iframe,
        // re-bind every per-module event listener, and preserve the
        // active section. A page reload does all that for free, and the
        // language change has just been persisted by save() so the new
        // catalog is what the reloaded window fetches at startup.
        document.addEventListener('kage:i18n-changed', () => {
            // The hash + section query param survive the reload, so the
            // user stays on the section they were on.
            window.location.reload();
        });

        // Unsaved-edit tracking. Edits only persist on the global Save, so
        // a section with edits must never be reloaded from config, and a
        // section without edits must never be written back (its DOM may be
        // stale versus changes made from other windows). Capture phase on
        // document so module handlers that stopPropagation() can't hide an
        // edit from us.
        this._dirty = new DirtyTracker();
        // Bumped whenever config is (re)applied or saved, so a refresh whose
        // get_config was issued earlier can't apply an older snapshot.
        this._refreshGen = 0;
        const onEdit = (event) => {
            const id = dirtySectionForEvent(event);
            if (id) this.markDirty(id);
        };
        for (const type of DIRTY_EVENT_TYPES) {
            document.addEventListener(type, onEdit, true);
        }

        // Changes made outside this window (shortcut/automation added from
        // the floating window, grants changed, ...) broadcast config_updated.
        // Refresh the visible section if it has no unsaved edits; hidden
        // sections refresh when they're next shown (switchSection), and
        // clean sections are never written by save(), so none can put a
        // stale copy over the newer config. Debounced because a burst of
        // writes (and the echo of our own Save) arrives back-to-back.
        this._configRefreshTimer = null;
        onConfigChange(() => {
            if (this._configRefreshTimer) clearTimeout(this._configRefreshTimer);
            this._configRefreshTimer = setTimeout(() => {
                this._configRefreshTimer = null;
                const active = this._activeModule();
                if (active) this._refreshIfClean(active);
            }, 150);
        });
    }

    /** Flag a module as holding unsaved edits (see the constructor). */
    markDirty(moduleId) {
        const module = this.modules.find((m) => m.id === moduleId);
        if (!module || module.persistsImmediately) return;
        this._dirty.mark(moduleId);
    }

    isDirty(moduleId) {
        return this._dirty.isDirty(moduleId);
    }

    /** The module whose section is currently visible, if any. */
    _activeModule() {
        const section = document.querySelector('[data-section-content]:not(.hidden)');
        const id = section?.dataset.sectionContent;
        return id ? this.modules.find((m) => m.id === id) || null : null;
    }

    /**
     * Reload one module from fresh config unless it holds unsaved edits.
     * Re-checks after the get_config round trip: the user may have started
     * typing meanwhile, or a newer refresh / save may have superseded it.
     */
    async _refreshIfClean(module) {
        if (this._dirty.isDirty(module.id)) return;
        const gen = ++this._refreshGen;
        let config;
        try {
            config = await this.invoke('get_config');
        } catch (e) {
            console.warn(`[Settings] Refresh of ${module.id} failed:`, e);
            return;
        }
        if (gen !== this._refreshGen) return;
        if (this._dirty.isDirty(module.id) || !this.modules.includes(module)) return;
        // Callers fire-and-forget this, so a throwing load() must not become
        // an unhandled rejection.
        try {
            this._applyModuleConfig(module, config);
        } catch (e) {
            console.error(`Settings module ${module.id} load failed:`, e);
        }
    }

    /**
     * Register a settings module. Accepts either a legacy
     * SettingsModule subclass (used by first-party modules) or a
     * SandboxedExtensionSettingsModule (used by all extensions).
     */
    registerModule(module) {
        const isLegacy = module instanceof SettingsModule;
        const isSandboxed = module instanceof SandboxedExtensionSettingsModule;
        if (!isLegacy && !isSandboxed) {
            throw new Error(
                'Module must extend SettingsModule or SandboxedExtensionSettingsModule'
            );
        }
        this.modules.push(module);
    }

    /**
     * Render all registered modules
     */
    render() {
        const container = document.getElementById('settingsModules');
        if (!container) {
            console.error('Settings modules container not found');
            return;
        }

        // Each module gets its own section, keyed by module ID.
        // The first module ('appearance') is visible by default; the rest are hidden.
        let html = '';
        this.modules.forEach((module, index) => {
            const hidden = index === 0 ? '' : ' hidden';
            html += `<div class="settings-section${hidden}" data-section-content="${module.id}">`;
            if (module._extensionId) {
                const extId = escapeAttr(module._extensionId);
                // Framework-owned header: icon + title on left, enable/disable button on right.
                // icon/title/description come from the (untrusted) extension manifest and
                // this window has full __TAURI__ access — always escape.
                html += `<h2 class="settings-section-header ext-section-header">
                    <span>${escapeHtml(module.icon)} ${escapeHtml(module.title)}</span>
                    <button class="setting-button" id="ext-toggle-btn-${extId}" style="min-width:80px;font-size:12px;" data-action="toggleExtension" data-arg="${extId}">Disable</button>
                    <input type="hidden" id="ext-enabled-${extId}" value="true">
                </h2>`;
                if (module.description) {
                    html += `<p style="font-size:12px;color:var(--kage-text-muted);margin:0 0 16px;line-height:1.4;">${escapeHtml(module.description)}</p>`;
                }
                // Capability badges — visible surface of the extension permission system
                html += renderCapabilityBadges(module._capabilities, module._legacyPermissions);
                html += `<div id="ext-content-${extId}">`;
                html += module.renderContent ? module.renderContent() : module.render();
                html += `</div>`;
            } else {
                html += module.render();
            }
            html += `</div>`;
        });

        container.innerHTML = html;

        // Mount sandboxed-extension-settings rendered containers into their
        // placeholder slots. The renderer wrote into a floating div we
        // created earlier; we just move those children into the live DOM.
        this.modules.forEach((module) => {
            if (module instanceof SandboxedExtensionSettingsModule) {
                const slot = document.getElementById(`ext-sandbox-slot-${module._extensionId}`);
                if (slot && module._rendered?.container) {
                    while (module._rendered.container.firstChild) {
                        slot.appendChild(module._rendered.container.firstChild);
                    }
                    // Subsequent writes (e.g. load()) go through the renderer,
                    // which still holds references to the now-moved DOM
                    // nodes — querySelector calls work because we moved the
                    // actual nodes, not copies. But for future renders the
                    // renderer looks up by id scoped to its container; swap
                    // the container reference to the slot so lookups still
                    // succeed after the move.
                    module._rendered.container = slot;
                }
            }
        });

        // Initialize the visible section eagerly; the rest are
        // initialised lazily on first reveal in `switchSection`.
        // Several initialise() impls do `await import(...)` of heavy
        // helpers (mascot for About, mermaid/graphviz for code-block
        // demos in Appearance, etc.); doing them all up-front made the
        // settings window's first paint slow even though the user only
        // looks at one section at a time.
        this._initialized = new Set();
        if (this.modules.length > 0) {
            this._initializeModule(this.modules[0]);
        }
    }

    /**
     * Run a module's initialize() the first time it's needed; no-op on
     * subsequent calls. Tolerates async initialize() implementations —
     * the returned promise is awaitable but most callers fire-and-forget.
     */
    _initializeModule(module) {
        if (this._initialized.has(module.id)) return false;
        this._initialized.add(module.id);
        // Some initialize() impls build the widgets load() populates (e.g.
        // the hotkey pickers), so re-load just this module once init has
        // finished. _loadModule skips it if the user already started editing
        // while an async initialize() was in flight.
        const reload = () =>
            this._loadModule(module).catch((e) =>
                console.error(`Settings module ${module.id} load failed:`, e)
            );
        try {
            const result = module.initialize();
            if (result && typeof result.then === 'function') {
                result.then(reload, (e) => {
                    // Don't unset _initialized — a busted initialize will keep
                    // throwing on every reveal otherwise. Surface the error
                    // and leave the section in whatever state it reached.
                    console.error(`Settings module ${module.id} initialize failed:`, e);
                });
            } else {
                reload();
            }
        } catch (e) {
            console.error(`Settings module ${module.id} initialize failed:`, e);
        }
        return true;
    }

    /** Load a single module (plus its extension enabled state) from saved config. */
    async _loadModule(module) {
        const config = await this.invoke('get_config');
        if (this._dirty.isDirty(module.id)) return;
        this._applyModuleConfig(module, config);
    }

    _applyModuleConfig(module, config) {
        module.load(config);
        // Load extension enabled state
        if (module._extensionId) {
            const extId = module._extensionId;
            const states = config.extension_states || {};
            const enabled = states[extId] !== false;
            const hiddenInput = document.getElementById('ext-enabled-' + extId);
            if (hiddenInput) hiddenInput.value = enabled ? 'true' : 'false';
            _updateExtToggleUI(extId, enabled);
        }
    }

    /**
     * Switch to a different section
     */
    switchSection(sectionId) {
        // Update sidebar active state
        document.querySelectorAll('.sidebar-item').forEach((item) => {
            if (item.dataset.section === sectionId) {
                item.classList.add('active');
            } else {
                item.classList.remove('active');
            }
        });

        // Show/hide section content
        document.querySelectorAll('[data-section-content]').forEach((section) => {
            if (section.dataset.sectionContent === sectionId) {
                section.classList.remove('hidden');
            } else {
                section.classList.add('hidden');
            }
        });

        // Lazy initialise: most settings modules only need to wire up
        // their event listeners + load() once, the first time the user
        // navigates to them. See render() for the rationale.
        // On later reveals, reload the section from fresh config so changes
        // made elsewhere show up - but only if it holds no unsaved edits:
        // edits persist only on the global Save, and reloading a dirty
        // section silently reverted them. A dirty section gets onShow()
        // instead, to refresh backend-derived data (models, update status)
        // while keeping the edits; for a clean one load() covers that.
        const targetModule = this.modules.find((m) => m.id === sectionId);
        if (targetModule && !this._initializeModule(targetModule)) {
            if (!this._dirty.isDirty(targetModule.id)) {
                this._refreshIfClean(targetModule);
            } else {
                try {
                    targetModule.onShow?.();
                } catch (e) {
                    console.error(`Settings module ${targetModule.id} onShow failed:`, e);
                }
            }
        }

        // Reset scroll to top
        const content = document.querySelector('.settings-content');
        if (content) content.scrollTop = 0;
    }

    /**
     * Load settings from backend
     */
    async load() {
        try {
            const config = await this.invoke('get_config');
            // A full load is authoritative (boot, extension re-render, backup
            // import): every section now mirrors config, so none is dirty,
            // and any in-flight per-module refresh is superseded.
            this._refreshGen++;
            this._dirty.clearAll();
            this.modules.forEach((module) => {
                this._applyModuleConfig(module, config);
            });
        } catch (error) {
            this.showStatus(errLabel(t('settings.manager.error.failed_load'), error), 'error');
            throw error;
        }
    }

    /**
     * Save settings to backend
     */
    async save() {
        try {
            // Only modules with unsaved edits are validated and written.
            // `config` below is fresh from get_config, so it already holds
            // the newest values for every clean module - calling their
            // save() would overwrite those with this window's possibly
            // stale DOM (e.g. drop a shortcut added from the floating
            // window since this section was last shown). Snapshot the dirty
            // set up front so edits made while the save is in flight stay
            // dirty afterwards.
            const dirtySnapshot = this._dirty.snapshot();
            const dirtyModules = this.modules.filter((m) => dirtySnapshot.has(m.id));

            // Validate (legacy sync, sandboxed async)
            for (const module of dirtyModules) {
                const raw = module.validate();
                const validation = raw && typeof raw.then === 'function' ? await raw : raw;
                if (!validation || typeof validation !== 'object' || !('valid' in validation)) {
                    this.showStatus(
                        `[${module.title}] validate() must return { valid: true/false, error?: string }`,
                        'error'
                    );
                    return false;
                }
                if (!validation.valid) {
                    this.showStatus(
                        t('settings.manager.validation.module_label', {
                            title: module.title,
                            error:
                                validation.error || t('settings.manager.error.validation_failed'),
                        }),
                        'error'
                    );
                    return false;
                }
            }

            // Start from the current config so fields not owned by any module
            // (e.g. first_run_completed) are preserved across saves.
            //
            // Do NOT touch `config.version` — the backend bumps it inside
            // `Config::load`'s migration runner, and overwriting it here
            // makes the next launch re-migrate from scratch. The 2→3
            // migration interprets a v1-stamped config with
            // first_run_completed=true as a pre-telemetry user and
            // force-disables their opt-in. So a "harmless" `version = 1`
            // here was silently flipping telemetry off on every Settings
            // save. Trust whatever value `get_config` already returned.
            const config = await this.invoke('get_config');
            dirtyModules.forEach((module) => {
                module.save(config);
                // Save extension enabled state
                if (module._extensionId) {
                    if (!config.extension_states) config.extension_states = {};
                    const el = document.getElementById('ext-enabled-' + module._extensionId);
                    if (el) {
                        config.extension_states[module._extensionId] = el.value === 'true';
                    }
                }
            });

            // Save to backend. Still done when nothing is dirty so Save keeps
            // re-applying runtime config exactly as before.
            await this.invoke('save_config', { config });
            this._dirty.clearSaved(dirtySnapshot);
            // A refresh that fetched config before this write must not
            // apply it over the values just saved.
            this._refreshGen++;

            // Check if any module needs a restart
            const needsRestart = this.modules.some((m) => m._needsRestart);
            if (needsRestart) {
                // Reset the flag so it doesn't trigger again on next save
                this.modules.forEach((m) => {
                    m._needsRestart = false;
                });
                // Inline banner with a "Restart now" button — no native
                // dialog. Native dialogs steal focus, block the settings
                // window, and feel alien on Windows; an inline banner keeps
                // the user in the same surface and lets them keep
                // adjusting other settings if they're not done.
                this.showRestartPrompt();
                // Return false so saveAndClose doesn't immediately close
                // the window — the user needs to see and act on the
                // restart prompt. The save itself succeeded; we just
                // don't want to dismiss the surface that hosts the prompt.
                return false;
            }

            this.showStatus(t('settings.manager.status.saved'), 'success');
            return true;
        } catch (error) {
            console.error('[Settings] Save failed:', error);
            const msg =
                typeof error === 'string'
                    ? error
                    : error?.message ||
                      error?.toString() ||
                      JSON.stringify(error) ||
                      t('settings.manager.status.unknown_error');
            this.showStatus(t('settings.manager.status.save_failed', { message: msg }), 'error');
            return false;
        }
    }

    /**
     * Show status message
     * @param {string} message - The message to display
     * @param {string} type - 'success' or 'error'
     */
    showStatus(message, type) {
        const statusEl = document.getElementById('statusMessage');
        if (!statusEl) return;

        // Cancel any prior auto-hide so a stale timer can't fire mid-read —
        // showStatus and showRestartPrompt share this element, and an
        // uncancelled timer from an earlier "Saved" could hide the persistent
        // restart prompt out from under the user.
        if (this._statusHideTimer) clearTimeout(this._statusHideTimer);

        statusEl.textContent = message;
        statusEl.className = 'status-message ' + type;
        statusEl.style.display = 'block';

        this._statusHideTimer = setTimeout(() => {
            statusEl.style.display = 'none';
            this._statusHideTimer = null;
        }, 5000);
    }

    /**
     * Render the post-save "restart required" banner. Persists until the
     * user clicks Restart now or dismisses — auto-dismiss would lose the
     * call to action.
     */
    showRestartPrompt() {
        const statusEl = document.getElementById('statusMessage');
        if (!statusEl) return;
        // Cancel any pending showStatus auto-hide — this banner is persistent
        // and must not be dismissed by a timer armed for an earlier message.
        if (this._statusHideTimer) {
            clearTimeout(this._statusHideTimer);
            this._statusHideTimer = null;
        }
        // Build inline. Don't use innerHTML interpolation for the user-
        // facing text — t() returns trusted catalog strings, but going
        // through DOM API keeps the buttons properly wired.
        statusEl.textContent = '';
        statusEl.className = 'status-message restart-prompt';
        statusEl.style.display = 'flex';

        const message = document.createElement('span');
        message.textContent = t('settings.manager.status.saved_restart_needed');
        message.className = 'restart-prompt-text';

        const restartBtn = document.createElement('button');
        restartBtn.type = 'button';
        restartBtn.className = 'restart-prompt-btn restart-prompt-btn-primary';
        restartBtn.textContent = t('settings.manager.dialog.restart.now_btn');
        restartBtn.addEventListener('click', () => {
            this.invoke('restart_app');
        });

        const dismissBtn = document.createElement('button');
        dismissBtn.type = 'button';
        dismissBtn.className = 'restart-prompt-btn';
        dismissBtn.textContent = t('settings.manager.dialog.restart.later_btn');
        dismissBtn.addEventListener('click', () => {
            statusEl.style.display = 'none';
        });

        statusEl.appendChild(message);
        statusEl.appendChild(restartBtn);
        statusEl.appendChild(dismissBtn);
    }

    /**
     * Close settings window
     */
    close() {
        this.appWindow.close();
    }

    /**
     * Cleanup all modules
     */
    destroy() {
        this.modules.forEach((module) => module.destroy());
        this.modules = [];
    }
}

/**
 * Add a sidebar item dynamically to the Extensions section, in alphabetical order.
 * Static items (store, integration, shortcuts) stay at the top.
 */
export function addExtensionSidebarItem(id, icon, label) {
    const section = document.getElementById('extensionsSidebarSection');
    if (!section) return;
    // Don't add duplicates
    if (section.querySelector(`.sidebar-item[data-section="${id}"]`)) return;

    const item = document.createElement('div');
    item.className = 'sidebar-item';
    item.dataset.section = id;
    item.dataset.extSidebar = 'true'; // mark as dynamic extension item
    item.dataset.action = 'switchSection';
    item.dataset.arg = id;
    const iconSpan = document.createElement('span');
    iconSpan.className = 'sidebar-item-icon';
    iconSpan.textContent = icon;
    const labelSpan = document.createElement('span');
    labelSpan.textContent = label;
    item.appendChild(iconSpan);
    item.appendChild(labelSpan);

    // Insert alphabetically among other dynamic extension items
    const extItems = [...section.querySelectorAll('.sidebar-item[data-ext-sidebar="true"]')];
    const lowerLabel = label.toLowerCase();
    const insertBefore = extItems.find((el) => {
        const elLabel = el.querySelector('span:last-child')?.textContent?.toLowerCase() || '';
        return elLabel > lowerLabel;
    });

    if (insertBefore) {
        section.insertBefore(item, insertBefore);
    } else {
        section.appendChild(item);
    }
}

// --- Sandbox pool helpers ---------------------------------------------------

export function createSandboxPool(invoke) {
    return new ExtensionSandboxPool(invoke);
}

export function normalizeExtensionPermissions(permissions, id) {
    return normalizePermissions(permissions, id);
}

// --- Manager-owned action handlers -----------------------------------------

function _updateExtToggleUI(extId, enabled) {
    const btn = document.getElementById('ext-toggle-btn-' + extId);
    const content = document.getElementById('ext-content-' + extId);
    if (btn) {
        btn.textContent = enabled ? 'Disable' : 'Enable';
        btn.style.background = enabled ? 'var(--kage-error)' : 'var(--kage-accent)';
        btn.style.color = 'white';
        btn.style.border = 'none';
    }
    if (content) {
        content.style.opacity = enabled ? '' : '0.4';
        content.style.pointerEvents = enabled ? '' : 'none';
    }
}

/**
 * Wire the manager-owned actions (saveAndClose, switchSection, toggle, etc.)
 * into the delegated dispatcher. Must be called after `setSettingsManager`
 * so the handlers can find the live manager.
 */
export function registerManagerActions() {
    registerSettingsActions({
        switchSection: (sectionId) => {
            const mgr = _getMgr();
            if (mgr) mgr.switchSection(sectionId);
        },
        saveAndClose: async () => {
            const mgr = _getMgr();
            if (!mgr) return;
            const success = await mgr.save();
            if (success) mgr.close();
        },
        closeSettings: () => {
            const mgr = _getMgr();
            if (mgr) mgr.close();
        },
        toggleExtension: (extId) => {
            const hiddenInput = document.getElementById('ext-enabled-' + extId);
            if (!hiddenInput) return;
            const nowEnabled = hiddenInput.value !== 'true';
            hiddenInput.value = nowEnabled ? 'true' : 'false';
            _updateExtToggleUI(extId, nowEnabled);
        },
        openStore: () => {
            if (window.__TAURI__?.core) {
                window.__TAURI__.core.invoke('open_store_window', { tab: 'extensions' });
            }
        },
    });
}

// Late-bound lookup so the actions registered above always see the
// current manager (the dispatcher is installed before the manager is
// constructed).
function _getMgr() {
    // Imported lazily to avoid a circular import on module init.
    return _settingsManagerHandle();
}

let _settingsManagerHandle = () => null;

/**
 * Test/setup hook so callers (main.js) can hand the manager to the
 * action wiring without going through a window global.
 */
export function setManagerHandle(getter) {
    _settingsManagerHandle = getter;
    setSettingsManager(getter());
}
