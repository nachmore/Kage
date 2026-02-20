/**
 * System Settings Module
 */
class SystemSettingsModule extends SettingsModule {
    constructor() {
        super('system', 'System', '⚡');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                
                ${this.createSettingRow(
                    'Auto-start on system startup',
                    'Launch Kiro Assistant automatically when you log in',
                    `
                    <label class="toggle-switch">
                        <input type="checkbox" id="autoStart">
                        <span class="toggle-slider"></span>
                    </label>
                    `
                )}
            </div>
        `;
    }

    load(config) {
        if (config.system) {
            const autoStart = document.getElementById('autoStart');
            if (autoStart) {
                autoStart.checked = config.system.auto_start;
            }
        }
    }

    save(config) {
        config.system = {
            auto_start: document.getElementById('autoStart').checked
        };
    }
}
