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
 *
 * Defaults come from the manifest. When the manifest's `config` block
 * declares a default for a control's id, the control shows the effective
 * value (the manifest default unless the user saved something else) and its
 * own `default` is ignored. Only controls for ids the manifest doesn't
 * declare (scratch fields, legacy extensions) use their own `default`. A
 * control with id `enabled` belongs to the host: it is never shown, holds
 * the host's on/off state, and is never saved. See extension-config.js.
 */

import { ENABLED_KEY, manifestConfigDefaults, mergeSettingsSave } from './extension-config.js';
import {
    copyTextError,
    forEachValueControl,
    isScratchId,
    isVisible,
    sanitizeInfoHtml,
    validateSchema,
} from './settings-schema.js';
import { escapeAttr, escapeHtml } from './tool-utils.js';
import { runSettingsHostSideEffect } from './settings-host-effects.js';
import { neutralizeLinks } from './link-handler.js';
import { t } from './i18n.js';

/** How long a copy action's "Copied" status stays up. */
const COPIED_STATUS_MS = 2000;

/**
 * `extensionId\0controlId` pairs already warned about a control default
 * that disagrees with the manifest, so a page that re-renders (refresh,
 * reload) warns once per control rather than on every render.
 */
const warnedDefaultMismatch = new Set();

/**
 * Timeout for a settings action RPC. Far longer than the default 10s
 * sandbox RPC cap because settings actions legitimately block on the
 * *user* — the longest is Spotify's OAuth flow, whose loopback listener
 * waits up to 300s for the browser redirect. 10 minutes clears that with
 * margin while still bounding a runaway (an action stuck in an infinite
 * loop or a promise that never resolves) so the button can't be wedged
 * disabled forever.
 */
const ACTION_RPC_TIMEOUT_MS = 10 * 60 * 1000;

/**
 * Rendered settings instance. Produced by renderSchema().
 * Callers interact with it via `load(config)`, `save(stored)`, and `validate()`
 * to plug into the existing SettingsModule interface.
 */
