/**
 * The launcher's tray of cards for commands an agent asked Kage to run
 * outside its sandbox: a card whose request is over goes when the user
 * sends their next prompt and on every reset of the launcher (Ctrl/⌘+L,
 * /clear, a show that clears it: `resetUI`); one still waiting for an
 * answer or its run stays, since the agent is still waiting on that job.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../ui/js/shared/i18n.js', async (importOriginal) => ({
    ...(await importOriginal()),
    t: (key) => key,
}));

const { InputMethods } = await import('../../ui/js/floating/app/input.js');
const { UiStateMethods } = await import('../../ui/js/floating/app/ui-state.js');
const { disposeHostCommandUi, installHostCommandUi, onHostCommandUpdate, showHostCommandPrompt } =
    await import('../../ui/js/shared/host-command-view.js');

function prompt(requestId) {
    return {
        requestId,
        command: 'brew install --cask wallspace',
        cwd: '/Users/me',
        reason: 'sandboxed',
        matched: true,
        plain: true,
        canAlways: false,
        guard: [],
        expiresAt: Date.now() + 300000,
    };
}

function launcher() {
    const el = () => document.createElement('div');
    return {
        elements: { contentArea: el(), expandBtn: el() },
        _removeShellActivity: vi.fn(),
        windowManager: { resetHeightForNewMessage: vi.fn(async () => {}) },
        startThinking: vi.fn(),
        updateDatetimeVisibility: vi.fn(),
        _clearResponseForNewTurn: InputMethods._clearResponseForNewTurn,
    };
}

afterEach(() => {
    disposeHostCommandUi();
    delete window.__TAURI__;
    document.body.innerHTML = '';
});

describe('the launcher tray on a new prompt', () => {
    it('drops the cards that are over and keeps the one still waiting', async () => {
        window.__TAURI__ = {
            core: { invoke: vi.fn(async () => []) },
            event: { listen: vi.fn(async () => () => {}) },
        };
        const tray = document.createElement('div');
        document.body.append(tray);
        await installHostCommandUi({ tray });
        showHostCommandPrompt(prompt('hc-withdrawn'));
        onHostCommandUpdate({ requestId: 'hc-withdrawn', phase: 'closed', reason: 'withdrawn' });
        showHostCommandPrompt(prompt('hc-waiting'));

        const app = launcher();
        await app._clearResponseForNewTurn();

        const left = Array.from(tray.querySelectorAll('.host-cmd-card')).map(
            (c) => c.dataset.requestId
        );
        expect(left).toEqual(['hc-waiting']);
        expect(app._removeShellActivity).toHaveBeenCalled();
    });
});

/** A launcher with what `resetUI` touches. */
function resettable() {
    const el = (tag = 'div') => document.createElement(tag);
    const elements = {
        input: el('textarea'),
        appSuggestions: el(),
        contentArea: el(),
        expandBtn: el(),
        floatingStopBtn: el(),
    };
    return {
        ...UiStateMethods,
        elements,
        attachmentManager: { clear: vi.fn() },
        stopThinking: vi.fn(),
        _removeShellActivity: vi.fn(),
        updateDatetimeVisibility: vi.fn(),
        speech: null,
    };
}

describe('the launcher tray on a reset', () => {
    it('keeps only the cards still waiting, however the launcher was reset', async () => {
        window.__TAURI__ = {
            core: { invoke: vi.fn(async () => []) },
            event: { listen: vi.fn(async () => () => {}) },
        };
        const tray = document.createElement('div');
        document.body.append(tray);
        await installHostCommandUi({ tray });
        showHostCommandPrompt(prompt('hc-timed-out'));
        onHostCommandUpdate({ requestId: 'hc-timed-out', phase: 'closed', reason: 'expired' });
        showHostCommandPrompt(prompt('hc-waiting'));

        // Not only Ctrl/⌘+L: a launcher that clears itself when it's shown
        // again (no "keep last response"), or after a shortcut or a path
        // it opened, goes through the same reset.
        resettable().resetUI();

        const left = Array.from(tray.querySelectorAll('.host-cmd-card')).map(
            (c) => c.dataset.requestId
        );
        expect(left).toEqual(['hc-waiting']);
    });
});
