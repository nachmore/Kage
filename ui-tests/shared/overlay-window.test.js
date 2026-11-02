/**
 * ui/js/shared/overlay-window.js — on macOS overlays are presented by the
 * backend (non-activating panel, no app activation); elsewhere, or when that
 * fails, it's the plain show + setFocus.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';

async function load(platform, invoke) {
    Object.defineProperty(navigator, 'platform', { value: platform, configurable: true });
    window.__TAURI__ = invoke ? { core: { invoke } } : undefined;
    vi.resetModules(); // isMac() caches per module instance
    return (await import('../../ui/js/shared/overlay-window.js')).presentOverlayWindow;
}

const fakeWindow = () => ({ show: vi.fn(async () => {}), setFocus: vi.fn(async () => {}) });

afterEach(() => {
    delete window.__TAURI__;
});

describe('presentOverlayWindow', () => {
    it('macOS: asks the backend and never calls setFocus (which activates the app)', async () => {
        const invoke = vi.fn(async () => {});
        const present = await load('MacIntel', invoke);
        const w = fakeWindow();
        await present(w);
        expect(invoke).toHaveBeenCalledWith('present_overlay_window');
        expect(w.setFocus).not.toHaveBeenCalled();
        expect(w.show).not.toHaveBeenCalled();
    });

    it('macOS: falls back to show + setFocus if the backend call fails', async () => {
        const invoke = vi.fn(async () => {
            throw new Error('nope');
        });
        const present = await load('MacIntel', invoke);
        const w = fakeWindow();
        await present(w);
        expect(w.show).toHaveBeenCalled();
        expect(w.setFocus).toHaveBeenCalled();
    });

    it('Windows: plain show + setFocus, no backend call', async () => {
        const invoke = vi.fn(async () => {});
        const present = await load('Win32', invoke);
        const w = fakeWindow();
        await present(w);
        expect(invoke).not.toHaveBeenCalled();
        expect(w.show).toHaveBeenCalled();
        expect(w.setFocus).toHaveBeenCalled();
    });
});
