import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
    parseContextPercent,
    contextRingColor,
    contextRingTrackColor,
    drawContextRing,
} from '../../ui/js/shared/context-usage.js';

describe('parseContextPercent', () => {
    it('parses a percent out of the message field', () => {
        expect(parseContextPercent({ message: 'Context: 42% used' })).toBe(42);
    });

    it('parses 0 and 100 at the bounds', () => {
        expect(parseContextPercent({ message: '0% used' })).toBe(0);
        expect(parseContextPercent({ message: '100% of the window' })).toBe(100);
    });

    it('falls back to stringifying when there is no message field', () => {
        // Some agents return a bare object; the parser stringifies it.
        expect(parseContextPercent({ usage: '77%' })).toBe(77);
    });

    it('takes the first percent token when several are present', () => {
        expect(parseContextPercent({ message: '12% of 90% budget' })).toBe(12);
    });

    it('returns null when no percent token is present', () => {
        expect(parseContextPercent({ message: 'no number here' })).toBeNull();
    });

    it('returns null for null / undefined input', () => {
        expect(parseContextPercent(null)).toBeNull();
        expect(parseContextPercent(undefined)).toBeNull();
    });

    it('handles a plain string result', () => {
        // JSON.stringify('55%') === '"55%"' — the digits still match.
        expect(parseContextPercent('55%')).toBe(55);
    });
});

describe('contextRingColor', () => {
    it('is green below 75', () => {
        expect(contextRingColor(0)).toBe('#22c55e');
        expect(contextRingColor(74)).toBe('#22c55e');
    });

    it('is yellow from 75 to 89', () => {
        expect(contextRingColor(75)).toBe('#eab308');
        expect(contextRingColor(89)).toBe('#eab308');
    });

    it('is red at 90 and above', () => {
        expect(contextRingColor(90)).toBe('#ef4444');
        expect(contextRingColor(100)).toBe('#ef4444');
    });
});

describe('contextRingTrackColor', () => {
    beforeEach(() => {
        document.body.className = '';
    });

    it('uses a light track on dark theme', () => {
        document.body.classList.add('dark-theme');
        expect(contextRingTrackColor()).toBe('rgba(255,255,255,0.15)');
    });

    it('uses a dark track on light theme', () => {
        document.body.classList.add('light-theme');
        expect(contextRingTrackColor()).toBe('rgba(0,0,0,0.1)');
    });

    it('defaults to the light theme track when no theme class is set', () => {
        expect(contextRingTrackColor()).toBe('rgba(0,0,0,0.1)');
    });
});

describe('drawContextRing', () => {
    it('is a no-op for a null canvas', () => {
        expect(() => drawContextRing(null, 50)).not.toThrow();
    });

    it('is a no-op when getContext returns null', () => {
        const canvas = { getContext: () => null };
        expect(() => drawContextRing(canvas, 50)).not.toThrow();
    });

    it('paints the track plus the arc when percent > 0', () => {
        const calls = [];
        const ctx = new Proxy(
            {},
            {
                get(_t, prop) {
                    if (prop === 'arc' || prop === 'stroke' || prop === 'beginPath') {
                        return (...args) => calls.push([prop, ...args]);
                    }
                    // clearRect and property setters (strokeStyle, lineWidth…)
                    return typeof prop === 'string' ? () => {} : undefined;
                },
                set() {
                    return true;
                },
            }
        );
        const canvas = { getContext: () => ctx };
        drawContextRing(canvas, 50);
        const strokes = calls.filter((c) => c[0] === 'stroke').length;
        // One stroke for the track, one for the usage arc.
        expect(strokes).toBe(2);
    });

    it('paints only the track when percent is 0', () => {
        let strokes = 0;
        const ctx = {
            clearRect() {},
            beginPath() {},
            arc() {},
            stroke() {
                strokes++;
            },
        };
        const canvas = { getContext: () => ctx };
        drawContextRing(canvas, 0);
        expect(strokes).toBe(1);
    });

    it('honours an explicit track override', () => {
        let track = null;
        const ctx = {
            clearRect() {},
            beginPath() {},
            arc() {},
            stroke() {},
            set strokeStyle(v) {
                // first set is the track colour
                if (track === null) track = v;
            },
            get strokeStyle() {
                return track;
            },
            set lineWidth(_v) {},
            set lineCap(_v) {},
        };
        const canvas = { getContext: () => ctx };
        drawContextRing(canvas, 50, { track: '#abcdef' });
        expect(track).toBe('#abcdef');
    });
});
