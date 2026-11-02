import { SettingsModule } from './base.js';
import { renderAllInto } from '../shared/mac-permissions.js';
import { t } from '../shared/i18n.js';
/**
 * macOS Permissions Settings Module.
 *
 * Shows the TCC permissions Kage uses (Accessibility, Screen Recording)
 * with their live status, a deep link into the right pane of System
 * Settings, and a Repair action for a grant macOS holds for an older
 * build (see shared/mac-permissions.js). Status refreshes whenever the
 * section is shown and whenever the window regains focus, so it is
 * current when the user comes back from System Settings.
 *
 * Only registered on macOS — see main.js for the conditional registration.
 * All rendering/behavior delegates to shared/mac-permissions.js so the same
 * logic is reused by the Welcome wizard.
 */
export class MacPermissionsSettingsModule extends SettingsModule {
    constructor() {
        super('mac-permissions', t('settings.mac_permissions.title'), '');
        // Read-only status pane; save() is a no-op.
        this.persistsImmediately = true;
        this._cards = null;
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
        this._cards = renderAllInto(container, invoke, 'macPermSetting');
    }

    // The TCC state lives in the OS, not in Kage's config, so there is
    // nothing to load or save. The manager calls load() each time this
    // (never-dirty) section is shown, which is the cue to re-read status.
    load(_config) {
        this._cards?.refresh();
    }
    save(_config) {
        /* noop */
    }
    validate() {
        return { valid: true };
    }

    destroy() {
        this._cards?.dispose();
        this._cards = null;
    }
}
