/**
 * Month-calendar date picker — a host-owned DOM component.
 *
 * Three views share one header: days (6×7 grid), months (4×3) and years
 * (4×3, a decade plus one either side). Clicking the title steps up a level
 * (days → months → years → days); picking a month/year steps back down.
 *
 * Keyboard (WAI-ARIA APG date-grid pattern): arrows move by day/week (mirrored
 * in RTL), Home/End to the week's edges, PageUp/PageDown by month,
 * Shift+PageUp/PageDown by year, Enter/Space select, Escape closes. In the
 * month/year views the same keys move through the 3-column grid.
 *
 * Busy-day dots come from `loadMarkers(start, end)` (one call per visible
 * grid; see date-markers.js for validation + caching). The picker paints
 * immediately and fills dots in when the promise resolves — a slow calendar
 * never blocks it. Each lookup can spawn an OS calendar query (EventKit
 * helper / PowerShell + Outlook COM), so while the user pages: a cached
 * range (`peekMarkers`) paints at once, an uncached one is requested only
 * after `markerDebounceMs` without another page, and at most one lookup is
 * in flight — superseded months never reach the backend.
 *
 * Implementation note: every interactive element is created ONCE and
 * updated in place. Rebuilding the grid on each page would destroy the
 * focused button; in WebView2 removing the focused element can blur the
 * whole window, and the floating window hides on blur.
 */

import { t as hostT } from './i18n.js';
import {
    addDays,
    addMonths,
    addYears,
    buildMonthGrid,
    daysInMonth,
    formatDayNumber,
    formatFullDate,
    formatMonthYear,
    formatYear,
    isValidIsoDate,
    monthNames,
    moveCellFocus,
    moveDayFocus,
    parseIsoDate,
    todayIso,
    weekdayHeaders,
    yearPage,
} from './month-grid.js';

const CHEVRON_PATH = 'M15 18l-6-6 6-6';

let _instanceSeq = 0;

function svgChevron(doc) {
    const ns = 'http://www.w3.org/2000/svg';
    const svg = doc.createElementNS(ns, 'svg');
    svg.setAttribute('width', '14');
    svg.setAttribute('height', '14');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('fill', 'none');
    svg.setAttribute('stroke', 'currentColor');
    svg.setAttribute('stroke-width', '2.2');
    svg.setAttribute('stroke-linecap', 'round');
    svg.setAttribute('stroke-linejoin', 'round');
    svg.setAttribute('aria-hidden', 'true');
    const path = doc.createElementNS(ns, 'path');
    path.setAttribute('d', CHEVRON_PATH);
    svg.appendChild(path);
    return svg;
}

export class DatePicker {
    /**
     * @param {object} o
     * @param {string} o.locale       BCP-47 tag for names/digits
     * @param {number} o.weekStart    0 = Sunday … 6 = Saturday
     * @param {boolean} [o.rtl]
     * @param {string} [o.today]      ISO day (defaults to the local date)
     * @param {string} [o.selected]   ISO day to highlight + open on
     * @param {(start: string, end: string) => Promise<{status: string, days: Map}>} [o.loadMarkers]
     * @param {(start: string, end: string) => ({status: string, days: Map}|null)} [o.peekMarkers]
     *   synchronous cache lookup; a hit paints without waiting or debouncing
     * @param {number} [o.markerDebounceMs]  quiet time after paging before an
     *   uncached range is requested (the first range is requested at once)
     * @param {(iso: string) => void} [o.onSelect]
     * @param {() => void} [o.onClose]
     * @param {(key: string, vars?: object) => string} [o.t]
     * @param {Document} [o.document]
     */
    constructor(o) {
        this.locale = o.locale || 'en';
        this.weekStart = Number.isInteger(o.weekStart) ? o.weekStart : 0;
        this.rtl = !!o.rtl;
        this.today = isValidIsoDate(o.today) ? o.today : todayIso();
        this.selected = isValidIsoDate(o.selected) ? o.selected : null;
        this.loadMarkers = o.loadMarkers || null;
        this.peekMarkers = typeof o.peekMarkers === 'function' ? o.peekMarkers : null;
        this.markerDebounceMs = Number.isFinite(o.markerDebounceMs)
            ? Math.max(0, o.markerDebounceMs)
            : 180;
        this.onSelect = o.onSelect || (() => {});
        this.onClose = o.onClose || (() => {});
        this.t = o.t || hostT;
        this.doc = o.document || globalThis.document;

        /** Active (keyboard-focus) day; the month/year views derive from it. */
        this.focusIso = this.selected || this.today;
        /** @type {'days'|'months'|'years'} */
        this.view = 'days';
        /** Year shown by the months view (can differ from focusIso's while browsing). */
        this.viewYear = parseIsoDate(this.focusIso).year;
        /** First year of the years-view page. */
        this.pageYear = this.viewYear;

        this.markers = null; // { status, days } for the visible grid, once loaded
        this._markerRange = ''; // `start|end` of the visible grid
        this._markerTimer = null; // pending debounced request
        this._markerInFlight = null; // range key of the lookup in flight
        this._markerAsked = false; // the first range skips the debounce
        this._destroyed = false;

        this._id = `kdp-${++_instanceSeq}`;
        this._build();
        this._render();
    }

