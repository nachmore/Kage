/**
 * Settings Manager
 * Coordinates all settings modules and handles save/load operations.
 */

import { errLabel } from '../shared/error-message.js';
import {
    applyManifestI18n,
    fetchExtensionLocaleViaInvoke,
    fetchSharedSourcesViaInvoke,
} from '../shared/extension-manager.js';
import { ExtensionSandboxPool } from '../shared/extension-sandbox-host.js';
import { normalizePermissions } from '../shared/extension-permissions.js';
import { renderSchema } from '../shared/settings-renderer.js';
import { escapeAttr, escapeHtml } from '../shared/tool-utils.js';
import { t } from '../shared/i18n.js';
import { SettingsModule } from './base.js';
import { registerSettingsActions, setSettingsManager } from './module-registry.js';

// Capability → icon. Labels and descriptions resolve through i18n at
// render time so language switches reflect immediately. Keep in sync with
// ui/js/shared/extension-permissions.js.
const CAPABILITY_ICONS = Object.freeze({
    storage: '💾',
    clipboard: '📋',
    urls: '🔗',
    launch: '🚀',
    network: '📡',
    oauth: '🔐',
    filesystem: '📂',
    window: '🪟',
    windows: '🧿',
    notifications: '🔔',
    calendar: '📅',
    session: '💬',
    agent: '🤖',
    activity: '📊',
    automation: '⚡',
    tts: '🔈',
});

function capabilityLabel(cap) {
    return CAPABILITY_ICONS[cap] ? t(`settings.manager.cap.${cap}.label`) : cap;
}
function capabilityDesc(cap) {
    return CAPABILITY_ICONS[cap]
        ? t(`settings.manager.cap.${cap}.desc`)
        : t('settings.manager.cap.unknown.desc');
}

function renderCapabilityBadges(capabilities, legacy) {
    if (!Array.isArray(capabilities) || capabilities.length === 0) {
        return `<div class="ext-capabilities ext-capabilities-none" title="${escapeAttr(t('settings.manager.cap.none.title'))}">${t('settings.manager.cap.none')}</div>`;
    }
    const pills = capabilities
        .map((cap) => {
            const icon = CAPABILITY_ICONS[cap] || '❓';
            const label = capabilityLabel(cap);
            const desc = capabilityDesc(cap);
            return `<span class="ext-capability-pill" title="${escapeAttr(desc)}">${icon} ${escapeHtml(label)}</span>`;
        })
        .join('');
    const legacyBanner = legacy
        ? `<div class="ext-capabilities-legacy">${t('settings.manager.cap.legacy_warning')}</div>`
        : '';
    return `<div class="ext-capabilities">${pills}</div>${legacyBanner}`;
}

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
                const extId = module._extensionId;
                // Framework-owned header: icon + title on left, enable/disable button on right
                html += `<h2 class="settings-section-header ext-section-header">
                    <span>${module.icon} ${module.title}</span>
                    <button class="setting-button" id="ext-toggle-btn-${extId}" style="min-width:80px;font-size:12px;" data-action="toggleExtension" data-arg="${extId}">Disable</button>
                    <input type="hidden" id="ext-enabled-${extId}" value="true">
                </h2>`;
                if (module.description) {
                    html += `<p style="font-size:12px;color:var(--kage-text-muted);margin:0 0 16px;line-height:1.4;">${module.description}</p>`;
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
        if (this._initialized.has(module.id)) return;
        this._initialized.add(module.id);
        try {
            const result = module.initialize();
            if (result && typeof result.catch === 'function') {
                result.catch((e) => {
                    // Don't unset _initialized — a busted initialize will keep
                    // throwing on every reveal otherwise. Surface the error
                    // and leave the section in whatever state it reached.
                    console.error(`Settings module ${module.id} initialize failed:`, e);
                });
            }
        } catch (e) {
            console.error(`Settings module ${module.id} initialize failed:`, e);
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
        const targetModule = this.modules.find((m) => m.id === sectionId);
        if (targetModule) {
            this._initializeModule(targetModule);
        }

        // Reload config when switching tabs so data is fresh
        this.load();

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
            this.modules.forEach((module) => {
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
            // Validate all modules (legacy sync, sandboxed async)
            for (const module of this.modules) {
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
            this.modules.forEach((module) => {
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

            // Save to backend
            await this.invoke('save_config', { config });

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

        statusEl.textContent = message;
        statusEl.className = 'status-message ' + type;
        statusEl.style.display = 'block';

        setTimeout(() => {
            statusEl.style.display = 'none';
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
        btn.style.background = enabled ? '#c44' : 'var(--kage-accent)';
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
