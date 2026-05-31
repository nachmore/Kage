import { initCache, getEvents } from './cache.js';

/**
 * Calendar next-meeting overlay widget.
 * Mounts into the floating-bottom slot. Re-renders every 60 seconds.
 */
export default class CalendarNextMeetingWidget {
    initialize(context) {
        this._invoke = context.invoke;
        initCache(context.invoke);
        this._config = context.config || {};
        this._dismissedIds = new Set();
        this._cachedEvent = null;
        this._cachedConcurrent = 1;
        this.t = context.i18n?.t?.bind(context.i18n) || ((k) => k);
    }

    onConfigUpdate(config) {
        this._config = config || {};
    }

    getRefreshInterval() {
        // Poll once a minute so the "in Nm" countdown stays fresh.
        return this._config.show_overlay !== false ? 60_000 : 0;
    }

    async render() {
        if (this._config.show_overlay === false) return null;
        await this._updateNextEvent();
        const event = this._cachedEvent;
        if (!event) return null;

        const concurrent = this._cachedConcurrent;
        const start = new Date(event.start_time);
        const now = new Date();
        const diffMin = Math.round((start - now) / 60000);
        const dayPrefix = this._dayPrefix(start, now);

        let timeLabel;
        if (diffMin <= 0) {
            timeLabel = this.t('widget.now');
        } else if (diffMin < 60 && !dayPrefix) {
            timeLabel = this.t('widget.in_minutes', { minutes: diffMin });
        } else {
            const time = start.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
            timeLabel = dayPrefix ? `${dayPrefix} ${time}` : time;
        }

        const concurrentHtml = concurrent > 1
            ? `<span style="font-size:10px;opacity:0.7;margin-left:4px;">${escape(this.t('widget.concurrent_more', { count: concurrent - 1 }))}</span>`
            : '';
        const joinLabel = this.t('result.join_btn');
        const joinHtml = event.online_url
            ? `<button data-ext-action="join" class="extension-bar-btn cal-join-btn"
                       style="font-size:11px;padding:2px 10px;" title="${escape(event.online_url)}">${escape(joinLabel)}</button>`
            : '';

        return {
            className: 'extension-bar',
            html: `
                <span class="extension-bar-icon">📅</span>
                <span class="extension-bar-text" style="flex:1;font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
                    <strong>[${escape(timeLabel)}]</strong> ${escape(event.subject)}${concurrentHtml}
                </span>
                <div class="extension-bar-controls">
                    ${joinHtml}
                    <button data-ext-action="dismiss" class="extension-bar-btn"
                            style="font-size:11px;padding:1px 4px;" title="${escape(this.t('widget.dismiss_title'))}">✕</button>
                </div>
            `,
            actions: [
                { id: 'join', rpc: 'join' },
                { id: 'dismiss', rpc: 'dismiss' },
            ],
        };
    }

    async onAction(actionId) {
        if (actionId === 'join') {
            const e = this._cachedEvent;
            if (e?.online_url && this._invoke) {
                await this._invoke('open_url', { url: e.online_url });
            }
            return {};
        }
        if (actionId === 'dismiss') {
            const e = this._cachedEvent;
            if (e) this._dismissedIds.add(e.id);
            this._cachedEvent = null;
            // Ask host to re-render immediately so we move to the next event.
            await this._updateNextEvent();
            return { rerender: true };
        }
        return {};
    }

    destroy() {}

    // --- Internals ---

    async _updateNextEvent() {
        const hours = this._config.lookahead_hours || 8;
        const events = await getEvents({ hours });
        const now = new Date();
        const cutoff = new Date(now.getTime() + hours * 3600_000);

        const upcoming = events.filter(e => {
            if (e.all_day) return false;
            if (this._dismissedIds.has(e.id)) return false;
            const start = new Date(e.start_time);
            const end = new Date(start.getTime() + (e.duration_minutes || 30) * 60000);
            return end > now && start <= cutoff;
        });

        if (upcoming.length === 0) {
            this._cachedEvent = null;
            this._cachedConcurrent = 1;
            return;
        }

        const inProgress = upcoming.filter(e => new Date(e.start_time) <= now);
        inProgress.sort((a, b) => (b.online_url ? 1 : 0) - (a.online_url ? 1 : 0));
        const future = upcoming.filter(e => new Date(e.start_time) > now);

        let next;
        if (inProgress.length > 0 && future.length > 0) {
            const minsUntilNext = (new Date(future[0].start_time) - now) / 60000;
            next = minsUntilNext <= 10 ? future[0] : inProgress[0];
        } else if (inProgress.length > 0) {
            next = inProgress[0];
        } else {
            next = upcoming[0];
        }

        const nextStart = new Date(next.start_time);
        const concurrent = upcoming.filter(e => {
            const s = new Date(e.start_time);
            return Math.abs(s - nextStart) < 15 * 60000;
        }).length;

        this._cachedEvent = next;
        this._cachedConcurrent = concurrent;
    }

    _dayPrefix(eventDate, now) {
        const todayStart = new Date(now); todayStart.setHours(0,0,0,0);
        const eventDay = new Date(eventDate); eventDay.setHours(0,0,0,0);
        const diffDays = Math.round((eventDay - todayStart) / 86400000);
        if (diffDays === 0) return null;
        if (diffDays === 1) return '[Tomorrow]';
        return '[' + eventDate.toLocaleDateString(undefined, { weekday: 'long' }) + ']';
    }
}

function escape(s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
