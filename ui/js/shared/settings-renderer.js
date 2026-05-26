/**
 * Host-side renderer for extension settings schemas.
 *
 * Produces the HTML + event wiring for a validated SettingsSchema.
 * Extensions do not touch the DOM — all they do is return a schema
 * object and handle RPC calls for actions and validation. Everything
 * else (input binding, conditional visibility, range labels, action
 * buttons, confirm dialogs, file download / pick) is implemented here.
 *
 * Used by ui/js/settings/manager.js to replace the old
 * `SettingsModule` extension-authored class.
 */

import {
    forEachValueControl,
    isVisible,
    sanitizeInfoHtml,
    validateSchema,
} from './settings-schema.js';
import { formatBytes } from './tool-utils.js';

/**
 * Rendered settings instance. Produced by renderSchema().
 * Callers interact with it via `load(config)`, `save(config)`, and `validate()`
 * to plug into the existing SettingsModule interface.
 */
export class RenderedSettings {
    /**
     * @param {object} args
     * @param {string} args.extensionId
     * @param {object} args.schema
     * @param {HTMLElement} args.container - host div to render into
     * @param {object} args.sandbox - ExtensionSandbox instance (for action RPCs)
     * @param {object} [args.logger] - optional { warn, error } logger
     */
    constructor({ extensionId, schema, container, sandbox, logger }) {
        this.extensionId = extensionId;
        this.schema = schema;
        this.container = container;
        this.sandbox = sandbox;
        this.log = logger || console;
        /** Current values map, id → value. */
        this.values = {};
        /** DOM nodes per control id, for updates. */
        this._rowsById = new Map();
        this._controlsById = new Map();
    }

    render() {
        const pieces = [];
        if (this.schema.description) {
            pieces.push(
                `<p class="ext-settings-description">${escapeHtml(this.schema.description)}</p>`
            );
        }
        for (const section of this.schema.sections || []) {
            pieces.push(this._renderSection(section));
        }
        this.container.innerHTML = pieces.join('\n');

        // Materialize info blocks that need sanitized HTML injection,
        // since we wrote placeholders in the template step.
        this._injectInfoHtml();

        // Cache row references + wire up events.
        this._cacheRowsAndControls();
        this._wireEvents();
        this._applyVisibility();
    }

    load(stored) {
        // `stored` is the raw extension config object (config.extensions[id]).
        // Controls with a stored value adopt it; others keep their default.
        for (const section of this.schema.sections || []) {
            for (const ctrl of section.controls || []) {
                if (!ctrl.id) continue;
                if (ctrl.type === 'info' || ctrl.type === 'action') continue;
                const has = stored && Object.hasOwn(stored, ctrl.id);
                const v = has ? stored[ctrl.id] : ctrl.default;
                this.values[ctrl.id] = coerceValue(ctrl, v);
                this._writeToControl(ctrl, this.values[ctrl.id]);
            }
        }
        this._applyVisibility();
    }

    save() {
        // Read current control values back into `values` and return them.
        forEachValueControl(this.schema, (ctrl) => {
            this.values[ctrl.id] = this._readFromControl(ctrl);
        });
        return { ...this.values };
    }

    /**
     * Ask the sandbox to validate. If the extension doesn't implement
     * validateSettings, we fall back to schema-level min/max checks.
     */
    async validate() {
        let current = this.save();

        // Optional normalization — extensions can return a canonicalized
        // values map (e.g. trimming whitespace, adding implicit trailing
        // delimiters). If the sandbox returns an object with a `values`
        // field, we adopt it and surface through subsequent save().
        if (this.sandbox) {
            try {
                const normalized = await this.sandbox.call('normalizeSettings', {
                    values: current,
                });
                if (
                    normalized &&
                    typeof normalized === 'object' &&
                    normalized.values &&
                    typeof normalized.values === 'object'
                ) {
                    current = normalized.values;
                    // Write the normalized values back into the UI so the
                    // user sees what will actually be saved.
                    for (const section of this.schema.sections || []) {
                        for (const ctrl of section.controls || []) {
                            if (!ctrl.id) continue;
                            if (ctrl.type === 'info' || ctrl.type === 'action') continue;
                            if (Object.hasOwn(current, ctrl.id)) {
                                this.values[ctrl.id] = current[ctrl.id];
                                this._writeToControl(ctrl, current[ctrl.id]);
                            }
                        }
                    }
                }
            } catch {
                // No normalize() implemented — proceed with raw values.
            }
        }

        let extResult = null;
        if (this.sandbox) {
            try {
                extResult = await this.sandbox.call('validateSettings', { values: current });
            } catch (_e) {
                extResult = null;
            }
        }
        if (extResult && extResult.valid === false) {
            return { valid: false, error: extResult.error || 'Invalid settings' };
        }
        // Schema-level min/max for number/range.
        for (const section of this.schema.sections || []) {
            for (const ctrl of section.controls || []) {
                if (!ctrl.id) continue;
                if (ctrl.type !== 'number' && ctrl.type !== 'range') continue;
                const v = current[ctrl.id];
                if (typeof ctrl.min === 'number' && v < ctrl.min) {
                    return { valid: false, error: `${ctrl.label} must be at least ${ctrl.min}` };
                }
                if (typeof ctrl.max === 'number' && v > ctrl.max) {
                    return { valid: false, error: `${ctrl.label} must be at most ${ctrl.max}` };
                }
            }
        }
        return { valid: true };
    }

