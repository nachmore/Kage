/**
 * Window Walker settings provider (sandboxed).
 *
 * Stored config uses `trigger` with a trailing space (that's the
 * activation delimiter). The UI shows it trimmed to avoid exposing
 * that implementation detail to the user.
 */
export default class WindowWalkerSettingsProvider {
    initialize(context) {
        this.config = context.config || {};
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    getSettings() {
        const storedTrigger = (this.config.trigger ?? 'w ').replace(/\s+$/, '');
        return {
            description: 'Quickly switch between open windows. Type the trigger keyword to list and filter windows.',
            sections: [
                {
                    controls: [
                        {
                            type: 'text',
                            id: 'trigger',
                            label: 'Trigger keyword',
                            description: 'Type this in the floating window to activate window search. (A trailing space is added automatically.)',
                            default: storedTrigger,
                            placeholder: 'w',
                            maxWidth: 120,
                        },
                        {
                            type: 'checkbox',
                            id: 'show_icons',
                            label: 'Show window icons',
                            description: 'Extract and display application icons next to each window. Disable for faster results on slower machines.',
                            default: true,
                        },
                        {
                            type: 'checkbox',
                            id: 'hide_minimized',
                            label: 'Hide minimized windows',
                            description: 'Exclude minimized windows from the list.',
                            default: false,
                        },
                    ],
                },
            ],
        };
    }

    validate(values) {
        const trigger = String(values.trigger || '').trim();
        if (!trigger) {
            return { valid: false, error: 'Trigger keyword cannot be empty' };
        }
        return { valid: true };
    }

    /**
     * Re-apply the trailing-space invariant. The search provider uses
     * a startsWith check against `config.trigger`, so the trigger needs
     * to include the activation delimiter. We hide that from the user
     * in the settings UI and re-add it here at save time.
     */
    normalize(values) {
        const trigger = String(values.trigger || '').trim() + ' ';
        return { values: { ...values, trigger } };
    }
}
