/**
 * The chat window acts on the session it shows, through the production
 * listeners:
 *   - main is pinned (by the backend) to A, and selecting B fails before
 *     that pin is replaced. Inline assist's prompt then lands on B (its
 *     bubble, its turn) while the backend's setup refusal names A. That
 *     error is this window's own turn's (its `turnId`): it is never
 *     dropped for naming another session, it shows with Sign in, and the
 *     sign-in sends the turn once on B, the session shown, with the pin
 *     moved to B. Never on A, and not after the user moved on.
 *   - a session the sign-in sets up is painted only while the window still
 *     shows it: a late load_session reply for a session the user left
 *     paints nothing (transcript, header, sidebar highlight).
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { marked } from 'marked';
import { _resetAgentSignInForTests } from '../../ui/js/shared/agent-sign-in.js';
import { _setTerminalFactoryForTests } from '../../ui/js/shared/terminal-view.js';

vi.mock('../../ui/js/shared/network.js', async (importOriginal) => ({
    ...(await importOriginal()),
    checkOnError: async () => true,
    checkOnline: async () => true,
}));

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
 * starting `s-` resolves, a function is called, anything else rejects);
 * the last one repeats. `loads`: load_session's answer per session id (a
 * function is called), else an empty transcript.
 */
function setup({ switches, loads = {} }) {
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
            return { reconnected: true, sessionId: args?.sessionId ?? null, sessionReloaded: false };
        }
        if (cmd === 'switch_acp_session') {
            const answer = switches[Math.min(switchAt++, switches.length - 1)];
            if (typeof answer === 'function') return answer(args);
            if (typeof answer === 'string' && answer.startsWith('s-')) return answer;
            throw answer;
        }
        if (cmd === 'load_session') {
            const answer = loads[args?.sessionId];
            if (typeof answer === 'function') return answer();
            return answer ?? { messages: [] };
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

describe("chat: open-in-chat's refusal names the pin, main shows another session", () => {
    /** open_chat_with_message's MESSAGE_ERROR: it never sent the prompt. */
    const unsent = (sessionId) => ({
        ...SIGNED_OUT,
        detail: null,
        sessionId,
        resendable: true,
        turnId: 'assist-turn-1',
    });

    /**
     * Main is pinned to s-a; selecting s-b is refused (the pin stays s-a);
     * inline assist's prompt lands on s-b and its refusal names s-a, as
     * the production listeners see it. `refreshed`: the focus refresh read
     * the backend pin (s-a) back into `currentAcpSessionId` first.
     */
    async function pinnedElsewhereAndRefused({ refreshed = false, switches } = {}) {
        const ctx = setup({ switches: switches ?? [SIGNED_OUT, 's-b'] });
        const { app, listeners } = ctx;
        app.activeSessionId = 's-a';
        app.currentAcpSessionId = 's-a';
        app.setupStreamingListeners();
        app._listenForInitialMessageSession();
        await app.selectSession('s-b');
        await flush();
        expect(app.activeSessionId).toBe('s-b');
        listeners.initial_message({
            payload: { message: 'explain this', turnId: 'assist-turn-1' },
        });
        expect(app.isWaitingForResponse).toBe(true);
        if (refreshed) app.currentAcpSessionId = 's-a';
        await listeners.message_error({ payload: unsent('s-a') });
        await flush(20);
        return ctx;
    }

    const sendsOf = (named) => named('send_message_streaming').map(([, a]) => a);

    for (const refreshed of [false, true]) {
        const when = refreshed ? 'after the focus refresh' : 'before the focus refresh';
        it(`${when}: the error shows with Sign in, which sends the turn once on the shown session`, async () => {
            const { app, starts, named } = await pinnedElsewhereAndRefused({ refreshed });
            expect(app.isWaitingForResponse).toBe(false);
            expect(document.querySelector('#errorContainer').textContent).toContain(
                'Kiro needs you to sign in'
            );
            expect(signInBtn()).toBeTruthy();

            await signInCleanly(starts);
            expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: 's-b' }]]);
            // The refused selection, then the one after the sign-in: the
            // pin moves to s-b before anything is sent.
            expect(named('switch_acp_session')).toEqual([
                ['switch_acp_session', { sessionId: 's-b' }],
                ['switch_acp_session', { sessionId: 's-b' }],
            ]);
            expect(app.activeSessionId).toBe('s-b');
            expect(app.currentAcpSessionId).toBe('s-b');
            const sends = sendsOf(named);
            expect(sends.map((a) => [a.sessionId, a.message])).toEqual([['s-b', 'explain this']]);
            expect(sends[0].turnId).not.toBe('assist-turn-1');
            expect(document.querySelectorAll('#messagesArea .message.user').length).toBe(1);
            expect(status()).toBe('shared.agent_sign_in.status.done');
            await flush(10);
            expect(sendsOf(named).length).toBe(1);
        });
    }

    it('Reconnect does the same, once', async () => {
        const { app, named } = await pinnedElsewhereAndRefused();
        document.getElementById('errorReconnectBtn').click();
        await flush(20);
        expect(app.currentAcpSessionId).toBe('s-b');
        expect(sendsOf(named).map((a) => [a.sessionId, a.message])).toEqual([
            ['s-b', 'explain this'],
        ]);
    });

    it('nothing is sent after the user moved to another session', async () => {
        const { app, starts, named } = await pinnedElsewhereAndRefused();
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        app.activeSessionId = 's-9';
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);
        expect(app.activeSessionId).toBe('s-9');
        expect(sendsOf(named)).toEqual([]);
    });

    it('still refused after the sign-in: the error again with Sign in, nothing sent, then once', async () => {
        const { app, starts, named } = await pinnedElsewhereAndRefused({
            switches: [SIGNED_OUT],
        });
        await signInCleanly(starts);
        expect(sendsOf(named)).toEqual([]);
        expect(app.activeSessionId).toBe('s-b');
        expect(signInBtn()).toBeTruthy();
        const orig = app.invoke.getMockImplementation();
        app.invoke.mockImplementation(async (cmd, args) =>
            cmd === 'switch_acp_session' ? 's-b' : orig(cmd, args)
        );
        await signInCleanly(starts);
        expect(sendsOf(named).map((a) => [a.sessionId, a.message])).toEqual([
            ['s-b', 'explain this'],
        ]);
    });
});

