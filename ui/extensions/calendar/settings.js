/**
 * Calendar Extension Settings Module
 */
class CalendarExtSettingsModule extends SettingsModule {
    constructor() {
        super('calendar', 'Calendar', '📅');
        this.description = 'Type "cal" or "meetings" to see upcoming events.';
    }

    renderContent() {
        return `
            ${this.createCheckboxRow(
                'Show next meeting overlay',
                'Display the next upcoming meeting above the input with a join button.',
                'calendarShowOverlay',
                true
            )}
            <div class="dialog-field" style="margin-top:12px;">
                <label style="font-size:13px;font-weight:500;">Lookahead (hours)</label>
                <input type="number" id="calendarLookahead" class="setting-input" min="1" max="72" value="8" style="width:80px;">
                <div class="setting-description" style="margin-top:4px;">How far ahead to look for meetings (1–72 hours).</div>
            </div>
            <div class="setting-row" style="margin-top:12px;">
                <button class="setting-button" id="calendarTestBtn">🔄 Test Calendar Access</button>
                <span class="setting-description" id="calendarTestStatus" style="margin-left:8px;"></span>
            </div>
        `;
    }

    initialize() {
        document.getElementById('calendarTestBtn')?.addEventListener('click', async () => {
            const status = document.getElementById('calendarTestStatus');
            if (status) status.textContent = '⏳ Checking...';
            try {
                const invoke = window.__TAURI__.core.invoke;
                const hours = parseInt(document.getElementById('calendarLookahead')?.value || '8', 10);
                const events = await invoke('get_calendar_events', { hours });
                if (status) status.textContent = `✅ Found ${events.length} event${events.length !== 1 ? 's' : ''} in the next ${hours} hours`;
            } catch (e) {
                if (status) status.textContent = '❌ ' + e;
            }
        });
    }

    load(config) {
        const extConfig = config.extensions?.calendar || {};
        const overlay = document.getElementById('calendarShowOverlay');
        if (overlay) overlay.checked = extConfig.show_overlay !== false;
        const lookahead = document.getElementById('calendarLookahead');
        if (lookahead) lookahead.value = extConfig.lookahead_hours || 8;
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions.calendar = config.extensions.calendar || {};
        config.extensions.calendar.show_overlay = document.getElementById('calendarShowOverlay')?.checked ?? true;
        config.extensions.calendar.lookahead_hours = Math.min(72, Math.max(1, parseInt(document.getElementById('calendarLookahead')?.value || '8', 10)));
    }

    validate() { return { valid: true }; }
}

// Expose to global scope for dynamic script loading
window.CalendarExtSettingsModule = CalendarExtSettingsModule;
