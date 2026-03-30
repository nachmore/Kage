import { describe, it, expect } from 'vitest';
import { detectScript } from '../../js/shared/language-detect.js';

describe('detectScript', () => {
  it('detects Arabic text', () => {
    expect(detectScript('مرحبا')).toBe('ar');
  });

  it('detects Hebrew text', () => {
    expect(detectScript('שלום')).toBe('he');
  });

  it('detects Japanese hiragana', () => {
    expect(detectScript('こんにちは')).toBe('ja');
  });

  it('detects Japanese katakana', () => {
    expect(detectScript('カタカナ')).toBe('ja');
  });

  it('detects Korean text', () => {
    expect(detectScript('안녕하세요')).toBe('ko');
  });

  it('detects Chinese text', () => {
    expect(detectScript('你好世界')).toBe('zh');
  });

  it('detects Thai text', () => {
    expect(detectScript('สวัสดี')).toBe('th');
  });

  it('detects Hindi (Devanagari) text', () => {
    expect(detectScript('नमस्ते')).toBe('hi');
  });

  it('detects Bengali text', () => {
    expect(detectScript('বাংলা')).toBe('bn');
  });

  it('detects Tamil text', () => {
    expect(detectScript('தமிழ்')).toBe('ta');
  });

  it('detects Georgian text', () => {
    expect(detectScript('გამარჯობა')).toBe('ka');
  });

  it('detects Armenian text', () => {
    expect(detectScript('Բարեdelays')).toBe('hy');
  });

  it('returns null for Latin text', () => {
    expect(detectScript('Hello world')).toBeNull();
  });

  it('returns null for empty string', () => {
    expect(detectScript('')).toBeNull();
  });

  it('returns null for null input', () => {
    expect(detectScript(null)).toBeNull();
  });

  it('returns null for numbers only', () => {
    expect(detectScript('12345')).toBeNull();
  });

  it('returns the dominant script when mixed', () => {
    // Mostly Arabic with a Latin word
    expect(detectScript('مرحبا hello مرحبا بالعالم')).toBe('ar');
  });
});
