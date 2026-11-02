/**
 * Shared hotkey picker component.
 * Click to capture a hotkey, then immediately tries to register it. Shows
 * an error if the combo is already taken.
 *
 * Capture path per platform:
 *   - Windows: `capture_hotkey_combo` (low-level keyboard hook in a helper
 *     process — WebView2 would swallow Win-key combos).
 *   - macOS: DOM keydown in this (focused) webview, bracketed by
 *     `begin_hotkey_recording` / `end_hotkey_recording`. Needs no TCC
 *     permission, so recording can never trigger an "approve Kage in
 *     System Settings" prompt. If the combo is bound to a macOS system
 *     shortcut (⌘Space → Spotlight) the picker asks before letting Kage
 *     take it over while it runs.
 *
 * Usage:
 *   const picker = new HotkeyPicker(containerEl, invoke, { modifiers: ['Alt'], key: 'Space' });
 *   picker.onChange((hotkey) => { ... });
 */
import { errMessage } from './error-message.js';
import { t } from './i18n.js';
import { isMac } from './shortcuts.js';
import { classifyKeyEvent, formatCombo } from './hotkey-capture.js';

/** Matches the native capture paths' 10 s window. */
const RECORD_TIMEOUT_MS = 10000;

/**
 * Cancel function of the recording in progress on this page, if any. Settings
 * shows several pickers in one webview; each recording installs a
 * capture-phase keydown listener that swallows the key, so two at once would
 * let the first picker steal (and save) the combo meant for the second.
 * Starting a recording cancels the previous one.
 */
let activeRecordingCancel = null;

