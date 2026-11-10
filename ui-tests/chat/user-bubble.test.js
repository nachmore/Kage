/**
 * The user's bubble in the chat window: the text keeps its line breaks
 * (history and live), and a Copy button beside the time copies exactly what
 * the user typed — not the selection Kage attached, nor image previews.
 */
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { marked } from 'marked';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

let ChatApp;
let userText;
beforeAll(async () => {
    globalThis.marked = marked;
    ({ ChatApp } = await import('../../ui/js/chat/app.js'));
    ({ userText } = await import('../../ui/js/chat/app/messages.js'));
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
    document.body.innerHTML = '<div class="messages-area" id="messagesArea"></div>';
    const app = new ChatApp(vi.fn(async () => ({})), {}, vi.fn(async () => () => {}));
    app.elements = { messagesArea: document.getElementById('messagesArea') };
    return app;
}

const TYPED = 'ok, please review (keep in mind the house style)\n\nAt the launch we will:\n- do one thing\n- and another';

function stubClipboard() {
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    return writeText;
}

describe('user bubble', () => {
    it('keeps the line breaks of a prompt reopened from history', () => {
        const app = makeApp();
        const withSelection =
            '<_kage_ctx app="Notes" title="Plan"/>\nThe following text is currently selected in my active window:\n```\nsome selected text\n```\n\n' +
            TYPED;
        app.addMessageFromHistory('user', withSelection, null, null);
        const content = document.querySelector('.message.user .message-content');
        expect(content.textContent).toBe(TYPED);
    });

    it('has a Copy button beside the time that copies only what was typed', async () => {
        const app = makeApp();
        const writeText = stubClipboard();
        app.addMessageFromHistory('user', TYPED, [{ type: 'image', previewUrl: 'data:image/png;base64,AA' }], null);
        const actions = document.querySelector('.message.user .user-actions');
        const btn = actions.querySelector('[data-action="copy"]');
        expect(btn).not.toBeNull();
        expect(actions.lastElementChild.classList.contains('msg-timestamp')).toBe(true);
        btn.click();
        expect(writeText).toHaveBeenCalledWith(TYPED);
    });

    it('userText leaves out the attachment previews', () => {
        const div = document.createElement('div');
        div.textContent = 'hello\nthere';
        div.insertAdjacentHTML('beforeend', '<div class="message-attachments"><span>📄 notes.txt</span></div>');
        expect(userText(div)).toBe('hello\nthere');
    });

    it('the stylesheet keeps user line breaks and shows Copy on hover', () => {
        const css = readFileSync(path.join(repoRoot, 'ui/css/chat.css'), 'utf8');
        const rule = css.match(/\.message\.user \.message-content \{[^}]*\}/)[0];
        expect(rule).toContain('white-space: pre-wrap');
        expect(css).toMatch(/\.message\.user \.message-bubble:hover \.user-actions \.msg-action-btn/);
    });
});
