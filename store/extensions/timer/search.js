/**
 * Timer search provider — extracted from floating-timer.js.
 * Parses "timer 5m", "stopwatch", etc.
 */

export default class TimerSearchProvider {
    initialize(context) {
        this.config = context.config || {};
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    match(query) {
        const parsed = parseTimerCommand(query);
        if (!parsed) return [];
        return [{
            id: 'timer:' + parsed.type,
            type: 'timer_cmd',
            label: parsed.type === 'hint' ? 'Timer' : parsed.type === 'timer' ? 'Start Timer' : 'Stopwatch',
            description: parsed.type === 'hint' ? 'timer 5m · timer 1h30m · timer 90s' : 'Press Enter',
            icon: '⏱️',
            score: 91,
            data: parsed,
        }];
    }

    execute(result) {
        // Timer execution is handled by the widget/app layer, not a simple copy
        return { type: 'custom', data: result.data };
    }

    destroy() {}
}

// --- Timer command parsing (moved from floating-timer.js) ---

function parseTimerCommand(input) {
    const lower = input.trim().toLowerCase();
    if (lower === 'timer' || lower === 'countdown') {
        return { type: 'hint' };
    }
    const timerMatch = lower.match(/^(?:timer|countdown)\s+(.+)$/);
    if (timerMatch) {
        const ms = parseDuration(timerMatch[1]);
        if (ms && ms > 0) return { type: 'timer', durationMs: ms, label: timerMatch[1] };
    }
    if (lower === 'stopwatch' || lower === 'sw') {
        return { type: 'stopwatch' };
    }
    return null;
}

function parseDuration(str) {
    const s = str.trim().toLowerCase();
    // Pure number = seconds
    if (/^\d+$/.test(s)) return parseInt(s) * 1000;
    // "90s", "5m", "1h", "1h30m", "2m30s"
    let total = 0;
    const hMatch = s.match(/(\d+)\s*h/);
    const mMatch = s.match(/(\d+)\s*m(?!s)/);
    const sMatch = s.match(/(\d+)\s*s/);
    if (hMatch) total += parseInt(hMatch[1]) * 3600000;
    if (mMatch) total += parseInt(mMatch[1]) * 60000;
    if (sMatch) total += parseInt(sMatch[1]) * 1000;
    return total > 0 ? total : null;
}
