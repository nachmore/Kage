/**
 * Month-calendar date math (ui/js/shared/month-grid.js): leap years, month
 * and year paging with day clamping, grid generation for Sunday / Monday /
 * Saturday week starts, DST-change months in real time zones, keyboard
 * navigation, and locale formatting.
 */

import { afterEach, describe, expect, it } from 'vitest';
import {
    addDays,
    addMonths,
    addYears,
    buildMonthGrid,
    daysInMonth,
    formatDayNumber,
    formatFullDate,
    formatMonthYear,
    isLeapYear,
    isValidIsoDate,
    moveCellFocus,
    moveDayFocus,
    parseIsoDate,
    resolvePickerLocale,
    toIsoDate,
    weekdayHeaders,
    weekdayOf,
    weekStartFor,
    yearPage,
} from '../../ui/js/shared/month-grid.js';

const ORIGINAL_TZ = process.env.TZ;
function withTz(tz, fn) {
    process.env.TZ = tz;
    try {
        return fn();
    } finally {
        process.env.TZ = ORIGINAL_TZ;
    }
}
afterEach(() => {
    process.env.TZ = ORIGINAL_TZ;
});

describe('ISO day parsing', () => {
    it('accepts only real calendar dates in strict YYYY-MM-DD form', () => {
        expect(parseIsoDate('2026-11-05')).toEqual({ year: 2026, month: 10, day: 5 });
        expect(isValidIsoDate('2028-02-29')).toBe(true);
        for (const bad of ['2027-02-29', '2026-02-30', '2026-13-01', '2026-00-10', '2026-1-05', '2026/11/05', '', null, '2026-11-05T00:00']) {
            expect(isValidIsoDate(bad), String(bad)).toBe(false);
        }
    });

    it('round-trips through a local Date', () => {
        expect(toIsoDate(new Date(2026, 10, 5, 23, 59))).toBe('2026-11-05');
        expect(toIsoDate(new Date(2026, 0, 1, 0, 0))).toBe('2026-01-01');
    });
});

describe('leap years and month lengths', () => {
    it('follows the Gregorian rules', () => {
        expect(isLeapYear(2024)).toBe(true);
        expect(isLeapYear(2026)).toBe(false);
        expect(isLeapYear(1900)).toBe(false);
        expect(isLeapYear(2000)).toBe(true);
        expect(daysInMonth(2028, 1)).toBe(29);
        expect(daysInMonth(2027, 1)).toBe(28);
        expect(daysInMonth(2026, 10)).toBe(30);
        expect(daysInMonth(2026, 11)).toBe(31);
    });
});

describe('paging', () => {
    it('moves by days across month and year ends', () => {
        expect(addDays('2026-12-31', 1)).toBe('2027-01-01');
        expect(addDays('2028-02-28', 1)).toBe('2028-02-29');
        expect(addDays('2027-03-01', -1)).toBe('2027-02-28');
        expect(addDays('2026-11-05', 42)).toBe('2026-12-17');
    });

    it('clamps the day when paging months', () => {
        expect(addMonths('2026-01-31', 1)).toBe('2026-02-28');
        expect(addMonths('2028-01-31', 1)).toBe('2028-02-29');
        expect(addMonths('2026-03-31', -1)).toBe('2026-02-28');
        expect(addMonths('2026-12-15', 1)).toBe('2027-01-15');
        expect(addMonths('2026-01-15', -1)).toBe('2025-12-15');
        expect(addMonths('2026-05-10', -17)).toBe('2024-12-10');
    });

    it('clamps Feb 29 when paging years', () => {
        expect(addYears('2028-02-29', 1)).toBe('2029-02-28');
        expect(addYears('2028-02-29', 4)).toBe('2032-02-29');
        expect(addYears('2026-11-05', -10)).toBe('2016-11-05');
    });
});

