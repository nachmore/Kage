/**
 * Shared hotkey picker component.
 * Click to capture a hotkey via low-level keyboard hook, then immediately
 * tries to register it. Shows error if the combo is already taken.
 *
 * Usage:
 *   const picker = new HotkeyPicker(containerEl, invoke, { modifiers: ['Alt'], key: 'Space' });
 *   picker.onChange((hotkey) => { ... });
 */
export class HotkeyPicker {
    constructor(container, invoke, initialHotkey) {
        this.container = container;
        this.invoke = invoke;
        this.hotkey = initialHotkey || { modifiers: ['Alt'], key: 'Space' };
        this.capturing = false;
        this._callbacks = [];
        this.render();
        this.bind();
    }

    onChange(cb) { this._callbacks.push(cb); }
    _notify() { for (const cb of this._callbacks) cb(this.hotkey); }

    setHotkey(hk) {
        this.hotkey = hk;
        this.renderKeycaps();
        this.clearStatus();
    }

    render() {
        this.container.innerHTML = `
            <div class="hk-picker">
                <div class="hk-picker-display" title="Click to change hotkey">
                    <span class="hk-picker-keycaps"></span>
                    <span class="hk-picker-edit-hint">click to change</span>
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
        this.keycapsEl.innerHTML = parts.map((k, i) => {
            const sep = i < parts.length - 1 ? '<span class="hk-picker-sep">+</span>' : '';
            return `<kbd class="hk-picker-key">${k}</kbd>${sep}`;
        }).join('');
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
        this.keycapsEl.innerHTML = '<span class="hk-picker-waiting">Press any key combo...</span>';
        this.clearStatus();

        try {
            const result = await this.invoke('capture_hotkey_combo');
            if (result && result.key) {
                // Try to register immediately
                try {
                    await this.invoke('try_register_hotkey', {
                        modifiers: result.modifiers,
                        key: result.key
                    });
                    this.hotkey = { modifiers: result.modifiers, key: result.key };
                    this.showStatus('✓ Hotkey registered', 'success');
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
        if (msg.includes('already registered') || msg.includes('HotKey')) {
            return 'This shortcut is already in use by another application';
        }
        return 'Failed to register: ' + msg;
    }
}
