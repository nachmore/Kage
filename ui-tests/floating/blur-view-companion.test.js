/**
 * The launcher stays beside an extension view it opened, and still hides
 * when the user leaves Kage (docs/design/EXTENSION_VIEWS.md §15.2):
 *
 * - Rule A: opening a view arms the blur suppression and marks the
 *   launcher as the view's companion, before `ext_view_open`.
 * - Rule B: a blur that moved focus to a view keeps the launcher;
 *   `ext_view_focus_lost` hides it only when it's a companion and isn't
 *   focused itself.
 * - `floating_compose` fills the input, selects it and never sends.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../ui/js/floating/app/dependencies.js', async (importOriginal) => ({
    ...(await importOriginal()),
    checkOnline: vi.fn(async () => true),
    getConfig: vi.fn(async () => ({ ui: {} })),
    onNetworkChange: vi.fn(),
    renderUnifiedResults: vi.fn(),
    signalMascot: vi.fn(),
    signalMascotUpdated: vi.fn(),
    unifiedSearch: vi.fn(async () => []),
}));

import { getConfig } from '../../ui/js/floating/app/dependencies.js';
import {
    LifecycleVisibilityMethods,
    VIEW_OPEN_BLUR_GRACE_MS,
} from '../../ui/js/floating/app/lifecycle-visibility.js';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

beforeEach(() => {
    vi.useFakeTimers();
    delete window._kageFloatingHidden;
    delete window._kagePostUpdateSuppressUntil;
});

afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
    document.documentElement.classList.remove('animations-paused');
    document.getElementById('kage-anim-pause-style')?.remove();
    document.getElementById('permissionModal')?.remove();
    delete window._kageFloatingHidden;
});

/**
 * A launcher with the visibility mixin. `viewFocused` is what
 * `ext_view_focus_state` answers; `viewCapable` what the extension manager
 * says about view-capable extensions.
 */
function launcher({ viewFocused = false, viewCapable = true, visible = true } = {}) {
    const listeners = {};
    const input = document.createElement('input');
    const app = {
        ...LifecycleVisibilityMethods,
        appWindow: {
            listen: (event, callback) => {
                listeners[event] = callback;
            },
            isVisible: vi.fn(async () => visible),
            hide: vi.fn(async () => {}),
        },
        banner: { checkForUpdateBanner: vi.fn(async () => false), dismiss: vi.fn() },
        extensionManager: {
            setWidgetVisibility: vi.fn(async () => {}),
            hasViewCapableExtensions: vi.fn(() => viewCapable),
        },
        windowManager: { resizeWindow: vi.fn(), _hasResponseContent: () => true },
        _refreshAppModeChip: vi.fn(async () => {}),
        _updateToolbarVisibility: vi.fn(),
        _restoreOverlaysAfterClipboard: vi.fn(),
        _pollOllamaStatusOnce: vi.fn(async () => {}),
        updateDatetimeVisibility: vi.fn(),
        clearSuggestions: vi.fn(),
        resetUI: vi.fn(),
        sendChatMessage: vi.fn(),
        _updateBannerChecked: true,
        invoke: vi.fn(async (cmd) =>
            cmd === 'ext_view_focus_state' ? { view_focused: viewFocused } : undefined
        ),
        currentMatches: [],
        _noBlurTools: new Set(),
        isWaitingForResponse: false,
        elements: { input },
    };
    app.setupVisibilityTracking();
    return {
        app,
        input,
        focus: () => listeners['tauri://focus'](),
        blur: () => listeners['tauri://blur'](),
        focusLost: () => listeners.ext_view_focus_lost?.({ payload: null }),
        compose: (payload) => listeners.floating_compose?.({ payload }),
        listeners,
    };
}

const askedViewFocus = (app) =>
    app.invoke.mock.calls.filter(([cmd]) => cmd === 'ext_view_focus_state').length;

