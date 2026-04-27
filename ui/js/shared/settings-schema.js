/**
 * Declarative settings schema for sandboxed extensions.
 *
 * The shape below is what an extension's settings provider returns from
 * its `getSettings()` RPC. The host validates + renders from it; the
 * extension never touches host DOM.
 *
 * Capability story: settings rendering itself is pure data exchange and
 * doesn't require any capability. Action buttons that invoke Tauri
 * commands still go through the sandbox's normal `invoke()`, which means
 * the extension needs the right capability in its manifest for the
 * command to work.
 *
 * @typedef {object} SettingsSchema
 * @property {string} [title]      Display title (falls back to manifest name)
 * @property {string} [description] Short description shown under the title
 * @property {SchemaSection[]} sections
 *
 * @typedef {object} SchemaSection
 * @property {string} [label]      Optional section header
 * @property {SchemaControl[]} controls
 *
 * @typedef {(
 *   SchemaCheckbox | SchemaText | SchemaNumber | SchemaSelect |
 *   SchemaRange    | SchemaAction | SchemaInfo
 * )} SchemaControl
 *
 * @typedef {object} SchemaShowWhen
 *   Conditional visibility. Shown only when the control at `id` has the
 *   given value (or is in `oneOf`). Missing → always shown.
 * @property {string} id
 * @property {*} [equals]
 * @property {*[]} [oneOf]
 *
 * @typedef {object} SchemaCheckbox
 * @property {'checkbox'} type
 * @property {string} id
 * @property {string} label
 * @property {string} [description]
 * @property {boolean} [default]
 * @property {SchemaShowWhen} [showWhen]
 *
 * @typedef {object} SchemaText
 * @property {'text'} type
 * @property {string} id
 * @property {string} label
 * @property {string} [description]
 * @property {string} [default]
 * @property {string} [placeholder]
 * @property {number} [maxWidth]    Preferred max-width in px for the input
 * @property {SchemaShowWhen} [showWhen]
 *
 * @typedef {object} SchemaNumber
 * @property {'number'} type
 * @property {string} id
 * @property {string} label
 * @property {string} [description]
 * @property {number} [default]
 * @property {number} [min]
 * @property {number} [max]
 * @property {number} [step]
 * @property {number} [maxWidth]
 * @property {SchemaShowWhen} [showWhen]
 *
 * @typedef {object} SchemaSelect
 * @property {'select'} type
 * @property {string} id
 * @property {string} label
 * @property {string} [description]
 * @property {string} [default]
 * @property {Array<{value: string, label: string}>} options
 * @property {number} [maxWidth]
 * @property {SchemaShowWhen} [showWhen]
 *
 * @typedef {object} SchemaRange
 * @property {'range'} type
 * @property {string} id
 * @property {string} label
 * @property {string} [description]
 * @property {number} [default]
 * @property {number} min
 * @property {number} max
 * @property {number} [step]
 * @property {string} [unit]        Shown next to the value label, e.g. "×"
 * @property {SchemaShowWhen} [showWhen]
 *
 * @typedef {object} SchemaAction
 * @property {'action'} type
 * @property {string} id            Host-local identifier for the button row
 * @property {string} label         Button text
 * @property {string} [description]
 * @property {string} action        RPC name to call on the extension's settings provider
 * @property {'default'|'danger'|'primary'} [variant]
 * @property {string} [confirm]     Native confirm() prompt before running
 * @property {SchemaShowWhen} [showWhen]
 *
 * @typedef {object} SchemaInfo
 *   Static informational block. `html` is passed through a strict
 *   sanitizer that only allows a small whitelist (see sanitizeInfoHtml).
 *   Use it for attribution, help text, or command references.
 * @property {'info'} type
 * @property {string} [label]
 * @property {string} html
 * @property {SchemaShowWhen} [showWhen]
 */

export const CONTROL_TYPES = Object.freeze([
    'checkbox', 'text', 'number', 'select', 'range', 'action', 'info',
]);

export const ACTION_VARIANTS = Object.freeze(['default', 'danger', 'primary']);

