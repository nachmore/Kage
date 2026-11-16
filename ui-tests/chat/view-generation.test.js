/**
 * The chat window's view generation: every event that changes what the
 * window shows, or starts a turn in it, supersedes the async work still in
 * flight for what it showed before. A superseded result (a selection's
 * transcript or switch, a New Chat, the current session's paint, a read of
 * the backend pin, a session list) changes nothing, and where the backend
 * pin may now disagree with what is shown it pins the window back to that,
 * never to a refused session.
 *
 * Through the production handlers and listeners:
 *   - V1: a New Chat in flight while main has no session, then open in chat
 *     (its prompt on a session the backend makes): the New Chat's late
 *     answer neither replaces the prompt's session nor keeps the pin.
 *   - V2: a focus refresh while a switch is in flight, then the switch is
 *     refused: the window holds no older session as its own, so a turn
 *     finishing or migrating there stays a background one.
 *   - V3: the shared floating session's history still loading when a
 *     prompt is sent from Floating: the history's late arrival doesn't
 *     erase the prompt or detach its reply.
 */
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { marked } from 'marked';

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

afterEach(() => {
    delete window.__TAURI__;
    document.body.innerHTML = '';
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

const flush = async (n = 12) => {
    for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0));
};

const LOCKED = 'Session is active in another process (PID 4242)';

/** A transcript of `n` user prompts, `label 0` … `label n-1`. */
const history = (label, n = 3) => ({
    messages: Array.from({ length: n }, (_, i) => ({
        kind: 'Prompt',
        message_id: `${label}-${i}`,
        content: [{ kind: 'text', data: `${label} ${i}` }],
    })),
});

/**
 * Main, with the backend's window → session pins in `pins`. Every invoke
 * answers at once unless a test holds it first (`hold(cmd, when)`): the
 * next matching call then waits for the returned handle's `resolve` /
 * `reject`. `switch_acp_session` pins main to the session it answers with,
 * as the backend does; `disk`: the sessions list_sessions answers with;
 * `transcripts`: load_session's answer per session id.
 */
function setup({ pins: initialPins = { main: 's-a' }, disk = [], transcripts = {} } = {}) {
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
    const pins = { ...initialPins };
    const holds = [];
    let made = 0;
    const hold = (cmd, when = () => true) => {
        const h = { cmd, when };
        h.promise = new Promise((resolve, reject) => {
            h.resolve = resolve;
            h.reject = reject;
        });
        holds.push(h);
        return h;
    };
    const answer = (cmd, args) => {
        if (cmd === 'get_window_session') return pins[args?.label] ?? null;
        if (cmd === 'set_window_session') {
            pins[args.label] = args.sessionId;
            return null;
        }
        if (cmd === 'switch_acp_session') return args?.sessionId ?? `s-new${++made}`;
        if (cmd === 'load_session') return transcripts[args?.sessionId] ?? { messages: [] };
        if (cmd === 'list_sessions') return disk.map((s) => ({ ...s }));
        if (cmd === 'get_session_stream_snapshot') return { inFlight: false };
        if (cmd === 'get_config') return {};
        return null;
    };
    const invoke = vi.fn(async (cmd, args) => {
        calls.push([cmd, args]);
        const at = holds.findIndex((h) => h.cmd === cmd && h.when(args));
        let result;
        if (at >= 0) {
            const [h] = holds.splice(at, 1);
            result = await h.promise;
        } else {
            result = answer(cmd, args);
        }
        if (cmd === 'switch_acp_session') pins.main = result;
        return result;
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
    app.windowLabel = 'main';
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
        'showSuggestionChips',
        'clearSuggestions',
        'hideSuggestionChips',
    ]) {
        app[m] = vi.fn();
    }
    app.handleMessageChunk = vi.fn();
    app.handleMessageComplete = vi.fn(() => {
        app.isWaitingForResponse = false;
    });
    app.sessions = disk.map((s) => ({ ...s }));
    app.setupStreamingListeners();
    app._listenForInitialMessageSession();
    const named = (name) => calls.filter(([c]) => c === name);
    return { app, listeners, pins, hold, named };
}

