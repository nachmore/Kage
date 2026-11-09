/**
 * The view page's watchdog (docs/design/EXTENSION_VIEWS.md §8.7).
 *
 * While the window is visible the page pings the view sandbox every 5 s.
 * A ping that is still unanswered when the next tick comes is a miss.
 * After 3 misses in a row (15 s) the page shows the "isn't responding"
 * banner. Any answer clears it. The watchdog never kills the view: the
 * user decides, with Reload or Close.
 *
 * Pure apart from the timers, so it can be tested with fake timers.
 */

/** How often a visible view is pinged. */
export const PING_INTERVAL_MS = 5_000;

/** Missed answers in a row before the banner shows. */
export const MISSES_BEFORE_BANNER = 3;

/**
 * @param {object} opts
 * @param {() => Promise<unknown>} opts.ping - sends one `viewPing`. It must
 *   not time out on its own: the watchdog counts the wait. Any settlement
 *   counts as an answer, because a reply of any kind (even an error) means
 *   the sandbox is running.
 * @param {() => void} [opts.onUnresponsive] - the third miss in a row.
 * @param {() => void} [opts.onRecovered] - an answer after `onUnresponsive`.
 * @param {number} [opts.intervalMs]
 * @param {number} [opts.missesBeforeBanner]
 */
export function createWatchdog({
    ping,
    onUnresponsive = () => {},
    onRecovered = () => {},
    intervalMs = PING_INTERVAL_MS,
    missesBeforeBanner = MISSES_BEFORE_BANNER,
}) {
    let timer = null;
    let inFlight = false;
    let misses = 0;
    let unresponsive = false;
    // Bumped by reset(), so an answer for a sandbox that has since been
    // replaced can't clear the new sandbox's state.
    let generation = 0;

    function answered(gen) {
        if (gen !== generation) return;
        inFlight = false;
        misses = 0;
        if (unresponsive) {
            unresponsive = false;
            onRecovered();
        }
    }

    function send() {
        inFlight = true;
        const gen = generation;
        let pending;
        try {
            pending = Promise.resolve(ping());
        } catch (e) {
            pending = Promise.reject(e);
        }
        pending.then(
            () => answered(gen),
            () => answered(gen)
        );
    }

    function stop() {
        if (timer === null) return;
        clearInterval(timer);
        timer = null;
    }

    function tick() {
        if (!inFlight) {
            send();
            return;
        }
        misses += 1;
        if (misses >= missesBeforeBanner && !unresponsive) {
            unresponsive = true;
            onUnresponsive();
        }
    }

    return {
        /** Start pinging (no-op while running). The first ping goes at once. */
        start() {
            if (timer !== null) return;
            timer = setInterval(tick, intervalMs);
            if (!inFlight) send();
        },

        /** Stop pinging. A ping already sent can still be answered. */
        stop,

        /** Stop and forget everything, for a new sandbox. */
        reset() {
            stop();
            generation += 1;
            inFlight = false;
            misses = 0;
            unresponsive = false;
        },

        get running() {
            return timer !== null;
        },

        /** Missed answers in a row so far. */
        get misses() {
            return misses;
        },

        /** The banner condition holds (3 misses, no answer since). */
        get unresponsive() {
            return unresponsive;
        },

        /**
         * The last ping went unanswered for at least one interval. The page
         * reloads the sandbox when a window in this state is opened again,
         * because a frozen sandbox can't recover by itself.
         */
        get lastPingFailed() {
            return misses > 0;
        },
    };
}
