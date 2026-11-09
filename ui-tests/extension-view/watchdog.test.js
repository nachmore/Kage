/**
 * The view page's watchdog (docs/design/EXTENSION_VIEWS.md §8.7): a ping
 * every 5 s while visible, the banner after 3 missed answers, any answer
 * clears it, and it never kills the view.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    createWatchdog,
    MISSES_BEFORE_BANNER,
    PING_INTERVAL_MS,
} from '../../ui/js/extension-view/watchdog.js';

/** A ping whose answers the test hands out. */
function controlledPing() {
    const pending = [];
    const ping = vi.fn(
        () =>
            new Promise((resolve, reject) => {
                pending.push({ resolve, reject });
            })
    );
    return {
        ping,
        answer: () => pending.shift()?.resolve('pong'),
        fail: () => pending.shift()?.reject(new Error('rpc error')),
        get outstanding() {
            return pending.length;
        },
    };
}

describe('watchdog', () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it('pings every 5 s and shows the banner after 3 misses (15 s)', async () => {
        expect(PING_INTERVAL_MS).toBe(5_000);
        expect(MISSES_BEFORE_BANNER).toBe(3);
        const p = controlledPing();
        const onUnresponsive = vi.fn();
        const wd = createWatchdog({ ping: p.ping, onUnresponsive });
        wd.start();
        expect(p.ping).toHaveBeenCalledTimes(1); // the first ping goes at once

        await vi.advanceTimersByTimeAsync(5_000);
        expect(wd.misses).toBe(1);
        expect(wd.lastPingFailed).toBe(true);
        await vi.advanceTimersByTimeAsync(5_000);
        expect(onUnresponsive).not.toHaveBeenCalled();
        await vi.advanceTimersByTimeAsync(5_000);
        expect(onUnresponsive).toHaveBeenCalledTimes(1);
        expect(wd.unresponsive).toBe(true);
        // A frozen sandbox doesn't get a pile of pings: one stays in flight.
        expect(p.ping).toHaveBeenCalledTimes(1);

        await vi.advanceTimersByTimeAsync(20_000);
        expect(onUnresponsive).toHaveBeenCalledTimes(1);
        wd.stop();
    });

    it('an answer clears the banner and the miss count', async () => {
        const p = controlledPing();
        const onUnresponsive = vi.fn();
        const onRecovered = vi.fn();
        const wd = createWatchdog({ ping: p.ping, onUnresponsive, onRecovered });
        wd.start();
        await vi.advanceTimersByTimeAsync(15_000);
        expect(onUnresponsive).toHaveBeenCalledTimes(1);

        p.answer();
        await vi.advanceTimersByTimeAsync(0);
        expect(onRecovered).toHaveBeenCalledTimes(1);
        expect(wd.unresponsive).toBe(false);
        expect(wd.misses).toBe(0);
        expect(wd.lastPingFailed).toBe(false);

        // Pinging resumes on the next tick.
        await vi.advanceTimersByTimeAsync(5_000);
        expect(p.ping).toHaveBeenCalledTimes(2);
        wd.stop();
    });

    it('a timely answer is never a miss', async () => {
        const p = controlledPing();
        const onUnresponsive = vi.fn();
        const wd = createWatchdog({ ping: p.ping, onUnresponsive });
        wd.start();
        for (let i = 0; i < 6; i++) {
            p.answer();
            await vi.advanceTimersByTimeAsync(5_000);
            expect(wd.misses).toBe(0);
        }
        expect(onUnresponsive).not.toHaveBeenCalled();
        wd.stop();
    });

    it('an error reply counts as an answer: the sandbox is running', async () => {
        const p = controlledPing();
        const wd = createWatchdog({ ping: p.ping });
        wd.start();
        p.fail();
        await vi.advanceTimersByTimeAsync(5_000);
        expect(wd.misses).toBe(0);
        wd.stop();
    });

    it('a ping that throws at once counts as an answer too', async () => {
        const wd = createWatchdog({
            ping: () => {
                throw new Error('not ready');
            },
        });
        wd.start();
        await vi.advanceTimersByTimeAsync(5_000);
        expect(wd.misses).toBe(0);
        wd.stop();
    });

    it('stop() stops counting; lastPingFailed survives for the next open', async () => {
        const p = controlledPing();
        const onUnresponsive = vi.fn();
        const wd = createWatchdog({ ping: p.ping, onUnresponsive });
        wd.start();
        await vi.advanceTimersByTimeAsync(5_000);
        wd.stop();
        expect(wd.running).toBe(false);
        await vi.advanceTimersByTimeAsync(60_000);
        expect(wd.misses).toBe(1);
        expect(wd.lastPingFailed).toBe(true);
        expect(onUnresponsive).not.toHaveBeenCalled();
    });

    it('start() is a no-op while running and keeps the ping in flight', async () => {
        const p = controlledPing();
        const wd = createWatchdog({ ping: p.ping });
        wd.start();
        wd.start();
        expect(p.ping).toHaveBeenCalledTimes(1);
        wd.stop();
        wd.start(); // the first ping is still unanswered: don't send another
        expect(p.ping).toHaveBeenCalledTimes(1);
        wd.stop();
    });

    it('reset() forgets everything, and a late answer for the old sandbox is ignored', async () => {
        const p = controlledPing();
        const onRecovered = vi.fn();
        const wd = createWatchdog({ ping: p.ping, onRecovered });
        wd.start();
        await vi.advanceTimersByTimeAsync(15_000);
        expect(wd.unresponsive).toBe(true);
        wd.reset();
        expect(wd.running).toBe(false);
        expect(wd.unresponsive).toBe(false);
        expect(wd.misses).toBe(0);

        wd.start(); // new sandbox: a fresh ping
        expect(p.ping).toHaveBeenCalledTimes(2);
        p.answer(); // the old sandbox's answer
        await vi.advanceTimersByTimeAsync(0);
        expect(onRecovered).not.toHaveBeenCalled();
        await vi.advanceTimersByTimeAsync(5_000);
        expect(wd.misses).toBe(1); // the new ping is still unanswered
        wd.stop();
    });
});
