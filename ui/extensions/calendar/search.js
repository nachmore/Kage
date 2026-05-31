import { initCache, getEvents } from './cache.js';

/**
 * Calendar search provider.
 * Runs in the sandbox. Overlay logic lives in widget.js now.
 */
export default class CalendarSearchProvider {
    async initialize(context) {
        this._invoke = context.invoke;
        initCache(context.invoke);
        this._config = context.config || {};
        this.t = context.i18n?.t?.bind(context.i18n) || ((k) => k);
    }

    async onConfigUpdate(config) {
        this._config = config || {};
    }

    destroy() {}

    match(_query) { return []; }

    async matchAsync(query) {
        const q = query.toLowerCase().trim();
        const triggers = ['cal', 'calendar', 'meetings'];
        const isCalQuery = triggers.some(t => q === t || q.startsWith(t + ' '));
        const isRefresh = q === 'cal-refresh';
        if (!isCalQuery && !isRefresh) return [];

        if (isRefresh) {
            return [{
                id: 'cal:refresh',
                type: 'calendar_refresh',
                label: this.t('result.refresh.label'),
                description: this.t('result.refresh.description'),
                icon: '🔄',
                score: 90,
                data: { action: 'refresh' },
            }];
        }

        const dateArg = q.replace(/^(cal|calendar|meetings)\s*/i, '').trim();
        if (dateArg) {
            const resolved = this._resolveDate(dateArg);
            if (resolved) return this._fetchEventsForDate(resolved);

            const events = await this._fetchEvents();
            const filtered = events.filter(e => {
                const haystack = [e.subject, e.location, e.organizer].filter(Boolean).join(' ').toLowerCase();
                return haystack.includes(dateArg.toLowerCase());
            });
            if (filtered.length === 0) {
                return [{
                    id: 'cal:no-match',
                    type: 'calendar_event',
                    label: this.t('result.no_match.label', { query: dateArg }),
                    description: this.t('result.no_match.description'),
                    icon: '📅',
                    score: 85,
                    data: null,
                }];
            }
            return filtered.slice(0, 8).map(e => this._eventToResult(e));
        }

        const events = await this._fetchEvents();
        if (events.length === 0) {
            return [{
                id: 'cal:no-upcoming',
                type: 'calendar_event',
                label: this.t('result.no_upcoming.label'),
                description: this.t('result.no_upcoming.description', {
                    count: this._config.lookahead_hours || 8,
                }),
                icon: '📅',
                score: 85,
                data: null,
            }];
        }
        return events.slice(0, 8).map(e => this._eventToResult(e));
    }

    execute(result) {
        if (result.type === 'calendar_refresh') {
            this._fetchEvents(true);
            return { type: 'display', value: this.t('result.refresh.executed') };
        }
        const e = result.data;
        if (!e) return null;

        const lines = [`📅 **${e.subject}**`];
        const time = this._formatTimeWithDay(e.start_time);
        const dur = this._formatDuration(e.duration_minutes);
        lines.push(`🕐 ${time}${dur ? ' · ' + dur : ''}`);
        if (e.location) {
            const cleanLoc = this._stripUrlsFromLocation(e.location);
            if (cleanLoc) lines.push(`📍 ${this._formatLocation(cleanLoc)}`);
        }
        if (e.organizer) lines.push(`👤 ${e.organizer}`);
        if (e.online_url) {
            const provider = this._meetingProvider(e.online_url);
            const joinLabel = this.t('result.join_btn');
            lines.push(`🔗 [${joinLabel} ${provider}](${e.online_url})`);
        }
        const info = lines.join('\n');

        // Kick off "open URL" asynchronously via the context invoke —
        // this returns a display action synchronously so the floating
        // window shows the formatted info immediately.
        if (e.online_url && this._invoke) {
            this._invoke('open_url', { url: e.online_url }).catch(() => {});
        }
        return { type: 'display', value: info };
    }

