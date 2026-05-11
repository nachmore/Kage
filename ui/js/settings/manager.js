/**
 * Settings Manager
 * Coordinates all settings modules and handles save/load operations.
 *
 * For sandboxed extension settings we rely on helpers hoisted onto
 * `window.__kageSettingsSandbox` by js/settings/sandbox-bootstrap.js
 * (loaded as a module from settings.html). See that file for the
 * actual import wiring.
 */

// Capability → (icon, description) used to render permission badges on
// extension settings pages. Keep in sync with ui/js/shared/extension-permissions.js.
const CAPABILITY_INFO = Object.freeze({
    storage:       { icon: '💾', label: 'Storage',       desc: 'Read/write its own sandboxed data and config' },
    clipboard:     { icon: '📋', label: 'Clipboard',     desc: 'Read clipboard contents and history' },
    shell:         { icon: '🌐', label: 'Shell',         desc: 'Open URLs, paths, and apps externally' },
    filesystem:    { icon: '📂', label: 'Filesystem',    desc: 'Scan folders and search files' },
    window:        { icon: '🪟', label: 'Window',        desc: 'Resize and reposition Kage windows' },
    windows:       { icon: '🧿', label: 'Open windows',  desc: 'List and focus other apps\' windows' },
    notifications: { icon: '🔔', label: 'Notifications', desc: 'Show system notifications' },
    calendar:      { icon: '📅', label: 'Calendar',      desc: 'Read calendar events' },
    session:       { icon: '💬', label: 'Sessions',      desc: 'List and read chat sessions' },
    agent:         { icon: '🤖', label: 'Agent',         desc: 'Send messages to the AI agent' },
    activity:      { icon: '📊', label: 'Activity',      desc: 'Read app usage statistics' },
    automation:    { icon: '⚡', label: 'Automation',    desc: 'Emit signals that can trigger automations' },
    tts:           { icon: '🔈', label: 'TTS',           desc: 'Use text-to-speech' },
});

function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
}

