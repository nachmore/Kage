/**
 * Regression tests for the floating-window manual-resize floor.
 *
 * Bug: the resize handle's minimum height was computed from only the input
 * box (+ extension bars), ignoring the response content area. With a response
 * showing, the user could drag the window down until only the first line of
 * the response remained; worse, the next reflow snapped it back up to the full
 * natural height (because `_applyNaturalHeight` grows past `userSetHeight` when
 * content needs more room). The floor must include the full natural content
 * height — capped at the screen ceiling so a scrolling response can still be
 * shrunk.
 *
 * `_resizeFloor` is the extracted pure arithmetic; the surrounding DOM-driven
 * handler can't run under jsdom (no real layout → offsetHeight is 0).
 */

import { describe, it, expect } from 'vitest';
import { WindowManager } from '../../ui/js/floating/window.js';

const DEFAULT_HEIGHT = 76; // keep in sync with window.js

function mgr() {
    return new WindowManager(async () => {});
}

describe('WindowManager._resizeFloor', () => {
    it('floors at the natural content height (response cannot be clipped)', () => {
        // A 400px-tall response + input at scale 1 must not be shrinkable
        // below 400px.
        const floor = mgr()._resizeFloor(400, 2000, 1);
        expect(floor).toBe(400);
    });

    it('never goes below the collapsed launcher height', () => {
        // Tiny natural height (just the launcher, no response) → floor is the
        // launcher default, not something smaller.
        const floor = mgr()._resizeFloor(40, 2000, 1);
        expect(floor).toBe(DEFAULT_HEIGHT);
    });

    it('caps the floor at the screen ceiling for tall (scrolling) responses', () => {
        // A response taller than the screen already scrolls inside
        // content-area, so the user must be able to shrink down to the cap.
        const floor = mgr()._resizeFloor(5000, 900, 1);
        expect(floor).toBe(900);
    });

    it('scales the content floor by the device pixel ratio', () => {
        // 300 logical px at 2x DPI → 600 physical px floor.
        const floor = mgr()._resizeFloor(300, 4000, 2);
        expect(floor).toBe(600);
    });

    it('applies the launcher minimum in physical px under DPI scaling', () => {
        // No response: floor is DEFAULT_HEIGHT * scale.
        const floor = mgr()._resizeFloor(40, 4000, 2);
        expect(floor).toBe(DEFAULT_HEIGHT * 2);
    });
});

describe('WindowManager._suggestionCap', () => {
    // Regression: the suggestions cap used to be derived from the list's
    // ALREADY-CAPPED offsetHeight and cleared whenever the (now shorter)
    // layout fit under the ceiling. With a tall response + suggestions
    // open (type `focus` right after a focus AI summary), the observer
    // loop alternated cap → fits → clear → overflows → cap …, visibly
    // jittering the window height forever. The cap must be a pure
    // function of the UN-capped layout so identical inputs re-derive an
    // identical cap and the loop settles.

    it('subtracts the overflow from the uncapped height', () => {
        expect(mgr()._suggestionCap(300, 100)).toBe(200);
    });

    it('is idempotent for the same uncapped inputs (no oscillation)', () => {
        const m = mgr();
        const first = m._suggestionCap(300, 120);
        const second = m._suggestionCap(300, 120);
        expect(second).toBe(first);
    });

    it('returns null when the capped list would be unusably small', () => {
        expect(mgr()._suggestionCap(120, 90)).toBe(null); // 30px < 40px floor
    });

    it('returns null at exactly the 40px threshold', () => {
        expect(mgr()._suggestionCap(140, 100)).toBe(null); // 40 is not > 40
    });
});
