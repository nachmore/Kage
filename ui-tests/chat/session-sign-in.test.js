/**
 * Sign in under a failed session setup in the chat window: a signed-out
 * agent refusing `switch_acp_session` (`{ kind: 'auth_required' }`) for a
 * selected session or a New Chat shows the error with Sign in; a clean exit
 * restarts the agent (`reconnect_acp`) and then loads or creates the
 * session the window wanted, sending nothing (no prompt was waiting); a
 * setup that still fails shows the error again with Sign in; a plain-string
 * error gets no Sign in, and a New Chat's other failures stay quiet.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { marked } from 'marked';
import { _resetAgentSignInForTests } from '../../ui/js/shared/agent-sign-in.js';
import { _setTerminalFactoryForTests } from '../../ui/js/shared/terminal-view.js';

let ChatApp;
beforeAll(async () => {
    globalThis.marked = marked;
    ({ ChatApp } = await import('../../ui/js/chat/app.js'));
});

const flush = async (n = 12) => {
    for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0));
};

const CTX = {
    available: true,
    agent: 'Kiro CLI',
    command: 'kiro-cli login',
    terminal: false,
    timeoutMinutes: 15,
};
const SIGNED_OUT = {
    kind: 'auth_required',
    key: 'errors.agent.auth_required_kiro',
    message: 'Kiro needs you to sign in. Run kiro-cli login in a terminal, then click Reconnect.',
};

function fakeTerminal(opts) {
    return {
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
}

/**
 * `switches`: what each `switch_acp_session` answers, in order (a string
 * starting `s-` resolves, anything else rejects); the last one repeats.
 * `reconnect`: reconnect_acp's answer.
 */
function setup({ switches, reconnect = null }) {
    vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ text: async () => '<svg xmlns="http://www.w3.org/2000/svg"/>' }))
    );
    document.body.innerHTML = `
        <div id="errorContainer"></div>
        <div id="agentSignInSlot" class="agent-sign-in-slot"></div>
        <div class="messages-area" id="messagesArea"></div>
        <div id="chatHeaderTitle"></div>
        <textarea id="chatInput"></textarea>
        <button id="sendBtn"></button>`;
    const calls = [];
    const starts = [];
    let switchAt = 0;
    const invoke = vi.fn(async (cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'agent_sign_in_context') return CTX;
        if (cmd === 'start_agent_sign_in') {
            return new Promise((resolve, reject) => starts.push({ args, resolve, reject }));
        }
        if (cmd === 'reconnect_acp') {
            return (
                reconnect ?? {
                    reconnected: true,
                    sessionId: args?.sessionId ?? null,
                    sessionReloaded: false,
                }
            );
        }
        if (cmd === 'switch_acp_session') {
            const answer = switches[Math.min(switchAt++, switches.length - 1)];
            if (typeof answer === 'string' && answer.startsWith('s-')) return answer;
            throw answer;
        }
        if (cmd === 'load_session') return { messages: [] };
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
        listen: vi.fn(async () => () => {}),
    };
    const listeners = {};
    const listen = vi.fn(async (evt, cb) => {
        listeners[evt] = cb;
        return () => {};
    });
    const app = new ChatApp(invoke, appWindow, listen);
    app.elements = {
        errorContainer: document.getElementById('errorContainer'),
        messagesArea: document.getElementById('messagesArea'),
        chatInput: document.getElementById('chatInput'),
        sendBtn: document.getElementById('sendBtn'),
        chatHeaderTitle: document.getElementById('chatHeaderTitle'),
    };
    for (const m of [
        'showTypingIndicator',
        'hideTypingIndicator',
        'updateInputState',
        'scrollToBottom',
        'updateConnectionStatus',
        'renderSessionList',
    ]) {
        app[m] = vi.fn();
    }
    app.displaySession = vi.fn(async () => {});
    app.sessions = [];
    app.activeSessionId = 's-1';
    app.currentAcpSessionId = 's-1';
    app.isWaitingForResponse = false;
    const named = (name) => calls.filter(([c]) => c === name);
    return { app, starts, named, listeners };
}

