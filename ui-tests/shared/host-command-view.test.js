/**
 * Commands an agent asks Kage to run outside its sandbox: the prompt shows
 * the whole command, where it runs, the agent's reason and the warnings,
 * and answers only its own request — Run once, Always allow commands
 * starting with [editable words], Deny. Nothing runs from a prompt for a
 * command with hidden characters; nothing is focused when a prompt
 * appears; the backend's updates drive the run view and close the card.
 *
 * Strings are i18n keys (no catalog loaded), so copy changes don't break
 * these.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    disposeHostCommandUi,
    guardText,
    installHostCommandUi,
    onHostCommandUpdate,
    rehomeHostCommandCards,
    showHostCommandPrompt,
} from '../../ui/js/shared/host-command-view.js';
import { _resetShellRunsForTests } from '../../ui/js/shared/shell-run-view.js';

const flush = () => new Promise((r) => setTimeout(r, 0));

function fakeTauri({ answer, pending = [] } = {}) {
    const calls = [];
    const listeners = {};
    const invoke = vi.fn((cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'answer_host_command') {
            return answer ? answer(args) : Promise.resolve(null);
        }
        if (cmd === 'pending_host_commands') return Promise.resolve(pending);
        return Promise.resolve(null);
    });
    const listen = vi.fn(async (event, cb) => {
        listeners[event] = cb;
        return () => delete listeners[event];
    });
    window.__TAURI__ = { core: { invoke }, event: { listen } };
    return {
        calls,
        answers: () => calls.filter(([c]) => c === 'answer_host_command').map(([, a]) => a),
        emit: (event, payload) => listeners[event]?.({ payload }),
    };
}

function prompt(extra = {}) {
    return {
        requestId: 'hc-1',
        command: 'brew install awscli',
        cwd: '/Users/me',
        shell: 'zsh',
        reason: 'sandbox-exec: sandbox_apply: Operation not permitted',
        agent: 'Kiro',
        sessionId: 's1',
        toolCallId: 'call-1',
        matched: true,
        background: false,
        plain: true,
        canAlways: true,
        suggestedPrefix: 'brew install',
        guard: [],
        timeoutSeconds: 600,
        expiresAt: Date.now() + 5 * 60 * 1000,
        ...extra,
    };
}

const q = (root, sel) => root.querySelector(sel);
const qa = (root, sel) => Array.from(root.querySelectorAll(sel));
const runOnce = (card) => q(card, '.host-cmd-run-once');
const always = (card) => q(card, '.host-cmd-always-btn');
const deny = (card) => q(card, '.host-cmd-deny');
const prefixInput = (card) => q(card, '.host-cmd-prefix');
const click = (btn, detail = 1) =>
    btn.dispatchEvent(new MouseEvent('click', { bubbles: true, detail }));

let tray;
beforeEach(() => {
    tray = document.createElement('div');
    tray.id = 'hostCommandTray';
    document.body.append(tray);
});
afterEach(() => {
    disposeHostCommandUi();
    _resetShellRunsForTests();
    delete window.__TAURI__;
    document.body.innerHTML = '';
    vi.useRealTimers();
});

describe('the prompt', () => {
    it('shows the whole command, where it runs and the agent reason', async () => {
        fakeTauri();
        await installHostCommandUi({ tray });
        const card = showHostCommandPrompt(prompt()).el;
        expect(tray.contains(card)).toBe(true);
        expect(q(card, '.shell-run-confirm-command-text').textContent).toBe('brew install awscli');
        expect(q(card, '.shell-run-confirm-path').textContent).toBe('/Users/me');
        expect(q(card, '.host-cmd-reason-text').textContent).toBe(
            'sandbox-exec: sandbox_apply: Operation not permitted'
        );
        expect(q(card, '.host-cmd-title').textContent).toBe('shared.host_command.title');
        expect(q(card, '.shell-run-confirm-warn').textContent).toBe(
            'shared.shell.sandbox.confirm.warning'
        );
        // Three answers, the prefix editable and filled in.
        expect(runOnce(card)).not.toBeNull();
        expect(deny(card)).not.toBeNull();
        expect(prefixInput(card).value).toBe('brew install');
        expect(q(card, '.host-cmd-expiry').textContent).toBe('shared.host_command.expires');
    });

    it('takes no focus when it appears, so keys typed elsewhere answer nothing', async () => {
        fakeTauri();
        await installHostCommandUi({ tray });
        const input = document.createElement('textarea');
        document.body.append(input);
        input.focus();
        const card = showHostCommandPrompt(prompt()).el;
        expect(document.activeElement).toBe(input);
        expect(card.contains(document.activeElement)).toBe(false);
    });

    it('shows one prompt per request, each answering only its own', async () => {
        const tauri = fakeTauri();
        await installHostCommandUi({ tray });
        tauri.emit('host_command_prompt', prompt());
        tauri.emit('host_command_prompt', prompt()); // sent twice: still one card
        tauri.emit('host_command_prompt', prompt({ requestId: 'hc-2', command: 'make' }));
        const cards = qa(tray, '.host-cmd-card');
        expect(cards).toHaveLength(2);
        click(deny(cards[1]));
        await flush();
        expect(tauri.answers()).toEqual([{ requestId: 'hc-2', decision: 'deny', prefix: null }]);
        // The other one is still waiting, buttons and all.
        expect(runOnce(cards[0]).disabled).toBe(false);
        click(runOnce(cards[0]));
        await flush();
        expect(tauri.answers()[1]).toEqual({
            requestId: 'hc-1',
            decision: 'run_once',
            prefix: null,
        });
    });

    it('runs once per deliberate click and ignores the rest of a double-click', async () => {
        const tauri = fakeTauri();
        await installHostCommandUi({ tray });
        const card = showHostCommandPrompt(prompt()).el;
        click(runOnce(card), 2);
        await flush();
        expect(tauri.answers()).toHaveLength(0);
        click(runOnce(card));
        click(runOnce(card));
        await flush();
        expect(tauri.answers()).toHaveLength(1);
        expect(q(card, '.host-cmd-status').textContent).toBe('shared.host_command.starting');
        expect(q(card, '.host-cmd-actions')).toBeNull();
    });

    it('"Always allow" sends the words the user left in the field', async () => {
        const tauri = fakeTauri();
        await installHostCommandUi({ tray });
        const card = showHostCommandPrompt(prompt({ command: 'brew install --cask iterm2' })).el;
        prefixInput(card).value = 'brew';
        // Enter in the field approves nothing.
        prefixInput(card).dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })
        );
        await flush();
        expect(tauri.answers()).toHaveLength(0);
        click(always(card));
        await flush();
        expect(tauri.answers()).toEqual([{ requestId: 'hc-1', decision: 'always', prefix: 'brew' }]);
        expect(q(card, '.host-cmd-status').textContent).toBe('shared.host_command.rule_added');
    });

    it('keeps the prompt open with the error when an answer is refused', async () => {
        fakeTauri({
            answer: () =>
                Promise.reject({ kind: 'invalid_input', message: "doesn't start with those words" }),
        });
        await installHostCommandUi({ tray });
        const card = showHostCommandPrompt(prompt()).el;
        prefixInput(card).value = 'rm';
        click(always(card));
        await flush();
        await flush();
        expect(q(card, '.host-cmd-status').hidden).toBe(false);
        expect(q(card, '.host-cmd-status').textContent).toBe('shared.host_command.answer_failed');
        expect(always(card).disabled).toBe(false);
        expect(runOnce(card).disabled).toBe(false);
    });

    it('Deny closes the prompt and runs nothing', async () => {
        const tauri = fakeTauri();
        await installHostCommandUi({ tray });
        const card = showHostCommandPrompt(prompt()).el;
        click(deny(card));
        await flush();
        expect(tauri.answers()[0].decision).toBe('deny');
        expect(q(card, '.host-cmd-actions')).toBeNull();
        expect(q(card, '.host-cmd-status').textContent).toBe('shared.host_command.denied');
        expect(q(card, '.shell-run-live')).toBeNull();
    });

    it('says so and drops its buttons when nobody answered in time', async () => {
        const tauri = fakeTauri();
        await installHostCommandUi({ tray });
        const card = showHostCommandPrompt(prompt()).el;
        tauri.emit('host_command_update', { requestId: 'hc-1', phase: 'closed', reason: 'expired' });
        expect(q(card, '.host-cmd-actions')).toBeNull();
        expect(q(card, '.host-cmd-status').textContent).toBe('shared.host_command.expired');
        expect(q(card, '.host-cmd-expiry').hidden).toBe(true);
        // A late click can't answer it.
        expect(runOnce(card)).toBeNull();
    });

    it('closes when the request ended before the answer arrived', async () => {
        fakeTauri({ answer: () => Promise.reject({ kind: 'cancelled', message: 'gone' }) });
        await installHostCommandUi({ tray });
        const card = showHostCommandPrompt(prompt()).el;
        click(runOnce(card));
        await flush();
        await flush();
        expect(q(card, '.host-cmd-actions')).toBeNull();
        expect(q(card, '.host-cmd-status').textContent).toBe('shared.host_command.closed');
    });

    it('warns about credentials and offers no rule when the guard flagged it', async () => {
        fakeTauri();
        await installHostCommandUi({ tray });
        const card = showHostCommandPrompt(
            prompt({
                command: 'cat ~/.aws/credentials',
                guard: ['aws'],
                canAlways: false,
                suggestedPrefix: null,
            })
        ).el;
        expect(q(card, '.host-cmd-guard').textContent).toBe('shared.host_command.guard');
        expect(always(card)).toBeNull();
        expect(prefixInput(card)).toBeNull();
        // It can still be run once, by the user's own click.
        expect(runOnce(card)).not.toBeNull();
        expect(guardText(['ssh', 'aws', 'not-a-kind'])).toBe('shared.host_command.guard');
        expect(guardText([])).toBe('');
    });

    it('offers only Copy and Deny for a command with hidden characters', async () => {
        fakeTauri();
        await installHostCommandUi({ tray });
        // The backend marks it, and the window checks on its own too.
        for (const p of [
            prompt({ requestId: 'a', command: 'rm -rf ~ \u202Egnp.txt', plain: false }),
            prompt({ requestId: 'b', command: 'ls\u200B -la', plain: true }),
        ]) {
            const card = showHostCommandPrompt(p).el;
            expect(runOnce(card)).toBeNull();
            expect(always(card)).toBeNull();
            expect(deny(card)).not.toBeNull();
            expect(q(card, '.host-cmd-actions').textContent).toContain('shared.shell.copy_command');
            expect(q(card, '.shell-run-confirm-warn').textContent).toBe(
                'shared.shell.sandbox.warn_hidden'
            );
            // The hidden character is shown as a mark, never as text.
            expect(q(card, '.shell-cmd-mark')).not.toBeNull();
        }
    });

    it('says when the request isn’t from one of Kage’s chats', async () => {
        fakeTauri();
        await installHostCommandUi({ tray });
        const card = showHostCommandPrompt(
            prompt({ matched: false, agent: null, canAlways: false })
        ).el;
        expect(q(card, '.host-cmd-origin').textContent).toBe('shared.host_command.unmatched');
        expect(q(card, '.host-cmd-title').textContent).toBe('shared.host_command.title_unknown');
    });

    it('shows only the prompts and updates for this window', async () => {
        const tauri = fakeTauri();
        await installHostCommandUi({ tray, windowLabel: 'chat-1' });
        tauri.emit('host_command_prompt', prompt({ window: 'floating' }));
        tauri.emit('host_command_update', {
            requestId: 'hc-5',
            window: 'floating',
            phase: 'started',
            runId: 'kage-run-dddddddddddddddddddddddd',
            approvedBy: 'rule',
            command: 'make',
        });
        expect(qa(tray, '.host-cmd-card')).toHaveLength(0);
        tauri.emit('host_command_prompt', prompt({ window: 'chat-1' }));
        expect(qa(tray, '.host-cmd-card')).toHaveLength(1);
    });

    it('shows again the prompts still waiting after a reload', async () => {
        fakeTauri({ pending: [prompt(), prompt({ requestId: 'hc-9' })] });
        await installHostCommandUi({ tray });
        expect(qa(tray, '.host-cmd-card').map((c) => c.dataset.requestId)).toEqual([
            'hc-1',
            'hc-9',
        ]);
    });
});

describe('placement', () => {
    it('goes where the window puts it, else to the tray, and back to the tray if it leaves the page', async () => {
        fakeTauri();
        const answer = document.createElement('div');
        document.body.append(answer);
        await installHostCommandUi({
            tray,
            place: (card, p) => {
                if (p.sessionId !== 's1') return false;
                answer.append(card);
                return true;
            },
        });
        const inAnswer = showHostCommandPrompt(prompt()).el;
        const other = showHostCommandPrompt(prompt({ requestId: 'hc-2', sessionId: 's2' })).el;
        expect(answer.contains(inAnswer)).toBe(true);
        expect(tray.contains(other)).toBe(true);
        // The answer is re-rendered: the prompt still waits, so it comes back.
        answer.remove();
        rehomeHostCommandCards();
        expect(tray.contains(inAnswer)).toBe(true);
    });
});

describe('the run', () => {
    it('shows the approved run with its output, then its result, with no chat message', async () => {
        const tauri = fakeTauri();
        await installHostCommandUi({ tray });
        const card = showHostCommandPrompt(prompt()).el;
        click(runOnce(card));
        await flush();
        onHostCommandUpdate({
            requestId: 'hc-1',
            phase: 'started',
            runId: 'kage-run-0123456789abcdef01234567',
            approvedBy: 'user',
            command: 'brew install awscli',
            cwd: '/Users/me',
            timeoutSeconds: 600,
        });
        const live = q(card, '.shell-run-live');
        expect(live).not.toBeNull();
        expect(live.dataset.state).toBe('running');
        expect(q(live, '.shell-run-stop').hidden).toBe(false);
        onHostCommandUpdate({
            requestId: 'hc-1',
            phase: 'finished',
            runId: 'kage-run-0123456789abcdef01234567',
            result: {
                outcome: 'exited',
                exitCode: 0,
                signal: null,
                durationMs: 900,
                output: '==> Pouring awscli\n',
                truncated: false,
                outputBytes: 19,
            },
        });
        expect(live.dataset.state).toBe('success');
        expect(q(live, '.shell-run-output').textContent).toContain('Pouring awscli');
        expect(q(live, '.shell-run-agent-text').textContent).toBe(
            'shared.host_command.result_returned'
        );
        // No "Tell the agent" here: the result went back as the tool's result.
        expect(q(live, '.shell-run-tell')).toBeNull();
        expect(tauri.calls.map(([c]) => c)).not.toContain('send_message_streaming');
    });

    it('Stop stops only this run', async () => {
        const tauri = fakeTauri();
        await installHostCommandUi({ tray });
        onHostCommandUpdate({
            requestId: 'hc-7',
            phase: 'started',
            runId: 'kage-run-aaaaaaaaaaaaaaaaaaaaaaaa',
            approvedBy: 'rule',
            rule: 'brew install',
            command: 'brew install jq',
            cwd: '/Users/me',
            agent: 'Kiro',
            timeoutSeconds: 120,
        });
        const card = q(tray, '.host-cmd-card');
        expect(card.dataset.requestId).toBe('hc-7');
        // Ran without a prompt: the card says which rule let it.
        expect(q(card, '.host-cmd-status').textContent).toBe('shared.host_command.by_rule');
        expect(q(card, '.host-cmd-title').textContent).toBe('shared.host_command.auto_title');
        expect(runOnce(card)).toBeNull();
        click(q(card, '.shell-run-stop'));
        await flush();
        expect(tauri.calls.filter(([c]) => c === 'cancel_user_shell_command')).toEqual([
            ['cancel_user_shell_command', { runId: 'kage-run-aaaaaaaaaaaaaaaaaaaaaaaa' }],
        ]);
    });

    it('reports a run that timed out with its own time limit', async () => {
        fakeTauri();
        await installHostCommandUi({ tray });
        onHostCommandUpdate({
            requestId: 'hc-8',
            phase: 'started',
            runId: 'kage-run-bbbbbbbbbbbbbbbbbbbbbbbb',
            approvedBy: 'allow_all',
            command: 'sleep 999',
            cwd: '/tmp',
            timeoutSeconds: 120,
        });
        const card = q(tray, '.host-cmd-card');
        expect(q(card, '.host-cmd-status').textContent).toBe('shared.host_command.by_allow_all');
        onHostCommandUpdate({
            requestId: 'hc-8',
            phase: 'finished',
            result: { outcome: 'timed_out', exitCode: null, signal: 15, output: '' },
        });
        expect(q(card, '.shell-run-live').dataset.state).toBe('failed');
        expect(q(card, '.shell-run-state').textContent).toBe('shared.shell.sandbox.run.timed_out');
    });
});