    destroy() {
        if (this._cleanup)
            this._cleanup.forEach((fn) => {
                try {
                    fn();
                } catch {}
            });
        this._cleanup = [];
        this._rowsById.clear();
        this._controlsById.clear();
    }

    // --- Rendering ---------------------------------------------------------

    _renderSection(section) {
        const labelHtml = section.label
            ? `<div class="setting-section-label">${escapeHtml(section.label)}</div>`
            : '';
        const rows = (section.controls || []).map((c) => this._renderRow(c)).join('\n');
        return `${labelHtml}\n${rows}`;
    }

    _renderRow(ctrl) {
        const rowId = this._rowId(ctrl);
        const showWhenAttr = ctrl.showWhen
            ? ` data-ext-showwhen='${escapeAttr(JSON.stringify(ctrl.showWhen))}'`
            : '';

        switch (ctrl.type) {
            case 'checkbox':
                return this._renderCheckboxRow(ctrl, rowId, showWhenAttr);
            case 'text':
                return this._renderTextRow(ctrl, rowId, showWhenAttr);
            case 'number':
                return this._renderNumberRow(ctrl, rowId, showWhenAttr);
            case 'select':
                return this._renderSelectRow(ctrl, rowId, showWhenAttr);
            case 'range':
                return this._renderRangeRow(ctrl, rowId, showWhenAttr);
            case 'action':
                return this._renderActionRow(ctrl, rowId, showWhenAttr);
            case 'info':
                return this._renderInfoRow(ctrl, rowId, showWhenAttr);
            default:
                return '';
        }
    }

    _renderCheckboxRow(ctrl, rowId, showWhenAttr) {
        const inputId = this._inputId(ctrl);
        return `
            <div class="setting-row setting-row-checkbox" id="${rowId}"${showWhenAttr}>
                <div class="setting-label-with-checkbox">
                    <label class="setting-label-inline">
                        <span>${escapeHtml(ctrl.label)}</span>
                        <input type="checkbox" id="${inputId}" data-ext-ctrl>
                    </label>
                    ${ctrl.description ? `<div class="setting-description">${escapeHtml(ctrl.description)}</div>` : ''}
                </div>
            </div>`;
    }

    _renderTextRow(ctrl, rowId, showWhenAttr) {
        const inputId = this._inputId(ctrl);
        const style = ctrl.maxWidth ? ` style="max-width:${+ctrl.maxWidth}px;"` : '';
        return `
            <div class="setting-row" id="${rowId}"${showWhenAttr}>
                <div class="setting-label">${escapeHtml(ctrl.label)}</div>
                ${ctrl.description ? `<div class="setting-description">${escapeHtml(ctrl.description)}</div>` : ''}
                <div class="setting-control">
                    <input type="text" class="setting-input" id="${inputId}" data-ext-ctrl
                        placeholder="${escapeAttr(ctrl.placeholder || '')}"${style}>
                </div>
            </div>`;
    }

