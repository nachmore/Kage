/**
 * Tests for the sandbox heartbeat that logs frozen-sandbox stalls
 * (createStallWatch in ui/js/extension-sandbox/runtime.js).
 */
import { describe, it, expect } from 'vitest';
import { createStallWatch } from '../../ui/js/extension-sandbox/runtime.js';

function harness(opts = {}) {
    const state = { t: 0, visible: true, reports: [] };
    const watch = createStallWatch({
        now: () => state.t,
        isVisible: () => state.visible,
        report: (late) => state.reports.push(late),
        periodMs: 2000,
        thresholdMs: 3000,
        cooldownMs: 300_000,
        ...opts,
    });
    const beatAt = (t, visible = state.visible) => {
        state.t = t;
        state.visible = visible;
        watch.beat();
    };
    return { state, beatAt };
}

describe('createStallWatch', () => {
    it('stays quiet when beats are on time or mildly late', () => {
        const { state, beatAt } = harness();
        beatAt(2000);
        beatAt(4100);
        beatAt(8500); // 2400ms late — under the 3000ms threshold
        expect(state.reports).toEqual([]);
    });

    it('reports a visible stall with how late the beat was', () => {
        // The incident shape: a timer frozen ~10s while the window was visible.
        const { state, beatAt } = harness();
        beatAt(2000);
        beatAt(14_000);
        expect(state.reports).toEqual([10_000]);
    });

    it('ignores lateness that spans a hidden window (normal throttling)', () => {
        const { state, beatAt } = harness();
        beatAt(2000, false);
        beatAt(60_000, true); // throttled while hidden, now visible again
        beatAt(80_000, false);
        expect(state.reports).toEqual([]);
    });

    it('logs a long stall once per cooldown, not every beat', () => {
        const { state, beatAt } = harness();
        beatAt(10_000);
        beatAt(20_000);
        beatAt(30_000);
        expect(state.reports).toHaveLength(1);
        beatAt(400_000);
        expect(state.reports).toHaveLength(2);
    });
});
