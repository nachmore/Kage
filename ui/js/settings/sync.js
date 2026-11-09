import { SettingsModule } from './base.js';
import { t } from '../shared/i18n.js';

/**
 * Settings → Sync (docs/design/SYNC.md §13).
 *
 * Skeleton from the contracts commit: it isn't registered in manager.js
 * yet, and draws only its header. WP-S6 builds the page and registers it;
 * the sidebar item stays hidden while `sync_get_status` reports
 * `unavailable`.
 */
export class SyncSettingsModule extends SettingsModule {
    constructor() {
        super('sync', t('settings.sync.title'), '🔄');
        // Every control calls its own sync_* command; save() is a no-op.
        this.persistsImmediately = true;
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
            </div>
        `;
    }

    load(_config) {}

    save(_config) {}

    validate() {
        return { valid: true };
    }
}
