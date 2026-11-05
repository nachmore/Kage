/**
 * Month-calendar date picker DOM component (ui/js/shared/date-picker.js):
 * grid rendering + a11y attributes, busy-day dots from the markers source,
 * the permission / no-provider hints, month/year views, Today, keyboard
 * navigation (incl. RTL) and Escape.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';
import { DatePicker } from '../../ui/js/shared/date-picker.js';
import { normalizeMarkers } from '../../ui/js/shared/date-markers.js';

// Echo keys + vars so assertions don't depend on catalog wording.
const t = (key, vars) => (vars ? `${key}:${JSON.stringify(vars)}` : key);

const pickers = [];
function make(opts = {}) {
    const p = new DatePicker({
        locale: 'en-US',
        weekStart: 0,
        today: '2026-11-04',
        selected: '2026-11-04',
        t,
        ...opts,
    });
    document.body.appendChild(p.element);
    pickers.push(p);
    return p;
}
afterEach(() => {
    while (pickers.length) pickers.pop().destroy();
});

const flush = () => new Promise((r) => setTimeout(r, 0));
const dayBtn = (p, iso) => p.element.querySelector(`.kdp-day[data-date="${iso}"]`);
const key = (el, k, extra = {}) => {
    const ev = new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...extra });
    el.dispatchEvent(ev);
    return ev;
};

describe('DatePicker — weekday headers', () => {
    it('narrow names when a short one overflows its column; full name kept for AT', () => {
        const p = make({ locale: 'en-US' });
        const ths = [...p.element.querySelectorAll('th')];
        // jsdom has no layout: nothing overflows.
        expect(p.fitWeekdayHeaders()).toBe(false);
        expect(ths[0].textContent).toBe('Sun');
        Object.defineProperty(ths[3], 'scrollWidth', { configurable: true, get: () => 39 });
        Object.defineProperty(ths[3], 'clientWidth', { configurable: true, get: () => 35 });
        expect(p.fitWeekdayHeaders()).toBe(true);
        expect(ths.map((th) => th.textContent)).toEqual(['S', 'M', 'T', 'W', 'T', 'F', 'S']);
        expect(ths[0].getAttribute('abbr')).toBe('Sunday');
    });
});

describe('DatePicker — day grid', () => {
    it('renders a 6×7 grid with locale weekday headers from the week start', () => {
        const p = make({ weekStart: 1 });
        const heads = [...p.element.querySelectorAll('th')].map((th) => th.textContent);
        expect(heads).toEqual(['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']);
        expect(p.element.querySelectorAll('th')[0].getAttribute('abbr')).toBe('Monday');
        expect(p.element.querySelectorAll('.kdp-day')).toHaveLength(42);
        expect(p.element.querySelector('.kdp-day').dataset.date).toBe('2026-10-26');
        expect(p.element.querySelector('table').getAttribute('role')).toBe('grid');
    });

    it('titles the month and labels the grid with it', () => {
        const p = make();
        expect(p.element.querySelector('.kdp-title').textContent).toBe('November 2026');
        const labelId = p.element.querySelector('table').getAttribute('aria-labelledby');
        expect(document.getElementById(labelId).textContent).toBe('November 2026');
        expect(p.element.getAttribute('role')).toBe('dialog');
    });

    it('highlights today and the selected day, with aria-selected / aria-current', () => {
        const p = make({ selected: '2026-11-26' });
        const today = dayBtn(p, '2026-11-04');
        const sel = dayBtn(p, '2026-11-26');
        expect(today.classList.contains('is-today')).toBe(true);
        expect(today.getAttribute('aria-current')).toBe('date');
        expect(sel.classList.contains('is-selected')).toBe(true);
        expect(sel.parentElement.getAttribute('aria-selected')).toBe('true');
        expect(today.parentElement.getAttribute('aria-selected')).toBe('false');
        // Roving tabindex: the selected (opened-on) day is the tab stop.
        expect(sel.tabIndex).toBe(0);
        expect(today.tabIndex).toBe(-1);
        expect(dayBtn(p, '2026-12-01').classList.contains('is-outside')).toBe(true);
    });

    it('selecting a day reports it', () => {
        const onSelect = vi.fn();
        const p = make({ onSelect });
        dayBtn(p, '2026-11-18').click();
        expect(onSelect).toHaveBeenCalledWith('2026-11-18');
        expect(dayBtn(p, '2026-11-18').classList.contains('is-selected')).toBe(true);
    });
});

describe('DatePicker — busy-day dots', () => {
    it('asks once for the visible grid range and paints dots + counts', async () => {
        const loadMarkers = vi.fn(async (start, end) =>
            normalizeMarkers({ status: 'ok', days: [{ date: '2026-11-05', count: 2 }, { date: '2026-10-27', count: 1 }] }, start, end)
        );
        const p = make({ loadMarkers });
        expect(loadMarkers).toHaveBeenCalledWith('2026-11-01', '2026-12-13');
        await flush();
        const busy = dayBtn(p, '2026-11-05');
        expect(busy.classList.contains('has-events')).toBe(true);
        expect(busy.querySelectorAll('.kdp-dot')).toHaveLength(1);
        expect(busy.getAttribute('aria-label')).toBe(
            'shared.date_picker.day_meetings:{"date":"Thursday, November 5, 2026","count":2}'
        );
        const free = dayBtn(p, '2026-11-06');
        expect(free.querySelector('.kdp-dot')).toBeNull();
        expect(free.getAttribute('aria-label')).toContain('"count":0');
        // Selecting a day doesn't refetch.
        dayBtn(p, '2026-11-07').click();
        expect(loadMarkers).toHaveBeenCalledTimes(1);
    });

    it('one dot per calendar colour, capped at three', async () => {
        const loadMarkers = async (s, e) =>
            normalizeMarkers({ days: { '2026-11-05': { count: 5, colors: ['#e11d48', '#2563eb', '#16a34a', '#ca8a04'] } } }, s, e);
        const p = make({ loadMarkers });
        await flush();
        const dots = [...dayBtn(p, '2026-11-05').querySelectorAll('.kdp-dot')];
        expect(dots).toHaveLength(3);
        expect(dots[0].style.backgroundColor).toBe('rgb(225, 29, 72)');
    });

    it('paints without waiting for markers, and ignores a stale answer after paging', async () => {
        const resolvers = [];
        const loadMarkers = vi.fn(() => new Promise((r) => resolvers.push(r)));
        const p = make({ loadMarkers, markerDebounceMs: 0 });
        expect(dayBtn(p, '2026-11-05')).not.toBeNull(); // painted already
        p.element.querySelector('.kdp-next').click(); // → December
        // One lookup at a time: December waits for November's to land.
        expect(loadMarkers).toHaveBeenCalledTimes(1);
        // November's answer arrives late: must not paint into December…
        resolvers[0](normalizeMarkers({ days: { '2026-12-01': 9 } }, '2026-11-01', '2026-12-13'));
        await flush();
        expect(dayBtn(p, '2026-12-01').classList.contains('has-events')).toBe(false);
        // …and December is asked for as soon as it does.
        expect(loadMarkers).toHaveBeenCalledTimes(2);
        expect(loadMarkers).toHaveBeenLastCalledWith('2026-11-29', '2027-01-10');
        resolvers[1](normalizeMarkers({ days: { '2026-12-01': 1 } }, '2026-11-29', '2027-01-10'));
        await flush();
        expect(dayBtn(p, '2026-12-01').classList.contains('has-events')).toBe(true);
    });

    it('debounces paging: only the month the user stops on is looked up', async () => {
        vi.useFakeTimers();
        try {
            const loadMarkers = vi.fn(async () => ({ status: 'ok', days: new Map() }));
            const p = make({ loadMarkers, markerDebounceMs: 200 });
            expect(loadMarkers).toHaveBeenCalledTimes(1); // the first range: at once
            await vi.advanceTimersByTimeAsync(0);
            for (let i = 0; i < 6; i++) {
                p.element.querySelector('.kdp-next').click();
                await vi.advanceTimersByTimeAsync(100);
            }
            expect(loadMarkers).toHaveBeenCalledTimes(1);
            expect(p.element.querySelector('table').getAttribute('aria-busy')).toBe('true');
            await vi.advanceTimersByTimeAsync(200);
            expect(loadMarkers).toHaveBeenCalledTimes(2);
            expect(loadMarkers).toHaveBeenLastCalledWith('2027-04-25', '2027-06-06'); // May 2027
            expect(p.element.querySelector('table').hasAttribute('aria-busy')).toBe(false);
        } finally {
            vi.useRealTimers();
        }
    });

    it('a cached range paints at once, with no lookup and no debounce', () => {
        const cached = { status: 'ok', days: new Map([['2026-12-02', { count: 1, colors: [] }]]) };
        const loadMarkers = vi.fn(async () => ({ status: 'ok', days: new Map() }));
        const peekMarkers = vi.fn((start) => (start === '2026-11-29' ? cached : null));
        const p = make({ loadMarkers, peekMarkers, markerDebounceMs: 10_000 });
        expect(loadMarkers).toHaveBeenCalledTimes(1); // November: not cached
        p.element.querySelector('.kdp-next').click(); // December: cached
        expect(dayBtn(p, '2026-12-02').classList.contains('has-events')).toBe(true);
        expect(loadMarkers).toHaveBeenCalledTimes(1);
    });

    it('shows the permission hint, and nothing for plain errors', async () => {
        const denied = make({ loadMarkers: async () => ({ status: 'permission_denied', days: new Map() }) });
        await flush();
        expect(denied.element.querySelector('.kdp-hint').textContent).toBe('shared.date_picker.hint.permission');
        expect(denied.element.querySelector('.kdp-hint').hidden).toBe(false);

        const broken = make({ loadMarkers: async () => { throw new Error('boom'); } });
        await flush();
        expect(broken.element.querySelector('.kdp-hint').hidden).toBe(true);
        expect(broken.element.querySelector('.kdp-dot')).toBeNull();
    });

    it('shows the no-provider hint', async () => {
        const p = make({ loadMarkers: async () => ({ status: 'no_provider', days: new Map() }) });
        await flush();
        expect(p.element.querySelector('.kdp-hint').textContent).toBe('shared.date_picker.hint.no_provider');
    });

    it('shows the update hint for an outdated provider', async () => {
        const p = make({ loadMarkers: async () => ({ status: 'provider_outdated', days: new Map() }) });
        await flush();
        expect(p.element.querySelector('.kdp-hint').textContent).toBe('shared.date_picker.hint.update_provider');
    });
});

describe('DatePicker — paging and views', () => {
    it('prev/next page months', () => {
        const p = make();
        p.element.querySelector('.kdp-prev').click();
        expect(p.element.querySelector('.kdp-title').textContent).toBe('October 2026');
        p.element.querySelector('.kdp-next').click();
        p.element.querySelector('.kdp-next').click();
        expect(p.element.querySelector('.kdp-title').textContent).toBe('December 2026');
    });

    it('title → month list → year range, and back down by picking', () => {
        const p = make();
        p.element.querySelector('.kdp-title').click();
        expect(p.element.dataset.view).toBe('months');
        expect(p.element.querySelector('.kdp-title').textContent).toBe('2026');
        expect(p.element.querySelector('.kdp-months').hidden).toBe(false);
        expect(p.element.querySelector('table').hidden).toBe(true);
        p.element.querySelector('.kdp-next').click(); // → 2027
        p.element.querySelector('.kdp-title').click();
        expect(p.element.dataset.view).toBe('years');
        expect(p.element.querySelector('.kdp-title').textContent).toBe('2020 – 2029');
        p.element.querySelector('.kdp-next').click(); // → 2030s
        expect(p.element.querySelector('.kdp-title').textContent).toBe('2030 – 2039');
        const y2031 = [...p.element.querySelectorAll('.kdp-years .kdp-pick')].find((b) => b.textContent === '2031');
        y2031.click();
        expect(p.element.dataset.view).toBe('months');
        p.element.querySelector('.kdp-months .kdp-pick[data-month="1"]').click(); // February
        expect(p.element.dataset.view).toBe('days');
        expect(p.element.querySelector('.kdp-title').textContent).toBe('February 2031');
    });

    it('Today jumps back to the current month', () => {
        const p = make();
        p.showDate('2031-02-10');
        expect(p.element.querySelector('.kdp-title').textContent).toBe('February 2031');
        p.element.querySelector('.kdp-today').click();
        expect(p.element.querySelector('.kdp-title').textContent).toBe('November 2026');
        expect(dayBtn(p, '2026-11-04').tabIndex).toBe(0);
    });

    it('marks this month/year and the selected one in the list views', () => {
        const p = make({ selected: '2026-03-10' });
        p.element.querySelector('.kdp-title').click();
        const months = p.element.querySelectorAll('.kdp-months .kdp-pick');
        expect(months[10].classList.contains('is-today')).toBe(true);
        expect(months[2].classList.contains('is-selected')).toBe(true);
        expect(months[2].parentElement.getAttribute('aria-selected')).toBe('true');
        expect(months[2].getAttribute('aria-label')).toBe('March');
    });
});

describe('DatePicker — keyboard', () => {
    it('arrows, PageUp/PageDown and Shift+PageUp move the focused day', () => {
        const p = make();
        p.focus();
        expect(document.activeElement).toBe(dayBtn(p, '2026-11-04'));
        key(document.activeElement, 'ArrowRight');
        expect(document.activeElement).toBe(dayBtn(p, '2026-11-05'));
        key(document.activeElement, 'ArrowDown');
        expect(document.activeElement.dataset.date).toBe('2026-11-12');
        key(document.activeElement, 'PageDown');
        expect(document.activeElement.dataset.date).toBe('2026-12-12');
        expect(p.element.querySelector('.kdp-title').textContent).toBe('December 2026');
        key(document.activeElement, 'PageUp', { shiftKey: true });
        expect(document.activeElement.dataset.date).toBe('2025-12-12');
        key(document.activeElement, 'Home');
        expect(document.activeElement.dataset.date).toBe('2025-12-07'); // Sunday week start
    });

    it('moving past the grid edge pages the month in place', () => {
        const p = make();
        p.showDate('2026-11-30');
        p.focus();
        key(document.activeElement, 'ArrowDown');
        expect(document.activeElement.dataset.date).toBe('2026-12-07');
        expect(p.element.querySelector('.kdp-title').textContent).toBe('December 2026');
    });

    it('mirrors left/right in RTL', () => {
        const p = make({ rtl: true, locale: 'he' });
        expect(p.element.getAttribute('dir')).toBe('rtl');
        p.focus();
        key(document.activeElement, 'ArrowLeft');
        expect(document.activeElement.dataset.date).toBe('2026-11-05');
    });

    it('Enter on a day selects it (native button activation)', () => {
        const onSelect = vi.fn();
        const p = make({ onSelect });
        p.focus();
        key(document.activeElement, 'ArrowRight');
        document.activeElement.click(); // what Enter/Space do on a <button>
        expect(onSelect).toHaveBeenCalledWith('2026-11-05');
    });

    it('Escape closes and does not reach document-level handlers', () => {
        const onClose = vi.fn();
        const docHandler = vi.fn();
        document.addEventListener('keydown', docHandler);
        try {
            const p = make({ onClose });
            p.focus();
            const ev = key(document.activeElement, 'Escape');
            expect(onClose).toHaveBeenCalledTimes(1);
            expect(ev.defaultPrevented).toBe(true);
            expect(docHandler).not.toHaveBeenCalled();
        } finally {
            document.removeEventListener('keydown', docHandler);
        }
    });

    it('navigates the month list with arrows and pages the year at the edges', () => {
        const p = make();
        p.focus();
        p.element.querySelector('.kdp-title').click();
        expect(document.activeElement.dataset.month).toBe('10'); // November
        key(document.activeElement, 'ArrowDown');
        expect(document.activeElement.dataset.month).toBe('1'); // Feb, next year
        expect(p.element.querySelector('.kdp-title').textContent).toBe('2027');
        key(document.activeElement, 'ArrowLeft');
        expect(document.activeElement.dataset.month).toBe('0');
    });

    it('a press on the weekday row, a cell gap or the footer keeps focus on the grid', () => {
        const p = make();
        p.focus();
        const before = document.activeElement;
        for (const el of [
            p.element.querySelector('th'),
            p.element.querySelector('td'),
            p.element.querySelector('.kdp-footer'),
            p.element,
        ]) {
            const md = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
            el.dispatchEvent(md);
            expect(md.defaultPrevented).toBe(true);
        }
        expect(document.activeElement).toBe(before);
        // A day itself may take focus (it's the click target).
        const md = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
        dayBtn(p, '2026-11-10').dispatchEvent(md);
        expect(md.defaultPrevented).toBe(false);
    });

    it('a press inside pulls focus back in if it had left the picker', () => {
        const p = make();
        document.body.focus();
        p.element.querySelector('.kdp-footer').dispatchEvent(
            new MouseEvent('mousedown', { bubbles: true, cancelable: true })
        );
        expect(document.activeElement.dataset.date).toBe('2026-11-04');
    });

    it('keeps focus on a grid cell when chrome buttons are clicked', () => {
        const p = make();
        p.focus();
        const md = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
        p.element.querySelector('.kdp-next').dispatchEvent(md);
        expect(md.defaultPrevented).toBe(true);
        p.element.querySelector('.kdp-next').click();
        expect(document.activeElement.classList.contains('kdp-day')).toBe(true);
        expect(document.activeElement.dataset.date).toBe('2026-12-04');
    });
});
