import { SettingsModule } from './base.js';
import { t } from '../shared/i18n.js';
/**
 * Timer & Stopwatch Settings Module
 */
export class TimerSettingsModule extends SettingsModule {
    constructor() {
        super('timer', t('settings.timer.title'), '⏱️');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                ${this.createCheckboxRow(
                    t('settings.timer.enable.label'),
                    t('settings.timer.enable.description'),
                    'timerEnabled',
                    true
                )}

                <div class="setting-section-label">${t('settings.timer.completion.section')}</div>

                ${this.createCheckboxRow(
                    t('settings.timer.notify.label'),
                    t('settings.timer.notify.description'),
                    'timerNotify',
                    true
                )}

                ${this.createCheckboxRow(
                    t('settings.timer.sound.label'),
                    t('settings.timer.sound.description'),
                    'timerSound',
                    true
                )}

                <div class="setting-row" id="timerSoundOptions">
                    <div class="setting-label">${t('settings.timer.sound_id.label')}</div>
                    <div class="setting-description">${t('settings.timer.sound_id.description')}</div>
                    <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                        <select class="setting-input" id="timerSoundId" style="max-width:200px;">
                            <option value="two-tone">${t('settings.timer.sound.two_tone')}</option>
                            <option value="chime">${t('settings.timer.sound.chime')}</option>
                            <option value="alert">${t('settings.timer.sound.alert')}</option>
                            <option value="gentle">${t('settings.timer.sound.gentle')}</option>
                            <option value="bell">${t('settings.timer.sound.bell')}</option>
                            <option value="success">${t('settings.timer.sound.success')}</option>
                            <option value="custom">${t('settings.timer.sound.custom')}</option>
                        </select>
                        <button class="setting-button" id="timerSoundPreview" title="${t('settings.timer.preview_btn.title')}">${t('settings.timer.preview_btn')}</button>
                    </div>
                    <div id="timerCustomPathRow" style="display:none;margin-top:8px;">
                        <div class="setting-control">
                            <input type="text" class="setting-input" id="timerCustomSoundPath" placeholder="${t('settings.timer.custom_path.placeholder')}" style="max-width:350px;">
                        </div>
                    </div>
                    <div style="margin-top:8px;">
                        <div class="setting-description">${t('settings.timer.repeats.description')}</div>
                        <div class="setting-control" style="display:flex;gap:8px;align-items:center;margin-top:4px;">
                            <input type="range" id="timerSoundRepeats" min="1" max="10" value="3" style="width:120px;">
                            <span id="timerSoundRepeatsLabel">3×</span>
                        </div>
                    </div>
                </div>

                ${this.createCheckboxRow(
                    t('settings.timer.show_window.label'),
                    t('settings.timer.show_window.description'),
                    'timerShowWindow',
                    true
                )}
            </div>
        `;
    }

    load(config) {
        const tcfg = config.timer || {};
        const set = (id, val) => {
            const el = document.getElementById(id);
            if (el) el.checked = val !== false;
        };
        set('timerEnabled', tcfg.enabled);
        set('timerNotify', tcfg.notify_on_complete);
        set('timerSound', tcfg.sound_on_complete);
        set('timerShowWindow', tcfg.show_window_on_complete);

        const soundSelect = document.getElementById('timerSoundId');
        if (soundSelect) soundSelect.value = tcfg.sound_id || 'two-tone';

        const customPath = document.getElementById('timerCustomSoundPath');
        if (customPath) customPath.value = tcfg.custom_sound_path || '';

        const repeats = document.getElementById('timerSoundRepeats');
        if (repeats) {
            repeats.value = tcfg.sound_repeats || 3;
            const label = document.getElementById('timerSoundRepeatsLabel');
            if (label) label.textContent = repeats.value + '×';
        }

        this._updateCustomPathVisibility();
    }

    save(config) {
        config.timer = {
            enabled: document.getElementById('timerEnabled')?.checked ?? true,
            notify_on_complete: document.getElementById('timerNotify')?.checked ?? true,
            sound_on_complete: document.getElementById('timerSound')?.checked ?? true,
            sound_id: document.getElementById('timerSoundId')?.value || 'two-tone',
            custom_sound_path:
                document.getElementById('timerCustomSoundPath')?.value?.trim() || null,
            sound_repeats: parseInt(document.getElementById('timerSoundRepeats')?.value || '3', 10),
            show_window_on_complete: document.getElementById('timerShowWindow')?.checked ?? true,
        };
    }

    initialize() {
        const soundSelect = document.getElementById('timerSoundId');
        if (soundSelect) {
            soundSelect.addEventListener('change', () => this._updateCustomPathVisibility());
        }

        const previewBtn = document.getElementById('timerSoundPreview');
        if (previewBtn) {
            previewBtn.addEventListener('click', () => this._previewSound());
        }

        const repeatsSlider = document.getElementById('timerSoundRepeats');
        if (repeatsSlider) {
            repeatsSlider.addEventListener('input', () => {
                const label = document.getElementById('timerSoundRepeatsLabel');
                if (label) label.textContent = repeatsSlider.value + '×';
            });
        }
    }

    _updateCustomPathVisibility() {
        const soundSelect = document.getElementById('timerSoundId');
        const customRow = document.getElementById('timerCustomPathRow');
        if (soundSelect && customRow) {
            customRow.style.display = soundSelect.value === 'custom' ? '' : 'none';
        }
    }

    async _previewSound() {
        const btn = document.getElementById('timerSoundPreview');
        const soundId = document.getElementById('timerSoundId')?.value || 'two-tone';
        const customPath = document.getElementById('timerCustomSoundPath')?.value?.trim() || '';
        const repeats = parseInt(document.getElementById('timerSoundRepeats')?.value || '3', 10);

        try {
            const { playTimerSound, stopTimerSound, isSoundPlaying } = await import(
                '../shared/timer-sounds.js'
            );

            if (isSoundPlaying()) {
                stopTimerSound();
                if (btn) {
                    btn.textContent = t('settings.timer.preview_btn');
                }
                return;
            }

            if (btn) {
                btn.textContent = t('settings.timer.preview_btn.stop');
            }
            playTimerSound(soundId, customPath, repeats, () => {
                if (btn) {
                    btn.textContent = t('settings.timer.preview_btn');
                }
            });
        } catch (e) {
            console.error('Failed to preview sound:', e);
            if (btn) {
                btn.textContent = t('settings.timer.preview_btn');
            }
        }
    }
}