    /** The picker's root element (mount it wherever the host wants). */
    get element() {
        return this.root;
    }

    /** Move keyboard focus to the active cell of the current view. */
    focus() {
        this._activeCell()?.focus({ preventScroll: true });
    }

    /** Whether focus is currently inside the picker. */
    containsFocus() {
        return this.root.contains(this.doc.activeElement);
    }

    /**
     * Swap the weekday headers to their narrow forms when any short name
     * overflows its column (fa's short names are whole words; ar/bn/ur run
     * a pixel or two over). Measures, so call it once the picker is in the
     * DOM. The full name stays in each header's `abbr`.
     * @returns {boolean} whether the headers were narrowed
     */
    fitWeekdayHeaders() {
        const overflows = this.weekdayThs.some((th) => th.scrollWidth > th.clientWidth + 0.5);
        if (!overflows) return false;
        this.weekdayThs.forEach((th, i) => {
            th.textContent = this.weekdays[i].narrow;
            th.title = this.weekdays[i].long;
        });
        return true;
    }

    destroy() {
        this._destroyed = true;
        clearTimeout(this._markerTimer);
        this._markerTimer = null;
        this.root.remove();
    }

    // --- Construction -------------------------------------------------------

    _el(tag, cls, attrs) {
        const el = this.doc.createElement(tag);
        if (cls) el.className = cls;
        if (attrs) for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
        return el;
    }

