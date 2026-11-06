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
}));

import { getConfig, showExtensionBar, updateExtensionBar } from '../../ui/js/floating/app/dependencies.js';
import { CommandsMethods } from '../../ui/js/floating/app/commands.js';
import { LifecycleVisibilityMethods } from '../../ui/js/floating/app/lifecycle-visibility.js';
import { UiStateMethods } from '../../ui/js/floating/app/ui-state.js';

beforeEach(() => {
    vi.useFakeTimers();
    delete window._kageFloatingHidden;
});

afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
    document.documentElement.classList.remove('animations-paused');
    document.getElementById('kage-anim-pause-style')?.remove();
    delete window._kageFloatingHidden;
});

describe('floating extension host review regressions', () => {
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
