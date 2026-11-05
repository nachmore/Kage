/**
 * Keyboard path to the month calendar from the launcher input
 * (InputMethods.handleKeyDown → FloatingCalendarPicker.openFromKeyboard).
 *
 * The date openers (built-in date, widget-bar date, result-row dates) can't
 * be reached with Tab — the input swallows Tab to cycle suggestions — so
 * Alt+↓ / ⌥↓ (the WAI-ARIA "open the date picker" key) opens the popup.
 * With text in the input it falls through so ⌥↓ still moves the caret,
 * unless the highlighted result row carries a date.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../ui/js/floating/app/helpers.js', () => ({
    caretVisualRowInfo: () => ({ isFirstRow: true, isLastRow: true }),
}));

const { InputMethods } = await import('../../ui/js/floating/app/input.js');
const { FloatingCalendarPicker } = await import('../../ui/js/floating/calendar-picker.js');

function setup() {
    document.body.innerHTML = `
        <div class="speech-bubble">
            <div class="input-container">
                <textarea id="promptInput"></textarea>
                <div class="datetime-display"><button id="datetimeDate">Wed, Nov 4</button></div>
            </div>
            <div class="app-suggestions visible" id="appSuggestions"></div>
        </div>`;
    const input = document.getElementById('promptInput');
    const appSuggestions = document.getElementById('appSuggestions');
    let handler = null;
    const extMgr = {
        setDateAffordanceHandler: (fn) => (handler = fn),
        getDateProvider: () => 'calendar',
        missingDateProviderStatus: () => 'no_provider',
        fetchDateMarkers: vi.fn(async () => ({ status: 'ok', days: new Map() })),
        resolveDateQuery: vi.fn(async (_id, iso) => `cal ${iso}`),
    };
    const calendarPicker = new FloatingCalendarPicker({
        extensionManager: extMgr,
        input,
        now: () => new Date(2026, 10, 4, 9, 30),
        getLocale: () => 'en-US',
        getRtl: () => false,
        getWeekStart: () => 0,
    });
    calendarPicker.attachBuiltInDate(document.getElementById('datetimeDate'));
    const app = {
        elements: { input, appSuggestions },
        currentMatches: [],
        selectedIndex: -1,
        _messageHistory: [],
        _historyIndex: -1,
        _historySaved: '',
        calendarPicker,
        handleKeyDown: InputMethods.handleKeyDown,
        _resizeInputToContent() {},
    };
    return { app, input, appSuggestions, calendarPicker, handler: () => handler };
}

const altDown = (extra = {}) =>
    new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, cancelable: true, ...extra });

let ctx;
beforeEach(() => {
    ctx = setup();
});
afterEach(() => {
    ctx.calendarPicker.close({ immediate: true });
    document.body.innerHTML = '';
});

describe('Alt+↓ in the launcher input', () => {
    it('opens the calendar on today from an empty input', async () => {
        const ev = altDown();
        await ctx.app.handleKeyDown.call(ctx.app, ev);
        expect(ev.defaultPrevented).toBe(true);
        expect(ctx.calendarPicker.isOpen()).toBe(true);
        expect(document.activeElement.dataset.date).toBe('2026-11-04');
    });

    it('falls through with text in the input and nothing dated highlighted', async () => {
        ctx.input.value = 'draft';
        const ev = altDown();
        await ctx.app.handleKeyDown.call(ctx.app, ev);
        expect(ev.defaultPrevented).toBe(false);
        expect(ctx.calendarPicker.isOpen()).toBe(false);
    });

    it('opens from the highlighted result row’s date', async () => {
        ctx.input.value = 'cal 2026-11-26';
        ctx.appSuggestions.innerHTML = `
            <div class="app-suggestion-item">No date here</div>
            <div class="app-suggestion-item">[<button class="kage-date-affordance" id="chip">Thu, Nov 26</button>] 10:00</div>`;
        const chip = document.getElementById('chip');
        chip.addEventListener('click', () =>
            ctx.handler()({ anchor: chip, date: '2026-11-26', extensionId: 'calendar' })
        );
        ctx.app.currentMatches = [{}, {}];
        ctx.app.selectedIndex = 1;
        const ev = altDown();
        await ctx.app.handleKeyDown.call(ctx.app, ev);
        expect(ev.defaultPrevented).toBe(true);
        expect(ctx.calendarPicker.isOpen()).toBe(true);
        expect(chip.getAttribute('aria-expanded')).toBe('true');
        expect(document.querySelector('.kdp-day.is-selected').dataset.date).toBe('2026-11-26');
    });

    it('needs Alt alone: Ctrl/⌘/Shift+Alt+↓ and a plain ↓ are left alone', async () => {
        for (const extra of [{ ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { altKey: false }]) {
            await ctx.app.handleKeyDown.call(ctx.app, altDown(extra));
            expect(ctx.calendarPicker.isOpen()).toBe(false);
        }
    });
});
