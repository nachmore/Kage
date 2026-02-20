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

        // Render all modules
        container.innerHTML = this.modules.map(module => module.render()).join('');

        // Initialize all modules
        this.modules.forEach(module => module.initialize());
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
            
            this.showStatus('Settings saved successfully! Please restart the application for changes to take effect.', 'success');
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