/** Main shows s-a, its backend pin. */
function shownOnA(opts = {}) {
    const disk = opts.disk ?? [
        { session_id: 's-a', title: 'Chat A' },
        { session_id: 's-b', title: 'Chat B' },
        { session_id: 's-c', title: 'Chat C' },
    ];
    const ctx = setup({ ...opts, disk });
    ctx.app.activeSessionId = 's-a';
    ctx.app.currentAcpSessionId = 's-a';
    ctx.app._transcriptOf = 's-a';
    ctx.app.elements.messagesArea.innerHTML = '<div class="message">A transcript</div>';
    ctx.app.elements.chatHeaderTitle.textContent = 'Chat A';
    return ctx;
}

const userBubbles = () => [...document.querySelectorAll('#messagesArea .message.user')];

describe('V1: New Chat, then open in chat on a session the backend makes', () => {
    it("New Chat's late answer neither replaces the prompt's session nor keeps the pin", async () => {
        const { app, listeners, pins, hold, named } = setup({ pins: {} });
        app.activeSessionId = null;
        app.currentAcpSessionId = null;
        const newChat = hold('switch_acp_session', (a) => a?.sessionId === null);
        const creating = app.createNewSession();
        await flush();

        // Inline assist's open in chat: main has no pin, so the backend
        // makes a session for the prompt, pins main to it and says so.
        listeners.initial_message({
            payload: { message: 'explain this', turnId: 'assist-1', sessionId: null },
        });
        pins.main = 's-p';
        listeners.initial_message_session({ payload: { sessionId: 's-p' } });
        expect(app.activeSessionId).toBe('s-p');

        // New Chat's switch lands last: the backend pinned main to it.
        newChat.resolve('s-new');
        expect(await creating).toBeNull();
        await flush();
        expect(app.activeSessionId).toBe('s-p');
        expect(app.currentAcpSessionId).toBe('s-p');
        expect(pins.main).toBe('s-p');
        expect(app._lastTurn).toMatchObject({ sessionId: 's-p', turnId: 'assist-1' });
        expect(named('send_steering_message')).toEqual([]);

        // The reply renders here and ends the wait.
        listeners.message_chunk({ payload: { sessionId: 's-p', text: 'Sure.' } });
        listeners.message_complete({ payload: { sessionId: 's-p', oldSessionId: 's-p' } });
        await flush();
        expect(app.handleMessageChunk).toHaveBeenCalledTimes(1);
        expect(app.handleMessageComplete).toHaveBeenCalledTimes(1);
        expect(app.isWaitingForResponse).toBe(false);
        expect(app.streamRegistry.isUnread('s-p')).toBe(false);
        expect(userBubbles().map((el) => el.textContent)).toEqual([
            expect.stringContaining('explain this'),
        ]);
    });

    it("New Chat's late answer before the prompt's session is announced changes nothing either", async () => {
        const { app, listeners, pins, hold } = setup({ pins: {} });
        app.activeSessionId = null;
        app.currentAcpSessionId = null;
        const newChat = hold('switch_acp_session', (a) => a?.sessionId === null);
        const creating = app.createNewSession();
        await flush();
        listeners.initial_message({
            payload: { message: 'explain this', turnId: 'assist-1', sessionId: null },
        });
        newChat.resolve('s-new');
        expect(await creating).toBeNull();
        await flush();
        expect(app.activeSessionId).toBeNull();
        expect(app.isWaitingForResponse).toBe(true);

        // The backend pins main to the prompt's session after New Chat's.
        pins.main = 's-p';
        listeners.initial_message_session({ payload: { sessionId: 's-p' } });
        await flush();
        expect(app.activeSessionId).toBe('s-p');
        expect(pins.main).toBe('s-p');
        listeners.message_complete({ payload: { sessionId: 's-p', oldSessionId: 's-p' } });
        await flush();
        expect(app.handleMessageComplete).toHaveBeenCalledTimes(1);
    });
});

