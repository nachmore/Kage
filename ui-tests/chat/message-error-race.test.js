/**
 * The chat window handles a prompt's MESSAGE_ERROR after an online probe
 * that can take seconds (slowest exactly when the network is why the agent
 * failed). If the user moved to another session and sent a prompt there
 * meanwhile, the old error must not tear that turn down.
 */
import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
import { marked } from 'marked';

let probe;
vi.mock('../../ui/js/shared/network.js', async (importOriginal) => ({
    ...(await importOriginal()),
    checkOnError: () => probe.promise,
}));

import * as i18n from '../../ui/js/shared/i18n.js';
import { EVT } from '../../ui/js/shared/events.js';
import { initEnglish } from '../helpers/sync-i18n.js';

function deferred() {
    let resolve;
    const promise = new Promise((r) => {
        resolve = r;
    });
    return { promise, resolve };
}

function promptFailure(sessionId = 's1') {
    return {
        kind: 'agent_unreachable',
        key: 'errors.agent.unreachable_after_restart_kiro',
        message: 'Kage restarted Kiro and tried again, but it still failed.',
        detail: 'dispatch failure',
        sessionId,
        resendable: true,
    };
}

let ChatApp;
beforeAll(async () => {
    globalThis.marked = marked;
    await initEnglish(i18n);
    ({ ChatApp } = await import('../../ui/js/chat/app.js'));
});
beforeEach(() => {
    probe = deferred();
});
afterEach(() => {
    document.body.innerHTML = '';
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

function makeApp() {
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
            return { reconnected: true, sessionId: args.sessionId, sessionReloaded: true };
        }
        return null;
    });
    const listen = vi.fn(async (evt, cb) => {
        listeners[evt] = cb;
        return () => {};
    });
    const app = new ChatApp(invoke, {}, listen);
    app.appWindow = { listen: vi.fn(async () => () => {}) };
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
    app.setupStreamingListeners();
    return { app, invoke, listeners };
}

async function flush() {
    for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0));
}

/** selectSession's detach from a mid-stream session, then the switch. */
function switchTo(app, sessionId) {
    app.currentStreamingMessage = null;
    app.currentStreamingContent = '';
    app.isWaitingForResponse = false;
    app.activeSessionId = sessionId;
    app.currentAcpSessionId = sessionId;
}

describe('chat MESSAGE_ERROR handled after the window moved on', () => {
    it('leaves a turn started on another session during the probe alone', async () => {
        const { app, invoke, listeners } = makeApp();
        app.elements.chatInput.value = 'A';
        await app.sendMessage();

        // A's error names the shown session; it waits on the probe.
        listeners[EVT.MESSAGE_ERROR]({ payload: promptFailure('s1') });
        await flush();

        // Meanwhile the user moves to s2 and sends B there.
        switchTo(app, 's2');
        app.elements.chatInput.value = 'B';
        await app.sendMessage();
        const bBubble = app.currentStreamingMessage;
        expect(bBubble).toBeTruthy();

        probe.resolve(true);
        await flush();

        expect(bBubble.isConnected).toBe(true);
        expect(app.currentStreamingMessage).toBe(bBubble);
        expect(app.isWaitingForResponse).toBe(true);
        expect(app.elements.errorContainer.innerHTML).toBe('');
        expect(
            invoke.mock.calls.filter(([cmd]) => cmd === 'send_message_streaming')
        ).toHaveLength(2);
    });

    it('leaves a turn started during the probe of a not-shown session alone', async () => {
        const { app, listeners } = makeApp();
        app.elements.chatInput.value = 'A';
        await app.sendMessage();
        switchTo(app, 's2');

        // A's error for s1 arrives while s2 is idle, and waits on the probe.
        listeners[EVT.MESSAGE_ERROR]({ payload: promptFailure('s1') });
        await flush();
        app.elements.chatInput.value = 'B';
        await app.sendMessage();
        const bBubble = app.currentStreamingMessage;

        probe.resolve(true);
        await flush();

        expect(bBubble.isConnected).toBe(true);
        expect(app.isWaitingForResponse).toBe(true);
        expect(app.elements.errorContainer.innerHTML).toBe('');
    });

    it('still shows the error of the turn on screen after the probe', async () => {
        const { app, listeners } = makeApp();
        app.elements.chatInput.value = 'A';
        await app.sendMessage();

        listeners[EVT.MESSAGE_ERROR]({ payload: promptFailure('s1') });
        probe.resolve(true);
        await flush();

        expect(app.isWaitingForResponse).toBe(false);
        expect(document.getElementById('errorReconnectBtn')).not.toBeNull();
    });
});