describe('buildMonthGrid', () => {
    const days = (g) => g.weeks.flat().map((c) => c.iso);

    it('always has 6 complete weeks and a matching [start, end) range', () => {
        for (const [year, month] of [
            [2026, 1], // Feb 2026 starts on a Sunday
            [2026, 10],
            [2027, 1],
            [2028, 1],
        ]) {
            const g = buildMonthGrid({ year, month, weekStart: 1 });
            expect(g.weeks).toHaveLength(6);
            for (const w of g.weeks) expect(w).toHaveLength(7);
            const all = days(g);
            expect(all[0]).toBe(g.start);
            expect(addDays(all[41], 1)).toBe(g.end);
            // Contiguous.
            for (let i = 1; i < all.length; i++) expect(all[i]).toBe(addDays(all[i - 1], 1));
        }
    });

    it('starts weeks on Monday for weekStart=1', () => {
        const g = buildMonthGrid({ year: 2026, month: 10, weekStart: 1 }); // Nov 2026
        expect(g.start).toBe('2026-10-26'); // a Monday
        expect(g.weeks[0].map((c) => c.weekday)).toEqual([1, 2, 3, 4, 5, 6, 0]);
        expect(g.weeks[0].map((c) => c.day)).toEqual([26, 27, 28, 29, 30, 31, 1]);
        expect(g.weeks[0][6].inMonth).toBe(true);
        expect(g.weeks[0][5].inMonth).toBe(false);
    });

    it('starts weeks on Sunday for weekStart=0', () => {
        const g = buildMonthGrid({ year: 2026, month: 10, weekStart: 0 });
        expect(g.start).toBe('2026-11-01'); // Nov 1 2026 is a Sunday
        expect(g.weeks[0][0].day).toBe(1);
    });

    it('starts weeks on Saturday for weekStart=6', () => {
        const g = buildMonthGrid({ year: 2026, month: 10, weekStart: 6 });
        expect(g.start).toBe('2026-10-31');
        expect(g.weeks[0].map((c) => c.weekday)).toEqual([6, 0, 1, 2, 3, 4, 5]);
    });

    it('shows Feb 29 in a leap February', () => {
        const g = buildMonthGrid({ year: 2028, month: 1, weekStart: 1 });
        const feb = g.weeks.flat().filter((c) => c.inMonth);
        expect(feb).toHaveLength(29);
        expect(feb.at(-1).iso).toBe('2028-02-29');
    });

    it('flags today and the selected day', () => {
        const g = buildMonthGrid({
            year: 2026,
            month: 10,
            weekStart: 1,
            today: '2026-11-04',
            selected: '2026-11-26',
        });
        const flat = g.weeks.flat();
        expect(flat.filter((c) => c.isToday).map((c) => c.iso)).toEqual(['2026-11-04']);
        expect(flat.filter((c) => c.isSelected).map((c) => c.iso)).toEqual(['2026-11-26']);
    });

    it('stays contiguous across DST changes in real zones', () => {
        // US spring-forward (Mar 8 2026), EU fall-back (Oct 25 2026), and
        // Chile, where clocks jump at local MIDNIGHT (Sep 6 2026) — the day
        // a naive midnight-based Date arithmetic loses or repeats.
        for (const [tz, year, month] of [
            ['America/New_York', 2026, 2],
            ['Europe/London', 2026, 9],
            ['America/Santiago', 2026, 8],
            ['America/Santiago', 2026, 3],
        ]) {
            withTz(tz, () => {
                const g = buildMonthGrid({ year, month, weekStart: 1 });
                const all = g.weeks.flat();
                expect(all.filter((c) => c.inMonth)).toHaveLength(daysInMonth(year, month));
                for (let i = 1; i < all.length; i++) {
                    expect(all[i].iso, `${tz} ${all[i - 1].iso}`).toBe(addDays(all[i - 1].iso, 1));
                }
                expect(weekdayOf('2026-09-06')).toBe(0);
            });
        }
    });
});

describe('weekStartFor', () => {
    const cases = [
        ['en-US', 0],
        ['en-GB', 1],
        ['de-DE', 1],
        ['fr', 1],
        ['he-IL', 0],
        ['ja-JP', 0],
        ['ar-EG', 6],
        ['fa-IR', 6],
        ['pt-BR', 0],
    ];

    it.each(cases)('table fallback: %s → %i', (locale, expected) => {
        expect(weekStartFor(locale, { useIntl: false })).toBe(expected);
    });

    it('bare languages fall back to their likely region', () => {
        expect(weekStartFor('en', { useIntl: false })).toBe(0);
        expect(weekStartFor('de', { useIntl: false })).toBe(1);
        expect(weekStartFor('ar', { useIntl: false })).toBe(6);
    });

    it('agrees with Intl week info where the engine has it', () => {
        for (const [locale, expected] of cases) {
            expect(weekStartFor(locale), locale).toBe(expected);
        }
    });

    it('never throws on junk', () => {
        expect(weekStartFor('not a locale!!')).toBe(1);
    });
});

describe('resolvePickerLocale', () => {
    it('uses the OS region when it matches the UI language', () => {
        expect(resolvePickerLocale('en', 'en-GB')).toBe('en-GB');
        expect(resolvePickerLocale('de', 'de_AT')).toBe('de-AT');
    });

    it('uses the UI language when the OS speaks another one', () => {
        expect(resolvePickerLocale('ja', 'en-US')).toBe('ja');
        expect(resolvePickerLocale('fr', '')).toBe('fr');
    });
});

