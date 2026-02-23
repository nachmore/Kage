/**
 * Math Settings Module
 * Controls the inline math calculator feature
 */
class MathSettingsModule extends SettingsModule {
    constructor() {
        super('math', 'Math', '🧮');
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
        const math = config.math || {};
        const enabled = document.getElementById('mathEnabled');
        const precision = document.getElementById('mathPrecision');
        const autoCopy = document.getElementById('mathAutoCopy');
        const thousands = document.getElementById('mathThousandsSeparator');

        if (enabled) enabled.checked = math.enabled !== false; // default true
        if (precision) precision.value = math.precision ?? 0;
        if (autoCopy) autoCopy.checked = math.auto_copy !== false; // default true
        if (thousands) thousands.checked = math.thousands_separator === true;
    }

    save(config) {
        config.math = {
            enabled: document.getElementById('mathEnabled')?.checked ?? true,
            precision: parseInt(document.getElementById('mathPrecision')?.value ?? '0'),
            auto_copy: document.getElementById('mathAutoCopy')?.checked ?? true,
            thousands_separator: document.getElementById('mathThousandsSeparator')?.checked ?? false,
        };
    }

    validate() {
        const precision = parseInt(document.getElementById('mathPrecision')?.value ?? '0');
        if (precision < 0 || precision > 15) {
            return { valid: false, error: 'Math precision must be between 0 and 15' };
        }
        return { valid: true };
    }
}
