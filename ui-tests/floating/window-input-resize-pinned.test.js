/**
 * Regression test for the "input + extension bar jump up, then snap back"
 * bug on multi-line prompts when the floating window is already large.
 *
 * Bug: animateInputResize() always grew the OS window by the textarea's
 * growth. When the window was pinned — at the screen ceiling, or at a
 * height the user dragged it to — the next observer pass shrank it straight
 * back to the cap, so every wrap/Shift+Enter (up to the 100px textarea cap,
 * i.e. the first ~4 lines) bounced the input and bars, flashing the
 * transparent window background where they'd been.
 *
 * Fix: resolve the settled window target up front (same `_targetHeight` the
 * observer uses), move the OS window only as far as that allows, and have
 * the content-area give up the rest on the same curve.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WindowManager } from '../../ui/js/floating/window.js';

describe('WindowManager._clampInputResizeTarget', () => {
    const wm = new WindowManager(async () => {});

    it('grows freely when the target has room', () => {
        expect(wm._clampInputResizeTarget(500, 24, 524)).toBe(524);
        expect(wm._clampInputResizeTarget(500, 24, 900)).toBe(524);
    });

    it('does not grow a window pinned at its target', () => {
        expect(wm._clampInputResizeTarget(900, 24, 900)).toBe(900);
    });

    it('grows only up to the target when partly pinned', () => {
        expect(wm._clampInputResizeTarget(890, 24, 900)).toBe(900);
    });

    it('never moves against the input direction', () => {
        // Target below the current height must not shrink the window while
        // the input grows (that's the observer's job, not the keystroke's).
        expect(wm._clampInputResizeTarget(900, 24, 700)).toBe(900);
        expect(wm._clampInputResizeTarget(700, -24, 900)).toBe(700);
    });

    it('keeps a pinned window still when the input shrinks', () => {
        // Content is still taller than the cap after the line is removed.
        expect(wm._clampInputResizeTarget(900, -24, 900)).toBe(900);
    });

    it('snaps sub-pixel measurement noise to the free answer', () => {
        expect(wm._clampInputResizeTarget(500, 24, 522)).toBe(524);
    });
});

describe('WindowManager.animateInputResize when pinned', () => {
    let contentArea;
    let input;
    let rafCallbacks;

    beforeEach(() => {
        document.body.innerHTML = '';
        contentArea = document.createElement('div');
        contentArea.id = 'contentArea';
        Object.defineProperty(contentArea, 'offsetHeight', { configurable: true, get: () => 400 });
        document.body.appendChild(contentArea);
        input = document.createElement('textarea');
        document.body.appendChild(input);

        rafCallbacks = [];
        vi.stubGlobal('requestAnimationFrame', (cb) => {
            rafCallbacks.push(cb);
            return rafCallbacks.length;
        });
        vi.stubGlobal('devicePixelRatio', 1);
        vi.stubGlobal('innerHeight', 900);
    });

    afterEach(() => {
        vi.unstubAllGlobals();
        document.body.innerHTML = '';
    });

    function runToEnd(wm) {
        const start = performance.now();
        // Drive frames past the 80ms duration.
        while (rafCallbacks.length) {
            const cb = rafCallbacks.shift();
            cb(start + 1000);
        }
        return wm;
    }

    it('shrinks the content-area instead of resizing the OS window', async () => {
        const invoke = vi.fn(async () => {});
        const wm = new WindowManager(invoke);
        wm._maxPhys = 900; // screen ceiling == current window height
        wm._measureNaturalHeight = () => 1400; // response far taller than the cap

        const done = wm.animateInputResize(input, 24, 48);
        // Mid-flight the lock is on, and the content-area is giving up space.
        expect(contentArea.style.flex).toBe('0 0 auto');
        runToEnd(wm);
        await done;

        expect(invoke).not.toHaveBeenCalledWith('resize_floating_window', expect.anything());
        expect(input.style.height).toBe('48px');
        // Lock released at the end — flex layout takes over at the new size.
        expect(contentArea.style.height).toBe('');
        expect(wm._lastTarget).toBe(900);
    });

    it('still grows the OS window when there is room', async () => {
        const invoke = vi.fn(async () => {});
        const wm = new WindowManager(invoke);
        wm._maxPhys = 1200;
        wm._measureNaturalHeight = () => 900; // auto-fit: window == natural

        const done = wm.animateInputResize(input, 24, 48);
        runToEnd(wm);
        await done;

        expect(invoke).toHaveBeenLastCalledWith('resize_floating_window', { height: 924 });
        expect(wm._lastTarget).toBe(924);
    });
});