    _build() {
        const t = this.t;
        const root = this._el('div', 'kage-date-picker', {
            role: 'dialog',
            'aria-modal': 'false',
            'aria-label': t('shared.date_picker.dialog_label'),
            dir: this.rtl ? 'rtl' : 'ltr',
            lang: this.locale,
        });
        this.root = root;

        // Header: ‹ title ›
        const header = this._el('div', 'kdp-header');
        this.prevBtn = this._el('button', 'kdp-nav kdp-prev', { type: 'button' });
        this.prevBtn.appendChild(svgChevron(this.doc));
        this.nextBtn = this._el('button', 'kdp-nav kdp-next', { type: 'button' });
        this.nextBtn.appendChild(svgChevron(this.doc));
        this.titleBtn = this._el('button', 'kdp-title', { type: 'button' });
        // Plain-text title for aria-labelledby + month-change announcements.
        this.srTitle = this._el('span', 'kdp-sr-only', {
            id: `${this._id}-title`,
            'aria-live': 'polite',
        });
        header.append(this.prevBtn, this.titleBtn, this.nextBtn, this.srTitle);

        // Days view
        const table = this._el('table', 'kdp-grid', {
            role: 'grid',
            'aria-labelledby': `${this._id}-title`,
        });
        const thead = this._el('thead');
        const headRow = this._el('tr');
        this.weekdays = weekdayHeaders(this.locale, this.weekStart);
        this.weekdayThs = this.weekdays.map((wd) => {
            const th = this._el('th', 'kdp-weekday', { scope: 'col', abbr: wd.long });
            if (wd.weekday === 0 || wd.weekday === 6) th.classList.add('kdp-weekend');
            th.textContent = wd.short;
            headRow.appendChild(th);
            return th;
        });
        thead.appendChild(headRow);
        const tbody = this._el('tbody');
        /** @type {Array<{ td: HTMLElement, btn: HTMLButtonElement, num: HTMLElement, dots: HTMLElement }>} */
        this.dayCells = [];
        for (let w = 0; w < 6; w++) {
            const tr = this._el('tr');
            for (let d = 0; d < 7; d++) {
                const td = this._el('td', 'kdp-cell', { role: 'gridcell' });
                const btn = this._el('button', 'kdp-day', { type: 'button', tabindex: '-1' });
                const num = this._el('span', 'kdp-num', { 'aria-hidden': 'true' });
                const dots = this._el('span', 'kdp-dots', { 'aria-hidden': 'true' });
                btn.append(num, dots);
                td.appendChild(btn);
                tr.appendChild(td);
                this.dayCells.push({ td, btn, num, dots });
            }
            tbody.appendChild(tr);
        }
        table.append(thead, tbody);
        this.daysView = table;

        // Months + years views: 4 rows × 3 cells each.
        const buildCells = (cls) => {
            const wrap = this._el('div', `kdp-cells ${cls}`, {
                role: 'grid',
                'aria-labelledby': `${this._id}-title`,
            });
            const cells = [];
            for (let r = 0; r < 4; r++) {
                const row = this._el('div', 'kdp-cells-row', { role: 'row' });
                for (let c = 0; c < 3; c++) {
                    const cell = this._el('div', 'kdp-cells-cell', { role: 'gridcell' });
                    const btn = this._el('button', 'kdp-pick', { type: 'button', tabindex: '-1' });
                    cell.appendChild(btn);
                    row.appendChild(cell);
                    cells.push({ cell, btn });
                }
                wrap.appendChild(row);
            }
            return { wrap, cells };
        };
        const months = buildCells('kdp-months');
        this.monthsView = months.wrap;
        this.monthCells = months.cells;
        const names = monthNames(this.locale);
        this.monthCells.forEach(({ btn }, i) => {
            btn.textContent = names[i].short;
            btn.setAttribute('aria-label', names[i].long);
            btn.dataset.month = String(i);
        });
        const years = buildCells('kdp-years');
        this.yearsView = years.wrap;
        this.yearCells = years.cells;

        // Footer: hint · Today
        const footer = this._el('div', 'kdp-footer');
        this.hint = this._el('span', 'kdp-hint', { role: 'status' });
        this.todayBtn = this._el('button', 'kdp-today', { type: 'button' });
        this.todayBtn.textContent = t('shared.date_picker.today');
        footer.append(this.hint, this.todayBtn);

        root.append(header, this.daysView, this.monthsView, this.yearsView, footer);

        // Events
        root.addEventListener('keydown', (e) => this._onKeyDown(e));
        // A mouse press anywhere but a grid cell must not move focus — not
        // onto the chrome buttons (prev/next/title/today), and not to <body>
        // when it lands on the weekday row, the gaps between days, the
        // padding or the footer. Focus stays on the grid cell, so the arrow
        // keys keep working after a click, Escape still reaches the picker
        // (on <body> it would hit the window's own Escape → hide), and
        // nothing focused is ever torn down.
        root.addEventListener('mousedown', (e) => {
            if (e.target.closest?.('.kdp-day, .kdp-pick')) return;
            e.preventDefault();
            if (!this.containsFocus()) this.focus();
        });
        this.prevBtn.addEventListener('click', () => this._page(-1));
        this.nextBtn.addEventListener('click', () => this._page(1));
        this.titleBtn.addEventListener('click', () => this._zoomOut());
        this.todayBtn.addEventListener('click', () => this.goToToday());
        tbody.addEventListener('click', (e) => {
            const btn = e.target.closest?.('.kdp-day');
            if (btn?.dataset.date) this._select(btn.dataset.date);
        });
        this.monthsView.addEventListener('click', (e) => {
            const btn = e.target.closest?.('.kdp-pick');
            if (btn?.dataset.month) this._pickMonth(Number(btn.dataset.month));
        });
        this.yearsView.addEventListener('click', (e) => {
            const btn = e.target.closest?.('.kdp-pick');
            if (btn?.dataset.year) this._pickYear(Number(btn.dataset.year));
        });
    }

    // --- Public navigation (also used by tests) -----------------------------

    /** Show the month containing `iso`, focusing that day. */
    showDate(iso) {
        if (!isValidIsoDate(iso)) return;
        this.focusIso = iso;
        this.viewYear = parseIsoDate(iso).year;
        this._setView('days');
    }

    goToToday() {
        this.showDate(this.today);
    }

    /** The month currently on screen as `{ year, month }`. */
    get visibleMonth() {
        const p = parseIsoDate(this.focusIso);
        return { year: p.year, month: p.month };
    }