describe('V2: a focus refresh during a switch that is then refused', () => {
    async function refusedAfterFocus() {
        const ctx = shownOnA();
        const { app, hold } = ctx;
        // A turn runs on s-a in the background.
        app.streamRegistry.begin('s-a');
        const switchB = hold('switch_acp_session', (a) => a?.sessionId === 's-b');
        const selecting = app.selectSession('s-b');
        await flush();
        expect(app.activeSessionId).toBe('s-b');
        // The window gets focus: its refresh reads the backend pin (still A).
        await app.loadCurrentSessionId();
        switchB.reject(LOCKED);
        await selecting;
        await flush();
        app.elements.messagesArea.innerHTML = '<div class="message">B transcript</div>';
        return ctx;
    }

    it('the refusal leaves no session of its own: a turn finishing on A stays a background one', async () => {
        const { app, listeners, pins } = await refusedAfterFocus();
        expect(app.currentAcpSessionId).toBeNull();
        listeners.message_complete({ payload: { sessionId: 's-a', oldSessionId: 's-a' } });
        await flush();
        expect(app.activeSessionId).toBe('s-b');
        expect(app.currentAcpSessionId).toBeNull();
        expect(app.handleMessageComplete).not.toHaveBeenCalled();
        expect(app.streamRegistry.isUnread('s-a')).toBe(true);
        expect(pins.main).toBe('s-a');
        expect(app.elements.messagesArea.textContent).toBe('B transcript');
    });

    it('a migration on A does not take the view either', async () => {
        const { app, listeners, pins } = await refusedAfterFocus();
        listeners.session_migrated({ payload: { oldSessionId: 's-a', newSessionId: 's-a2' } });
        listeners.message_complete({ payload: { sessionId: 's-a2', oldSessionId: 's-a' } });
        await flush();
        expect(app.activeSessionId).toBe('s-b');
        expect(app.currentAcpSessionId).toBeNull();
        expect(app.handleMessageComplete).not.toHaveBeenCalled();
        expect(pins.main).toBe('s-a');
    });

    it('a pin read that resolves after a switch landed does not undo it', async () => {
        const { app, hold, pins } = shownOnA();
        const read = hold('get_window_session');
        const reading = app.loadCurrentSessionId();
        await app.selectSession('s-b');
        await flush();
        expect(app.currentAcpSessionId).toBe('s-b');
        read.resolve('s-a');
        await reading;
        expect(app.currentAcpSessionId).toBe('s-b');
        expect(pins.main).toBe('s-b');
    });
});

describe('the pre-migration session, selected again after its turn moved on', () => {
    /**
     * Main sent a turn on s-a; recovery migrated it to s-a2 and main took
     * s-a2; the turn completed. Then the user selects the old s-a, still in
     * the sidebar.
     */
    async function backOnPreMigration() {
        const ctx = shownOnA();
        const { app, listeners } = ctx;
        app._recordTurn('s-a', 'hello', null);
        listeners.session_migrated({ payload: { oldSessionId: 's-a', newSessionId: 's-a2' } });
        await flush();
        expect(app.activeSessionId).toBe('s-a2');
        listeners.message_complete({ payload: { sessionId: 's-a2', oldSessionId: 's-a' } });
        await flush();
        expect(app.handleMessageComplete).toHaveBeenCalledTimes(1);
        app.handleMessageComplete.mockClear();
        await app.selectSession('s-a');
        await flush();
        expect(app.activeSessionId).toBe('s-a');
        expect(ctx.pins.main).toBe('s-a');
        return ctx;
    }

    it('a turn finishing on the migrated session (sent elsewhere) stays a background one', async () => {
        const { app, listeners, pins } = await backOnPreMigration();
        // A turn sent on s-a2 from another window streams in the background.
        app.streamRegistry.begin('s-a2');
        listeners.message_complete({ payload: { sessionId: 's-a2', oldSessionId: 's-a2' } });
        await flush();
        expect(app.activeSessionId).toBe('s-a');
        expect(app.currentAcpSessionId).toBe('s-a');
        expect(app.handleMessageComplete).not.toHaveBeenCalled();
        expect(app.streamRegistry.isUnread('s-a2')).toBe(true);
        expect(pins.main).toBe('s-a');
    });

    it("still in flight on the migrated session shown, a completion naming the send's first id is ours", async () => {
        const { app, listeners, pins } = shownOnA();
        app.isWaitingForResponse = true;
        app._recordTurn('s-a', 'hello', null);
        listeners.session_migrated({ payload: { oldSessionId: 's-a', newSessionId: 's-a2' } });
        await flush();
        expect(app.activeSessionId).toBe('s-a2');
        // A second recovery whose session_migrated hasn't reached main: the
        // completion names the final id and the id the send went out on.
        listeners.message_complete({ payload: { sessionId: 's-a3', oldSessionId: 's-a' } });
        await flush();
        expect(app.handleMessageComplete).toHaveBeenCalledTimes(1);
        expect(app.activeSessionId).toBe('s-a3');
        expect(app.currentAcpSessionId).toBe('s-a3');
        expect(pins.main).toBe('s-a3');
    });

    it('a later migration of the migrated session does not take the view either', async () => {
        const { app, listeners, pins } = await backOnPreMigration();
        listeners.session_migrated({ payload: { oldSessionId: 's-a2', newSessionId: 's-a3' } });
        await flush();
        expect(app.activeSessionId).toBe('s-a');
        expect(app.currentAcpSessionId).toBe('s-a');
        expect(pins.main).toBe('s-a');
    });
});

