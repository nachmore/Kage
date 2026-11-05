/**
 * Effective extension config: the one place an extension's manifest
 * `config` defaults meet the user's saved values.
 *
 * The manifest `config` block is the only source of an extension's setting
 * defaults. `config.extensions[<id>]` holds just the user's overrides: the
 * values that differ from those defaults (see mergeSettingsSave). Every
 * reader goes through effectiveExtensionConfig / resolveExtensionConfig:
 * the runtime `context.config`, every onConfigUpdate payload, the Settings
 * page, and the settings provider's own `context.config`. So they all see
 * the same values, including keys a later version added and keys no
 * settings control shows.
 *
 * `enabled` belongs to the host. In the effective config it is whether
 * the extension is switched on (`extension_states`, the same state the
 * Settings header's Disable/Enable button sets). A saved `enabled` value is
 * ignored. Older hosts saved one from extensions' own "Enable" checkboxes;
 * a saved `false` (the user's "off") is moved to `extension_states` when the
 * backend loads config (`Config::fold_legacy_extension_enabled` in
 * src/config.rs), so it is still honoured.
 *
 * Pure functions with no DOM or Tauri access, so every window (and the
 * tests) use the same code. See docs/EXTENSIONS.md § Config Schema.
 */

import { isScratchId } from './settings-schema.js';

/** Host-owned key: whether the extension is switched on. */
export const ENABLED_KEY = 'enabled';

/** Types the manifest `config` schema supports (docs/EXTENSIONS.md). */
const MANIFEST_TYPES = new Set(['boolean', 'number', 'string']);

function isPlainObject(v) {
    return !!v && typeof v === 'object' && !Array.isArray(v);
}

/**
 * Keys the host never reads from saved config and never stores: scratch
 * (`__`) ids, which also covers a JSON `__proto__` key, and the host-owned
 * `enabled`.
 */
export function isHostManagedKey(key) {
    return key === ENABLED_KEY || isScratchId(key);
}

/** The manifest's declared entry for `key`, or null. */
function declaredEntry(manifest, key) {
    const schema = manifest?.config;
    if (!isPlainObject(schema) || !Object.hasOwn(schema, key)) return null;
    const entry = schema[key];
    return isPlainObject(entry) ? entry : null;
}

/** The manifest's declared type for `key` when it's a supported one, else null. */
function declaredType(manifest, key) {
    const type = declaredEntry(manifest, key)?.type;
    return MANIFEST_TYPES.has(type) ? type : null;
}

/**
 * `value` as the declared `type`, or undefined when it can't be read as
 * one. Lenient only where there's one obvious reading: a select control
 * always stores a string, so "5" for a number key and "true" for a
 * boolean key are taken as 5 and true.
 */
function asDeclaredType(type, value) {
    switch (type) {
        case 'boolean':
            if (typeof value === 'boolean') return value;
            if (value === 'true') return true;
            if (value === 'false') return false;
            return undefined;
        case 'number': {
            if (typeof value === 'number') return Number.isFinite(value) ? value : undefined;
            if (typeof value === 'string' && value.trim() !== '') {
                const n = Number(value);
                return Number.isFinite(n) ? n : undefined;
            }
            return undefined;
        }
        case 'string':
            if (typeof value === 'string') return value;
            if (typeof value === 'boolean') return String(value);
            if (typeof value === 'number' && Number.isFinite(value)) return String(value);
            return undefined;
        default:
            return value;
    }
}

/**
 * id → default for every key the manifest declares with a `default`.
 * Host-managed keys are left out: `enabled` comes from the host's own
 * state, and scratch ids are never config.
 *
 * @param {object} [manifest]
 * @returns {Record<string, unknown>}
 */
export function manifestConfigDefaults(manifest) {
    const out = {};
    const schema = manifest?.config;
    if (!isPlainObject(schema)) return out;
    for (const [key, entry] of Object.entries(schema)) {
        if (isHostManagedKey(key)) continue;
        if (!isPlainObject(entry) || !Object.hasOwn(entry, 'default')) continue;
        out[key] = entry.default;
    }
    return out;
}