/**
 * Validate a schema object before we render it, so we fail fast with a
 * clear error rather than rendering broken UI.
 *
 * @param {unknown} schema
 * @returns {{ok: true, schema: SettingsSchema} | {ok: false, error: string}}
 */
export function validateSchema(schema) {
    if (!schema || typeof schema !== 'object' || Array.isArray(schema)) {
        return { ok: false, error: 'schema must be an object' };
    }
    if (!Array.isArray(schema.sections)) {
        return { ok: false, error: 'schema.sections must be an array' };
    }
    const seenIds = new Set();
    for (let si = 0; si < schema.sections.length; si++) {
        const section = schema.sections[si];
        if (!section || typeof section !== 'object') {
            return { ok: false, error: `section[${si}] must be an object` };
        }
        if (!Array.isArray(section.controls)) {
            return { ok: false, error: `section[${si}].controls must be an array` };
        }
        for (let ci = 0; ci < section.controls.length; ci++) {
            const ctrl = section.controls[ci];
            const where = `section[${si}].controls[${ci}]`;
            const err = validateControl(ctrl, where, seenIds);
            if (err) return { ok: false, error: err };
        }
    }
    return { ok: true, schema };
}

function validateControl(ctrl, where, seenIds) {
    if (!ctrl || typeof ctrl !== 'object') return `${where} must be an object`;
    if (typeof ctrl.type !== 'string' || !CONTROL_TYPES.includes(ctrl.type)) {
        return `${where}.type must be one of ${CONTROL_TYPES.join(', ')}`;
    }
    if (ctrl.type !== 'info' && typeof ctrl.id !== 'string') {
        return `${where}.id must be a string`;
    }
    if (ctrl.id) {
        if (!/^[a-zA-Z_][a-zA-Z0-9_]{0,63}$/.test(ctrl.id)) {
            return `${where}.id must be a simple identifier (alphanumeric + underscore, up to 64 chars)`;
        }
        if (seenIds.has(ctrl.id)) {
            return `${where}.id '${ctrl.id}' is duplicated — ids must be unique across the whole schema`;
        }
        seenIds.add(ctrl.id);
    }

    switch (ctrl.type) {
        case 'select': {
            if (!Array.isArray(ctrl.options) || ctrl.options.length === 0) {
                return `${where}.options must be a non-empty array`;
            }
            for (let i = 0; i < ctrl.options.length; i++) {
                const opt = ctrl.options[i];
                if (!opt || typeof opt !== 'object') return `${where}.options[${i}] must be an object`;
                if (typeof opt.value !== 'string') return `${where}.options[${i}].value must be a string`;
                if (typeof opt.label !== 'string') return `${where}.options[${i}].label must be a string`;
            }
            break;
        }
        case 'range': {
            if (typeof ctrl.min !== 'number' || typeof ctrl.max !== 'number') {
                return `${where}.min and .max must be numbers`;
            }
            if (ctrl.max <= ctrl.min) {
                return `${where}.max must be greater than .min`;
            }
            break;
        }
        case 'action': {
            if (typeof ctrl.action !== 'string' || !ctrl.action) {
                return `${where}.action must be a non-empty RPC name`;
            }
            if (ctrl.variant && !ACTION_VARIANTS.includes(ctrl.variant)) {
                return `${where}.variant must be one of ${ACTION_VARIANTS.join(', ')}`;
            }
            break;
        }
        case 'info': {
            if (typeof ctrl.html !== 'string') {
                return `${where}.html must be a string`;
            }
            break;
        }
        default:
            break;
    }
    return null;
}

/**
 * Strict sanitizer for the `info` control's HTML. Only whitelisted tags
 * and attributes are allowed. Unknown tags are stripped; unknown
 * attributes are dropped. Used on host side before injecting into the DOM.
 *
 * Goals:
 *   - Preserve basic rich text (strong, em, code, br, p, ul/ol/li, a)
 *   - Block scripts, event handlers, inline styles, and dangerous URLs
 *
 * This isn't a general-purpose sanitizer — it covers what settings-page
 * info blocks actually need and nothing more.
 */
