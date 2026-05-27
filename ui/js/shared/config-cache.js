/**
 * Per-window cache for the kage Config object.
 *
 * Reading config from the backend used to mean a fresh `invoke('get_config')`
 * IPC roundtrip every time — and the floating + chat windows hit this on
 * focus events, every quick-action chip render, every translate-button
 * click, theme reapply, and so on. A single user session would routinely
 * fire 20+ identical config reads.
 *
 * This module memoises the result. The first call invokes the backend
 * and caches the resolved Config. Subsequent calls return a fresh
 * `structuredClone` of the cached value — callers are free to mutate
 * what they receive without polluting the cache for everyone else.
 * Concurrent first-time calls share the same in-flight invoke promise,
 * so a burst of simultaneous readers triggers exactly one IPC.
 *
 * Invalidation is automatic via the `config_updated` Tauri event, which
 * the backend broadcasts on every successful `save_config`. The cache
 * also invalidates if the invoke itself fails, so a transient backend
 * error doesn't poison subsequent reads.
 *
 * Mutation hazard: callsites that read get_config and then immediately
 * mutate the result before saving (e.g. settings/manager.js's `save()`)
 * are safe because we always hand back clones, not the cached reference.
 *
 * Per-window scope: each Tauri window runs its own JS context, so each
 * window has its own cache that invalidates independently when
 * config_updated arrives. That's fine — windows don't share state in JS.
 */

import { EVT } from './events.js';

let _cachedPromise = null;
let _listenerInstalled = false;

/**
 * Install the `config_updated` listener once per module-load. Idempotent —
 * subsequent calls are no-ops. Uses the global `window.__TAURI__.event.listen`
 * so we don't need every caller to plumb it in.
 */
function _ensureInvalidationListener() {
    if (_listenerInstalled) return;
    const listen = window?.__TAURI__?.event?.listen;
    if (typeof listen !== 'function') {
        // Tauri globals haven't loaded yet (rare — only happens for code
        // that imports this module before tauri.js does its window-global
        // injection). Caller will retry on next getConfig.
        return;
    }
    _listenerInstalled = true;
    listen(EVT.CONFIG_UPDATED, () => {
        _cachedPromise = null;
    }).catch(() => {
        // listen() rejected — drop the flag so a later getConfig retries.
        _listenerInstalled = false;
    });
}

/**
 * Get a clone of the current Config, fetching from the backend if needed.
 *
 * @param {Function} invoke - The Tauri `invoke` function. Pass it in
 *   rather than reading a global so callers that already have a bound
 *   reference (e.g. `this.invoke` on app classes) can use that.
 * @returns {Promise<object>} a fresh clone of the cached Config.
 */
export async function getConfig(invoke) {
    _ensureInvalidationListener();

    if (!_cachedPromise) {
        // Cache the *promise* so a burst of concurrent first-time readers
        // shares one invoke. If the invoke rejects we drop the cached
        // promise so a later caller retries.
        _cachedPromise = (async () => {
            try {
                return await invoke('get_config');
            } catch (e) {
                _cachedPromise = null;
                throw e;
            }
        })();
    }

    const config = await _cachedPromise;
    return structuredClone(config);
}

/**
 * Drop the cached value. Next `getConfig` call will re-fetch from the
 * backend. Useful from tests and for any caller that knows the cache is
 * stale (typically not needed — `config_updated` handles invalidation).
 */
export function invalidateConfig() {
    _cachedPromise = null;
}
