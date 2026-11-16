/**
 * The agent sign-in wired into the floating window: a plain-string error
 * keeps just Reconnect; a signed-out error object adds Sign in; the run
 * lives in #agentSignInSlot, which a new turn, a reset or a show keeps
 * while it's going; when it ends with the launcher hidden (the user was in
 * the browser) the launcher comes back, except at the time limit; and a
 * clean exit resends the failed turn once, with its attachments, without
 * telling the chat window again — under Reconnect's rules: only the turn
 * whose error was clicked, and not when any of it got through, it was a
 * rate limit or a newer prompt went out.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { marked } from 'marked';
import {
    _resetAgentSignInForTests,
    isAgentSignInActive,
} from '../../ui/js/shared/agent-sign-in.js';
import { _setTerminalFactoryForTests } from '../../ui/js/shared/terminal-view.js';
import { WindowManager } from '../../ui/js/floating/window.js';

// Online: the error path shows the agent's error, not the offline notice.
vi.mock('../../ui/js/shared/network.js', async (importOriginal) => ({
    ...(await importOriginal()),
    checkOnError: async () => true,
    checkOnline: async () => true,
}));

let FloatingApp;
beforeAll(async () => {
    globalThis.marked = marked;
    ({ FloatingApp } = await import('../../ui/js/floating/app.js'));
});

const flush = async (n = 8) => {
    for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0));
};
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

const CTX = {
    available: true,
    agent: 'Kiro CLI',
    command: 'kiro-cli login',
    terminal: false,
    timeoutMinutes: 15,
};
const SIGNED_OUT = { kind: 'auth_required', message: 'Signed out', sessionId: 's1' };

let terms;
function fakeTerminal(opts) {
    const term = {
        opts,
        cols: 80,
        rows: opts.rows,
        options: {},
        open: vi.fn(),
        write: vi.fn(),
        focus: vi.fn(),
        dispose: vi.fn(),
        loadAddon: vi.fn(),
        onData: () => ({ dispose() {} }),
    };
    terms.push(term);
    return term;
}

function setup({ ctx = CTX } = {}) {
    document.body.innerHTML = `
        <div class="speech-bubble">
            <div class="content-area" id="contentArea">
                <div class="response-text" id="responseText"></div>
            </div>
            <div class="agent-sign-in-slot" id="agentSignInSlot"></div>
        </div>`;
    const calls = [];
    const starts = [];
    const reconnect = { value: true };
    const invoke = vi.fn(async (cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'agent_sign_in_context') return ctx;
        if (cmd === 'start_agent_sign_in') {
            return new Promise((resolve, reject) => starts.push({ args, resolve, reject }));
        }
        if (cmd === 'reconnect_acp') {
            return { reconnected: reconnect.value, sessionId: 's1', sessionReloaded: true };
        }
        if (cmd === 'get_config') return { ui: { preserve_last_response: false } };
        if (cmd === 'notify_response_ready') return true;
        return null;
    });
    const emit = vi.fn();
    class Channel {
        constructor() {
            this.onmessage = null;
        }
    }
    window.__TAURI__ = {
        core: { invoke, Channel },
        event: { emit, listen: vi.fn(async () => () => {}) },
    };
    const appWindow = {
        show: vi.fn(async () => {}),
        setFocus: vi.fn(async () => {}),
        hide: vi.fn(async () => {}),
        listen: vi.fn(async () => () => {}),
    };
    const app = new FloatingApp(invoke, appWindow, vi.fn(async () => () => {}));
    const div = () => document.createElement('div');
    app.elements = {
        contentArea: document.getElementById('contentArea'),
        responseText: document.getElementById('responseText'),
        loadingDots: div(),
        mascotContainer: div(),
        expandBtn: div(),
        floatingStopBtn: div(),
        appSuggestions: div(),
        input: document.createElement('textarea'),
    };
    app.windowManager = {
        resizeWindow: vi.fn(async () => {}),
        resetHeightForNewMessage: vi.fn(async () => {}),
    };
    app.banner = { dismiss: vi.fn() };
    app.attachmentManager = { clear: vi.fn() };
    app.stopThinking = vi.fn();
    app.startThinking = vi.fn();
    app.updateDatetimeVisibility = vi.fn();
    app._removeShellActivity = vi.fn();
    app._finishShellActivity = vi.fn();
    app._showFloatingResponseActions = vi.fn();
    app._showResponseActions = vi.fn();
    app._refreshContextUsage = vi.fn();
    app.floatingSessionId = 's1';
    app.isWaitingForResponse = true;
    const named = (name) => calls.filter(([c]) => c === name);
    return { app, appWindow, invoke, emit, starts, named, reconnect };
}

async function failTurn(app, payload) {
    app.isWaitingForResponse = true;
    await app.messageStreamController.handleError({ payload });
    await flush();
}

/** Send a turn the way handleInput does, and let the backend take it. */
async function sendTurn(app, message, attachments = null) {
    await app._startResponseTurn();
    await app._sendTrackedTurn({ sessionId: 's1', message, attachments });
    app._settleTurn('s1');
}

