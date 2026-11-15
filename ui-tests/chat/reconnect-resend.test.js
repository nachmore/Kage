/**
 * Reconnect under a failed prompt in the chat window: it restarts the agent
 * (reconnect_acp) and then resends the failed turn, but only when nothing of
 * it streamed, it's still the window's latest send on the session it shows,
 * and it wasn't a rate limit. The resend adds no second user bubble and
 * leaves the composer's draft alone.
 */
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { marked } from 'marked';
import * as i18n from '../../ui/js/shared/i18n.js';
import { initEnglish } from '../helpers/sync-i18n.js';

const INCIDENT =
    'Failed to send: ACP error: Internal error — Encountered an error in the response stream: An unknown error occurred: dispatch failure';

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

async function sendAndFail(app, payload = INCIDENT) {
    app.elements.chatInput.value = 'hello';
    await app.sendMessage();
    await fail(app, payload);
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
        });
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
        await fail(app, INCIDENT);

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
        await fail(app, INCIDENT);

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
        await fail(app, INCIDENT);

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
        listeners.initial_message({ payload: 'from inline assist' });
        await fail(app, INCIDENT);

        clickReconnect();
        await flush();

        expect(sends(invoke)).toHaveLength(1);
        expect(sends(invoke)[0][1]).toEqual({
            sessionId: 's1',
            message: 'from inline assist',
            attachments: null,
        });
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
        await sendAndFail(app, 'Rate limit exceeded. Please wait a moment.');

        clickReconnect();
        await flush();

        expect(sends(invoke)).toHaveLength(1);
    });
});
