/**
 * Small pieces of the extension-facing host API that the sandbox runtime
 * exposes. Self-contained (no imports): the host inlines this file into the
 * srcdoc runtime (see RUNTIME_SOURCE_PATHS in extension-sandbox-host.js).
 */

/**
 * Optional host behaviours an extension can feature-detect through
 * `context.hostFeatures` before relying on them. Hosts older than this list
 * don't set `context.hostFeatures` at all, so a missing list means "none".
 * Names are only ever added, never repurposed. See docs/EXTENSIONS.md.
 */
export const HOST_FEATURES = Object.freeze([
    // Settings `action` controls with a `copy` field: the host copies the
    // text to the clipboard itself, inside the click.
    'settings.copy_action',
    // invoke() rejections carry the Rust AppError's `kind` and `key`.
    'invoke.error_kind',
    // `context.config` is the manifest `config` defaults overlaid with the
    // user's saved values, and `config.enabled` is the host's on/off switch.
    // Settings controls whose id the manifest declares take that default and
    // ignore their own.
    'config.manifest_defaults',
    // `list_open_windows` entries carry `minimized: boolean`. Whether
    // minimized windows are listed and flagged varies by platform and
    // permission (docs/EXTENSIONS.md § Other apps' windows).
    'windows.minimized',
    // Restoring a backup never replaces what the manifest's `sensitiveData`
    // and `config.<field>.sensitive` declare with an empty or missing copy,
    // and none of it is in an unencrypted backup (docs/EXTENSIONS.md §
    // Sensitive data and backups).
    'export.sensitive_data',
    // Settings-only (unencrypted) backups copy the settings `config`
    // declares as booleans, numbers, `enum` choices or `backup: "plain"`
    // strings (docs/EXTENSIONS.md § Marking settings safe to copy).
    'export.plain_settings',
    // `save_extension_data` replaces the file atomically (write temp +
    // rename), so a crash never leaves a truncated value.
    'storage.atomic_write',
    // `context.storage.onChanged` tells this instance when the extension's
    // stored data changed in another window (docs/design/EXTENSION_VIEWS.md
    // §10). Key names only; re-read what you need.
    'storage.change_events',
]);

/** Ops and sources a storage change may carry (§10.1). */
const STORAGE_CHANGE_OPS = Object.freeze(['save', 'delete', 'bulk']);
const STORAGE_CHANGE_SOURCES = Object.freeze(['window', 'import', 'sync', 'uninstall']);

/**
 * `context.storage` for every surface: `onChanged(fn)` subscribes to the
 * host's `storage-changed` messages and returns an unsubscribe function.
 * `fn` gets a frozen array of `{ key, op, self, source }`. A listener that
 * throws is logged and doesn't stop the others.
 *
 * @param {{ log: (level: string, ...args: unknown[]) => void }} deps
 */
export function createStorageEvents({ log }) {
    const listeners = new Set();
    const api = Object.freeze({
        onChanged(fn) {
            if (typeof fn !== 'function') throw new TypeError('onChanged needs a function');
            // A wrapper per call, so subscribing the same function twice
            // gives two independent subscriptions.
            const entry = { fn };
            listeners.add(entry);
            return () => {
                listeners.delete(entry);
            };
        },
    });
    return {
        api,
        /** Deliver one `storage-changed` batch to every listener. */
        deliver(changes) {
            if (!Array.isArray(changes) || listeners.size === 0) return;
            const clean = [];
            for (const c of changes) {
                if (!c || typeof c.key !== 'string' || !STORAGE_CHANGE_OPS.includes(c.op)) continue;
                clean.push(
                    Object.freeze({
                        key: c.key,
                        op: c.op,
                        self: c.self === true,
                        source: STORAGE_CHANGE_SOURCES.includes(c.source) ? c.source : 'window',
                    })
                );
            }
            if (clean.length === 0) return;
            const batch = Object.freeze(clean);
            for (const entry of [...listeners]) {
                try {
                    entry.fn(batch);
                } catch (e) {
                    log('warn', `storage.onChanged listener failed: ${e?.message || e}`);
                }
            }
        },
    };
}

/**
 * The Error an extension's `invoke()` promise rejects with. `message` is
 * the human-readable text; `kind` / `key` are set when the backend returned a
 * structured AppError, e.g. `kind: 'timeout'` vs `kind: 'cancelled'` from
 * `oauth_loopback_await`.
 *
 * @param {{error: unknown, errorKind?: unknown, errorKey?: unknown}} msg
 *   an `invoke-response` port message
 * @returns {Error & {kind?: string, key?: string}}
 */
export function invokeError(msg) {
    const err = new Error(String(msg.error));
    if (typeof msg.errorKind === 'string') err.kind = msg.errorKind;
    if (typeof msg.errorKey === 'string') err.key = msg.errorKey;
    return err;
}
