import { SettingsModule } from './base.js';
import { renderAllInto } from '../shared/mac-permissions.js';
import { t } from '../shared/i18n.js';
/**
 * macOS Permissions Settings Module.
 *
 * Shows the three TCC permissions Kage needs (Accessibility, Input Monitoring,
 * Screen Recording) with deep-link buttons into the right pane of System
 * Settings. The macOS privacy model prompts the user at runtime the first
 * time a restricted API is called, so this section is a discoverable way
 * to grant (or revisit) the permissions before they are triggered.
 *
 * Only registered on macOS — see manager.js for the conditional registration.
 * All rendering/behavior delegates to shared/mac-permissions.js so the same
 * logic is reused by the Welcome wizard.
 */
export class MacPermissionsSettingsModule extends SettingsModule {
    constructor() {
        super('mac-permissions', t('settings.mac_permissions.title'), '');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                <div class="setting-description" style="margin-bottom: 16px;">
                    ${t('settings.mac_permissions.description')}
                </div>

                <div id="macPermCards"></div>

                <div class="mac-perm-note">
                    ${t('settings.mac_permissions.restart_note')}
                </div>
            </div>
        `;
    }

    initialize() {
        const container = document.getElementById('macPermCards');
        if (!container) return;
        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) return;
        renderAllInto(container, invoke, 'macPermSetting');
    }

    // This module has no persisted settings — the TCC state lives in the OS,
    // not in Kage's config. load/save are no-ops.
    load(_config) {
        /* noop */
    }
    save(_config) {
        /* noop */
    }
    validate() {
        return { valid: true };
    }
}
