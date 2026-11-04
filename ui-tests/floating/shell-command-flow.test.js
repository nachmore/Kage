/**
 * Shell command blocks wired into the floating window, end to end: a real
 * FloatingApp's MessageStreamController fed tool updates, text chunks and
 * the turn's end, asserting on the activity slot under the response text,
 * the chip row and the settled states.
 *
 * Pins the host wiring the view tests can't see: the slot shows the running
 * command, text resuming prunes finished ones, the end of the turn (complete
 * / stop / error) settles every call and hands blocks left open into the
 * command list, and a call whose kind arrives late moves from the plain
 * chip to the shell chip instead of being listed twice.
 */
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { marked } from 'marked';
import { isShellCommandExpanded, setShellCommandExpanded } from '../../ui/js/shared/shell-command-view.js';

let FloatingApp;
beforeAll(async () => {
    globalThis.marked = marked;
    ({ FloatingApp } = await import('../../ui/js/floating/app.js'));
});
afterEach(() => {
    document.body.innerHTML = '';
});

const SID = 's1';

function makeApp() {
    document.body.innerHTML = `
        <div class="speech-bubble">
            <div class="content-area" id="contentArea">
                <div class="response-text" id="responseText"></div>
            </div>
        </div>`;
    const app = new FloatingApp(
        vi.fn(async () => null),
        {},
        vi.fn(async () => () => {})
    );
    const div = () => document.createElement('div');
    app.elements = {
        contentArea: document.getElementById('contentArea'),
        responseText: document.getElementById('responseText'),
        loadingDots: div(),
        mascotContainer: div(),
        expandBtn: div(),
        floatingStopBtn: div(),
        input: document.createElement('textarea'),
    };
    // Window chrome and layout: not under test.
    app.windowManager = { resizeWindow: vi.fn(async () => {}) };
    app.stopThinking = vi.fn();
    app.updateDatetimeVisibility = vi.fn();
    app._showFloatingResponseActions = vi.fn();
    app._showResponseActions = vi.fn();
    app._refreshContextUsage = vi.fn();
    app._windowFocused = true;
    app.floatingSessionId = SID;
    app.isWaitingForResponse = true;
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

const slot = () => document.getElementById('shellActivity');
const slotBlocks = () => [...(slot()?.querySelectorAll('.shell-cmd') ?? [])];
const row = () => document.getElementById('toolSources');
const listBlocks = () => [...row().querySelectorAll('.shell-cmd-list .shell-cmd')];
const commandText = (block) => block.querySelector('.shell-cmd-text').textContent;
const plainChips = () => [...row().querySelectorAll('.tool-sources-static .tool-chip')];

describe('floating window shell commands', () => {
    it('shows the running command, then settles every call into the chip list at the end', async () => {
        const app = makeApp();
        start(app, 'a', 'npm test');
        expect(slotBlocks()).toHaveLength(1);
        expect(slotBlocks()[0].dataset.state).toBe('running');
        // The command block replaces the generic spinner line.
        expect(app.elements.responseText.querySelector('.tool-running-indicator')).toBeNull();

        chunk(app, 'Running the tests…');
        start(app, 'b', 'sleep 100'); // never reports how it finished
        finish(app, 'a', '27 passing');
        await completeTurn(app);

        // No spinner survives the turn, the slot is gone, the list has both.
        expect(document.querySelector('.shell-cmd-spinner')).toBeNull();
        expect(slot()).toBeNull();
        expect(listBlocks().map(commandText)).toEqual(['npm test', 'sleep 100']);
        expect(listBlocks().map((b) => b.dataset.state)).toEqual(['success', 'unknown']);
        expect(document.querySelectorAll('.shell-cmd')).toHaveLength(2);
    });

    it('drops a finished, collapsed block from the slot when text resumes', () => {
        const app = makeApp();
        start(app, 'a', 'ls');
        finish(app, 'a', 'file');
        expect(slotBlocks()).toHaveLength(1);
        chunk(app, 'Here are the files.');
        expect(slotBlocks()).toHaveLength(0);
        // It's still listed under the chip.
        expect(listBlocks().map(commandText)).toEqual(['ls']);
    });

    it('moves a block expanded while running into the list, listed once and still open', async () => {
        const app = makeApp();
        chunk(app, 'Building…');
        start(app, 'a', 'cargo build');
        const live = slotBlocks()[0];
        setShellCommandExpanded(live, true);
        finish(app, 'a', 'Finished');
        chunk(app, ' done.');
        expect(slotBlocks()).toEqual([live]); // open blocks stay while streaming
        await completeTurn(app);

        expect(slot()).toBeNull();
        expect(document.querySelectorAll('.shell-cmd')).toHaveLength(1);
        expect(listBlocks()[0]).toBe(live);
        expect(isShellCommandExpanded(live)).toBe(true);
        expect(row().querySelector('.shell-cmd-list').hidden).toBe(false);
    });

    it('moves a call whose kind arrives late from the plain chip to the shell chip', async () => {
        const app = makeApp();
        chunk(app, 'Let me check.');
        tool(app, { sessionUpdate: 'tool_call', toolCallId: 'k', title: 'shell' });
        expect(plainChips().map((c) => c.textContent.replace(/\s+/g, ' ').trim())).toEqual(['🔧 shell']);

        tool(app, { sessionUpdate: 'tool_call_update', toolCallId: 'k', kind: 'execute', rawInput: { command: 'ls' } });
        expect(plainChips()).toHaveLength(0);
        expect(row().querySelectorAll('.shell-chip')).toHaveLength(1);
        finish(app, 'k', 'a\nb');
        await completeTurn(app);
        expect(plainChips()).toHaveLength(0);
        expect(listBlocks().map(commandText)).toEqual(['ls']);
    });

    it('stopping the response settles running commands', () => {
        const app = makeApp();
        chunk(app, 'Working');
        start(app, 'a', 'sleep 100');
        app.stopGenerating();
        expect(document.querySelector('.shell-cmd-spinner')).toBeNull();
        expect(listBlocks()[0].dataset.state).toBe('unknown');
    });

    it('an error ends the turn and settles running commands', async () => {
        const app = makeApp();
        app.showError = vi.fn();
        chunk(app, 'Working');
        start(app, 'a', 'sleep 100');
        await app.messageStreamController.host.onError({ payload: 'boom' }, true);
        expect(document.querySelector('.shell-cmd-spinner')).toBeNull();
        expect(listBlocks()[0].dataset.state).toBe('unknown');
    });
});
