/**
 * Focus Tracker settings provider (sandboxed).
 */
export default class FocusTrackerSettingsProvider {
    initialize(context) { this.config = context.config || {}; }
    onConfigUpdate(config) { this.config = config || {}; }

    getSettings() {
        return {
            description: 'Track app usage, context switches, and focus streaks. Type your trigger keyword to see reports.',
            sections: [
                {
                    controls: [
                        {
                            type: 'text',
                            id: 'trigger',
                            label: 'Trigger Keyword',
                            description: 'Type this keyword to see focus reports (e.g. "focus today", "focus week").',
                            default: 'focus',
                            placeholder: 'focus',
                            maxWidth: 120,
                        },
                        {
                            type: 'number',
                            id: 'poll_interval',
                            label: 'Poll Interval (seconds)',
                            description: 'How often to check the active window. Lower = more accurate but uses more resources.',
                            default: 5,
                            min: 2,
                            max: 60,
                            maxWidth: 80,
                        },
                        {
                            type: 'checkbox',
                            id: 'auto_start',
                            label: 'Auto-start Tracking',
                            description: 'Start tracking automatically when the app launches',
                            default: true,
                        },
                    ],
                },
                {
                    label: 'Data to Track',
                    controls: [
                        { type: 'checkbox', id: 'track_screen_time', label: 'Screen Time',       description: 'Track time spent in each application', default: true },
                        { type: 'checkbox', id: 'track_switches',    label: 'Context Switches', description: 'Count how often you switch between apps', default: true },
                        { type: 'checkbox', id: 'track_streaks',     label: 'Focus Streaks',    description: 'Track longest uninterrupted focus periods', default: true },
                    ],
                },
            ],
        };
    }

    validate(values) {
        const n = Number(values.poll_interval);
        if (!Number.isFinite(n) || n < 2 || n > 60) {
            return { valid: false, error: 'Poll interval must be between 2 and 60 seconds' };
        }
        return { valid: true };
    }
}
