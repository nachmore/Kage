import { describe, it, expect, beforeEach } from 'vitest';
import { evaluateMath, resetMathCache } from '../../js/shared/math-eval.js';
import { create, all } from 'mathjs';

// Use real mathjs instance for accurate testing
const mathInstance = create(all);

beforeEach(() => {
  window.math = mathInstance;
  resetMathCache();
});

// ---------------------------------------------------------------------------
// Basic arithmetic
// ---------------------------------------------------------------------------
describe('basic arithmetic', () => {
  it.each([
    ['2 + 3', 5],
    ['2+3', 5],
    ['10 - 4', 6],
    ['10-4', 6],
    ['4 * 5', 20],
    ['4*5', 20],
    ['10 / 2', 5],
    ['10/2', 5],
  ])('evaluates "%s" = %s', (expr, expected) => {
    const r = evaluateMath(expr);
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(expected);
  });
});

// ---------------------------------------------------------------------------
// Multiplication patterns (the reported bug)
// ---------------------------------------------------------------------------
describe('multiplication patterns', () => {
  it.each([
    ['10*12', 120],
    ['10 * 12', 120],
    ['10*12+(10*6)', 180],
    ['10*12 + (10*6)', 180],
    ['3*3', 9],
    ['100*0.5', 50],
    ['2*3*4', 24],
    ['1.5*2', 3],
  ])('evaluates "%s" = %s', (expr, expected) => {
    const r = evaluateMath(expr);
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(expected);
  });
});

// ---------------------------------------------------------------------------
// Division patterns
// ---------------------------------------------------------------------------
describe('division patterns', () => {
  it.each([
    ['10/3', 10 / 3],
    ['100/4', 25],
    ['1/3', 1 / 3],
    ['22/7', 22 / 7],
  ])('evaluates "%s"', (expr, expected) => {
    const r = evaluateMath(expr);
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(expected);
  });
});

// ---------------------------------------------------------------------------
// Parentheses and order of operations
// ---------------------------------------------------------------------------
describe('parentheses and order of operations', () => {
  it.each([
    ['(2 + 3) * 4', 20],
    ['2 * (3 + 4)', 14],
    ['(10+5)*(2+3)', 75],
    ['((2+3))', 5],
    ['(1+2)*(3+4)*(5+6)', 231],
    ['10*(2+3)', 50],
    ['(100-50)/2', 25],
  ])('evaluates "%s" = %s', (expr, expected) => {
    const r = evaluateMath(expr);
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(expected);
  });
});

// ---------------------------------------------------------------------------
// Exponents and powers
// ---------------------------------------------------------------------------
describe('exponents and powers', () => {
  it.each([
    ['2^10', 1024],
    ['3^3', 27],
    ['10^2', 100],
    ['2^0', 1],
    ['2^-1', 0.5],
  ])('evaluates "%s" = %s', (expr, expected) => {
    const r = evaluateMath(expr);
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(expected);
  });
});

// ---------------------------------------------------------------------------
// Modulo / remainder
// ---------------------------------------------------------------------------
describe('modulo', () => {
  it.each([
    ['10 % 3', 1],
    ['10%3', 1],
    ['15 % 4', 3],
    ['100 % 7', 2],
  ])('evaluates "%s" = %s', (expr, expected) => {
    const r = evaluateMath(expr);
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(expected);
  });
});

// ---------------------------------------------------------------------------
// Factorial
// ---------------------------------------------------------------------------
describe('factorial', () => {
  it.each([
    ['5!', 120],
    ['3!', 6],
    ['0!', 1],
    ['10!', 3628800],
  ])('evaluates "%s" = %s', (expr, expected) => {
    const r = evaluateMath(expr);
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(expected);
  });
});

// ---------------------------------------------------------------------------
// Math functions
// ---------------------------------------------------------------------------
describe('math functions', () => {
  it.each([
    ['sqrt(144)', 12],
    ['sqrt(2)', Math.sqrt(2)],
    ['abs(-5)', 5],
    ['ceil(4.2)', 5],
    ['floor(4.8)', 4],
    ['round(4.5)', 5],
    ['log(1)', 0],
    ['sin(0)', 0],
    ['cos(0)', 1],
  ])('evaluates "%s"', (expr, expected) => {
    const r = evaluateMath(expr);
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(expected, 5);
  });
});

