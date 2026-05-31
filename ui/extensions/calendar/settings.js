/**
 * Calendar settings provider (sandboxed).
 */
export default class CalendarSettingsProvider {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
        this.t = context.i18n?.t?.bind(context.i18n) || ((k) => k);
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    getSettings() {
        const t = this.t;
        return {
            description: t('settings.description'),
            sections: [
                {
                    controls: [
                        {
                            type: 'checkbox',
                            id: 'show_overlay',
                            label: t('settings.show_overlay.label'),
                            description: t('settings.show_overlay.description'),
                            default: true,
                        },
                        {
                            type: 'number',
                            id: 'lookahead_hours',
                            label: t('settings.lookahead.label'),
                            description: t('settings.lookahead.description'),
                            default: 8,
                            min: 1,
                            max: 72,
                            maxWidth: 80,
                        },
                        {
                            type: 'action',
                            id: 'test',
                            label: t('settings.test_btn'),
                            action: 'test',
                        },
                        {
                            type: 'info',
                            label: t('settings.commands.label'),
                            html: t('settings.commands.html'),
                        },
                    ],
                },
            ],
        };
    }

    validate(values) {
        const hours = Number(values.lookahead_hours);
        if (!Number.isFinite(hours) || hours < 1 || hours > 72) {
            return { valid: false, error: this.t('settings.lookahead.error') };
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
                    status: this.t('settings.test.success', { count, hours }),
                };
            } catch (e) {
                const msg = e?.message || e || '';
                // If it's a permission error, open System Settings to the Calendar privacy pane
                if (typeof msg === 'string' && (msg.toLowerCase().includes('denied') || msg.toLowerCase().includes('permission'))) {
                    try {
                        await this.invoke('open_url', { url: 'x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Calendars' });
                    } catch (_) { /* best effort */ }
                    return { status: this.t('settings.test.denied') };
                }
                return { status: this.t('settings.test.failed', { message: msg }) };
            }
        }
        return {};
    }
}
