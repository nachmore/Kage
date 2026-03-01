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
                html += `<div style="display:flex;align-items:center;justify-content:space-between;">
                    <h2 class="settings-section-header" style="margin:0;border-bottom:none;padding-bottom:0;">${module.icon} ${module.title}</h2>
                    <button class="setting-button" id="ext-toggle-btn-${extId}" style="min-width:80px;font-size:12px;" onclick="toggleExtension('${extId}')">Disable</button>
                    <input type="hidden" id="ext-enabled-${extId}" value="true">
                </div>`;
                html += `<hr style="border:none;border-top:1px solid var(--kiro-border-subtle);margin:12px 0 8px;">`;
                if (module.description) {
                    html += `<p style="font-size:12px;color:var(--kiro-text-muted);margin:0 0 16px;line-height:1.4;">${module.description}</p>`;
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
                if (!validation.valid) {
                    this.showStatus(validation.error, 'error');
                    return false;
                }
            }

            // Build config object
            const config = { version: 1 };
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
            
            this.showStatus('Settings saved! All changes apply immediately.', 'success');
            return true;
        } catch (error) {
            this.showStatus('Failed to save settings: ' + error, 'error');
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

// Extension metadata for dynamic loading
const BUNDLED_EXTENSIONS = [
    { id: 'math', scriptPath: 'extensions/math/settings.js', className: 'MathExtSettingsModule', sidebarIcon: '🧮', sidebarLabel: 'Math' },
    { id: 'color-picker', scriptPath: 'extensions/color-picker/settings.js', className: 'ColorPickerExtSettingsModule', sidebarIcon: '🎨', sidebarLabel: 'Color Picker' },
    { id: 'dev-tools', scriptPath: 'extensions/dev-tools/settings.js', className: 'DevToolsExtSettingsModule', sidebarIcon: '🛠️', sidebarLabel: 'Developer Tools' },
    { id: 'timer', scriptPath: 'extensions/timer/settings.js', className: 'TimerExtSettingsModule', sidebarIcon: '⏱️', sidebarLabel: 'Timer' },
];

/**
 * Dynamically load a script and return a promise that resolves when loaded.
 */
function loadScript(src) {
    return new Promise((resolve, reject) => {
        const script = document.createElement('script');
        script.src = src;
        script.onload = () => {
            // Small delay to ensure the script has been parsed and executed
            setTimeout(resolve, 0);
        };
        script.onerror = (e) => {
            console.error('Failed to load script:', src, e);
            reject(e);
        };
        document.head.appendChild(script);
    });
}

/**
 * Initialize settings on page load
 */
window.addEventListener('DOMContentLoaded', async () => {
    settingsManager = new SettingsManager();
    
    // Register core modules (order matches sidebar)
    // User Experience
    settingsManager.registerModule(new AppearanceSettingsModule());
    settingsManager.registerModule(new HotkeySettingsModule());
    settingsManager.registerModule(new SystemSettingsModule());
    // Kiro Assistant
    settingsManager.registerModule(new AssistantSettingsModule());
    settingsManager.registerModule(new ConnectionSettingsModule());
    settingsManager.registerModule(new ModelSettingsModule());
    settingsManager.registerModule(new ToolPermissionsSettingsModule());
    // Core extensions (non-pluggable)
    settingsManager.registerModule(new IntegrationSettingsModule());
    settingsManager.registerModule(new ShortcutsSettingsModule());

    // Dynamically load and register extension settings modules
    for (const ext of BUNDLED_EXTENSIONS) {
        try {
            await loadScript(ext.scriptPath);
            const ModuleClass = window[ext.className];
            if (ModuleClass) {
                const mod = new ModuleClass();
                mod._extensionId = ext.id; // tag it so the framework can inject the enable toggle
                settingsManager.registerModule(mod);
            }
        } catch (e) {
            console.warn(`Failed to load extension settings for '${ext.id}':`, e);
        }
    }

    // About
    settingsManager.registerModule(new UpdatesSettingsModule());
    settingsManager.registerModule(new AboutSettingsModule());
    
    // Render and load
    settingsManager.render();
    await settingsManager.load();
});

/**
 * Global functions for UI
 */
function saveSettings() {
    settingsManager.save();
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
        btn.style.background = enabled ? '#c44' : 'var(--kiro-accent)';
        btn.style.color = 'white';
        btn.style.border = 'none';
    }
    if (content) {
        content.style.opacity = enabled ? '' : '0.4';
        content.style.pointerEvents = enabled ? '' : 'none';
    }
}