    _renderNumberRow(ctrl, rowId, showWhenAttr) {
        const inputId = this._inputId(ctrl);
        const style = ` style="max-width:${+(ctrl.maxWidth || 80)}px;"`;
        const minAttr = typeof ctrl.min === 'number' ? ` min="${ctrl.min}"` : '';
        const maxAttr = typeof ctrl.max === 'number' ? ` max="${ctrl.max}"` : '';
        const stepAttr = typeof ctrl.step === 'number' ? ` step="${ctrl.step}"` : '';
        return `
            <div class="setting-row" id="${rowId}"${showWhenAttr}>
                <div class="setting-label">${escapeHtml(ctrl.label)}</div>
                ${ctrl.description ? `<div class="setting-description">${escapeHtml(ctrl.description)}</div>` : ''}
                <div class="setting-control">
                    <input type="number" class="setting-input" id="${inputId}" data-ext-ctrl${minAttr}${maxAttr}${stepAttr}${style}>
                </div>
            </div>`;
    }

    _renderSelectRow(ctrl, rowId, showWhenAttr) {
        const inputId = this._inputId(ctrl);
        const style = ` style="max-width:${+(ctrl.maxWidth || 200)}px;"`;
        const opts = (ctrl.options || [])
            .map((o) => `<option value="${escapeAttr(o.value)}">${escapeHtml(o.label)}</option>`)
            .join('');
        return `
            <div class="setting-row" id="${rowId}"${showWhenAttr}>
                <div class="setting-label">${escapeHtml(ctrl.label)}</div>
                ${ctrl.description ? `<div class="setting-description">${escapeHtml(ctrl.description)}</div>` : ''}
                <div class="setting-control">
                    <select class="setting-input" id="${inputId}" data-ext-ctrl${style}>${opts}</select>
                </div>
            </div>`;
    }

    _renderRangeRow(ctrl, rowId, showWhenAttr) {
        const inputId = this._inputId(ctrl);
        const labelId = `${inputId}__label`;
        const minAttr = ` min="${ctrl.min}"`;
        const maxAttr = ` max="${ctrl.max}"`;
        const stepAttr = typeof ctrl.step === 'number' ? ` step="${ctrl.step}"` : '';
        const unit = ctrl.unit ? escapeHtml(ctrl.unit) : '';
        return `
            <div class="setting-row" id="${rowId}"${showWhenAttr}>
                <div class="setting-label">${escapeHtml(ctrl.label)}</div>
                ${ctrl.description ? `<div class="setting-description">${escapeHtml(ctrl.description)}</div>` : ''}
                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                    <input type="range" id="${inputId}" data-ext-ctrl${minAttr}${maxAttr}${stepAttr} style="width:140px;">
                    <span id="${labelId}" class="setting-range-value"></span><span class="setting-range-unit">${unit}</span>
                </div>
            </div>`;
    }

    _renderActionRow(ctrl, rowId, showWhenAttr) {
        const btnId = this._inputId(ctrl);
        const statusId = `${btnId}__status`;
        const variantClass =
            ctrl.variant === 'danger' ? ' danger' : ctrl.variant === 'primary' ? ' primary' : '';
        return `
            <div class="setting-row" id="${rowId}"${showWhenAttr}>
                ${ctrl.description ? `<div class="setting-description">${escapeHtml(ctrl.description)}</div>` : ''}
                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                    <button class="setting-button${variantClass}" id="${btnId}" data-ext-action
                        data-ext-action-name="${escapeAttr(ctrl.action)}"
                        ${ctrl.confirm ? `data-ext-action-confirm="${escapeAttr(ctrl.confirm)}"` : ''}>
                        ${escapeHtml(ctrl.label)}
                    </button>
                    <span id="${statusId}" class="setting-description ext-action-status"></span>
                </div>
            </div>`;
    }

    _renderInfoRow(ctrl, rowId, showWhenAttr) {
        // We can't inject the sanitized HTML via a template string — we
        // need the DocumentFragment path. So we emit a placeholder and
        // fill it in during _injectInfoHtml().
        const slotId = `${rowId}__info`;
        return `
            <div class="setting-row setting-row-info" id="${rowId}"${showWhenAttr}>
                ${ctrl.label ? `<div class="setting-label">${escapeHtml(ctrl.label)}</div>` : ''}
                <div class="setting-info-body" id="${slotId}" data-ext-info></div>
            </div>`;
    }

    // --- Binding -----------------------------------------------------------