/** Sign in from the current error and let `kiro-cli login` exit 0. */
async function signInCleanly(starts) {
    await clickSignIn();
    starts[starts.length - 1].resolve({ outcome: 'exited', exitCode: 0 });
    await flush(12);
}

const sends = (named) => named('send_message_streaming').map(([, args]) => args.message);

async function clickSignIn() {
    const btn = document.querySelector('#responseText .agent-sign-in-btn');
    btn.dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
    await flush();
}

beforeEach(() => {
    terms = [];
    _setTerminalFactoryForTests(fakeTerminal);
    _resetAgentSignInForTests();
    window._kageFloatingHidden = false;
});

afterEach(() => {
    _resetAgentSignInForTests();
    _setTerminalFactoryForTests(null);
    delete window.__TAURI__;
    delete window._kageFloatingHidden;
    document.body.innerHTML = '';
});

describe('floating: the error row', () => {
    it('a plain-string error keeps just Reconnect', async () => {
        const { app, named } = setup();
        await failTurn(app, 'Error: dispatch failure');
        expect(document.querySelector('#responseText .floating-error-reconnect')).toBeTruthy();
        expect(document.querySelector('.agent-sign-in-btn')).toBeNull();
        expect(named('agent_sign_in_context').length).toBe(0);
    });

    it('a signed-out error object adds Sign in beside Reconnect', async () => {
        const { app } = setup();
        await failTurn(app, SIGNED_OUT);
        const actions = document.querySelector('#responseText .floating-error-actions');
        expect(actions.querySelector('.agent-sign-in-btn')).toBeTruthy();
        expect(actions.querySelector('.floating-error-reconnect')).toBeTruthy();
    });
});

describe('floating: a live sign-in', () => {
    it('opens in the slot, arms blur suppression, and survives a new turn and a reset', async () => {
        const { app, starts } = setup();
        await failTurn(app, SIGNED_OUT);
        const t0 = Date.now();
        await clickSignIn();
        expect(starts.length).toBe(1);
        expect(starts[0].args.rows).toBe(10);
        expect(app._suppressBlurHideUntil).toBeGreaterThanOrEqual(t0 + 1000);
        const slot = document.getElementById('agentSignInSlot');
        expect(slot.querySelector('.agent-sign-in')).toBeTruthy();
        // The floating window says it may hide while the browser is up.
        expect(slot.querySelector('.agent-sign-in-hint-hides')).toBeTruthy();
        // The launcher's size floor counts the panel as content.
        expect(WindowManager.prototype._hasResponseContent.call({})).toBe(true);

        app.isWaitingForResponse = false;
        await app._clearResponseForNewTurn();
        expect(slot.querySelector('.agent-sign-in')).toBeTruthy();
        app.resetUI();
        expect(slot.querySelector('.agent-sign-in')).toBeTruthy();
    });

    it('a show while it runs keeps it and puts the keyboard back in the terminal', async () => {
        const { app } = setup();
        await failTurn(app, SIGNED_OUT);
        await clickSignIn();
        app.isWaitingForResponse = false;
        app.resetUI = vi.fn();
        app._isWindowVisible = async () => true;
        app._updateBannerChecked = true;
        app._refreshAppModeChip = async () => {};
        app._updateToolbarVisibility = vi.fn();
        app._restoreOverlaysAfterClipboard = vi.fn();
        app.currentMatches = [];
        terms[0].focus.mockClear();
        app._visibilitySequence = 0;
        await app._onFocus();
        await wait(80);
        expect(app.resetUI).not.toHaveBeenCalled();
        expect(terms[0].focus).toHaveBeenCalled();
    });

    it('a finished sign-in leaves with the next turn', async () => {
        const { app, starts } = setup();
        await failTurn(app, SIGNED_OUT);
        await clickSignIn();
        starts[0].resolve({ outcome: 'exited', exitCode: 1 });
        await flush();
        expect(isAgentSignInActive()).toBe(false);
        app.isWaitingForResponse = false;
        await app._clearResponseForNewTurn();
        expect(document.getElementById('agentSignInSlot').children.length).toBe(0);
    });
});