const signInBtn = () => document.querySelector('#errorContainer .agent-sign-in-btn');
const status = () => document.querySelector('.agent-sign-in-status')?.textContent;

async function signInCleanly(starts) {
    signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
    await flush();
    starts[starts.length - 1].resolve({ outcome: 'exited', exitCode: 0 });
    await flush(20);
}

beforeEach(() => {
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

describe('chat: selecting a session a signed-out agent refuses', () => {
    it('shows the error with Sign in and keeps the input off', async () => {
        const { app } = setup({ switches: [SIGNED_OUT] });
        await app.selectSession('s-2');
        await flush();
        expect(document.querySelector('#errorContainer').textContent).toContain(
            'Kiro needs you to sign in'
        );
        expect(signInBtn()).toBeTruthy();
        expect(app.elements.chatInput.disabled).toBe(true);
    });

    it('after a clean exit: restarts, loads that session again, sends nothing', async () => {
        const { app, starts, named } = setup({ switches: [SIGNED_OUT, 's-2'] });
        await app.selectSession('s-2');
        await flush();
        await signInCleanly(starts);
        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: 's-2' }]]);
        expect(named('switch_acp_session')).toEqual([
            ['switch_acp_session', { sessionId: 's-2' }],
            ['switch_acp_session', { sessionId: 's-2' }],
        ]);
        expect(app.currentAcpSessionId).toBe('s-2');
        expect(app.activeSessionId).toBe('s-2');
        expect(app.elements.chatInput.disabled).toBe(false);
        expect(named('send_message_streaming')).toEqual([]);
        expect(document.querySelector('#errorContainer').textContent).toBe('');
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    it('takes the session the restart already reloaded without loading it twice', async () => {
        const { app, starts, named } = setup({
            switches: [SIGNED_OUT],
            reconnect: { reconnected: true, sessionId: 's-2', sessionReloaded: true },
        });
        await app.selectSession('s-2');
        await flush();
        await signInCleanly(starts);
        expect(named('switch_acp_session').length).toBe(1);
        expect(app.currentAcpSessionId).toBe('s-2');
        expect(app.elements.chatInput.disabled).toBe(false);
    });

    it('still refused after the sign-in: the error again, with Sign in', async () => {
        const { app, starts, named } = setup({ switches: [SIGNED_OUT] });
        await app.selectSession('s-2');
        await flush();
        await signInCleanly(starts);
        expect(named('switch_acp_session').length).toBe(2);
        expect(signInBtn()).toBeTruthy();
        expect(app.elements.chatInput.disabled).toBe(true);
        expect(status()).toBe('shared.agent_sign_in.status.restart_failed');
    });

    it('not after the user moved to another session', async () => {
        const { app, starts, named } = setup({ switches: [SIGNED_OUT] });
        await app.selectSession('s-2');
        await flush();
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        app.activeSessionId = 's-9';
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);
        expect(named('switch_acp_session').length).toBe(1);
        expect(app.activeSessionId).toBe('s-9');
    });

    it('a plain-string failure gets no Sign in', async () => {
        const { app, named } = setup({ switches: ['Session load failed: disk'] });
        await app.selectSession('s-2');
        await flush();
        expect(document.querySelector('#errorContainer').textContent).toContain(
            'Session load failed'
        );
        expect(signInBtn()).toBeNull();
        expect(named('agent_sign_in_context').length).toBe(0);
    });
});

