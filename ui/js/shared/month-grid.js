/**
 * Month-calendar date math for the date picker — pure, DOM-free, unit-tested
 * (ui-tests/shared/month-grid.test.js).
 *
 * Dates travel as local-time ISO day strings (`YYYY-MM-DD`): no time, no
 * zone. That's the unit the calendar backend speaks (get_calendar_events_for_date,
 * get_calendar_busy_days) and it sidesteps every DST trap a `Date` invites.
 * When arithmetic needs a `Date`, it's built at local NOON: midnight doesn't
 * exist on some DST-change days in some zones, noon always does, and ±1 day
 * from noon can never land on the wrong calendar date.
 *
 * Formatting always uses the Gregorian calendar (`calendar: 'gregory'`), even
 * for locales whose default is another (fa → Persian, th → Buddhist): the grid
 * is Gregorian because event dates are, and a Persian month name over a
 * Gregorian grid would be wrong. Digits still follow the locale (fa → ۱۲۳).
 */

const ISO_DAY = /^(\d{4})-(\d{2})-(\d{2})$/;

/** Pad to two digits. */
function p2(n) {
    return String(n).padStart(2, '0');
}

/** Local-noon `Date` for a (year, 0-based month, day). Overflowing days roll. */
function noon(year, month0, day) {
    const d = new Date(year, month0, day, 12, 0, 0, 0);
    // new Date(y, …) maps years 0–99 to 1900–1999; set it explicitly.
    if (year >= 0 && year < 100) d.setFullYear(year, month0, day);
    return d;
}

/** `YYYY-MM-DD` of a `Date`, in local time. */
export function toIsoDate(date) {
    return `${String(date.getFullYear()).padStart(4, '0')}-${p2(date.getMonth() + 1)}-${p2(date.getDate())}`;
}

/** Today's local date as `YYYY-MM-DD`. */
export function todayIso(now = new Date()) {
    return toIsoDate(now);
}

/**
 * Parse `YYYY-MM-DD` into `{ year, month (0-based), day }`, or null when the
 * string isn't a real calendar date (`2026-02-30`, `2026-2-1`, …).
 */
export function parseIsoDate(iso) {
    const m = typeof iso === 'string' ? ISO_DAY.exec(iso) : null;
    if (!m) return null;
    const year = Number(m[1]);
    const month = Number(m[2]) - 1;
    const day = Number(m[3]);
    if (month < 0 || month > 11 || day < 1 || day > daysInMonth(year, month)) return null;
    return { year, month, day };
}

export function isValidIsoDate(iso) {
    return parseIsoDate(iso) !== null;
}

export function isLeapYear(year) {
    return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
}

/** Days in a 0-based month. */
export function daysInMonth(year, month0) {
    return [31, isLeapYear(year) ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month0];
}

/** Local `Date` (noon) for an ISO day. Throws on an invalid date. */
export function isoToDate(iso) {
    const p = parseIsoDate(iso);
    if (!p) throw new RangeError(`invalid ISO date: ${iso}`);
    return noon(p.year, p.month, p.day);
}

export function addDays(iso, n) {
    const p = parseIsoDate(iso);
    return toIsoDate(noon(p.year, p.month, p.day + n));
}

/**
 * Shift by whole months, clamping the day to the target month's length
 * (Jan 31 + 1 month → Feb 28/29, never Mar 3).
 */
export function addMonths(iso, n) {
    const p = parseIsoDate(iso);
    const total = p.year * 12 + p.month + n;
    const year = Math.floor(total / 12);
    const month = ((total % 12) + 12) % 12;
    const day = Math.min(p.day, daysInMonth(year, month));
    return `${String(year).padStart(4, '0')}-${p2(month + 1)}-${p2(day)}`;
}

/** Shift by whole years (Feb 29 + 1 year → Feb 28). */
export function addYears(iso, n) {
    return addMonths(iso, n * 12);
}

/** Day of week of an ISO day, 0 = Sunday … 6 = Saturday. */
export function weekdayOf(iso) {
    return isoToDate(iso).getDay();
}

/** Compare ISO days (they sort lexicographically). */
export function compareIso(a, b) {
    return a < b ? -1 : a > b ? 1 : 0;
}

// --- First day of the week -------------------------------------------------

// CLDR `weekData.firstDay` for the regions that don't start on Monday — the
// fallback when the engine lacks Intl.Locale week info. Monday is the
// CLDR world default.
const SUNDAY_REGIONS = new Set(
    (
        'AG AS BD BR BS BT BW BZ CA CN CO DM DO ET GT GU HK HN ID IL IN JM JP KE KH KR LA MH MM MO ' +
        'MT MX MZ NI NP PA PE PH PK PR PT PY SA SG SV TH TT TW UM US VE VI WS YE ZA ZW'
    ).split(' ')
);
const SATURDAY_REGIONS = new Set('AE AF BH DJ DZ EG IQ IR JO KW LY OM QA SD SY'.split(' '));
const FRIDAY_REGIONS = new Set(['MV']);

