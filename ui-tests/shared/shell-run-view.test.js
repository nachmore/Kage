/**
 * "Run it here" / "Run in Terminal" on the sandbox notice: a click opens a
 * confirm with the whole command, where it runs and what's risky — and
 * runs nothing; only the confirm's Run does, once per click; Cancel and
 * Escape run nothing. While it runs: output, elapsed time, Stop. After:
 * exit status, and the result told to the agent (queued while it's busy)
 * when the user left "Tell the agent the result" on.
 *
 * Strings are i18n keys (no catalog loaded), so copy changes don't break
 * these.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createShellCommandBlock } from '../../ui/js/shared/shell-command-view.js';
import {
    _resetShellRunsForTests,
    isShellRunActive,
    setShellRunHost,
} from '../../ui/js/shared/shell-run-view.js';
import { processToolCallUpdate } from '../../ui/js/shared/streaming-utils.js';

const SANDBOXED = 'sandbox-exec: sandbox_apply: Operation not permitted';
const flush = () => new Promise((r) => setTimeout(r, 0));

function blockedRecord(command, rawInput = {}) {
    const state = { toolUsages: [], toolSources: [] };
    const evt = (update) => ({ payload: { params: { sessionId: 's1', update } } });
    processToolCallUpdate(
        evt({
            sessionUpdate: 'tool_call',
            toolCallId: 'call-1',
            title: 'shell',
            kind: 'execute',
            status: 'in_progress',
            rawInput: { command, ...rawInput },
        }),
        state
    );
    processToolCallUpdate(
        evt({
            toolCallId: 'call-1',
            status: 'failed',
            rawOutput: {
                items: [{ Json: { exit_status: 'exit status: 1', stdout: '', stderr: SANDBOXED } }],
            },
        }),
        state
    );
    return state.toolUsages[0].shell;
}

/** A Tauri stand-in: records invokes; runs resolve when the test says. */
function fakeTauri({ ctx = { cwd: '/home/me', shell: 'zsh', terminal: false } } = {}) {
    const calls = [];
    const runs = [];
    let outputListener = null;
    const invoke = vi.fn((cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'user_shell_run_context') {
            return ctx instanceof Error ? Promise.reject(ctx) : Promise.resolve(ctx);
        }
        if (cmd === 'run_user_shell_command') {
            return new Promise((resolve, reject) => runs.push({ args, resolve, reject }));
        }
        if (cmd === 'cancel_user_shell_command') return Promise.resolve(true);
        return Promise.resolve(null);
    });
    const listen = vi.fn(async (event, cb) => {
        outputListener = { event, cb };
        return () => {};
    });
    window.__TAURI__ = { core: { invoke }, event: { listen } };
    return {
        invoke,
        calls,
        runs,
        commands: () => calls.map(([c]) => c),
        emit: (payload) => outputListener?.cb({ payload }),
        listenedTo: () => outputListener?.event,
    };
}

const q = (block, sel) => block.querySelector(sel);
const runHereBtn = (block) => q(block, '.shell-run-here');
const confirmBox = (block) => q(block, '.shell-run-confirm');
const confirmRun = (block) => q(block, '.shell-run-confirm .shell-run-go');
const confirmCancel = (block) => q(block, '.shell-run-confirm .shell-run-cancel');

async function openConfirm(block) {
    runHereBtn(block).click();
    await flush();
    await flush();
    return confirmBox(block);
}

function doneResult(extra = {}) {
    return {
        runId: 'x',
        cwd: '/home/me',
        shell: 'zsh',
        outcome: 'exited',
        exitCode: 0,
        signal: null,
        durationMs: 1200,
        output: '==> Pouring awscli\n',
        truncated: false,
        outputBytes: 19,
        ...extra,
    };
}

let writeText;
beforeEach(() => {
    writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
});
afterEach(() => {
    _resetShellRunsForTests();
    delete window.__TAURI__;
    document.body.innerHTML = '';
    vi.useRealTimers();
});

