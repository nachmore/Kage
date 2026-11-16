/**
 * The agent sign-in wired into the chat window: Sign in sits in the error's
 * actions for a signed-out error object (never for a plain string); the run
 * lives in #agentSignInSlot, outside #errorContainer, so a later error or a
 * session switch can't remove it while it's going; a clean exit resends the
 * failed turn once, with its attachments and no second user bubble, under
 * Reconnect's rules (only the turn whose error was clicked, and not when any
 * of it got through, it was a rate limit or a newer prompt went out); any
 * other ending sends nothing; and the window comes back when it ends.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { marked } from 'marked';
import {
    _resetAgentSignInForTests,
    isAgentSignInActive,
} from '../../ui/js/shared/agent-sign-in.js';
import { _setTerminalFactoryForTests } from '../../ui/js/shared/terminal-view.js';

let ChatApp;
beforeAll(async () => {
    globalThis.marked = marked;
    ({ ChatApp } = await import('../../ui/js/chat/app.js'));
});

const flush = async (n = 10) => {
    for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0));
};

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

function setup() {
    vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ text: async () => '<svg xmlns="http://www.w3.org/2000/svg"/>' }))
    );
    document.body.innerHTML = `
        <div id="errorContainer"></div>
        <div id="agentSignInSlot" class="agent-sign-in-slot"></div>
        <div class="messages-area" id="messagesArea"></div>
        <textarea id="chatInput"></textarea>`;
    const calls = [];
    const starts = [];
    const invoke = vi.fn(async (cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'agent_sign_in_context') return CTX;
        if (cmd === 'start_agent_sign_in') {
            return new Promise((resolve, reject) => starts.push({ args, resolve, reject }));
        }
        if (cmd === 'reconnect_acp') {
            return { reconnected: true, sessionId: 's1', sessionReloaded: true };
        }
        if (cmd === 'get_config') return {};
        if (cmd === 'notify_response_ready') return true;
        return null;
    });
    class Channel {
        constructor() {
            this.onmessage = null;
        }
    }
    window.__TAURI__ = { core: { invoke, Channel }, event: { emit: vi.fn() } };
    const appWindow = {
        label: 'main',
        show: vi.fn(async () => {}),
        setFocus: vi.fn(async () => {}),
    };
    const app = new ChatApp(invoke, appWindow, vi.fn(async () => () => {}));
    app.elements = {
        errorContainer: document.getElementById('errorContainer'),
        messagesArea: document.getElementById('messagesArea'),
        chatInput: document.getElementById('chatInput'),
    };
    for (const m of [
        'showTypingIndicator',
        'hideTypingIndicator',
        'updateInputState',
        'scrollToBottom',
        'updateConnectionStatus',
    ]) {
        app[m] = vi.fn();
    }
    app.activeSessionId = 's1';
    app.isWaitingForResponse = false;
    const named = (name) => calls.filter(([c]) => c === name);
    return { app, appWindow, starts, named };
}

/** The turn failed with `payload` (MESSAGE_ERROR), as the stream reports it. */
async function failTurn(app, payload) {
    app.isWaitingForResponse = true;
    await app.messageStreamController.host.onError({ payload }, true);
    await flush();
}

/** Sign in from the current error and let `kiro-cli login` exit 0. */
async function signInCleanly(starts) {
    await clickSignIn();
    starts[starts.length - 1].resolve({ outcome: 'exited', exitCode: 0 });
    await flush();
}

const sends = (named) => named('send_message_streaming').map(([, args]) => args.message);

async function clickSignIn() {
    const btn = document.querySelector('#errorContainer .agent-sign-in-btn');
    btn.dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
    await flush();
}

beforeEach(() => {
    terms = [];
    _setTerminalFactoryForTests(fakeTerminal);
    _resetAgentSignInForTests();
});