    /**
     * Custom render for a single search result row. Host sanitizes the
     * returned HTML before injecting. Buttons that should route back
     * must carry `data-ext-action="<id>"`; host wires them to
     * `onResultAction(actionId)`.
     */
    renderCustom(result) {
        const e = result?.data;
        if (!e) {
            return {
                html: `<div class="app-icon">📅</div>
                       <div class="app-info" style="flex:1;">
                           <div class="app-name">${escape(result?.label || '')}</div>
                           <div class="app-description">${escape(result?.description || '')}</div>
                       </div>`,
            };
        }
        const time = this._formatTimeWithDay(e.start_time);
        const dur = this._formatDuration(e.duration_minutes);
        const provider = e.online_url ? this._meetingProvider(e.online_url) : '';
        // Remember the event's URL so onResultAction('join') can open it
        // without the url itself being inlined into click-handlers.
        if (e.online_url) {
            this._lastJoinUrlByResultId = this._lastJoinUrlByResultId || new Map();
            this._lastJoinUrlByResultId.set(result.id, e.online_url);
        }
        const joinLabel = this.t('result.join_btn');
        const joinBtn = e.online_url
            ? `<button data-ext-action="join:${escape(result.id)}" class="extension-bar-btn"
                       style="font-size:11px;padding:2px 8px;" title="${escape(e.online_url)}">${escape(joinLabel)}${provider ? ' ' + escape(provider) : ''}</button>`
            : '';
        const locSuffix = e.location && !/^https?:\/\//i.test(e.location.trim()) ? ' · ' + escape(e.location) : '';
        return {
            html: `<div class="app-icon">📅</div>
                   <div class="app-info" style="flex:1;">
                       <div class="app-name">${escape(e.subject)}</div>
                       <div class="app-description">${escape(time)}${dur ? ' · ' + escape(dur) : ''}${provider ? ' · ' + escape(provider) : ''}${locSuffix}</div>
                   </div>
                   ${joinBtn}`,
        };
    }

    async onResultAction(actionId) {
        if (actionId?.startsWith('join:')) {
            const resultId = actionId.substring('join:'.length);
            const url = this._lastJoinUrlByResultId?.get(resultId);
            if (url && this._invoke) {
                await this._invoke('open_url', { url });
            }
        }
        return {};
    }

    // --- Helpers ---

    async _fetchEvents(forceRefresh = false) {
        const hours = this._config.lookahead_hours || 8;
        return getEvents({ hours, force: forceRefresh });
    }

    _resolveDate(input) {
        const s = input.toLowerCase().trim();
        const now = new Date();
        const fmt = (d) => d.toISOString().slice(0, 10);

        if (s === 'today') return fmt(now);
        if (s === 'tomorrow') { const d = new Date(now); d.setDate(d.getDate() + 1); return fmt(d); }
        if (s === 'yesterday') { const d = new Date(now); d.setDate(d.getDate() - 1); return fmt(d); }
        if (/^\d{4}-\d{2}-\d{2}$/.test(s)) return s;

        const days = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday'];
        const isNext = s.startsWith('next ');
        const dayName = isNext ? s.substring(5) : s;
        const dayIdx = days.indexOf(dayName);
        if (dayIdx !== -1) {
            const today = now.getDay();
            let diff = dayIdx - today;
            if (diff <= 0 || isNext) diff += 7;
            const d = new Date(now); d.setDate(d.getDate() + diff);
            return fmt(d);
        }
        return null;
    }

    async _fetchEventsForDate(dateStr) {
        try {
            const events = await this._invoke('get_calendar_events_for_date', { date: dateStr });
            const label = this._formatDateLabel(dateStr);
            if (!events || events.length === 0) {
                return [{
                    id: 'cal:no-date:' + dateStr,
                    type: 'calendar_event',
                    label: this.t('result.no_date.label', { label }),
                    description: dateStr,
                    icon: '📅',
                    score: 85,
                    data: null,
                }];
            }
            return events.slice(0, 10).map(e => this._eventToResult(e));
        } catch (e) {
            console.warn('[Calendar] Failed to fetch events for date:', e);
            return [];
        }
    }