/** LTR-isolate a combo so ⌘Space doesn't render as "Space⌘" inside RTL text. */
const ltrIsolate = (s) => `\u2066${s}\u2069`;

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

        let result = null;
        try {
            result = isMac()
                ? await this._recordInWebview()
                : await this.invoke('capture_hotkey_combo');
        } catch (e) {
            console.error('Hotkey capture failed:', e);
        }

        this.renderKeycaps();
        this.displayEl.classList.remove('hk-capturing');
        this.capturing = false;

        if (result?.key) {
            await this.applyCombo({ modifiers: result.modifiers || [], key: result.key });
        }
    }

    /**
     * Try to adopt `hk` as this slot's hotkey: check for a macOS system
     * shortcut on the same combo (asking before taking it over), then
     * register. Also used by preset buttons ("Use ⌘Space").
     */
    async applyCombo(hk) {
        if (isMac()) {
            let conflicts = [];
            try {
                conflicts =
                    (await this.invoke('check_system_shortcut_conflicts', {
                        modifiers: hk.modifiers,
                        key: hk.key,
                    })) || [];
            } catch (e) {
                console.warn('[hotkey-picker] system shortcut check failed:', e);
            }
            const pending = conflicts.filter((c) => !c.approved);
            if (pending.length) {
                this._showSystemConflict(hk, pending);
                return;
            }
        }
        await this._register(hk);
    }

    async _register(hk) {
        try {
            await this.invoke('try_register_hotkey', {
                modifiers: hk.modifiers,
                key: hk.key,
                slot: this.slot,
            });
            this.hotkey = { modifiers: hk.modifiers, key: hk.key };
            this.renderKeycaps();
            this.showStatus(t('shared.hotkey_picker.registered'), 'success');
            this._notify();
        } catch (e) {
            this.showStatus('✗ ' + this.friendlyError(e), 'error');
        }
    }

    /**
     * "⌘Space is used by macOS for Spotlight search" + [Use for Kage] [Keep].
     * Built with DOM APIs — names come from the backend catalog but are
     * still treated as text.
     */
    _showSystemConflict(hk, conflicts) {
        const names = [...new Set(conflicts.map((c) => c.name))].join(', ');
        this.statusEl.className = 'hk-picker-status hk-status-warning';
        this.statusEl.replaceChildren();

        const msg = document.createElement('div');
        msg.className = 'hk-picker-conflict-msg';
        msg.textContent = t('shared.hotkey_picker.system_conflict', {
            combo: ltrIsolate(formatCombo(hk, { mac: true })),
            name: names,
        });
        const hint = document.createElement('div');
        hint.className = 'hk-picker-conflict-hint';
        hint.textContent = t('shared.hotkey_picker.system_conflict_hint');

        const actions = document.createElement('div');
        actions.className = 'hk-picker-conflict-actions';
        const replace = document.createElement('button');
        replace.type = 'button';
        replace.className = 'setting-btn-primary hk-picker-conflict-replace';
        replace.textContent = t('shared.hotkey_picker.system_conflict_replace');
        const keep = document.createElement('button');
        keep.type = 'button';
        keep.className = 'setting-btn-secondary hk-picker-conflict-keep';
        keep.textContent = t('shared.hotkey_picker.system_conflict_keep');

        replace.addEventListener('click', async () => {
            replace.disabled = true;
            keep.disabled = true;
            try {
                await this.invoke('allow_system_shortcut_override', {
                    ids: conflicts.map((c) => c.id),
                });
            } catch (e) {
                this.showStatus('✗ ' + this.friendlyError(e), 'error');
                return;
            }
            await this._register(hk);
        });
        keep.addEventListener('click', () => this.clearStatus());

        actions.append(replace, keep);
        this.statusEl.append(msg, hint, actions);
    }

    /**
     * Record one combo from keydown events in this webview. Resolves to
     * `{ modifiers, key }`, or `null` on Escape / timeout / window blur.
     * Always pairs `begin_hotkey_recording` with `end_hotkey_recording`
     * (the backend also self-heals after 15 s if this page goes away).
     */
    _recordInWebview() {
        activeRecordingCancel?.();
        return new Promise((resolve) => {
            let done = false;
            // Resolves to the backend's generation token (or null if begin
            // failed); handed back to `end` so a late end from this
            // recording can't tear down a newer one.
            const begun = Promise.resolve()
                .then(() => this.invoke('begin_hotkey_recording'))
                .catch((e) => {
                    console.warn('[hotkey-picker] begin_hotkey_recording failed:', e);
                    return null;
                });

            const finish = async (value) => {
                if (done) return;
                done = true;
                if (activeRecordingCancel === cancel) activeRecordingCancel = null;
                window.removeEventListener('keydown', onKey, true);
                window.removeEventListener('blur', onBlur);
                clearTimeout(timer);
                // Serialise with begin so an instant cancel can't leave
                // system shortcuts paused.
                const generation = await begun;
                try {
                    await this.invoke('end_hotkey_recording', {
                        generation: typeof generation === 'number' ? generation : null,
                    });
                } catch (e) {
                    console.warn('[hotkey-picker] end_hotkey_recording failed:', e);
                }
                resolve(value);
            };
            const cancel = () => finish(null);
            const onKey = (e) => {
                const r = classifyKeyEvent(e);
                if (r.kind === 'ignore') return;
                // Keep the combo from also acting on this window (⌘W hide,
                // ⌘Q → "Hide Kage", Esc closing a dialog, …).
                e.preventDefault();
                e.stopImmediatePropagation();
                if (r.kind === 'cancel') {
                    finish(null);
                } else if (r.kind === 'unsupported') {
                    this.showStatus(t('shared.hotkey_picker.unsupported_key'), 'error');
                } else {
                    finish({ modifiers: r.modifiers, key: r.key });
                }
            };
            const onBlur = () => finish(null);
            const timer = setTimeout(() => finish(null), RECORD_TIMEOUT_MS);

            activeRecordingCancel = cancel;
            window.addEventListener('keydown', onKey, true);
            window.addEventListener('blur', onBlur);
        });
    }

    friendlyError(err) {
        // AppError rejections are `{ kind, message }`; String() gives "[object Object]".
        const msg = errMessage(err);
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