describe('floating: when the run ends', () => {
    it('hidden for the browser: the launcher comes back', async () => {
        const { app, appWindow, starts } = setup();
        await failTurn(app, SIGNED_OUT);
        await clickSignIn();
        window._kageFloatingHidden = true;
        app._suppressBlurHideUntil = 0;
        const t0 = Date.now();
        starts[0].resolve({ outcome: 'exited', exitCode: 1 });
        await flush();
        expect(appWindow.show).toHaveBeenCalled();
        expect(appWindow.setFocus).toHaveBeenCalled();
        expect(app._suppressBlurHideUntil).toBeGreaterThanOrEqual(t0 + 1000);
    });

    it('at the time limit: a notification, not a re-show', async () => {
        const { app, appWindow, starts, named } = setup();
        await failTurn(app, SIGNED_OUT);
        await clickSignIn();
        window._kageFloatingHidden = true;
        starts[0].resolve({ outcome: 'timed_out', exitCode: null, signal: 'Terminated' });
        await flush();
        expect(appWindow.show).not.toHaveBeenCalled();
        const [[, args]] = named('notify_response_ready');
        expect(args.targetLabel).toBe('floating');
    });

    it('a clean exit resends the failed turn once, with its attachments', async () => {
        const { app, emit, starts, named } = setup();
        const attachments = [{ name: 'a.png', data: 'x' }];
        await sendTurn(app, 'hello agent', attachments);
        await failTurn(app, SIGNED_OUT);
        await clickSignIn();
        emit.mockClear();
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(12);
        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: 's1' }]]);
        expect(named('send_message_streaming').slice(1)).toEqual([
            ['send_message_streaming', { sessionId: 's1', message: 'hello agent', attachments }],
        ]);
        expect(emit.mock.calls.filter(([name]) => name === 'floating_message_sent')).toEqual([]);
        expect(document.querySelector('.agent-sign-in-status').textContent).toBe(
            'shared.agent_sign_in.status.done'
        );
    });

    it('a failed sign-in sends nothing', async () => {
        const { app, starts, named } = setup();
        await sendTurn(app, 'hello agent');
        await failTurn(app, SIGNED_OUT);
        await clickSignIn();
        starts[0].resolve({ outcome: 'exited', exitCode: 2 });
        await flush();
        expect(named('reconnect_acp').length).toBe(0);
        expect(sends(named)).toEqual(['hello agent']);
    });
});

describe('floating: what a sign-in resends', () => {
    const UNREACHABLE = {
        kind: 'agent_unreachable',
        message: 'Kage restarted Kiro and tried again, but it still failed.',
        sessionId: 's1',
    };

    it('not a turn that already streamed output', async () => {
        const { app, starts, named } = setup();
        await sendTurn(app, 'write the file');
        app.currentResponse = 'Writing the file now…';
        await failTurn(app, UNREACHABLE);
        await signInCleanly(starts);
        expect(named('reconnect_acp').length).toBe(1);
        expect(sends(named)).toEqual(['write the file']);
        expect(document.querySelector('.agent-sign-in-status').textContent).toBe(
            'shared.agent_sign_in.status.ready'
        );
    });

    it('not a turn that ran a tool', async () => {
        const { app, starts, named } = setup();
        await sendTurn(app, 'run it');
        app.toolUsages = [{ id: 't1', title: 'shell' }];
        await failTurn(app, UNREACHABLE);
        await signInCleanly(starts);
        expect(sends(named)).toEqual(['run it']);
    });

    it('not a rate-limited turn', async () => {
        const { app, starts, named } = setup();
        await sendTurn(app, 'hello agent');
        await failTurn(app, { ...UNREACHABLE, message: 'Too many requests (429)' });
        await signInCleanly(starts);
        expect(sends(named)).toEqual(['hello agent']);
    });

    it('not once a newer prompt went out', async () => {
        const { app, starts, named } = setup();
        await sendTurn(app, 'hello agent');
        await failTurn(app, SIGNED_OUT);
        app._promptGeneration++;
        await signInCleanly(starts);
        expect(sends(named)).toEqual(['hello agent']);
    });

    it('the turn that failed, not an earlier failed one', async () => {
        const { app, starts, named } = setup();
        await sendTurn(app, 'prompt A');
        await failTurn(app, 'Error: dispatch failure');
        // A command's result handed back to the agent ("Run it here").
        app.isWaitingForResponse = false;
        await app.sendAgentPrompt('s1', 'shell result B');
        app._settleTurn('s1');
        await failTurn(app, SIGNED_OUT);
        await signInCleanly(starts);
        expect(sends(named)).toEqual(['prompt A', 'shell result B', 'shell result B']);
    });
});
