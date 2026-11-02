/**
 * Tests for ui/js/shared/hotkey-picker.js.
 *
 * macOS: records from DOM keydown (no TCC permission), always pairs
 * begin_hotkey_recording / end_hotkey_recording, and asks before taking
 * over a macOS system shortcut (⌘Space → Spotlight).
 * Windows: unchanged — native capture via capture_hotkey_combo.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const EN_CATALOG = JSON.parse(
    readFileSync(
        join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'locales', 'en', 'messages.json'),
        'utf-8'
    )
);

async function loadPicker(platform) {
    Object.defineProperty(navigator, 'platform', { value: platform, configurable: true });
    vi.resetModules(); // isMac() caches the platform per module instance
    // Same fresh module graph as the picker, so its t() sees real EN strings.
    const { initI18n } = await import('../../ui/js/shared/i18n.js');
    await initI18n(async () => ({
        language: 'en',
        rtl: false,
        catalog: EN_CATALOG,
        fallback: EN_CATALOG,
    }));
    return (await import('../../ui/js/shared/hotkey-picker.js')).HotkeyPicker;
}

function makeInvoke(handlers = {}) {
    const calls = [];
    const invoke = vi.fn(async (cmd, args) => {
        calls.push(cmd);
        if (handlers[cmd]) return handlers[cmd](args);
        return null;
    });
    return { invoke, calls };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

function press(code, init = {}) {
    const e = new KeyboardEvent('keydown', { code, bubbles: true, cancelable: true, ...init });
    document.body.dispatchEvent(e);
    return e;
}

let container;
beforeEach(() => {
    document.body.innerHTML = '<div id="c"></div>';
    container = document.getElementById('c');
});
afterEach(() => {
    vi.useRealTimers();
});

describe('HotkeyPicker on macOS', () => {
    it('records ⌘Space in-webview and registers it when there is no conflict', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        const { invoke, calls } = makeInvoke({ check_system_shortcut_conflicts: () => [] });
        const picker = new HotkeyPicker(container, invoke, { modifiers: ['Alt'], key: 'Space' });
        const changed = vi.fn();
        picker.onChange(changed);

        const done = picker.startCapture();
        await flush();
        press('Space', { metaKey: true, key: ' ' });
        await done;

        expect(calls).toEqual([
            'begin_hotkey_recording',
            'end_hotkey_recording',
            'check_system_shortcut_conflicts',
            'try_register_hotkey',
        ]);
        expect(calls).not.toContain('capture_hotkey_combo');
        expect(invoke).toHaveBeenCalledWith('try_register_hotkey', {
            modifiers: ['Super'],
            key: 'Space',
            slot: 'main',
        });
        expect(changed).toHaveBeenCalledWith({ modifiers: ['Super'], key: 'Space' });
        expect(picker.capturing).toBe(false);
    });

    it('swallows the recorded keydown so it cannot act on the window', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        const { invoke } = makeInvoke({ check_system_shortcut_conflicts: () => [] });
        const picker = new HotkeyPicker(container, invoke);
        const other = vi.fn();
        document.addEventListener('keydown', other);

        const done = picker.startCapture();
        await flush();
        const e = press('KeyW', { metaKey: true, key: 'w' }); // ⌘W would hide Settings
        await done;

        expect(e.defaultPrevented).toBe(true);
        expect(other).not.toHaveBeenCalled();
        document.removeEventListener('keydown', other);
    });

    it('ignores modifier-only presses and cancels on bare Escape', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        const { invoke, calls } = makeInvoke();
        const picker = new HotkeyPicker(container, invoke);

        const done = picker.startCapture();
        await flush();
        press('MetaLeft', { key: 'Meta', metaKey: true });
        await flush();
        expect(picker.capturing).toBe(true);
        press('Escape', { key: 'Escape' });
        await done;

        expect(calls).toEqual(['begin_hotkey_recording', 'end_hotkey_recording']);
        expect(picker.hotkey).toEqual({ modifiers: ['Alt'], key: 'Space' });
    });

    it('ends recording on window blur and on timeout', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        const { invoke, calls } = makeInvoke();
        const picker = new HotkeyPicker(container, invoke);

        let done = picker.startCapture();
        await flush();
        window.dispatchEvent(new Event('blur'));
        await done;
        expect(calls.filter((c) => c === 'end_hotkey_recording')).toHaveLength(1);

        vi.useFakeTimers();
        done = picker.startCapture();
        await vi.advanceTimersByTimeAsync(10_001);
        await done;
        expect(calls.filter((c) => c === 'end_hotkey_recording')).toHaveLength(2);
        expect(calls).not.toContain('try_register_hotkey');
    });

    it('still ends recording when begin_hotkey_recording fails', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        const { invoke, calls } = makeInvoke({
            begin_hotkey_recording: () => {
                throw new Error('boom');
            },
        });
        const picker = new HotkeyPicker(container, invoke);
        const done = picker.startCapture();
        await flush();
        press('Escape', { key: 'Escape' });
        await done;
        expect(calls).toContain('end_hotkey_recording');
    });

    it('asks before taking over Spotlight, then approves and registers', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        const evil = '<img src=x onerror=alert(1)><b>Spotlight</b>';
        const { invoke, calls } = makeInvoke({
            check_system_shortcut_conflicts: () => [
                { id: 64, name: evil, combo: 'Super+Space', approved: false },
            ],
        });
        const picker = new HotkeyPicker(container, invoke, { modifiers: ['Alt'], key: 'Space' });
        const changed = vi.fn();
        picker.onChange(changed);

        await picker.applyCombo({ modifiers: ['Super'], key: 'Space' });

        expect(calls).not.toContain('try_register_hotkey');
        expect(changed).not.toHaveBeenCalled();
        const status = container.querySelector('.hk-picker-status');
        expect(status.classList.contains('hk-status-warning')).toBe(true);
        // Backend-supplied names are rendered as text, never parsed as HTML.
        expect(status.querySelector('img, b')).toBeNull();
        expect(status.textContent).toContain(evil);
        // The combo is LTR-isolated so RTL locales don't flip it to "Space⌘".
        expect(status.textContent).toContain('\u2066⌘Space\u2069');

        container.querySelector('.hk-picker-conflict-replace').click();
        await flush();
        await flush();

        expect(invoke).toHaveBeenCalledWith('allow_system_shortcut_override', { ids: [64] });
        expect(invoke).toHaveBeenCalledWith('try_register_hotkey', {
            modifiers: ['Super'],
            key: 'Space',
            slot: 'main',
        });
        expect(changed).toHaveBeenCalledWith({ modifiers: ['Super'], key: 'Space' });
    });

    it('keeps the current hotkey when the user declines the takeover', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        const { invoke, calls } = makeInvoke({
            check_system_shortcut_conflicts: () => [
                { id: 64, name: 'Spotlight search', combo: 'Super+Space', approved: false },
            ],
        });
        const picker = new HotkeyPicker(container, invoke, { modifiers: ['Alt'], key: 'Space' });
        await picker.applyCombo({ modifiers: ['Super'], key: 'Space' });
        container.querySelector('.hk-picker-conflict-keep').click();
        await flush();

        expect(calls).not.toContain('allow_system_shortcut_override');
        expect(calls).not.toContain('try_register_hotkey');
        expect(picker.hotkey).toEqual({ modifiers: ['Alt'], key: 'Space' });
        expect(container.querySelector('.hk-picker-status').textContent).toBe('');
    });

    it('skips the prompt for an already-approved takeover', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        const { invoke, calls } = makeInvoke({
            check_system_shortcut_conflicts: () => [
                { id: 64, name: 'Spotlight search', combo: 'Super+Space', approved: true },
            ],
        });
        const picker = new HotkeyPicker(container, invoke);
        await picker.applyCombo({ modifiers: ['Super'], key: 'Space' });
        expect(calls).toEqual(['check_system_shortcut_conflicts', 'try_register_hotkey']);
    });

    it('treats a failed conflict check as no conflict', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        const { invoke, calls } = makeInvoke({
            check_system_shortcut_conflicts: () => {
                throw new Error('nope');
            },
        });
        const picker = new HotkeyPicker(container, invoke);
        await picker.applyCombo({ modifiers: ['Ctrl', 'Alt'], key: 'K' });
        expect(calls).toContain('try_register_hotkey');
    });
});

describe('HotkeyPicker recording lifecycle (macOS)', () => {
    it('hands begin\'s generation back to end', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        const { invoke } = makeInvoke({ begin_hotkey_recording: () => 42 });
        const picker = new HotkeyPicker(container, invoke);
        const done = picker.startCapture();
        await flush();
        press('Escape', { key: 'Escape' });
        await done;
        expect(invoke).toHaveBeenCalledWith('end_hotkey_recording', { generation: 42 });
    });

    it('does not send end before a slow begin has finished', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        let releaseBegin;
        const order = [];
        const { invoke } = makeInvoke({
            begin_hotkey_recording: () =>
                new Promise((r) => {
                    releaseBegin = () => {
                        order.push('begin-resolved');
                        r(7);
                    };
                }),
            end_hotkey_recording: () => {
                order.push('end');
            },
        });
        const picker = new HotkeyPicker(container, invoke);
        const done = picker.startCapture();
        await flush();
        press('Escape', { key: 'Escape' }); // instant cancel while begin is in flight
        await flush();
        expect(order).toEqual([]); // end must wait for begin
        releaseBegin();
        await done;
        expect(order).toEqual(['begin-resolved', 'end']);
    });

    it('a second picker cancels the first, and the key goes to the second slot', async () => {
        const HotkeyPicker = await loadPicker('MacIntel');
        let gen = 0;
        const { invoke } = makeInvoke({
            begin_hotkey_recording: () => ++gen,
            check_system_shortcut_conflicts: () => [],
        });
        document.body.innerHTML = '<div id="a"></div><div id="b"></div>';
        const main = new HotkeyPicker(document.getElementById('a'), invoke, undefined, 'main');
        const voice = new HotkeyPicker(document.getElementById('b'), invoke, undefined, 'voice');
        const mainChanged = vi.fn();
        const voiceChanged = vi.fn();
        main.onChange(mainChanged);
        voice.onChange(voiceChanged);

        const first = main.startCapture();
        await flush();
        const second = voice.startCapture();
        await first; // cancelled by the second
        await flush();
        expect(main.capturing).toBe(false);
        expect(invoke).toHaveBeenCalledWith('end_hotkey_recording', { generation: 1 });

        press('KeyV', { ctrlKey: true, altKey: true, key: 'v' });
        await second;

        expect(mainChanged).not.toHaveBeenCalled();
        expect(voiceChanged).toHaveBeenCalledWith({ modifiers: ['Ctrl', 'Alt'], key: 'V' });
        expect(invoke).toHaveBeenCalledWith('try_register_hotkey', {
            modifiers: ['Ctrl', 'Alt'],
            key: 'V',
            slot: 'voice',
        });
        expect(invoke).toHaveBeenCalledWith('end_hotkey_recording', { generation: 2 });
    });
});

describe('HotkeyPicker on Windows', () => {
    it('uses native capture and never the macOS recording / takeover commands', async () => {
        const HotkeyPicker = await loadPicker('Win32');
        const { invoke, calls } = makeInvoke({
            capture_hotkey_combo: () => ({ modifiers: ['Ctrl'], key: 'Space', display: 'Ctrl+Space' }),
        });
        const picker = new HotkeyPicker(container, invoke);
        await picker.startCapture();
        expect(calls).toEqual(['capture_hotkey_combo', 'try_register_hotkey']);
    });
});
