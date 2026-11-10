/**
 * Switching into a session the window thinks is mid-turn. A "streaming"
 * mark can outlive any turn (a chunk or tool update that wasn't part of one,
 * e.g. around the session reload after an update), and then the chat window
 * sat on a typing indicator and a Stop button forever. selectSession now asks
 * the backend first and only attaches to a live stream that's really there.
 */
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { marked } from 'marked';

let ChatApp;
beforeAll(async () => {
    globalThis.marked = marked;
    ({ ChatApp } = await import('../../ui/js/chat/app.js'));
});
afterEach(() => {
    document.body.innerHTML = '';
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

function makeApp(snapshot) {
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
    const calls = [];
    const invoke = vi.fn(async (cmd, args) => {
        calls.push(cmd);
        if (cmd === 'load_session') return { messages: [] };
        if (cmd === 'get_session_stream_snapshot') return snapshot;
        if (cmd === 'switch_acp_session') return args.sessionId;
        return null;
    });
    const app = new ChatApp(invoke, {}, vi.fn(async () => () => {}));
    app.elements = {
        errorContainer: document.getElementById('errorContainer'),
        messagesArea: document.getElementById('messagesArea'),
        chatHeaderTitle: document.getElementById('chatHeaderTitle'),
        chatInput: document.getElementById('chatInput'),
        sendBtn: document.getElementById('sendBtn'),
    };
    for (const m of ['renderSessionList', 'updateConnectionStatus', 'updateInputState', 'scrollToBottom']) {
        app[m] = vi.fn();
    }
    app.displaySession = vi.fn(async () => {});
    app.showTypingIndicator = vi.fn();
    app.hideTypingIndicator = vi.fn();
    app.activeSessionId = 'other';
    app.sessions = [{ session_id: 's1', title: 'S1' }];
    app.streamRegistry.begin('s1');
    return { app, calls };
}

describe('switching into a session marked as streaming', () => {
    it('clears the mark and opens the session idle when no turn is in flight', async () => {
        const { app, calls } = makeApp({ inFlight: false, text: '', prompt: null });
        await app.selectSession('s1');
        expect(app.streamRegistry.isStreaming('s1')).toBe(false);
        expect(app.isWaitingForResponse).toBe(false);
        expect(app.showTypingIndicator).not.toHaveBeenCalled();
        expect(calls).toContain('switch_acp_session');
        expect(app.elements.chatInput.disabled).toBe(false);
    });

    it('still attaches to a turn that really is running', async () => {
        const { app, calls } = makeApp({ inFlight: true, text: '', prompt: 'hi' });
        await app.selectSession('s1');
        expect(app.streamRegistry.isStreaming('s1')).toBe(true);
        expect(app.isWaitingForResponse).toBe(true);
        expect(app.showTypingIndicator).toHaveBeenCalled();
        expect(calls).not.toContain('switch_acp_session');
    });

    it('keeps attaching when an older backend says nothing about it', async () => {
        const { app } = makeApp({ text: 'partial', prompt: null });
        await app.selectSession('s1');
        expect(app.isWaitingForResponse).toBe(true);
    });
});