// ---------------------------------------------------------------------------
// Decimal / floating point
// ---------------------------------------------------------------------------
describe('decimal and floating point', () => {
  it.each([
    ['0.1 + 0.2', 0.3],
    ['1.5 * 2.5', 3.75],
    ['3.14 * 2', 6.28],
    ['100.50 - 0.50', 100],
    ['99.99 + 0.01', 100],
  ])('evaluates "%s"', (expr, expected) => {
    const r = evaluateMath(expr);
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(expected);
  });
});

// ---------------------------------------------------------------------------
// Negative numbers
// ---------------------------------------------------------------------------
describe('negative numbers', () => {
  it.each([
    ['-5 + 3', -2],
    ['-5 * -3', 15],
    ['-10 / 2', -5],
    ['(-5)', -5],
  ])('evaluates "%s" = %s', (expr, expected) => {
    const r = evaluateMath(expr);
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(expected);
  });
});

// ---------------------------------------------------------------------------
// Large and small numbers
// ---------------------------------------------------------------------------
describe('large and small numbers', () => {
  it.each([
    ['1000000 * 1000000', 1e12],
    ['1e6 * 1e6', 1e12],
    ['0.001 * 0.001', 0.000001],
  ])('evaluates "%s"', (expr, expected) => {
    const r = evaluateMath(expr);
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(expected);
  });
});

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------
describe('constants', () => {
  it('evaluates pi', () => {
    const r = evaluateMath('pi * 2');
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(Math.PI * 2);
  });

  it('evaluates e', () => {
    const r = evaluateMath('e ^ 2');
    expect(r).not.toBeNull();
    expect(r.result).toBeCloseTo(Math.E ** 2);
  });
});

// ---------------------------------------------------------------------------
// Precision parameter
// ---------------------------------------------------------------------------
describe('precision parameter', () => {
  it('formats to 2 decimal places', () => {
    const r = evaluateMath('10 / 3', 2);
    expect(r).not.toBeNull();
    expect(r.display).toBe('3.33');
  });

  it('precision 0 means auto-format (not toFixed(0))', () => {
    const r = evaluateMath('10 / 3', 0);
    expect(r).not.toBeNull();
    // Auto mode: up to 12 significant digits, trailing zeros trimmed
    expect(r.display).toBe('3.33333333333');
  });

  it('formats to 5 decimal places', () => {
    const r = evaluateMath('1 / 7', 5);
    expect(r).not.toBeNull();
    expect(r.display).toBe('0.14286');
  });
});

