/**
 * Tests for ExtensionManager's widget refresh-budget circuit breaker.
 *
 * The breaker has four trip conditions:
 *   1. Re-entrant ticks (renderInFlight set when a new tick fires).
 *   2. Render takes >= WIDGET_SLOW_RENDER_MS (5s, absolute).
 *   3. Render takes >= interval * WIDGET_SLOW_RENDER_RATIO (0.7).
 *   4. Render throws (e.g. RPC timeout).
 *
 * After WIDGET_FAILURE_TRIP_THRESHOLD (3) consecutive failures the
 * breaker trips: timer cleared, "Widget paused" notice rendered,
 * controller.tripped = true. A successful render before the threshold
 * resets the counter.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

function makeController(overrides = {}) {
    const host = document.createElement('div');
    return {
        extensionId: 'test-ext',
        widgetId: 'w1',
        slot: 'top',
        host,
        timer: null,
        destroyed: false,
        refreshIntervalMs: 1_000,
        renderInFlight: false,
        consecutiveFailures: 0,
        tripped: false,
        ...overrides,
    };
}

function makeManagerWithSandbox({ renderImpl }) {
    const mgr = new ExtensionManager(async () => undefined);
    const sandbox = {
        widgetIds: ['w1'],
        async call(method, params) {
            if (method === 'renderWidget') return renderImpl(params);
            return null;
        },
    };
    mgr.extensions.set('test-ext', {
        manifest: { id: 'test-ext' },
        basePath: null,
        userInstalled: false,
        capabilities: [],
        sandbox,
    });
    return mgr;
}

describe('Widget budget cap', () => {
    beforeEach(() => {
        vi.restoreAllMocks();
    });

    it('successful render resets the failure counter', async () => {
        let n = 0;
        const mgr = makeManagerWithSandbox({
            renderImpl: () => {
                n++;
                if (n === 1 || n === 2) throw new Error('boom');
                return { html: '<div>ok</div>' };
            },
        });
        const ctrl = makeController();

        // Suppress the console.warn spam from the failed renders so the
        // test output stays focused on assertions.
        vi.spyOn(console, 'warn').mockImplementation(() => {});

        await mgr._renderWidget(ctrl);
        expect(ctrl.consecutiveFailures).toBe(1);

        await mgr._renderWidget(ctrl);
        expect(ctrl.consecutiveFailures).toBe(2);

        // Third call returns successfully — counter must reset, NOT trip.
        await mgr._renderWidget(ctrl);
        expect(ctrl.consecutiveFailures).toBe(0);
        expect(ctrl.tripped).toBe(false);
    });

    it('trips after WIDGET_FAILURE_TRIP_THRESHOLD consecutive throws', async () => {
        const mgr = makeManagerWithSandbox({
            renderImpl: () => {
                throw new Error('always fails');
            },
        });
        const ctrl = makeController();

        vi.spyOn(console, 'warn').mockImplementation(() => {});

        await mgr._renderWidget(ctrl);
        await mgr._renderWidget(ctrl);
        expect(ctrl.tripped).toBe(false);

        await mgr._renderWidget(ctrl);
        expect(ctrl.tripped).toBe(true);
        // Host should now contain a paused notice with a Retry link.
        expect(ctrl.host.querySelector('.ext-widget-paused')).toBeTruthy();
        expect(ctrl.host.querySelector('.ext-widget-paused a')).toBeTruthy();
    });

    it('skips re-entrant ticks and counts them as failures', async () => {
        // First call hangs forever (we resolve it manually after).
        let resolveFirst;
        let renderCount = 0;
        const mgr = makeManagerWithSandbox({
            renderImpl: () => {
                renderCount++;
                if (renderCount === 1) {
                    return new Promise((r) => {
                        resolveFirst = r;
                    });
                }
                return { html: '<div>ok</div>' };
            },
        });
        const ctrl = makeController();

        vi.spyOn(console, 'warn').mockImplementation(() => {});

        const p1 = mgr._renderWidget(ctrl);
        expect(ctrl.renderInFlight).toBe(true);

        // Second call while first is in-flight — must skip and increment
        // failures, not actually invoke renderWidget again.
        await mgr._renderWidget(ctrl);
        expect(renderCount).toBe(1); // still 1 — second call skipped
        expect(ctrl.consecutiveFailures).toBe(1);

        // Third skipped tick.
        await mgr._renderWidget(ctrl);
        expect(ctrl.consecutiveFailures).toBe(2);

        // Fourth skipped tick — trips the breaker.
        await mgr._renderWidget(ctrl);
        expect(ctrl.consecutiveFailures).toBe(3);
        expect(ctrl.tripped).toBe(true);

        // Resolve the originally-stuck render so the test can exit cleanly.
        resolveFirst({ html: '<div>late</div>' });
        await p1;
    });

    it('flags a slow_relative render when it eats >= 70% of the interval', async () => {
        // Mock performance.now() so we can simulate a 750ms render
        // against a 1000ms interval (75% of the budget). The mock has
        // to advance between the start-time read and the post-await
        // read inside _renderWidget.
        let now = 0;
        const perfSpy = vi.spyOn(performance, 'now').mockImplementation(() => now);

        const mgr = makeManagerWithSandbox({
            renderImpl: async () => {
                now = 750; // simulate elapsed time
                return { html: '<div>slow</div>' };
            },
        });
        const ctrl = makeController({ refreshIntervalMs: 1_000 });

        vi.spyOn(console, 'warn').mockImplementation(() => {});

        await mgr._renderWidget(ctrl);
        expect(ctrl.consecutiveFailures).toBe(1);

        // Reset elapsed for the second render — same outcome.
        now = 0;
        await mgr._renderWidget(ctrl);
        expect(ctrl.consecutiveFailures).toBe(2);

        now = 0;
        await mgr._renderWidget(ctrl);
        expect(ctrl.tripped).toBe(true);

        perfSpy.mockRestore();
    });

    it('flags slow_absolute regardless of declared interval', async () => {
        // 6-second render against an hourly interval — the relative
        // check forgives this (6s is far less than 70% of 1 hour) but
        // the absolute 5s ceiling should still trip the failure.
        let now = 0;
        const perfSpy = vi.spyOn(performance, 'now').mockImplementation(() => now);

        const mgr = makeManagerWithSandbox({
            renderImpl: async () => {
                now = 6_000;
                return { html: '<div>ok-but-slow</div>' };
            },
        });
        const ctrl = makeController({ refreshIntervalMs: 3_600_000 });

        vi.spyOn(console, 'warn').mockImplementation(() => {});

        await mgr._renderWidget(ctrl);
        expect(ctrl.consecutiveFailures).toBe(1);

        perfSpy.mockRestore();
    });

    it('retry resets state and re-renders', async () => {
        let renders = 0;
        const mgr = makeManagerWithSandbox({
            renderImpl: () => {
                renders++;
                if (renders <= 3) throw new Error('boom');
                return { html: '<div>recovered</div>' };
            },
        });
        const ctrl = makeController();

        vi.spyOn(console, 'warn').mockImplementation(() => {});

        // Trip the breaker.
        await mgr._renderWidget(ctrl);
        await mgr._renderWidget(ctrl);
        await mgr._renderWidget(ctrl);
        expect(ctrl.tripped).toBe(true);

        // Retry should reset counter + run one more render. That render
        // happens to succeed (the 4th call) and the host should contain
        // the recovered HTML, not the paused notice.
        await mgr._retryWidget(ctrl);
        // Wait one microtask in case _retryWidget resolves async.
        await Promise.resolve();
        expect(ctrl.tripped).toBe(false);
        expect(ctrl.consecutiveFailures).toBe(0);
        expect(ctrl.host.innerHTML).toContain('recovered');
    });

    it('does not render after destroyed', async () => {
        let calls = 0;
        const mgr = makeManagerWithSandbox({
            renderImpl: () => {
                calls++;
                return { html: '<div>x</div>' };
            },
        });
        const ctrl = makeController({ destroyed: true });
        await mgr._renderWidget(ctrl);
        expect(calls).toBe(0);
    });

    it('does not render after tripped', async () => {
        let calls = 0;
        const mgr = makeManagerWithSandbox({
            renderImpl: () => {
                calls++;
                return { html: '<div>x</div>' };
            },
        });
        const ctrl = makeController({ tripped: true });
        await mgr._renderWidget(ctrl);
        expect(calls).toBe(0);
    });

    it('skips render while floating window is hidden, without counting as failure', async () => {
        let calls = 0;
        const mgr = makeManagerWithSandbox({
            renderImpl: () => {
                calls++;
                return { html: '<div>x</div>' };
            },
        });
        const ctrl = makeController();

        window._kageFloatingHidden = true;
        try {
            await mgr._renderWidget(ctrl);
            expect(calls).toBe(0);
            // Crucially, this is NOT a failure — the breaker counter
            // must stay at 0 so a long hidden period doesn't leave us
            // one tick away from tripping when the user reopens the
            // window.
            expect(ctrl.consecutiveFailures).toBe(0);
            expect(ctrl.tripped).toBe(false);
        } finally {
            delete window._kageFloatingHidden;
        }
    });

    it('destroy() clears widget timers and flips destroyed', () => {
        const mgr = makeManagerWithSandbox({ renderImpl: () => ({ html: '<div>x</div>' }) });
        const ctrl = makeController();
        // Simulate a mounted controller with an active timer.
        ctrl.timer = setInterval(() => {}, 1_000);
        mgr._widgetInstances = new Map([['test-ext:w1', ctrl]]);

        mgr.destroy();

        expect(ctrl.destroyed).toBe(true);
        expect(ctrl.timer).toBe(null);
        expect(mgr._widgetInstances.size).toBe(0);
        expect(mgr.extensions.size).toBe(0);
    });
});
