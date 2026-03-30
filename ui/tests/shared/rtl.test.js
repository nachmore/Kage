import { describe, it, expect, beforeEach } from 'vitest';
import { setupRtlDetection } from '../../js/shared/rtl.js';

describe('setupRtlDetection', () => {
  let input, container, responseEl;

  beforeEach(() => {
    input = document.createElement('textarea');
    container = document.createElement('div');
    responseEl = document.createElement('div');
    document.body.appendChild(input);
    document.body.appendChild(container);
    document.body.appendChild(responseEl);
  });

  function simulateInput(value) {
    input.value = value;
    input.dispatchEvent(new Event('input'));
  }

  it('adds rtl class when typing Arabic text', () => {
    setupRtlDetection(input, container, responseEl);
    simulateInput('مرحبا');
    expect(container.classList.contains('rtl')).toBe(true);
    expect(responseEl.classList.contains('rtl')).toBe(true);
  });

  it('adds rtl class when typing Hebrew text', () => {
    setupRtlDetection(input, container, responseEl);
    simulateInput('שלום');
    expect(container.classList.contains('rtl')).toBe(true);
  });

  it('removes rtl class when typing English text', () => {
    setupRtlDetection(input, container, responseEl);
    simulateInput('مرحبا');
    expect(container.classList.contains('rtl')).toBe(true);
    simulateInput('Hello');
    expect(container.classList.contains('rtl')).toBe(false);
    expect(responseEl.classList.contains('rtl')).toBe(false);
  });

  it('removes rtl class when input is cleared', () => {
    setupRtlDetection(input, container, responseEl);
    simulateInput('مرحبا');
    expect(container.classList.contains('rtl')).toBe(true);
    simulateInput('');
    expect(container.classList.contains('rtl')).toBe(false);
    expect(responseEl.classList.contains('rtl')).toBe(false);
  });

  it('does not add rtl class for LTR text', () => {
    setupRtlDetection(input, container, responseEl);
    simulateInput('Hello world');
    expect(container.classList.contains('rtl')).toBe(false);
  });

  it('works without responseEl', () => {
    setupRtlDetection(input, container);
    simulateInput('مرحبا');
    expect(container.classList.contains('rtl')).toBe(true);
  });

  it('does nothing if inputEl is null', () => {
    // Should not throw
    setupRtlDetection(null, container);
  });

  it('does nothing if container is null', () => {
    setupRtlDetection(input, null);
  });
});
