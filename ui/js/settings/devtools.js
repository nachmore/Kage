import { SettingsModule } from './base.js';
import { t } from '../shared/i18n.js';
/**
 * Developer Tools Settings Module
 */
export class DevToolsSettingsModule extends SettingsModule {
    constructor() {
        super('devtools', t('settings.devtools.title'), '🛠️');
        this.bindFields([
            { id: 'devToolsEnabled', path: 'dev_tools.enabled', kind: 'checkbox', default: true },
            { id: 'devToolUuid', path: 'dev_tools.uuid', kind: 'checkbox', default: true },
            { id: 'devToolBase64', path: 'dev_tools.base64', kind: 'checkbox', default: true },
            { id: 'devToolHash', path: 'dev_tools.hash', kind: 'checkbox', default: true },
            { id: 'devToolEpoch', path: 'dev_tools.epoch', kind: 'checkbox', default: true },
            {
                id: 'devToolJson',
                path: 'dev_tools.json_format',
                kind: 'checkbox',
                default: true,
            },
        ]);
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                ${this.createCheckboxRow(
                    t('settings.devtools.enable.label'),
                    t('settings.devtools.enable.description'),
                    'devToolsEnabled',
                    true
                )}

                <div class="setting-row" style="margin-top: 16px;">
                    <div class="setting-label">${t('settings.devtools.individual.label')}</div>
                    <div class="setting-description">${t('settings.devtools.individual.description')}</div>
                </div>

                ${this.createCheckboxRow(
                    t('settings.devtools.uuid.label'),
                    t('settings.devtools.uuid.description'),
                    'devToolUuid',
                    true
                )}

                ${this.createCheckboxRow(
                    t('settings.devtools.base64.label'),
                    t('settings.devtools.base64.description'),
                    'devToolBase64',
                    true
                )}

                ${this.createCheckboxRow(
                    t('settings.devtools.hash.label'),
                    t('settings.devtools.hash.description'),
                    'devToolHash',
                    true
                )}

                ${this.createCheckboxRow(
                    t('settings.devtools.epoch.label'),
                    t('settings.devtools.epoch.description'),
                    'devToolEpoch',
                    true
                )}

                ${this.createCheckboxRow(
                    t('settings.devtools.json.label'),
                    t('settings.devtools.json.description'),
                    'devToolJson',
                    true
                )}
            </div>
        `;
    }

    load(config) {
        this.loadFields(config);
    }

    save(config) {
        this.saveFields(config);
    }
}
