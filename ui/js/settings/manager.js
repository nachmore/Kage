/**
 * Settings Manager
 * Coordinates all settings modules and handles save/load operations
 */
class SettingsManager {
    constructor() {
        this.modules = [];
        this.invoke = window.__TAURI__.tauri.invoke;
        this.appWindow = window.__TAURI__.window.appWindow;
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

        // Group modules by section
        const kiroModules = this.modules.filter(m => m.id !== 'shortcuts' && m.id !== 'tool-permissions');
        const shortcutsModule = this.modules.find(m => m.id === 'shortcuts');
        const toolPermissionsModule = this.modules.find(m => m.id === 'tool-permissions');

        let html = '';

        // Render Kiro section (all modules except shortcuts and tool-permissions)
        if (kiroModules.length > 0) {
            html += `<div class="settings-section" data-section-content="kiro">`;
            html += kiroModules.map(module => module.render()).join('');
            html += `</div>`;
        }

        // Render Shortcuts section
        if (shortcutsModule) {
            html += `<div class="settings-section hidden" data-section-content="shortcuts">`;
            html += shortcutsModule.render();
            html += `</div>`;
        }

        // Render Tool Permissions section
        if (toolPermissionsModule) {
            html += `<div class="settings-section hidden" data-section-content="tool-permissions">`;
            html += toolPermissionsModule.render();
            html += `</div>`;
        }

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
            this.modules.forEach(module => module.load(config));
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
            this.modules.forEach(module => module.save(config));

            // Save to backend
            await this.invoke('save_config', { config });
            
            this.showStatus('Settings saved! Most changes apply immediately. Hotkey changes require a restart.', 'success');
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

/**
 * Initialize settings on page load
 */
window.addEventListener('DOMContentLoaded', async () => {
    settingsManager = new SettingsManager();
    
    // Register all modules
    settingsManager.registerModule(new HotkeySettingsModule());
    settingsManager.registerModule(new ConnectionSettingsModule());
    settingsManager.registerModule(new AppearanceSettingsModule());
    settingsManager.registerModule(new SystemSettingsModule());
    settingsManager.registerModule(new ShortcutsSettingsModule());
    settingsManager.registerModule(new ToolPermissionsSettingsModule());
    
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

function closeSettings() {
    settingsManager.close();
}

function switchSection(sectionId) {
    settingsManager.switchSection(sectionId);
}
