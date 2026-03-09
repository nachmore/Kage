/**
 * Calendar Tool Provider — exposes calendar data to the LLM agent.
 */
export default class CalendarToolProvider {
    initialize(context) {
        this.invoke = context.invoke;
        this.config = context.config;
    }

    onConfigUpdate(config) {
        this.config = config;
    }

    getTools() {
        return [
            {
                name: 'list_appointments',
                description: 'List upcoming calendar appointments within a time window from now',
                parameters: {
                    hours_ahead: {
                        type: 'number',
                        description: 'Hours to look ahead from now',
                        default: 8,
                    },
                },
            },
            {
                name: 'get_appointments_for_date',
                description: 'Get all calendar appointments for a specific date',
                parameters: {
                    date: {
                        type: 'string',
                        description: 'Date in YYYY-MM-DD format',
                    },
                },
            },
        ];
    }

    async execute(toolName, params) {
        if (toolName === 'list_appointments') return this._listAppointments(params);
        if (toolName === 'get_appointments_for_date') return this._getAppointmentsForDate(params);
        return { error: `Unknown tool: ${toolName}` };
    }

    async _listAppointments(params) {
        const hoursAhead = params.hours_ahead ?? this.config?.lookahead_hours ?? 8;
        try {
            const events = await this.invoke('get_calendar_events', { hoursAhead });
            return { result: { appointments: this._mapEvents(events), count: events?.length || 0 } };
        } catch (e) {
            return { error: `Failed to fetch calendar events: ${e.message || e}` };
        }
    }

    async _getAppointmentsForDate(params) {
        const date = params.date;
        if (!date || !/^\d{4}-\d{2}-\d{2}$/.test(date)) {
            return { error: 'Invalid date format. Use YYYY-MM-DD.' };
        }
        try {
            const events = await this.invoke('get_calendar_events_for_date', { date });
            return { result: { date, appointments: this._mapEvents(events), count: events?.length || 0 } };
        } catch (e) {
            return { error: `Failed to fetch events for ${date}: ${e.message || e}` };
        }
    }

    _mapEvents(events) {
        if (!events || events.length === 0) return [];
        return events.map(e => ({
            subject: e.subject,
            start: e.start_time,
            duration_minutes: e.duration_minutes,
            all_day: e.all_day || false,
            location: e.location || null,
            join_url: e.online_url || null,
            organizer: e.organizer || null,
        }));
    }

    destroy() {}
}
