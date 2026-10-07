/**
 * Tests for the welcome-screen mascot tour (ui/js/shared/mascot-showcase.js).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { SHOWCASE_MOMENTS, startMascotShowcase } from '../../ui/js/shared/mascot-showcase.js';

const firstSignals = Object.fromEntries(
    Object.entries(SHOWCASE_MOMENTS).map(([name, steps]) => [steps[0][0], name])
);

describe('startMascotShowcase', () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    const run = (ms, opts = {}) => {
        const signals = [];
        const stop = startMascotShowcase({ signal: (n) => signals.push(n) }, opts);
        vi.advanceTimersByTime(ms);
        return { signals, stop };
    };

    it('opens with a wave', () => {
        const { signals } = run(500);
        expect(signals).toEqual(['wave']);
    });

    it('plays every moment once before any repeats, never the thinking hop', () => {
        const { signals } = run(10 * 60_000);
        const starts = signals.filter((s) => s in firstSignals).map((s) => firstSignals[s]);
        const n = Object.keys(SHOWCASE_MOMENTS).length;
        expect(starts.length).toBeGreaterThan(n * 2);
        for (let k = 0; k + n <= starts.length; k += n) {
            expect(new Set(starts.slice(k, k + n)).size).toBe(n);
        }
        // No back-to-back repeat across a bag refill.
        for (let i = 1; i < starts.length; i++) expect(starts[i]).not.toBe(starts[i - 1]);
        expect(signals).not.toContain('think');
        for (const sad of ['error', 'offline', 'rateLimit']) expect(signals).not.toContain(sad);
    });

    it('closes every moment it opens', () => {
        const { signals } = run(10 * 60_000);
        const count = (n) => signals.filter((s) => s === n).length;
        // Within one of each other: the tour may be stopped mid-moment.
        expect(Math.abs(count('updated') - count('reopen'))).toBeLessThanOrEqual(1);
        expect(Math.abs(count('compact') - count('compactDone'))).toBeLessThanOrEqual(1);
        const hints = signals.filter((s) => s.startsWith('hint:'));
        for (let i = 0; i < hints.length; i += 2) {
            expect(hints[i]).not.toBe('hint:none');
            if (i + 1 < hints.length) expect(hints[i + 1]).toBe('hint:none');
        }
    });

    it('pauses between moments within the configured range', () => {
        const { signals } = run(2500 + 400, { random: () => 0, pauseMin: 3000, pauseMax: 3000 });
        // Wave at 400ms, wave lasts 2200ms, then a 3000ms pause: nothing else yet.
        expect(signals).toEqual(['wave']);
    });

    it('stop() cancels the tour', () => {
        const { signals, stop } = run(500);
        stop();
        vi.advanceTimersByTime(60_000);
        expect(signals).toEqual(['wave']);
    });

    it('survives a controller that throws', () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        let calls = 0;
        startMascotShowcase({
            signal: () => {
                calls++;
                throw new Error('boom');
            },
        });
        vi.advanceTimersByTime(60_000);
        expect(calls).toBeGreaterThan(3);
        warn.mockRestore();
    });
});