    _cacheRowsAndControls() {
        for (const section of this.schema.sections || []) {
            for (const ctrl of section.controls || []) {
                if (!ctrl.id && ctrl.type !== 'info') continue;
                const rowId = this._rowId(ctrl);
                const row = this.container.querySelector(`#${cssEscapeId(rowId)}`);
                if (row) this._rowsById.set(ctrl.id || rowId, row);
                if (ctrl.id) this._controlsById.set(ctrl.id, ctrl);
            }
        }
    }

    _injectInfoHtml() {
        for (const section of this.schema.sections || []) {
            for (const ctrl of section.controls || []) {
                if (ctrl.type !== 'info') continue;
                const rowId = this._rowId(ctrl);
                const slot = this.container.querySelector(`#${cssEscapeId(rowId)}__info`);
                if (!slot) continue;
                const frag = sanitizeInfoHtml(ctrl.html || '');
                slot.appendChild(frag);
            }
        }
    }

    _wireEvents() {
        this._cleanup = [];

        // Value-bearing controls: updating values on input, re-evaluating visibility.
        forEachValueControl(this.schema, (ctrl) => {
            const el = this._inputEl(ctrl);
            if (!el) return;
            const handler = () => {
                this.values[ctrl.id] = this._readFromControl(ctrl);
                if (ctrl.type === 'range') this._updateRangeLabel(ctrl);
                this._applyVisibility();
            };
            const eventName =
                ctrl.type === 'text' || ctrl.type === 'number' || ctrl.type === 'range'
                    ? 'input'
                    : 'change';
            el.addEventListener(eventName, handler);
            this._cleanup.push(() => el.removeEventListener(eventName, handler));

            if (ctrl.type === 'range') this._updateRangeLabel(ctrl);
        });

        // Action buttons.
        const buttons = this.container.querySelectorAll('[data-ext-action]');
        buttons.forEach((btn) => {
            const onClick = (ev) => this._handleActionClick(btn, ev);
            btn.addEventListener('click', onClick);
            this._cleanup.push(() => btn.removeEventListener('click', onClick));
        });
    }

    async _handleActionClick(btn, _ev) {
        const action = btn.getAttribute('data-ext-action-name');
        const confirmMsg = btn.getAttribute('data-ext-action-confirm');
        if (confirmMsg && !window.confirm(confirmMsg)) return;

        const statusEl = this.container.querySelector(`#${cssEscapeId(btn.id)}__status`);
        const setStatus = (text) => {
            if (statusEl) statusEl.textContent = text || '';
        };

        const prevText = btn.textContent;
        btn.disabled = true;
        setStatus('Working…');

        try {
            // Action handlers run inside the sandbox. The current values
            // are handed along so the action can use up-to-date settings
            // without reading from DOM.
            const result = await this.sandbox.call('runSettingsAction', {
                action,
                values: this.save(),
            });

            if (result && typeof result === 'object') {
                if (result.status) setStatus(String(result.status));
                else setStatus('');

                if (result.host) {
                    // onFileSelected's return value needs to update the
                    // same button's status. We pass the status setter
                    // down so the effect can call it.
                    await this._runHostSideEffect(result.host, { setStatus });
                }
                if (result.error) setStatus(`❌ ${result.error}`);
            } else {
                setStatus('');
            }
        } catch (e) {
            this.log.warn?.(`action '${action}' failed:`, e);
            setStatus(`❌ ${e?.message || e}`);
        } finally {
            btn.disabled = false;
            btn.textContent = prevText;
        }
    }

