/**
 * Shared hotkey picker component.
 * Click to capture a hotkey via low-level keyboard hook, then immediately
 * tries to register it. Shows error if the combo is already taken.
 *
 * Usage:
 *   const picker = new HotkeyPicker(containerEl, invoke, { modifiers: ['Alt'], key: 'Space' });
 *   picker.onChange((hotkey) => { ... });
 */
import { t } from './i18n.js';

export class HotkeyPicker {
    constructor(container, invoke, initialHotkey, slot) {
        this.container = container;
        this.invoke = invoke;
        this.hotkey = initialHotkey || { modifiers: ['Alt'], key: 'Space' };
        this.slot = slot || 'main';
        this.capturing = false;
        this._callbacks = [];
        this.render();
        this.bind();
    }

    onChange(cb) {
        this._callbacks.push(cb);
    }
    _notify() {
        for (const cb of this._callbacks) cb(this.hotkey);
    }

    setHotkey(hk) {
        this.hotkey = hk;
        this.renderKeycaps();
        this.clearStatus();
    }

    render() {
        this.container.innerHTML = `
            <div class="hk-picker">
                <div class="hk-picker-display" title="${t('shared.hotkey_picker.click_title')}">
                    <span class="hk-picker-keycaps"></span>
                    <span class="hk-picker-edit-hint">${t('shared.hotkey_picker.click_hint')}</span>
                </div>
                <div class="hk-picker-status"></div>
            </div>
        `;
        this.displayEl = this.container.querySelector('.hk-picker-display');
        this.keycapsEl = this.container.querySelector('.hk-picker-keycaps');
        this.statusEl = this.container.querySelector('.hk-picker-status');
        this.renderKeycaps();
    }

    bind() {
        this.displayEl.addEventListener('click', () => this.startCapture());
    }

    renderKeycaps() {
        const parts = [...(this.hotkey.modifiers || []), this.hotkey.key];
        this.keycapsEl.innerHTML = parts
            .map((k, i) => {
                const sep = i < parts.length - 1 ? '<span class="hk-picker-sep">+</span>' : '';
                return `<kbd class="hk-picker-key">${k}</kbd>${sep}`;
            })
            .join('');
    }

    clearStatus() {
        this.statusEl.textContent = '';
        this.statusEl.className = 'hk-picker-status';
    }

    showStatus(msg, type) {
        this.statusEl.textContent = msg;
        this.statusEl.className = 'hk-picker-status hk-status-' + type;
    }

    async startCapture() {
        if (this.capturing) return;
        this.capturing = true;
        this.displayEl.classList.add('hk-capturing');
        this.keycapsEl.innerHTML = `<span class="hk-picker-waiting">${t('shared.hotkey_picker.waiting')}</span>`;
        this.clearStatus();

        try {
            const result = await this.invoke('capture_hotkey_combo');
            if (result?.key) {
                // Try to register immediately
                try {
                    await this.invoke('try_register_hotkey', {
                        modifiers: result.modifiers,
                        key: result.key,
                        slot: this.slot,
                    });
                    this.hotkey = { modifiers: result.modifiers, key: result.key };
                    this.showStatus(t('shared.hotkey_picker.registered'), 'success');
                    this._notify();
                } catch (e) {
                    this.showStatus('✗ ' + this.friendlyError(e), 'error');
                }
            }
        } catch (e) {
            console.error('Hotkey capture failed:', e);
        }

        this.renderKeycaps();
        this.displayEl.classList.remove('hk-capturing');
        this.capturing = false;
    }

    friendlyError(err) {
        const msg = String(err);
        if (msg.includes('already used as the main hotkey')) {
            return t('shared.hotkey_picker.error.main');
        }
        if (msg.includes('already used as the clipboard hotkey')) {
            return t('shared.hotkey_picker.error.clipboard');
        }
        if (msg.includes('already registered') || msg.includes('HotKey')) {
            return t('shared.hotkey_picker.error.in_use');
        }
        return t('shared.hotkey_picker.error.generic', { message: msg });
    }
}
