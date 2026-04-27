/**
 * Math settings provider (sandboxed).
 *
 * Runs inside the extension sandbox iframe. Returns a declarative schema;
 * the host renders and wires everything up. No DOM access from here.
 */
export default class MathSettingsProvider {
    initialize(context) {
        this.config = context.config || {};
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    getSettings() {
        return {
            description: 'Evaluate math expressions directly in the input bar without sending them to the agent.',
            sections: [
                {
                    controls: [
                        {
                            type: 'number',
                            id: 'precision',
                            label: 'Decimal Precision',
                            description: 'Number of decimal places to display (-1 = auto, 0 = integer)',
                            default: 2,
                            min: -1,
                            max: 15,
                            maxWidth: 80,
                        },
                        {
                            type: 'checkbox',
                            id: 'auto_copy',
                            label: 'Auto-copy Result',
                            description: 'Automatically copy the answer to clipboard when pressing Enter',
                            default: true,
                        },
                        {
                            type: 'checkbox',
                            id: 'thousands_separator',
                            label: 'Use Thousands Separator',
                            description: 'Format large numbers with commas (e.g. 1,000,000)',
                            default: false,
                        },
                    ],
                },
            ],
        };
    }

    validate(values) {
        const p = Number(values.precision);
        if (!Number.isFinite(p) || p < -1 || p > 15) {
            return { valid: false, error: 'Math precision must be between -1 and 15' };
        }
        return { valid: true };
    }
}
