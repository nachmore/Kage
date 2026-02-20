/**
 * Base class for settings modules
 * Each settings module should extend this class
 */
class SettingsModule {
    constructor(id, title, icon) {
        this.id = id;
        this.title = title;
        this.icon = icon;
    }

    /**
     * Render the settings section HTML in IDE style
     * @returns {string} HTML string for the settings section
     */
    render() {
        throw new Error('render() must be implemented by subclass');
    }

    /**
     * Helper to create a setting row
     */
    createSettingRow(label, description, control) {
        return `
            <div class="setting-row">
                <div class="setting-label-container">
                    <div class="setting-label">${label}</div>
                    ${description ? `<div class="setting-description">${description}</div>` : ''}
                </div>
                <div class="setting-control">
                    ${control}
                </div>
            </div>
        `;
    }

    /**
     * Load settings from config object
     * @param {Object} config - The configuration object
     */
    load(config) {
        throw new Error('load() must be implemented by subclass');
    }

    /**
     * Save settings to config object
     * @param {Object} config - The configuration object to update
     */
    save(config) {
        throw new Error('save() must be implemented by subclass');
    }

    /**
     * Validate settings before saving
     * @returns {Object} { valid: boolean, error?: string }
     */
    validate() {
        return { valid: true };
    }

    /**
     * Initialize event listeners after rendering
     */
    initialize() {
        // Optional: Override if needed
    }

    /**
     * Cleanup when module is destroyed
     */
    destroy() {
        // Optional: Override if needed
    }
}