afterEach(() => {
    _resetAgentSignInForTests();
    _setTerminalFactoryForTests(null);
    delete window.__TAURI__;
    document.body.innerHTML = '';
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

describe('chat: the error row', () => {
    it('a plain-string error has no Sign in and asks nothing', async () => {
        const { app, named } = setup();
        app.showError('Error: dispatch failure', { signIn: 'Error: dispatch failure' });
        await flush();
        expect(document.querySelector('.agent-sign-in-btn')).toBeNull();
        expect(named('agent_sign_in_context').length).toBe(0);
        expect(document.getElementById('errorReconnectBtn')).toBeTruthy();
    });

    it('a signed-out error object gets Sign in first in .chat-error-actions', async () => {
        const { app } = setup();
        app.showError('Signed out', { signIn: SIGNED_OUT });
        await flush();
        const actions = document.querySelector('#errorContainer .chat-error-actions');
        const btn = actions.querySelector('.agent-sign-in-btn');
        expect(btn).toBeTruthy();
        expect(actions.firstElementChild).toBe(btn);
        expect(btn.classList.contains('chat-error-btn')).toBe(true);
    });

    it('is wired from MESSAGE_ERROR', async () => {
        const { app } = setup();
        app.isWaitingForResponse = true;
        await app.messageStreamController.host.onError({ payload: SIGNED_OUT }, true);
        await flush();
        expect(document.querySelector('#errorContainer .agent-sign-in-btn')).toBeTruthy();
    });
});

describe('chat: a live sign-in', () => {
    it('a later error or a session switch leaves it in place', async () => {
        const { app, starts } = setup();
        app.showError('Signed out', { signIn: SIGNED_OUT });
        await flush();
        await clickSignIn();
        expect(starts.length).toBe(1);
        expect(starts[0].args.rows).toBe(16);
        const slot = document.getElementById('agentSignInSlot');
        expect(slot.querySelector('.agent-sign-in')).toBeTruthy();
        app.showError('Something else');
        expect(slot.querySelector('.agent-sign-in')).toBeTruthy();
        // What a session switch calls (session-history.js).
        app._clearAgentSignInSlot();
        expect(slot.querySelector('.agent-sign-in')).toBeTruthy();
        // Once it's over, the next switch clears it.
        starts[0].resolve({ outcome: 'exited', exitCode: 1 });
        await flush();
        expect(isAgentSignInActive()).toBe(false);
        app._clearAgentSignInSlot();
        expect(slot.children.length).toBe(0);
    });
});

describe('chat: after the run', () => {
    it('a clean exit resends the failed turn once, without a second bubble', async () => {
        const { app, appWindow, starts, named } = setup();
        const attachments = [{ name: 'a.png', data: 'x' }];
        app._recordTurn('s1', 'hello agent', attachments);
        const addUserMessage = vi.spyOn(app, 'addUserMessage');
        await failTurn(app, SIGNED_OUT);
        await clickSignIn();
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush();
        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: 's1' }]]);
        expect(named('send_message_streaming')).toEqual([
            ['send_message_streaming', { sessionId: 's1', message: 'hello agent', attachments }],
        ]);
        expect(addUserMessage).not.toHaveBeenCalled();
        // The error went; the window came back to show it.
        expect(document.getElementById('errorContainer').innerHTML).toBe('');
        expect(appWindow.show).toHaveBeenCalled();
    });

    it('a failed, stopped or timed-out run sends nothing', async () => {
        for (const report of [
            { outcome: 'exited', exitCode: 1 },
            { outcome: 'cancelled', exitCode: null, signal: 'Terminated' },
            { outcome: 'timed_out', exitCode: null, signal: 'Terminated' },
        ]) {
            _resetAgentSignInForTests();
            const { app, starts, named } = setup();
            app._recordTurn('s1', 'hello agent', null);
            await failTurn(app, SIGNED_OUT);
            await clickSignIn();
            starts[0].resolve(report);
            await flush();
            expect(named('send_message_streaming').length).toBe(0);
            expect(named('reconnect_acp').length).toBe(0);
        }
    });

    it('the time limit posts a notification for this window instead of re-showing it', async () => {
        const { app, appWindow, starts, named } = setup();
        app.showError('Signed out', { signIn: SIGNED_OUT });
        await flush();
        await clickSignIn();
        starts[0].resolve({ outcome: 'timed_out', exitCode: null });
        await flush();
        expect(appWindow.show).not.toHaveBeenCalled();
        const [[, args]] = named('notify_response_ready');
        expect(args.targetLabel).toBe('main');
    });

    it('with no failed turn on record it restarts and sends nothing', async () => {
        const { app, starts, named } = setup();
        await failTurn(app, SIGNED_OUT);
        await signInCleanly(starts);
        expect(named('reconnect_acp').length).toBe(1);
        expect(sends(named)).toEqual([]);
        expect(document.querySelector('.agent-sign-in-status').textContent).toBe(
            'shared.agent_sign_in.status.ready'
        );
    });
});

describe('chat: what a sign-in resends', () => {
    const UNREACHABLE = {
        kind: 'agent_unreachable',
        message: 'Kage restarted Kiro and tried again, but it still failed.',
        sessionId: 's1',
    };

    it('not a turn that already streamed output', async () => {
        const { app, starts, named } = setup();
        app._recordTurn('s1', 'write the file', null);
        app.currentStreamingContent = 'Writing the file now…';
        await failTurn(app, UNREACHABLE);
        await signInCleanly(starts);
        expect(named('reconnect_acp').length).toBe(1);
        expect(sends(named)).toEqual([]);
    });

    it('not a turn that ran a tool', async () => {
        const { app, starts, named } = setup();
        app._recordTurn('s1', 'run it', null);
        app.toolUsages = [{ id: 't1', title: 'shell' }];
        await failTurn(app, UNREACHABLE);
        await signInCleanly(starts);
        expect(sends(named)).toEqual([]);
    });

    it('not a rate-limited turn', async () => {
        const { app, starts, named } = setup();
        app._recordTurn('s1', 'hello agent', null);
        await failTurn(app, { ...UNREACHABLE, message: 'Rate limit exceeded' });
        await signInCleanly(starts);
        expect(sends(named)).toEqual([]);
    });

    it('not once a newer prompt went out', async () => {
        const { app, starts, named } = setup();
        app._recordTurn('s1', 'hello agent', null);
        await failTurn(app, SIGNED_OUT);
        app._recordTurn('s1', 'a newer prompt', null);
        await signInCleanly(starts);
        expect(sends(named)).toEqual([]);
    });

    it('the turn that failed, not an earlier failed one', async () => {
        const { app, starts, named } = setup();
        app._recordTurn('s1', 'prompt A', null);
        await failTurn(app, 'Error: dispatch failure');
        // "Run it here" hands a command's result back to the agent.
        await app.sendAgentPrompt('s1', 'shell result B');
        await failTurn(app, SIGNED_OUT);
        await signInCleanly(starts);
        expect(sends(named)).toEqual(['shell result B', 'shell result B']);
    });

    it('not into a session the user switched to', async () => {
        const { app, starts, named } = setup();
        app._recordTurn('s1', 'hello agent', null);
        await failTurn(app, SIGNED_OUT);
        await clickSignIn();
        app.activeSessionId = 's9';
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush();
        expect(sends(named)).toEqual([]);
        expect(app.activeSessionId).toBe('s9');
    });
});