describe('the confirm', () => {
    it('opens on "Run it here" and runs nothing', async () => {
        const tauri = fakeTauri();
        const cmd = 'brew install awscli && echo "done" # really';
        const block = createShellCommandBlock(blockedRecord(cmd));
        document.body.append(block);
        expect(runHereBtn(block).hidden).toBe(false);
        const box = await openConfirm(block);
        expect(box).not.toBeNull();
        // Only the context lookup — no run.
        expect(tauri.commands()).toEqual(['user_shell_run_context']);
        expect(tauri.calls[0][1]).toEqual({ cwd: null, shell: null });
        // The whole command, exactly as it would run.
        expect(q(box, '.shell-run-confirm-command-text').textContent).toBe(cmd);
        expect(q(box, '.shell-run-confirm-path').textContent).toBe('/home/me');
        expect(q(box, '.shell-run-confirm-where').textContent).toContain(
            'shared.shell.sandbox.confirm.shell'
        );
        expect(q(box, '.shell-run-confirm-warn').textContent).toBe(
            'shared.shell.sandbox.confirm.warning'
        );
        expect(q(box, '.shell-run-confirm-risks').hidden).toBe(true);
        expect(confirmRun(block).disabled).toBe(false);
        // Focus starts on Cancel, and the trigger can't open a second one.
        expect(document.activeElement).toBe(confirmCancel(block));
        expect(runHereBtn(block).disabled).toBe(true);
        expect(isShellRunActive()).toBe(false);
    });

    it('runs nothing on Cancel or Escape', async () => {
        const tauri = fakeTauri();
        const block = createShellCommandBlock(blockedRecord('brew install awscli'));
        document.body.append(block);
        await openConfirm(block);
        confirmCancel(block).click();
        expect(confirmBox(block)).toBeNull();
        expect(document.activeElement).toBe(runHereBtn(block));
        expect(runHereBtn(block).disabled).toBe(false);

        const box = await openConfirm(block);
        box.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        expect(confirmBox(block)).toBeNull();
        expect(tauri.commands()).not.toContain('run_user_shell_command');
    });

    it('runs the exact command once per Run click, from the directory it showed', async () => {
        const tauri = fakeTauri({ ctx: { cwd: '/repo', shell: 'bash', terminal: false } });
        const cmd = "brew install awscli 2>&1 | tail -20";
        const block = createShellCommandBlock(blockedRecord(cmd, { cwd: '/repo' }));
        document.body.append(block);
        await openConfirm(block);
        expect(tauri.calls[0][1]).toEqual({ cwd: '/repo', shell: null });
        const run = confirmRun(block);
        run.click();
        run.click(); // a second click on the same confirm runs nothing more
        await flush();
        const runCalls = tauri.calls.filter(([c]) => c === 'run_user_shell_command');
        expect(runCalls).toHaveLength(1);
        expect(runCalls[0][1]).toMatchObject({ command: cmd, cwd: '/repo', shell: null });
        // Random, not a per-window counter: output events reach every
        // listening window, so ids must not collide across windows.
        expect(runCalls[0][1].runId).toMatch(/^kage-run-[0-9a-f]{24}$/);
        expect(confirmBox(block)).toBeNull();
        expect(isShellRunActive()).toBe(true);
        expect(tauri.listenedTo()).toBe('user_shell_output');
    });

    it('ignores the rest of a double-click that lands on Run', async () => {
        const tauri = fakeTauri();
        const block = createShellCommandBlock(blockedRecord('brew install awscli'));
        document.body.append(block);
        await openConfirm(block);
        const run = confirmRun(block);
        // The confirm opened under the pointer: a second (and third) click
        // of the same multi-click on "Run it here" runs nothing.
        run.dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 2 }));
        run.dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 3 }));
        await flush();
        expect(tauri.commands()).not.toContain('run_user_shell_command');
        expect(confirmBox(block)).not.toBeNull();
        // A click of its own does.
        run.dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        expect(tauri.commands().filter((c) => c === 'run_user_shell_command')).toHaveLength(1);
    });

    it('runs in the shell the confirm was opened for', async () => {
        const tauri = fakeTauri({ ctx: { cwd: 'C:\\Users\\me', shell: 'cmd', terminal: false } });
        const rec = blockedRecord('dir /b');
        rec.title = 'execute_cmd';
        const block = createShellCommandBlock(rec);
        document.body.append(block);
        await openConfirm(block);
        expect(tauri.calls[0][1]).toEqual({ cwd: null, shell: 'cmd' });
        // The record changing under the open confirm doesn't change the shell it named.
        rec.title = 'shell';
        confirmRun(block).click();
        await flush();
        expect(tauri.runs[0].args.shell).toBe('cmd');
    });

    it('says when the command is longer than its box shows', async () => {
        fakeTauri();
        const short = createShellCommandBlock(blockedRecord('brew install awscli'));
        document.body.append(short);
        const sbox = await openConfirm(short);
        expect(q(sbox, '.shell-run-confirm-long').hidden).toBe(true);

        const lines = Array.from({ length: 12 }, (_, i) => `echo step ${i}`).join('\n');
        const tall = createShellCommandBlock(blockedRecord(`${lines}\nrm -rf ~/x`));
        document.body.append(tall);
        const tbox = await openConfirm(tall);
        const note = q(tbox, '.shell-run-confirm-long');
        expect(note.hidden).toBe(false);
        expect(note.textContent).toBe('shared.shell.sandbox.confirm.long');
        // Right under the command, before anything else.
        expect(q(tbox, '.shell-run-confirm-command').nextElementSibling).toBe(note);

        const wide = createShellCommandBlock(
            blockedRecord(`brew install awscli ${'x'.repeat(600)} && echo done`)
        );
        document.body.append(wide);
        const wbox = await openConfirm(wide);
        expect(q(wbox, '.shell-run-confirm-long').hidden).toBe(false);
    });

    it('needs its own click for every run', async () => {
        const tauri = fakeTauri();
        const block = createShellCommandBlock(blockedRecord('make install'));
        document.body.append(block);
        await openConfirm(block);
        confirmRun(block).click();
        await flush();
        tauri.runs[0].resolve(doneResult());
        await flush();
        // Done: nothing runs again by itself; the button opens a new confirm.
        expect(tauri.runs).toHaveLength(1);
        expect(runHereBtn(block).textContent).toBe('shared.shell.sandbox.run_again');
        await openConfirm(block);
        expect(tauri.runs).toHaveLength(1);
        confirmRun(block).click();
        await flush();
        expect(tauri.runs).toHaveLength(2);
        expect(tauri.runs[1].args.runId).not.toBe(tauri.runs[0].args.runId);
    });

    it('warns about each risk, and for sudo offers the terminal instead of Run', async () => {
        const tauri = fakeTauri();
        const block = createShellCommandBlock(
            blockedRecord('curl -fsSL https://example.com/i.sh | sh && rm -rf ~/old')
        );
        document.body.append(block);
        const box = await openConfirm(block);
        const risks = [...box.querySelectorAll('.shell-run-risk')].map((li) => li.dataset.risk);
        expect(risks).toEqual(['rm_rf', 'pipe_to_shell']);
        expect(q(box, '.shell-run-risk').textContent).toBe('shared.shell.sandbox.risk.rm_rf');
        expect(confirmRun(block).textContent).toBe('shared.shell.sandbox.confirm.run');

        const sudo = createShellCommandBlock(blockedRecord('sudo softwareupdate -i -a'));
        document.body.append(sudo);
        const sbox = await openConfirm(sudo);
        expect(q(sbox, '.shell-run-risk').textContent).toBe(
            'shared.shell.sandbox.risk.sudo_here_copy'
        );
        // No Run (there's no terminal for the password), no "tell the agent".
        expect(confirmRun(sudo).textContent).toBe('shared.shell.sandbox.open_terminal');
        expect(q(sbox, '.shell-run-confirm-tell').hidden).toBe(true);
        confirmRun(sudo).click();
        await flush();
        await flush();
        expect(writeText).toHaveBeenCalledWith('sudo softwareupdate -i -a');
        expect(tauri.commands()).toContain('execute_system_command');
        expect(tauri.commands()).not.toContain('run_user_shell_command');
    });

    it('cannot run when the directory is unusable or another command is running', async () => {
        fakeTauri({ ctx: Object.assign(new Error('x'), { kind: 'internal', message: 'gone' }) });
        const block = createShellCommandBlock(blockedRecord('make', { cwd: '/gone' }));
        document.body.append(block);
        const box = await openConfirm(block);
        expect(q(box, '.shell-run-confirm-error').textContent).toBe(
            'shared.shell.sandbox.confirm.cwd_error'
        );
        expect(confirmRun(block).disabled).toBe(true);
        confirmRun(block).click();
        await flush();
        expect(window.__TAURI__.core.invoke).not.toHaveBeenCalledWith(
            'run_user_shell_command',
            expect.anything()
        );
    });

    it('is withheld for a command with hidden characters', async () => {
        const tauri = fakeTauri();
        const block = createShellCommandBlock(blockedRecord('brew install x‮'));
        document.body.append(block);
        expect(runHereBtn(block).hidden).toBe(true);
        expect(q(block, '.shell-run-terminal').hidden).toBe(true);
        runHereBtn(block).click();
        await flush();
        expect(confirmBox(block)).toBeNull();
        expect(tauri.invoke).not.toHaveBeenCalled();
    });

    it('closes when the command it shows changes', async () => {
        fakeTauri();
        const rec = blockedRecord('make');
        const block = createShellCommandBlock(rec);
        document.body.append(block);
        await openConfirm(block);
        rec.command = 'make clean';
        rec.rev += 1;
        // Re-render through the public path.
        const { updateShellCommandBlock } = await import(
            '../../ui/js/shared/shell-command-view.js'
        );
        updateShellCommandBlock(block, rec);
        expect(confirmBox(block)).toBeNull();
    });
});

