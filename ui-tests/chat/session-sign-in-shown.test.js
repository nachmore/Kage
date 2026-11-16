/**
 * The chat window and the session a prompt went to, through the production
 * listeners:
 *   - inline assist's open in chat prompts on main's backend pin, A, and
 *     `initial_message` names it. Main may show B meanwhile (a selection
 *     still under way, or one that was refused): it records the turn on A
 *     and shows A, so the prompt and its reply render there, and the
 *     superseded selection's late answer changes nothing. A refused
 *     selection never pins the window to B; a signed-out refusal of the
 *     prompt names A and its Sign in sends the turn there once.
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
function setup({ switches, loads = {}, pins: initialPins = {} }) {
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
    // The backend's window → session pins (set_window_session, a switch
    // that worked), which get_window_session reads.
    const pins = { ...initialPins };
    let switchAt = 0;
    const invoke = vi.fn(async (cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'agent_sign_in_context') return CTX;
        if (cmd === 'get_window_session') return pins[args?.label] ?? null;
        if (cmd === 'set_window_session') {
            pins[args.label] = args.sessionId;
            return null;
        }
        if (cmd === 'start_agent_sign_in') {
            return new Promise((resolve, reject) => starts.push({ args, resolve, reject }));
        }
        if (cmd === 'reconnect_acp') {
            return { reconnected: true, sessionId: args?.sessionId ?? null, sessionReloaded: false };
        }
        if (cmd === 'switch_acp_session') {
            const answer = switches[Math.min(switchAt++, switches.length - 1)];
            const id = typeof answer === 'function' ? await answer(args) : answer;
            if (typeof id === 'string' && id.startsWith('s-')) {
                pins.main = id;
                return id;
            }
            throw id;
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
    return { app, starts, named, listeners, pins };
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

describe('chat: open in chat prompts on main\'s pin, and main shows that session', () => {
    const LOCKED = 'Session is active in another process (PID 4242)';
    const sendsOf = (named) => named('send_message_streaming').map(([, a]) => a);
    const userBubbles = () => document.querySelectorAll('#messagesArea .message.user');

    /**
     * Main shows s-a, its backend pin. `switches`: what selecting s-b
     * answers (see `setup`).
     */
    function shownOnA({ switches }) {
        const ctx = setup({ switches, pins: { main: 's-a' } });
        const { app } = ctx;
        app.windowLabel = 'main';
        app.activeSessionId = 's-a';
        app.currentAcpSessionId = 's-a';
        app.sessions = [
            { session_id: 's-a', title: 'Chat A' },
            { session_id: 's-b', title: 'Chat B' },
        ];
        app.handleMessageChunk = vi.fn();
        app.handleMessageComplete = vi.fn(() => {
            app.isWaitingForResponse = false;
        });
        app.setupStreamingListeners();
        app._listenForInitialMessageSession();
        return ctx;
    }

    /**
     * Inline assist's open in chat, as the backend runs it: it reads main's
     * pin (get_window_session), announces the prompt on it
     * (initial_message names it) and prompts there. Resolves to that pin.
     */
    async function openInChat(app, listeners, message = 'explain this', turnId = 'assist-turn-1') {
        const sessionId = await app.invoke('get_window_session', { label: 'main' });
        listeners.initial_message({ payload: { message, turnId, sessionId } });
        return sessionId;
    }

    /** The reply streams on `sid` and completes, with the focus refresh in between. */
    async function replyOn(app, listeners, sid) {
        listeners.message_chunk({ payload: { sessionId: sid, text: 'Sure.' } });
        // open_chat_with_message focused main: its refresh reads the pin.
        await app.loadCurrentSessionId();
        listeners.message_complete({ payload: { sessionId: sid, oldSessionId: sid } });
        await flush();
    }

    it('a selection still under way: the prompt and its reply show on the pinned session, and a refusal of the selection after it changes nothing', async () => {
        let refuse;
        const { app, listeners, named, pins } = shownOnA({
            switches: [() => new Promise((_, reject) => (refuse = reject))],
        });
        const selecting = app.selectSession('s-b');
        await flush();
        expect(app.activeSessionId).toBe('s-b');

        const on = await openInChat(app, listeners);
        expect(on).toBe('s-a');
        // Main shows the session the prompt went to, with its turn there.
        expect(app.activeSessionId).toBe('s-a');
        expect(app.currentAcpSessionId).toBe('s-a');
        expect(app._lastTurn).toMatchObject({ sessionId: 's-a', turnId: 'assist-turn-1' });
        expect(app.elements.chatHeaderTitle.textContent).toBe('Chat A');
        expect(app.isWaitingForResponse).toBe(true);
        expect(userBubbles().length).toBe(1);

        refuse(SIGNED_OUT);
        await selecting;
        await flush();
        // The superseded selection's refusal shows nothing and pins nothing.
        expect(document.querySelector('#errorContainer').textContent).toBe('');
        expect(app.activeSessionId).toBe('s-a');
        expect(pins.main).toBe('s-a');

        await replyOn(app, listeners, on);
        expect(app.handleMessageChunk).toHaveBeenCalledTimes(1);
        expect(app.handleMessageComplete).toHaveBeenCalledTimes(1);
        expect(app.isWaitingForResponse).toBe(false);
        expect(app.activeSessionId).toBe('s-a');
        expect(app.elements.sendBtn.disabled).toBe(false);
        expect(named('set_window_session').map(([, a]) => a.sessionId)).not.toContain('s-b');
        expect(sendsOf(named)).toEqual([]);
    });

    it('a selection that lands after the prompt went out: main stays on the pinned session and pins it back', async () => {
        let land;
        const { app, listeners, pins } = shownOnA({
            switches: [() => new Promise((resolve) => (land = resolve))],
        });
        const selecting = app.selectSession('s-b');
        await flush();
        const on = await openInChat(app, listeners);
        land('s-b');
        await selecting;
        await flush();
        expect(app.activeSessionId).toBe('s-a');
        expect(app.currentAcpSessionId).toBe('s-a');
        expect(pins.main).toBe('s-a');
        await replyOn(app, listeners, on);
        expect(app.handleMessageChunk).toHaveBeenCalledTimes(1);
        expect(app.isWaitingForResponse).toBe(false);
    });

    it("the transcript on screen is kept while it is still the pinned session's", async () => {
        let loadB;
        const { app, listeners, named } = shownOnA({ switches: ['s-b'] });
        const orig = app.invoke.getMockImplementation();
        app.invoke.mockImplementation(async (cmd, args) =>
            cmd === 'load_session' && args.sessionId === 's-b'
                ? new Promise((resolve) => (loadB = () => resolve({ messages: [] })))
                : orig(cmd, args)
        );
        await app.selectSession('s-a', { force: true });
        app.elements.messagesArea.innerHTML = '<div class="message">A transcript</div>';
        // Selecting s-b: its transcript is still loading, s-a's is on screen.
        const selecting = app.selectSession('s-b');
        await flush();
        await openInChat(app, listeners);
        expect(app.elements.messagesArea.textContent).toContain('A transcript');
        expect(userBubbles().length).toBe(1);

        app.displaySession.mockClear();
        loadB();
        await selecting;
        await flush();
        // The superseded selection paints nothing and switches nothing.
        expect(app.displaySession).not.toHaveBeenCalled();
        expect(named('switch_acp_session')).toEqual([]);
        expect(app.elements.messagesArea.textContent).toContain('A transcript');
        expect(app.activeSessionId).toBe('s-a');
    });

    it('another session\'s transcript on screen gives way to the turn', async () => {
        const { app, listeners } = shownOnA({ switches: [LOCKED] });
        await app.selectSession('s-b');
        await flush();
        app.elements.messagesArea.innerHTML = '<div class="message">B transcript</div>';
        await openInChat(app, listeners);
        expect(app.elements.messagesArea.textContent).not.toContain('B transcript');
        expect(userBubbles().length).toBe(1);
    });

    it('a refused selection never pins the window to that session: open in chat prompts on the pin, and its reply shows', async () => {
        const { app, listeners, named, pins } = shownOnA({ switches: [LOCKED] });
        await app.selectSession('s-b');
        await flush();
        expect(app.activeSessionId).toBe('s-b');
        expect(app.elements.chatInput.disabled).toBe(true);
        expect(pins.main).toBe('s-a');
        expect(named('set_window_session')).toEqual([]);

        const on = await openInChat(app, listeners);
        expect(on).toBe('s-a');
        expect(app.activeSessionId).toBe('s-a');
        await replyOn(app, listeners, on);
        expect(app.handleMessageChunk).toHaveBeenCalledTimes(1);
        expect(app.handleMessageComplete).toHaveBeenCalledTimes(1);
        expect(app.isWaitingForResponse).toBe(false);
        expect(named('set_window_session').map(([, a]) => a.sessionId)).not.toContain('s-b');
    });

    it('an earlier selection landing after a refused one pins the session it loaded, never the refused one', async () => {
        let landC;
        const { app, pins } = shownOnA({
            switches: [() => new Promise((resolve) => (landC = resolve)), LOCKED],
        });
        const selectingC = app.selectSession('s-c');
        await flush();
        await app.selectSession('s-b');
        await flush();
        expect(app.activeSessionId).toBe('s-b');
        landC('s-c');
        await selectingC;
        await flush();
        expect(app.activeSessionId).toBe('s-b');
        expect(pins.main).toBe('s-c');
    });

    it("a signed-out refusal of the selection, then of open in chat's prompt on the pin: Sign in sends it there once", async () => {
        const { app, starts, listeners, named, pins } = shownOnA({ switches: [SIGNED_OUT] });
        // The restart reloads the session it is asked to.
        const orig = app.invoke.getMockImplementation();
        app.invoke.mockImplementation(async (cmd, args) => {
            const answer = await orig(cmd, args);
            return cmd === 'reconnect_acp' ? { ...answer, sessionReloaded: true } : answer;
        });
        await app.selectSession('s-b');
        await flush();
        expect(pins.main).toBe('s-a');
        expect(signInBtn()).toBeTruthy();

        const on = await openInChat(app, listeners);
        // The backend's connect was refused: nothing of the prompt went out.
        await listeners.message_error({
            payload: {
                ...SIGNED_OUT,
                detail: null,
                sessionId: on,
                resendable: true,
                turnId: 'assist-turn-1',
            },
        });
        await flush(20);
        expect(app.activeSessionId).toBe('s-a');
        expect(app.isWaitingForResponse).toBe(false);
        expect(document.querySelector('#errorContainer').textContent).toContain(
            'Kiro needs you to sign in'
        );

        await signInCleanly(starts);
        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: 's-a' }]]);
        // Only the refused selection: nothing switches to s-b again.
        expect(named('switch_acp_session').map(([, a]) => a.sessionId)).toEqual(['s-b']);
        const sends = sendsOf(named);
        expect(sends.map((a) => [a.sessionId, a.message])).toEqual([['s-a', 'explain this']]);
        expect(sends[0].turnId).not.toBe('assist-turn-1');
        expect(userBubbles().length).toBe(1);
        expect(status()).toBe('shared.agent_sign_in.status.done');
        await flush(10);
        expect(sendsOf(named).length).toBe(1);
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
