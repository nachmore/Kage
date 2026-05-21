import { describe, it, expect } from 'vitest';
import { parseColor, rgbToHex, rgbToHsl, formatAllColors } from '../../ui/js/floating/color.js';

describe('parseColor', () => {
  // --- Hex ---
  it('parses 6-digit hex', () => {
    const c = parseColor('#ff8800');
    expect(c).toEqual({ r: 255, g: 136, b: 0, source: 'hex' });
  });

  it('parses 3-digit hex', () => {
    const c = parseColor('#f80');
    expect(c).toEqual({ r: 255, g: 136, b: 0, source: 'hex' });
  });

  it('parses uppercase hex', () => {
    const c = parseColor('#FF0000');
    expect(c).toEqual({ r: 255, g: 0, b: 0, source: 'hex' });
  });

  // --- RGB ---
  it('parses rgb() with commas', () => {
    const c = parseColor('rgb(100, 200, 50)');
    expect(c).toEqual({ r: 100, g: 200, b: 50, source: 'rgb' });
  });

  it('parses rgb() with spaces', () => {
    const c = parseColor('rgb(100 200 50)');
    expect(c).toEqual({ r: 100, g: 200, b: 50, source: 'rgb' });
  });

  it('rejects rgb values > 255', () => {
    expect(parseColor('rgb(300, 0, 0)')).toBeNull();
  });

  // --- HSL ---
  it('parses hsl()', () => {
    const c = parseColor('hsl(120, 100%, 50%)');
    expect(c).not.toBeNull();
    expect(c.source).toBe('hsl');
    expect(c.g).toBeGreaterThan(200); // bright green
  });

  it('parses hsl without % signs', () => {
    const c = parseColor('hsl(0, 100, 50)');
    expect(c).not.toBeNull();
    expect(c.r).toBe(255); // red
    expect(c.g).toBe(0);
    expect(c.b).toBe(0);
  });

  // --- Named colors ---
  it('parses named color "red"', () => {
    const c = parseColor('red');
    expect(c).toEqual({ r: 255, g: 0, b: 0, source: 'name' });
  });

  it('parses named color "teal"', () => {
    const c = parseColor('teal');
    expect(c).toEqual({ r: 0, g: 128, b: 128, source: 'name' });
  });

  it('is case-insensitive for named colors', () => {
    expect(parseColor('RED')).toEqual({ r: 255, g: 0, b: 0, source: 'name' });
  });

  // --- Invalid ---
  it('returns null for plain text', () => {
    expect(parseColor('hello')).toBeNull();
  });

  it('returns null for empty string', () => {
    expect(parseColor('')).toBeNull();
  });

  it('returns null for invalid hex', () => {
    expect(parseColor('#xyz')).toBeNull();
    expect(parseColor('#12345')).toBeNull();
  });
});

describe('rgbToHex', () => {
  it('converts black', () => {
    expect(rgbToHex(0, 0, 0)).toBe('#000000');
  });

  it('converts white', () => {
    expect(rgbToHex(255, 255, 255)).toBe('#ffffff');
  });

  it('converts red', () => {
    expect(rgbToHex(255, 0, 0)).toBe('#ff0000');
  });

  it('pads single-digit hex values', () => {
    expect(rgbToHex(1, 2, 3)).toBe('#010203');
  });
});

describe('rgbToHsl', () => {
  it('converts red', () => {
    const hsl = rgbToHsl(255, 0, 0);
    expect(hsl.h).toBe(0);
    expect(hsl.s).toBe(100);
    expect(hsl.l).toBe(50);
  });

  it('converts green', () => {
    const hsl = rgbToHsl(0, 255, 0);
    expect(hsl.h).toBe(120);
    expect(hsl.s).toBe(100);
    expect(hsl.l).toBe(50);
  });

  it('converts blue', () => {
    const hsl = rgbToHsl(0, 0, 255);
    expect(hsl.h).toBe(240);
    expect(hsl.s).toBe(100);
    expect(hsl.l).toBe(50);
  });

  it('converts white', () => {
    const hsl = rgbToHsl(255, 255, 255);
    expect(hsl.h).toBe(0);
    expect(hsl.s).toBe(0);
    expect(hsl.l).toBe(100);
  });

  it('converts black', () => {
    const hsl = rgbToHsl(0, 0, 0);
    expect(hsl.h).toBe(0);
    expect(hsl.s).toBe(0);
    expect(hsl.l).toBe(0);
  });

  it('converts gray', () => {
    const hsl = rgbToHsl(128, 128, 128);
    expect(hsl.s).toBe(0);
    expect(hsl.l).toBeCloseTo(50, 0);
  });
});

describe('formatAllColors', () => {
  it('returns all three formats', () => {
    const result = formatAllColors(255, 0, 0);
    expect(result.hex).toBe('#FF0000');
    expect(result.rgb).toBe('rgb(255, 0, 0)');
    expect(result.hsl).toBe('hsl(0, 100%, 50%)');
  });

  it('formats teal correctly', () => {
    const result = formatAllColors(0, 128, 128);
    expect(result.hex).toBe('#008080');
    expect(result.rgb).toBe('rgb(0, 128, 128)');
    expect(result.hsl).toBe('hsl(180, 100%, 25%)');
  });
});

describe('parseColor → formatAllColors roundtrip', () => {
  it('hex roundtrips correctly', () => {
    const parsed = parseColor('#3498db');
    const formatted = formatAllColors(parsed.r, parsed.g, parsed.b);
    expect(formatted.hex).toBe('#3498DB');
  });

  it('named color roundtrips', () => {
    const parsed = parseColor('coral');
    expect(parsed).not.toBeNull();
    const formatted = formatAllColors(parsed.r, parsed.g, parsed.b);
    expect(formatted.hex).toBe('#FF7F50');
  });
});
