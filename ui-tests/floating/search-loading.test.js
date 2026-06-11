/**
 * Tests for the floating search "loading more…" hint lifecycle
 * (_requestSearchLoading / _hideSearchLoading on FloatingApp).
 *
 * The behaviour these pin, which fixed the jarring in/out flicker:
 *   - a fast search (hidden before the delay gate fires) NEVER shows the
 *     hint — the timer is cancelled, nothing is inserted;
 *   - a slow search (still running past the gate) DOES show it;
 *   - hiding a visible hint fades it out (adds the -out class, removes
 *     after the animation) rather than yanking it instantly;
 *   - a hint armed by an older search generation can't appear over a
 *     newer one;
 *   - re-appending after a render-wipe mid-stream skips the entry fade-in.
 *
 * FloatingApp's constructor has heavy deps (mascot, WindowManager, live
 * DOM), so we exercise the methods on a prototype-based stub with just
 * the fields they touch.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { FloatingApp } from '../../ui/js/floating/app.js';

const DELAY = FloatingApp.SEARCH_LOADING_DELAY_MS;

function makeApp() {
    const appSuggestions = document.createElement('div');
    document.body.appendChild(appSuggestions);
    // Minimal stub: only the fields the loading helpers read.
    const app = Object.create(FloatingApp.prototype);
    app.elements = { appSuggestions };
    app.windowManager = { resizeWindow: vi.fn() };
    app._searchGeneration = 1;
    app._searchLoadingTimer = null;
    app._searchLoadingShownGen = -1;
    app._pendingSearchLoadingLabel = null;
    return app;
}

function hint(app) {
    return app.elements.appSuggestions.querySelector('.suggestions-loading');
}

describe('floating search loading hint', () => {
    beforeEach(() => {
        vi.useFakeTimers();
    });
    afterEach(() => {
        vi.useRealTimers();
        document.body.innerHTML = '';
    });

    it('does not show the hint before the delay gate elapses', () => {
        const app = makeApp();
        app._requestSearchLoading(1, 'loading…');
        expect(hint(app)).toBeNull();
        vi.advanceTimersByTime(DELAY - 50);
        expect(hint(app)).toBeNull();
    });

    it('fast search: hide before the gate cancels the hint entirely', () => {
        const app = makeApp();
        app._requestSearchLoading(1, 'loading…');
        vi.advanceTimersByTime(DELAY - 100);
        app._hideSearchLoading();
        // Even after the original delay would have fired, nothing appears.
        vi.advanceTimersByTime(500);
        expect(hint(app)).toBeNull();
    });

    it('slow search: hint appears once the gate elapses', () => {
        const app = makeApp();
        app._requestSearchLoading(1, 'loading more…');
        vi.advanceTimersByTime(DELAY);
        const el = hint(app);
        expect(el).not.toBeNull();
        expect(el.textContent).toBe('loading more…');
    });

    it('a stale generation never shows its hint', () => {
        const app = makeApp();
        app._requestSearchLoading(1, 'old…');
        // A newer search starts before the gate fires.
        app._searchGeneration = 2;
        vi.advanceTimersByTime(DELAY);
        expect(hint(app)).toBeNull();
    });

    it('hiding a visible hint fades it out, then removes it', () => {
        const app = makeApp();
        app._requestSearchLoading(1, 'loading…');
        vi.advanceTimersByTime(DELAY);
        const el = hint(app);
        expect(el).not.toBeNull();

        app._hideSearchLoading();
        // Still in the DOM, now marked for fade-out.
        expect(el.classList.contains('suggestions-loading-out')).toBe(true);
        expect(el.isConnected).toBe(true);

        // The animationend handler (or the 250ms fallback) removes it.
        vi.advanceTimersByTime(250);
        expect(el.isConnected).toBe(false);
    });

    it('re-append after the gate (render-wipe) skips the entry fade-in', () => {
        const app = makeApp();
        app._requestSearchLoading(1, 'loading…');
        vi.advanceTimersByTime(DELAY);
        expect(hint(app)).not.toBeNull();

        // Simulate renderUnifiedResults wiping the container mid-stream.
        app.elements.appSuggestions.innerHTML = '';

        // Next partial re-requests the hint — same generation, gate already
        // elapsed, so it re-appears immediately without the fade-in class.
        app._requestSearchLoading(1, 'loading more…');
        const el = hint(app);
        expect(el).not.toBeNull();
        expect(el.classList.contains('suggestions-loading-no-in')).toBe(true);
        expect(el.textContent).toBe('loading more…');
    });
});
