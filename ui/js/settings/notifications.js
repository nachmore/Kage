import { SettingsModule } from './base.js';
import { t } from '../shared/i18n.js';
/**
 * Notifications Settings Module
 * Example of how easy it is to add a new settings category
 */
export class NotificationsSettingsModule extends SettingsModule {
    constructor() {
        super('notifications', t('settings.notifications.title'), '🔔');
        this.bindFields([
            {
                id: 'notificationsEnabled',
                path: 'notifications.enabled',
                kind: 'checkbox',
                default: false,
            },
            {
                id: 'notificationSound',
                path: 'notifications.sound',
                kind: 'value',
                default: 'default',
            },
            {
                id: 'notificationDuration',
                path: 'notifications.duration',
                kind: 'int',
                default: 5,
            },
        ]);
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2>${this.icon} ${this.title}</h2>
                <div class="setting-item">
                    <label class="setting-label">${t('settings.notifications.enable.label')}</label>
                    <label class="toggle-switch">
                        <input type="checkbox" id="notificationsEnabled">
                        <span class="toggle-slider"></span>
                    </label>
                    <div class="setting-description">${t('settings.notifications.enable.description')}</div>
                </div>
                <div class="setting-item">
                    <label class="setting-label">${t('settings.notifications.sound.label')}</label>
                    <select class="setting-input" id="notificationSound">
                        <option value="none">${t('settings.notifications.sound.none')}</option>
                        <option value="default">${t('settings.notifications.sound.default')}</option>
                        <option value="chime">${t('settings.notifications.sound.chime')}</option>
                        <option value="bell">${t('settings.notifications.sound.bell')}</option>
                    </select>
                    <div class="setting-description">${t('settings.notifications.sound.description')}</div>
                </div>
                <div class="setting-item">
                    <label class="setting-label">${t('settings.notifications.duration.label')}</label>
                    <input type="number" class="setting-input" id="notificationDuration" min="1" max="30" value="5">
                    <div class="setting-description">${t('settings.notifications.duration.description')}</div>
                </div>
            </div>
        `;
    }

    load(config) {
        this.loadFields(config);
    }

    save(config) {
        this.saveFields(config);
    }

    validate() {
        const duration = parseInt(document.getElementById('notificationDuration').value, 10);
        if (duration < 1 || duration > 30) {
            return {
                valid: false,
                error: t('settings.notifications.duration.error'),
            };
        }
        return { valid: true };
    }
}
