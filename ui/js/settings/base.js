/**
 * Base class for settings modules
 * Each settings module should extend this class
 */
export class SettingsModule {
    constructor(id, title, icon) {
        this.id = id;
        this.title = title;
        this.icon = icon;
    }

    /**
     * Render the settings section HTML
     * @returns {string} HTML string for the settings section
     */
    render() {
        throw new Error('render() must be implemented by subclass');
    }

    /**
     * Create a checkbox setting row (title, then checkbox + description inline)
     */
    createCheckboxRow(label, description, checkboxId, checked) {
        const checkedAttr = checked ? ' checked' : '';
        return `
            <div class="setting-row">
                <div class="setting-label">${label}</div>
                <div class="setting-checkbox-row">
                    <label class="kage-checkbox">
                        <input type="checkbox" id="${checkboxId}"${checkedAttr}>
                    </label>
                    ${description ? `<div class="setting-description">${description}</div>` : ''}
                </div>
            </div>
        `;
    }

    /**
     * Create a control setting row (title, description, then control below)
     */
    createControlRow(label, description, controlHtml) {
        return `
            <div class="setting-row">
                <div class="setting-label">${label}</div>
                ${description ? `<div class="setting-description">${description}</div>` : ''}
                <div class="setting-control">
                    ${controlHtml}
                </div>
            </div>
        `;
    }

    /**
     * Create a control row with an action button beside the input
     */
    createControlWithActionRow(label, description, controlHtml, actionHtml) {
        return `
            <div class="setting-row">
                <div class="setting-label">${label}</div>
                ${description ? `<div class="setting-description">${description}</div>` : ''}
                <div class="setting-control-with-action">
                    ${controlHtml}
                    ${actionHtml}
                </div>
            </div>
        `;
    }

    /**
     * Legacy helper — kept for backward compat
     */
    createSettingRow(label, description, control) {
        return `
            <div class="setting-row">
                <div class="setting-label">${label}</div>
                ${description ? `<div class="setting-description">${description}</div>` : ''}
                <div class="setting-control">
                    ${control}
                </div>
            </div>
        `;
    }

    /**
     * Load settings from config object
     * @param {Object} config - The configuration object
     */
    load(_config) {
        throw new Error('load() must be implemented by subclass');
    }

    /**
     * Save settings to config object
     * @param {Object} config - The configuration object to update
     */
    save(_config) {
        throw new Error('save() must be implemented by subclass');
    }

    /**
     * Validate settings before saving
     * @returns {Object} { valid: boolean, error?: string }
     */
    validate() {
        return { valid: true };
    }

    /**
     * Initialize event listeners after rendering
     */
    initialize() {
        // Optional: Override if needed
    }

    /**
     * Cleanup when module is destroyed
     */
    destroy() {
        // Optional: Override if needed
    }

    // -----------------------------------------------------------------
    // Field-binding DSL
    // -----------------------------------------------------------------
    //
    // Most settings modules end up writing the same trio for every
    // field: render the input, populate it from config in load(), and
    // read it back into config in save(). The plumbing is mechanical
    // and identical across modules — every "checkbox + path-into-
    // config" pair is `el.checked ?? default` to load and the
    // assignment to a nested config path to save.
    //
    // `bindFields([...])` records the contract once; `loadFields(cfg)`
    // and `saveFields(cfg)` walk it. A module that's pure
    // bind-load-save can collapse from 30+ lines of getElementById
    // ceremony to a single bindFields call and two one-line load/save
    // overrides. Custom behaviour stays in the load() / save()
    // overrides, which still call super-equivalent helpers when they
    // want.
    //
    // Each spec entry:
    //   {
    //     id:    DOM id of the input element
    //     path:  dotted path into config, e.g. 'ui.font_size'
    //     kind:  'checkbox' | 'value' | 'int' | 'float'
    //     default: value to use when the config key is missing/undefined
    //   }
    //
    // `kind` semantics:
    //   - 'checkbox' — el.checked  ↔  config[path] (Boolean)
    //   - 'value'    — el.value    ↔  config[path] (String, or null if empty)
    //   - 'int'      — parseInt(el.value, 10)
    //   - 'float'    — parseFloat(el.value)
    //
    // Modules that need extra coercion (custom "default true vs
    // default false" semantics, post-save side effects, etc.) keep
    // those special cases in their hand-rolled load/save and use
    // bindFields for the boilerplate-only fields.

    /**
     * Record the field bindings. Call from the constructor once.
     * @param {Array<object>} specs
     */
    bindFields(specs) {
        this._fieldBindings = specs;
    }

    /**
     * Apply config → DOM for every binding registered via bindFields.
     */
    loadFields(config) {
        if (!this._fieldBindings) return;
        for (const spec of this._fieldBindings) {
            const el = document.getElementById(spec.id);
            if (!el) continue;
            const value = _readPath(config, spec.path);
            const resolved = value === undefined ? spec.default : value;
            switch (spec.kind) {
                case 'checkbox':
                    el.checked = !!resolved;
                    break;
                case 'value':
                case 'int':
                case 'float':
                    el.value = resolved == null ? '' : String(resolved);
                    break;
                default:
                    console.warn(
                        `[settings:${this.id}] unknown bind kind '${spec.kind}' for '${spec.id}'`
                    );
            }
        }
    }

    /**
     * Apply DOM → config for every binding registered via bindFields.
     */
    saveFields(config) {
        if (!this._fieldBindings) return;
        for (const spec of this._fieldBindings) {
            const el = document.getElementById(spec.id);
            // Missing element: leave the existing config value alone.
            // Pre-bind the section may render conditionally and a field
            // could legitimately not exist on this paint.
            if (!el) continue;
            let parsed;
            switch (spec.kind) {
                case 'checkbox':
                    parsed = !!el.checked;
                    break;
                case 'value':
                    parsed = el.value;
                    break;
                case 'int': {
                    const n = parseInt(el.value, 10);
                    parsed = Number.isFinite(n) ? n : spec.default;
                    break;
                }
                case 'float': {
                    const n = parseFloat(el.value);
                    parsed = Number.isFinite(n) ? n : spec.default;
                    break;
                }
                default:
                    console.warn(
                        `[settings:${this.id}] unknown bind kind '${spec.kind}' for '${spec.id}'`
                    );
                    continue;
            }
            _writePath(config, spec.path, parsed);
        }
    }
}

/**
 * Read a dotted path from a (possibly partially-initialised) object.
 * Returns undefined for missing intermediate keys so the caller can
 * fall back to the spec's default value.
 */
function _readPath(obj, path) {
    if (!obj) return undefined;
    const parts = path.split('.');
    let cur = obj;
    for (const part of parts) {
        if (cur == null || typeof cur !== 'object') return undefined;
        cur = cur[part];
    }
    return cur;
}

/**
 * Write a value at a dotted path, creating intermediate objects on
 * the way. `config.ui = config.ui ?? {}` was the most repeated line
 * in the saving paths; this collapses it.
 */
function _writePath(obj, path, value) {
    const parts = path.split('.');
    let cur = obj;
    for (let i = 0; i < parts.length - 1; i++) {
        const part = parts[i];
        if (cur[part] == null || typeof cur[part] !== 'object') {
            cur[part] = {};
        }
        cur = cur[part];
    }
    cur[parts[parts.length - 1]] = value;
}
