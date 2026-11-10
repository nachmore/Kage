/**
 * `>chats` opens the chat window on the default session — the one the
 * floating launcher is on. The backend sends `chat_open_session` to main
 * once the window is shown; only the window the payload names acts on it,
 * because an event sent to one window still reaches global listeners in
 * the others.
 */
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { marked } from 'marked';
import { EVT } from '../../ui/js/shared/events.js';

let ChatApp;
beforeAll(async () => {
    globalThis.marked = marked;
    ({ ChatApp } = await import('../../ui/js/chat/app.js'));
});
afterEach(() => {
    document.body.innerHTML = '';
    vi.restoreAllMocks();
});

function makeApp({ label = 'main', sessions = [], active = 'chat-own' } = {}) {
    document.body.innerHTML = '<h1 id="title"></h1>';
    const handlers = {};
    const listen = vi.fn(async (name, cb) => {
        handlers[name] = cb;
        return () => {};
    });
    const app = new ChatApp(vi.fn(async () => ({})), { label }, listen);
    app.windowLabel = label;
    app.sessions = sessions.map((id) => ({ session_id: id, title: `Title ${id}` }));
    app.activeSessionId = active;
    app.elements = { chatHeaderTitle: document.getElementById('title') };
    app.loadSessions = vi.fn(async () => {});
    app.renderSessionList = vi.fn();
    app.selectSession = vi.fn(async (id) => {
        app.activeSessionId = id;
    });
    app._listenForOpenSession();
    return { app, fire: (payload) => handlers[EVT.CHAT_OPEN_SESSION]({ payload }) };
}

const settle = () => new Promise((r) => setTimeout(r, 0));

describe('opening the default session', () => {
    it('switches main to the launcher session and names it in the header', async () => {
        const { app, fire } = makeApp({ sessions: ['chat-own', 'floating-1'] });
        fire({ sessionId: 'floating-1', label: 'main' });
        await settle();
        expect(app.selectSession).toHaveBeenCalledWith('floating-1', { emptyIfNew: true });
        expect(app.floatingSessionId).toBe('floating-1');
        expect(app.elements.chatHeaderTitle.textContent).toBe('Title floating-1');
    });

    it('ignores the event in a window it was not meant for', async () => {
        const { app, fire } = makeApp({ label: 'chat-1234', sessions: ['floating-1'] });
        fire({ sessionId: 'floating-1', label: 'main' });
        await settle();
        expect(app.selectSession).not.toHaveBeenCalled();
    });

    it('does nothing when that session is already showing', async () => {
        const { app, fire } = makeApp({ sessions: ['floating-1'], active: 'floating-1' });
        fire({ sessionId: 'floating-1', label: 'main' });
        await settle();
        expect(app.selectSession).not.toHaveBeenCalled();
    });

    it('lists a launcher session that has nothing on disk yet, then opens it', async () => {
        const { app, fire } = makeApp({ sessions: ['chat-own'] });
        fire({ sessionId: 'fresh', label: 'main' });
        await settle();
        expect(app.loadSessions).toHaveBeenCalled();
        expect(app.sessions[0].session_id).toBe('fresh');
        expect(app.selectSession).toHaveBeenCalledWith('fresh', { emptyIfNew: true });
    });

    it('ignores an event without a session id', async () => {
        const { app, fire } = makeApp({ sessions: ['chat-own'] });
        fire({ label: 'main' });
        await settle();
        expect(app.selectSession).not.toHaveBeenCalled();
    });
});
