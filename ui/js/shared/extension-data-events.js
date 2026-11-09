// Storage change events for extension sandboxes
// (docs/design/EXTENSION_VIEWS.md §10).
//
// Rust emits `extension_data_changed` to every window that runs extension
// instances after one of an extension's keys was saved or deleted, an import
// wrote its data, or (later) folder sync changed it:
//
//   { extension_id, changes: [{ key, op, source, window? }] }
//
// Each window's host code installs this listener once. It forwards a batch
// only to the sandbox of that extension, and only when the sandbox holds
// `storage`. It turns `window` (the label of the window whose sandbox wrote
// the key) into `self` and drops the label, so a sandbox never learns other
// window labels. Events carry key names only, never data: an instance
// re-reads what it needs. They are hints, not a log.

import { EVT } from './events.js';

/** What happened to a key. Mirrors `extension_data_events::ChangeOp`. */
export const CHANGE_OPS = Object.freeze(['save', 'delete', 'bulk']);

/** What wrote it. Mirrors `extension_data_events::ChangeSource`. */
export const CHANGE_SOURCES = Object.freeze(['window', 'import', 'sync', 'uninstall']);

/** The key of a `bulk` change: any key may have changed. */
export const BULK_KEY = '*';

/** Most changes in one batch (Rust's `MAX_BATCH_KEYS`). */
export const MAX_BATCH_CHANGES = 64;

// `extensions::validate_extension_id` and `extensions::validate_data_key`.
const EXTENSION_ID_RE = /^[a-z0-9][a-z0-9_-]{0,63}$/;
const DATA_KEY_RE = /^[A-Za-z0-9._-]{1,128}$/;

function validKey(key, op) {
    if (typeof key !== 'string') return false;
    if (op === 'bulk') return key === BULK_KEY;
    return DATA_KEY_RE.test(key) && !key.includes('..');
}

/**
 * Turn one `extension_data_changed` payload into what a sandbox sees.
 * Entries that don't have the expected shape are dropped.
 *
 * @param {unknown} payload The event payload.
 * @param {string} myLabel This window's label.
 * @returns {{ extensionId: string, changes: Array<{ key: string, op: string, self: boolean, source: string }> } | null}
 *   `null` when the payload names no valid extension or no valid change.
 */
export function toSandboxChanges(payload, myLabel) {
    if (!payload || typeof payload !== 'object') return null;
    const extensionId = payload.extension_id;
    if (typeof extensionId !== 'string' || !EXTENSION_ID_RE.test(extensionId)) return null;
    if (!Array.isArray(payload.changes)) return null;
    const changes = [];
    for (const change of payload.changes.slice(0, MAX_BATCH_CHANGES)) {
        if (!change || typeof change !== 'object') continue;
        const { key, op, source, window } = change;
        if (!CHANGE_OPS.includes(op) || !CHANGE_SOURCES.includes(source)) continue;
        if (!validKey(key, op)) continue;
        changes.push({
            key,
            op,
            self: typeof window === 'string' && typeof myLabel === 'string' && window === myLabel,
            source,
        });
    }
    return changes.length ? { extensionId, changes } : null;
}

/**
 * Whether a sandbox may hear of storage changes: it holds `storage`.
 *
 * @param {{ capabilities?: Set<string> | string[] } | null | undefined} sandbox
 */
export function holdsStorage(sandbox) {
    const caps = sandbox?.capabilities;
    if (!caps) return false;
    if (typeof caps.has === 'function') return caps.has('storage');
    return Array.isArray(caps) && caps.includes('storage');
}

/**
 * A `deliver` for {@link installExtensionDataEvents} that hands a batch to
 * the extension's sandbox (`sandbox.notifyStorageChanged(changes)`), when
 * this window runs one and it holds `storage`.
 *
 * @param {(extensionId: string) => any} sandboxFor This window's sandbox
 *   for an extension id, or a falsy value.
 * @returns {(extensionId: string, changes: object[]) => boolean} `true`
 *   when the batch was handed over.
 */
export function deliverToSandbox(sandboxFor) {
    return (extensionId, changes) => {
        const sandbox = sandboxFor(extensionId);
        if (!holdsStorage(sandbox) || typeof sandbox.notifyStorageChanged !== 'function') {
            return false;
        }
        sandbox.notifyStorageChanged(changes);
        return true;
    };
}

/**
 * Listen for `extension_data_changed` in this window and pass each batch
 * on as `deliver(extensionId, changes)`.
 *
 * @param {object} opts
 * @param {(event: string, handler: (event: { payload: unknown }) => void) => Promise<() => void> | (() => void)} opts.listen
 *   Tauri's `listen` (or a stand-in in tests).
 * @param {string | (() => string)} opts.myLabel This window's label, or a
 *   function that returns it.
 * @param {(extensionId: string, changes: object[]) => void} opts.deliver
 *   Called once per batch with the sandbox-facing changes. Use
 *   {@link deliverToSandbox} to reach only that extension's sandbox, and
 *   only with `storage`.
 * @returns {Promise<() => void>} Stops listening.
 */
export async function installExtensionDataEvents({ listen, myLabel, deliver }) {
    if (typeof listen !== 'function' || typeof deliver !== 'function') {
        throw new TypeError('installExtensionDataEvents needs listen and deliver');
    }
    const labelOf = typeof myLabel === 'function' ? myLabel : () => myLabel;
    const unlisten = await listen(EVT.EXTENSION_DATA_CHANGED, (event) => {
        const batch = toSandboxChanges(event?.payload, labelOf());
        if (!batch) return;
        try {
            deliver(batch.extensionId, batch.changes);
        } catch (e) {
            console.warn(`[ext ${batch.extensionId}] storage change delivery failed:`, e);
        }
    });
    return typeof unlisten === 'function' ? unlisten : () => {};
}
