/**
 * Tests for ui/js/shared/hotkey-capture.js — DOM keydown → Kage hotkey,
 * used by the macOS hotkey picker (records in-webview so no TCC
 * permission is ever needed).
 */
import { describe, it, expect } from 'vitest';
import {
    classifyKeyEvent,
    codeToKeyName,
    formatCombo,
    modifiersFromEvent,
} from '../../ui/js/shared/hotkey-capture.js';

const ev = (code, init = {}) => ({ code, key: init.key ?? '', repeat: false, ...init });

describe('codeToKeyName', () => {
    it('maps letters, digits and numpad digits by physical code', () => {
        expect(codeToKeyName('KeyA')).toBe('A');
        expect(codeToKeyName('KeyZ')).toBe('Z');
        expect(codeToKeyName('Digit0')).toBe('0');
        expect(codeToKeyName('Numpad7')).toBe('Num7');
    });

    it('maps function keys F1–F24 and rejects F25', () => {
        expect(codeToKeyName('F1')).toBe('F1');
        expect(codeToKeyName('F12')).toBe('F12');
        expect(codeToKeyName('F24')).toBe('F24');
        expect(codeToKeyName('F25')).toBeNull();
    });

    it('uses the names the Rust side / global-hotkey parser expect', () => {
        expect(codeToKeyName('Space')).toBe('Space');
        expect(codeToKeyName('ArrowUp')).toBe('Up');
        expect(codeToKeyName('Backquote')).toBe('`');
        expect(codeToKeyName('Backslash')).toBe('\\');
        // `NumEqual`, not the old (unparseable) `NumEquals`.
        expect(codeToKeyName('NumpadEqual')).toBe('NumEqual');
    });

    it('returns null for keys Kage cannot bind', () => {
        expect(codeToKeyName('IntlBackslash')).toBeNull();
        expect(codeToKeyName('AudioVolumeUp')).toBeNull();
        expect(codeToKeyName('')).toBeNull();
        expect(codeToKeyName(undefined)).toBeNull();
        // Prototype keys must not leak through the lookup table.
        expect(codeToKeyName('toString')).toBeNull();
    });
});

describe('modifiersFromEvent', () => {
    it('emits Ctrl, Alt, Shift, Super in that order', () => {
        expect(
            modifiersFromEvent({ metaKey: true, shiftKey: true, altKey: true, ctrlKey: true })
        ).toEqual(['Ctrl', 'Alt', 'Shift', 'Super']);
        expect(modifiersFromEvent({})).toEqual([]);
    });
});

describe('classifyKeyEvent', () => {
    it('captures ⌘Space', () => {
        expect(classifyKeyEvent(ev('Space', { metaKey: true, key: ' ' }))).toEqual({
            kind: 'combo',
            modifiers: ['Super'],
            key: 'Space',
        });
    });

    it('uses the physical key for Option combos (⌥A types "å")', () => {
        expect(classifyKeyEvent(ev('KeyA', { altKey: true, key: 'å' }))).toEqual({
            kind: 'combo',
            modifiers: ['Alt'],
            key: 'A',
        });
    });

    it('ignores modifier-only presses, auto-repeat and IME composition', () => {
        expect(classifyKeyEvent(ev('MetaLeft', { key: 'Meta', metaKey: true })).kind).toBe('ignore');
        expect(classifyKeyEvent(ev('ShiftRight', { key: 'Shift' })).kind).toBe('ignore');
        expect(classifyKeyEvent(ev('CapsLock', { key: 'CapsLock' })).kind).toBe('ignore');
        expect(classifyKeyEvent(ev('KeyK', { metaKey: true, repeat: true })).kind).toBe('ignore');
        expect(classifyKeyEvent(ev('KeyK', { isComposing: true })).kind).toBe('ignore');
        expect(classifyKeyEvent(ev('', { keyCode: 229, key: 'Process' })).kind).toBe('ignore');
        expect(classifyKeyEvent(null).kind).toBe('ignore');
    });

    it('treats bare Escape as cancel but allows modified Escape', () => {
        expect(classifyKeyEvent(ev('Escape', { key: 'Escape' }))).toEqual({ kind: 'cancel' });
        expect(classifyKeyEvent(ev('Escape', { key: 'Escape', ctrlKey: true }))).toEqual({
            kind: 'combo',
            modifiers: ['Ctrl'],
            key: 'Escape',
        });
    });

    it('reports unsupported keys instead of dropping them', () => {
        expect(classifyKeyEvent(ev('IntlBackslash', { metaKey: true, key: '§' }))).toEqual({
            kind: 'unsupported',
            code: 'IntlBackslash',
        });
    });
});

describe('formatCombo', () => {
    it('renders macOS symbols', () => {
        expect(formatCombo({ modifiers: ['Super'], key: 'Space' }, { mac: true })).toBe('⌘Space');
        expect(
            formatCombo({ modifiers: ['Ctrl', 'Alt', 'Shift', 'Super'], key: 'K' }, { mac: true })
        ).toBe('⌃⌥⇧⌘K');
    });

    it('renders plus-joined names elsewhere', () => {
        expect(formatCombo({ modifiers: ['Ctrl', 'Shift'], key: 'Space' })).toBe('Ctrl+Shift+Space');
        expect(formatCombo({ modifiers: [], key: 'F13' })).toBe('F13');
        expect(formatCombo(null)).toBe('');
    });
});
