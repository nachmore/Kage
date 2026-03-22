/**
 * Focus Tracker search provider — shows activity reports via keyword trigger.
 * Subcommands: focus today, focus week, focus month, focus all
 */

const PERIODS = {
    'today': 'today',
    'week': 'week',
    'month': 'month',
    'all': 'all',
};

export default class FocusTrackerSearchProvider {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
        this._cache = new Map();
        this._started = false;

        // Auto-start tracker if configured
        if (this.config.auto_start !== false) {
            this._ensureStarted();
        }
    }

    onConfigUpdate(config) {
        this.config = config || {};
        this._cache.clear();
        if (this.config.auto_start !== false && !this._started) {
            this._ensureStarted();
        }
    }

    match(query) {
        const parsed = this._parseQuery(query);
        if (!parsed) return [];

        // Return cached report if available
        const cached = this._cache.get(parsed.period);
        if (cached && Date.now() - cached.time < 10000) {
            return this._formatReport(cached.data, parsed.period);
        }

        return [];
    }

    async matchAsync(query) {
        const parsed = this._parseQuery(query);
        if (!parsed) return [];

        await this._ensureStarted();

        // Check cache (10s TTL)
        const cached = this._cache.get(parsed.period);
        if (cached && Date.now() - cached.time < 10000) {
            return [];
        }

        try {
            const report = await this.invoke('get_activity_report', { period: parsed.period });
            this._cache.set(parsed.period, { data: report, time: Date.now() });
            return this._formatReport(report, parsed.period);
        } catch (e) {
            console.warn('[FocusTracker] Report failed:', e);
            return [{
                id: 'focus-error',
                type: 'focus-tracker',
                label: '📊 Could not load activity report',
                description: String(e),
                icon: '📊',
                score: 85,
                data: { type: 'error' },
            }];
        }
    }

    execute(result) {
        if (result.data?.type === 'insight') {
            return { type: 'send_prompt', value: result.data.prompt };
        }
        if (result.data?.copyText) {
            return { type: 'copy', value: result.data.copyText };
        }
        return null;
    }

    renderResult(result, element) {
        if (result.data?.type === 'summary') {
            element.innerHTML = this._renderSummaryHtml(result.data);
            return true;
        }
        if (result.data?.type === 'app-row') {
            element.innerHTML = this._renderAppRowHtml(result.data);
            return true;
        }
        if (result.data?.type === 'insight') {
            element.innerHTML = this._renderInsightHtml(result.data);
            return true;
        }
        return false;
    }

    destroy() {
        this._cache.clear();
    }

    // --- Private ---

    _parseQuery(query) {
        const trigger = (this.config.trigger ?? 'focus').trim().toLowerCase();
        const trimmed = query.trim().toLowerCase();

        if (!trigger) return null;
        if (!trimmed.startsWith(trigger)) return null;

        const rest = trimmed.slice(trigger.length).trim();

        // "focus" alone defaults to "today"
        if (!rest) return { period: 'today' };

        // Match period
        for (const [key, value] of Object.entries(PERIODS)) {
            if (rest === key || rest.startsWith(key)) {
                return { period: value };
            }
        }

        return null;
    }

    async _ensureStarted() {
        if (this._started) return;
        try {
            const running = await this.invoke('is_activity_tracker_running');
            if (!running) {
                const interval = this.config.poll_interval || 5;
                await this.invoke('start_activity_tracker', { pollInterval: interval });
            }
            this._started = true;
        } catch (e) {
            console.warn('[FocusTracker] Failed to start tracker:', e);
        }
    }

    _formatReport(report, period) {
        const results = [];

        // Summary card
        const totalHrs = (report.total_seconds / 3600).toFixed(1);
        const totalMin = Math.round(report.total_seconds / 60);
        const timeStr = report.total_seconds >= 3600 ? `${totalHrs}h` : `${totalMin}m`;
        const streakMin = Math.round(report.longest_streak_seconds / 60);

        results.push({
            id: `focus-summary-${period}`,
            type: 'focus-tracker',
            label: `📊 ${report.period}: ${timeStr} tracked`,
            description: `${report.context_switches} switches · ${streakMin}m best streak (${report.longest_streak_app})`,
            icon: '📊',
            score: 86,
            data: {
                type: 'summary',
                period: report.period,
                timeStr,
                switches: report.context_switches,
                streakMin,
                streakApp: report.longest_streak_app,
                appCount: report.apps.length,
                copyText: `${report.period}: ${timeStr} tracked, ${report.context_switches} switches, ${streakMin}m best streak (${report.longest_streak_app})`,
            },
        });

        // Top apps (max 5)
        const showApps = this.config.track_screen_time !== false;
        if (showApps) {
            for (let i = 0; i < Math.min(report.apps.length, 5); i++) {
                const app = report.apps[i];
                const appMin = Math.round(app.seconds / 60);
                const appHrs = (app.seconds / 3600).toFixed(1);
                const appTime = app.seconds >= 3600 ? `${appHrs}h` : `${appMin}m`;
                const pct = app.percentage.toFixed(0);
                results.push({
                    id: `focus-app-${period}-${app.process_name}`,
                    type: 'focus-tracker',
                    label: `${app.display_name}: ${appTime} (${pct}%)`,
                    description: `${app.switches_to} sessions`,
                    icon: _appEmoji(app.process_name),
                    score: 85 - i,
                    data: {
                        type: 'app-row',
                        name: app.display_name,
                        time: appTime,
                        pct: parseFloat(pct),
                        sessions: app.switches_to,
                        copyText: `${app.display_name}: ${appTime} (${pct}%), ${app.switches_to} sessions`,
                    },
                });
            }
        }

        // AI insight suggestion
        if (report.context_switches > 10 && report.apps.length > 2) {
            const topApps = report.apps.slice(0, 3).map(a => a.display_name).join(', ');
            results.push({
                id: `focus-insight-${period}`,
                type: 'focus-tracker',
                label: '💡 Get AI focus insights',
                description: 'Press Enter to ask the AI for productivity tips',
                icon: '💡',
                score: 80,
                data: {
                    type: 'insight',
                    prompt: `Here's my app usage for ${report.period.toLowerCase()}:\n\n` +
                        `Total tracked time: ${timeStr}\n` +
                        `Context switches: ${report.context_switches}\n` +
                        `Longest focus streak: ${streakMin} minutes (${report.longest_streak_app})\n` +
                        `Top apps: ${report.apps.slice(0, 5).map(a => `${a.display_name}: ${Math.round(a.seconds/60)}m (${a.percentage.toFixed(0)}%)`).join(', ')}\n\n` +
                        `Based on this data, give me 2-3 specific, actionable suggestions to improve my focus and reduce context switching. Be concise.`,
                },
            });
        }

        return results;
    }

    _renderSummaryHtml(data) {
        return `
            <div class="focus-result focus-summary">
                <span class="focus-icon">📊</span>
                <div class="focus-summary-content">
                    <div class="focus-title">${_esc(data.period)}: ${_esc(data.timeStr)} tracked</div>
                    <div class="focus-meta">${data.switches} switches · ${data.streakMin}m best streak (${_esc(data.streakApp)}) · ${data.appCount} apps</div>
                </div>
            </div>
        `;
    }

    _renderAppRowHtml(data) {
        return `
            <div class="focus-result focus-app-row">
                <div class="focus-bar-bg"><div class="focus-bar-fill" style="width:${Math.min(data.pct, 100)}%"></div></div>
                <div class="focus-app-info">
                    <span class="focus-app-name">${_esc(data.name)}</span>
                    <span class="focus-app-time">${_esc(data.time)} (${data.pct}%)</span>
                    <span class="focus-app-sessions">${data.sessions} sessions</span>
                </div>
            </div>
        `;
    }

    _renderInsightHtml(data) {
        return `
            <div class="focus-result focus-insight">
                <span class="focus-icon">💡</span>
                <span class="focus-insight-text">Get AI focus insights</span>
            </div>
        `;
    }
}

function _esc(s) {
    return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function _appEmoji(processName) {
    const map = {
        'code': '💻', 'chrome': '🌐', 'firefox': '🦊', 'msedge': '🌐',
        'slack': '💬', 'teams': '💬', 'discord': '💬', 'outlook': '📧',
        'explorer': '📁', 'windowsterminal': '⬛', 'spotify': '🎵',
        'winword': '📝', 'excel': '📊', 'powerpnt': '📽️', 'notepad': '📝',
    };
    return map[processName.toLowerCase()] || '🪟';
}
