import { SettingsModule } from './base.js';
/**
 * Startup Settings Module
 */

export function getSystemIcon() {
    const platform = navigator.platform || '';
    if (platform.startsWith('Win')) return '🪟';
    if (platform.startsWith('Mac') || platform.startsWith('iPhone')) return '\uF8FF';
    return '🐧';
}

export class SystemSettingsModule extends SettingsModule {
    constructor() {
        super('system', 'Startup', '🚀');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                
                ${this.createCheckboxRow(
                    'Auto-start on system startup',
                    'Launch Kage automatically when you log in.',
                    'autoStart',
                    false
                )}
            </div>
        `;
    }

    load(config) {
        // Check actual registry state, not config
        const autoStart = document.getElementById('autoStart');
        if (autoStart) {
            window.__TAURI__.core
                .invoke('get_startup_enabled')
                .then((enabled) => {
                    autoStart.checked = enabled;
                })
                .catch(() => {
                    if (config.system) autoStart.checked = config.system.auto_start;
                });
        }
    }

    initialize() {
        // Toggle startup registry entry when checkbox changes
        const autoStart = document.getElementById('autoStart');
        if (autoStart) {
            autoStart.addEventListener('change', async () => {
                try {
                    await window.__TAURI__.core.invoke('set_startup_enabled', {
                        enabled: autoStart.checked,
                    });
                } catch (e) {
                    console.error('Failed to set startup:', e);
                }
            });
        }
    }

    save(config) {
        config.system = config.system || {};
        config.system.auto_start = document.getElementById('autoStart')?.checked ?? false;
    }
}
