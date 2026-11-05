/**
 * Busy-day markers (ui/js/shared/date-markers.js): validation of the
 * untrusted provider payload, and the short per-range cache.
 */

import { describe, expect, it, vi } from 'vitest';
import {
    createMarkerCache,
    emptyMarkers,
    normalizeMarkers,
} from '../../ui/js/shared/date-markers.js';

const START = '2026-10-26';
const END = '2026-12-07';

describe('normalizeMarkers', () => {
    it('accepts the get_calendar_busy_days array shape', () => {
        const m = normalizeMarkers(
            { status: 'ok', days: [{ date: '2026-11-05', count: 2 }, { date: '2026-11-20', count: 1 }] },
            START,
            END
        );
        expect(m.status).toBe('ok');
        expect([...m.days]).toEqual([
            ['2026-11-05', { count: 2, colors: [] }],
            ['2026-11-20', { count: 1, colors: [] }],
        ]);
    });

    it('accepts an object keyed by day, with bare counts or objects', () => {
        const m = normalizeMarkers({ days: { '2026-11-05': 3, '2026-11-06': { count: 1 } } }, START, END);
        expect(m.days.get('2026-11-05').count).toBe(3);
        expect(m.days.get('2026-11-06').count).toBe(1);
    });

    it('drops days outside [start, end), malformed days and empty counts', () => {
        const m = normalizeMarkers(
            {
                days: {
                    '2026-10-25': 1, // before start
                    '2026-12-07': 1, // end is exclusive
                    '2026-02-30': 1, // not a date
                    '2026-11-5': 1, // wrong shape
                    '<b>x</b>': 1,
                    '2026-11-10': 0,
                    '2026-11-11': -4,
                    '2026-11-12': 'lots',
                    '2026-10-26': 1, // start is inclusive
                },
            },
            START,
            END
        );
        expect([...m.days.keys()]).toEqual(['2026-10-26']);
    });

    it('clamps counts and keeps at most three distinct hex colours', () => {
        const m = normalizeMarkers(
            {
                days: {
                    '2026-11-05': {
                        count: 1e9,
                        colors: ['#ff0000', '#FF0000', 'red', 'url(x)', '#0f0', '#123456aa', '#abc', 7],
                    },
                },
            },
            START,
            END
        );
        expect(m.days.get('2026-11-05')).toEqual({ count: 999, colors: ['#ff0000', '#0f0', '#123456aa'] });
    });

    it('keeps the status vocabulary closed', () => {
        expect(normalizeMarkers({ status: 'permission_denied', days: [] }, START, END).status).toBe('permission_denied');
        expect(normalizeMarkers({ status: '<script>', days: [] }, START, END).status).toBe('ok');
        expect(normalizeMarkers(null, START, END).status).toBe('error');
        expect(normalizeMarkers('nope', START, END).status).toBe('error');
    });
});

describe('createMarkerCache', () => {
    const answer = (n = 1) => normalizeMarkers({ days: { '2026-11-05': n } }, START, END);

    it('serves a fresh hit without refetching, and refetches after the TTL', async () => {
        let now = 0;
        const fetch = vi.fn(async () => answer());
        const cache = createMarkerCache({ fetch, ttlMs: 60_000, now: () => now });
        await cache.get('calendar', START, END);
        await cache.get('calendar', START, END);
        expect(fetch).toHaveBeenCalledTimes(1);
        expect(cache.peek('calendar', START, END)?.days.get('2026-11-05').count).toBe(1);
        now = 61_000;
        expect(cache.peek('calendar', START, END)).toBeNull();
        await cache.get('calendar', START, END);
        expect(fetch).toHaveBeenCalledTimes(2);
    });

    it('shares one in-flight fetch between concurrent callers', async () => {
        let release;
        const fetch = vi.fn(() => new Promise((r) => (release = r)));
        const cache = createMarkerCache({ fetch });
        const a = cache.get('calendar', START, END);
        const b = cache.get('calendar', START, END);
        await new Promise((r) => setTimeout(r, 0)); // fetch starts on a microtask
        release(answer(2));
        expect((await a).days.get('2026-11-05').count).toBe(2);
        expect(await b).toBe(await a);
        expect(fetch).toHaveBeenCalledTimes(1);
    });

    it('keys by provider and range', async () => {
        const fetch = vi.fn(async () => answer());
        const cache = createMarkerCache({ fetch });
        await cache.get('calendar', START, END);
        await cache.get('calendar', '2026-11-30', '2027-01-11');
        await cache.get('other', START, END);
        expect(fetch).toHaveBeenCalledTimes(3);
    });

    it('turns a rejected fetch into an error answer and retries it sooner', async () => {
        let now = 0;
        const fetch = vi.fn(async () => {
            throw new Error('backend down');
        });
        const cache = createMarkerCache({ fetch, ttlMs: 60_000, errorTtlMs: 5_000, now: () => now });
        const m = await cache.get('calendar', START, END);
        expect(m.status).toBe('error');
        expect(m.days.size).toBe(0);
        now = 6_000;
        await cache.get('calendar', START, END);
        expect(fetch).toHaveBeenCalledTimes(2);
    });

    it('caches a permission denial for the full TTL (it will not fix itself)', async () => {
        let now = 0;
        const fetch = vi.fn(async () => emptyMarkers('permission_denied'));
        const cache = createMarkerCache({ fetch, ttlMs: 60_000, errorTtlMs: 5_000, now: () => now });
        await cache.get('calendar', START, END);
        now = 30_000;
        await cache.get('calendar', START, END);
        expect(fetch).toHaveBeenCalledTimes(1);
    });

    it('bounds its size', async () => {
        const fetch = vi.fn(async () => answer());
        const cache = createMarkerCache({ fetch, maxEntries: 2 });
        await cache.get('a', START, END);
        await cache.get('b', START, END);
        await cache.get('c', START, END);
        expect(cache.peek('a', START, END)).toBeNull();
        expect(cache.peek('c', START, END)).not.toBeNull();
    });
});
