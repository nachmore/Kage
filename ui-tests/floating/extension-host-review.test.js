import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../ui/js/floating/app/dependencies.js', () => ({
    buildExecCtx: vi.fn((args) => args.extra),
    getConfig: vi.fn(),
    showExtensionBar: vi.fn(),
    updateExtensionBar: vi.fn(),
    hideExtensionBar: vi.fn(),
    formatBytes: vi.fn(),
    checkOnline: vi.fn(async () => true),
    signalMascot: vi.fn(),
    signalMascotUpdated: vi.fn(),
}));

import { getConfig, showExtensionBar, updateExtensionBar, signalMascot, signalMascotUpdated } from '../../ui/js/floating/app/dependencies.js';
import { CommandsMethods } from '../../ui/js/floating/app/commands.js';
import { LifecycleVisibilityMethods } from '../../ui/js/floating/app/lifecycle-visibility.js';
import { UiStateMethods } from '../../ui/js/floating/app/ui-state.js';

beforeEach(() => {
    vi.useFakeTimers();
    delete window._kageFloatingHidden;
    delete window._kageMascot;
});

afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
    document.documentElement.classList.remove('animations-paused');
    document.getElementById('kage-anim-pause-style')?.remove();
    delete window._kageFloatingHidden;
    delete window._kageMascot;
});

function visibilityFixture() {
    const listeners = {};
    const app = {
        ...UiStateMethods,
        ...LifecycleVisibilityMethods,
        appWindow: {
            listen: (event, callback) => { listeners[event] = callback; },
            isVisible: vi.fn(async () => true),
            hide: vi.fn(async () => {}),
        },
        banner: { checkForUpdateBanner: vi.fn(async () => true), dismiss: vi.fn() },
        extensionManager: { setWidgetVisibility: vi.fn(async () => {}) },
        windowManager: { resizeWindow: vi.fn(), _hasResponseContent: () => true },
        _refreshAppModeChip: vi.fn(async () => {}),
        _updateToolbarVisibility: vi.fn(),
        _restoreOverlaysAfterClipboard: vi.fn(),
        updateDatetimeVisibility: vi.fn(),
        invoke: vi.fn(async (cmd) => cmd === 'ollama_probe' ? { status: 'Reachable' } : []),
        currentMatches: [],
        _noBlurTools: new Set(),
        isWaitingForResponse: true,
        elements: { input: document.createElement('input') },
    };
    app.setupVisibilityTracking();
    return { app, focus: listeners['tauri://focus'], blur: listeners['tauri://blur'] };
}