/**
 * The config an extension runs with: the manifest defaults, overlaid with
 * the user's saved values, plus the host-owned `enabled`.
 *
 * - A key with no saved value takes the manifest default.
 * - A saved value for a declared, typed key is read as that type. A value
 *   that can't be (a hand-edited config.json, or a key whose type changed
 *   between versions) is ignored, so the default applies.
 * - Saved keys the manifest doesn't declare are passed through unchanged.
 *   They may come from an older or newer version, or the extension saved
 *   them itself.
 * - Saved scratch (`__`) and `enabled` values are ignored.
 *
 * Returns a new object on every call.
 *
 * @param {object} [manifest] - the extension manifest (only `config` is read)
 * @param {unknown} [saved] - config.extensions[id] as stored
 * @param {{ enabled?: boolean }} [opts] - the host's on/off state (default on)
 * @returns {Record<string, unknown>}
 */
export function effectiveExtensionConfig(manifest, saved, { enabled = true } = {}) {
    const out = manifestConfigDefaults(manifest);
    if (isPlainObject(saved)) {
        for (const [key, value] of Object.entries(saved)) {
            if (isHostManagedKey(key)) continue;
            const type = declaredType(manifest, key);
            if (!type) {
                out[key] = value;
                continue;
            }
            const typed = asDeclaredType(type, value);
            if (typed !== undefined) out[key] = typed;
        }
    }
    out[ENABLED_KEY] = enabled !== false;
    return out;
}

/** Whether the host has the extension switched on (`extension_states`). */
export function isExtensionEnabled(appConfig, id) {
    return appConfig?.extension_states?.[id] !== false;
}

/**
 * effectiveExtensionConfig for extension `id`, read from the full app
 * config (what `get_config` returns).
 *
 * @param {object} appConfig
 * @param {string} id
 * @param {object} [manifest]
 */
export function resolveExtensionConfig(appConfig, id, manifest) {
    return effectiveExtensionConfig(manifest, appConfig?.extensions?.[id], {
        enabled: isExtensionEnabled(appConfig, id),
    });
}

/**
 * The stored overrides after saving one extension's Settings page.
 *
 * - Only keys in `pageValues` (the page's value controls) can change. Every
 *   other stored key is kept as is: keys with no control on this page, and
 *   values the extension saved itself.
 * - A value equal to the manifest default is not stored, and an existing
 *   override equal to it is removed. So a later version's new default
 *   reaches everyone who never changed the setting.
 * - A value for a declared, typed key is stored as that type when it can
 *   be read as one.
 * - Scratch (`__`) and `enabled` values are never stored, and every stored
 *   one is removed, whether or not this page has a control for it. Nothing
 *   reads them (effectiveExtensionConfig skips them), and a scratch field
 *   an older host saved may hold a secret, such as a pasted token from a
 *   field a later version renamed.
 * - Values for keys the manifest doesn't declare are stored as is. With no
 *   manifest default to compare against, leaving one out would change what
 *   the extension sees.
 *
 * @param {object} [manifest]
 * @param {unknown} stored - config.extensions[id] before the save
 * @param {Record<string, unknown>} pageValues - id → live value of every value control on the page
 * @returns {Record<string, unknown>} the new config.extensions[id]
 */
export function mergeSettingsSave(manifest, stored, pageValues) {
    const out = {};
    if (isPlainObject(stored)) {
        for (const [key, value] of Object.entries(stored)) {
            if (!isHostManagedKey(key)) out[key] = value;
        }
    }
    const defaults = manifestConfigDefaults(manifest);
    for (const [key, value] of Object.entries(pageValues || {})) {
        if (isHostManagedKey(key)) continue;
        const type = declaredType(manifest, key);
        const typed = type ? asDeclaredType(type, value) : undefined;
        const toStore = typed !== undefined ? typed : value;
        if (Object.hasOwn(defaults, key) && sameValue(toStore, defaults[key], type)) {
            delete out[key];
        } else {
            out[key] = toStore;
        }
    }
    return out;
}

/** Whether `value` equals the manifest default, comparing as the declared type. */
function sameValue(value, defaultValue, type) {
    const d = type ? asDeclaredType(type, defaultValue) : defaultValue;
    return value === (d === undefined ? defaultValue : d);
}