    // --- Internals ------------------------------------------------------------

    /** Focus is on a grid cell (day / month / year), not on the chrome. */
    _focusInGrid() {
        const a = this.doc.activeElement;
        return (
            !!a &&
            this.root.contains(a) &&
            !!a.classList?.contains?.(this.view === 'days' ? 'kdp-day' : 'kdp-pick')
        );
    }

    _setView(view) {
        // Clicking chrome never moves focus (mousedown is suppressed), so if
        // focus sat on a cell it follows to the new view's active cell.
        const follow = this._focusInGrid() || this.view !== view;
        const hadFocus = this.containsFocus();
        this.view = view;
        this._render({ focus: hadFocus && follow });
    }

    _select(iso) {
        if (!isValidIsoDate(iso)) return;
        this.selected = iso;
        this.focusIso = iso;
        this._render();
        this.onSelect(iso);
    }

    _page(dir) {
        if (this.view === 'days') this.focusIso = addMonths(this.focusIso, dir);
        else if (this.view === 'months') this.viewYear += dir;
        else this.pageYear += dir * 10;
        // Keyboard users on the ‹ › buttons keep their place there.
        this._render({ focus: this._focusInGrid() });
    }

    _zoomOut() {
        if (this.view === 'days') {
            this.viewYear = parseIsoDate(this.focusIso).year;
            this._setView('months');
        } else if (this.view === 'months') {
            this.pageYear = this.viewYear;
            this._setView('years');
        } else {
            this._setView('days');
        }
    }

    _pickMonth(month) {
        const p = parseIsoDate(this.focusIso);
        const day = Math.min(p.day, daysInMonth(this.viewYear, month));
        this.focusIso = `${String(this.viewYear).padStart(4, '0')}-${String(month + 1).padStart(2, '0')}-${String(day).padStart(2, '0')}`;
        this._setView('days');
    }

    _pickYear(year) {
        this.viewYear = year;
        this._setView('months');
    }

    _activeCell() {
        if (this.view === 'days') {
            return this.dayCells.find((c) => c.btn.dataset.date === this.focusIso)?.btn || null;
        }
        if (this.view === 'months') return this.monthCells[this._monthActiveIndex()]?.btn || null;
        return this.yearCells[this._yearActiveIndex()]?.btn || null;
    }

    _monthActiveIndex() {
        if (this._monthFocus != null) return this._monthFocus;
        const m = parseIsoDate(this.focusIso);
        return this.viewYear === m.year ? m.month : 0;
    }

    _yearActiveIndex() {
        if (this._yearFocus != null) return this._yearFocus;
        const idx = yearPage(this.pageYear).findIndex((y) => y.year === this.viewYear);
        return idx >= 0 ? idx : 1;
    }

    _onKeyDown(e) {
        if (e.key === 'Escape') {
            e.preventDefault();
            e.stopPropagation();
            this.onClose();
            return;
        }
        const target = e.target;
        if (this.view === 'days' && target.classList?.contains('kdp-day')) {
            const next = moveDayFocus(this.focusIso, e.key, {
                shiftKey: e.shiftKey,
                rtl: this.rtl,
                weekStart: this.weekStart,
            });
            if (next) {
                e.preventDefault();
                e.stopPropagation();
                this.focusIso = next;
                this._render({ focus: true });
            }
            // Enter/Space fall through to the button's native click.
            return;
        }
        if (this.view !== 'days' && target.classList?.contains('kdp-pick')) {
            const cells = this.view === 'months' ? this.monthCells : this.yearCells;
            const current = cells.findIndex((c) => c.btn === target);
            const next = moveCellFocus(current, e.key, { rtl: this.rtl });
            if (next === null) return;
            e.preventDefault();
            e.stopPropagation();
            if (this.view === 'months') {
                if (next < 0 || next > 11) {
                    this.viewYear += next < 0 ? -1 : 1;
                    this._monthFocus = ((next % 12) + 12) % 12;
                } else {
                    this._monthFocus = next;
                }
            } else if (next < 0 || next > 11) {
                this.pageYear += next < 0 ? -10 : 10;
                this._yearFocus = ((next % 12) + 12) % 12;
            } else {
                this._yearFocus = next;
            }
            this._render({ focus: true });
        }
    }

