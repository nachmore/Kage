/**
 * Focus Tracker Settings Module
 */
class FocusTrackerExtSettingsModule extends SettingsModule {
    constructor() {
        super('focus-tracker', 'Focus Tracker', '📊');
        this.description = 'Track app usage, context switches, and focus streaks. Type your trigger keyword to see reports.';
    }

    renderContent() {
        return `
            ${this.createControlRow(
                'Trigger Keyword',
                'Type this keyword to see focus reports (e.g. "focus today", "focus week").',
                '<input type="text" class="setting-input" id="focusTrigger" placeholder="focus" value="focus" style="width: 100px;">'
            )}
            ${this.createControlRow(
                'Poll Interval (seconds)',
                'How often to check the active window. Lower = more accurate but uses more resources.',
                '<input type="number" class="setting-input" id="focusPollInterval" min="2" max="60" value="5" style="width: 80px;">'
            )}
            ${this.createCheckboxRow('Auto-start Tracking', 'Start tracking automatically when the app launches', 'focusAutoStart', true)}
            <div class="setting-section-label" style="margin-top: 16px; font-weight: 600; font-size: 13px;">Data to Track</div>
            ${this.createCheckboxRow('Screen Time', 'Track time spent in each application', 'focusTrackScreenTime', true)}
            ${this.createCheckboxRow('Context Switches', 'Count how often you switch between apps', 'focusTrackSwitches', true)}
            ${this.createCheckboxRow('Focus Streaks', 'Track longest uninterrupted focus periods', 'focusTrackStreaks', true)}
        `;
    }

    render() { return this.renderContent(); }

    load(config) {
        const ext = (config.extensions && config.extensions['focus-tracker']) || {};
        const el = (id, def) => document.getElementById(id);
        const trigger = el('focusTrigger');
        const interval = el('focusPollInterval');
        const autoStart = el('focusAutoStart');
        const screenTime = el('focusTrackScreenTime');
        const switches = el('focusTrackSwitches');
        const streaks = el('focusTrackStreaks');
        if (trigger) trigger.value = ext.trigger ?? 'focus';
        if (interval) interval.value = ext.poll_interval ?? 5;
        if (autoStart) autoStart.checked = ext.auto_start !== false;
        if (screenTime) screenTime.checked = ext.track_screen_time !== false;
        if (switches) switches.checked = ext.track_switches !== false;
        if (streaks) streaks.checked = ext.track_streaks !== false;
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions['focus-tracker'] = {
            trigger: document.getElementById('focusTrigger')?.value ?? 'focus',
            poll_interval: parseInt(document.getElementById('focusPollInterval')?.value ?? '5'),
            auto_start: document.getElementById('focusAutoStart')?.checked ?? true,
            track_screen_time: document.getElementById('focusTrackScreenTime')?.checked ?? true,
            track_switches: document.getElementById('focusTrackSwitches')?.checked ?? true,
            track_streaks: document.getElementById('focusTrackStreaks')?.checked ?? true,
        };
    }

    validate() {
        const interval = parseInt(document.getElementById('focusPollInterval')?.value ?? '5');
        if (interval < 2 || interval > 60) {
            return { valid: false, error: 'Poll interval must be between 2 and 60 seconds' };
        }
        return { valid: true };
    }
}

window.FocusTrackerExtSettingsModule = FocusTrackerExtSettingsModule;