describe("V3: a prompt from Floating while the shared session's history loads", () => {
    it('the late history neither erases the prompt nor detaches its reply', async () => {
        const { app, listeners, hold } = setup({
            pins: { main: 's-f', floating: 's-f' },
            disk: [{ session_id: 's-f', title: 'Floating chat' }],
        });
        delete app.displaySession; // the production render
        app.floatingSessionId = 's-f';
        app.currentAcpSessionId = 's-f';
        app.activeSessionId = null;
        const load = hold('load_session', (a) => a?.sessionId === 's-f');
        const painting = app._showCurrentSession();
        await flush();
        expect(app.activeSessionId).toBe('s-f');

        listeners.floating_message_sent({ payload: { message: 'from floating' } });
        expect(app.isWaitingForResponse).toBe(true);
        const reply = app.currentStreamingMessage;
        expect(reply?.isConnected).toBe(true);

        load.resolve(history('old'));
        await painting;
        await flush();
        expect(userBubbles().map((el) => el.textContent)).toEqual([
            expect.stringContaining('from floating'),
        ]);
        expect(app.currentStreamingMessage).toBe(reply);
        expect(reply.isConnected).toBe(true);
        expect(app.isWaitingForResponse).toBe(true);

        listeners.message_chunk({ payload: { sessionId: 's-f', text: 'Hi.' } });
        listeners.message_complete({ payload: { sessionId: 's-f', oldSessionId: 's-f' } });
        await flush();
        expect(app.handleMessageChunk).toHaveBeenCalledTimes(1);
        expect(app.handleMessageComplete).toHaveBeenCalledTimes(1);
        expect(app.isWaitingForResponse).toBe(false);
    });

    it('during a selection of that session: its late transcript and switch change nothing', async () => {
        const { app, listeners, hold, pins } = shownOnA({
            pins: { main: 's-a', floating: 's-b' },
        });
        delete app.displaySession;
        app.floatingSessionId = 's-b';
        const load = hold('load_session', (a) => a?.sessionId === 's-b');
        const selecting = app.selectSession('s-b');
        await flush();
        listeners.floating_message_sent({ payload: { message: 'from floating' } });
        const reply = app.currentStreamingMessage;
        load.resolve(history('old'));
        await selecting;
        await flush();
        expect(userBubbles().map((el) => el.textContent)).toEqual([
            expect.stringContaining('from floating'),
        ]);
        expect(reply.isConnected).toBe(true);
        expect(app.isWaitingForResponse).toBe(true);
        expect(app.activeSessionId).toBe('s-b');
        expect(app.currentAcpSessionId).toBe('s-b');
        expect(pins.main).toBe('s-b');
    });
});

