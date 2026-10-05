/**
 * Regression tests for the "input + extension bar jump up, then snap back"
 * bug on multi-line prompts (Shift+Enter / wrapping, up to the 100px
 * textarea cap) while a response is showing.
 *
 * Root cause: the textarea height changed on a timer (an 80ms lockstep
 * animation) while the OS window resize — an IPC round trip plus a WebView2
 * viewport resize — landed one or more frames later. The animation released
 * its layout lock on the frame it sent the last resize, so the flex
 * `.content-area` absorbed the delta against the OLD viewport: the input and
 * the bars above it jumped up, then snapped back when the window landed.
 * When the window was pinned (screen ceiling / user-set height) the window
 * grew anyway and the observer immediately shrank it back.
 *
 * Fix: freeze the layout, request the final size once (from the same
 * `_targetHeight` the observer uses), and apply the new textarea height in
 * the viewport `resize` event — which fires before the first paint at the
 * new size. A pinned window gets no IPC: DOM-only, one paint.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WindowManager } from '../../ui/js/floating/window.js';

describe('WindowManager._clampInputResizeTarget', () => {
    const wm = new WindowManager(async () => {});

    it('returns the settled target exactly when moving with the input', () => {
        expect(wm._clampInputResizeTarget(500, 24, 524)).toBe(524);
        // Off-by-rounding from the free answer: use the target, so the
        // observer pass that follows has nothing to correct.
        expect(wm._clampInputResizeTarget(500, 24, 526)).toBe(526);
    });

    it('does not grow a window pinned at its target', () => {
        expect(wm._clampInputResizeTarget(900, 24, 900)).toBe(900);
    });

    it('never moves against the input direction', () => {
        expect(wm._clampInputResizeTarget(900, 24, 700)).toBe(900);
        expect(wm._clampInputResizeTarget(700, -24, 900)).toBe(700);
    });

    it('keeps a pinned window still when the input shrinks', () => {
        expect(wm._clampInputResizeTarget(900, -24, 900)).toBe(900);
    });
});

describe('WindowManager.animateInputResize', () => {
    let contentArea;
    let input;
    let viewportH;

    beforeEach(() => {
        document.body.innerHTML = '';
        contentArea = document.createElement('div');
        contentArea.id = 'contentArea';
        Object.defineProperty(contentArea, 'offsetHeight', { configurable: true, get: () => 400 });
        document.body.appendChild(contentArea);
        input = document.createElement('textarea');
        input.style.height = '24px';
        document.body.appendChild(input);

        viewportH = 900;
        vi.stubGlobal('devicePixelRatio', 1);
        Object.defineProperty(window, 'innerHeight', { configurable: true, get: () => viewportH });
    });

    afterEach(() => {
        vi.useRealTimers();
        vi.unstubAllGlobals();
        document.body.innerHTML = '';
    });

    it('pinned window: DOM-only change in one step, no resize IPC, no lock', async () => {
        const invoke = vi.fn(async () => {});
        const wm = new WindowManager(invoke);
        wm._maxPhys = 900; // screen ceiling == current window height
        wm._measureNaturalHeight = () => 1400; // response far taller than the cap

        await wm.animateInputResize(input, 24, 48);

        expect(invoke).not.toHaveBeenCalled();
        expect(input.style.height).toBe('48px');
        expect(contentArea.style.flex).toBe('');
        expect(wm._inputAnimating).toBeFalsy();
        expect(wm._lastTarget).toBe(900);
    });

    it('free window: holds the layout until the viewport reaches the target', async () => {
        const invoke = vi.fn(async () => {});
        const wm = new WindowManager(invoke);
        wm._maxPhys = 1200;
        wm._measureNaturalHeight = () => 900; // auto-fit: window == natural

        await wm.animateInputResize(input, 24, 48);

        // One request, straight to the settled size.
        expect(invoke).toHaveBeenCalledTimes(1);
        expect(invoke).toHaveBeenCalledWith('resize_floating_window', { height: 924 });
        // Nothing has changed yet: the viewport is still the old size.
        expect(input.style.height).toBe('24px');
        expect(contentArea.style.flex).toBe('0 0 auto');
        expect(wm._inputAnimating).toBe(true);

        // A resize to some other size (e.g. a stray intermediate) is ignored.
        viewportH = 910;
        window.dispatchEvent(new Event('resize'));
        expect(input.style.height).toBe('24px');

        // The viewport lands: apply in the resize event, before paint.
        viewportH = 924;
        window.dispatchEvent(new Event('resize'));
        expect(input.style.height).toBe('48px');
        expect(contentArea.style.flex).toBe('');
        expect(contentArea.style.height).toBe('');
        expect(wm._inputAnimating).toBe(false);
        expect(wm._lastTarget).toBe(924);
    });

    it('falls back to applying if the window never reaches the target', async () => {
        vi.useFakeTimers();
        const wm = new WindowManager(vi.fn(async () => {}));
        wm._maxPhys = 1200;
        wm._measureNaturalHeight = () => 900;

        await wm.animateInputResize(input, 24, 48);
        expect(input.style.height).toBe('24px');

        vi.advanceTimersByTime(300);
        expect(input.style.height).toBe('48px');
        expect(contentArea.style.flex).toBe('');
        expect(wm._inputAnimating).toBe(false);
    });

    it('applies immediately if the resize IPC fails', async () => {
        const wm = new WindowManager(
            vi.fn(async () => {
                throw new Error('nope');
            })
        );
        wm._maxPhys = 1200;
        wm._measureNaturalHeight = () => 900;

        await wm.animateInputResize(input, 24, 48);
        expect(input.style.height).toBe('48px');
        expect(contentArea.style.flex).toBe('');
    });

    it('a second line while waiting applies the first before starting', async () => {
        const invoke = vi.fn(async () => {});
        const wm = new WindowManager(invoke);
        wm._maxPhys = 1200;
        wm._measureNaturalHeight = () => 900;

        await wm.animateInputResize(input, 24, 48);
        await wm.animateInputResize(input, 48, 72);

        // First was applied (lock released, then re-taken by the second with
        // unlocked originals); second waits for its own viewport.
        expect(invoke).toHaveBeenCalledTimes(2);
        viewportH = 924; // second measured from 900 → +24
        window.dispatchEvent(new Event('resize'));
        expect(input.style.height).toBe('72px');
        expect(contentArea.style.flex).toBe('');
        expect(contentArea.style.height).toBe('');
    });
});
