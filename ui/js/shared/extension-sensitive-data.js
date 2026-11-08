/**
 * Manifest declarations of what an extension stores that grants access if
 * leaked — OAuth tokens, API keys, passwords, private URLs. Backups use them
 * to encrypt or leave that data out (docs/EXTENSIONS.md § Sensitive data and
 * backups).
 *
 *   - `sensitiveData`: an array of storage keys (the `key` passed to
 *     `save_extension_data`), each exact ("creds") or a prefix glob ending in
 *     "*" ("token_*"). At most 32 entries.
 *   - `config.<field>.sensitive: true`: a setting whose value is sensitive.
 *
 * The backend (`src/extensions.rs`) is authoritative and applies the same
 * rules; this is the frontend's normalisation, matching how capabilities
 * (`normalizePermissions`) and mascot activities are read: malformed entries
 * are dropped with a console warning, never fatal. Pure functions, no DOM.
 */

/** Most `sensitiveData` entries read. Mirrors MAX_SENSITIVE_DATA_ENTRIES. */
export const SENSITIVE_DATA_MAX_ENTRIES = 32;

/**
 * Safety net: an undeclared storage key or setting whose name matches is
 * treated as sensitive anyway. Mirrors SENSITIVE_NAME_PATTERN in
 * src/extensions.rs.
 */
export const SENSITIVE_NAME_RE =
    /(token|secret|credential|creds|password|passwd|passphrase|passcode|api[-_\s]?key|license[-_]?key|account[-_]?key|private[-_]?key|access[-_]?key|auth|oauth|session|cookie|totp|otp[-_]?(secret|seed)|mnemonic|(^|[-_.])(pin|pass|pwd)$)/i;

/** The host's data-key rule (`validate_data_key` in src/extensions.rs). */
function isValidDataKey(key) {
    return (
        typeof key === 'string' &&
        key.length >= 1 &&
        key.length <= 128 &&
        /^[A-Za-z0-9._-]+$/.test(key) &&
        !key.includes('..')
    );
}

/**
 * Validate a manifest's raw `sensitiveData`. Returns the accepted entries,
 * lowercased (matching ignores case) and deduplicated, in manifest order.
 *
 * @param {unknown} raw
 * @param {string} extensionId - for warnings
 * @returns {string[]}
 */
export function normalizeSensitiveData(raw, extensionId) {
    if (raw === undefined || raw === null) return [];
    if (!Array.isArray(raw)) {
        console.warn(
            `Extension '${extensionId}': sensitiveData must be an array of storage keys — ignored`
        );
        return [];
    }
    const out = [];
    for (let i = 0; i < raw.length; i++) {
        if (i >= SENSITIVE_DATA_MAX_ENTRIES) {
            console.warn(
                `Extension '${extensionId}': sensitiveData has more than ${SENSITIVE_DATA_MAX_ENTRIES} entries; the rest are ignored`
            );
            break;
        }
        const entry = raw[i];
        if (typeof entry !== 'string') {
            console.warn(
                `Extension '${extensionId}': sensitiveData entry ${i} is not a string — ignored`
            );
            continue;
        }
        const glob = entry.endsWith('*');
        const key = glob ? entry.slice(0, -1) : entry;
        const valid = glob
            ? !key.includes('*') && (key === '' || isValidDataKey(key))
            : isValidDataKey(key);
        if (!valid) {
            console.warn(
                `Extension '${extensionId}': invalid sensitiveData entry ${JSON.stringify(entry)} — ignored`
            );
            continue;
        }
        const normalized = entry.toLowerCase();
        if (!out.includes(normalized)) out.push(normalized);
    }
    return out;
}

/**
 * Whether storage `key` is covered by normalised `patterns`.
 *
 * @param {string} key
 * @param {string[]} patterns - from {@link normalizeSensitiveData}
 */
export function matchesSensitiveData(key, patterns) {
    if (typeof key !== 'string' || !Array.isArray(patterns)) return false;
    const k = key.toLowerCase();
    return patterns.some((p) => (p.endsWith('*') ? k.startsWith(p.slice(0, -1)) : k === p));
}

/**
 * Setting names the manifest marks `sensitive: true`. A non-boolean
 * `sensitive` is malformed: ignored with a warning.
 *
 * @param {object} [manifest]
 * @returns {string[]}
 */
export function sensitiveConfigFields(manifest) {
    const schema = manifest?.config;
    if (!schema || typeof schema !== 'object' || Array.isArray(schema)) return [];
    const out = [];
    for (const [field, entry] of Object.entries(schema)) {
        if (!entry || typeof entry !== 'object' || !Object.hasOwn(entry, 'sensitive')) continue;
        if (entry.sensitive === true) out.push(field);
        else if (entry.sensitive !== false) {
            console.warn(
                `Extension '${manifest.id}': config.${field}.sensitive must be true or false — ignored`
            );
        }
    }
    return out;
}

/**
 * Setting names a settings-only (unencrypted) backup may copy as free-form
 * strings: marked `backup: "plain"` and not `sensitive: true` (which wins).
 * Any other `backup` value is malformed: ignored with a warning. Booleans,
 * numbers and `enum` choices need no mark. The backend
 * (`config_field_decls` in src/extensions.rs) is authoritative.
 *
 * @param {object} [manifest]
 * @returns {string[]}
 */
export function plainBackupFields(manifest) {
    const schema = manifest?.config;
    if (!schema || typeof schema !== 'object' || Array.isArray(schema)) return [];
    const out = [];
    for (const [field, entry] of Object.entries(schema)) {
        if (!entry || typeof entry !== 'object' || !Object.hasOwn(entry, 'backup')) continue;
        if (entry.backup !== 'plain') {
            console.warn(
                `Extension '${manifest.id}': config.${field}.backup must be "plain" — ignored`
            );
        } else if (entry.sensitive === true) {
            console.warn(
                `Extension '${manifest.id}': config.${field} is both sensitive and backup "plain"; it's treated as sensitive`
            );
        } else {
            out.push(field);
        }
    }
    return out;
}
