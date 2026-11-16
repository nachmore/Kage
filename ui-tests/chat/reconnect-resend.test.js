/**
 * Reconnect under a failed prompt in the chat window: it restarts the agent
 * (reconnect_acp) and then resends the failed turn, but only when the
 * backend's error says it is `resendable` for that turn's session, nothing
 * of it streamed, it's still the window's latest send on the session it
 * shows (before and after the Reconnect), and it wasn't a rate limit. The
 * resend adds no second user bubble and leaves the composer's draft alone.
 */
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { marked } from 'marked';
import * as i18n from '../../ui/js/shared/i18n.js';
import { initEnglish } from '../helpers/sync-i18n.js';

const INCIDENT =
    'Failed to send: ACP error: Internal error — Encountered an error in the response stream: An unknown error occurred: dispatch failure';

/**
 * The backend's object form for a prompt that failed: `resendable` is true
 * only when its turn progress shows nothing got through (no text, tool,
 * permission request, fs/terminal request or cancel).
 */
function promptFailure(sessionId = 's1', resendable = true, turnId = undefined) {
    return {
        kind: 'agent_unreachable',
        key: 'errors.agent.unreachable_after_restart_kiro',
        message: 'Kage restarted Kiro and tried again, but it still failed.',
        detail: INCIDENT,
        sessionId,
        resendable,
        turnId,
    };
}

/** The turn id the latest send_message_streaming carried. */
function lastTurnId(invoke) {
    return sends(invoke).at(-1)?.[1]?.turnId;
}

let ChatApp;
beforeAll(async () => {
    globalThis.marked = marked;
    await initEnglish(i18n);
    ({ ChatApp } = await import('../../ui/js/chat/app.js'));
});
afterEach(() => {
    document.body.innerHTML = '';
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

function makeApp({ reconnect } = {}) {
    vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ text: async () => '<svg xmlns="http://www.w3.org/2000/svg"/>' }))
    );
    document.body.innerHTML = `
        <div id="errorContainer"></div>
        <div class="messages-area" id="messagesArea"></div>
        <div id="chatHeaderTitle"></div>
        <textarea id="chatInput"></textarea>
        <button id="sendBtn"></button>`;
    const listeners = {};
    const invoke = vi.fn(async (cmd, args) => {
        if (cmd === 'reconnect_acp') {
            if (reconnect) return reconnect();
            return { reconnected: true, sessionId: args.sessionId, sessionReloaded: true };
        }
        return null;
    });
    const listen = vi.fn(async (evt, cb) => {
        listeners[evt] = cb;
        return () => {};
    });
    const app = new ChatApp(invoke, {}, listen);
    app.elements = {
        errorContainer: document.getElementById('errorContainer'),
        messagesArea: document.getElementById('messagesArea'),
        chatHeaderTitle: document.getElementById('chatHeaderTitle'),
        chatInput: document.getElementById('chatInput'),
        sendBtn: document.getElementById('sendBtn'),
    };
    for (const m of [
        'renderSessionList',
        'updateConnectionStatus',
        'updateInputState',
        'scrollToBottom',
        'clearSuggestions',
        'hideSuggestionChips',
        'showTypingIndicator',
        'hideTypingIndicator',
    ]) {
        app[m] = vi.fn();
    }
    app.activeSessionId = 's1';
    return { app, invoke, listeners };
}

async function flush() {
    for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0));
}

function sends(invoke) {
    return invoke.mock.calls.filter(([cmd]) => cmd === 'send_message_streaming');
}

function userBubbles(app) {
    return app.elements.messagesArea.querySelectorAll('.message.user');
}

/** The turn failed: the host's onError, as the stream controller calls it. */
async function fail(app, payload) {
    await app.messageStreamController.host.onError({ payload }, true);
}

/** Send "hello", then fail it: by default with an error naming its turn. */
async function sendAndFail(app, payload) {
    app.elements.chatInput.value = 'hello';
    await app.sendMessage();
    await fail(app, payload ?? promptFailure('s1', true, lastTurnId(app.invoke)));
}

function clickReconnect() {
    document.getElementById('errorReconnectBtn').click();
}