describe('Rule A: opening a view from the launcher', () => {
    it('arms the blur suppression and the companion flag', () => {
        const { app } = launcher();
        const now = Date.now();
        app._armViewCompanion();
        expect(app._viewCompanion).toBe(true);
        expect(app._suppressBlurHideUntil).toBe(now + VIEW_OPEN_BLUR_GRACE_MS);
    });

    it('never shortens a longer suppression (the update banner)', () => {
        const { app } = launcher();
        app._suppressBlurHideUntil = Date.now() + 5000;
        app._armViewCompanion();
        expect(app._suppressBlurHideUntil).toBe(Date.now() + 5000);
    });

    it('a blur while the view window is built neither hides nor asks Rust', async () => {
        const { app, blur } = launcher({ viewFocused: false });
        app._armViewCompanion();
        await blur();
        expect(app.appWindow.hide).not.toHaveBeenCalled();
        expect(askedViewFocus(app)).toBe(0);
        // After the grace period an ordinary blur hides again.
        vi.advanceTimersByTime(VIEW_OPEN_BLUR_GRACE_MS + 1);
        await blur();
        expect(app.appWindow.hide).toHaveBeenCalledTimes(1);
    });

    it('is armed before ext_view_open by the launcher-wired extension manager', async () => {
        const { app } = launcher();
        let stateAtOpen = null;
        const invoke = vi.fn(async (cmd) => {
            if (cmd === 'ext_view_open') {
                stateAtOpen = {
                    companion: app._viewCompanion,
                    suppressed: app._suppressBlurHideUntil > Date.now(),
                };
                return { label: 'ext-view:mindmap:explorer', reused: false };
            }
            return undefined;
        });
        const mgr = new ExtensionManager(invoke, {
            hostFeatures: ['views'],
            resultSource: 'launcher',
            onBeforeOpenView: () => app._armViewCompanion(),
        });
        mgr._configCache = { extension_states: {} };
        mgr.extensions.set('mindmap', {
            manifest: {
                id: 'mindmap',
                contributes: { views: [{ id: 'explorer', title: 'E', module: './e.js' }] },
            },
            capabilities: ['views'],
            sandbox: null,
        });
        const out = await mgr.openView('mindmap', { type: 'open_view', view: 'explorer' });
        expect(out.ok).toBe(true);
        expect(stateAtOpen).toEqual({ companion: true, suppressed: true });
    });

    it('the real launcher wires its extension manager for views', async () => {
        const { FloatingApp } = await import('../../ui/js/floating/app.js');
        const floating = new FloatingApp(
            vi.fn(async () => null),
            { label: 'floating', listen: vi.fn() },
            vi.fn(async () => () => {})
        );
        const mgr = floating.extensionManager;
        expect(mgr._hostFeatures).toEqual(expect.arrayContaining(['views', 'date-picker']));
        expect(mgr.resultViewSource).toBe('launcher');
        expect(mgr._windowLabel).toBe('floating');
        floating._armViewCompanion = vi.fn();
        mgr._onBeforeOpenView();
        expect(floating._armViewCompanion).toHaveBeenCalledTimes(1);
    });
});

describe('Rule B: focus moving between the launcher and views', () => {
    it('stays as the companion when focus went to a view', async () => {
        const { app, blur } = launcher({ viewFocused: true });
        await blur();
        expect(app.appWindow.hide).not.toHaveBeenCalled();
        expect(app._viewCompanion).toBe(true);
        expect(askedViewFocus(app)).toBe(1);
    });

    it('hides when focus went anywhere else', async () => {
        const { app, blur } = launcher({ viewFocused: false });
        await blur();
        expect(app.appWindow.hide).toHaveBeenCalledTimes(1);
        expect(window._kageFloatingHidden).toBe(true);
        expect(app._viewCompanion).toBe(false);
    });

    it('asks Rust nothing when no extension can have a view', async () => {
        const { app, blur } = launcher({ viewFocused: true, viewCapable: false });
        await blur();
        expect(askedViewFocus(app)).toBe(0);
        expect(app.appWindow.hide).toHaveBeenCalledTimes(1);
    });

    it('a launcher hidden already (Esc) still runs its hide path', async () => {
        const { app, blur } = launcher({ viewFocused: true, visible: false });
        window._kageMascot = { pause: vi.fn(), resume: vi.fn() };
        await blur();
        expect(app.appWindow.hide).toHaveBeenCalledTimes(1);
        expect(window._kageMascot.pause).toHaveBeenCalledTimes(1);
        expect(app._viewCompanion).toBe(false);
        delete window._kageMascot;
    });

    it("doesn't hide when the launcher got focus back during the question", async () => {
        const { app, blur, focus } = launcher();
        let answer;
        app.invoke.mockImplementation((cmd) =>
            cmd === 'ext_view_focus_state'
                ? new Promise((resolve) => {
                      answer = resolve;
                  })
                : Promise.resolve(undefined)
        );
        const pending = blur();
        await focus();
        answer({ view_focused: false });
        await pending;
        expect(app.appWindow.hide).not.toHaveBeenCalled();
    });

    it('a failed question counts as "not a view" and hides', async () => {
        const { app, blur } = launcher();
        app.invoke.mockImplementation(async () => {
            throw new Error('no such command');
        });
        await blur();
        expect(app.appWindow.hide).toHaveBeenCalledTimes(1);
    });

    it('the existing guards still win (a permission prompt is open)', async () => {
        const { app, blur } = launcher({ viewFocused: false });
        const modal = document.createElement('div');
        modal.id = 'permissionModal';
        document.body.appendChild(modal);
        await blur();
        expect(app.appWindow.hide).not.toHaveBeenCalled();
        expect(askedViewFocus(app)).toBe(0);
    });
});

