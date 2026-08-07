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
    // Pure step: scale a logical-px floor to physical px, never below the
    // collapsed launcher height. The logical floor itself (input + bars,
    // plus one response line when a response shows) is measured from the DOM
    // in _measureResizeFloorLogical, which jsdom can't exercise.

    it('scales the logical floor by the device pixel ratio', () => {
        expect(mgr()._resizeFloor(300, 1)).toBe(300);
        expect(mgr()._resizeFloor(300, 2)).toBe(600);
    });

    it('never goes below the collapsed launcher height', () => {
        // A tiny logical floor (e.g. bubble not laid out yet) → launcher min.
        expect(mgr()._resizeFloor(40, 1)).toBe(DEFAULT_HEIGHT);
    });

    it('applies the launcher minimum in physical px under DPI scaling', () => {
        expect(mgr()._resizeFloor(40, 2)).toBe(DEFAULT_HEIGHT * 2);
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

describe('WindowManager._targetHeight', () => {
    // No manual size: auto-fit content, floored at the launcher minimum and
    // capped at the ceiling. A long answer (8000px) caps at maxPhys — past it
    // the response scrolls inside content-area. (Regression: the pre-cap code
    // grew unbounded to 8000px+ once manually resized.)

    it('caps content-driven growth at the ceiling (no user size)', () => {
        const m = mgr();
        m.userSetHeight = null;
        expect(m._targetHeight(8000, 76, 900)).toBe(900);
    });

    it('floors at the collapsed launcher minimum with no user size', () => {
        const m = mgr();
        m.userSetHeight = null;
        expect(m._targetHeight(40, 76, 900)).toBe(76);
    });

    it('fits content between the floor and ceiling with no user size', () => {
        const m = mgr();
        m.userSetHeight = null;
        expect(m._targetHeight(500, 76, 900)).toBe(500);
    });

    // Manual size set: honour it as the budget for everything except the
    // dropdown, regardless of response content. The window is a fixed size the
    // user chose; content scrolls within it. We must NOT grow to fit content —
    // that snap-to-content was what jumped the window back to full-screen the
    // instant the user tried to shrink it below a long answer.

    it('honours a manual size exactly when content is taller', () => {
        const m = mgr();
        m.userSetHeight = 400; // user shrank to 400px…
        expect(m._targetHeight(8000, 76, 900)).toBe(400); // …despite an 8000px response
    });

    it('honours a manual size exactly when content is shorter', () => {
        const m = mgr();
        m.userSetHeight = 600;
        expect(m._targetHeight(300, 76, 900)).toBe(600);
    });

    // Dropdown is transient typeahead: it expands the window ON TOP OF the
    // user's budget and collapses back, rather than eating into the size they
    // chose for the response.

    it('adds the dropdown height on top of a manual size', () => {
        const m = mgr();
        m.userSetHeight = 400;
        expect(m._targetHeight(8000, 76, 900, 150)).toBe(550); // 400 + 150 dropdown
    });

    it('caps budget + dropdown at the ceiling', () => {
        const m = mgr();
        m.userSetHeight = 850;
        expect(m._targetHeight(300, 76, 900, 200)).toBe(900); // 850 + 200 → capped
    });

    it('collapses back to the budget when the dropdown closes', () => {
        const m = mgr();
        m.userSetHeight = 400;
        expect(m._targetHeight(300, 76, 900, 0)).toBe(400);
    });

    it('ignores the dropdown height when there is no manual size', () => {
        const m = mgr();
        m.userSetHeight = null;
        // Auto-fit already measures the dropdown in naturalPhys, so the
        // suggestionsPhys arg is only for the userSetHeight branch.
        expect(m._targetHeight(500, 76, 900, 150)).toBe(500);
    });
});