    _formatDateLabel(dateStr) {
        try {
            const d = new Date(dateStr + 'T00:00:00');
            return d.toLocaleDateString(undefined, { weekday: 'long', month: 'short', day: 'numeric' });
        } catch { return dateStr; }
    }

    _eventToResult(event) {
        const timeStr = this._formatTimeWithDay(event.start_time);
        const dur = this._formatDuration(event.duration_minutes);
        const provider = event.online_url ? this._meetingProvider(event.online_url) : '';
        const loc = event.location && !/^https?:\/\//i.test(event.location.trim()) ? event.location : '';
        const parts = [timeStr, dur, provider, loc].filter(Boolean);
        return {
            id: 'cal:' + (event.id || event.subject + ':' + event.start_time),
            type: 'calendar_event',
            label: event.subject,
            description: parts.join(' · '),
            icon: '📅',
            score: 85,
            data: event,
        };
    }

    _formatTimeWithDay(isoString) {
        try {
            const d = new Date(isoString);
            const now = new Date();
            const time = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
            const prefix = this._dayPrefix(d, now);
            return prefix ? `${prefix} ${time}` : time;
        } catch { return ''; }
    }

    _dayPrefix(eventDate, now) {
        const todayStart = new Date(now); todayStart.setHours(0,0,0,0);
        const eventDay = new Date(eventDate); eventDay.setHours(0,0,0,0);
        const diffDays = Math.round((eventDay - todayStart) / 86400000);
        if (diffDays === 0) return null;
        if (diffDays === 1) return this.t('result.day.tomorrow');
        return '[' + eventDate.toLocaleDateString(undefined, { weekday: 'long' }) + ']';
    }

    _formatDuration(minutes) {
        if (!minutes) return '';
        if (minutes < 60) return this.t('result.duration.minutes', { minutes });
        if (minutes < 1440) {
            const h = Math.floor(minutes / 60);
            const m = minutes % 60;
            return m > 0
                ? this.t('result.duration.hours_minutes', { hours: h, minutes: m })
                : this.t('result.duration.hours', { hours: h });
        }
        const days = Math.round(minutes / 1440);
        return days === 1 ? this.t('result.duration.day') : this.t('result.duration.days', { days });
    }

    _meetingProvider(url) {
        if (!url) return '';
        const lower = url.toLowerCase();
        if (lower.includes('zoom.us') || lower.includes('zoom.com')) return 'Zoom';
        if (lower.includes('teams.microsoft') || lower.includes('teams.live')) return 'Teams';
        if (lower.includes('meet.google') || lower.includes('hangouts.google')) return 'Google Meet';
        if (lower.includes('webex') || lower.includes('cisco.com')) return 'Webex';
        if (lower.includes('chime.aws') || lower.includes('chime://')) return 'Chime';
        if (lower.includes('slack.com') || lower.includes('slack://')) return 'Slack';
        if (lower.includes('bluejeans')) return 'BlueJeans';
        if (lower.includes('gotomeeting') || lower.includes('goto.com')) return 'GoTo';
        return 'Meeting';
    }

    _stripUrlsFromLocation(location) {
        if (!location) return '';
        return location
            .replace(/https?:\/\/\S+/gi, '')
            .replace(/[;,]\s*$/, '')
            .replace(/^\s*[;,]\s*/, '')
            .trim();
    }

    _formatLocation(location) {
        if (!location) return '';
        if (/^https?:\/\//i.test(location.trim())) return `[${location}](${location})`;
        const encoded = encodeURIComponent(location);
        return `[${location}](https://www.google.com/maps/search/?api=1&query=${encoded})`;
    }
}

function escape(str) {
    return String(str).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
