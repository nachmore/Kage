/**
 * Calendar search provider — shows upcoming meetings for >cal / >meetings queries.
 * Also manages the next-meeting overlay bar in the floating window.
 */
export default class CalendarSearchProvider {
    constructor() {
        this._events = [];
        this._lastFetch = 0;
        this._overlayInterval = null;
        this._config = {};
        this._dismissedIds = new Set(); // Track all dismissed event IDs
    }

    async initialize(context) {
        this._invoke = context.invoke;
        this._config = context.config || {};
        console.log('[Calendar] Extension initialized, config:', JSON.stringify(this._config));
        // Start overlay polling if enabled
        if (this._config.show_overlay !== false) {
            console.log('[Calendar] Starting overlay polling');
            this._startOverlayPolling();
        } else {
            console.log('[Calendar] Overlay disabled in config');
        }
    }

    async onConfigUpdate(config) {
        this._config = config || {};
        if (this._config.show_overlay !== false) {
            this._startOverlayPolling();
        } else {
            this._stopOverlayPolling();
            this._hideOverlay();
        }
    }

    destroy() {
        this._stopOverlayPolling();
        this._hideOverlay();
    }

    // --- Search provider ---

    match(query) {
        return []; // sync — no results
    }

    async matchAsync(query) {
        const q = query.toLowerCase().trim();
        const triggers = ['cal', 'calendar', 'meetings'];
        const isCalQuery = triggers.some(t => q === t || q.startsWith(t + ' '));
        const isRefresh = q === 'cal-refresh';
        if (!isCalQuery && !isRefresh) return [];

        if (isRefresh) {
            return [{
                type: 'calendar_refresh',
                label: 'Refresh calendar',
                description: 'Force refresh calendar data from Outlook',
                icon: '🔄',
                score: 90,
                data: { action: 'refresh' },
                _extensionId: 'calendar',
            }];
        }

        // Check for date-specific query: "cal 2026-03-15", "cal tomorrow", "cal next monday", etc.
        const dateArg = q.replace(/^(cal|calendar|meetings)\s*/i, '').trim();
        if (dateArg) {
            const resolved = this._resolveDate(dateArg);
            if (resolved) {
                return this._fetchEventsForDate(resolved);
            }
        }

        const events = await this._fetchEvents();
        if (events.length === 0) {
            return [{
                type: 'calendar_event',
                label: 'No upcoming meetings',
                description: 'No events found in the next ' + (this._config.lookahead_hours || 8) + ' hours',
                icon: '📅',
                score: 85,
                data: null,
                _extensionId: 'calendar',
            }];
        }
        return events.slice(0, 8).map(e => this._eventToResult(e));
    }

    execute(result) {
        if (result.type === 'calendar_refresh') {
            this._fetchEvents(true).then(() => this._updateOverlay());
            return { type: 'display', value: 'Calendar refreshed' };
        }
        if (result.data?.online_url) {
            return { type: 'url', value: result.data.online_url };
        }
        return null;
    }

    renderResult(result, container) {
        const e = result.data;
        if (!e) {
            // No events placeholder
            container.innerHTML = `
                <div class="app-icon">📅</div>
                <div class="app-info" style="flex:1;">
                    <div class="app-name">${result.label}</div>
                    <div class="app-description">${result.description || ''}</div>
                </div>
            `;
            return true;
        }
        const time = this._formatTimeWithDay(e.start_time);
        const dur = e.duration_minutes ? `${e.duration_minutes}m` : '';
        const joinBtn = e.online_url
            ? `<button class="timer-btn" style="font-size:11px;padding:2px 8px;" onclick="event.stopPropagation();window.__TAURI__.core.invoke('open_url',{url:'${e.online_url.replace(/'/g, "\\'")}'})">Join</button>`
            : '';
        container.innerHTML = `
            <div class="app-icon">📅</div>
            <div class="app-info" style="flex:1;">
                <div class="app-name">${this._escapeHtml(e.subject)}</div>
                <div class="app-description">${time}${dur ? ' · ' + dur : ''}${e.location ? ' · ' + this._escapeHtml(e.location) : ''}</div>
            </div>
            ${joinBtn}
        `;
        return true;
    }

    // --- Overlay ---

    _startOverlayPolling() {
        if (this._overlayInterval) return;
        this._updateOverlay(); // immediate
        this._overlayInterval = setInterval(() => this._updateOverlay(), 60_000); // every minute
    }