describe('moveDayFocus (keyboard)', () => {
    const from = '2026-11-05'; // Thursday

    it('arrows move by day and week', () => {
        expect(moveDayFocus(from, 'ArrowLeft')).toBe('2026-11-04');
        expect(moveDayFocus(from, 'ArrowRight')).toBe('2026-11-06');
        expect(moveDayFocus(from, 'ArrowUp')).toBe('2026-10-29');
        expect(moveDayFocus(from, 'ArrowDown')).toBe('2026-11-12');
    });

    it('left/right follow reading direction in RTL', () => {
        expect(moveDayFocus(from, 'ArrowLeft', { rtl: true })).toBe('2026-11-06');
        expect(moveDayFocus(from, 'ArrowRight', { rtl: true })).toBe('2026-11-04');
    });

    it('Home/End go to the edges of the locale week', () => {
        expect(moveDayFocus(from, 'Home', { weekStart: 1 })).toBe('2026-11-02');
        expect(moveDayFocus(from, 'End', { weekStart: 1 })).toBe('2026-11-08');
        expect(moveDayFocus(from, 'Home', { weekStart: 0 })).toBe('2026-11-01');
        expect(moveDayFocus(from, 'Home', { weekStart: 6 })).toBe('2026-10-31');
        expect(moveDayFocus(from, 'End', { weekStart: 6 })).toBe('2026-11-06');
    });

    it('PageUp/PageDown page months; with Shift, years', () => {
        expect(moveDayFocus('2026-03-31', 'PageUp')).toBe('2026-02-28');
        expect(moveDayFocus(from, 'PageDown')).toBe('2026-12-05');
        expect(moveDayFocus('2028-02-29', 'PageDown', { shiftKey: true })).toBe('2029-02-28');
        expect(moveDayFocus(from, 'PageUp', { shiftKey: true })).toBe('2025-11-05');
    });

    it('ignores other keys', () => {
        expect(moveDayFocus(from, 'Enter')).toBeNull();
        expect(moveDayFocus(from, 'a')).toBeNull();
    });
});

describe('moveCellFocus (month/year grids)', () => {
    it('moves through a 3-column grid and pages by 12', () => {
        expect(moveCellFocus(4, 'ArrowRight')).toBe(5);
        expect(moveCellFocus(4, 'ArrowRight', { rtl: true })).toBe(3);
        expect(moveCellFocus(4, 'ArrowUp')).toBe(1);
        expect(moveCellFocus(4, 'ArrowDown')).toBe(7);
        expect(moveCellFocus(4, 'PageUp')).toBe(-8);
        expect(moveCellFocus(4, 'End')).toBe(11);
        expect(moveCellFocus(4, 'Tab')).toBeNull();
    });
});

describe('yearPage', () => {
    it('is the decade plus one year either side', () => {
        const page = yearPage(2026);
        expect(page.map((y) => y.year)).toEqual([2019, 2020, 2021, 2022, 2023, 2024, 2025, 2026, 2027, 2028, 2029, 2030]);
        expect(page.filter((y) => !y.inDecade).map((y) => y.year)).toEqual([2019, 2030]);
        expect(yearPage(2030)[1].year).toBe(2030);
    });
});

describe('locale formatting', () => {
    it('orders weekday headers from the week start', () => {
        expect(weekdayHeaders('en-US', 1).map((w) => w.short)).toEqual(['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']);
        expect(weekdayHeaders('en-US', 6).map((w) => w.weekday)).toEqual([6, 0, 1, 2, 3, 4, 5]);
        expect(weekdayHeaders('de', 1)[0].long).toBe('Montag');
    });

    it('uses the Gregorian calendar but the locale digits', () => {
        // fa defaults to the Persian calendar and th to the Buddhist one —
        // the grid is Gregorian, so its title must be too.
        expect(formatMonthYear('fa', 2026, 10)).toMatch(/۲۰۲۶/);
        expect(formatMonthYear('th', 2026, 10)).toContain('2026');
        expect(formatDayNumber('fa', '2026-11-05')).toBe('۵');
        expect(formatDayNumber('en', '2026-11-05')).toBe('5');
    });

    it('spells out the full date for accessible names', () => {
        expect(formatFullDate('en-US', '2026-11-05')).toBe('Thursday, November 5, 2026');
        expect(formatMonthYear('en-US', 2026, 10)).toBe('November 2026');
    });
});