describe('a run', () => {
    it('shows output as it streams, the time taken, and Stop', async () => {
        vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'Date'] });
        const tauri = fakeTauri();
        const block = createShellCommandBlock(blockedRecord('brew install awscli'));
        document.body.append(block);
        await openConfirm(block);
        confirmRun(block).click();
        await flush();
        const live = q(block, '.shell-run-live');
        expect(live.dataset.state).toBe('running');
        expect(q(live, '.shell-run-state').textContent).toBe('shared.shell.sandbox.run.running');
        expect(q(live, '.shell-run-output').textContent).toBe(
            'shared.shell.sandbox.run.no_output_yet'
        );
        const runId = tauri.runs[0].args.runId;
        tauri.emit({ runId, text: '\x1b[1m==> Downloading\x1b[0m\n', skipped: 0 });
        tauri.emit({ runId: 'someone-else', text: 'not ours\n', skipped: 0 });
        expect(q(live, '.shell-run-output').textContent).toBe('==> Downloading\n');
        vi.advanceTimersByTime(3000);
        expect(q(live, '.shell-run-elapsed').textContent).toBe('0:03');

        const stop = q(live, '.shell-run-stop');
        expect(stop.hidden).toBe(false);
        stop.click();
        await flush();
        expect(tauri.calls.find(([c]) => c === 'cancel_user_shell_command')[1]).toEqual({ runId });
        expect(q(live, '.shell-run-state').textContent).toBe('shared.shell.sandbox.run.stopping');

        tauri.runs[0].resolve(doneResult({ outcome: 'cancelled', exitCode: null, signal: 15 }));
        await flush();
        expect(q(block, '.shell-run-state').textContent).toBe('shared.shell.sandbox.run.cancelled');
        expect(q(block, '.shell-run-stop').hidden).toBe(true);
        expect(isShellRunActive()).toBe(false);
    });

    it('reports the exit status, or why it could not run', async () => {
        const tauri = fakeTauri();
        const block = createShellCommandBlock(blockedRecord('make'));
        document.body.append(block);
        await openConfirm(block);
        confirmRun(block).click();
        await flush();
        tauri.runs[0].resolve(doneResult({ exitCode: 2, output: 'make: *** Error 2\n' }));
        await flush();
        expect(q(block, '.shell-run-live').dataset.state).toBe('failed');
        expect(q(block, '.shell-run-state').textContent).toBe('shared.shell.sandbox.run.exit_code');
        expect(q(block, '.shell-run-output').textContent).toBe('make: *** Error 2\n');

        await openConfirm(block);
        confirmRun(block).click();
        await flush();
        tauri.runs[1].reject({ kind: 'internal', message: 'Another command is already running' });
        await flush();
        expect(q(block, '.shell-run-state').textContent).toBe('shared.shell.sandbox.run.failed');
    });

    it('stays with its record when the block is rebuilt', async () => {
        const tauri = fakeTauri();
        const rec = blockedRecord('make');
        const block = createShellCommandBlock(rec);
        document.body.append(block);
        await openConfirm(block);
        confirmRun(block).click();
        await flush();
        const twin = createShellCommandBlock(rec);
        document.body.append(twin);
        expect(q(twin, '.shell-run-live').dataset.state).toBe('running');
        expect(q(twin, '.shell-run-here').disabled).toBe(true);
        tauri.emit({ runId: tauri.runs[0].args.runId, text: 'hello\n', skipped: 0 });
        expect(q(twin, '.shell-run-output').textContent).toBe('hello\n');
        expect(q(block, '.shell-run-output').textContent).toBe('hello\n');
    });
});