    _stopOverlayPolling() {
        if (this._overlayInterval) {
            clearInterval(this._overlayInterval);
            this._overlayInterval = null;
        }
    }

    async _updateOverlay() {
        const events = await this._fetchEvents();
        const now = new Date();
        const lookaheadMs = (this._config.lookahead_hours || 8) * 3600_000;
        const cutoff = new Date(now.getTime() + lookaheadMs);

        // Find all events that haven't ended yet within the lookahead window
        // Skip dismissed events (tracked by event ID)
        const upcoming = events.filter(e => {
            if (e.all_day) return false;
            if (this._dismissedIds.has(e.id)) return false;
            const start = new Date(e.start_time);
            const end = new Date(start.getTime() + (e.duration_minutes || 30) * 60000);
            return end > now && start <= cutoff;
        });

        if (upcoming.length === 0) {
            this._hideOverlay();
            return;
        }

        // Show the soonest event, with a count of concurrent/overlapping meetings
        const next = upcoming[0];
        const nextStart = new Date(next.start_time);
        const concurrent = upcoming.filter(e => {
            const s = new Date(e.start_time);
            return Math.abs(s - nextStart) < 15 * 60000; // within 15 min of each other
        }).length;

        this._showOverlay(next, concurrent);
    }

    _showOverlay(event, concurrentCount = 1) {
        let bar = document.getElementById('calendarOverlayBar');
        if (!bar) {
            bar = document.createElement('div');
            bar.id = 'calendarOverlayBar';
            bar.className = 'timer-bar';
            bar.style.cssText = 'cursor:default;';
            const inputContainer = document.querySelector('.input-container');
            if (inputContainer) inputContainer.parentNode.insertBefore(bar, inputContainer);
        }

        const start = new Date(event.start_time);
        const now = new Date();
        const diffMin = Math.round((start - now) / 60000);
        const dayPrefix = this._dayPrefix(start, now);
        let timeLabel;
        if (diffMin <= 0) {
            timeLabel = 'Now';
        } else if (diffMin < 60 && !dayPrefix) {
            timeLabel = `in ${diffMin}m`;
        } else {
            const time = start.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
            timeLabel = dayPrefix ? `${dayPrefix} ${time}` : time;
        }

        const joinHtml = event.online_url
            ? `<button class="timer-btn cal-join-btn" id="calendarJoinBtn" title="Join meeting">Join</button>`
            : '';
        const dismissHtml = `<button class="timer-btn" id="calendarDismissBtn" style="font-size:11px;padding:1px 4px;" title="Dismiss this meeting">✕</button>`;

        const concurrentHtml = concurrentCount > 1
            ? `<span style="font-size:10px;opacity:0.7;margin-left:4px;">+${concurrentCount - 1} more</span>`
            : '';

        bar.innerHTML = `
            <span class="timer-bar-icon">📅</span>
            <span class="timer-bar-time" style="flex:1;font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
                <strong>[${timeLabel}]</strong> ${this._escapeHtml(event.subject)}${concurrentHtml}
            </span>
            <div class="timer-bar-controls">
                ${joinHtml}
                ${dismissHtml}
            </div>
            <style>
                .cal-join-btn { font-size:11px; padding:2px 10px; border:1px solid var(--kiro-accent) !important; color:var(--kiro-text-bright) !important; background:transparent; }
                .cal-join-btn:hover { background:var(--kiro-accent) !important; color:#fff !important; }
            </style>
        `;
        bar.style.display = 'flex';

        // Prevent buttons from stealing focus (which hides the floating window)
        bar.querySelectorAll('button').forEach(btn => {
            btn.addEventListener('mousedown', e => { e.preventDefault(); e.stopPropagation(); });
        });

        const joinBtn = document.getElementById('calendarJoinBtn');
        if (joinBtn && event.online_url) {
            joinBtn.onclick = (e) => {
                e.stopPropagation();
                window.__TAURI__?.core?.invoke('open_url', { url: event.online_url });
            };
        }
        const dismissBtn = document.getElementById('calendarDismissBtn');
        if (dismissBtn) {
            dismissBtn.onclick = (e) => {
                e.stopPropagation();
                // Dismiss this specific event — immediately show the next one
                this._dismissedIds.add(event.id);
                this._hideOverlay();
                this._updateOverlay();
            };
        }

        // Trigger resize
        window.dispatchEvent(new Event('resize'));
    }

