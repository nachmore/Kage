/**
 * Regression tests for FloatingApp.handleInputChange — the input handler
 * that drives the suggestions dropdown.
 *
 * The module split (5e43a84) moved handleInputChange into
 * floating/app/search.js but dropped the measureTextareaContentHeight
 * import (it lives in ./helpers.js). The handler then threw a
 * ReferenceError on its first line, before the debounce/search ever ran,
 * so typing never produced a suggestions dropdown — a totally dead
 * feature with no console.error (uncaught exceptions weren't piped to
 * the app log at the time).
 *
 * These tests drive the real prototype method on a stub app, so ANY
 * future missing-import / ReferenceError in the handler's synchronous
 * prefix fails loudly here.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { FloatingApp } from '../../ui/js/floating/app.js';

function makeApp(query) {
    const appSuggestions = document.createElement('div');
    document.body.appendChild(appSuggestions);

    const input = document.createElement('textarea');
    input.value = query;
    document.body.appendChild(input);

    const app = Object.create(FloatingApp.prototype);
    app.elements = { appSuggestions, input };
    app.windowManager = {
        resizeWindow: vi.fn(async () => {}),
        animateInputResize: vi.fn(),
    };
    app.banner = { dismiss: vi.fn() };
    app.updateDatetimeVisibility = vi.fn();
    app._searchGeneration = 0;
    app._searchLoadingTimer = null;
    app._searchLoadingShownGen = -1;
    app._historyIndex = -1;
    app._historySaved = '';
    app._tabCycleActive = false;
    app._clipboardMode = false;
    app.searchTimeout = null;
    app.currentMatches = [];
    app.selectedIndex = -1;
    app._noMatchSinceLen = 0;
    return app;
}

describe('floating handleInputChange', () => {
    beforeEach(() => {
        vi.useFakeTimers();
    });
    afterEach(() => {
        vi.useRealTimers();
        document.body.innerHTML = '';
    });

    it('runs without throwing and arms the debounced search', async () => {
        const app = makeApp('spot');
        // The regression: this threw `measureTextareaContentHeight is not
        // defined` before any search logic ran.
        await app.handleInputChange({});
        expect(app.searchTimeout).not.toBeNull();
        expect(app._searchGeneration).toBe(1);
    });

    it('clears the dropdown when the input empties', async () => {
        const app = makeApp('');
        app.elements.appSuggestions.classList.add('visible');
        app.currentMatches = [{ type: 'app', name: 'x' }];
        app.selectedIndex = 0;

        await app.handleInputChange({});

        expect(app.elements.appSuggestions.classList.contains('visible')).toBe(false);
        expect(app.currentMatches).toEqual([]);
        expect(app.selectedIndex).toBe(-1);
        expect(app.searchTimeout).toBeNull();
    });

    it('resets tab-cycle and history browsing state on typing', async () => {
        const app = makeApp('abc');
        app._tabCycleActive = true;
        app._historyIndex = 2;
        app._historySaved = 'stash';

        await app.handleInputChange({});

        expect(app._tabCycleActive).toBe(false);
        expect(app._historyIndex).toBe(-1);
        expect(app._historySaved).toBe('');
    });
});