describe('a superseded result changes nothing', () => {
    /**
     * Work still in flight when the view changes, each started on main
     * showing s-a. `start` returns the hold to release and the promise of
     * the work; `release` answers it late.
     */
    const pending = {
        "a selection's transcript": {
            start: ({ app, hold }) => {
                const h = hold('load_session', (a) => a?.sessionId === 's-b');
                return { h, done: app.selectSession('s-b') };
            },
            release: (h) => h.resolve(history('b')),
        },
        "a selection's switch": {
            start: ({ app, hold }) => {
                const h = hold('switch_acp_session', (a) => a?.sessionId === 's-b');
                return { h, done: app.selectSession('s-b') };
            },
            release: (h) => h.resolve('s-b'),
        },
        "a refused selection's switch": {
            start: ({ app, hold }) => {
                const h = hold('switch_acp_session', (a) => a?.sessionId === 's-b');
                return { h, done: app.selectSession('s-b') };
            },
            release: (h) => h.reject(LOCKED),
        },
        'a New Chat': {
            start: ({ app, hold }) => {
                const h = hold('switch_acp_session', (a) => a?.sessionId === null);
                return { h, done: app.createNewSession() };
            },
            release: (h) => h.resolve('s-late'),
        },
        "the current session's paint": {
            start: ({ app, hold }) => {
                app.activeSessionId = null;
                const h = hold('load_session', (a) => a?.sessionId === 's-a');
                return { h, done: app._showCurrentSession() };
            },
            release: (h) => h.resolve(history('a')),
        },
        'a read of the backend pin': {
            start: ({ app, hold }) => {
                const h = hold('get_window_session');
                return { h, done: app.loadCurrentSessionId() };
            },
            release: (h) => h.resolve('s-stale'),
        },
    };

    /** What makes the view change while that work is in flight. */
    const superseders = {
        'the user selects another session': async ({ app }) => app.selectSession('s-c'),
        'a New Chat': async ({ app }) => app.createNewSession(),
        'open in chat on the pin': async ({ app, listeners, pins }) => {
            listeners.initial_message({
                payload: { message: 'explain this', turnId: 'assist-1', sessionId: pins.main },
            });
        },
    };

    const snapshot = (app, pins) => ({
        active: app.activeSessionId,
        current: app.currentAcpSessionId,
        pin: pins.main,
        header: app.elements.chatHeaderTitle.textContent,
        transcript: app.elements.messagesArea.innerHTML,
        inputDisabled: app.elements.chatInput.disabled,
        waiting: app.isWaitingForResponse,
        sessions: app.sessions.map((s) => s.session_id),
    });

    for (const [what, work] of Object.entries(pending)) {
        for (const [by, supersede] of Object.entries(superseders)) {
            it(`${what}, superseded when ${by}`, async () => {
                const ctx = shownOnA();
                const { app, pins, named } = ctx;
                delete app.displaySession;
                const { h, done } = work.start(ctx);
                await flush();
                await supersede(ctx);
                await flush();
                const before = snapshot(app, pins);
                const pinsBefore = named('set_window_session').length;
                // The shown session is set up and pinned.
                expect(before.active).toBe(before.current);
                expect(before.pin).toBe(before.active ?? before.pin);

                work.release(h);
                await done;
                await flush();
                expect(snapshot(app, pins)).toEqual(before);
                // Nothing pins the window to a session it doesn't show.
                const repins = named('set_window_session').slice(pinsBefore);
                for (const [, a] of repins) expect(a.sessionId).toBe(before.active);
            });
        }
    }

    it('a forced re-render of a session whose switch is in flight shares that switch and applies it', async () => {
        const { app, hold, named, pins } = shownOnA();
        const switchB = hold('switch_acp_session', (a) => a?.sessionId === 's-b');
        const selecting = app.selectSession('s-b');
        await flush();
        // The Other Agents view painted over the transcript; back to Kage.
        const forced = app.selectSession('s-b', { force: true });
        await flush();
        switchB.resolve('s-b');
        await Promise.all([selecting, forced]);
        await flush();
        expect(named('switch_acp_session').map(([, a]) => a.sessionId)).toEqual(['s-b']);
        expect(app.currentAcpSessionId).toBe('s-b');
        expect(app.elements.chatInput.disabled).toBe(false);
        expect(app.elements.sendBtn.disabled).toBe(false);
        expect(pins.main).toBe('s-b');
    });

    it('a superseded session list is not applied; a fresh one is', async () => {
        const { app, hold } = shownOnA();
        const list = hold('list_sessions');
        const loading = app.loadSessions();
        await flush();
        await app.selectSession('s-c');
        list.resolve([{ session_id: 's-stale', title: 'Stale' }]);
        await loading;
        await flush();
        expect(app.sessions.map((s) => s.session_id)).toEqual(['s-a', 's-b', 's-c']);
    });

    it('a superseded switch that lands after a refused one never pins the refused session', async () => {
        const { app, hold, pins, named } = shownOnA();
        const switchC = hold('switch_acp_session', (a) => a?.sessionId === 's-c');
        const switchB = hold('switch_acp_session', (a) => a?.sessionId === 's-b');
        const selectingC = app.selectSession('s-c');
        await flush();
        const selectingB = app.selectSession('s-b');
        await flush();
        switchB.reject(LOCKED);
        await selectingB;
        switchC.resolve('s-c');
        await selectingC;
        await flush();
        expect(app.activeSessionId).toBe('s-b');
        expect(app.currentAcpSessionId).toBeNull();
        expect(named('set_window_session').map(([, a]) => a.sessionId)).not.toContain('s-b');
        expect(pins.main).toBe('s-c');
    });
});