describe('chat: a late paint of a session the window no longer shows', () => {
    it("the sign-in's session loads late after the user picked another: nothing of it is painted", async () => {
        let releaseNew;
        const newData = { messages: [{ role: 'user', content: 'from s-new' }] };
        const bData = { messages: [{ role: 'user', content: 'from s-b' }] };
        const ctx = setup({
            switches: [SIGNED_OUT, 's-new', 's-b'],
            loads: {
                's-new': () => new Promise((resolve) => (releaseNew = () => resolve(newData))),
                's-b': bData,
            },
        });
        const { app, starts } = ctx;
        app.windowLabel = 'chat-abc';
        app.activeSessionId = null;
        app.currentAcpSessionId = null;
        app.sessions = [
            { session_id: 's-old', title: 'Older chat' },
            { session_id: 's-b', title: 'Chat B' },
        ];
        await app._bootstrapChatPeerSession();
        await flush();
        expect(signInBtn()).toBeTruthy();

        await signInCleanly(starts);
        // The sign-in set up s-new and is loading its transcript; the user
        // picks s-b meanwhile.
        expect(releaseNew).toBeTypeOf('function');
        await app.selectSession('s-b');
        await flush();
        expect(app.activeSessionId).toBe('s-b');
        app.elements.chatHeaderTitle.textContent = 'Chat B';
        app.displaySession.mockClear();
        app.renderSessionList.mockClear();

        releaseNew();
        await flush(20);
        expect(app.displaySession).not.toHaveBeenCalled();
        expect(app.elements.chatHeaderTitle.textContent).toBe('Chat B');
        expect(app.activeSessionId).toBe('s-b');
        expect(app.elements.messagesArea.textContent).not.toContain(
            'chat.placeholder.continue'
        );
    });

    it('a late load failure of a session left paints no placeholder either', async () => {
        let failNew;
        const ctx = setup({
            switches: [SIGNED_OUT, 's-new', 's-b'],
            loads: {
                's-new': () => new Promise((_, reject) => (failNew = () => reject('not on disk'))),
            },
        });
        const { app, starts } = ctx;
        app.windowLabel = 'chat-abc';
        app.activeSessionId = null;
        app.currentAcpSessionId = null;
        app.sessions = [{ session_id: 's-b', title: 'Chat B' }];
        await app._bootstrapChatPeerSession();
        await flush();
        await signInCleanly(starts);
        await app.selectSession('s-b');
        await flush();
        app.elements.messagesArea.innerHTML = '<div class="message">B transcript</div>';
        app.elements.chatHeaderTitle.textContent = 'Chat B';

        failNew();
        await flush(20);
        expect(app.elements.messagesArea.textContent).toBe('B transcript');
        expect(app.elements.chatHeaderTitle.textContent).toBe('Chat B');
    });
});
