/**
 * Shell command blocks wired into the chat window, end to end: a real
 * ChatApp's MessageStreamController fed tool updates, text chunks and the
 * turn's end, asserting on the live slot next to the streaming message, the
 * chip row rendered into the finished message, and the message actions.
 *
 * Pins the host wiring the view tests can't see: the slot shows the running
 * command, the end of the turn (complete / stop) settles every call and
 * hands blocks left open into the message's command list, and Copy / Speak
 * / Translate take the reply only — never the commands or their output.
 */
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { marked } from 'marked';
import { isShellCommandExpanded, setShellCommandExpanded } from '../../ui/js/shared/shell-command-view.js';
import { buildHistoryTools } from '../../ui/js/shared/tool-call-history.js';
import { _resetShellRunsForTests } from '../../ui/js/shared/shell-run-view.js';

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

function makeApp() {
    // The message avatar fetches the mascot SVG.
    vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ text: async () => '<svg xmlns="http://www.w3.org/2000/svg"/>' }))
    );
    document.body.innerHTML = `
        <div class="messages-area" id="messagesArea"></div>
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
    // Window chrome, sidebar and layout: not under test.
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
    app.startStreaming();
    return app;
}

const tool = (app, update) =>
    app.messageStreamController.handleToolCallUpdate({
        payload: { params: { sessionId: SID, update } },
    });
const chunk = (app, text) =>
    app.messageStreamController.handleChunk({ payload: { text, sessionId: SID } });
const completeTurn = (app) => app.messageStreamController.handleComplete();

const start = (app, id, command) =>
    tool(app, {
        sessionUpdate: 'tool_call',
        toolCallId: id,
        title: 'shell',
        kind: 'execute',
        status: 'in_progress',
        rawInput: { command },
    });
const finish = (app, id, stdout, exit = 0) =>
    tool(app, {
        sessionUpdate: 'tool_call_update',
        toolCallId: id,
        status: 'completed',
        rawOutput: { items: [{ Json: { exit_status: `exit status: ${exit}`, stdout, stderr: '' } }] },
    });

const message = () => document.querySelector('.message.assistant');
const slot = () => message().querySelector('.shell-activity');
const listBlocks = () => [...message().querySelectorAll('.message-content .shell-cmd-list .shell-cmd')];
const commandText = (block) => block.querySelector('.shell-cmd-text').textContent;

describe('chat window shell commands', () => {
    it('shows the running command next to the message, then lists every call in it', async () => {
        const app = makeApp();
        chunk(app, 'Running the tests…');
        start(app, 'a', 'npm test');
        expect(slot().querySelectorAll('.shell-cmd')).toHaveLength(1);
        // The slot lives outside the markdown target, which re-renders per chunk.
        expect(slot().parentElement.querySelector(':scope > .message-content')).not.toBeNull();
        start(app, 'b', 'sleep 100'); // never reports how it finished
        finish(app, 'a', '27 passing');
        chunk(app, ' All green.');
        await completeTurn(app);

        expect(document.querySelector('.shell-cmd-spinner')).toBeNull();
        expect(slot()).toBeNull();
        expect(listBlocks().map(commandText)).toEqual(['npm test', 'sleep 100']);
        expect(listBlocks().map((b) => b.dataset.state)).toEqual(['success', 'unknown']);
    });

    it('moves a block expanded while running into the message, listed once and still open', async () => {
        const app = makeApp();
        chunk(app, 'Building…');
        start(app, 'a', 'cargo build');
        const live = slot().querySelector('.shell-cmd');
        setShellCommandExpanded(live, true);
        finish(app, 'a', 'Finished');
        await completeTurn(app);

        expect(slot()).toBeNull();
        expect(document.querySelectorAll('.shell-cmd')).toHaveLength(1);
        expect(listBlocks()[0]).toBe(live);
        expect(isShellCommandExpanded(live)).toBe(true);
    });

    it("keeps a command the agent's sandbox blocked in view, then lists it open in the message", async () => {
        const app = makeApp();
        chunk(app, 'Installing…');
        start(app, 'a', 'brew install --cask betterdisplay');
        tool(app, {
            sessionUpdate: 'tool_call_update',
            toolCallId: 'a',
            status: 'completed',
            rawOutput: {
                items: [
                    {
                        Json: {
                            exit_status: 'exit status: 1',
                            stdout: '',
                            stderr: 'sandbox-exec: sandbox_apply: Operation not permitted\nError: betterdisplay: Download failed',
                        },
                    },
                ],
            },
        });
        const live = slot().querySelector('.shell-cmd');
        expect(live.querySelector('.shell-cmd-sandbox')).not.toBeNull();
        chunk(app, ' The install was blocked.');
        expect(slot().querySelector('.shell-cmd')).toBe(live);
        await completeTurn(app);

        expect(slot()).toBeNull();
        expect(listBlocks()).toEqual([live]);
        expect(message().querySelector('.shell-cmd-list').hidden).toBe(false);
        expect(document.querySelectorAll('.shell-cmd-sandbox')).toHaveLength(1);
    });

    it('a reopened session shows the same notice, its list open', () => {
        const app = makeApp();
        const { tools } = buildHistoryTools(
            [{ toolUseId: 'h1', name: 'shell', input: { command: 'brew install --cask betterdisplay' } }],
            new Map([
                [
                    'h1',
                    {
                        toolUseId: 'h1',
                        status: 'error',
                        content: [
                            {
                                kind: 'json',
                                data: {
                                    exit_status: 'exit status: 1',
                                    stdout: '',
                                    stderr: 'sandbox-exec: sandbox_apply: Operation not permitted',
                                },
                            },
                        ],
                    },
                ],
            ]),
            new Map([['h1', { builtin: 'ExecuteCmd' }]])
        );
        document.querySelector('.message.assistant')?.remove();
        app.addMessageFromHistory('assistant', 'It was blocked.', null, null, tools);
        expect(message().querySelector('.shell-cmd-list').hidden).toBe(false);
        expect(listBlocks()).toHaveLength(1);
        expect(listBlocks()[0].querySelector('.shell-cmd-sandbox')).not.toBeNull();
    });

    it('stopping the response settles running commands', () => {
        const app = makeApp();
        chunk(app, 'Working');
        start(app, 'a', 'sleep 100');
        app.stopGenerating();
        expect(document.querySelector('.shell-cmd-spinner')).toBeNull();
        expect(listBlocks()[0].dataset.state).toBe('unknown');
    });

    it('Copy, Speak and Translate take the reply, not the commands or their output', async () => {
        const app = makeApp();
        const writeText = vi.fn(() => Promise.resolve());
        Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
        chunk(app, 'Here is the summary.');
        start(app, 'a', 'cat ~/.aws/credentials');
        finish(app, 'a', 'aws_secret_access_key = SECRET123');
        await completeTurn(app);
        const speakText = vi.fn();
        app.speech = { cancelSpeech: vi.fn(), speakText };
        // The user opens and closes the command once.
        message().querySelector('.shell-chip').click();
        const block = listBlocks()[0];
        setShellCommandExpanded(block, true);
        setShellCommandExpanded(block, false);

        const action = (name) => message().querySelector(`[data-action="${name}"]`);
        action('copy').click();
        expect(writeText).toHaveBeenCalledWith('Here is the summary.\n');
        action('speak').click();
        expect(speakText).toHaveBeenCalledWith('Here is the summary.\n');
        await action('translate').onclick();
        const prompt = app.elements.chatInput.value;
        expect(prompt).toContain('Here is the summary.');
        for (const text of [writeText.mock.calls[0][0], speakText.mock.calls[0][0], prompt]) {
            expect(text).not.toContain('credentials');
            expect(text).not.toContain('SECRET123');
            expect(text).not.toContain('shell');
        }
    });
});

describe('chat window: running a blocked command yourself', () => {
    const flush = () => new Promise((r) => setTimeout(r, 0));

    function blockedTurn(app, command) {
        start(app, 'a', command);
        tool(app, {
            sessionUpdate: 'tool_call_update',
            toolCallId: 'a',
            status: 'failed',
            rawOutput: {
                items: [
                    {
                        Json: {
                            exit_status: 'exit status: 1',
                            stdout: '',
                            stderr: 'sandbox-exec: sandbox_apply: Operation not permitted',
                        },
                    },
                ],
            },
        });
        return completeTurn(app);
    }

    function fakeBackend() {
        const runs = [];
        const invoke = vi.fn((cmd, args) => {
            if (cmd === 'user_shell_run_context') {
                return Promise.resolve({ cwd: '/home/me', shell: 'zsh', terminal: false });
            }
            if (cmd === 'run_user_shell_command') {
                return new Promise((resolve) => runs.push({ args, resolve }));
            }
            return Promise.resolve(null);
        });
        window.__TAURI__ = { core: { invoke }, event: { listen: vi.fn(async () => () => {}) } };
        return runs;
    }

    afterEach(() => {
        _resetShellRunsForTests();
        delete window.__TAURI__;
    });

    it('tells the agent as the next prompt, once the turn in progress has ended', async () => {
        const app = makeApp();
        const runs = fakeBackend();
        app.elements.chatInput.value = 'a draft the user is typing';
        await blockedTurn(app, 'brew install awscli');
        const block = listBlocks()[0];
        block.querySelector('.shell-run-here').click();
        await flush();
        await flush();
        block.querySelector('.shell-run-confirm .shell-run-go').click();
        await flush();
        expect(runs).toHaveLength(1);
        expect(runs[0].args).toMatchObject({ command: 'brew install awscli', sessionId: SID });

        // The user asked something else meanwhile: the result waits for it.
        app.isWaitingForResponse = true;
        runs[0].resolve({
            outcome: 'exited',
            exitCode: 0,
            signal: null,
            output: '🍺 awscli installed\n',
            truncated: false,
        });
        await flush();
        await flush();
        expect(app.invoke).not.toHaveBeenCalledWith('send_message_streaming', expect.anything());
        expect(block.querySelector('.shell-run-agent-text').textContent).toBe(
            'shared.shell.sandbox.agent.queued'
        );

        app.isWaitingForResponse = false;
        await new Promise((r) => setTimeout(r, 650));
        const sends = app.invoke.mock.calls.filter(([c]) => c === 'send_message_streaming');
        expect(sends).toHaveLength(1);
        const { sessionId, message: text, attachments } = sends[0][1];
        expect(sessionId).toBe(SID);
        expect(attachments).toBeNull();
        expect(text).toContain('I ran this outside your sandbox: `brew install awscli` — exit code 0');
        expect(text).toContain('🍺 awscli installed');
        // Shown as the user's message; the draft in the composer is untouched.
        const bubbles = [...document.querySelectorAll('.message.user')];
        expect(bubbles.at(-1).textContent).toContain('I ran this outside your sandbox');
        expect(app.elements.chatInput.value).toBe('a draft the user is typing');
        expect(app.isWaitingForResponse).toBe(true);
        expect(block.querySelector('.shell-run-agent-text').textContent).toBe(
            'shared.shell.sandbox.agent.sent'
        );
    });

    it('sends only to the session in view, when no turn is in flight', () => {
        const app = makeApp();
        app.isWaitingForResponse = false;
        expect(app.canSendAgentPrompt(SID)).toBe(true);
        expect(app.canSendAgentPrompt('other')).toBe(false);
        app.isWaitingForResponse = true;
        expect(app.canSendAgentPrompt(SID)).toBe(false);
    });
});
