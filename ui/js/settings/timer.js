/**
 * Timer & Stopwatch Settings Module
 */
class TimerSettingsModule extends SettingsModule {
    constructor() {
        super('timer', 'Timer & Stopwatch', '⏱️');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                ${this.createCheckboxRow(
                    'Enable timer & stopwatch',
                    'Type "timer 5m" for a countdown or "stopwatch" to count up.',
                    'timerEnabled',
                    true
                )}

                <div class="setting-row" style="margin-top: 16px;">
                    <div class="setting-label">When timer completes</div>
                </div>

                ${this.createCheckboxRow(
                    'Show system notification',
                    'Display a desktop notification when the countdown reaches zero.',
                    'timerNotify',
                    true
                )}

                ${this.createCheckboxRow(
                    'Play sound',
                    'Play a short beep when the countdown reaches zero.',
                    'timerSound',
                    true
                )}

                ${this.createCheckboxRow(
                    'Show floating window',
                    'Automatically show the floating window if it is hidden when the timer completes.',
                    'timerShowWindow',
                    true
                )}
            </div>
        `;
    }

    load(config) {
        const t = config.timer || {};
        const set = (id, val) => { const el = document.getElementById(id); if (el) el.checked = val !== false; };
        set('timerEnabled', t.enabled);
        set('timerNotify', t.notify_on_complete);
        set('timerSound', t.sound_on_complete);
        set('timerShowWindow', t.show_window_on_complete);
    }

    save(config) {
        config.timer = {
            enabled: document.getElementById('timerEnabled')?.checked ?? true,
            notify_on_complete: document.getElementById('timerNotify')?.checked ?? true,
            sound_on_complete: document.getElementById('timerSound')?.checked ?? true,
            show_window_on_complete: document.getElementById('timerShowWindow')?.checked ?? true,
        };
    }
}
