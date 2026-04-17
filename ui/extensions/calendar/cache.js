/**
 * Shared calendar event cache.
 *
 * Cache expires at :25 and :55 past each hour so that a fresh Outlook query
 * runs just before the half-hour and full-hour marks — catching any last-minute
 * invites that arrived since the previous fetch.
 *
 * All calendar providers (search, triggers, tools) should use this instead of
 * calling invoke('get_calendar_events') directly.
 */

let _invoke = null;
let _events = [];
let _lastFetch = 0;
let _fetchInFlight = null;

/**
 * Initialise the cache with a Tauri invoke function.
 * Safe to call multiple times — only the first call (or a call with a new
 * invoke reference) takes effect.
 */
export function initCache(invoke) {
    if (invoke) _invoke = invoke;
}

/**
 * Return true if the cache is stale and a new Outlook query is needed.
 *
 * The cache boundary is the most recent :25 or :55 minute mark.  If the last
 * fetch happened before that boundary, the cache is stale.
 */
function isCacheExpired() {
    if (_lastFetch === 0) return true;

    const now = new Date();
    const mins = now.getMinutes();

    // Find the most recent boundary (:25 or :55) in the current hour
    let boundary = new Date(now);
    boundary.setSeconds(0, 0);
    if (mins >= 55) {
        boundary.setMinutes(55);
    } else if (mins >= 25) {
        boundary.setMinutes(25);
    } else {
        // Before :25 — boundary is :55 of the *previous* hour
        boundary.setMinutes(55);
        boundary.setTime(boundary.getTime() - 3600_000);
    }

    return _lastFetch < boundary.getTime();
}

/**
 * Get cached events, refreshing from Outlook if the cache has expired.
 *
 * @param {object} opts
 * @param {number} [opts.hours]        Lookahead hours (default 8)
 * @param {boolean} [opts.force]       Force a refresh regardless of cache state
 * @returns {Promise<Array>}
 */
export async function getEvents({ hours = 8, force = false } = {}) {
    if (!_invoke) return _events;

    if (!force && !isCacheExpired() && _events.length > 0) {
        return _events;
    }

    // If a fetch is already in flight, piggyback on it
    if (_fetchInFlight) return _fetchInFlight;

    _fetchInFlight = (async () => {
        try {
            _events = await _invoke('get_calendar_events', { hours });
            _lastFetch = Date.now();
        } catch (e) {
            console.warn('[Calendar cache] Failed to fetch events:', e);
        } finally {
            _fetchInFlight = null;
        }
        return _events;
    })();

    return _fetchInFlight;
}

/**
 * Force-clear the cache so the next getEvents() call will re-query.
 */
export function invalidate() {
    _lastFetch = 0;
}