describe('floating extension host review regressions', () => {
    it('reads visibility once and signals reopen, summon, then updated', async () => {
        const { app, focus } = visibilityFixture();
        const order = [];
        signalMascot.mockImplementation((name) => order.push(name));
        signalMascotUpdated.mockImplementation(() => order.push('updated'));
        window._kageMascot = { resume: vi.fn() };
        await focus();
        expect(order).toEqual(['reopen', 'summon', 'updated']);
        expect(app.appWindow.isVisible).toHaveBeenCalledTimes(1);
        expect(window._kageMascot.resume).toHaveBeenCalledTimes(1);
    });

    it('ignores a visible reply delivered after blur hid the window', async () => {
        const { app, focus, blur } = visibilityFixture();
        let release;
        app.appWindow.isVisible.mockImplementation(() => new Promise((resolve) => { release = resolve; }));
        window._kageMascot = { resume: vi.fn(), pause: vi.fn() };
        app._ollamaStatus = { baseUrl: 'http://localhost:11434', probeSkipped: true };
        const pending = focus();
        await blur();
        release(true);
        await pending;
        expect(window._kageFloatingHidden).toBe(true);
        expect(app.extensionManager.setWidgetVisibility).not.toHaveBeenCalledWith(true);
        expect(window._kageMascot.resume).not.toHaveBeenCalled();
        expect(window._kageMascot.pause).toHaveBeenCalledTimes(1);
        expect(app.banner.checkForUpdateBanner).not.toHaveBeenCalled();
        expect(app.invoke).not.toHaveBeenCalled();
        expect(signalMascot).not.toHaveBeenCalled();
    });

    it('debounces show probes, catches hidden skips, and clears disabled Ollama status', async () => {
        const { app, focus, blur } = visibilityFixture();
        app._updateBannerChecked = true;
        app._ollamaStatus = { baseUrl: 'http://lan.invalid:11434', model: 'test' };
        await app._pollOllamaStatusOnce();
        for (let i = 0; i < 10; i++) {
            await blur();
            await focus();
        }
        const probes = () => app.invoke.mock.calls.filter(([cmd]) => cmd === 'ollama_probe').length;
        expect(probes()).toBe(1);
        await blur();
        await app._pollOllamaStatusOnce(); // scheduled poll skipped while hidden
        await focus();
        expect(probes()).toBe(2);
        await blur();
        vi.setSystemTime(Date.now() + 30_001);
        await focus();
        expect(probes()).toBe(3);
        getConfig.mockResolvedValue({});
        await app._refreshOllamaStatusWidget();
        expect(app._ollamaStatus).toBeNull();
        await blur();
        await focus();
        expect(probes()).toBe(3);
    });

    it('does not drop a re-seed while the previous Ollama probe is pending', async () => {
        const { app } = visibilityFixture();
        const cfg = (base_url) => ({
            acp: { active_connection_id: 'c', connections: [{
                id: 'c', preset_id: 'ollama',
                ollama_settings: { show_status_widget: true, base_url },
            }] },
        });
        let release;
        app.invoke.mockImplementationOnce(() => new Promise((resolve) => { release = resolve; }));
        getConfig.mockResolvedValue(cfg('http://old.invalid'));
        await app._refreshOllamaStatusWidget();
        getConfig.mockResolvedValue(cfg('http://new.invalid'));
        await app._refreshOllamaStatusWidget();
        await vi.advanceTimersByTimeAsync(0);
        expect(app.invoke).toHaveBeenCalledWith('ollama_probe', { baseUrl: 'http://new.invalid' });
        release({ status: 'Reachable' });
        await vi.advanceTimersByTimeAsync(0);
        expect(app.invoke).not.toHaveBeenCalledWith('ollama_list_models', { baseUrl: 'http://old.invalid' });
        app._stopOllamaStatusPoll();
    });
    it('refresh_widgets requests a forced repaint after the API settle delay', async () => {
        const renderAllWidgets = vi.fn(async () => {});
        const app = {
            extensionManager: { renderAllWidgets },
            elements: { input: document.createElement('input') },
        };
        const context = CommandsMethods._getExecCtx.call(app);
        context.onRefreshWidgets();
        await vi.advanceTimersByTimeAsync(299);
        expect(renderAllWidgets).not.toHaveBeenCalled();
        await vi.advanceTimersByTimeAsync(1);
        expect(renderAllWidgets).toHaveBeenCalledExactlyOnceWith({ force: true });
    });

    it('startup pauses widgets while allowing the immediate Ollama status probe', async () => {
        getConfig.mockResolvedValue({
            acp: {
                active_connection_id: 'local',
                connections: [{
                    id: 'local',
                    preset_id: 'ollama',
                    ollama_settings: {
                        show_status_widget: true,
                        base_url: 'http://localhost:11434',
                        model: 'test-model',
                    },
                }],
            },
        });
        const app = {
            ...UiStateMethods,
            extensionManager: { setWidgetVisibility: vi.fn() },
            appWindow: { listen: vi.fn() },
            invoke: vi.fn(async (cmd) => cmd === 'ollama_probe'
                ? { status: 'Reachable', version: 'test' }
                : []),
        };
        LifecycleVisibilityMethods.setupVisibilityTracking.call(app);
        expect(app.extensionManager.setWidgetVisibility).toHaveBeenCalledWith(false);
        expect(window._kageFloatingHidden).toBeUndefined();
        await app._refreshOllamaStatusWidget();
        await vi.advanceTimersByTimeAsync(0);
        expect(showExtensionBar).toHaveBeenCalledWith(expect.objectContaining({ text: 'test-model · checking…' }));
        expect(app.invoke).toHaveBeenCalledWith('ollama_probe', { baseUrl: 'http://localhost:11434' });
        expect(updateExtensionBar).toHaveBeenCalledWith('ollama-status', {
            text: 'test-model · ready (Ollama test)',
        });
        app._stopOllamaStatusPoll();
    });

    it('probes Ollama on a real show if startup focused the hidden webview first', async () => {
        const listeners = {};
        const app = {
            _ollamaStatus: { probeSkipped: true },
            _isWindowVisible: vi.fn(async () => true),
            _updateBannerChecked: true,
            _pollOllamaStatusOnce: vi.fn(async () => {}),
            _refreshAppModeChip: vi.fn(async () => {}),
            _updateToolbarVisibility: vi.fn(),
            _restoreOverlaysAfterClipboard: vi.fn(),
            updateDatetimeVisibility: vi.fn(),
            windowManager: { resizeWindow: vi.fn() },
            extensionManager: { setWidgetVisibility: vi.fn(async () => {}) },
            appWindow: { listen: (event, callback) => { listeners[event] = callback; } },
            invoke: vi.fn(async () => {}),
            currentMatches: [],
            isWaitingForResponse: true,
            elements: { input: document.createElement('input') },
        };
        LifecycleVisibilityMethods.setupVisibilityTracking.call(app);
        window._kageFloatingHidden = true;
        await listeners['tauri://focus']();
        expect(app._pollOllamaStatusOnce).toHaveBeenCalledTimes(1);
        expect(window._kageFloatingHidden).toBe(false);
        await listeners['tauri://focus']();
        expect(app._pollOllamaStatusOnce).toHaveBeenCalledTimes(1);
    });
});
