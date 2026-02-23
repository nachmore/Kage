/**
 * System Integration Settings Module
 */
class IntegrationSettingsModule extends SettingsModule {
    constructor() {
        super('integration', 'System Integration', getSystemIcon());
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                ${this.createCheckboxRow(
                    'Capture selected text',
                    'Grab selected text from the active window when the hotkey is pressed (uses Ctrl+C). Disable if this interferes with terminal apps or other programs.',
                    'captureSelection',
                    true
                )}
            </div>
        `;
    }

    initialize() {
        const sidebarIcon = document.getElementById('integrationSidebarIcon');
        if (sidebarIcon) sidebarIcon.textContent = this.icon;
    }

    load(config) {
        const captureSel = document.getElementById('captureSelection');
        if (captureSel) captureSel.checked = config.system?.capture_selection !== false;
    }

    save(config) {
        config.system = config.system || {};
        config.system.capture_selection = document.getElementById('captureSelection')?.checked ?? true;
    }
}
