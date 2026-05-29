import { SettingsModule } from './base.js';
import { t } from '../shared/i18n.js';
/**
 * Math Settings Module
 * Controls the inline math calculator feature
 */
export class MathSettingsModule extends SettingsModule {
    constructor() {
        super('math', t('settings.math.title'), '🧮');
        this.bindFields([
            { id: 'mathEnabled', path: 'math.enabled', kind: 'checkbox', default: true },
            { id: 'mathPrecision', path: 'math.precision', kind: 'int', default: 0 },
            { id: 'mathAutoCopy', path: 'math.auto_copy', kind: 'checkbox', default: true },
            {
                id: 'mathThousandsSeparator',
                path: 'math.thousands_separator',
                kind: 'checkbox',
                default: false,
            },
        ]);
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2>${this.icon} ${this.title}</h2>
                <p class="section-description">
                    ${t('settings.math.section_description')}
                </p>
                ${this.createCheckboxRow(
                    t('settings.math.enable.label'),
                    t('settings.math.enable.description'),
                    'mathEnabled',
                    true
                )}
                ${this.createControlRow(
                    t('settings.math.precision.label'),
                    t('settings.math.precision.description'),
                    '<input type="number" class="setting-input" id="mathPrecision" min="0" max="15" value="0">'
                )}
                ${this.createCheckboxRow(
                    t('settings.math.auto_copy.label'),
                    t('settings.math.auto_copy.description'),
                    'mathAutoCopy',
                    true
                )}
                ${this.createCheckboxRow(
                    t('settings.math.thousands.label'),
                    t('settings.math.thousands.description'),
                    'mathThousandsSeparator',
                    false
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

    validate() {
        const precision = parseInt(document.getElementById('mathPrecision')?.value ?? '0', 10);
        if (precision < 0 || precision > 15) {
            return { valid: false, error: t('settings.math.precision.error') };
        }
        return { valid: true };
    }
}