    /**
     * Handle host-side side effects requested by an action's return value.
     * Supported effects:
     *   - download: trigger a file download without touching disk
     *   - pick_file: open a native file picker (via browser <input type=file>),
     *       read the file, then send its contents back via a follow-up RPC
     *   - play_timer_sound: preview a built-in timer sound on the host
     *   - reload: call reload() on the ExtensionManager (rarely needed)
     */
    async _runHostSideEffect(host, ctx = {}) {
        if (!host || typeof host !== 'object') return;
        const setStatus = ctx.setStatus || (() => {});
        switch (host.type) {
            case 'download': {
                const filename = String(host.filename || 'export.txt');
                const content = String(host.content || '');
                const mime = String(host.mime || 'application/octet-stream');
                const blob = new Blob([content], { type: mime });
                const url = URL.createObjectURL(blob);
                const a = document.createElement('a');
                a.href = url;
                a.download = filename;
                document.body.appendChild(a);
                a.click();
                a.remove();
                setTimeout(() => URL.revokeObjectURL(url), 1000);
                break;
            }
            case 'pick_file': {
                const accept = typeof host.accept === 'string' ? host.accept : '';
                const content = await pickFileContents(accept);
                if (content === null) return; // user cancelled
                try {
                    const result = await this.sandbox.call('onFileSelected', {
                        action: host.action || null,
                        filename: content.filename,
                        content: content.text,
                        values: this.save(),
                    });
                    if (result && typeof result === 'object' && result.status) {
                        setStatus(String(result.status));
                    }
                } catch (e) {
                    this.log.warn?.('onFileSelected RPC failed:', e);
                    setStatus(`❌ ${e?.message || e}`);
                }
                break;
            }
            case 'link_metadata': {
                // Host-side bridge for the Link Preview extension's
                // cache management. Extensions can't call settings-only
                // Tauri commands directly (and we don't want to widen
                // the extension capability surface for what is, in the
                // end, a tiny shared cache). Instead the extension's
                // `runSettingsAction` returns a `host` effect with
                // op = 'clear' or 'stats' and we run it here. Status
                // text shown on the action row is whatever the host
                // command returns.
                const op = String(host.op || '');
                const invoke = window?.__TAURI__?.core?.invoke;
                if (!invoke) {
                    setStatus('❌ host invoke unavailable');
                    return;
                }
                try {
                    if (op === 'clear') {
                        await invoke('link_metadata_clear_cache');
                        setStatus('✓ Cache cleared.');
                    } else if (op === 'stats') {
                        const stats = await invoke('link_metadata_cache_stats');
                        const entries = stats?.entries ?? 0;
                        const bytes = stats?.bytes ?? 0;
                        setStatus(`${entries} URLs · ${formatBytes(bytes) || '0 B'}`);
                    } else {
                        setStatus(`❌ unknown link_metadata op: ${op}`);
                    }
                } catch (e) {
                    setStatus(`❌ ${e?.message || e}`);
                }
                break;
            }
            case 'play_timer_sound': {
                // Preview a built-in or custom timer sound. Audio playback
                // is a host capability because the extension sandbox
                // doesn't share an AudioContext with the parent, and our
                // timer-sounds module lives in the main-window bundle.
                try {
                    const { playTimerSound, stopTimerSound, isSoundPlaying } = await import(
                        './timer-sounds.js'
                    );
                    if (isSoundPlaying()) {
                        stopTimerSound();
                        return;
                    }
                    const soundId = String(host.soundId || 'two-tone');
                    const customPath = host.customPath ? String(host.customPath) : '';
                    const repeats = Number(host.repeats) > 0 ? Number(host.repeats) : 1;
                    playTimerSound(soundId, customPath, repeats, () => {});
                } catch (e) {
                    this.log.warn?.('play_timer_sound failed:', e);
                }
                break;
            }
            default:
                // unknown effect — ignore (forwards-compatible)
                break;
        }
    }

    // --- Visibility --------------------------------------------------------

    _applyVisibility() {
        for (const section of this.schema.sections || []) {
            for (const ctrl of section.controls || []) {
                const rowId = this._rowId(ctrl);
                const row = this.container.querySelector(`#${cssEscapeId(rowId)}`);
                if (!row) continue;
                const show = isVisible(ctrl.showWhen, this.values);
                row.style.display = show ? '' : 'none';
            }
        }
    }

    _updateRangeLabel(ctrl) {
        const labelEl = this.container.querySelector(`#${cssEscapeId(this._inputId(ctrl))}__label`);
        if (labelEl) labelEl.textContent = String(this.values[ctrl.id] ?? '');
    }

    // --- Control I/O ------------------------------------------------------

    _inputEl(ctrl) {
        return this.container.querySelector(`#${cssEscapeId(this._inputId(ctrl))}`);
    }

    _writeToControl(ctrl, value) {
        const el = this._inputEl(ctrl);
        if (!el) return;
        switch (ctrl.type) {
            case 'checkbox':
                el.checked = !!value;
                break;
            case 'range':
                el.value = String(value);
                this._updateRangeLabel(ctrl);
                break;
            default:
                el.value = value == null ? '' : String(value);
                break;
        }
    }

