/**
 * Network status detection.
 * Uses a combination of navigator.onLine (instant but unreliable) and
 * an actual HTTP ping to detect real internet connectivity.
 *
 * Usage:
 *   import { isOnline, checkOnline, onNetworkChange, OFFLINE_MESSAGE } from './network.js';
 *   if (!isOnline()) { ... }
 *   await checkOnline(); // force a real check
 *   onNetworkChange((online) => { ... });
 */

let _online = navigator.onLine;
let _lastCheck = 0;
const _listeners = [];
const CHECK_INTERVAL = 30_000; // don't re-ping more than once per 30s

window.addEventListener('online', () => _handleBrowserEvent(true));
window.addEventListener('offline', () => _handleBrowserEvent(false));

function _handleBrowserEvent(browserOnline) {
    if (!browserOnline) {
        // Browser says offline — trust it immediately
        _setOnline(false);
    } else {
        // Browser says online — verify with a real ping
        checkOnline();
    }
}

function _setOnline(value) {
    if (_online !== value) {
        _online = value;
        for (const fn of _listeners) {
            try { fn(_online); } catch (e) { console.warn('Network listener error:', e); }
        }
    }
}

/**
 * Perform a real connectivity check by fetching a small resource.
 * Updates the internal state and notifies listeners.
 * Returns the result.
 */
export async function checkOnline() {
    const now = Date.now();
    if (now - _lastCheck < CHECK_INTERVAL) return _online;
    _lastCheck = now;

    try {
        // Use a tiny fetch with a short timeout to test real connectivity.
        // We hit a known always-available endpoint. The HEAD request is ~0 bytes.
        const controller = new AbortController();
        const timeout = setTimeout(() => controller.abort(), 2000);
        await fetch('https://aws.amazon.com/favicon.ico', {
            method: 'HEAD',
            mode: 'no-cors',
            cache: 'no-store',
            signal: controller.signal,
        });
        clearTimeout(timeout);
        _setOnline(true);
    } catch {
        _setOnline(false);
    }
    return _online;
}

/** Returns the last known online status (may be stale — use checkOnline() for a fresh check). */
export function isOnline() {
    return _online;
}

/** Register a callback for network status changes. Returns an unsubscribe function. */
export function onNetworkChange(fn) {
    _listeners.push(fn);
    return () => {
        const idx = _listeners.indexOf(fn);
        if (idx >= 0) _listeners.splice(idx, 1);
    };
}

/**
 * Call this when an ACP or network-related error is detected.
 * Triggers a real connectivity check and returns the result.
 */
export async function checkOnError() {
    _lastCheck = 0; // force a fresh check
    return checkOnline();
}

/**
 * Call this when a successful agent response is received.
 * Marks us as online without needing a ping.
 */
export function markOnline() {
    _setOnline(true);
}

/** User-friendly message explaining what works offline. */
export const OFFLINE_MESSAGE = 'No internet connection. Search, shortcuts, and app launching still work, but AI features need a connection.';