// ---------------------------------------------------------------------------
// Should return null (not math)
// ---------------------------------------------------------------------------
describe('rejects non-math input', () => {
  it.each([
    [''],
    ['hello world'],
    ['no numbers here'],
    ['there are five cats'],
    ['the quick brown fox'],
  ])('returns null for "%s"', (expr) => {
    expect(evaluateMath(expr)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Plain numbers (no calculation) should return null
// ---------------------------------------------------------------------------
describe('rejects plain numbers', () => {
  it.each([
    ['42'],
    ['100'],
    ['3.14'],
    ['0'],
  ])('returns null for plain number "%s"', (expr) => {
    expect(evaluateMath(expr)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------
describe('edge cases', () => {
  it('returns null when window.math is not available', () => {
    delete window.math;
    expect(evaluateMath('2 + 3')).toBeNull();
  });

  it('returns null for Infinity result', () => {
    const r = evaluateMath('1/0');
    expect(r).toBeNull();
  });

  it('handles whitespace around expression', () => {
    const r = evaluateMath('  2 + 3  ');
    expect(r).not.toBeNull();
    expect(r.result).toBe(5);
  });

  it('handles mixed operators', () => {
    const r = evaluateMath('2 + 3 * 4 - 1');
    expect(r).not.toBeNull();
    expect(r.result).toBe(13);
  });
});

// ---------------------------------------------------------------------------
// Failed-prefix caching (performance optimization)
// ---------------------------------------------------------------------------
describe('failed-prefix caching', () => {
  it('skips evaluate for inputs extending a previously failed prefix', () => {
    // "1 2" fails evaluate → "1 2 3", "1 2 3 4" should skip
    expect(evaluateMath('1 2')).toBeNull();
    const evalSpy = vi.spyOn(window.math, 'evaluate');
    expect(evaluateMath('1 2 3')).toBeNull();
    expect(evaluateMath('1 2 3 4')).toBeNull();
    expect(evaluateMath('1 2 3 4 5')).toBeNull();
    expect(evalSpy).not.toHaveBeenCalled();
    evalSpy.mockRestore();
  });

  it('clears cache when a valid expression is entered', () => {
    expect(evaluateMath('1 2')).toBeNull();
    // A completely different valid expression should still work
    const r = evaluateMath('2+3');
    expect(r).not.toBeNull();
    expect(r.result).toBe(5);
  });

  it('does not cache across unrelated inputs', () => {
    expect(evaluateMath('1 2')).toBeNull();
    // "5*3" does NOT start with "1 2", so it should still be evaluated
    const r = evaluateMath('5*3');
    expect(r).not.toBeNull();
    expect(r.result).toBe(15);
  });

  it('resets cache between tests via resetMathCache', () => {
    // beforeEach calls resetMathCache(), so evaluate should be called fresh
    const evalSpy = vi.spyOn(window.math, 'evaluate');
    evaluateMath('1 2');
    expect(evalSpy).toHaveBeenCalled();
    evalSpy.mockRestore();
  });
});

// ---------------------------------------------------------------------------
// Incremental typing simulation (keystroke-by-keystroke)
// ---------------------------------------------------------------------------
describe('incremental typing', () => {
  it('evaluates 22/10 after typing keystroke by keystroke', () => {
    // Simulates: 2 → 22 → 22/ → 22/1 → 22/10
    expect(evaluateMath('2')).toBeNull();   // plain number
    expect(evaluateMath('22')).toBeNull();  // plain number
    expect(evaluateMath('22/')).toBeNull(); // incomplete — should NOT poison cache
    const r = evaluateMath('22/1');
    expect(r).not.toBeNull();
    expect(r.result).toBe(22);
    const r2 = evaluateMath('22/10');
    expect(r2).not.toBeNull();
    expect(r2.result).toBeCloseTo(2.2);
  });

  it('evaluates 10*12 after typing keystroke by keystroke', () => {
    expect(evaluateMath('1')).toBeNull();
    expect(evaluateMath('10')).toBeNull();
    expect(evaluateMath('10*')).toBeNull();
    const r = evaluateMath('10*1');
    expect(r).not.toBeNull();
    expect(r.result).toBe(10);
    const r2 = evaluateMath('10*12');
    expect(r2).not.toBeNull();
    expect(r2.result).toBe(120);
  });

  it('evaluates 5+3 after typing keystroke by keystroke', () => {
    expect(evaluateMath('5')).toBeNull();
    expect(evaluateMath('5+')).toBeNull();
    const r = evaluateMath('5+3');
    expect(r).not.toBeNull();
    expect(r.result).toBe(8);
  });

  it('evaluates (2+3)*4 after typing keystroke by keystroke', () => {
    expect(evaluateMath('(')).toBeNull();
    expect(evaluateMath('(2')).toBeNull();
    expect(evaluateMath('(2+')).toBeNull();
    expect(evaluateMath('(2+3')).toBeNull();
    // (2+3) is a valid expression = 5
    const r1 = evaluateMath('(2+3)');
    expect(r1).not.toBeNull();
    expect(r1.result).toBe(5);
    expect(evaluateMath('(2+3)*')).toBeNull();
    const r = evaluateMath('(2+3)*4');
    expect(r).not.toBeNull();
    expect(r.result).toBe(20);
  });
});
