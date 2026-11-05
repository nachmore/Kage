/**
 * Every tool's calls in the chat window, end to end through a real
 * ChatApp: a live turn's non-shell calls end up under toggle chips in the
 * finished message (settled at the end of the turn), and a reopened
 * session (`displaySession` with kiro's log as `load_session` returns it)
 * draws the same chips and details under its replies — while Copy keeps
 * taking the reply's text only.
 */
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { marked } from 'marked';
import { isToolCallExpanded, setToolCallExpanded } from '../../ui/js/shared/tool-call-view.js';

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

const SID = 's1';

function makeApp({ streaming = true } = {}) {
    vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ text: async () => '<svg xmlns="http://www.w3.org/2000/svg"/>' }))
    );
    document.body.innerHTML = `
        <div class="messages-area" id="messagesArea"></div>
        <div id="chatHeaderTitle"></div>
        <textarea id="chatInput"></textarea>`;
    const app = new ChatApp(
        vi.fn(async () => ({})),
        {},
        vi.fn(async () => () => {})
    );
    app.elements = {
        messagesArea: document.getElementById('messagesArea'),
        chatInput: document.getElementById('chatInput'),
        chatHeaderTitle: document.getElementById('chatHeaderTitle'),
    };
    for (const m of [
        'showTypingIndicator',
        'hideTypingIndicator',
        'updateInputState',
        'scrollToBottom',
        'loadSessions',
        'loadFloatingSessionId',
        'showSuggestionChips',
        'updateConnectionStatus',
    ]) {
        app[m] = vi.fn();
    }
    app._windowFocused = true;
    app.activeSessionId = SID;
    app.sessions = [];
    if (streaming) app.startStreaming();
    return app;
}

const tool = (app, update) =>
    app.messageStreamController.handleToolCallUpdate({
        payload: { params: { sessionId: SID, update } },
    });
const chunk = (app, text) =>
    app.messageStreamController.handleChunk({ payload: { text, sessionId: SID } });

const chipsIn = (root) => [...root.querySelectorAll('.tool-call-chip')];
const label = (chip) => chip.querySelector('.tool-call-chip-label').textContent;
const listOf = (chip) => document.getElementById(chip.getAttribute('aria-controls'));

describe('chat window: live tool calls', () => {
    it("lists the turn's calls under toggle chips in the finished message, settled", async () => {
        const app = makeApp();
        chunk(app, 'Checking.');
        tool(app, { sessionUpdate: 'tool_call', toolCallId: 'r', title: 'read', kind: 'read', status: 'in_progress', rawInput: { path: '/a' } });
        tool(app, { sessionUpdate: 'tool_call_update', toolCallId: 'r', status: 'completed', rawOutput: { items: [{ Text: 'x' }] } });
        tool(app, { sessionUpdate: 'tool_call', toolCallId: 'm', title: 'Running: @builder-mcp/InternalSearch', status: 'in_progress' });
        chunk(app, ' Done.');
        await app.messageStreamController.handleComplete();

        const msg = document.querySelector('.message.assistant');
        const chips = chipsIn(msg);
        expect(chips.map(label)).toEqual(['read', 'builder-mcp']);
        chips[1].click();
        const block = listOf(chips[1]).querySelector('.tool-call');
        expect(block.dataset.state).toBe('unknown');
        expect(document.querySelector('.shell-cmd-spinner')).toBeNull();
    });
});

// --- Reopened sessions -------------------------------------------------------

const prompt = (id, text) => ({ kind: 'Prompt', message_id: id, content: [{ kind: 'text', data: text }] });
const assistant = (id, ...content) => ({ kind: 'AssistantMessage', message_id: id, content });

