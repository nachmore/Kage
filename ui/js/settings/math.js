import { SettingsModule } from './base.js';
/**
 * Math Settings Module
 * Controls the inline math calculator feature
 */
export class MathSettingsModule extends SettingsModule {
    constructor() {
        super('math', 'Math', '🧮');
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
                    Evaluate math expressions directly in the input bar without sending them to the agent.
                </p>
                ${this.createCheckboxRow(
                    'Enable Math Calculator',
                    'Detect and evaluate math expressions as you type',
                    'mathEnabled',
                    true
                )}
                ${this.createControlRow(
                    'Decimal Precision',
                    'Number of decimal places to display (0 = auto)',
                    '<input type="number" class="setting-input" id="mathPrecision" min="0" max="15" value="0">'
                )}
                ${this.createCheckboxRow(
                    'Auto-copy Result',
                    'Automatically copy the answer to clipboard when pressing Enter',
                    'mathAutoCopy',
                    true
                )}
                ${this.createCheckboxRow(
                    'Use Thousands Separator',
                    'Format large numbers with commas (e.g. 1,000,000)',
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
            return { valid: false, error: 'Math precision must be between 0 and 15' };
        }
        return { valid: true };
    }
}
