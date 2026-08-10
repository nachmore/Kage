/**
 * Tests for ArrowUp/ArrowDown prompt-history navigation in the floating input.
 *
 * The rule (text-editor semantics keyed on the caret's *visual* row):
 *   - ArrowUp on the first visual row recalls the previous prompt; off the
 *     first row it moves the caret up within a multi-line / wrapped prompt.
 *   - ArrowDown on the last visual row steps forward toward the draft; off the
 *     last row it moves the caret down.
 *   - Recall only fires when the input is empty or we're already browsing, so
 *     Up in freshly-typed text never clobbers it.
 *
 * caretVisualRowInfo depends on layout (offsetTop/scrollHeight), which jsdom
 * does not compute, so it's mocked here to drive the row conditions directly.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

const rowInfo = vi.fn();
vi.mock('../../ui/js/floating/app/helpers.js', () => ({
    caretVisualRowInfo: (...args) => rowInfo(...args),
}));

const { InputMethods } = await import('../../ui/js/floating/app/input.js');

function makeApp({ history = [], matches = [] } = {}) {
    const input = document.createElement('textarea');
    document.body.appendChild(input);
    const appSuggestions = document.createElement('div');
    document.body.appendChild(appSuggestions);

    return {
        elements: { input, appSuggestions },
        currentMatches: matches,
        selectedIndex: matches.length ? 0 : -1,
        _messageHistory: history,
        _historyIndex: -1,
        _historySaved: '',
        // Methods under test, bound to this mock instance.
        handleKeyDown: InputMethods.handleKeyDown,
        // Resize is a layout side-effect (SearchMethods, needs the window
        // manager); stub it — history state, not sizing, is under test here.
        _resizeInputToContent() {},
    };
}

function keydown(key) {
    const ev = new KeyboardEvent('keydown', { key, cancelable: true });
    return ev;
}

describe('floating input history navigation', () => {
    beforeEach(() => {
        rowInfo.mockReset();
        // Default: single-row input — caret is on both first and last row.
        rowInfo.mockReturnValue({ isFirstRow: true, isLastRow: true });
    });

    it('ArrowUp on an empty input recalls the most recent prompt', async () => {
        const app = makeApp({ history: ['second', 'first'] });
        await app.handleKeyDown.call(app, keydown('ArrowUp'));
        expect(app.elements.input.value).toBe('second');
        expect(app._historyIndex).toBe(0);
        // Caret parked at end so subsequent Up walks up the recalled prompt.
        expect(app.elements.input.selectionStart).toBe('second'.length);
    });

    it('ArrowUp cycles further back through history', async () => {
        const app = makeApp({ history: ['c', 'b', 'a'] });
        await app.handleKeyDown.call(app, keydown('ArrowUp'));
        await app.handleKeyDown.call(app, keydown('ArrowUp'));
        expect(app.elements.input.value).toBe('b');
        expect(app._historyIndex).toBe(1);
    });

    it('stops at the oldest prompt', async () => {
        const app = makeApp({ history: ['b', 'a'] });
        await app.handleKeyDown.call(app, keydown('ArrowUp'));
        await app.handleKeyDown.call(app, keydown('ArrowUp'));
        await app.handleKeyDown.call(app, keydown('ArrowUp')); // no-op past the end
        expect(app.elements.input.value).toBe('a');
        expect(app._historyIndex).toBe(1);
    });

    it('ArrowDown walks forward and lands back on a blank input at the bottom', async () => {
        const app = makeApp({ history: ['b', 'a'] });
        await app.handleKeyDown.call(app, keydown('ArrowUp')); // -> 'b'
        await app.handleKeyDown.call(app, keydown('ArrowUp')); // -> 'a'
        await app.handleKeyDown.call(app, keydown('ArrowDown')); // -> 'b'
        expect(app.elements.input.value).toBe('b');
        expect(app._historyIndex).toBe(0);
        await app.handleKeyDown.call(app, keydown('ArrowDown')); // -> back to blank
        expect(app.elements.input.value).toBe('');
        expect(app._historyIndex).toBe(-1);
    });

    it('ArrowUp off the first visual row moves the caret, does not recall', async () => {
        const app = makeApp({ history: ['old'] });
        app.elements.input.value = 'line1\nline2';
        // Caret on the second row: not first row.
        rowInfo.mockReturnValue({ isFirstRow: false, isLastRow: true });
        const ev = keydown('ArrowUp');
        await app.handleKeyDown.call(app, ev);
        // Left to default behavior — value untouched, event not prevented.
        expect(app.elements.input.value).toBe('line1\nline2');
        expect(app._historyIndex).toBe(-1);
        expect(ev.defaultPrevented).toBe(false);
    });

    it('ArrowUp on the first row of a recalled multi-line prompt recalls the previous one', async () => {
        const app = makeApp({ history: ['top\nbottom', 'older'] });
        await app.handleKeyDown.call(app, keydown('ArrowUp')); // recall 'top\nbottom'
        expect(app.elements.input.value).toBe('top\nbottom');
        // Now on the first row of the recalled prompt.
        rowInfo.mockReturnValue({ isFirstRow: true, isLastRow: false });
        await app.handleKeyDown.call(app, keydown('ArrowUp'));
        expect(app.elements.input.value).toBe('older');
        expect(app._historyIndex).toBe(1);
    });

    it('ArrowUp in freshly-typed non-empty text does not clobber the draft', async () => {
        const app = makeApp({ history: ['old'] });
        app.elements.input.value = 'typing something';
        const ev = keydown('ArrowUp');
        await app.handleKeyDown.call(app, ev);
        expect(app.elements.input.value).toBe('typing something');
        expect(app._historyIndex).toBe(-1);
        expect(ev.defaultPrevented).toBe(false);
    });

    it('does not navigate history while suggestions are showing', async () => {
        const app = makeApp({
            history: ['old'],
            matches: [{ type: 'app', name: 'Calc' }],
        });
        // Suggestion item present so the suggestion-nav branch is live.
        const item = document.createElement('div');
        item.className = 'app-suggestion-item';
        app.elements.appSuggestions.appendChild(item);
        await app.handleKeyDown.call(app, keydown('ArrowUp'));
        expect(app.elements.input.value).toBe('');
        expect(app._historyIndex).toBe(-1);
    });
});