    /**
     * Repaint in place. With `focus`, the active cell is focused BEFORE the
     * other views are hidden, so a focused element is never hidden or removed
     * (see the file comment).
     */
    _render({ focus = false } = {}) {
        const t = this.t;
        const { year, month } = this.visibleMonth;
        const views = { days: this.daysView, months: this.monthsView, years: this.yearsView };
        views[this.view].hidden = false;
        this.root.dataset.view = this.view;
        if (this.view !== 'months') this._monthFocus = null;
        if (this.view !== 'years') this._yearFocus = null;
        this._paint(t, year, month);
        if (focus) this.focus();
        for (const [name, el] of Object.entries(views)) {
            if (name !== this.view) el.hidden = true;
        }
    }

    _paint(t, year, month) {
        let title;
        if (this.view === 'days') {
            title = formatMonthYear(this.locale, year, month);
            this.prevBtn.setAttribute('aria-label', t('shared.date_picker.prev_month'));
            this.nextBtn.setAttribute('aria-label', t('shared.date_picker.next_month'));
            this.titleBtn.setAttribute('aria-label', t('shared.date_picker.title_days', { title }));
            this._renderDays(year, month);
        } else if (this.view === 'months') {
            title = formatYear(this.locale, this.viewYear);
            this.prevBtn.setAttribute('aria-label', t('shared.date_picker.prev_year'));
            this.nextBtn.setAttribute('aria-label', t('shared.date_picker.next_year'));
            this.titleBtn.setAttribute(
                'aria-label',
                t('shared.date_picker.title_months', { title })
            );
            this._renderMonths();
        } else {
            const page = yearPage(this.pageYear);
            title = `${formatYear(this.locale, page[1].year)} – ${formatYear(this.locale, page[10].year)}`;
            this.prevBtn.setAttribute('aria-label', t('shared.date_picker.prev_decade'));
            this.nextBtn.setAttribute('aria-label', t('shared.date_picker.next_decade'));
            this.titleBtn.setAttribute(
                'aria-label',
                t('shared.date_picker.title_years', { title })
            );
            this._renderYears(page);
        }
        for (const b of [this.prevBtn, this.nextBtn, this.titleBtn]) {
            b.title = b.getAttribute('aria-label');
        }
        this.titleBtn.textContent = title;
        if (this.srTitle.textContent !== title) this.srTitle.textContent = title;
        this.todayBtn.setAttribute(
            'aria-label',
            t('shared.date_picker.today_label', {
                date: formatFullDate(this.locale, this.today),
            })
        );
        this._renderHint();
    }

    _renderDays(year, month) {
        const grid = buildMonthGrid({
            year,
            month,
            weekStart: this.weekStart,
            today: this.today,
            selected: this.selected || '',
        });
        const cells = grid.weeks.flat();
        cells.forEach((c, i) => {
            const { td, btn, num } = this.dayCells[i];
            btn.dataset.date = c.iso;
            num.textContent = formatDayNumber(this.locale, c.iso);
            btn.tabIndex = c.iso === this.focusIso ? 0 : -1;
            btn.classList.toggle('is-outside', !c.inMonth);
            btn.classList.toggle('is-today', c.isToday);
            btn.classList.toggle('is-selected', c.isSelected);
            btn.classList.toggle('is-weekend', c.weekday === 0 || c.weekday === 6);
            td.setAttribute('aria-selected', c.isSelected ? 'true' : 'false');
            if (c.isToday) btn.setAttribute('aria-current', 'date');
            else btn.removeAttribute('aria-current');
        });
        const range = `${grid.start}|${grid.end}`;
        if (range !== this._markerRange) {
            this._markerRange = range;
            this.markers = null;
            this._requestMarkers(grid.start, grid.end);
        }
        this._applyMarkers();
    }

    _renderMonths() {
        const now = parseIsoDate(this.today);
        const sel = this.selected ? parseIsoDate(this.selected) : null;
        const active = this._monthActiveIndex();
        this.monthCells.forEach(({ cell, btn }, i) => {
            btn.tabIndex = i === active ? 0 : -1;
            const isSel = !!sel && sel.year === this.viewYear && sel.month === i;
            btn.classList.toggle('is-selected', isSel);
            btn.classList.toggle('is-today', now.year === this.viewYear && now.month === i);
            cell.setAttribute('aria-selected', isSel ? 'true' : 'false');
        });
    }