describe('telling the agent', () => {
    function host({ busy = () => false } = {}) {
        const sent = [];
        const h = {
            sessionId: () => 's1',
            canSend: (sid) => sid === 's1' && !busy(),
            send: vi.fn(async (sid, text) => {
                sent.push({ sid, text });
            }),
        };
        setShellRunHost(h);
        return { h, sent };
    }

    async function runToEnd(block, tauri, result, { tell = true } = {}) {
        const box = await openConfirm(block);
        const checkbox = q(box, '.shell-run-confirm-tell input');
        expect(checkbox.checked).toBe(true); // on by default
        if (!tell) checkbox.click();
        confirmRun(block).click();
        await flush();
        tauri.runs.at(-1).resolve(result);
        await flush();
        await flush();
    }

    it('sends the result as the next prompt when the agent is free', async () => {
        const tauri = fakeTauri();
        const { sent } = host();
        const block = createShellCommandBlock(blockedRecord('brew install awscli'));
        document.body.append(block);
        await runToEnd(block, tauri, doneResult());
        expect(tauri.runs[0].args.sessionId).toBe('s1');
        expect(sent).toHaveLength(1);
        expect(sent[0].sid).toBe('s1');
        expect(sent[0].text).toContain(
            'I ran this outside your sandbox: `brew install awscli` — exit code 0'
        );
        expect(sent[0].text).toContain('==> Pouring awscli');
        expect(q(block, '.shell-run-agent-text').textContent).toBe('shared.shell.sandbox.agent.sent');
    });

    it('holds it while the agent is busy, and can drop it', async () => {
        vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'], shouldAdvanceTime: true });
        const tauri = fakeTauri();
        let busy = true;
        const { sent } = host({ busy: () => busy });
        const block = createShellCommandBlock(blockedRecord('brew install awscli'));
        document.body.append(block);
        await runToEnd(block, tauri, doneResult());
        expect(sent).toHaveLength(0);
        expect(q(block, '.shell-run-agent-text').textContent).toBe(
            'shared.shell.sandbox.agent.queued'
        );
        busy = false;
        await vi.advanceTimersByTimeAsync(600);
        expect(sent).toHaveLength(1);
        expect(q(block, '.shell-run-agent-text').textContent).toBe('shared.shell.sandbox.agent.sent');

        // Again, but dropped while waiting.
        busy = true;
        await runToEnd(block, tauri, doneResult({ exitCode: 1 }));
        q(block, '.shell-run-dont-tell').click();
        busy = false;
        await vi.advanceTimersByTimeAsync(1000);
        expect(sent).toHaveLength(1);
        expect(q(block, '.shell-run-agent-text').textContent).toBe(
            'shared.shell.sandbox.agent.not_told'
        );
    });

    it('stays quiet when unchecked or stopped, until "Tell the agent" is clicked', async () => {
        const tauri = fakeTauri();
        const { sent } = host();
        const block = createShellCommandBlock(blockedRecord('make'));
        document.body.append(block);
        await runToEnd(block, tauri, doneResult(), { tell: false });
        expect(sent).toHaveLength(0);
        q(block, '.shell-run-tell').click();
        await flush();
        expect(sent).toHaveLength(1);

        await runToEnd(block, tauri, doneResult({ outcome: 'cancelled', exitCode: null, signal: 15 }));
        expect(sent).toHaveLength(1);
        expect(q(block, '.shell-run-agent-text').textContent).toBe(
            'shared.shell.sandbox.agent.stopped_not_told'
        );
    });

    it('offers no checkbox without a window to send from', async () => {
        fakeTauri();
        const block = createShellCommandBlock(blockedRecord('make'));
        document.body.append(block);
        const box = await openConfirm(block);
        expect(q(box, '.shell-run-confirm-tell').hidden).toBe(true);
    });
});