describe('W: a send or a pin while a selection is under way', () => {
    it("Enter during a selection's history load keeps the draft (nothing sent, nothing cleared)", async () => {
        const { app, hold, named } = shownOnA();
        const load = hold('load_session', (a) => a?.sessionId === 's-b');
        const selecting = app.selectSession('s-b');
        await flush();
        app.elements.chatInput.value = 'my draft';
        await app.handleEnterKey();
        await flush();
        expect(named('send_message_streaming')).toEqual([]);
        expect(app.elements.chatInput.value).toBe('my draft');
        load.resolve({ messages: [] });
        await selecting;
    });

    it('a text file read finishing after the user moved to another session sends nothing there', async () => {
        const { app, hold, named } = shownOnA();
        let finishRead;
        const file = { name: 'notes.txt', text: () => new Promise((r) => (finishRead = r)) };
        app._pendingFiles = [file];
        app.elements.chatInput.value = 'summarise this';
        const sending = app.sendMessage();
        await flush();
        // The read is under way; the user picks B.
        const load = hold('load_session', (a) => a?.sessionId === 's-b');
        const selecting = app.selectSession('s-b');
        await flush();
        finishRead('file body');
        await sending;
        await flush();
        expect(named('send_message_streaming')).toEqual([]);
        // The draft and the file are kept for the user to send again.
        expect(app.elements.chatInput.value).toBe('summarise this');
        expect(app._pendingFiles).toEqual([file]);
        load.resolve({ messages: [] });
        await selecting;
    });

    it("a superseded switch landing while the next selection loads never pins a session that isn't set up", async () => {
        const { app, pins, hold, named } = shownOnA();
        const switchB = hold('switch_acp_session', (a) => a?.sessionId === 's-b');
        const toB = app.selectSession('s-b');
        await flush();
        // C's history load is under way when B's (superseded) switch lands.
        const loadC = hold('load_session', (a) => a?.sessionId === 's-c');
        const switchC = hold('switch_acp_session', (a) => a?.sessionId === 's-c');
        const toC = app.selectSession('s-c');
        await flush();
        switchB.resolve('s-b');
        await flush();
        expect(named('set_window_session').map(([, a]) => a.sessionId)).not.toContain('s-c');
        loadC.resolve({ messages: [] });
        await flush();
        // C's own switch is refused: the backend stays pinned to a session
        // that is set up, never to C.
        switchC.reject({ kind: 'internal', message: LOCKED });
        await Promise.allSettled([toB, toC]);
        await flush();
        expect(pins.main).not.toBe('s-c');
    });
});