    _renderYears(page) {
        const now = parseIsoDate(this.today).year;
        const sel = this.selected ? parseIsoDate(this.selected).year : null;
        const active = this._yearActiveIndex();
        page.forEach((y, i) => {
            const { cell, btn } = this.yearCells[i];
            btn.dataset.year = String(y.year);
            btn.textContent = formatYear(this.locale, y.year);
            btn.tabIndex = i === active ? 0 : -1;
            btn.classList.toggle('is-outside', !y.inDecade);
            btn.classList.toggle('is-selected', sel === y.year);
            btn.classList.toggle('is-today', now === y.year);
            cell.setAttribute('aria-selected', sel === y.year ? 'true' : 'false');
        });
    }

    /** The visible grid changed to `[start, end)`: paint cached dots or schedule a lookup. */
    _requestMarkers(start, end) {
        if (!this.loadMarkers) return;
        clearTimeout(this._markerTimer);
        this._markerTimer = null;
        if (this._receiveCached(start, end)) return;
        this.daysView.setAttribute('aria-busy', 'true');
        const first = !this._markerAsked;
        this._markerAsked = true;
        if (first || this.markerDebounceMs === 0) {
            this._flushMarkers();
            return;
        }
        this._markerTimer = setTimeout(() => {
            this._markerTimer = null;
            this._flushMarkers();
        }, this.markerDebounceMs);
    }

    /** Paint a synchronous cache hit for `[start, end)`; true if there was one. */
    _receiveCached(start, end) {
        const hit = this.peekMarkers ? this.peekMarkers(start, end) : null;
        if (!hit || !(hit.days instanceof Map)) return false;
        this._receiveMarkers(hit);
        return true;
    }

    /** Look up the visible grid's markers — unless a lookup is already running. */
    _flushMarkers() {
        if (this._destroyed || !this._markerRange) return;
        // One lookup at a time: when it lands, it re-flushes for whatever
        // range is visible by then (see below), skipping every month paged
        // past in between.
        if (this._markerInFlight) return;
        const range = this._markerRange;
        const [start, end] = range.split('|');
        if (this._receiveCached(start, end)) return;
        this._markerInFlight = range;
        let pending;
        try {
            pending = Promise.resolve(this.loadMarkers(start, end));
        } catch (e) {
            pending = Promise.reject(e);
        }
        pending
            .catch(() => null)
            .then((m) => {
                this._markerInFlight = null;
                if (this._destroyed) return;
                if (range === this._markerRange) {
                    clearTimeout(this._markerTimer);
                    this._markerTimer = null;
                    this._receiveMarkers(m);
                } else if (!this._markerTimer) {
                    // Paged away meanwhile and the debounce has already
                    // elapsed: fetch the month that's on screen now.
                    this._flushMarkers();
                }
            });
    }

    _receiveMarkers(m) {
        this.markers = m && m.days instanceof Map ? m : { status: 'error', days: new Map() };
        this.daysView.removeAttribute('aria-busy');
        this._applyMarkers();
        this._renderHint();
    }

    /** Paint dots + accessible names from the loaded markers (or just dates). */
    _applyMarkers() {
        const loaded = this.markers?.status === 'ok' ? this.markers.days : null;
        for (const { btn, dots } of this.dayCells) {
            const iso = btn.dataset.date;
            if (!iso) continue;
            const m = loaded?.get(iso) || null;
            dots.replaceChildren();
            if (m && m.count > 0) {
                const colors = m.colors?.length ? m.colors.slice(0, 3) : [null];
                for (const c of colors) {
                    const dot = this._el('span', 'kdp-dot');
                    if (c) dot.style.backgroundColor = c;
                    dots.appendChild(dot);
                }
            }
            btn.classList.toggle('has-events', !!(m && m.count > 0));
            const date = formatFullDate(this.locale, iso);
            btn.setAttribute(
                'aria-label',
                loaded
                    ? this.t('shared.date_picker.day_meetings', { date, count: m?.count || 0 })
                    : date
            );
        }
    }

    _renderHint() {
        const status = this.view === 'days' ? this.markers?.status : null;
        let text = '';
        if (status === 'permission_denied') text = this.t('shared.date_picker.hint.permission');
        else if (status === 'no_provider') text = this.t('shared.date_picker.hint.no_provider');
        else if (status === 'provider_outdated') {
            text = this.t('shared.date_picker.hint.update_provider');
        }
        if (this.hint.textContent !== text) this.hint.textContent = text;
        this.hint.hidden = !text;
    }
}

// Re-exported for callers that only need the navigation helpers.
export { addDays, addMonths, addYears };
