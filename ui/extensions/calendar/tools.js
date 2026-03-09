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
                description: 'List upcoming calendar appointments within a time window',
                parameters: {
                    hours_ahead: {
                        type: 'number',
                        description: 'Hours to look ahead from now',
                        default: 8,
                    },
                },
            },
        ];
    }

    async execute(toolName, params) {
        if (toolName === 'list_appointments') {
            return this._listAppointments(params);
        }
        return { error: `Unknown tool: ${toolName}` };
    }

    async _listAppointments(params) {
        const hoursAhead = params.hours_ahead ?? this.config?.lookahead_hours ?? 8;
        try {
            const events = await this.invoke('get_calendar_events', {
                hoursAhead,
            });

            if (!events || events.length === 0) {
                return { result: { appointments: [], message: 'No upcoming appointments found.' } };
            }

            const appointments = events.map(e => ({
                subject: e.subject,
                start: e.start_time,
                end: e.end_time,
                location: e.location || null,
                is_online: e.is_online_meeting || false,
                join_url: e.join_url || null,
                organizer: e.organizer || null,
            }));

            return { result: { appointments, count: appointments.length } };
        } catch (e) {
            return { error: `Failed to fetch calendar events: ${e.message || e}` };
        }
    }

    destroy() {}
}
