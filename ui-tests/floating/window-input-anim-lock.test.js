/**
 * Regression test for the "stuck large" floating window bug.
 *
 * Bug: animateInputResize() locks content-area + suggestions with inline
 * `flex:none; height:<px>; overflow:hidden` and restores them only in its
 * own cleanup(), which runs from inside its rAF loop. When a permission
 * confirmation dialog opened mid-animation, permissions.js → onShow called
 * suspendAutoResize(), which cancelled the rAF loop (so cleanup never ran)
 * and left the inline height lock orphaned.
 *
 * An empty `flex:none` element with an explicit inline height still reports
 * that height via scrollHeight, so _measureNaturalHeight stayed pinned at
 * the large value: the launcher froze large with no content. `>clear-ux`
 * (display:none) shrank it, but the next request re-showed the content area
 * and the stale inline height snapped it large again.
 *
 * Fix: expose the pending cleanup as `_inputAnimCleanup` and run it (a) from
 * suspendAutoResize before cancelling the frame, and (b) at the start of a
 * fresh animateInputResize (back-to-back wraps). These tests drive the real
 * methods and assert the inline locks are always released.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { WindowManager } from '../../ui/js/floating/window.js';

function setupDom() {
    const contentArea = document.createElement('div');
    contentArea.id = 'contentArea';
    document.body.appendChild(contentArea);

    const suggestions = document.createElement('div');
    suggestions.id = 'appSuggestions';
    document.body.appendChild(suggestions);

    const input = document.createElement('textarea');
    input.id = 'promptInput';
    document.body.appendChild(input);
    return { contentArea, suggestions, input };
}

describe('WindowManager input-animation lock lifecycle', () => {
    beforeEach(() => {
        document.body.innerHTML = '';
    });
    afterEach(() => {
        document.body.innerHTML = '';
    });

    it('applies the inline lock and registers a pending cleanup', () => {
        const { contentArea, input } = setupDom();
        const wm = new WindowManager(async () => {});

        // Fire-and-forget: the lock is applied synchronously before the Promise.
        wm.animateInputResize(input, 20, 40);

        expect(contentArea.style.flex).toBe('0 0 auto');
        expect(wm._inputAnimating).toBe(true);
        expect(typeof wm._inputAnimCleanup).toBe('function');
    });

    it('suspendAutoResize releases an orphaned lock (permission modal path)', () => {
        const { contentArea, suggestions, input } = setupDom();
        const wm = new WindowManager(async () => {});

        wm.animateInputResize(input, 20, 40);
        expect(contentArea.style.flex).toBe('0 0 auto');

        // Permission modal opens mid-animation.
        wm.suspendAutoResize();

        // Locks restored to their (empty) originals, not left frozen.
        expect(contentArea.style.flex).toBe('');
        expect(contentArea.style.height).toBe('');
        expect(suggestions.style.flex).toBe('');
        expect(wm._inputAnimating).toBe(false);
        expect(wm._inputAnimCleanup).toBeNull();
    });

    it('is idempotent — a second cleanup call is a no-op', () => {
        const { contentArea, input } = setupDom();
        const wm = new WindowManager(async () => {});

        wm.animateInputResize(input, 20, 40);
        const cleanup = wm._inputAnimCleanup;
        cleanup();
        // Simulate the aborted rAF step calling cleanup again later.
        cleanup();
        expect(contentArea.style.flex).toBe('');
        expect(wm._inputAnimating).toBe(false);
    });

    it('back-to-back animations do not leak the lock as the new "original"', () => {
        const { contentArea, input } = setupDom();
        const wm = new WindowManager(async () => {});

        wm.animateInputResize(input, 20, 40); // first, mid-flight
        wm.animateInputResize(input, 40, 60); // second interrupts the first

        // The second animation must have captured the UNLOCKED styles as its
        // originals; running its cleanup restores to empty, not to flex:none.
        wm._inputAnimCleanup();
        expect(contentArea.style.flex).toBe('');
        expect(contentArea.style.height).toBe('');
    });
});
