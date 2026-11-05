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
    // `list_open_windows` entries carry `minimized: boolean` (minimized
    // windows are listed and flagged, not filtered out).
    'windows.minimized',
]);

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
