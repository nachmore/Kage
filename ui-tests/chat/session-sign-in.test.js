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
    };
    const app = new ChatApp(invoke, appWindow, vi.fn(async () => () => {}));
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
    return { app, starts, named };
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
