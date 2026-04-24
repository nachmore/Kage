/**
 * Settings Manager
 * Coordinates all settings modules and handles save/load operations
 */
class SettingsManager {
    constructor() {
        this.modules = [];
        this.invoke = window.__TAURI__.core.invoke;
        this.appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
    }

    /**
     * Register a settings module
     * @param {SettingsModule} module - The settings module to register
     */
    registerModule(module) {
        if (!(module instanceof SettingsModule)) {
            throw new Error('Module must extend SettingsModule');
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
                    <button class="setting-button" id="ext-toggle-btn-${extId}" style="min-width:80px;font-size:12px;" onclick="toggleExtension('${extId}')">Disable</button>
                    <input type="hidden" id="ext-enabled-${extId}" value="true">
                </h2>`;
                if (module.description) {
                    html += `<p style="font-size:12px;color:var(--kage-text-muted);margin:0 0 16px;line-height:1.4;">${module.description}</p>`;
                }
                html += `<div id="ext-content-${extId}">`;
                html += module.renderContent ? module.renderContent() : module.render();
                html += `</div>`;
            } else {
                html += module.render();
            }
            html += `</div>`;
        });

        container.innerHTML = html;

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
            // Validate all modules
            for (const module of this.modules) {
                const validation = module.validate();
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
 * Dynamically load a script tag and wait for execution.
 */
function loadScript(src) {
    return new Promise((resolve, reject) => {
        const script = document.createElement('script');
        script.src = src;
        script.onload = () => setTimeout(resolve, 0);
        script.onerror = (e) => { console.error('Failed to load script:', src, e); reject(e); };
        document.head.appendChild(script);
    });
}

/**
 * Load a script from a string (for user-installed extensions).
 */
function loadScriptFromString(code) {
    return new Promise((resolve) => {
        const script = document.createElement('script');
        script.textContent = code;
        document.head.appendChild(script);
        setTimeout(resolve, 0);
    });
}

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
    item.onclick = () => switchSection(id);
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

    // 1. Load bundled extension settings from bundled.json
    try {
        const resp = await fetch('extensions/bundled.json');
        if (resp.ok) {
            const bundledList = await resp.json();
            for (const ext of bundledList) {
                try {
                    console.log(`[Settings] Loading bundled extension: ${ext.id} (class: ${ext.className})`);
                    await loadScript(`extensions/${ext.id}/settings.js`);
                    const ModuleClass = window[ext.className];
                    console.log(`[Settings] ${ext.id}: window.${ext.className} =`, ModuleClass ? 'found' : 'NOT FOUND');
                    if (ModuleClass) {
                        const mod = new ModuleClass();
                        mod._extensionId = ext.id;
                        settingsManager.registerModule(mod);
                        addExtensionSidebarItem(mod.id, ext.sidebarIcon || mod.icon, ext.sidebarLabel || mod.title);
                    }
                } catch (e) {
                    console.warn(`Failed to load bundled extension settings '${ext.id}':`, e);
                }
            }
        }
    } catch (e) {
        console.warn('Failed to load bundled.json:', e);
    }

    // 2. Load user-installed extension settings from backend
    try {
        const userExts = await invoke('list_extensions');
        // Get set of bundled IDs to avoid duplicates
        const bundledIds = new Set();
        try {
            const resp = await fetch('extensions/bundled.json');
            if (resp.ok) {
                const list = await resp.json();
                list.forEach(e => bundledIds.add(e.id));
            }
        } catch {}

        for (const item of userExts) {
            if (bundledIds.has(item.manifest.id)) continue; // already loaded as bundled
            const manifest = item.manifest;
            if (!manifest.contributes?.settingsModule) continue;

            try {
                const settingsPath = manifest.contributes.settingsModule.replace('./', '');
                const code = await invoke('read_extension_file', {
                    extensionId: manifest.id,
                    kind: 'extension',
                    filePath: settingsPath,
                });
                await loadScriptFromString(code);

                // The settings class should register itself on window with a predictable name
                // Convention: <PascalCaseId>ExtSettingsModule
                const className = manifest.id
                    .split('-')
                    .map(w => w.charAt(0).toUpperCase() + w.slice(1))
                    .join('') + 'ExtSettingsModule';
                const ModuleClass = window[className];
                if (ModuleClass) {
                    const mod = new ModuleClass();
                    mod._extensionId = manifest.id;
                    mod._extensionVersion = manifest.version;
                    settingsManager.registerModule(mod);
                    addExtensionSidebarItem(mod.id, manifest.icon || '📦', manifest.name);
                } else {
                    console.warn(`User extension '${manifest.id}' settings loaded but class '${className}' not found on window`);
                }
            } catch (e) {
                console.warn(`Failed to load user extension settings '${manifest.id}':`, e);
            }
        }
    } catch (e) {
        console.warn('Failed to load user extensions for settings:', e);
    }

    // About
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
            // Get bundled IDs
            const bundledIds = new Set();
            try {
                const resp = await fetch('extensions/bundled.json');
                if (resp.ok) {
                    const list = await resp.json();
                    list.forEach(e => bundledIds.add(e.id));
                }
            } catch {}

            let added = false;
            for (const item of userExts) {
                if (bundledIds.has(item.manifest.id)) continue;
                const manifest = item.manifest;
                if (!manifest.contributes?.settingsModule) continue;

                // Check if already registered — if version changed, tear down and reload
                const existingMod = settingsManager.modules.find(m => m._extensionId === manifest.id);
                if (existingMod) {
                    if (existingMod._extensionVersion === manifest.version) continue;
                    // Version changed — remove old module
                    console.log(`[Settings] Updating extension settings '${manifest.id}' from ${existingMod._extensionVersion} to ${manifest.version}`);
                    const idx = settingsManager.modules.indexOf(existingMod);
                    if (idx !== -1) settingsManager.modules.splice(idx, 1);
                    const sidebarItem = document.querySelector(`.sidebar-item[data-section="${existingMod.id}"]`);
                    if (sidebarItem) sidebarItem.remove();
                    try { existingMod.destroy?.(); } catch {}
                }

                try {
                    const settingsPath = manifest.contributes.settingsModule.replace('./', '');
                    const code = await invoke('read_extension_file', {
                        extensionId: manifest.id,
                        kind: 'extension',
                        filePath: settingsPath,
                    });
                    await loadScriptFromString(code);

                    const className = manifest.id
                        .split('-')
                        .map(w => w.charAt(0).toUpperCase() + w.slice(1))
                        .join('') + 'ExtSettingsModule';
                    const ModuleClass = window[className];
                    if (ModuleClass) {
                        const mod = new ModuleClass();
                        mod._extensionId = manifest.id;
                        mod._extensionVersion = manifest.version;
                        const insertIdx = Math.max(0, settingsManager.modules.length - 2);
                        settingsManager.modules.splice(insertIdx, 0, mod);
                        addExtensionSidebarItem(mod.id, manifest.icon || '📦', manifest.name);
                        added = true;
                        console.log(`[Settings] Hot-loaded extension settings: ${manifest.id} v${manifest.version}`);
                    }
                } catch (e) {
                    console.warn(`[Settings] Failed to hot-load extension settings '${manifest.id}':`, e);
                }
            }

            // Also remove modules for uninstalled extensions
            const installedIds = new Set(userExts.map(e => e.manifest.id));
            const toRemove = settingsManager.modules.filter(m =>
                m._extensionId && !bundledIds.has(m._extensionId) && !installedIds.has(m._extensionId)
            );
            for (const mod of toRemove) {
                const idx = settingsManager.modules.indexOf(mod);
                if (idx !== -1) {
                    settingsManager.modules.splice(idx, 1);
                    // Remove sidebar item
                    const sidebarItem = document.querySelector(`.sidebar-item[data-section="${mod.id}"]`);
                    if (sidebarItem) sidebarItem.remove();
                    added = true; // need re-render
                    console.log(`[Settings] Removed uninstalled extension settings: ${mod._extensionId}`);
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