describe('chat: a New Chat a signed-out agent refuses', () => {
    it('shows the error with Sign in, and after a clean exit makes the new chat', async () => {
        const { app, starts, named } = setup({ switches: [SIGNED_OUT, 's-new'] });
        expect(await app.createNewSession()).toBeNull();
        await flush();
        expect(signInBtn()).toBeTruthy();
        // Still on the previous session meanwhile.
        expect(app.activeSessionId).toBe('s-1');

        await signInCleanly(starts);
        // The restart reloads the session the window shows, then the New Chat.
        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: 's-1' }]]);
        expect(named('switch_acp_session')).toEqual([
            ['switch_acp_session', { sessionId: null }],
            ['switch_acp_session', { sessionId: null }],
        ]);
        expect(app.activeSessionId).toBe('s-new');
        expect(app.currentAcpSessionId).toBe('s-new');
        expect(named('send_message_streaming')).toEqual([]);
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    it('any other New Chat failure stays quiet, as before', async () => {
        const { app } = setup({ switches: [{ kind: 'internal', message: 'disk full' }] });
        await app.createNewSession();
        await flush();
        expect(document.querySelector('#errorContainer').textContent).toBe('');
        expect(app.activeSessionId).toBe('s-1');
    });
});

describe("chat: the window's first session a signed-out agent refuses", () => {
    /** A `chat-<uuid>` window whose launch bootstrap is refused. */
    async function peerWithFailedBootstrap(switches) {
        const ctx = setup({ switches });
        const { app } = ctx;
        app.windowLabel = 'chat-abc';
        app.activeSessionId = null;
        app.currentAcpSessionId = null;
        app.sessions = [{ session_id: 's-old', title: 'Older chat' }];
        app.elements.chatHeaderTitle.textContent = 'chat.header.default_title';
        await app._bootstrapChatPeerSession();
        await flush();
        return ctx;
    }

    it('shows the error with Sign in', async () => {
        await peerWithFailedBootstrap([SIGNED_OUT]);
        expect(signInBtn()).toBeTruthy();
    });

    it('after a clean exit draws the new session as init would', async () => {
        const { app, starts, named } = await peerWithFailedBootstrap([SIGNED_OUT, 's-new']);
        app.renderSessionList.mockClear();
        await signInCleanly(starts);

        expect(app.currentAcpSessionId).toBe('s-new');
        expect(app.activeSessionId).toBe('s-new');
        // A sidebar entry for it, highlighted, and the list redrawn.
        expect(app.sessions.map((s) => s.session_id)).toEqual(['s-new', 's-old']);
        expect(app.renderSessionList).toHaveBeenCalled();
        // Its transcript loaded and shown, and the header names it.
        expect(named('load_session')).toEqual([['load_session', { sessionId: 's-new' }]]);
        expect(app.displaySession).toHaveBeenCalledWith({ messages: [] });
        expect(app.elements.chatHeaderTitle.textContent).toBe('chat.session.current_title');
        expect(named('send_message_streaming')).toEqual([]);
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    it('still refused after the sign-in: the error again, nothing drawn', async () => {
        const { app, starts, named } = await peerWithFailedBootstrap([SIGNED_OUT]);
        await signInCleanly(starts);
        expect(signInBtn()).toBeTruthy();
        expect(app.currentAcpSessionId).toBeNull();
        expect(named('load_session')).toEqual([]);
        expect(status()).toBe('shared.agent_sign_in.status.restart_failed');
    });
});

describe('chat: an open-in-chat prompt a signed-out agent refused to set up for', () => {
    /**
     * open_chat_with_message's MESSAGE_ERROR when the agent refused to set
     * up for the prompt: nothing of it went out (`resendable`), naming its
     * turn and main's session (null: main had none, and none was made).
     */
    const unsent = (sessionId) => ({
        ...SIGNED_OUT,
        detail: null,
        sessionId,
        resendable: true,
        turnId: 'assist-turn-1',
    });

    /**
     * Main (on `shown`) gets inline assist's prompt, which fails unsent;
     * the error names `named` (main's session as the backend had it pinned).
     * `failedSelect`: main first selected that session and could not set
     * it up (so the backend never pinned it).
     */
    async function openedAndRefused({
        switches = ['s-new'],
        shown = null,
        named,
        failedSelect = null,
        reconnect = null,
    } = {}) {
        const ctx = setup({ switches, reconnect });
        const { app, listeners } = ctx;
        app.activeSessionId = shown;
        app.currentAcpSessionId = shown;
        if (failedSelect) {
            await app.selectSession(failedSelect);
            await flush();
        }
        app.setupStreamingListeners();
        app._listenForInitialMessageSession();
        listeners.initial_message({
            payload: { message: 'explain this', turnId: 'assist-turn-1' },
        });
        const payload = unsent(named === undefined ? shown : named);
        await app.messageStreamController.host.onError({ payload }, true);
        await flush();
        return ctx;
    }

    const sendsOf = (named) => named('send_message_streaming').map(([, a]) => a);
    const userBubbles = () => document.querySelectorAll('#messagesArea .message.user');

    it('with no session: after the sign-in main sets one up and sends the prompt once', async () => {
        const { app, starts, named } = await openedAndRefused();
        expect(signInBtn()).toBeTruthy();
        await signInCleanly(starts);

        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: null }]]);
        expect(named('switch_acp_session')).toEqual([['switch_acp_session', { sessionId: null }]]);
        expect(app.activeSessionId).toBe('s-new');
        expect(app.currentAcpSessionId).toBe('s-new');
        expect(app.sessions.map((s) => s.session_id)).toContain('s-new');
        expect(app.elements.chatHeaderTitle.textContent).toBe('chat.session.current_title');
        const sends = sendsOf(named);
        expect(sends).toEqual([
            {
                sessionId: 's-new',
                message: 'explain this',
                attachments: null,
                turnId: expect.any(String),
            },
        ]);
        // A turn of its own; the user's bubble is not drawn again.
        expect(sends[0].turnId).not.toBe('assist-turn-1');
        expect(userBubbles().length).toBe(1);
        expect(status()).toBe('shared.agent_sign_in.status.done');
        await flush(10);
        expect(sendsOf(named).length).toBe(1);
    });

    it('with no session: Reconnect does the same, once', async () => {
        const { app, named } = await openedAndRefused();
        document.getElementById('errorReconnectBtn').click();
        await flush(20);
        expect(named('switch_acp_session')).toEqual([['switch_acp_session', { sessionId: null }]]);
        expect(app.activeSessionId).toBe('s-new');
        expect(sendsOf(named).map((a) => [a.sessionId, a.message])).toEqual([
            ['s-new', 'explain this'],
        ]);
    });

    it('with no session: nothing is sent when the user sent something since', async () => {
        const { app, starts, named } = await openedAndRefused();
        app._recordTurn(null, 'newer', null);
        await signInCleanly(starts);
        expect(app.activeSessionId).toBe('s-new');
        expect(sendsOf(named)).toEqual([]);
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    it('with no session: a setup still refused shows the error again and sends nothing, then works once', async () => {
        const { app, starts, named } = await openedAndRefused({ switches: [SIGNED_OUT] });
        await signInCleanly(starts);
        expect(sendsOf(named)).toEqual([]);
        expect(app.activeSessionId).toBeNull();
        expect(signInBtn()).toBeTruthy();
        expect(status()).toBe('shared.agent_sign_in.status.restart_failed');

        const orig = app.invoke.getMockImplementation();
        app.invoke.mockImplementation(async (cmd, args) =>
            cmd === 'switch_acp_session' ? 's-later' : orig(cmd, args)
        );
        await signInCleanly(starts);
        expect(sendsOf(named).map((a) => [a.sessionId, a.message])).toEqual([
            ['s-later', 'explain this'],
        ]);
    });

    it("on main's session: after the sign-in it sends the prompt there, once", async () => {
        const { app, starts, named } = await openedAndRefused({ shown: 's-1' });
        await signInCleanly(starts);
        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: 's-1' }]]);
        expect(named('switch_acp_session')).toEqual([]);
        expect(app.activeSessionId).toBe('s-1');
        expect(sendsOf(named).map((a) => [a.sessionId, a.message])).toEqual([
            ['s-1', 'explain this'],
        ]);
    });

    // Main shows a session it failed to set up (the switch was refused, so
    // the backend never pinned it): the error names no session, but the
    // prompt's bubble sits in that session.
    const onUnsetSession = (extra = {}) =>
        openedAndRefused({
            switches: [SIGNED_OUT, 's-2'],
            shown: 's-1',
            failedSelect: 's-2',
            named: null,
            ...extra,
        });

    it('on a session main failed to set up: after the sign-in it sets that session up and sends the prompt there, once', async () => {
        const { app, starts, named } = await onUnsetSession();
        expect(app.activeSessionId).toBe('s-2');
        expect(signInBtn()).toBeTruthy();
        await signInCleanly(starts);

        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: 's-2' }]]);
        expect(named('switch_acp_session')).toEqual([
            ['switch_acp_session', { sessionId: 's-2' }],
            ['switch_acp_session', { sessionId: 's-2' }],
        ]);
        expect(app.activeSessionId).toBe('s-2');
        expect(app.currentAcpSessionId).toBe('s-2');
        expect(app.elements.chatInput.disabled).toBe(false);
        const sends = sendsOf(named);
        expect(sends.map((a) => [a.sessionId, a.message])).toEqual([['s-2', 'explain this']]);
        expect(sends[0].turnId).not.toBe('assist-turn-1');
        expect(userBubbles().length).toBe(1);
        expect(status()).toBe('shared.agent_sign_in.status.done');
        await flush(10);
        expect(sendsOf(named).length).toBe(1);
    });

    it('on a session main failed to set up: Reconnect does the same, once', async () => {
        const { app, named } = await onUnsetSession();
        document.getElementById('errorReconnectBtn').click();
        await flush(20);
        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: 's-2' }]]);
        expect(app.currentAcpSessionId).toBe('s-2');
        expect(sendsOf(named).map((a) => [a.sessionId, a.message])).toEqual([
            ['s-2', 'explain this'],
        ]);
        document.getElementById('errorReconnectBtn')?.click();
        await flush(20);
        expect(sendsOf(named).length).toBe(1);
    });

    it('on a session main failed to set up: still refused shows the error again with Sign in, then works once', async () => {
        const { app, starts, named } = await onUnsetSession({ switches: [SIGNED_OUT] });
        await signInCleanly(starts);
        expect(sendsOf(named)).toEqual([]);
        expect(app.activeSessionId).toBe('s-2');
        expect(app.elements.chatInput.disabled).toBe(true);
        expect(signInBtn()).toBeTruthy();
        expect(status()).toBe('shared.agent_sign_in.status.restart_failed');

        const orig = app.invoke.getMockImplementation();
        app.invoke.mockImplementation(async (cmd, args) =>
            cmd === 'switch_acp_session' ? 's-2' : orig(cmd, args)
        );
        await signInCleanly(starts);
        expect(sendsOf(named).map((a) => [a.sessionId, a.message])).toEqual([
            ['s-2', 'explain this'],
        ]);
    });

    it('on a session main failed to set up: nothing is sent after the user moved away', async () => {
        const { app, starts, named } = await onUnsetSession();
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        app.activeSessionId = 's-9';
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);
        expect(named('switch_acp_session').length).toBe(1);
        expect(app.activeSessionId).toBe('s-9');
        expect(sendsOf(named)).toEqual([]);
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    // The mirror: the backend has main pinned to a session, main shows
    // none (a New Chat it could not set up). The error names the pinned
    // session; main still sets up a session of its own and sends there.
    it('main showing no session while the backend has one pinned: after the sign-in it makes one and sends there, once', async () => {
        const { app, starts, named } = await openedAndRefused({
            shown: null,
            named: 's-1',
            reconnect: { reconnected: true, sessionId: 's-1', sessionReloaded: true },
        });
        expect(signInBtn()).toBeTruthy();
        await signInCleanly(starts);
        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: null }]]);
        expect(named('switch_acp_session')).toEqual([['switch_acp_session', { sessionId: null }]]);
        expect(app.activeSessionId).toBe('s-new');
        expect(sendsOf(named).map((a) => [a.sessionId, a.message])).toEqual([
            ['s-new', 'explain this'],
        ]);
        expect(status()).toBe('shared.agent_sign_in.status.done');
    });
});
