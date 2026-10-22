/**
 * Regression test for the floating window after a display-scale change
 * (e.g. undocking a laptop): a hidden window moved to a higher-scale
 * monitor kept its physical width, so the viewport fell below the layout's
 * min-width and the page grew scrollbars with the right edge clipped.
 * `ensureMinWidth` restores the width whenever the viewport is too narrow.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { WindowManager } from '../../ui/js/floating/window.js';
import { MIN_WIDTH_LOGICAL } from '../../ui/js/floating/window-sizing-config.js';

function viewport(width, dpr) {
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: width });
    Object.defineProperty(window, 'devicePixelRatio', { configurable: true, value: dpr });
}

describe('WindowManager.ensureMinWidth', () => {
    afterEach(() => viewport(1024, 1));

    it('widens a too-narrow viewport to the minimum at the current scale', async () => {
        const invoke = vi.fn(async () => {});
        const wm = new WindowManager(invoke);
        wm.resizeWindow = vi.fn();
        viewport(380, 1.5); // 570 physical px on a 150% screen
        await wm.ensureMinWidth();
        expect(invoke).toHaveBeenCalledWith('resize_floating_window', {
            width: Math.ceil(MIN_WIDTH_LOGICAL * 1.5),
        });
        expect(wm.resizeWindow).toHaveBeenCalled();
    });

    it('leaves a wide-enough viewport, and a user drag, alone', async () => {
        const invoke = vi.fn(async () => {});
        const wm = new WindowManager(invoke);
        viewport(MIN_WIDTH_LOGICAL, 1.25);
        await wm.ensureMinWidth();
        viewport(300, 1);
        wm.isResizing = true;
        await wm.ensureMinWidth();
        expect(invoke).not.toHaveBeenCalled();
    });
});