const ALLOWED_TAGS = new Set([
    'A', 'B', 'BR', 'CODE', 'DIV', 'EM', 'I', 'LI', 'OL', 'P', 'SPAN',
    'STRONG', 'UL',
]);

const ALLOWED_ATTRS_BY_TAG = {
    A: new Set(['href', 'title', 'target', 'rel']),
    SPAN: new Set(['class']),
    DIV: new Set(['class']),
    CODE: new Set(['class']),
};

/**
 * Returns a sanitized DocumentFragment from a trusted-but-constrained HTML
 * string. Call from host-side rendering code only.
 */
export function sanitizeInfoHtml(html) {
    const template = document.createElement('template');
    template.innerHTML = String(html);
    walkAndStrip(template.content);
    return template.content;
}

function walkAndStrip(node) {
    // Use a snapshot of children — we'll be mutating as we go.
    const kids = Array.from(node.childNodes);
    for (const child of kids) {
        if (child.nodeType === Node.ELEMENT_NODE) {
            processElement(child);
        } else if (child.nodeType === Node.COMMENT_NODE) {
            child.remove();
        }
        // Text nodes are fine as-is.
    }
}

function processElement(el) {
    if (!ALLOWED_TAGS.has(el.tagName)) {
        // Replace element with its text contents to preserve surrounding text.
        const text = document.createTextNode(el.textContent || '');
        el.replaceWith(text);
        return;
    }

    // Filter attributes.
    const allowed = ALLOWED_ATTRS_BY_TAG[el.tagName] || new Set();
    const toRemove = [];
    for (const attr of Array.from(el.attributes)) {
        const name = attr.name.toLowerCase();
        if (!allowed.has(name)) {
            toRemove.push(attr.name);
            continue;
        }
        if (name === 'href') {
            const val = attr.value.trim();
            // Only allow http(s), mailto, or in-page anchors.
            if (!/^(https?:|mailto:|#)/i.test(val)) {
                toRemove.push(attr.name);
            }
        }
    }
    for (const n of toRemove) el.removeAttribute(n);

    // Force target=_blank links to carry rel=noopener noreferrer.
    if (el.tagName === 'A' && el.getAttribute('target') === '_blank') {
        el.setAttribute('rel', 'noopener noreferrer');
    }

    walkAndStrip(el);
}

/**
 * Given a schema, produce the default values object. Used as the starting
 * point when no config is stored yet.
 */
export function defaultValues(schema) {
    const values = {};
    for (const section of schema.sections || []) {
        for (const ctrl of section.controls || []) {
            if (!ctrl.id) continue;
            switch (ctrl.type) {
                case 'checkbox': values[ctrl.id] = ctrl.default ?? false; break;
                case 'text':     values[ctrl.id] = ctrl.default ?? ''; break;
                case 'number':   values[ctrl.id] = ctrl.default ?? 0; break;
                case 'select':   values[ctrl.id] = ctrl.default ?? (ctrl.options?.[0]?.value ?? ''); break;
                case 'range':    values[ctrl.id] = ctrl.default ?? ctrl.min ?? 0; break;
                default: break;
            }
        }
    }
    return values;
}

/**
 * Walk the schema and call `fn` once per non-info, non-action control
 * (i.e. only "value-bearing" controls). Used for load/save passes.
 */
export function forEachValueControl(schema, fn) {
    for (const section of schema.sections || []) {
        for (const ctrl of section.controls || []) {
            if (!ctrl.id) continue;
            if (ctrl.type === 'info' || ctrl.type === 'action') continue;
            fn(ctrl);
        }
    }
}

/**
 * Evaluate a showWhen clause against the current values map. If no clause,
 * returns true.
 */
export function isVisible(showWhen, values) {
    if (!showWhen || typeof showWhen !== 'object') return true;
    const current = values[showWhen.id];
    if ('equals' in showWhen) {
        return current === showWhen.equals;
    }
    if (Array.isArray(showWhen.oneOf)) {
        return showWhen.oneOf.includes(current);
    }
    return true;
}
