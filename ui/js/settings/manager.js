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
        // The first module ('hotkey') is visible by default; the rest are hidden.
        let html = '';
        this.modules.forEach((module, index) => {
            const hidden = index === 0 ? '' : ' hidden';
            html += `<div class="settings-section${hidden}" data-section-content="${module.id}">`;
            html += module.render();
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

/**
 * Initialize settings on page load
 */
window.addEventListener('DOMContentLoaded', async () => {
    settingsManager = new SettingsManager();
    
    // Register all modules (order matches sidebar)
    // User Experience
    settingsManager.registerModule(new AppearanceSettingsModule());
    settingsManager.registerModule(new HotkeySettingsModule());
    settingsManager.registerModule(new SystemSettingsModule());
    // Kiro Assistant
    settingsManager.registerModule(new AssistantSettingsModule());
    settingsManager.registerModule(new ConnectionSettingsModule());
    settingsManager.registerModule(new ModelSettingsModule());
    settingsManager.registerModule(new ToolPermissionsSettingsModule());
    // Advanced
    settingsManager.registerModule(new IntegrationSettingsModule());
    settingsManager.registerModule(new ShortcutsSettingsModule());
    settingsManager.registerModule(new MathSettingsModule());
    
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
