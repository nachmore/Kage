/**
 * The Other Agents viewer (read-only native agent sessions): a session Kage
 * drove carries Kage's hidden extension tool steering ahead of the user's
 * text. The viewer shows the user's text only, like Kage's own transcript.
 */
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { marked } from 'marked';

let AgentSessionViewer;
let TOOL_STEERING_TAG;
beforeAll(async () => {
    globalThis.marked = marked;
    ({ AgentSessionViewer } = await import('../../ui/js/chat/agent-sessions.js'));
    ({ TOOL_STEERING_TAG } = await import('../../ui/js/shared/session-render.js'));
});
afterEach(() => {
    document.body.innerHTML = '';
});

const block = () =>
    `${TOOL_STEERING_TAG}\n<extension_tools>\nquotes\n</extension_tools>\n</_kage_tool_steering>`;

function render(messages) {
    const chatApp = {
        createMessageElement(role) {
            const el = document.createElement('div');
            el.className = `message ${role}-message`;
            el.innerHTML = '<div class="message-content"></div>';
            return el;
        },
    };
    const viewer = new AgentSessionViewer(async () => [], {}, chatApp);
    const area = document.createElement('div');
    viewer.renderMessages(area, messages, { provider_id: 'claude-code', extras: {} });
    return area;
}

describe('AgentSessionViewer', () => {
    it('hides the tool steering Kage put ahead of the user text', () => {
        const area = render([
            { role: 'user', content: block() },
            { role: 'user', content: `${block()}\nWhat's AAPL at?` },
            { role: 'assistant', content: 'About 200.' },
        ]);
        const users = [...area.querySelectorAll('.user-message')];
        expect(users).toHaveLength(1);
        expect(users[0].textContent).toContain("What's AAPL at?");
        expect(area.innerHTML).not.toContain('_kage_tool_steering');
        expect(area.innerHTML).not.toContain('extension_tools');
    });

    it('leaves other user text as it is', () => {
        const viewer = new AgentSessionViewer(async () => [], {}, {});
        expect(viewer.extractUserText({ role: 'user', content: 'plain' })).toBe('plain');
    });
});