    _readFromControl(ctrl) {
        const el = this._inputEl(ctrl);
        if (!el) return ctrl.default;
        switch (ctrl.type) {
            case 'checkbox':
                return !!el.checked;
            case 'number':
            case 'range': {
                const raw = el.value;
                if (raw === '' || raw == null) return ctrl.default ?? 0;
                const n = Number(raw);
                return Number.isFinite(n) ? n : (ctrl.default ?? 0);
            }
            default:
                return el.value;
        }
    }

    _rowId(ctrl) {
        // Info blocks get auto-generated row ids.
        const suffix = ctrl.id || `info-${this._autoInfoCounter(ctrl)}`;
        return `ext-row-${this.extensionId}-${suffix}`;
    }

    _inputId(ctrl) {
        return `ext-ctrl-${this.extensionId}-${ctrl.id}`;
    }

    _autoInfoCounter(ctrl) {
        // Deterministic: we index info blocks by position in the schema.
        // No need to be fancy — only used for DOM ids.
        if (!this._infoIndex) this._infoIndex = new WeakMap();
        if (this._infoIndex.has(ctrl)) return this._infoIndex.get(ctrl);
        const idx = (this._nextInfoIdx = (this._nextInfoIdx || 0) + 1);
        this._infoIndex.set(ctrl, idx);
        return idx;
    }
}

// --- Helpers ---------------------------------------------------------------

function escapeHtml(s) {
    return String(s).replace(
        /[&<>"']/g,
        (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]
    );
}

function escapeAttr(s) {
    return escapeHtml(s);
}

/**
 * Escape an id for use inside a CSS id selector. jsdom (and some older
 * browsers) don't have CSS.escape, so we provide a narrow fallback that
 * handles the character set our schema-validated ids can actually contain.
 * Ids are constrained to `[A-Za-z_][A-Za-z0-9_]*`, but our generated
 * rowIds/inputIds embed extension ids (which may contain hyphens) with a
 * fixed prefix, so we only need to escape a conservative set.
 */
function cssEscapeId(id) {
    if (typeof CSS !== 'undefined' && typeof CSS.escape === 'function') {
        return CSS.escape(id);
    }
    // Minimal fallback: backslash-escape anything not alphanumeric, _ or -.
    return String(id).replace(/[^a-zA-Z0-9_-]/g, (c) => '\\' + c);
}

function coerceValue(ctrl, v) {
    switch (ctrl.type) {
        case 'checkbox':
            return !!v;
        case 'number':
        case 'range': {
            if (v == null || v === '') return ctrl.default ?? ctrl.min ?? 0;
            const n = Number(v);
            return Number.isFinite(n) ? n : (ctrl.default ?? ctrl.min ?? 0);
        }
        default:
            return v == null ? (ctrl.default ?? '') : String(v);
    }
}

function pickFileContents(accept) {
    return new Promise((resolve) => {
        const input = document.createElement('input');
        input.type = 'file';
        if (accept) input.accept = accept;
        // Firefox and some Chromium builds need the input to be in the DOM
        // to trigger dialog on .click() from a non-user event path.
        input.style.display = 'none';
        document.body.appendChild(input);

        let settled = false;
        const done = (value) => {
            if (settled) return;
            settled = true;
            input.remove();
            resolve(value);
        };

        input.addEventListener('change', async () => {
            const file = input.files?.[0];
            if (!file) return done(null);
            try {
                const text = await file.text();
                done({ filename: file.name, text });
            } catch {
                done(null);
            }
        });
        // If the user cancels the picker we never hear a 'change' — detect
        // it via the 'focus' of the window returning without a file.
        const onFocus = () => {
            // Defer so the change event (if any) has a chance to fire first.
            setTimeout(() => {
                if (!input.files || input.files.length === 0) done(null);
            }, 300);
            window.removeEventListener('focus', onFocus);
        };
        window.addEventListener('focus', onFocus);

        input.click();
    });
}

/**
 * Build and return a RenderedSettings after validating the schema.
 * Throws on schema errors — callers should catch and fall back to a
 * "broken extension" message.
 */
export function renderSchema(args) {
    const r = validateSchema(args.schema);
    if (!r.ok) throw new Error(r.error);
    const inst = new RenderedSettings({ ...args, schema: r.schema });
    inst.render();
    return inst;
}
