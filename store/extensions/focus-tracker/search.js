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

        // Valid trigger but no cache yet — show placeholder so Enter doesn't send to agent
        return [{
            id: `focus-loading-${parsed.period}`,
            type: 'focus-tracker',
            label: `📊 Loading ${parsed.period} report...`,
            description: 'Fetching activity data',
            icon: '📊',
            score: 86,
            data: { type: 'loading', period: parsed.period },
        }];
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

            // Pre-fetch comparison period so it's cached for the AI summary on Enter
            const compPeriod = this._getComparisonPeriod(parsed.period);
            if (compPeriod && !this._cache.has(compPeriod)) {
                this.invoke('get_activity_report', { period: compPeriod }).then(compReport => {
                    this._cache.set(compPeriod, { data: compReport, time: Date.now() });
                }).catch(() => {});
            }

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
        if (result.data?.type === 'loading') {
            const cached = this._cache.get(result.data.period);
            if (cached) {
                return this._buildPromptAction(cached.data, result.data.period);
            }
            return null;
        }
        if (result.data?.type === 'insight') {
            return { type: 'prompt', value: result.data.prompt };
        }
        if (result.data?.type === 'summary' && result.data.report) {
            return this._buildPromptAction(result.data.report, result.data.report.period);
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
        if (result.data?.type === 'site-row') {
            element.innerHTML = this._renderSiteRowHtml(result.data);
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
            description: `${report.context_switches} switches · ${streakMin}m best streak (${report.longest_streak_app}) · Enter for AI summary`,
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
                report, // full report for display on Enter
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
                    // Use high base score minus a small fraction per app to maintain order
                    // Sites use the same base minus smaller fractions to stay under their parent
                    score: 85 - (i * 0.01),
                    tooltip: `${app.display_name}: ${appTime} (${pct}%), ${app.switches_to} sessions`,
                    data: {
                        type: 'app-row',
                        name: app.display_name,
                        time: appTime,
                        pct: parseFloat(pct),
                        sessions: app.switches_to,
                        copyText: `${app.display_name}: ${appTime} (${pct}%), ${app.switches_to} sessions`,
                    },
                });

                // Browser site breakdown
                if (app.sites && app.sites.length > 0) {
                    for (let j = 0; j < Math.min(app.sites.length, 5); j++) {
                        const site = app.sites[j];
                        const siteMin = Math.round(site.seconds / 60);
                        const siteTime = site.seconds >= 3600 ? `${(site.seconds / 3600).toFixed(1)}h` : `${siteMin}m`;
                        const sitePct = site.percentage.toFixed(0);
                        results.push({
                            id: `focus-site-${period}-${app.process_name}-${j}`,
                            type: 'focus-tracker',
                            label: `  ${site.site}: ${siteTime}`,
                            description: `${sitePct}% of ${app.display_name}`,
                            icon: '🔹',
                            score: 85 - (i * 0.01) - ((j + 1) * 0.001),
                            tooltip: `${site.site}: ${siteTime} (${sitePct}% of ${app.display_name})`,
                            data: {
                                type: 'site-row',
                                site: site.site,
                                parentApp: app.display_name,
                                time: siteTime,
                                pct: parseFloat(sitePct),
                                copyText: `${site.site}: ${siteTime} (${sitePct}% of ${app.display_name})`,
                            },
                        });
                    }
                }
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

    _renderSiteRowHtml(data) {
        return `
            <div class="focus-result focus-site-row">
                <div class="focus-bar-bg focus-bar-indent"><div class="focus-bar-fill focus-bar-site" style="width:${Math.min(data.pct, 100)}%"></div></div>
                <div class="focus-app-info">
                    <span class="focus-app-name focus-site-name">🔹 ${_esc(data.site)}</span>
                    <span class="focus-app-time">${_esc(data.time)}</span>
                    <span class="focus-app-sessions">${data.pct}% of ${_esc(data.parentApp)}</span>
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

    _getComparisonPeriod(period) {
        const map = { 'today': 'week', 'week': 'month', 'month': 'all' };
        return map[period] || null;
    }

    _buildPromptAction(report, period) {
        const compPeriod = this._getComparisonPeriod(period);
        const compCached = compPeriod ? this._cache.get(compPeriod) : null;

        let prompt = `Here is my focus/activity tracking data. Please give me a brief summary of what's going on, and 2-3 actionable recommendations.\n\n`;
        prompt += `## ${report.period} Report\n`;
        prompt += this._formatReportForPrompt(report);

        if (compCached) {
            const compLabel = compPeriod === 'all' ? 'All Time' : compPeriod.charAt(0).toUpperCase() + compPeriod.slice(1);
            prompt += `\n## ${compLabel} Report (for context)\n`;
            prompt += this._formatReportForPrompt(compCached.data);
            prompt += `\nCompare my ${report.period.toLowerCase()} to the ${compLabel.toLowerCase()} data — are things trending better or worse? What stands out?\n`;
        }

        prompt += `\nBe concise and specific. Focus on patterns, not just restating numbers.`;
        return { type: 'prompt', value: prompt };
    }

    _formatReportForPrompt(report) {
        const totalMin = Math.round(report.total_seconds / 60);
        const timeStr = report.total_seconds >= 3600 ? `${(report.total_seconds/3600).toFixed(1)}h` : `${totalMin}m`;
        const streakMin = Math.round(report.longest_streak_seconds / 60);

        let text = `Total: ${timeStr} tracked, ${report.context_switches} context switches, ${streakMin}m longest streak (${report.longest_streak_app})\n`;
        text += `Apps:\n`;
        for (const app of report.apps.slice(0, 10)) {
            const t = app.seconds >= 3600 ? `${(app.seconds/3600).toFixed(1)}h` : `${Math.round(app.seconds/60)}m`;
            text += `- ${app.display_name}: ${t} (${app.percentage.toFixed(0)}%), ${app.switches_to} sessions\n`;
            if (app.sites) {
                for (const site of app.sites.slice(0, 3)) {
                    const st = site.seconds >= 3600 ? `${(site.seconds/3600).toFixed(1)}h` : `${Math.round(site.seconds/60)}m`;
                    text += `  - ${site.site}: ${st} (${site.percentage.toFixed(0)}% of ${app.display_name})\n`;
                }
            }
        }
        return text;
    }

    _buildSummaryMarkdown(data) {
        const r = data.report;
        const trigger = (this.config.trigger ?? 'focus').trim();
        let md = `## 📊 ${data.period}\n\n`;
        md += `**${data.timeStr}** tracked · **${data.switches}** context switches · **${data.streakMin}m** best streak (${data.streakApp}) · **${data.appCount}** apps\n\n`;

        if (r.apps && r.apps.length > 0) {
            md += `| App | Time | % |\n|-----|------|---|\n`;
            for (const app of r.apps.slice(0, 10)) {
                const t = app.seconds >= 3600 ? `${(app.seconds/3600).toFixed(1)}h` : `${Math.round(app.seconds/60)}m`;
                md += `| ${app.display_name} | ${t} | ${app.percentage.toFixed(0)}% |\n`;
                if (app.sites) {
                    for (const site of app.sites.slice(0, 3)) {
                        const st = site.seconds >= 3600 ? `${(site.seconds/3600).toFixed(1)}h` : `${Math.round(site.seconds/60)}m`;
                        md += `| · ${site.site} | ${st} | ${site.percentage.toFixed(0)}% of ${app.display_name} |\n`;
                    }
                }
            }
        }

        md += `\n---\n`;
        md += `Try: \`${trigger} week\` · \`${trigger} month\` · \`${trigger} all\``;
        return md;
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
