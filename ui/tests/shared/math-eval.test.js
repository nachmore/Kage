import { describe, it, expect, beforeEach } from 'vitest';
import { evaluateMath } from '../../js/shared/math-eval.js';

// Mock window.math (mathjs) with basic evaluate
beforeEach(() => {
  window.math = {
    evaluate: (expr) => {
      // Simple eval for testing — handles basic arithmetic
      // Real mathjs handles units, functions, etc.
      const result = Function(`"use strict"; return (${expr})`)();
      return result;
    },
    isBigNumber: () => false,
  };
});

describe('evaluateMath', () => {
  it('evaluates basic arithmetic', () => {
    const result = evaluateMath('2 + 3');
    expect(result).not.toBeNull();
    expect(result.result).toBe(5);
    expect(result.display).toBe('5');
  });

  it('evaluates multiplication', () => {
    const result = evaluateMath('4 * 5');
    expect(result.result).toBe(20);
  });

  it('evaluates division', () => {
    const result = evaluateMath('10 / 3');
    expect(result).not.toBeNull();
    expect(result.result).toBeCloseTo(3.333, 2);
  });

  it('evaluates expressions with parentheses', () => {
    const result = evaluateMath('(2 + 3) * 4');
    expect(result.result).toBe(20);
  });

  it('returns null for plain text', () => {
    expect(evaluateMath('hello world')).toBeNull();
  });

  it('returns null for empty string', () => {
    expect(evaluateMath('')).toBeNull();
  });

  it('returns null for text without digits', () => {
    expect(evaluateMath('no numbers here')).toBeNull();
  });

  it('returns null for text without operators', () => {
    expect(evaluateMath('42')).toBeNull();
  });

  it('respects precision parameter', () => {
    const result = evaluateMath('10 / 3', 2);
    expect(result.display).toBe('3.33');
  });

  it('returns null when window.math is not available', () => {
    delete window.math;
    expect(evaluateMath('2 + 3')).toBeNull();
  });

  it('returns null for natural language with numbers', () => {
    expect(evaluateMath('there are 5 cats and 3 dogs')).toBeNull();
  });
});
