/**
 * Runs nothing shows any more: the floating window drops its response
 * (and the shell block running a command) on a new prompt or Ctrl/⌘+L.
 * The run must stay visible and stoppable — as a compact chip in the
 * window's tray — until it ends, for the user's own runs ("Run it here")
 * and for runs an agent asked for alike.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createShellCommandBlock } from '../../ui/js/shared/shell-command-view.js';
import {
    _resetShellRunsForTests,
    attachAgentRun,
    finishAgentRun,
    setRunChipTray,
    syncRunChips,
} from '../../ui/js/shared/shell-run-view.js';
import { processToolCallUpdate } from '../../ui/js/shared/streaming-utils.js';

const SANDBOXED = 'sandbox-exec: sandbox_apply: Operation not permitted';
const flush = () => new Promise((r) => setTimeout(r, 0));

function blockedRecord(command) {
    const state = { toolUsages: [], toolSources: [] };
    const evt = (update) => ({ payload: { params: { sessionId: 's1', update } } });
    processToolCallUpdate(
        evt({
            sessionUpdate: 'tool_call',
            toolCallId: 'call-1',
            title: 'shell',
            kind: 'execute',
            status: 'in_progress',
            rawInput: { command },
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

function fakeTauri() {
    const calls = [];
    const runs = [];
    const invoke = vi.fn((cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'user_shell_run_context') {
            return Promise.resolve({ cwd: '/home/me', shell: 'zsh', terminal: false });
        }
        if (cmd === 'run_user_shell_command') {
            return new Promise((resolve) => runs.push({ args, resolve }));
        }
        return Promise.resolve(true);
    });
    window.__TAURI__ = { core: { invoke }, event: { listen: vi.fn(async () => () => {}) } };
    return { calls, runs };
}

function makeTray() {
    const tray = document.createElement('div');
    tray.className = 'shell-run-chips';
    tray.hidden = true;
    document.body.append(tray);
    setRunChipTray(tray);
    return tray;
}

async function startUserRun(block) {
    block.querySelector('.shell-run-here').click();
    await flush();
    await flush();
    block.querySelector('.shell-run-confirm .shell-run-go').click();
    await flush();
}

afterEach(() => {
    _resetShellRunsForTests();
    delete window.__TAURI__;
    document.body.innerHTML = '';
});

describe('the running-command chip', () => {
    it('keeps a run whose block was cleared visible and stoppable until it ends', async () => {
        const tauri = fakeTauri();
        const tray = makeTray();
        const response = document.createElement('div');
        document.body.append(response);
        const block = createShellCommandBlock(blockedRecord('brew install awscli'));
        response.append(block);
        await startUserRun(block);
        syncRunChips();
        // Its block shows it: no chip.
        expect(tray.querySelector('.shell-run-chip')).toBeNull();
        expect(tray.hidden).toBe(true);

        // A new prompt (or Ctrl/⌘+L) clears the response.
        response.remove();
        syncRunChips();
        const chip = tray.querySelector('.shell-run-chip');
        expect(chip).not.toBeNull();
        expect(tray.hidden).toBe(false);
        expect(chip.querySelector('.shell-run-chip-command').textContent).toBe(
            '$ brew install awscli'
        );

        // Its Stop stops that run.
        chip.querySelector('.shell-run-stop').click();
        await flush();
        const runId = tauri.runs[0].args.runId;
        expect(tauri.calls).toContainEqual(['cancel_user_shell_command', { runId }]);

        // Once it ends, the chip goes.
        tauri.runs[0].resolve({ runId, outcome: 'cancelled', exitCode: null, output: '' });
        await flush();
        await flush();
        expect(tray.querySelector('.shell-run-chip')).toBeNull();
        expect(tray.hidden).toBe(true);
    });

    it('shows an agent run whose card left the page', async () => {
        const tauri = fakeTauri();
        const tray = makeTray();
        const card = document.createElement('div');
        const slot = document.createElement('div');
        card.append(slot);
        document.body.append(card);
        const run = attachAgentRun({
            runId: 'kage-run-cccccccccccccccccccccccc',
            command: 'brew install jq',
            cwd: '/tmp',
            timeoutSeconds: 600,
            card,
            slot,
        });
        syncRunChips();
        expect(tray.querySelector('.shell-run-chip')).toBeNull();
        card.remove();
        syncRunChips();
        const chip = tray.querySelector('.shell-run-chip');
        expect(chip).not.toBeNull();
        chip.querySelector('.shell-run-stop').click();
        await flush();
        expect(tauri.calls).toContainEqual([
            'cancel_user_shell_command',
            { runId: 'kage-run-cccccccccccccccccccccccc' },
        ]);
        finishAgentRun(run, { outcome: 'cancelled', exitCode: null, output: '' });
        expect(tray.querySelector('.shell-run-chip')).toBeNull();
    });

    it('does nothing in a window without a tray', async () => {
        fakeTauri();
        const response = document.createElement('div');
        document.body.append(response);
        const block = createShellCommandBlock(blockedRecord('make'));
        response.append(block);
        await startUserRun(block);
        response.remove();
        expect(() => syncRunChips()).not.toThrow();
        expect(document.querySelector('.shell-run-chip')).toBeNull();
    });
});