function renderCapabilityBadges(capabilities, legacy) {
    if (!Array.isArray(capabilities) || capabilities.length === 0) {
        return '<div class="ext-capabilities ext-capabilities-none" title="This extension requested no capabilities">🔒 No capabilities</div>';
    }
    const pills = capabilities.map(cap => {
        const info = CAPABILITY_INFO[cap] || { icon: '❓', label: cap, desc: 'Unknown capability' };
        return `<span class="ext-capability-pill" title="${escapeHtml(info.desc)}">${info.icon} ${escapeHtml(info.label)}</span>`;
    }).join('');
    const legacyBanner = legacy
        ? `<div class="ext-capabilities-legacy">⚠ Extension manifest does not declare 'permissions' — running with default set. Ask the author to specify.</div>`
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

class SandboxedExtensionSettingsModule {
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

    render() { return this.renderContent(); }

    load(config) {
        const stored = (config.extensions && config.extensions[this._extensionId]) || {};
        this._rendered.load(stored);
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions[this._extensionId] = this._rendered.save();
    }

    async validate() {
        return this._rendered.validate();
    }

    initialize() { /* event wiring happens inside RenderedSettings */ }

    destroy() {
        try { this._rendered.destroy(); } catch {}
    }
}

/**
 * Given a manifest and its source paths, boot a sandbox for settings
 * rendering, fetch the declared schema, and build the adapter module.
 */
async function buildSandboxedSettingsModule({ pool, manifest, capabilities, settingsProviderSource, currentConfig }) {
    // Collect sources: only the settings provider is needed for the
    // settings window. Search/tool/trigger providers are already loaded
    // in the floating/chat windows' own sandbox pools.
    const sources = { settingsProvider: settingsProviderSource };

    // Extension config values the provider should see.
    const extConfig = (currentConfig?.extensions && currentConfig.extensions[manifest.id]) || {};

    const sandbox = await pool.load({
        extensionId: manifest.id,
        capabilities,
        config: extConfig,
        sources,
    });

    if (!sandbox.hasSettings) {
        pool.unload(manifest.id);
        throw new Error(`extension '${manifest.id}' declared a settingsProvider but the sandbox didn't report one`);
    }

    const schema = await sandbox.call('getSettings', {});
    if (!schema || typeof schema !== 'object') {
        pool.unload(manifest.id);
        throw new Error(`extension '${manifest.id}' getSettings() returned nothing`);
    }

    const container = document.createElement('div');
    const rendered = window.__kageSettingsSandbox.renderSchema({
        extensionId: manifest.id,
        schema,
        container,
        sandbox,
    });

    return new SandboxedExtensionSettingsModule({
        extensionId: manifest.id,
        manifest,
        sandbox,
        rendered,
        capabilities,
    });
}

class SettingsManager {
    constructor() {
        this.modules = [];
        this.invoke = window.__TAURI__.core.invoke;
        this.appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
    }

    /**
     * Register a settings module. Accepts either a legacy
     * SettingsModule subclass (used by first-party modules) or a
     * SandboxedExtensionSettingsModule (used by all extensions).
     */
    registerModule(module) {
        const isLegacy = (typeof SettingsModule !== 'undefined') && (module instanceof SettingsModule);
        const isSandboxed = module instanceof SandboxedExtensionSettingsModule;
        if (!isLegacy && !isSandboxed) {
            throw new Error('Module must extend SettingsModule or SandboxedExtensionSettingsModule');
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
        this.modules.forEach(module => {
            if (module instanceof SandboxedExtensionSettingsModule) {
                const slot = document.getElementById(`ext-sandbox-slot-${module._extensionId}`);
                if (slot && module._rendered && module._rendered.container) {
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

        // Initialize all modules
        this.modules.forEach(module => module.initialize());
    }

    /**
     * Switch to a different section
     */
    switchSection(sectionId) {
        // Update sidebar active state
        document.querySelectorAll('.sidebar-item').forEach(item => {
            if (item.dataset.section === sectionId) {
                item.classList.add('active');
            } else {
                item.classList.remove('active');
            }
        });

        // Show/hide section content
        document.querySelectorAll('[data-section-content]').forEach(section => {
            if (section.dataset.sectionContent === sectionId) {
                section.classList.remove('hidden');
            } else {
                section.classList.add('hidden');
            }
        });

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
            this.modules.forEach(module => {
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
            this.showStatus('Failed to load settings: ' + error, 'error');
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
                const validation = (raw && typeof raw.then === 'function') ? await raw : raw;
                if (!validation || typeof validation !== 'object' || !('valid' in validation)) {
                    this.showStatus(`[${module.title}] validate() must return { valid: true/false, error?: string }`, 'error');
                    return false;
                }
                if (!validation.valid) {
                    this.showStatus(`[${module.title}] ${validation.error || 'Validation failed'}`, 'error');
                    return false;
                }
            }

            // Start from the current config so fields not owned by any module
            // (e.g. first_run_completed) are preserved across saves.
            const config = await this.invoke('get_config');
            config.version = 1;
            this.modules.forEach(module => {
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
            const needsRestart = this.modules.some(m => m._needsRestart);
            if (needsRestart) {
                // Reset the flag so it doesn't trigger again on next save
                this.modules.forEach(m => { m._needsRestart = false; });
                this.showStatus('Settings saved. Restart required for connection changes.', 'success');
                // Use setTimeout to let the status message render before showing dialog
                setTimeout(async () => {
                    try {
                        const { ask } = window.__TAURI__.dialog;
                        const restart = await ask('Connection settings changed. The app needs to restart to apply these changes.\n\nRestart now?', {
                            title: 'Restart Required',
                            kind: 'info',
                        });
                        if (restart) {
                            this.invoke('restart_app');
                        }
                    } catch {
                        // Fallback to native confirm if Tauri dialog not available
                        if (confirm('Connection settings changed. Restart now?')) {
                            this.invoke('restart_app');
                        }
                    }
                }, 100);
                return true;
            }

            this.showStatus('Settings saved! All changes apply immediately.', 'success');
            return true;
        } catch (error) {
            console.error('[Settings] Save failed:', error);
            const msg = typeof error === 'string' ? error 
                : error?.message || error?.toString() || JSON.stringify(error) || 'Unknown error';
            this.showStatus('Failed to save: ' + msg, 'error');
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
     * Close settings window
     */
    close() {
        this.appWindow.hide();
    }

    /**
     * Cleanup all modules
     */
    destroy() {
        this.modules.forEach(module => module.destroy());
        this.modules = [];
    }
}

// Global instance
let settingsManager;

/**
 * Add a sidebar item dynamically to the Extensions section, in alphabetical order.
 * Static items (store, integration, shortcuts) stay at the top.
 */
function addExtensionSidebarItem(id, icon, label) {
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
    const insertBefore = extItems.find(el => {
        const elLabel = el.querySelector('span:last-child')?.textContent?.toLowerCase() || '';
        return elLabel > lowerLabel;
    });

    if (insertBefore) {
        section.insertBefore(item, insertBefore);
    } else {
        section.appendChild(item);
    }
}

/**
 * Initialize settings on page load
 */
window.addEventListener('DOMContentLoaded', async () => {
    settingsManager = new SettingsManager();
    const invoke = window.__TAURI__.core.invoke;
    
    // Register core modules (order matches sidebar)
    settingsManager.registerModule(new AppearanceSettingsModule());
    settingsManager.registerModule(new HotkeySettingsModule());
    settingsManager.registerModule(new SystemSettingsModule());
    // macOS-only: privacy/TCC permissions pane. The sidebar item for this
    // module is hidden by default in settings.html and revealed here.
    if (window.kageMacPermissions?.isMacOS()) {
        settingsManager.registerModule(new MacPermissionsSettingsModule());
        const sidebarItem = document.getElementById('macPermissionsSidebarItem');
        if (sidebarItem) sidebarItem.classList.remove('hidden');
    }
    settingsManager.registerModule(new AssistantSettingsModule());
    settingsManager.registerModule(new ConnectionSettingsModule());
    settingsManager.registerModule(new ModelSettingsModule());
    settingsManager.registerModule(new McpSettingsModule());
    settingsManager.registerModule(new ToolPermissionsSettingsModule());
    settingsManager.registerModule(new IntegrationSettingsModule());
    settingsManager.registerModule(new ShortcutsSettingsModule());
    settingsManager.registerModule(new AutomationsSettingsModule());
    settingsManager.registerModule(new SpeechSettingsModule());
    settingsManager.registerModule(new StoreSettingsModule());

    // Settings-window-local sandbox pool. Separate from the floating/chat
    // windows' pools because each window has its own document; sandboxes
    // are tied to the document they mount into.
    const sandboxPool = window.__kageSettingsSandbox.createPool(invoke);

    // Preload current config so we can hand each sandbox the right initial values.
    let currentConfig = {};
    try { currentConfig = await invoke('get_config'); } catch (e) { console.warn('Failed to preload config:', e); }

    // Read all user-installed extensions once — we'll iterate over both
    // bundled and user-installed lists with a single loader path.
    let installedUser = [];
    try { installedUser = await invoke('list_extensions'); } catch {}

    // Helper: resolve granted capabilities for an extension. Bundled ones
    // get what their manifest declares (implicit grant); user-installed
    // ones get whatever `extension_grants[id].granted` says. See
    // docs/SECURITY_MODEL.md for the install-time grant story.
    function resolveCaps(manifest, bundled) {
        const requested = window.__kageSettingsSandbox.normalize(manifest.permissions, manifest.id);
        if (bundled) return requested;
        const record = (currentConfig.extension_grants || {})[manifest.id];
        if (!record) return [];
        const grantedSet = new Set(
            window.__kageSettingsSandbox.normalize(record.granted, manifest.id),
        );
        return requested.filter(cap => grantedSet.has(cap));
    }

    async function loadSandboxedSettings({ manifest, sourceCode, bundled }) {
        const capabilities = resolveCaps(manifest, bundled);
        const mod = await buildSandboxedSettingsModule({
            pool: sandboxPool,
            manifest,
            capabilities,
            settingsProviderSource: sourceCode,
            currentConfig,
        });
        settingsManager.registerModule(mod);
        addExtensionSidebarItem(mod.id, manifest.icon || '📦', manifest.name);
    }

    // 1. Load bundled extension settings.
    try {
        const resp = await fetch('extensions/bundled.json');
        if (resp.ok) {
            const bundledList = await resp.json();
            for (const entry of bundledList) {
                try {
                    const manifestResp = await fetch(`extensions/${entry.id}/manifest.json`);
                    if (!manifestResp.ok) continue;
                    const manifest = await manifestResp.json();
                    const settingsPath = manifest.contributes?.settingsProvider;
                    if (!settingsPath) continue; // extension has no settings UI
                    const srcResp = await fetch(`extensions/${entry.id}/${settingsPath.replace('./', '')}`);
                    if (!srcResp.ok) throw new Error(`HTTP ${srcResp.status}`);
                    const sourceCode = await srcResp.text();
                    await loadSandboxedSettings({ manifest, sourceCode, bundled: true });
                } catch (e) {
                    console.warn(`Failed to load bundled extension settings '${entry.id}':`, e);
                }
            }
        }
    } catch (e) {
        console.warn('Failed to load bundled.json:', e);
    }

    // 2. Load user-installed extension settings.
    try {
        const bundledIds = new Set();
        try {
            const resp = await fetch('extensions/bundled.json');
            if (resp.ok) (await resp.json()).forEach(e => bundledIds.add(e.id));
        } catch {}

        for (const item of installedUser) {
            if (bundledIds.has(item.manifest.id)) continue;
            const manifest = item.manifest;
            const settingsPath = manifest.contributes?.settingsProvider;
            if (!settingsPath) continue;
            try {
                const sourceCode = await invoke('read_extension_file', {
                    extensionId: manifest.id,
                    kind: 'extension',
                    filePath: settingsPath.replace('./', ''),
                });
                await loadSandboxedSettings({ manifest, sourceCode, bundled: false });
            } catch (e) {
                console.warn(`Failed to load user extension settings '${manifest.id}':`, e);
            }
        }
    } catch (e) {
        console.warn('Failed to load user extensions for settings:', e);
    }

    // About section — ordering here must match the sidebar in
    // ui/settings.html (Privacy → Updates → About). Several bits of
    // settings machinery key off registration order: the first module
    // is the default-visible section, extension module sidebar entries
    // are inserted relative to these, and switchSection() relies on
    // the one-to-one mapping between sidebar data-section IDs and the
    // module IDs registered here.
    settingsManager.registerModule(new PrivacySettingsModule());
    settingsManager.registerModule(new UpdatesSettingsModule());
    settingsManager.registerModule(new AboutSettingsModule());
    
    // Render and load
    settingsManager.render();
    await settingsManager.load();

    // Listen for extension install/uninstall — hot-load new settings modules
    const { listen } = window.__TAURI__.event;
    listen('extensions_changed', async () => {
        console.log('[Settings] extensions_changed — checking for new modules');
        try {
            const userExts = await invoke('list_extensions');
            // Refresh config so grants are current
            try { currentConfig = await invoke('get_config'); } catch {}

            const bundledIds = new Set();
            try {
                const resp = await fetch('extensions/bundled.json');
                if (resp.ok) (await resp.json()).forEach(e => bundledIds.add(e.id));
            } catch {}

            let added = false;
            for (const item of userExts) {
                if (bundledIds.has(item.manifest.id)) continue;
                const manifest = item.manifest;
                if (!manifest.contributes?.settingsProvider) continue;

                const existingMod = settingsManager.modules.find(m => m._extensionId === manifest.id);
                if (existingMod) {
                    if (existingMod._extensionVersion === manifest.version) continue;
                    console.log(`[Settings] Updating '${manifest.id}' from ${existingMod._extensionVersion} to ${manifest.version}`);
                    const idx = settingsManager.modules.indexOf(existingMod);
                    if (idx !== -1) settingsManager.modules.splice(idx, 1);
                    const sidebarItem = document.querySelector(`.sidebar-item[data-section="${existingMod.id}"]`);
                    if (sidebarItem) sidebarItem.remove();
                    try { existingMod.destroy?.(); } catch {}
                    sandboxPool.unload(manifest.id);
                }

                try {
                    const settingsPath = manifest.contributes.settingsProvider.replace('./', '');
                    const sourceCode = await invoke('read_extension_file', {
                        extensionId: manifest.id,
                        kind: 'extension',
                        filePath: settingsPath,
                    });
                    const capabilities = resolveCaps(manifest, false);
                    const mod = await buildSandboxedSettingsModule({
                        pool: sandboxPool,
                        manifest,
                        capabilities,
                        settingsProviderSource: sourceCode,
                        currentConfig,
                    });
                    const insertIdx = Math.max(0, settingsManager.modules.length - 2);
                    settingsManager.modules.splice(insertIdx, 0, mod);
                    addExtensionSidebarItem(mod.id, manifest.icon || '📦', manifest.name);
                    added = true;
                    console.log(`[Settings] Hot-loaded extension settings: ${manifest.id} v${manifest.version}`);
                } catch (e) {
                    console.warn(`[Settings] Failed to hot-load '${manifest.id}':`, e);
                }
            }

            // Remove modules for uninstalled extensions
            const installedIds = new Set(userExts.map(e => e.manifest.id));
            const toRemove = settingsManager.modules.filter(m =>
                m._extensionId && !bundledIds.has(m._extensionId) && !installedIds.has(m._extensionId),
            );
            for (const mod of toRemove) {
                const idx = settingsManager.modules.indexOf(mod);
                if (idx !== -1) {
                    settingsManager.modules.splice(idx, 1);
                    const sidebarItem = document.querySelector(`.sidebar-item[data-section="${mod.id}"]`);
                    if (sidebarItem) sidebarItem.remove();
                    try { mod.destroy?.(); } catch {}
                    sandboxPool.unload(mod._extensionId);
                    added = true;
                    console.log(`[Settings] Removed uninstalled extension: ${mod._extensionId}`);
                }
            }

            if (added) {
                settingsManager.render();
                await settingsManager.load();
            }
        } catch (e) {
            console.warn('[Settings] Failed to reload extensions:', e);
        }
    });
});

/**
 * Global functions for UI
 */
function saveSettings() {
    return settingsManager.save();
}

async function saveAndClose() {
    const success = await settingsManager.save();
    if (success) settingsManager.close();
}

function closeSettings() {
    settingsManager.close();
}

function switchSection(sectionId) {
    settingsManager.switchSection(sectionId);
}

function openStore() {
    if (window.__TAURI__?.core) {
        window.__TAURI__.core.invoke('open_store_window', { tab: 'extensions' });
    }
}

function toggleExtension(extId) {
    const hiddenInput = document.getElementById('ext-enabled-' + extId);
    if (!hiddenInput) return;
    const nowEnabled = hiddenInput.value !== 'true';
    hiddenInput.value = nowEnabled ? 'true' : 'false';
    _updateExtToggleUI(extId, nowEnabled);
}

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

// Register manager-owned actions with the delegated dispatcher (actions.js).
// These replace what used to be inline `onclick="..."` attributes calling
// window-level globals.
if (typeof window !== 'undefined' && window.registerSettingsActions) {
    window.registerSettingsActions({
        switchSection: (sectionId) => switchSection(sectionId),
        saveAndClose: () => saveAndClose(),
        closeSettings: () => closeSettings(),
        toggleExtension: (extId) => toggleExtension(extId),
        openStore: () => openStore(),
    });
}