describe('chat Reconnect after a failed prompt', () => {
    it('reconnects, then resends the turn without a second bubble', async () => {
        const { app, invoke } = makeApp();
        await sendAndFail(app);
        const bubbles = app.elements.messagesArea.children.length;
        app.elements.chatInput.value = 'a draft';

        clickReconnect();
        await flush();

        expect(invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's1' });
        expect(sends(invoke)).toHaveLength(2);
        expect(sends(invoke)[1][1]).toEqual({
            sessionId: 's1',
            message: 'hello',
            attachments: null,
            turnId: expect.any(String),
        });
        // The resend is a turn of its own.
        expect(sends(invoke)[1][1].turnId).not.toBe(sends(invoke)[0][1].turnId);
        expect(userBubbles(app)).toHaveLength(1);
        expect(app.messages.filter((m) => m.role === 'user')).toHaveLength(1);
        // Only the new streaming placeholder was added.
        expect(app.elements.messagesArea.children.length).toBe(bubbles + 1);
        expect(app.elements.chatInput.value).toBe('a draft');
        expect(app.elements.errorContainer.innerHTML).toBe('');
        expect(app.isWaitingForResponse).toBe(true);
    });

    it('does not resend a turn that already streamed', async () => {
        const { app, invoke } = makeApp();
        app.elements.chatInput.value = 'hello';
        await app.sendMessage();
        app.currentStreamingContent = 'Partial answer';
        await fail(app, promptFailure('s1', true, lastTurnId(invoke)));

        clickReconnect();
        await flush();

        expect(invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's1' });
        expect(sends(invoke)).toHaveLength(1);
    });

    it('does not resend after the window moved to another session', async () => {
        const { app, invoke } = makeApp();
        await sendAndFail(app);
        app.activeSessionId = 's9';

        clickReconnect();
        await flush();

        expect(invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's9' });
        expect(sends(invoke)).toHaveLength(1);
    });

    it('does not resend into the session the user switched to while the turn failed', async () => {
        const { app, invoke } = makeApp();
        app.elements.chatInput.value = 'hello';
        await app.sendMessage();
        // Switched before the error arrived (the ladder took its time).
        app.activeSessionId = 's2';
        await fail(app, promptFailure('s1', true, lastTurnId(invoke)));

        clickReconnect();
        await flush();

        expect(invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's2' });
        expect(sends(invoke)).toHaveLength(1);
    });

    it('follows a mid-turn session migration and resends to the new id', async () => {
        const { app, invoke, listeners } = makeApp();
        app.appWindow = { listen: vi.fn(async () => () => {}) };
        app.setupStreamingListeners();
        app.elements.chatInput.value = 'hello';
        await app.sendMessage();
        listeners.session_migrated({ payload: { oldSessionId: 's1', newSessionId: 's7' } });
        // As the MESSAGE_ERROR listener does: the shown session's stream ends.
        app.streamRegistry.fail(app.activeSessionId);
        await fail(app, promptFailure('s1', true, lastTurnId(invoke)));

        clickReconnect();
        await flush();

        expect(invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's7' });
        expect(sends(invoke)).toHaveLength(2);
        expect(sends(invoke)[1][1].sessionId).toBe('s7');
    });

    it('a new prompt clears the old error and its Reconnect', async () => {
        const { app } = makeApp();
        await sendAndFail(app);
        expect(document.getElementById('errorReconnectBtn')).not.toBeNull();

        app.elements.chatInput.value = 'next';
        await app.sendMessage();

        expect(document.getElementById('errorReconnectBtn')).toBeNull();
        expect(app.elements.errorContainer.innerHTML).toBe('');
    });

    it('does not resend once a newer prompt went out', async () => {
        const { app, invoke } = makeApp();
        await sendAndFail(app);
        app._recordTurn('s1', 'newer', null);

        clickReconnect();
        await flush();

        expect(sends(invoke)).toHaveLength(1);
    });

    it('resends a prompt that open_chat_with_message sent', async () => {
        const { app, invoke, listeners } = makeApp();
        app.appWindow = { listen: vi.fn(async () => () => {}) };
        app.setupStreamingListeners();
        // The backend names the turn it sent for this window.
        listeners.initial_message({
            payload: { message: 'from inline assist', turnId: 'assist-turn-1' },
        });
        await fail(app, promptFailure('s1', true, 'assist-turn-1'));

        clickReconnect();
        await flush();

        expect(sends(invoke)).toHaveLength(1);
        expect(sends(invoke)[0][1]).toEqual({
            sessionId: 's1',
            message: 'from inline assist',
            attachments: null,
            turnId: expect.any(String),
        });
        expect(userBubbles(app)).toHaveLength(1);
    });

    it('never resends an open_chat_with_message prompt whose turn has no id', async () => {
        const { app, invoke, listeners } = makeApp();
        app.appWindow = { listen: vi.fn(async () => () => {}) };
        app.setupStreamingListeners();
        // The old payload: the message alone.
        listeners.initial_message({ payload: 'from inline assist' });
        await fail(app, promptFailure());

        clickReconnect();
        await flush();

        expect(sends(invoke)).toHaveLength(0);
        expect(userBubbles(app)).toHaveLength(1);
    });

    it('renders a structured payload with an escaped details line', async () => {
        const { app } = makeApp();
        await sendAndFail(app, {
            kind: 'auth_required',
            key: 'errors.agent.auth_required_kiro',
            message: 'Kiro needs you to sign in. Run kiro-cli login in a terminal, then click Reconnect.',
            detail: 'ACP error: <b>Authentication required</b>',
            sessionId: 's1',
        });

        const box = app.elements.errorContainer;
        expect(box.textContent).toContain('Kiro needs you to sign in.');
        expect(box.textContent).not.toContain('[object Object]');
        expect(box.textContent).not.toContain('Error: ');
        const detail = box.querySelector('.error-detail');
        expect(detail.textContent).toBe('Details: ACP error: <b>Authentication required</b>');
        expect(detail.querySelector('b')).toBeNull();
        expect(detail.getAttribute('dir')).toBe('ltr');
    });

    it('keeps the "Error: …" text for plain-string payloads', async () => {
        const { app } = makeApp();
        await sendAndFail(app, INCIDENT);

        const box = app.elements.errorContainer;
        expect(box.textContent).toContain(`Error: ${INCIDENT}`);
        expect(box.querySelector('.error-detail')).toBeNull();
    });

    it('shows why when reconnect fails, and does not resend', async () => {
        const { app, invoke } = makeApp({
            reconnect: () =>
                Promise.reject({
                    kind: 'auth_required',
                    message: 'Kiro needs you to sign in. Run kiro-cli login in a terminal, then click Reconnect.',
                }),
        });
        await sendAndFail(app);

        clickReconnect();
        await flush();

        const text = app.elements.errorContainer.textContent;
        expect(text).toContain('Reconnection failed: Kiro needs you to sign in');
        expect(text).not.toContain('[object Object]');
        expect(sends(invoke)).toHaveLength(1);
    });

    it('never resends a rate-limited prompt', async () => {
        const { app, invoke } = makeApp();
        app.elements.chatInput.value = 'hello';
        await app.sendMessage();
        await fail(app, {
            ...promptFailure('s1', true, lastTurnId(invoke)),
            kind: 'rate_limited',
            message: 'Rate limit exceeded. Please wait a moment.',
        });

        clickReconnect();
        await flush();

        expect(sends(invoke)).toHaveLength(1);
    });

    // F3: the backend saw progress the window didn't (a permission request
    // the user approved, an fs/terminal request), so it says not resendable.
    it('does not resend when the backend says part of the turn got through', async () => {
        const { app, invoke } = makeApp();
        await sendAndFail(app, promptFailure('s1', false));

        clickReconnect();
        await flush();

        expect(invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's1' });
        expect(sends(invoke)).toHaveLength(1);
    });

    it('does not resend when the payload does not say it is resendable', async () => {
        const { app, invoke } = makeApp();
        app.elements.chatInput.value = 'hello';
        await app.sendMessage();
        const { resendable: _omitted, ...noFlag } = promptFailure('s1', true, lastTurnId(invoke));
        await fail(app, noFlag);
        clickReconnect();
        await flush();
        expect(sends(invoke)).toHaveLength(1);
    });

    it('never resends on a plain-string error (no session, no progress record)', async () => {
        const { app, invoke } = makeApp();
        await sendAndFail(app, INCIDENT);

        clickReconnect();
        await flush();

        expect(invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's1' });
        expect(sends(invoke)).toHaveLength(1);
    });

    it('does not resend a turn on another session than the error names', async () => {
        const { app, invoke } = makeApp();
        app.elements.chatInput.value = 'hello';
        await app.sendMessage();
        await fail(app, promptFailure('s5', true, lastTurnId(invoke)));

        clickReconnect();
        await flush();

        expect(sends(invoke)).toHaveLength(1);
    });

    // F2: A fails on s1 while the user has moved to s2 and sent B there.
    it("another session's error neither stops the shown turn nor becomes its retry", async () => {
        const { app, invoke, listeners } = makeApp();
        app.appWindow = { listen: vi.fn(async () => () => {}) };
        app.setupStreamingListeners();
        app.elements.chatInput.value = 'A';
        await app.sendMessage();
        const turnA = lastTurnId(invoke);
        // Switch away from the live turn, as selectSession does.
        app.hideTypingIndicator();
        app.currentStreamingMessage = null;
        app.currentStreamingContent = '';
        app.isWaitingForResponse = false;
        app.activeSessionId = 's2';
        app.currentAcpSessionId = 's2';
        app.elements.chatInput.value = 'B';
        await app.sendMessage();
        expect(sends(invoke).map(([, a]) => a.message)).toEqual(['A', 'B']);
        const bubble = app.currentStreamingMessage;
        app.streamRegistry.begin('s1');
        app.streamRegistry.begin('s2');

        // A's error arrives before B has any output.
        await listeners.message_error({ payload: promptFailure('s1', true, turnA) });
        await flush();

        expect(app.isWaitingForResponse).toBe(true);
        expect(app.currentStreamingMessage).toBe(bubble);
        expect(bubble.isConnected).toBe(true);
        expect(app.streamRegistry.isStreaming('s2')).toBe(true);
        expect(app.streamRegistry.isStreaming('s1')).toBe(false);
        document.getElementById('errorReconnectBtn')?.click();
        await flush();
        expect(invoke).not.toHaveBeenCalledWith('reconnect_acp', expect.anything());
        expect(sends(invoke)).toHaveLength(2);
    });

    it("another session's error still shows when nothing is in flight, without a resend", async () => {
        const { app, invoke, listeners } = makeApp();
        app.appWindow = { listen: vi.fn(async () => () => {}) };
        app.setupStreamingListeners();
        app.elements.chatInput.value = 'A';
        await app.sendMessage();
        app.hideTypingIndicator();
        app.currentStreamingMessage = null;
        app.isWaitingForResponse = false;
        app.activeSessionId = 's2';
        app.currentAcpSessionId = 's2';

        await listeners.message_error({ payload: promptFailure('s1') });
        await flush();
        expect(app.elements.errorContainer.textContent).toContain('still failed');

        clickReconnect();
        await flush();
        expect(invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's2' });
        expect(sends(invoke)).toHaveLength(1);
    });

    it("the shown session's error still ends its turn through the listener", async () => {
        const { app, invoke, listeners } = makeApp();
        app.appWindow = { listen: vi.fn(async () => () => {}) };
        app.setupStreamingListeners();
        app.elements.chatInput.value = 'hello';
        await app.sendMessage();

        await listeners.message_error({ payload: promptFailure('s1', true, lastTurnId(invoke)) });
        await flush();
        expect(app.isWaitingForResponse).toBe(false);

        clickReconnect();
        await flush();
        expect(sends(invoke)).toHaveLength(2);
    });

    it('a plain-string error still ends the shown turn, without a resend', async () => {
        const { app, invoke, listeners } = makeApp();
        app.appWindow = { listen: vi.fn(async () => () => {}) };
        app.setupStreamingListeners();
        app.elements.chatInput.value = 'hello';
        await app.sendMessage();

        await listeners.message_error({ payload: INCIDENT });
        await flush();
        expect(app.isWaitingForResponse).toBe(false);

        clickReconnect();
        await flush();
        expect(sends(invoke)).toHaveLength(1);
    });

    it('matches an error naming the pre-migration id to the migrated turn', async () => {
        const { app, invoke, listeners } = makeApp();
        app.appWindow = { listen: vi.fn(async () => () => {}) };
        app.setupStreamingListeners();
        app.elements.chatInput.value = 'hello';
        await app.sendMessage();
        listeners.session_migrated({ payload: { oldSessionId: 's1', newSessionId: 's7' } });

        await listeners.message_error({ payload: promptFailure('s1', true, lastTurnId(invoke)) });
        await flush();
        expect(app.isWaitingForResponse).toBe(false);

        clickReconnect();
        await flush();
        expect(sends(invoke)).toHaveLength(2);
        expect(sends(invoke)[1][1].sessionId).toBe('s7');
    });

    // F6: Reconnect started on s1, the user selected s2 before it finished.
    it('does not repin or resend when the window moved during Reconnect', async () => {
        let finish;
        const { app, invoke } = makeApp({
            reconnect: () =>
                new Promise((r) => {
                    finish = r;
                }),
        });
        await sendAndFail(app);
        clickReconnect();
        await flush();
        app.activeSessionId = 's2';
        app.currentAcpSessionId = 's2';
        finish({ reconnected: true, sessionId: 's1', sessionReloaded: true });
        await flush();

        expect(app.activeSessionId).toBe('s2');
        expect(app.currentAcpSessionId).toBe('s2');
        const pins = invoke.mock.calls.filter(([cmd]) => cmd === 'set_window_session');
        expect(pins.every(([, a]) => a.sessionId === 's2')).toBe(true);
        expect(sends(invoke)).toHaveLength(1);
    });

    it('does not adopt a fresh id for a session the window left', async () => {
        let finish;
        const { app, invoke } = makeApp({
            reconnect: () =>
                new Promise((r) => {
                    finish = r;
                }),
        });
        await sendAndFail(app);
        clickReconnect();
        await flush();
        app.activeSessionId = 's2';
        finish({ reconnected: true, sessionId: 's1-new', sessionReloaded: false });
        await flush();

        expect(app.activeSessionId).toBe('s2');
        expect(invoke).not.toHaveBeenCalledWith('set_window_session', {
            label: app.windowLabel,
            sessionId: 's1-new',
        });
        expect(sends(invoke)).toHaveLength(1);
    });

    // F7: the first Reconnect is refused (busy), the second goes through.
    it('a failed Reconnect keeps the failed turn for the next one, resent once', async () => {
        let calls = 0;
        const { app, invoke } = makeApp({
            reconnect: async () => {
                calls++;
                if (calls === 1) {
                    throw {
                        kind: 'connection_lost',
                        message: 'The agent is still working. Try again when it finishes.',
                    };
                }
                return { reconnected: true, sessionId: 's1', sessionReloaded: true };
            },
        });
        await sendAndFail(app);

        clickReconnect();
        await flush();
        expect(sends(invoke)).toHaveLength(1);
        expect(app.elements.errorContainer.textContent).toContain('Reconnection failed');

        clickReconnect();
        await flush();
        expect(sends(invoke)).toHaveLength(2);
        expect(sends(invoke)[1][1].message).toBe('hello');
        expect(document.getElementById('errorReconnectBtn')).toBeNull();
    });

    it('a Reconnect that reports not reconnected keeps the failed turn too', async () => {
        let calls = 0;
        const { app, invoke } = makeApp({
            reconnect: async () => {
                calls++;
                return calls === 1
                    ? { reconnected: false, sessionId: null, sessionReloaded: false }
                    : { reconnected: true, sessionId: 's1', sessionReloaded: true };
            },
        });
        await sendAndFail(app);

        clickReconnect();
        await flush();
        clickReconnect();
        await flush();

        expect(sends(invoke)).toHaveLength(2);
    });
});

