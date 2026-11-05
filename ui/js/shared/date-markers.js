/**
 * Busy-day markers for the date picker: validation of the (untrusted)
 * provider payload, and a short-lived per-range cache so paging back and
 * forth between months doesn't re-query the calendar every time.
 *
 * The provider is an extension's search provider (`getDateMarkers`), so its
 * answer is untrusted input to host UI: only ISO days inside the requested
 * range survive, counts are clamped integers, colours must be plain hex, and
 * the status is a closed vocabulary the host maps to its OWN hint strings —
 * the extension never gets to put text in the picker.
 */

import { compareIso, isValidIsoDate } from './month-grid.js';

/** Statuses a provider may report. Anything else collapses to `'ok'`. */
export const MARKER_STATUSES = Object.freeze([
    'ok',
    // The OS refused calendar access → the picker shows a "grant access" hint.
    'permission_denied',
    // Calendar backend failed or isn't available → quietly no dots.
    'error',
    // Host-side only: no extension can answer (none installed/enabled).
    'no_provider',
]);

const MAX_COUNT = 999;
const MAX_COLORS = 3;
const HEX_COLOR = /^#(?:[0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;

/**
 * @typedef {{ count: number, colors: string[] }} DayMarker
 * @typedef {{ status: string, days: Map<string, DayMarker> }} MarkerSet
 */

/** An empty result with the given status. */
export function emptyMarkers(status = 'ok') {
    return { status, days: new Map() };
}

/**
 * Validate a provider's `getDateMarkers` answer for `[start, end)`.
 *
 * Accepts `days` as an object keyed by ISO day (`{ '2026-11-05': { count: 2 } }`)
 * or as an array of `{ date, count, colors? }` — the second is the shape
 * `get_calendar_busy_days` returns, so a provider can pass it straight through.
 *
 * @returns {MarkerSet}
 */
export function normalizeMarkers(raw, start, end) {
    if (!raw || typeof raw !== 'object') return emptyMarkers('error');
    const status = MARKER_STATUSES.includes(raw.status) ? raw.status : 'ok';
    const days = new Map();
    const entries = Array.isArray(raw.days)
        ? raw.days.map((d) => [d?.date, d])
        : raw.days && typeof raw.days === 'object'
          ? Object.entries(raw.days)
          : [];
    for (const [iso, value] of entries) {
        if (!isValidIsoDate(iso)) continue;
        if (compareIso(iso, start) < 0 || compareIso(iso, end) >= 0) continue;
        const n = Math.floor(Number(typeof value === 'number' ? value : value?.count));
        if (!Number.isFinite(n) || n <= 0) continue;
        const colors = [];
        if (Array.isArray(value?.colors)) {
            for (const c of value.colors) {
                if (typeof c !== 'string' || !HEX_COLOR.test(c)) continue;
                const lc = c.toLowerCase();
                if (!colors.includes(lc)) colors.push(lc);
                if (colors.length >= MAX_COLORS) break;
            }
        }
        days.set(iso, { count: Math.min(n, MAX_COUNT), colors });
    }
    return { status, days };
}

/**
 * Per-range cache with in-flight de-duplication.
 *
 * - A fresh hit (younger than `ttlMs`) resolves without calling `fetch`.
 * - Concurrent requests for the same range share one fetch.
 * - `'error'` answers are kept only `errorTtlMs`, so a transient backend
 *   failure doesn't blank the dots for a whole minute; permission denials are
 *   cached normally (they don't fix themselves mid-session, and re-asking
 *   would just re-spawn the OS query).
 * - `fetch` rejecting is treated as an `'error'` answer — the picker never
 *   sees an exception.
 *
 * @param {object} o
 * @param {(key: string, start: string, end: string) => Promise<MarkerSet>} o.fetch
 * @param {number} [o.ttlMs]
 * @param {number} [o.errorTtlMs]
 * @param {() => number} [o.now]
 * @param {number} [o.maxEntries]
 */
export function createMarkerCache({
    fetch,
    ttlMs = 60_000,
    errorTtlMs = 5_000,
    now = () => Date.now(),
    maxEntries = 24,
}) {
    /** @type {Map<string, { at: number, value: MarkerSet }>} */
    const entries = new Map();
    /** @type {Map<string, Promise<MarkerSet>>} */
    const inflight = new Map();

    const keyOf = (scope, start, end) => `${scope}|${start}|${end}`;

    function fresh(key) {
        const e = entries.get(key);
        if (!e) return null;
        const ttl = e.value.status === 'error' ? errorTtlMs : ttlMs;
        if (now() - e.at > ttl) {
            entries.delete(key);
            return null;
        }
        return e.value;
    }

    return {
        /** Cached answer if still fresh, else null. Synchronous. */
        peek(scope, start, end) {
            return fresh(keyOf(scope, start, end));
        },

        /** Resolve the markers for `scope` (provider id) over `[start, end)`. */
        get(scope, start, end) {
            const key = keyOf(scope, start, end);
            const hit = fresh(key);
            if (hit) return Promise.resolve(hit);
            const pending = inflight.get(key);
            if (pending) return pending;
            const p = Promise.resolve()
                .then(() => fetch(scope, start, end))
                .catch(() => emptyMarkers('error'))
                .then((value) => {
                    const v = value && value.days instanceof Map ? value : emptyMarkers('error');
                    entries.set(key, { at: now(), value: v });
                    while (entries.size > maxEntries) {
                        entries.delete(entries.keys().next().value);
                    }
                    return v;
                })
                .finally(() => inflight.delete(key));
            inflight.set(key, p);
            return p;
        },

        clear() {
            entries.clear();
        },
    };
}
