/**
 * Cards for commands an agent asked Kage to run outside its sandbox, in a
 * chat window: as in the launcher, a card whose request is over goes when
 * the user sends their next prompt from this window; one still waiting
 * for an answer or its run stays. A turn this window only mirrors (the
 * launcher's prompt) leaves them alone.
 */
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { marked } from 'marked';
import {
    disposeHostCommandUi,
    installHostCommandUi,
    onHostCommandUpdate,
    showHostCommandPrompt,
} from '../../ui/js/shared/host-command-view.js';

let ChatApp;
beforeAll(async () => {
    globalThis.marked = marked;
    ({ ChatApp } = await import('../../ui/js/chat/app.js'));
});
afterEach(() => {
    disposeHostCommandUi();
    delete window.__TAURI__;
    document.body.innerHTML = '';
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

function makeApp() {
    // The message avatar fetches the mascot SVG.
    vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ text: async () => '<svg xmlns="http://www.w3.org/2000/svg"/>' }))
    );
    document.body.innerHTML = `
        <div class="messages-area" id="messagesArea"></div>
        <div class="host-cmd-tray" id="hostCommandTray"></div>
        <textarea id="chatInput"></textarea>`;
    const app = new ChatApp(
        vi.fn(async () => ({})),
        {},
        vi.fn(async () => () => {})
    );
    app.elements = {
        messagesArea: document.getElementById('messagesArea'),
        chatInput: document.getElementById('chatInput'),
    };
    for (const m of ['showTypingIndicator', 'updateInputState', 'scrollToBottom']) {
        app[m] = vi.fn();
    }
    app.activeSessionId = 's1';
    return app;
}

function prompt(requestId) {
    return {
        requestId,
        command: 'brew install jq',
        cwd: '/Users/me',
        reason: 'sandboxed',
        matched: true,
        plain: true,
        canAlways: false,
        guard: [],
        expiresAt: Date.now() + 300000,
    };
}

const left = (tray) =>
    Array.from(tray.querySelectorAll('.host-cmd-card')).map((c) => c.dataset.requestId);

describe('chat host-command cards', () => {
    it('go on the next prompt sent from here, and only the finished ones', async () => {
        const app = makeApp();
        window.__TAURI__ = {
            core: { invoke: vi.fn(async () => []) },
            event: { listen: vi.fn(async () => () => {}) },
        };
        const tray = document.getElementById('hostCommandTray');
        await installHostCommandUi({ tray });
        showHostCommandPrompt(prompt('hc-old'));
        onHostCommandUpdate({ requestId: 'hc-old', phase: 'closed', reason: 'expired' });
        showHostCommandPrompt(prompt('hc-waiting'));

        // A turn mirrored from the launcher: not this window's prompt.
        app.startStreaming(false);
        expect(left(tray)).toEqual(['hc-old', 'hc-waiting']);

        app.isWaitingForResponse = false;
        app.startStreaming();
        expect(left(tray)).toEqual(['hc-waiting']);
    });
});
