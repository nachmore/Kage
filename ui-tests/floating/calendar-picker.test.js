/**
 * Floating-window host for the date picker (ui/js/floating/calendar-picker.js):
 * where the popup mounts, opening/closing (outside click, toggle, Escape,
 * focus return), one busy-day lookup per month view, and the selection →
 * meetings routing through the provider's own launcher query.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { FloatingCalendarPicker } from '../../ui/js/floating/calendar-picker.js';
import { emptyMarkers, normalizeMarkers } from '../../ui/js/shared/date-markers.js';

const NOW = new Date(2026, 10, 4, 9, 30); // Wed Nov 4 2026, local
const flush = () => new Promise((r) => setTimeout(r, 0));

function buildDom() {
    document.body.className = '';
    document.body.innerHTML = `
        <div class="floating-container"><div class="speech-bubble">
            <div class="content-area" id="contentArea"></div>
            <div id="extWidgetSlotBottom" class="ext-widget-slot"><div class="ext-widget">
                <button id="widgetDate" data-kage-date="2026-11-05">Tomorrow</button>
            </div></div>
            <div class="input-container">
                <textarea id="promptInput"></textarea>
                <div class="datetime-display"><button id="datetimeDate">Wed, Nov 4</button></div>
            </div>
            <div class="floating-toolbar" id="floatingToolbar"></div>
            <div class="app-suggestions visible" id="appSuggestions">
                <div class="app-suggestion-item"><button id="rowDate">[Thu, Nov 26]</button></div>
            </div>
        </div></div>`;
}

function fakeExtMgr({ provider = 'calendar', markers, query = (iso) => `cal ${iso}` } = {}) {
    let handler = null;
    return {
        setDateAffordanceHandler: vi.fn((fn) => (handler = fn)),
        get handler() {
            return handler;
        },
        getDateProvider: vi.fn(() => provider),
        fetchDateMarkers: vi.fn(async (_id, start, end) =>
            markers ? normalizeMarkers(markers, start, end) : emptyMarkers('ok')
        ),
        resolveDateQuery: vi.fn(async (_id, iso) => query(iso)),
    };
}

function setup(opts) {
    buildDom();
    const extMgr = fakeExtMgr(opts);
    const input = document.getElementById('promptInput');
    const host = new FloatingCalendarPicker({
        extensionManager: extMgr,
        input,
        now: () => NOW,
        getLocale: () => 'en-US',
        getRtl: () => false,
    });
    const builtIn = document.getElementById('datetimeDate');
    host.attachBuiltInDate(builtIn);
    return { host, extMgr, input, builtIn };
}

const bubbleOrder = () =>
    [...document.querySelector('.speech-bubble').children].map(
        (c) => c.id || c.className.split(' ')[0]
    );

beforeEach(() => vi.useRealTimers());
afterEach(() => {
    document.body.innerHTML = '';
});

describe('FloatingCalendarPicker — opening', () => {
    it('registers itself as the extension date-affordance handler', () => {
        const { host, extMgr } = setup();
        expect(extMgr.setDateAffordanceHandler).toHaveBeenCalledTimes(1);
        extMgr.handler({ anchor: document.getElementById('widgetDate'), date: '2026-11-05', extensionId: 'calendar' });
        expect(host.isOpen()).toBe(true);
    });

    it('the built-in date opens on today, under the input row, routed to the provider', () => {
        const { host, builtIn, extMgr } = setup();
        builtIn.click();
        expect(host.isOpen()).toBe(true);
        expect(bubbleOrder()).toEqual([
            'contentArea',
            'extWidgetSlotBottom',
            'input-container',
            'kage-date-popover-row',
            'floatingToolbar',
            'appSuggestions',
        ]);
        const picker = document.querySelector('.kage-date-picker');
        expect(picker.querySelector('.kdp-day.is-selected').dataset.date).toBe('2026-11-04');
        expect(picker.querySelector('.kdp-day.is-today').dataset.date).toBe('2026-11-04');
        expect(document.activeElement.dataset.date).toBe('2026-11-04');
        expect(builtIn.getAttribute('aria-expanded')).toBe('true');
        expect(extMgr.getDateProvider).toHaveBeenCalled();
    });

    it('a widget date opens right under the widget bar, on that date', () => {
        const { host, extMgr } = setup();
        extMgr.handler({ anchor: document.getElementById('widgetDate'), date: '2026-11-05', extensionId: 'calendar' });
        expect(bubbleOrder().slice(1, 3)).toEqual(['extWidgetSlotBottom', 'kage-date-popover-row']);
        expect(document.querySelector('.kdp-day.is-selected').dataset.date).toBe('2026-11-05');
        host.close({ immediate: true });
    });

    it('a date inside the result list takes the list’s place while open', async () => {
        const { host, extMgr } = setup();
        extMgr.handler({ anchor: document.getElementById('rowDate'), date: '2026-11-26', extensionId: 'calendar' });
        expect(bubbleOrder().slice(-2)).toEqual(['kage-date-popover-row', 'appSuggestions']);
        expect(document.body.classList.contains('kage-date-picker-over-suggestions')).toBe(true);
        host.close();
        expect(document.body.classList.contains('kage-date-picker-over-suggestions')).toBe(false);
        await flush();
        expect(document.querySelector('.kage-date-popover-row')).toBeNull();
    });
});

describe('FloatingCalendarPicker — closing', () => {
    it('closes on an outside pointerdown', async () => {
        const { host, builtIn } = setup();
        builtIn.click();
        document.getElementById('contentArea').dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
        expect(host.isOpen()).toBe(false);
        expect(builtIn.getAttribute('aria-expanded')).toBe('false');
        await flush();
        expect(document.querySelector('.kage-date-picker')).toBeNull();
    });

    it('a pointerdown inside the popup keeps it open', () => {
        const { host, builtIn } = setup();
        builtIn.click();
        document.querySelector('.kdp-next').dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
        expect(host.isOpen()).toBe(true);
    });

    it('clicking the same opener again toggles it closed', () => {
        const { host, builtIn } = setup();
        builtIn.click();
        builtIn.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
        builtIn.click();
        expect(host.isOpen()).toBe(false);
    });

    it('Escape closes, refocuses the opener, and never reaches the hide-window handler', async () => {
        const { host, builtIn } = setup();
        const hideWindow = vi.fn();
        document.addEventListener('keydown', hideWindow);
        try {
            builtIn.click();
            document.activeElement.dispatchEvent(
                new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
            );
            expect(host.isOpen()).toBe(false);
            expect(hideWindow).not.toHaveBeenCalled();
            // jsdom has no layout, so offsetParent is null → falls back to the input.
            expect(document.activeElement.id).toBe('promptInput');
        } finally {
            document.removeEventListener('keydown', hideWindow);
        }
    });
});

describe('FloatingCalendarPicker — busy-day dots', () => {
    it('asks the provider once per month view and caches it', async () => {
        const { host, builtIn, extMgr } = setup({ markers: { days: [{ date: '2026-11-12', count: 3 }] } });
        builtIn.click();
        await flush();
        expect(extMgr.fetchDateMarkers).toHaveBeenCalledWith('calendar', '2026-11-01', '2026-12-13');
        expect(document.querySelector('.kdp-day[data-date="2026-11-12"]').classList.contains('has-events')).toBe(true);
        document.querySelector('.kdp-next').click();
        document.querySelector('.kdp-prev').click();
        await flush();
        // November again: served from the cache.
        expect(extMgr.fetchDateMarkers).toHaveBeenCalledTimes(2);
        host.close({ immediate: true });
        builtIn.click();
        await flush();
        expect(extMgr.fetchDateMarkers).toHaveBeenCalledTimes(2);
    });

    it('with no provider: no lookup, no dots, the no-provider hint', async () => {
        const { builtIn, extMgr } = setup({ provider: null });
        builtIn.click();
        await flush();
        expect(extMgr.fetchDateMarkers).not.toHaveBeenCalled();
        expect(document.querySelector('.kdp-dot')).toBeNull();
        expect(document.querySelector('.kdp-hint').hidden).toBe(false);
    });
});

describe('FloatingCalendarPicker — selection → meetings', () => {
    it('routes the picked day to the provider’s query and runs it like typing', async () => {
        const { host, builtIn, input, extMgr } = setup();
        const onInput = vi.fn();
        input.addEventListener('input', onInput);
        builtIn.click();
        document.querySelector('.kdp-day[data-date="2026-11-19"]').click();
        expect(host.isOpen()).toBe(false);
        await flush();
        expect(extMgr.resolveDateQuery).toHaveBeenCalledWith('calendar', '2026-11-19');
        expect(input.value).toBe('cal 2026-11-19');
        expect(onInput).toHaveBeenCalledTimes(1);
        expect(document.activeElement).toBe(input);
    });

    it('a widget/result opener routes to ITS extension', async () => {
        const { input, extMgr } = setup();
        extMgr.handler({ anchor: document.getElementById('rowDate'), date: '2026-11-26', extensionId: 'calendar' });
        document.querySelector('.kdp-day[data-date="2026-11-27"]').click();
        await flush();
        expect(extMgr.resolveDateQuery).toHaveBeenCalledWith('calendar', '2026-11-27');
        expect(input.value).toBe('cal 2026-11-27');
    });

    it('leaves the input alone when the provider declines (or none exists)', async () => {
        const declined = setup({ query: () => null });
        declined.input.value = 'draft';
        declined.builtIn.click();
        document.querySelector('.kdp-day[data-date="2026-11-19"]').click();
        await flush();
        expect(declined.input.value).toBe('draft');

        const none = setup({ provider: null });
        none.builtIn.click();
        document.querySelector('.kdp-day[data-date="2026-11-19"]').click();
        await flush();
        expect(none.extMgr.resolveDateQuery).not.toHaveBeenCalled();
        expect(none.host.isOpen()).toBe(false);
    });

    it('keyboard: arrows + Enter pick a day', async () => {
        const { builtIn, input } = setup();
        builtIn.click();
        const key = (k, extra) =>
            document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...extra }));
        key('ArrowDown');
        key('PageDown');
        expect(document.activeElement.dataset.date).toBe('2026-12-11');
        document.activeElement.click(); // Enter on a <button>
        await flush();
        expect(input.value).toBe('cal 2026-12-11');
    });
});