    _hideOverlay() {
        const bar = document.getElementById('calendarOverlayBar');
        if (bar) { bar.style.display = 'none'; bar.remove(); }
        window.dispatchEvent(new Event('resize'));
    }

    // --- Helpers ---

    async _fetchEvents(forceRefresh = false) {
        const now = Date.now();
        // Cache for 30 minutes (overlay polls every minute but reuses cached data)
        if (!forceRefresh && now - this._lastFetch < 1_800_000 && this._events.length > 0) {
            return this._events;
        }
        try {
            const hours = this._config.lookahead_hours || 8;
            this._events = await this._invoke('get_calendar_events', { hours });
            this._lastFetch = now;
            // Clear dismissed events on refresh
            this._dismissedIds.clear();
        } catch (e) {
            console.warn('[Calendar] Failed to fetch events:', e);
        }
        return this._events;
    }

    /**
     * Resolve a natural language date string to YYYY-MM-DD.
     * Supports: "today", "tomorrow", "yesterday", "YYYY-MM-DD",
     * "monday"–"sunday", "next monday"–"next sunday".
     */
    _resolveDate(input) {
        const s = input.toLowerCase().trim();
        const now = new Date();
        const fmt = (d) => d.toISOString().slice(0, 10);

        if (s === 'today') return fmt(now);
        if (s === 'tomorrow') {
            const d = new Date(now); d.setDate(d.getDate() + 1); return fmt(d);
        }
        if (s === 'yesterday') {
            const d = new Date(now); d.setDate(d.getDate() - 1); return fmt(d);
        }
        // ISO date
        if (/^\d{4}-\d{2}-\d{2}$/.test(s)) return s;

        // Day names: "monday", "next tuesday", etc.
        const days = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday'];
        const isNext = s.startsWith('next ');
        const dayName = isNext ? s.substring(5) : s;
        const dayIdx = days.indexOf(dayName);
        if (dayIdx !== -1) {
            const today = now.getDay();
            let diff = dayIdx - today;
            if (diff <= 0 || isNext) diff += 7;
            if (isNext && diff <= 7) diff += 0; // "next monday" = the coming one
            const d = new Date(now); d.setDate(d.getDate() + diff);
            return fmt(d);
        }

        return null;
    }

    /**
     * Fetch events for a specific date and return as search results.
     */
    async _fetchEventsForDate(dateStr) {
        try {
            const events = await this._invoke('get_calendar_events_for_date', { date: dateStr });
            const label = this._formatDateLabel(dateStr);
            if (!events || events.length === 0) {
                return [{
                    type: 'calendar_event',
                    label: `No meetings on ${label}`,
                    description: dateStr,
                    icon: '📅',
                    score: 85,
                    data: null,
                    _extensionId: 'calendar',
                }];
            }
            return events.slice(0, 10).map(e => this._eventToResult(e));
        } catch (e) {
            console.warn('[Calendar] Failed to fetch events for date:', e);
            return [];
        }
    }

    /**
     * Format a YYYY-MM-DD date as a friendly label.
     */
    _formatDateLabel(dateStr) {
        try {
            const d = new Date(dateStr + 'T00:00:00');
            return d.toLocaleDateString(undefined, { weekday: 'long', month: 'short', day: 'numeric' });
        } catch {
            return dateStr;
        }
    }

    _eventToResult(event) {
        const timeStr = this._formatTimeWithDay(event.start_time);
        const dur = event.duration_minutes ? `${event.duration_minutes}m` : '';
        const parts = [timeStr, dur, event.location].filter(Boolean);
        return {
            type: 'calendar_event',
            label: event.subject,
            description: parts.join(' · '),
            icon: '📅',
            score: 85,
            data: event,
            _extensionId: 'calendar',
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
        if (diffDays === 0) return null; // today — no prefix
        if (diffDays === 1) return '[Tomorrow]';
        return '[' + eventDate.toLocaleDateString(undefined, { weekday: 'long' }) + ']';
    }

    _formatTime(isoString) {
        try {
            const d = new Date(isoString);
            return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
        } catch { return ''; }
    }

    _escapeHtml(str) {
        return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
    }
}