export class RenderedSettings {
    /**
     * @param {object} args
     * @param {string} args.extensionId
     * @param {object} args.schema
     * @param {HTMLElement} args.container - host div to render into
     * @param {object} args.sandbox - ExtensionSandbox instance (for action RPCs)
     * @param {object} [args.manifest] - the extension manifest; its `config`
     *   block supplies the defaults and the declared types used on save
     * @param {object} [args.logger] - optional { warn, error } logger
     */
    constructor({ extensionId, schema, container, sandbox, manifest, logger }) {
        this.extensionId = extensionId;
        this.manifest = manifest || null;
        /** Manifest-declared defaults, id → value. */
        this.defaults = manifestConfigDefaults(this.manifest);
        this.container = container;
        this.sandbox = sandbox;
        this.log = logger || console;
        this.schema = this._withManifestDefaults(schema);
        /** Current values map, id → value. */
        this.values = {};
        /** DOM nodes per control id, for updates. */
        this._rowsById = new Map();
        this._controlsById = new Map();
        /** Action control per button element id, for the click handler. */
        this._actionsByButtonId = new Map();
        /** Pending "Copied" status-clear timers. */
        this._statusTimers = new Set();
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

    /**
     * Re-fetch the schema from the sandbox and re-render in place. Used by
     * the `refresh` host effect so an action that changed derived state
     * (connection status, whether there's a session to sign out of, the
     * Connect vs Reconnect label) updates the whole panel, not just the
     * status line.
     *
     * Current value-control values are carried across so an in-progress
     * edit (e.g. a half-typed trigger word) isn't lost to the re-render.
     * A control only the refreshed schema has (the next step of a setup
     * wizard) shows the config load() was given, like every control did
     * on first render. Falling back to its default instead would show the
     * wrong value, and Save would then delete the user's override.
     *
     * @param {object} [opts]
     * @param {string} [opts.preserveStatusFor] - id of the action control whose status span
     *        should keep showing `opts.status` after the rebuild.
     * @param {string} [opts.status] - status text to re-apply.
     */
    async refresh(opts = {}) {
        let schema;
        try {
            schema = await this.sandbox.call('getSettings', {});
        } catch (e) {
            this.log.warn?.('settings refresh: getSettings failed:', e);
            return;
        }
        // Re-validate: the template interpolation relies on validateSchema's
        // id / min / max guarantees, and the extension is untrusted, so a
        // refreshed schema gets the same check as the first one. Validate
        // before destroy() so a bad schema leaves the current panel intact.
        const r = validateSchema(schema);
        if (!r.ok) {
            this.log.warn?.('settings refresh: invalid schema:', r.error);
            return;
        }

        // Snapshot live values (scratch fields included) so the rebuild
        // doesn't reset them to defaults.
        const carried = this.currentValues();

        // Tear down old listeners before we replace the DOM.
        this.destroy();

        this.schema = this._withManifestDefaults(r.schema);
        this.values = {};
        this.render();
        this._applyValues(this._refreshSource(carried), { fromConfig: false });

        // Re-apply the status message on the originating action's status
        // span — the rebuild created a fresh (empty) one.
        if (opts.preserveStatusFor && opts.status) {
            const btnId = this._inputId({ id: opts.preserveStatusFor });
            const statusEl = this.container.querySelector(`#${cssEscapeId(btnId)}__status`);
            if (statusEl) statusEl.textContent = opts.status;
        }
    }

    /**
     * Show `config`: the effective extension config (resolveExtensionConfig
     * in extension-config.js), i.e. manifest defaults overlaid with saved
     * values plus the host's `enabled`.
     */
    load(config) {
        // Kept for refresh(): a control that only a refreshed schema has
        // takes its value from here.
        this._loaded = config && typeof config === 'object' ? { ...config } : null;
        this._applyValues(config, { fromConfig: true });
    }

    /**
     * What refresh() applies: the loaded config (minus scratch values,
     * which never come from config), then the live values `carried` from
     * the old page, then the host's current on/off state.
     */
    _refreshSource(carried) {
        const source = {};
        for (const [key, value] of Object.entries(this._loaded || {})) {
            if (!isScratchId(key)) source[key] = value;
        }
        Object.assign(source, carried);
        if (this._loaded && Object.hasOwn(this._loaded, ENABLED_KEY)) {
            source[ENABLED_KEY] = this._loaded[ENABLED_KEY];
        }
        return source;
    }

    /**
     * Mirror the host's on/off state (the Settings header's Disable/Enable
     * button) into a hidden `enabled` control, so `showWhen` clauses that
     * reference it follow the header.
     */
    setHostEnabled(enabled) {
        // Also for a page without the control: a refresh may add one.
        this._loaded = { ...(this._loaded || {}), [ENABLED_KEY]: !!enabled };
        const ctrl = this._controlsById.get(ENABLED_KEY);
        if (!isValueControl(ctrl)) return;
        this.values[ENABLED_KEY] = coerceValue(ctrl, !!enabled);
        this._writeToControl(ctrl, this.values[ENABLED_KEY]);
        this._applyVisibility();
    }

    /**
     * Controls with a value in `source` adopt it; others take their default.
     * Scratch (`__`) fields never take a value from stored config — they
     * were never meant to be there (older hosts persisted them), and their
     * default is what the extension wants shown.
     */
    _applyValues(source, { fromConfig }) {
        forEachValueControl(this.schema, (ctrl) => {
            const usable =
                source && Object.hasOwn(source, ctrl.id) && !(fromConfig && isScratchId(ctrl.id));
            this.values[ctrl.id] = coerceValue(ctrl, usable ? source[ctrl.id] : ctrl.default);
            this._writeToControl(ctrl, this.values[ctrl.id]);
        });
        this._applyVisibility();
    }

    /**
     * Every value control's live value, scratch fields included. This is
     * what actions, validate() and normalize() see.
     */
    currentValues() {
        forEachValueControl(this.schema, (ctrl) => {
            this.values[ctrl.id] = this._readFromControl(ctrl);
        });
        return { ...this.values };
    }

    /**
     * The new config.extensions[id] after saving this page, given what is
     * stored now (mergeSettingsSave in extension-config.js). Only this
     * page's keys change. Values equal to the manifest default are left
     * out, and scratch (`__`) and `enabled` values are never stored (stored
     * ones are removed).
     * validate() has already written normalize()'s rewrites into the
     * controls. A key normalize() left out keeps its live value, because
     * older extensions return only the keys they changed.
     *
     * @param {object} [stored] - config.extensions[id] before the save
     */
    save(stored) {
        return mergeSettingsSave(this.manifest, stored, this.currentValues());
    }

    /**
     * Ask the sandbox to validate. If the extension doesn't implement
     * validateSettings, we fall back to schema-level min/max checks.
     */
    async validate() {
        let current = this.currentValues();

        // Optional normalization — extensions can return a canonicalized
        // values map (e.g. trimming whitespace, adding implicit trailing
        // delimiters). If the sandbox returns an object with a `values`
        // field, its keys are written back into the controls, which is
        // where the next save() reads them from.
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
                    forEachValueControl(this.schema, (ctrl) => {
                        if (!Object.hasOwn(current, ctrl.id)) return;
                        this.values[ctrl.id] = coerceValue(ctrl, current[ctrl.id]);
                        this._writeToControl(ctrl, this.values[ctrl.id]);
                    });
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
        this._actionsByButtonId.clear();
        for (const timer of this._statusTimers) clearTimeout(timer);
        this._statusTimers.clear();
    }

    // --- Rendering ---------------------------------------------------------

    /**
     * `schema` with each value control's `default` replaced by the manifest
     * default for its id, if the manifest declares one. Every fallback in
     * this file (an unsaved value, an emptied number field) reads
     * `ctrl.default`, so they all use the manifest default. Warns once per
     * control whose own default would show something different. That's
     * harmless here, but older hosts show the control's default. Returns a
     * copy and leaves the extension's object alone.
     */
    _withManifestDefaults(schema) {
        if (!schema || !Array.isArray(schema.sections)) return schema;
        const sections = schema.sections.map((section) => {
            if (!section || !Array.isArray(section.controls)) return section;
            const controls = section.controls.map((ctrl) => {
                if (!isValueControl(ctrl) || !Object.hasOwn(this.defaults, ctrl.id)) return ctrl;
                const manifestDefault = this.defaults[ctrl.id];
                this._warnDefaultMismatch(ctrl, manifestDefault);
                return { ...ctrl, default: manifestDefault };
            });
            return { ...section, controls };
        });
        return { ...schema, sections };
    }

    /**
     * A text control whose default differs only in leading or trailing
     * whitespace isn't flagged. normalize() often trims or re-adds a
     * delimiter before an older host saves the shown default (window-walker
     * shows "w" and stores "w "), and this check can't run normalize().
     * Kage-Extensions' `check:defaults`, which does, is the real gate.
     */
    _warnDefaultMismatch(ctrl, manifestDefault) {
        if (ctrl.default === undefined) return;
        const shown = coerceValue(ctrl, ctrl.default);
        const declared = coerceValue({ ...ctrl, default: manifestDefault }, manifestDefault);
        if (shown === declared) return;
        if (ctrl.type === 'text' && String(shown).trim() === String(declared).trim()) return;
        const key = `${this.extensionId}\0${ctrl.id}`;
        if (warnedDefaultMismatch.has(key)) return;
        warnedDefaultMismatch.add(key);
        this.log.warn?.(
            `[settings] extension '${this.extensionId}': control '${ctrl.id}' default ` +
                `${JSON.stringify(ctrl.default)} differs from the manifest config default ` +
                `${JSON.stringify(manifestDefault)}; using the manifest default`
        );
    }

    _renderSection(section) {
        const labelHtml = section.label
            ? `<div class="setting-section-label">${escapeHtml(section.label)}</div>`
            : '';
        const rows = (section.controls || []).map((c) => this._renderRow(c)).join('\n');
        return `${labelHtml}\n${rows}`;
    }

    _renderRow(ctrl) {
        // Ids/numbers are schema-validated; escaping is defence in depth
        // since this markup lands in the privileged settings window.
        const rowId = escapeAttr(this._rowId(ctrl));
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
        const inputId = escapeAttr(this._inputId(ctrl));
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
        const inputId = escapeAttr(this._inputId(ctrl));
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
        const inputId = escapeAttr(this._inputId(ctrl));
        const style = ` style="max-width:${+(ctrl.maxWidth || 80)}px;"`;
        const minAttr = typeof ctrl.min === 'number' ? ` min="${escapeAttr(ctrl.min)}"` : '';
        const maxAttr = typeof ctrl.max === 'number' ? ` max="${escapeAttr(ctrl.max)}"` : '';
        const stepAttr = typeof ctrl.step === 'number' ? ` step="${escapeAttr(ctrl.step)}"` : '';
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
        const inputId = escapeAttr(this._inputId(ctrl));
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
        const inputId = escapeAttr(this._inputId(ctrl));
        const labelId = `${inputId}__label`;
        const minAttr = ` min="${escapeAttr(ctrl.min)}"`;
        const maxAttr = ` max="${escapeAttr(ctrl.max)}"`;
        const stepAttr = typeof ctrl.step === 'number' ? ` step="${escapeAttr(ctrl.step)}"` : '';
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
        const btnId = escapeAttr(this._inputId(ctrl));
        const statusId = `${btnId}__status`;
        const variantClass =
            ctrl.variant === 'danger' ? ' danger' : ctrl.variant === 'primary' ? ' primary' : '';
        return `
            <div class="setting-row" id="${rowId}"${showWhenAttr}>
                ${ctrl.description ? `<div class="setting-description">${escapeHtml(ctrl.description)}</div>` : ''}
                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                    <button class="setting-button${variantClass}" id="${btnId}" data-ext-action
                        data-ext-action-name="${escapeAttr(ctrl.action || '')}"
                        ${ctrl.disabled ? 'disabled' : ''}
                        ${ctrl.confirm ? `data-ext-action-confirm="${escapeAttr(ctrl.confirm)}"` : ''}>
                        ${escapeHtml(ctrl.label)}
                    </button>
                    <span id="${statusId}" class="setting-description ext-action-status" aria-live="polite"></span>
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
                // Extension-authored links (Spotify's developer-dashboard
                // setup link) must open in the system browser, not navigate
                // the Settings window. The window-wide link handler opens
                // them from data-href; neutralizing leaves no live href for
                // the engine to follow if that handler ever isn't there.
                neutralizeLinks(slot);
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
        for (const section of this.schema.sections || []) {
            for (const ctrl of section.controls || []) {
                if (ctrl.type !== 'action') continue;
                const btn = this._inputEl(ctrl);
                if (btn) this._actionsByButtonId.set(btn.id, ctrl);
            }
        }
        const buttons = this.container.querySelectorAll('[data-ext-action]');
        buttons.forEach((btn) => {
            const onClick = (ev) => this._handleActionClick(btn, ev);
            btn.addEventListener('click', onClick);
            this._cleanup.push(() => btn.removeEventListener('click', onClick));
        });
    }

    _statusEl(btn) {
        return this.container.querySelector(`#${cssEscapeId(btn.id)}__status`);
    }

    _statusSetter(btn) {
        const statusEl = this._statusEl(btn);
        return (text) => {
            if (statusEl) statusEl.textContent = text || '';
        };
    }

    /**
     * Not `async` on purpose: a copy action has to reach
     * navigator.clipboard.writeText() synchronously inside the click.
     * Returns a promise either way, settled once the action is done.
     */
    _handleActionClick(btn, _ev) {
        const ctrl = this._actionsByButtonId.get(btn.id);
        if (ctrl?.copy) return this._handleCopyClick(btn, ctrl);
        return this._runAction(btn, ctrl);
    }

    /**
     * Copy actions run entirely in the host. The sandbox iframe can't write
     * the clipboard, and a round-trip through it would lose the click's
     * user activation, which the clipboard API needs — so the text is
     * resolved and written synchronously, before any await.
     */
    _handleCopyClick(btn, ctrl) {
        const statusEl = this._statusEl(btn);
        const setStatus = this._statusSetter(btn);
        const text = this._copyText(ctrl);
        if (text === '') {
            setStatus(t('settings.ext_settings.action.copy_empty'));
            return Promise.resolve();
        }
        let written;
        try {
            written =
                text === null
                    ? Promise.reject(new Error('copy text is not plain text'))
                    : navigator.clipboard.writeText(text);
        } catch (e) {
            // No clipboard API at all (navigator.clipboard undefined).
            written = Promise.reject(e);
        }
        return Promise.resolve(written).then(
            () => {
                const copied = t('settings.ext_settings.action.copied');
                setStatus(copied);
                const timer = setTimeout(() => {
                    this._statusTimers.delete(timer);
                    // Leave anything that replaced it (another click) alone.
                    if (statusEl?.textContent === copied) statusEl.textContent = '';
                }, COPIED_STATUS_MS);
                this._statusTimers.add(timer);
            },
            (e) => {
                this.log.warn?.(`copy action '${ctrl.id}' failed:`, e);
                setStatus(t('settings.ext_settings.action.copy_failed'));
            }
        );
    }

    /**
     * The text a copy action should write: '' when its source control is
     * empty, null when the text isn't allowed on the clipboard (a source
     * value can be anything the user typed, so it's checked here, not just
     * by the schema validator).
     */
    _copyText(ctrl) {
        let text;
        if (Object.hasOwn(ctrl.copy, 'value')) {
            text = ctrl.copy.value;
        } else {
            // The raw field text, not _readFromControl(): that turns an
            // empty number field into its default (or 0), which would copy
            // a value the user can't see.
            const source = this._controlsById.get(ctrl.copy.from);
            const raw = source ? this._inputEl(source)?.value : '';
            text = raw == null ? '' : String(raw);
        }
        if (text === '') return '';
        return copyTextError(text) ? null : text;
    }

    async _runAction(btn, ctrl) {
        const action = btn.getAttribute('data-ext-action-name');
        const confirmMsg = btn.getAttribute('data-ext-action-confirm');
        if (confirmMsg && !window.confirm(confirmMsg)) return;

        const setStatus = this._statusSetter(btn);

        const prevText = btn.textContent;
        btn.disabled = true;
        setStatus(t('settings.ext_settings.action.working'));

        try {
            // Action handlers run inside the sandbox. The current values
            // are handed along so the action can use up-to-date settings
            // without reading from DOM.
            //
            // Generous RPC timeout: settings actions routinely block on the
            // *user*, not on work — e.g. an OAuth "Connect" flow waits for the
            // user to consent in a browser tab, which reliably exceeds the
            // default 10s RPC cap. We give it ACTION_RPC_TIMEOUT_MS (10 min,
            // comfortably past the 300s OAuth loopback) rather than disabling
            // the timeout entirely, so a runaway action still can't wedge the
            // button disabled forever.
            // A click on the action: it may open a file dialog
            // (EXTENSION_FILE_TEXT §5.2).
            const result = await this.sandbox.call(
                'runSettingsAction',
                { action, values: this.currentValues() },
                { timeoutMs: ACTION_RPC_TIMEOUT_MS, userGesture: true }
            );

            if (result && typeof result === 'object') {
                if (result.status) setStatus(String(result.status));
                else setStatus('');

                if (result.host) {
                    // onFileSelected's return value needs to update the
                    // same button's status. We pass the status setter
                    // down so the effect can call it. `refresh` also needs
                    // the just-set status preserved across the re-render,
                    // keyed by the control id (not the RPC name — the two
                    // can differ).
                    await this._runHostSideEffect(result.host, {
                        setStatus,
                        status: result.status ? String(result.status) : '',
                        sourceAction: ctrl?.id ?? action,
                    });
                }
                if (result.error) setStatus(`❌ ${result.error}`);
            } else {
                setStatus('');
            }
        } catch (e) {
            this.log.warn?.(`action '${action}' failed:`, e);
            setStatus(`❌ ${e?.message || e}`);
        } finally {
            // The button may have been replaced by a `refresh` effect; only
            // restore it if it's still the live node.
            if (btn.isConnected) {
                btn.disabled = false;
                btn.textContent = prevText;
            }
        }
    }

    /**
     * Handle host-side side effects requested by an action's return value.
     * Supported effects:
     *   - download: trigger a file download without touching disk
     *   - pick_file: open a native file picker (via browser <input type=file>),
     *       read the file, then send its contents back via a follow-up RPC
     *   - play_timer_sound: preview a built-in timer sound on the host
     *   - refresh: re-fetch getSettings() and re-render the panel in place,
     *       so an action that changed connection/auth state (e.g. Spotify's
     *       "Check connection" discovering a revoked token) is reflected in
     *       the info text, button labels, and disabled states immediately —
     *       not just in the transient status line.
     *   - reload: call reload() on the ExtensionManager (rarely needed)
     */
    async _runHostSideEffect(host, ctx = {}) {
        return runSettingsHostSideEffect(this, host, ctx, pickFileContents);
    }

    // --- Visibility --------------------------------------------------------

    _applyVisibility() {
        for (const section of this.schema.sections || []) {
            for (const ctrl of section.controls || []) {
                const rowId = this._rowId(ctrl);
                const row = this.container.querySelector(`#${cssEscapeId(rowId)}`);
                if (!row) continue;
                // The host's own enable switch (the section header) replaces
                // an extension's `enabled` checkbox.
                const hostOwned = isValueControl(ctrl) && ctrl.id === ENABLED_KEY;
                const show = !hostOwned && isVisible(ctrl.showWhen, this.values);
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

// escapeHtml / escapeAttr come from tool-utils — single canonical
// source for HTML escaping across the codebase.

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

/** A control that carries a value (not an action button or info block). */
function isValueControl(ctrl) {
    return !!ctrl?.id && ctrl.type !== 'info' && ctrl.type !== 'action';
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
