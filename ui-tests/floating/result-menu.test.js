/**
 * Floating launcher: what happens after a result-menu choice — the inline
 * uninstall confirmation (asks first, names the app, Cancel focused, never
 * confirmed by a stray Enter), and how each backend outcome is shown.
 */

import { describe, it, expect, vi } from 'vitest';

// No catalog in tests: render keys with their variables so the app name shows.
vi.mock('../../ui/js/shared/i18n.js', async (importOriginal) => ({
    ...(await importOriginal()),
    t: (key, vars) => (vars ? `${key}${JSON.stringify(vars)}` : key),
}));

const { FloatingApp } = await import('../../ui/js/floating/app.js');

function makeApp(invokeImpl) {
    const appSuggestions = document.createElement('div');
    document.body.appendChild(appSuggestions);
    const input = document.createElement('textarea');
    input.value = 'sl';
    document.body.appendChild(input);
    const app = Object.create(FloatingApp.prototype);
    app.elements = { appSuggestions, input };
    app.windowManager = { resizeWindow: vi.fn(async () => {}) };
    app.appWindow = { hide: vi.fn(async () => {}) };
    app.currentMatches = [];
    app.selectedIndex = -1;
    app.invoke = vi.fn(invokeImpl || (async () => ({ outcome: 'done', name: 'Slack' })));
    app._clearInput = vi.fn();
    app.handleInputChange = vi.fn();
    return app;
}

const APP = { kind: 'app', target: 'Slack' };
const MAC_INFO = { platform: 'macos', kind: 'app', name: 'Slack', actions: ['uninstall'] };
// Two ticks: the confirmation focuses Cancel after present_overlay_window.
const flush = async () => {
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));
};

describe('uninstall confirmation', () => {
    it('names the app and does nothing until confirmed', async () => {
        const app = makeApp();
        const run = app.runResultAction(APP, MAC_INFO, 'uninstall');
        await flush();
        const panel = app.elements.appSuggestions.querySelector('.result-confirm');
        expect(panel).not.toBeNull();
        expect(panel.getAttribute('role')).toBe('alertdialog');
        expect(panel.textContent).toContain('Slack');
        expect(app.invoke).not.toHaveBeenCalledWith('launcher_result_action', expect.anything());
        expect(app._resultConfirm).toBeTruthy();
        expect(document.activeElement).toBe(app._resultConfirm.cancelButton);

        panel.querySelector('.result-confirm-btn.danger').click();
        await run;
        expect(app.invoke).toHaveBeenCalledWith('launcher_result_action', {
            kind: 'app',
            target: 'Slack',
            action: 'uninstall',
            confirmed: true,
        });
        expect(app._resultConfirm).toBeNull();
    });

    it('Cancel and Escape back out without calling the backend', async () => {
        for (const how of ['cancel', 'escape']) {
            const app = makeApp();
            const run = app.runResultAction(APP, MAC_INFO, 'uninstall');
            await flush();
            const panel = app.elements.appSuggestions.querySelector('.result-confirm');
            if (how === 'cancel') {
                panel.querySelector('.result-confirm-btn:not(.danger)').click();
            } else {
                panel.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
            }
            await run;
            expect(app.invoke).not.toHaveBeenCalledWith(
                'launcher_result_action',
                expect.anything()
            );
            expect(app.elements.appSuggestions.querySelector('.result-confirm')).toBeNull();
        }
    });

    it('typing a new query abandons it', async () => {
        const app = makeApp();
        const run = app.runResultAction(APP, MAC_INFO, 'uninstall');
        await flush();
        app._cancelResultConfirm();
        await run;
        expect(app.invoke).not.toHaveBeenCalledWith('launcher_result_action', expect.anything());
    });

    it('Enter in the input never confirms', async () => {
        const app = makeApp();
        const run = app.runResultAction(APP, MAC_INFO, 'uninstall');
        await flush();
        app.handleEnterKey = vi.fn();
        const e = new KeyboardEvent('keydown', { key: 'Enter', cancelable: true });
        await app.handleKeyDown(e);
        expect(e.defaultPrevented).toBe(true);
        expect(app.handleEnterKey).not.toHaveBeenCalled();
        expect(app._resultConfirm).toBeTruthy();
        app._cancelResultConfirm();
        await run;
    });

    it('uses the Windows wording on Windows', async () => {
        const app = makeApp();
        const run = app.runResultAction(APP, { ...MAC_INFO, platform: 'windows' }, 'uninstall');
        await flush();
        const body = app.elements.appSuggestions.querySelector('.result-confirm-body');
        expect(body.textContent).toContain('result_menu.confirm.body_windows');
        app._cancelResultConfirm();
        await run;
    });
});

describe('action outcomes', () => {
    it('hides the launcher after open / reveal / hand-offs', async () => {
        for (const outcome of ['done', 'uninstaller_launched', 'settings_opened']) {
            const app = makeApp(async () => ({ outcome, name: 'Slack' }));
            await app.runResultAction(APP, MAC_INFO, 'open');
            expect(app._clearInput).toHaveBeenCalled();
            expect(app.appWindow.hide).toHaveBeenCalled();
        }
    });

    it('confirms a copy in place without hiding', async () => {
        const app = makeApp(async () => ({ outcome: 'copied', name: 'a.pdf' }));
        await app.runResultAction({ kind: 'path', target: '/a.pdf' }, MAC_INFO, 'copy_path');
        const notice = app.elements.appSuggestions.querySelector('.result-action-notice');
        expect(notice).not.toBeNull();
        expect(notice.getAttribute('role')).toBe('status');
        expect(app.appWindow.hide).not.toHaveBeenCalled();
    });

    it('re-searches after a move to the Trash', async () => {
        vi.useFakeTimers();
        try {
            const app = makeApp(async () => ({ outcome: 'moved_to_trash', name: 'a.pdf' }));
            await app.runResultAction({ kind: 'path', target: '/a.pdf' }, MAC_INFO, 'trash');
            await vi.advanceTimersByTimeAsync(300);
            expect(app.handleInputChange).toHaveBeenCalled();
        } finally {
            vi.useRealTimers();
        }
    });

    it('explains a refused macOS uninstall and keeps the window up', async () => {
        const app = makeApp(async () => ({ outcome: 'revealed_for_manual_removal', name: 'Xcode' }));
        const run = app.runResultAction(APP, MAC_INFO, 'uninstall');
        await flush();
        app.elements.appSuggestions.querySelector('.result-confirm-btn.danger').click();
        await run;
        expect(app.elements.appSuggestions.querySelector('.result-action-notice')).not.toBeNull();
        expect(app._suppressBlurHideUntil).toBeGreaterThan(Date.now());
        expect(app.appWindow.hide).not.toHaveBeenCalled();
    });

    it('shows backend errors as an alert', async () => {
        const app = makeApp(async () => {
            throw { kind: 'internal', message: 'Quit “Slack” first, then try again.' };
        });
        await app.runResultAction(APP, MAC_INFO, 'open');
        const notice = app.elements.appSuggestions.querySelector('.result-action-notice.error');
        expect(notice.textContent).toBe('Quit “Slack” first, then try again.');
        expect(notice.getAttribute('role')).toBe('alert');
        expect(app._resultActionBusy).toBe(false);
    });

    it('stays quiet when the user declined the UAC prompt', async () => {
        const app = makeApp(async () => ({ outcome: 'cancelled', name: 'Git Bash' }));
        await app.runResultAction(APP, MAC_INFO, 'run_as_admin');
        expect(app.elements.appSuggestions.children).toHaveLength(0);
        expect(app.appWindow.hide).not.toHaveBeenCalled();
    });
});