/** Likely region for a bare language when Intl.Locale#maximize is missing. */
const LANGUAGE_DEFAULT_REGION = {
    en: 'US',
    ja: 'JP',
    ko: 'KR',
    zh: 'CN',
    he: 'IL',
    ar: 'EG',
    fa: 'IR',
    hi: 'IN',
    bn: 'BD',
    th: 'TH',
    pt: 'BR',
    id: 'ID',
    ur: 'PK',
    vi: 'VN',
};

function regionOf(locale) {
    try {
        const loc = new Intl.Locale(locale);
        if (loc.region) return loc.region;
        const max = typeof loc.maximize === 'function' ? loc.maximize() : null;
        if (max?.region) return max.region;
        return LANGUAGE_DEFAULT_REGION[loc.language] || null;
    } catch {
        return LANGUAGE_DEFAULT_REGION[String(locale).split('-')[0]] || null;
    }
}

/**
 * First day of the week for a locale, as a JS weekday (0 = Sunday … 6 =
 * Saturday). Uses `Intl.Locale#getWeekInfo` / `#weekInfo` when the engine has
 * it (honouring `-u-fw-` overrides), otherwise the CLDR region table.
 *
 * @param {string} locale  BCP-47 tag, e.g. `en-US`, `de`, `ar-EG`
 * @param {{ useIntl?: boolean }} [opts]  `useIntl: false` forces the table
 *   (tests use it to cover engines without week info)
 */
export function weekStartFor(locale, { useIntl = true } = {}) {
    if (useIntl) {
        try {
            const loc = new Intl.Locale(locale);
            const info = typeof loc.getWeekInfo === 'function' ? loc.getWeekInfo() : loc.weekInfo;
            const first = Number(info?.firstDay);
            // Intl uses ISO numbering: 1 = Monday … 7 = Sunday.
            if (first >= 1 && first <= 7) return first % 7;
        } catch {
            /* fall through to the table */
        }
    }
    const region = regionOf(locale);
    if (region && SUNDAY_REGIONS.has(region)) return 0;
    if (region && SATURDAY_REGIONS.has(region)) return 6;
    if (region && FRIDAY_REGIONS.has(region)) return 5;
    return 1;
}

/**
 * The locale the picker formats with: the OS locale when it's a regional
 * flavour of the active UI language (so an en-GB user gets Monday weeks and
 * day-month order), else the UI language itself.
 */
export function resolvePickerLocale(activeLanguage, systemLanguage) {
    const active = String(activeLanguage || 'en');
    const system = String(systemLanguage || '');
    const stem = (s) => s.toLowerCase().split(/[-_]/)[0];
    if (system && stem(system) === stem(active)) {
        try {
            return new Intl.Locale(system.replace(/_/g, '-')).toString();
        } catch {
            /* malformed OS tag — use the UI language */
        }
    }
    return active;
}

// --- Grid ------------------------------------------------------------------

/**
 * The 6×7 day grid for a month. Always six weeks, so the picker keeps a
 * stable height while paging (no window resize per month).
 *
 * @param {object} o
 * @param {number} o.year
 * @param {number} o.month     0-based
 * @param {number} o.weekStart 0 = Sunday … 6 = Saturday
 * @param {string} [o.today]   ISO day to flag `isToday`
 * @param {string} [o.selected] ISO day to flag `isSelected`
 * @returns {{ year:number, month:number, start:string, end:string,
 *   weeks: Array<Array<{ iso:string, day:number, weekday:number,
 *   inMonth:boolean, isToday:boolean, isSelected:boolean }>> }}
 *   `start`/`end` bound the visible cells (`end` exclusive) — the range the
 *   busy-day lookup asks for.
 */
export function buildMonthGrid({ year, month, weekStart = 0, today = '', selected = '' }) {
    const first = noon(year, month, 1);
    const lead = (first.getDay() - weekStart + 7) % 7;
    const start = toIsoDate(noon(year, month, 1 - lead));
    const weeks = [];
    let iso = start;
    for (let w = 0; w < 6; w++) {
        const week = [];
        for (let i = 0; i < 7; i++) {
            const p = parseIsoDate(iso);
            week.push({
                iso,
                day: p.day,
                weekday: (weekStart + i) % 7,
                inMonth: p.year === year && p.month === month,
                isToday: iso === today,
                isSelected: iso === selected,
            });
            iso = addDays(iso, 1);
        }
        weeks.push(week);
    }
    return { year, month, start, end: iso, weeks };
}