describe('Run in Terminal (macOS)', () => {
    let platform;
    beforeEach(() => {
        platform = Object.getOwnPropertyDescriptor(navigator, 'platform');
        Object.defineProperty(navigator, 'platform', { value: 'MacIntel', configurable: true });
    });
    afterEach(() => {
        if (platform) Object.defineProperty(navigator, 'platform', platform);
        else delete navigator.platform;
    });

    it('confirms, then hands exactly the command to Terminal once', async () => {
        const tauri = fakeTauri();
        const cmd = 'sudo softwareupdate -i -a';
        const block = createShellCommandBlock(blockedRecord(cmd));
        document.body.append(block);
        const btn = q(block, '.shell-run-terminal');
        expect(btn.hidden).toBe(false);
        expect(btn.title).toBe('shared.shell.sandbox.run_terminal_title');
        btn.click();
        await flush();
        await flush();
        const box = confirmBox(block);
        expect(q(box, '.shell-run-confirm-title').textContent).toBe(
            'shared.shell.sandbox.confirm.title_terminal'
        );
        expect(q(box, '.shell-run-risk').textContent).toBe('shared.shell.sandbox.risk.sudo');
        expect(tauri.commands()).toEqual(['user_shell_run_context']);
        const go = confirmRun(block);
        go.click();
        go.click();
        await flush();
        const calls = tauri.calls.filter(([c]) => c === 'run_command_in_terminal');
        expect(calls).toEqual([['run_command_in_terminal', { command: cmd, cwd: '/home/me', sessionId: null }]]);
        expect(q(block, '.shell-cmd-sandbox-status').textContent).toBe(
            'shared.shell.sandbox.terminal.sent'
        );
    });

    it('only copies a command with a tab (Terminal would complete it)', async () => {
        const tauri = fakeTauri();
        const cmd = 'cat <<-EOF > out.txt\n\thello\nEOF';
        const block = createShellCommandBlock(blockedRecord(cmd));
        document.body.append(block);
        // Running it here is fine (no line editor); typing it isn't.
        expect(runHereBtn(block).hidden).toBe(false);
        expect(q(block, '.shell-run-terminal').hidden).toBe(true);

        const sudo = createShellCommandBlock(blockedRecord('sudo tee /etc/x <<-EOF\n\ty\nEOF'));
        document.body.append(sudo);
        const box = await openConfirm(sudo);
        expect(q(box, '.shell-run-risk').textContent).toBe(
            'shared.shell.sandbox.risk.sudo_here_copy'
        );
        expect(confirmRun(sudo).textContent).toBe('shared.shell.sandbox.open_terminal');
        confirmRun(sudo).click();
        await flush();
        await flush();
        expect(tauri.commands()).not.toContain('run_command_in_terminal');
    });

    it('offers Run in Terminal from the sudo confirm of "Run it here"', async () => {
        const tauri = fakeTauri();
        const block = createShellCommandBlock(blockedRecord('sudo make install'));
        document.body.append(block);
        const box = await openConfirm(block);
        expect(q(box, '.shell-run-risk').textContent).toBe('shared.shell.sandbox.risk.sudo_here');
        expect(confirmRun(block).textContent).toBe('shared.shell.sandbox.run_terminal');
        confirmRun(block).click();
        await flush();
        expect(tauri.commands()).toContain('run_command_in_terminal');
        expect(tauri.commands()).not.toContain('run_user_shell_command');
    });
});
