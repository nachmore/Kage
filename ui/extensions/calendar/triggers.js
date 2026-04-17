import { initCache, getEvents } from './cache.js';

/**
 * Calendar trigger provider — emits signals for meeting events.
 */
export default class CalendarTriggerProvider {
    initialize(context) {
        this.invoke = context.invoke;
        initCache(context.invoke);
        this._interval = null;
        this._lastNotified = new Set();
        this._startPolling();
    }

    onConfigUpdate(config) {
        this._config = config || {};
    }

    getTriggers() {
        return [
            { name: 'calendar:meeting_starting', description: 'A meeting is starting within 5 minutes', icon: '📅' },
            { name: 'calendar:meeting_started', description: 'A meeting has started (time reached)', icon: '📅' },
            { name: 'calendar:day_summary', description: 'Daily summary of upcoming meetings', icon: '📅' },
        ];
    }

    _startPolling() {
        // Check every 60 seconds for upcoming meetings
        this._checkMeetings();
        this._interval = setInterval(() => this._checkMeetings(), 60_000);
    }

    async _checkMeetings() {
        if (!this.invoke) return;
        try {
            const events = await getEvents({ hours: 1 });
            const now = new Date();
            for (const e of events) {
                const start = new Date(e.start_time);
                const diffMin = Math.round((start - now) / 60000);
                const key = `${e.id || e.subject}_${e.start_time}`;

                if (diffMin <= 5 && diffMin > 0 && !this._lastNotified.has('starting_' + key)) {
                    this._lastNotified.add('starting_' + key);
                    this._emitSignal('calendar:meeting_starting', {
                        subject: e.subject, start_time: e.start_time,
                        location: e.location, online_url: e.online_url,
                        minutes_until: diffMin,
                    });
                }
                if (diffMin <= 0 && diffMin > -2 && !this._lastNotified.has('started_' + key)) {
                    this._lastNotified.add('started_' + key);
                    this._emitSignal('calendar:meeting_started', {
                        subject: e.subject, start_time: e.start_time,
                        location: e.location, online_url: e.online_url,
                    });
                }
            }
            // Clean old entries (older than 1 hour)
            if (this._lastNotified.size > 100) this._lastNotified.clear();
        } catch {}
    }

    _emitSignal(name, data) {
        if (!this.invoke) return;
        this.invoke('emit_automation_signal', { name, data }).catch(() => {});
    }

    destroy() {
        if (this._interval) { clearInterval(this._interval); this._interval = null; }
    }
}
