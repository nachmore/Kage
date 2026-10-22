/**
 * Network status detection.
 * Uses a combination of navigator.onLine (instant but unreliable) and
 * an actual HTTP ping to detect real internet connectivity.
 *
 * Usage:
 *   import { isOnline, checkOnline, onNetworkChange, offlineMessage } from './network.js';
 *   if (!isOnline()) { ... }
 *   await checkOnline(); // force a real check
 *   onNetworkChange((online) => { ... });
 */

let _online = navigator.onLine;
let _lastCheck = 0;
let _inflight = null;
let _failures = 0;
let _retryTimer = null;
const _listeners = [];
const CHECK_INTERVAL = 30_000; // don't re-ping more than once per 30s
const PROBE_TIMEOUT_MS = 5_000;
// While the OS reports a connection, one failed probe isn't enough to
// say "offline": at login the network is often still coming up, and a
// single slow request would otherwise stick the banner on.
const FAILURES_BEFORE_OFFLINE = 2;
const CONFIRM_DELAY_MS = 3_000;
// While offline, keep re-checking so the banner clears on its own once the
// connection is back (previously only an agent reply cleared it).
const RETRY_DELAYS_MS = [5_000, 15_000, 30_000, 60_000];

// Probed in parallel; any response counts. Several unrelated hosts so one
// being blocked, slow, or filtered on a given network doesn't read as
// "no internet". `no-cors` resolves for any HTTP response (opaque).
const PROBE_URLS = [
    'https://www.msftconnecttest.com/connecttest.txt',
    'https://www.cloudflare.com/cdn-cgi/trace',
    'https://www.google.com/generate_204',
];

window.addEventListener('online', () => _handleBrowserEvent(true));
window.addEventListener('offline', () => _handleBrowserEvent(false));

function _handleBrowserEvent(browserOnline) {
    if (!browserOnline) {
        // Browser says offline — trust it immediately
        _failures = FAILURES_BEFORE_OFFLINE;
        _setOnline(false);
        _scheduleRetry();
    } else {
        // Browser says online — verify with a real ping
        _lastCheck = 0;
        checkOnline();
    }
}

function _setOnline(value) {
    if (_online !== value) {
        _online = value;
        for (const fn of _listeners) {
            try {
                fn(_online);
            } catch (e) {
                console.warn('Network listener error:', e);
            }
        }
    }
}

function _clearRetry() {
    clearTimeout(_retryTimer);
    _retryTimer = null;
}

function _scheduleRetry(delay) {
    _clearRetry();
    const step = Math.max(0, _failures - FAILURES_BEFORE_OFFLINE);
    const wait = delay ?? RETRY_DELAYS_MS[Math.min(step, RETRY_DELAYS_MS.length - 1)];
    _retryTimer = setTimeout(() => {
        _retryTimer = null;
        _lastCheck = 0;
        checkOnline();
    }, wait);
}

async function _probe(url) {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), PROBE_TIMEOUT_MS);
    try {
        await fetch(url, {
            method: 'HEAD',
            mode: 'no-cors',
            cache: 'no-store',
            signal: controller.signal,
        });
    } finally {
        clearTimeout(timeout);
    }
}

async function _runCheck() {
    try {
        await Promise.any(PROBE_URLS.map(_probe));
        _failures = 0;
        _clearRetry();
        _setOnline(true);
    } catch {
        _failures += 1;
        if (navigator.onLine === false || _failures >= FAILURES_BEFORE_OFFLINE) {
            _setOnline(false);
            _scheduleRetry();
        } else {
            // Unconfirmed: keep the current state and look again shortly.
            _scheduleRetry(CONFIRM_DELAY_MS);
        }
    }
    return _online;
}

/**
 * Perform a real connectivity check by probing a few well-known
 * endpoints. Updates the internal state and notifies listeners.
 * Concurrent callers share one check. Returns the result.
 */
export function checkOnline() {
    if (_inflight) return _inflight;
    const now = Date.now();
    if (now - _lastCheck < CHECK_INTERVAL) return Promise.resolve(_online);
    _lastCheck = now;
    _inflight = _runCheck().finally(() => {
        _inflight = null;
    });
    return _inflight;
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
    _failures = 0;
    _clearRetry();
    _setOnline(true);
}

import { t } from './i18n.js';

/** Resolve the offline message in the currently-active language.
 *  Always call this lazily — language can change at runtime. */
export function offlineMessage() {
    return t('shared.network.offline_message');
}

/** Snapshot of {@link offlineMessage}. The export exists for backward
 *  compatibility (older code and the network test suite); new callers
 *  should use {@link offlineMessage} so the string follows language
 *  changes at runtime. */
export const OFFLINE_MESSAGE =
    'No internet connection. Search, shortcuts, and app launching still work, but AI features need a connection.';