describe('ext_view_focus_lost', () => {
    it('hides a companion launcher that is not focused', async () => {
        const { app, blur, focusLost } = launcher({ viewFocused: true });
        await blur();
        expect(app._viewCompanion).toBe(true);
        await focusLost();
        expect(app.appWindow.hide).toHaveBeenCalledTimes(1);
        expect(app._viewCompanion).toBe(false);
    });

    it('does nothing without the companion flag', async () => {
        const { app, focusLost } = launcher();
        app._windowFocused = false;
        await focusLost();
        expect(app.appWindow.hide).not.toHaveBeenCalled();
    });

    it('does nothing while the launcher itself is focused', async () => {
        const { app, focusLost } = launcher();
        app._viewCompanion = true;
        app._windowFocused = true;
        await focusLost();
        expect(app.appWindow.hide).not.toHaveBeenCalled();
    });

    it('respects the guards that keep the launcher open', async () => {
        const { app, focusLost } = launcher();
        app._viewCompanion = true;
        app._windowFocused = false;
        app.computerControlActive = true;
        await focusLost();
        expect(app.appWindow.hide).not.toHaveBeenCalled();
    });

    it('a show clears the companion flag', async () => {
        const { app, focus } = launcher();
        window._kageFloatingHidden = true;
        app._viewCompanion = true;
        await focus();
        expect(app._viewCompanion).toBe(false);
    });

    it('listens on the launcher window itself', () => {
        const { listeners } = launcher();
        expect(typeof listeners.ext_view_focus_lost).toBe('function');
        expect(typeof listeners.floating_compose).toBe('function');
    });
});

describe('floating_compose', () => {
    it('replaces the input, selects it and never sends', () => {
        const { app, input, compose } = launcher();
        input.value = 'old text';
        const onInput = vi.fn();
        input.addEventListener('input', onInput);
        document.body.appendChild(input);
        compose({ text: 'Tell me about Bingo' });
        expect(input.value).toBe('Tell me about Bingo');
        expect(document.activeElement).toBe(input);
        expect(input.selectionStart).toBe(0);
        expect(input.selectionEnd).toBe('Tell me about Bingo'.length);
        expect(onInput).toHaveBeenCalledTimes(1);
        expect(app.sendChatMessage).not.toHaveBeenCalled();
        expect(app.invoke).not.toHaveBeenCalled();
    });

    it('ignores a payload without text', () => {
        const { input, compose } = launcher();
        input.value = 'keep';
        compose({});
        compose(null);
        compose({ text: 42 });
        expect(input.value).toBe('keep');
    });

    it("survives the show-time reset when 'preserve last response' is off", async () => {
        getConfig.mockResolvedValue({ ui: { preserve_last_response: false } });
        const { app, input, focus, compose } = launcher();
        window._kageFloatingHidden = true;
        const shown = focus();
        compose({ text: 'composed' });
        await shown;
        await vi.advanceTimersByTimeAsync(60);
        expect(app.resetUI).not.toHaveBeenCalled();
        expect(input.value).toBe('composed');

        // Without a recent compose the reset runs as before.
        await vi.advanceTimersByTimeAsync(2000);
        window._kageFloatingHidden = true;
        await focus();
        await vi.advanceTimersByTimeAsync(60);
        expect(app.resetUI).toHaveBeenCalledTimes(1);
    });
});