function sessionData() {
    return {
        session_id: SID,
        created_at: '',
        updated_at: '',
        message_timestamps: {},
        message_durations: {},
        messages: [
            prompt('p1', 'What changed?'),
            assistant(
                'a1',
                { kind: 'text', data: 'Let me look.' },
                { kind: 'toolUse', data: { toolUseId: 'w', name: 'write', input: { command: 'str_replace', path: '/repo/x.txt', oldStr: 'old', newStr: 'new' } } },
                { kind: 'toolUse', data: { toolUseId: 'm', name: 'InternalSearch', input: { query: 'SECRET-QUERY' } } },
                { kind: 'toolUse', data: { toolUseId: 's', name: 'shell', input: { command: 'cat ~/.aws/credentials' } } }
            ),
            {
                kind: 'ToolResults',
                message_id: 'r1',
                content: [
                    { kind: 'toolResult', data: { toolUseId: 'w', status: 'success', content: [{ kind: 'text', data: 'ok' }] } },
                    {
                        kind: 'toolResult',
                        data: {
                            toolUseId: 'm',
                            status: 'error',
                            content: [{ kind: 'json', data: { content: [{ type: 'text', text: 'denied' }], isError: true } }],
                        },
                    },
                    {
                        kind: 'toolResult',
                        data: {
                            toolUseId: 's',
                            status: 'success',
                            content: [{ kind: 'json', data: { exit_status: 'exit status: 0', stdout: 'aws_secret_access_key = SECRET123', stderr: '' } }],
                        },
                    },
                ],
                tool_kinds: { m: { mcp_server: 'builder-mcp', mcp_tool: 'InternalSearch' }, s: { builtin: 'ExecuteCmd' } },
            },
            assistant('a2', { kind: 'text', data: 'One line changed.' }),
        ],
    };
}

describe('chat window: reopened sessions', () => {
    it('shows the same tool chips and expandable details under the reply', async () => {
        const app = makeApp({ streaming: false });
        await app.displaySession(sessionData());
        const replies = [...document.querySelectorAll('.message.assistant')];
        expect(replies).toHaveLength(2);
        // Chips go under the turn's last reply, inside its content.
        expect(replies[0].querySelector('.tool-sources')).toBeNull();
        const row = replies[1].querySelector('.message-content > .tool-sources');
        expect(row).not.toBeNull();
        const chips = chipsIn(row);
        expect(chips.map(label)).toEqual(['write', 'builder-mcp']);
        expect(chips[1].dataset.state).toBe('failed');
        expect(row.querySelector('.shell-chip')).not.toBeNull();

        // Expand the edit: its diff is there.
        chips[0].click();
        const edit = listOf(chips[0]).querySelector('.tool-call');
        setToolCallExpanded(edit, true);
        expect(isToolCallExpanded(edit)).toBe(true);
        expect([...edit.querySelectorAll('.tool-diff-line')].map((l) => l.dataset.op)).toEqual(['del', 'add']);

        // Shell output from the log.
        row.querySelector('.shell-chip').click();
        const sh = row.querySelector('.shell-cmd');
        sh.querySelector('.shell-cmd-header').click();
        expect(sh.querySelector('.shell-cmd-stdout').textContent).toContain('SECRET123');
    });

    it('Copy on a reopened reply takes the reply only, not tool data', async () => {
        const app = makeApp({ streaming: false });
        const writeText = vi.fn(() => Promise.resolve());
        Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
        await app.displaySession(sessionData());
        const reply = [...document.querySelectorAll('.message.assistant')][1];
        for (const chip of chipsIn(reply)) chip.click();
        reply.querySelector('[data-action="copy"]').click();
        const copied = writeText.mock.calls[0][0];
        expect(copied).toContain('One line changed.');
        for (const s of ['SECRET-QUERY', 'SECRET123', 'builder-mcp', 'write', 'credentials']) {
            expect(copied).not.toContain(s);
        }
        // History messages kept for export are text only.
        expect(app.messages.map((m) => m.content)).toEqual(['What changed?', 'Let me look.', 'One line changed.']);
    });
});