/**
 * Twelve-year page for the year view: the decade containing `year`, with one
 * year either side (2019 … 2030 for 2026) so the grid fills 4×3.
 */
export function yearPage(year) {
    const start = Math.floor(year / 10) * 10 - 1;
    return Array.from({ length: 12 }, (_, i) => ({
        year: start + i,
        inDecade: i > 0 && i < 11,
    }));
}

// --- Keyboard navigation ---------------------------------------------------

/**
 * Where a key moves the focused day. Arrow keys move by day/week (left/right
 * swap in RTL so "forward" follows reading direction), Home/End jump to the
 * start/end of the week, PageUp/PageDown by month and Shift+PageUp/PageDown
 * by year. Returns null for keys that don't navigate.
 */
export function moveDayFocus(iso, key, { shiftKey = false, rtl = false, weekStart = 0 } = {}) {
    switch (key) {
        case 'ArrowLeft':
            return addDays(iso, rtl ? 1 : -1);
        case 'ArrowRight':
            return addDays(iso, rtl ? -1 : 1);
        case 'ArrowUp':
            return addDays(iso, -7);
        case 'ArrowDown':
            return addDays(iso, 7);
        case 'Home':
            return addDays(iso, -((weekdayOf(iso) - weekStart + 7) % 7));
        case 'End':
            return addDays(iso, 6 - ((weekdayOf(iso) - weekStart + 7) % 7));
        case 'PageUp':
            return shiftKey ? addYears(iso, -1) : addMonths(iso, -1);
        case 'PageDown':
            return shiftKey ? addYears(iso, 1) : addMonths(iso, 1);
        default:
            return null;
    }
}

/**
 * Index movement inside the 3-column month/year grids. `count` cells, `pageSize`
 * for PageUp/PageDown. Returns the new index (may be out of range — the caller
 * pages) or null for non-navigation keys.
 */
export function moveCellFocus(index, key, { rtl = false, columns = 3, pageSize = 12 } = {}) {
    switch (key) {
        case 'ArrowLeft':
            return index + (rtl ? 1 : -1);
        case 'ArrowRight':
            return index + (rtl ? -1 : 1);
        case 'ArrowUp':
            return index - columns;
        case 'ArrowDown':
            return index + columns;
        case 'Home':
            return 0;
        case 'End':
            return pageSize - 1;
        case 'PageUp':
            return index - pageSize;
        case 'PageDown':
            return index + pageSize;
        default:
            return null;
    }
}

// --- Locale formatting -----------------------------------------------------

const _fmtCache = new Map();

function fmt(locale, options) {
    const key = locale + '|' + JSON.stringify(options);
    let f = _fmtCache.get(key);
    if (!f) {
        try {
            f = new Intl.DateTimeFormat(locale, { calendar: 'gregory', ...options });
        } catch {
            f = new Intl.DateTimeFormat('en', options);
        }
        _fmtCache.set(key, f);
    }
    return f;
}

/** Weekday column headers starting at `weekStart`: `{ narrow, short, long }`. */
export function weekdayHeaders(locale, weekStart) {
    // 2024-01-07 is a Sunday.
    return Array.from({ length: 7 }, (_, i) => {
        const wd = (weekStart + i) % 7;
        const d = noon(2024, 0, 7 + wd);
        return {
            weekday: wd,
            narrow: fmt(locale, { weekday: 'narrow' }).format(d),
            short: fmt(locale, { weekday: 'short' }).format(d),
            long: fmt(locale, { weekday: 'long' }).format(d),
        };
    });
}

/** The 12 month names: `{ short, long }`. */
export function monthNames(locale) {
    return Array.from({ length: 12 }, (_, m) => {
        const d = noon(2024, m, 15);
        return {
            short: fmt(locale, { month: 'short' }).format(d),
            long: fmt(locale, { month: 'long' }).format(d),
        };
    });
}

/** "November 2026" in the locale's own order and script. */
export function formatMonthYear(locale, year, month) {
    return fmt(locale, { month: 'long', year: 'numeric' }).format(noon(year, month, 15));
}

/** A year in the locale's digits ("2026", "۲۰۲۶"). */
export function formatYear(locale, year) {
    return fmt(locale, { year: 'numeric' }).format(noon(year, 6, 1));
}

/** A day-of-month number in the locale's digits. */
export function formatDayNumber(locale, iso) {
    return fmt(locale, { day: 'numeric' }).format(isoToDate(iso));
}

/** "Thursday, November 5, 2026" — the accessible name of a day cell. */
export function formatFullDate(locale, iso) {
    return fmt(locale, {
        weekday: 'long',
        year: 'numeric',
        month: 'long',
        day: 'numeric',
    }).format(isoToDate(iso));
}
