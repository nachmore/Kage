/**
 * System Settings Module
 */

function getSystemIcon() {
    const platform = navigator.platform || '';
    if (platform.startsWith('Win')) return '🪟';
    if (platform.startsWith('Mac') || platform.startsWith('iPhone')) return '\uF8FF'; // Apple logo (may not render everywhere)
    return '🐧';
}

class SystemSettingsModule extends SettingsModule {
    constructor() {
        super('system', 'System', getSystemIcon());
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                
                ${this.createCheckboxRow(
                    'Auto-start on system startup',
                    'Launch Kiro Assistant automatically when you log in.',
                    'autoStart',
                    false
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

    initialize() {
        // Update the sidebar icon to match the OS
        const sidebarIcon = document.getElementById('systemSidebarIcon');
        if (sidebarIcon) {
            sidebarIcon.textContent = this.icon;
        }
    }

    save(config) {
        config.system = {
            auto_start: document.getElementById('autoStart').checked
        };
    }
}
