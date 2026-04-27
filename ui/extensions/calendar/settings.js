/**
 * Calendar settings provider (sandboxed).
 */
export default class CalendarSettingsProvider {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    getSettings() {
        return {
            description: 'Type "cal" or "meetings" to see upcoming events.',
            sections: [
                {
                    controls: [
                        {
                            type: 'checkbox',
                            id: 'show_overlay',
                            label: 'Show next meeting overlay',
                            description: 'Display the next upcoming meeting above the input with a join button.',
                            default: true,
                        },
                        {
                            type: 'number',
                            id: 'lookahead_hours',
                            label: 'Lookahead (hours)',
                            description: 'How far ahead to look for meetings (1–72 hours).',
                            default: 8,
                            min: 1,
                            max: 72,
                            maxWidth: 80,
                        },
                        {
                            type: 'action',
                            id: 'test',
                            label: '🔄 Test Calendar Access',
                            action: 'test',
                        },
                        {
                            type: 'info',
                            label: 'Launcher Commands',
                            html: '<code>cal</code> or <code>meetings</code> — show upcoming events<br>'
                                + '<code>cal tomorrow</code> — events for a specific day<br>'
                                + '<code>cal-refresh</code> — force refresh calendar data from Outlook',
                        },
                    ],
                },
            ],
        };
    }

    validate(values) {
        const hours = Number(values.lookahead_hours);
        if (!Number.isFinite(hours) || hours < 1 || hours > 72) {
            return { valid: false, error: 'Lookahead must be between 1 and 72 hours' };
        }
        return { valid: true };
    }

    async runAction(action, values) {
        if (action === 'test') {
            try {
                const hours = Number(values.lookahead_hours) || 8;
                const events = await this.invoke('get_calendar_events', { hours });
                const count = events?.length || 0;
                return {
                    status: `✅ Found ${count} event${count === 1 ? '' : 's'} in the next ${hours} hour${hours === 1 ? '' : 's'}`,
                };
            } catch (e) {
                return { status: `❌ ${e?.message || e}` };
            }
        }
        return {};
    }
}
