/**
 * Mascot showcase — drives a `createMascotController` instance through a
 * greeting wave and then a random tour of its cheerful moments, with idle
 * pauses in between (where the engine's own glances and winks play).
 *
 * Used by the first-run welcome screen, which has no app events of its own to
 * react to. The thinking hop is deliberately left out — it means "working",
 * which the launch overlay already says — as are the unhappy situations
 * (error, offline, rate limit): a welcome shouldn't open with a sulk.
 */

/**
 * Each moment is a list of `[signal, msUntilNextStep]`. The last step's delay
 * is how long the moment needs to finish before the pause starts.
 */
export const SHOWCASE_MOMENTS = Object.freeze({
    poke: [['poke', 3200]],
    wink: [['copy', 1600]],
    search: [
        ['tool:search', 4200],
        ['done', 3400],
    ],
    build: [
        ['tool:edit', 5200],
        ['done', 3400],
    ],
    read: [
        ['tool:read', 5600],
        ['done', 3400],
    ],
    music: [
        ['hint:music', 6400],
        ['hint:none', 900],
    ],
    timer: [
        ['hint:timer', 5200],
        ['hint:none', 900],
    ],
    compact: [
        ['compact', 3800],
        ['compactDone', 1600],
    ],
    // The party hat stays on after the balloons; `reopen` takes it off.
    party: [
        ['updated', 6200],
        ['reopen', 1400],
    ],
});

/**
 * Start the tour. Returns `stop()`, which cancels any pending step.
 *
 * Moments are drawn from a shuffled bag so every one plays before any
 * repeats, and a refilled bag never starts with the moment that just played.
 *
 * @param {{signal: (name: string) => void}} controller
 * @param {object} [opts]
 * @param {() => number} [opts.random] - injectable for tests
 * @param {number} [opts.pauseMin] - shortest idle gap between moments (ms)
 * @param {number} [opts.pauseMax] - longest idle gap between moments (ms)
 * @param {number} [opts.greetDelay] - wait before the opening wave (ms)
 * @param {typeof setTimeout} [opts.setTimer]
 * @param {typeof clearTimeout} [opts.clearTimer]
 */
export function startMascotShowcase(controller, opts = {}) {
    const {
        random = Math.random,
        pauseMin = 2500,
        pauseMax = 5500,
        greetDelay = 400,
        setTimer = setTimeout,
        clearTimer = clearTimeout,
    } = opts;
    const names = Object.keys(SHOWCASE_MOMENTS);
    let bag = [];
    let last = null;
    let timer = null;
    let stopped = false;

    const send = (name) => {
        try {
            controller.signal(name);
        } catch (e) {
            console.warn('[mascot-showcase] signal failed:', name, e);
        }
    };
    const later = (ms, fn) => {
        if (stopped) return;
        timer = setTimer(fn, ms);
    };

    const draw = () => {
        if (bag.length === 0) {
            bag = [...names];
            for (let i = bag.length - 1; i > 0; i--) {
                const j = Math.floor(random() * (i + 1));
                [bag[i], bag[j]] = [bag[j], bag[i]];
            }
            if (bag.length > 1 && bag[bag.length - 1] === last) {
                [bag[0], bag[bag.length - 1]] = [bag[bag.length - 1], bag[0]];
            }
        }
        last = bag.pop();
        return last;
    };

    const pause = () => pauseMin + random() * Math.max(0, pauseMax - pauseMin);

    const playMoment = (steps, i = 0) => {
        if (stopped) return;
        if (i >= steps.length) {
            later(pause(), () => playMoment(SHOWCASE_MOMENTS[draw()]));
            return;
        }
        const [name, wait] = steps[i];
        send(name);
        later(wait, () => playMoment(steps, i + 1));
    };

    later(greetDelay, () => playMoment([['wave', 2200]]));

    return function stop() {
        stopped = true;
        if (timer !== null) clearTimer(timer);
        timer = null;
    };
}