// R2: an error belongs to the turn its `turnId` names, never to whatever
// turn is on the same session now.
describe('chat MESSAGE_ERROR is matched to its turn by turnId', () => {
    function setupWithListeners() {
        const made = makeApp();
        made.app.appWindow = { listen: vi.fn(async () => () => {}) };
        made.app.setupStreamingListeners();
        return made;
    }

    async function send(app, text) {
        app.elements.chatInput.value = text;
        await app.sendMessage();
    }

    it("A's late error after B succeeded on the same session never resends B", async () => {
        const { app, invoke, listeners } = setupWithListeners();
        await send(app, 'A');
        const turnA = lastTurnId(invoke);
        // The user stops A (its backend recovery goes on) and sends B.
        app.stopGenerating();
        await send(app, 'B');
        // B succeeds.
        await listeners.message_complete({ payload: { sessionId: 's1', oldSessionId: 's1' } });
        await flush();
        expect(app.isWaitingForResponse).toBe(false);

        // Then A's failure arrives, naming A's turn.
        await listeners.message_error({ payload: promptFailure('s1', true, turnA) });
        await flush();
        document.getElementById('errorReconnectBtn')?.click();
        await flush();

        expect(sends(invoke).map(([, a]) => a.message)).toEqual(['A', 'B']);
    });

    it("A's late error while B streams on the same session leaves B alone", async () => {
        const { app, invoke, listeners } = setupWithListeners();
        await send(app, 'A');
        const turnA = lastTurnId(invoke);
        app.stopGenerating();
        await send(app, 'B');
        app.streamRegistry.begin('s1');
        const bubble = app.currentStreamingMessage;

        await listeners.message_error({ payload: promptFailure('s1', true, turnA) });
        await flush();

        expect(app.isWaitingForResponse).toBe(true);
        expect(app.currentStreamingMessage).toBe(bubble);
        expect(bubble.isConnected).toBe(true);
        expect(app.streamRegistry.isStreaming('s1')).toBe(true);
        expect(document.getElementById('errorReconnectBtn')).toBeNull();
    });

    it("B's own error still ends B and may resend B", async () => {
        const { app, invoke, listeners } = setupWithListeners();
        await send(app, 'A');
        app.stopGenerating();
        await send(app, 'B');

        await listeners.message_error({ payload: promptFailure('s1', true, lastTurnId(invoke)) });
        await flush();
        expect(app.isWaitingForResponse).toBe(false);

        clickReconnect();
        await flush();
        expect(sends(invoke).map(([, a]) => a.message)).toEqual(['A', 'B', 'B']);
    });

    it('every send carries a fresh turn id', async () => {
        const { app, invoke } = setupWithListeners();
        await send(app, 'A');
        app.stopGenerating();
        await send(app, 'B');
        const [a, b] = sends(invoke).map(([, args]) => args.turnId);
        expect(typeof a).toBe('string');
        expect(a).not.toBe('');
        expect(b).not.toBe(a);
    });

    it('an error without a turn id is never resent', async () => {
        const { app, invoke } = makeApp();
        await sendAndFail(app, promptFailure('s1', true, undefined));

        clickReconnect();
        await flush();

        expect(sends(invoke)).toHaveLength(1);
    });

    /**
     * Send B on s2 (B completes), then switch back to s1 and attach to the
     * turn still running there, as session-history's switch-in does.
     */
    async function sendElsewhereThenSwitchBack(app, listeners) {
        app.currentStreamingMessage = null;
        app.isWaitingForResponse = false;
        app.activeSessionId = 's2';
        app.currentAcpSessionId = 's2';
        await send(app, 'B');
        await listeners.message_complete({ payload: { sessionId: 's2', oldSessionId: 's2' } });
        await flush();
        app.activeSessionId = 's1';
        app.currentAcpSessionId = 's1';
        app.streamRegistry.begin('s1');
        app.isWaitingForResponse = true;
        app.currentStreamingMessage = app.createMessageElement('assistant', '');
        app.elements.messagesArea.appendChild(app.currentStreamingMessage);
    }

    it("switched back to A after sending B elsewhere: A's own error ends A's stream", async () => {
        const { app, invoke, listeners } = setupWithListeners();
        await send(app, 'A');
        const turnA = lastTurnId(invoke);
        app.streamRegistry.begin('s1');
        await sendElsewhereThenSwitchBack(app, listeners);

        await listeners.message_error({ payload: promptFailure('s1', true, turnA) });
        listeners.session_activity({ payload: { sessionId: 's1', kind: 'failed' } });
        await flush();

        expect(app.isWaitingForResponse).toBe(false);
        expect(app.currentStreamingMessage).toBeNull();
        expect(app.streamRegistry.isStreaming('s1')).toBe(false);
        expect(app.elements.errorContainer.textContent).toContain(
            'Kage restarted Kiro and tried again'
        );
        // A is not the window's last turn: nothing is resent for it.
        document.getElementById('errorReconnectBtn')?.click();
        await flush();
        expect(sends(invoke).map(([, a]) => a.message)).toEqual(['A', 'B']);
    });

    it("switched back to A2 after sending B elsewhere: A's late error leaves A2 alone", async () => {
        const { app, invoke, listeners } = setupWithListeners();
        await send(app, 'A');
        const turnA = lastTurnId(invoke);
        app.stopGenerating();
        await send(app, 'A2');
        app.streamRegistry.begin('s1');
        await sendElsewhereThenSwitchBack(app, listeners);
        const bubble = app.currentStreamingMessage;

        await listeners.message_error({ payload: promptFailure('s1', true, turnA) });
        await flush();

        expect(app.isWaitingForResponse).toBe(true);
        expect(app.currentStreamingMessage).toBe(bubble);
        expect(bubble.isConnected).toBe(true);
        expect(app.streamRegistry.isStreaming('s1')).toBe(true);
    });

    it("an error naming another turn's id is never resent", async () => {
        const { app, invoke } = makeApp();
        await sendAndFail(app, promptFailure('s1', true, 'some-other-turn'));
        // Not this turn's: it goes on, and no Reconnect is offered for it.
        expect(app.isWaitingForResponse).toBe(true);

        document.getElementById('errorReconnectBtn')?.click();
        await flush();

        expect(sends(invoke)).toHaveLength(1);
    });
});
