/**
 * Timer Settings Module — extension version.
 * Reads/writes config from config.extensions['timer'].
 */
class TimerExtSettingsModule extends SettingsModule {
    constructor() {
        super('timer', 'Timer & Stopwatch', '⏱️');
        this.description = 'Type "timer 5m" for a countdown or "stopwatch" to count up.';
    }

    renderContent() {
        return `
            <div class="setting-row">
                <div class="setting-label">When timer completes</div>
            </div>

            ${this.createCheckboxRow('Show system notification', 'Display a desktop notification when the countdown reaches zero.', 'timerNotify', true)}
            ${this.createCheckboxRow('Play sound', 'Play a notification sound when the countdown reaches zero.', 'timerSound', true)}

            <div class="setting-row" id="timerSoundOptions">
                <div class="setting-label">Notification sound</div>
                <div class="setting-description">Select a sound to play when the timer completes.</div>
                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                    <select class="setting-input" id="timerSoundId" style="max-width:200px;">
                        <option value="two-tone">Two-Tone Beep</option>
                        <option value="chime">Chime</option>
                        <option value="alert">Alert</option>
                        <option value="gentle">Gentle</option>
                        <option value="bell">Bell</option>
                        <option value="success">Success</option>
                        <option value="custom">Custom file...</option>
                    </select>
                    <button class="setting-button" id="timerSoundPreview" title="Preview sound">▶ Preview</button>
                </div>
                <div id="timerCustomPathRow" style="display:none;margin-top:8px;">
                    <div class="setting-control">
                        <input type="text" class="setting-input" id="timerCustomSoundPath" placeholder="Path to .wav or .mp3 file" style="max-width:350px;">
                    </div>
                </div>
                <div style="margin-top:8px;">
                    <div class="setting-description">Repeat count</div>
                    <div class="setting-control" style="display:flex;gap:8px;align-items:center;margin-top:4px;">
                        <input type="range" id="timerSoundRepeats" min="1" max="10" value="3" style="width:120px;">
                        <span id="timerSoundRepeatsLabel">3×</span>
                    </div>
                </div>
            </div>

            ${this.createCheckboxRow('Show floating window', 'Automatically show the floating window if hidden when the timer completes.', 'timerShowWindow', true)}
        `;
    }

    render() { return this.renderContent(); }

    load(config) {
        const t = (config.extensions && config.extensions['timer']) || {};
        const set = (id, val) => { const el = document.getElementById(id); if (el) el.checked = val !== false; };
        set('timerNotify', t.notify_on_complete);
        set('timerSound', t.sound_on_complete);
        set('timerShowWindow', t.show_window_on_complete);
        const soundSelect = document.getElementById('timerSoundId');
        if (soundSelect) soundSelect.value = t.sound_id || 'two-tone';
        const customPath = document.getElementById('timerCustomSoundPath');
        if (customPath) customPath.value = t.custom_sound_path || '';
        const repeats = document.getElementById('timerSoundRepeats');
        if (repeats) {
            repeats.value = t.sound_repeats || 3;
            const label = document.getElementById('timerSoundRepeatsLabel');
            if (label) label.textContent = repeats.value + '×';
        }
        this._updateCustomPathVisibility();
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions['timer'] = {
            notify_on_complete: document.getElementById('timerNotify')?.checked ?? true,
            sound_on_complete: document.getElementById('timerSound')?.checked ?? true,
            sound_id: document.getElementById('timerSoundId')?.value || 'two-tone',
            custom_sound_path: document.getElementById('timerCustomSoundPath')?.value?.trim() || null,
            sound_repeats: parseInt(document.getElementById('timerSoundRepeats')?.value || '3'),
            show_window_on_complete: document.getElementById('timerShowWindow')?.checked ?? true,
        };
    }

    initialize() {
        const soundSelect = document.getElementById('timerSoundId');
        if (soundSelect) soundSelect.addEventListener('change', () => this._updateCustomPathVisibility());
        const previewBtn = document.getElementById('timerSoundPreview');
        if (previewBtn) previewBtn.addEventListener('click', () => this._previewSound());
        const repeatsSlider = document.getElementById('timerSoundRepeats');
        if (repeatsSlider) repeatsSlider.addEventListener('input', () => {
            const label = document.getElementById('timerSoundRepeatsLabel');
            if (label) label.textContent = repeatsSlider.value + '×';
        });
    }

    _updateCustomPathVisibility() {
        const soundSelect = document.getElementById('timerSoundId');
        const customRow = document.getElementById('timerCustomPathRow');
        if (soundSelect && customRow) customRow.style.display = soundSelect.value === 'custom' ? '' : 'none';
    }

    async _previewSound() {
        const btn = document.getElementById('timerSoundPreview');
        const soundId = document.getElementById('timerSoundId')?.value || 'two-tone';
        const customPath = document.getElementById('timerCustomSoundPath')?.value?.trim() || '';
        const repeats = parseInt(document.getElementById('timerSoundRepeats')?.value || '3');
        try {
            const { playTimerSound, stopTimerSound, isSoundPlaying } = await import('../../js/timer-sounds.js');
            if (isSoundPlaying()) { stopTimerSound(); if (btn) btn.textContent = '▶ Preview'; return; }
            if (btn) btn.textContent = '⏹ Stop';
            playTimerSound(soundId, customPath, repeats, () => { if (btn) btn.textContent = '▶ Preview'; });
        } catch (e) {
            console.error('Failed to preview sound:', e);
            if (btn) btn.textContent = '▶ Preview';
        }
    }
}
window.TimerExtSettingsModule = TimerExtSettingsModule;
