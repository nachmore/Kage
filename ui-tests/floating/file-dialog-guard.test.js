/**
 * The launcher's dialog guard for files an extension asks the user for
 * (docs/design/EXTENSION_FILE_TEXT.md §5.3): the launcher hides before the
 * dialog opens and ignores the blur that causes; when the dialog closes it
 * shows again the usual way, ignores blurs for 1.5 s and keeps what it
 * shows (the extension's answer is still coming).
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../ui/js/floating/app/dependencies.js', async (importOriginal) => ({
    ...(await importOriginal()),
    checkOnline: vi.fn(async () => true),
    getConfig: vi.fn(async () => ({ ui: { preserve_last_response: false } })),
    onNetworkChange: vi.fn(),
    renderUnifiedResults: vi.fn(),
    signalMascot: vi.fn(),
    signalMascotUpdated: vi.fn(),
    unifiedSearch: vi.fn(async () => []),
}));

import {
    FILE_DIALOG_BLUR_GRACE_MS,
    LifecycleVisibilityMethods,
} from '../../ui/js/floating/app/lifecycle-visibility.js';
import { installFileTextBroker } from '../../ui/js/shared/extension-file-text.js';
import { ExtensionSandbox } from '../../ui/js/shared/extension-sandbox-host.js';

beforeEach(() => {
    vi.useFakeTimers();
    window._kageFloatingHidden = false;
});

afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
    document.body.innerHTML = '';
    document.documentElement.classList.remove('animations-paused');
    document.getElementById('kage-anim-pause-style')?.remove();
    delete window._kageFloatingHidden;
    installFileTextBroker({ isVisible: () => true });
});

function launcher() {
    const listeners = {};
    const input = document.createElement('input');
    const bubble = document.createElement('div');
    bubble.className = 'speech-bubble';
    bubble.append(input);
    document.body.append(bubble);
    const order = [];
    const app = {
        ...LifecycleVisibilityMethods,
        appWindow: {
            listen: (event, callback) => {
                listeners[event] = callback;
            },
            isVisible: vi.fn(async () => true),
            hide: vi.fn(async () => order.push('hide')),
            show: vi.fn(async () => order.push('show')),
            setFocus: vi.fn(async () => order.push('focus')),
        },
        listen: vi.fn(async () => () => {}),
        banner: { checkForUpdateBanner: vi.fn(async () => false), dismiss: vi.fn() },
        extensionManager: {
            setWidgetVisibility: vi.fn(async () => {}),
            hasViewCapableExtensions: vi.fn(() => false),
        },
        windowManager: { resizeWindow: vi.fn(), _hasResponseContent: () => true },
        _refreshAppModeChip: vi.fn(async () => {}),
        _updateToolbarVisibility: vi.fn(),
        _restoreOverlaysAfterClipboard: vi.fn(),
        _pollOllamaStatusOnce: vi.fn(async () => {}),
        updateDatetimeVisibility: vi.fn(),
        clearSuggestions: vi.fn(),
        resetUI: vi.fn(),
        _updateBannerChecked: true,
        invoke: vi.fn(async () => undefined),
        currentMatches: [],
        _noBlurTools: new Set(),
        isWaitingForResponse: false,
        elements: { input },
    };
    app.setupVisibilityTracking();
    return {
        app,
        input,
        bubble,
        order,
        focus: () => listeners['tauri://focus'](),
        blur: () => listeners['tauri://blur'](),
    };
}

describe('the launcher around a file dialog', () => {
    it('hides for the dialog and ignores the blur that causes', async () => {
        const { app, blur, order } = launcher();
        await app._beginFileDialog();
        expect(app._nativeDialogOpen).toBe(true);
        expect(order).toEqual(['hide']);
        await blur();
        // The blur handler didn't run its hide path (no second hide, no reset).
        expect(app.appWindow.hide).toHaveBeenCalledTimes(1);
        expect(app.banner.dismiss).not.toHaveBeenCalled();
        expect(window._kageFloatingHidden).toBe(false);
    });

    it('comes back after the dialog, ignoring blurs for a moment', async () => {
        const { app, blur, order } = launcher();
        await app._beginFileDialog();
        const now = Date.now();
        await app._endFileDialog();
        expect(app._nativeDialogOpen).toBe(false);
        expect(app._suppressBlurHideUntil).toBe(now + FILE_DIALOG_BLUR_GRACE_MS);
        expect(order).toEqual(['hide', 'show', 'focus']);
        await blur();
        expect(app.appWindow.hide).toHaveBeenCalledTimes(1);
        vi.advanceTimersByTime(FILE_DIALOG_BLUR_GRACE_MS + 1);
        await blur();
        expect(app.appWindow.hide).toHaveBeenCalledTimes(2);
    });

    it('keeps its content when it shows again after the dialog', async () => {
        const { app, focus, input } = launcher();
        await app._beginFileDialog();
        await app._endFileDialog();
        await focus();
        await vi.advanceTimersByTimeAsync(100);
        expect(app.resetUI).not.toHaveBeenCalled();
        expect(document.activeElement).toBe(input);
        // A later show resets as before (preserve_last_response is off).
        vi.advanceTimersByTime(FILE_DIALOG_BLUR_GRACE_MS);
        await focus();
        await vi.advanceTimersByTimeAsync(100);
        expect(app.resetUI).toHaveBeenCalledTimes(1);
    });

    it('its broker hides it around the dialog and shows the ask bar in the bubble', async () => {
        const { app, bubble, order } = launcher();
        app._installFileTextBroker();
        let finish;
        const rawInvoke = vi.fn((command) => {
            if (command !== 'pick_file_text') return Promise.resolve(true);
            order.push('pick');
            return new Promise((resolve) => {
                finish = resolve;
            });
        });
        const sb = new ExtensionSandbox(
            { extensionId: 'mindmap', capabilities: ['file_text'], config: {}, sources: {} },
            rawInvoke,
            document.body
        );
        sb._port = { postMessage: () => {}, close() {} };
        sb._ready = true;

        // Without a click: the ask bar, at the top of the bubble.
        sb._handleInvoke({ id: 1, command: 'pick_file_text', args: {} });
        await vi.advanceTimersByTimeAsync(0);
        expect(bubble.firstElementChild.className).toBe('kage-file-ask');
        expect(app.windowManager.resizeWindow).toHaveBeenCalled();
        bubble.querySelector('.kage-file-ask-dismiss').click();
        await vi.advanceTimersByTimeAsync(0);

        // After a click: hide, then the dialog; back when Rust answers.
        sb.call('execute', {}, { userGesture: true, timeoutMs: 0 }).catch(() => {});
        const run = sb._handleInvoke({ id: 2, command: 'pick_file_text', args: {} });
        await vi.advanceTimersByTimeAsync(0);
        expect(order).toEqual(['hide', 'pick']);
        expect(app._nativeDialogOpen).toBe(true);
        finish(null);
        await run;
        expect(order).toEqual(['hide', 'pick', 'show', 'focus']);
        expect(app._nativeDialogOpen).toBe(false);
    });
});
