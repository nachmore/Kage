/**
 * Recognising a command the agent's own sandbox refused: high precision —
 * every positive is a signature a terminal doesn't print, and the plain
 * permission errors a terminal does print (SIP, privacy prompts, file
 * modes, root-owned paths) never match, nor do most quoted copies of the
 * same text. A failed call counts; a piped one that exited 0 counts only on
 * the nested-Seatbelt line. The notice shows the whole command and asks
 * the user to read it, since any command can print these lines itself.
 */
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { initI18n } from '../../ui/js/shared/i18n.js';
import { createShellCommandBlock } from '../../ui/js/shared/shell-command-view.js';
import {
    detectSandboxDenial,
    hasUnquotedPipe,
    shellSandboxDenial,
} from '../../ui/js/shared/shell-commands.js';
import { processToolCallUpdate } from '../../ui/js/shared/streaming-utils.js';

// Real EN strings, so the notice's copy (and its {error} param) is checked
// as the user reads it.
beforeAll(async () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const catalog = JSON.parse(
        readFileSync(join(here, '..', '..', 'locales', 'en', 'messages.json'), 'utf-8')
    );
    await initI18n(async (cmd) => {
        if (cmd === 'get_i18n_catalog') return { language: 'en', rtl: false, catalog, fallback: catalog };
        throw new Error(`unexpected invoke: ${cmd}`);
    });
});

const BREW_CASK = [
    '==> Downloading https://example.com/BetterDisplay-v3.5.3.dmg',
    'sandbox-exec: sandbox_apply: Operation not permitted',
    'Error: betterdisplay: Download failed',
].join('\n');

describe('detectSandboxDenial: positives', () => {
    it('recognises Homebrew failing to nest its sandbox-exec profile (macOS Seatbelt)', () => {
        expect(detectSandboxDenial(BREW_CASK)).toEqual({
            reason: 'seatbelt',
            line: 'sandbox-exec: sandbox_apply: Operation not permitted',
        });
    });

    it('allows a few `name: ` prefixes, CRLF line ends and colour codes', () => {
        const prefixed = 'Error: betterdisplay: Download failed: sandbox-exec: sandbox_apply: Operation not permitted';
        expect(detectSandboxDenial(prefixed)?.reason).toBe('seatbelt');
        expect(detectSandboxDenial(prefixed)?.line).toBe(prefixed);
        expect(detectSandboxDenial(BREW_CASK.replace(/\n/g, '\r\n'))?.reason).toBe('seatbelt');
        const coloured = '\x1b[31mError:\x1b[0m sandbox-exec: sandbox_apply: Operation not permitted';
        expect(detectSandboxDenial(coloured)?.line).toBe(
            'Error: sandbox-exec: sandbox_apply: Operation not permitted'
        );
    });

    it('recognises the nested-Seatbelt line after any `…: ` (real Homebrew shapes)', () => {
        for (const line of [
            'Error: firefox@nightly: sandbox-exec: sandbox_apply: Operation not permitted',
            'Error: wget (1.24.5): sandbox-exec: sandbox_apply: Operation not permitted',
            'Error: /opt/homebrew/Cellar/x: sandbox-exec: sandbox_apply: Operation not permitted',
            "Error: Download failed on Cask 'betterdisplay' with message: sandbox-exec: sandbox_apply: Operation not permitted",
        ]) {
            expect(detectSandboxDenial(`==> Fetching x\n${line}\n`), line).toEqual({
                reason: 'seatbelt',
                line,
            });
        }
    });

    it('recognises Seatbelt violation reports', () => {
        const out = [
            'curl: (6) Could not resolve host: formulae.brew.sh',
            'Sandbox: curl(48213) deny(1) network-outbound /private/var/run/mDNSResponder',
        ].join('\n');
        expect(detectSandboxDenial(out)).toEqual({
            reason: 'seatbelt',
            line: 'Sandbox: curl(48213) deny(1) network-outbound /private/var/run/mDNSResponder',
        });
        expect(
            detectSandboxDenial('Sandbox: Google Chrome He(812) deny(1) file-write-create /Users/me/x')
                ?.reason
        ).toBe('seatbelt');
    });

    it("recognises Claude Code's <sandbox_violations> block", () => {
        const stderr = [
            'touch: /Users/me/.zshrc: Operation not permitted',
            '<sandbox_violations>',
            'touch(51234) deny(1) file-write-data /Users/me/.zshrc',
            '</sandbox_violations>',
        ].join('\n');
        expect(detectSandboxDenial(stderr)).toEqual({
            reason: 'sandbox-runtime',
            line: '<sandbox_violations>',
        });
    });

    it("recognises the sandbox runtime's proxy refusing a request", () => {
        const verbose = ['< HTTP/1.1 403 Forbidden', '< X-Proxy-Error: blocked-by-sandbox-runtime'].join(
            '\n'
        );
        expect(detectSandboxDenial(verbose)?.reason).toBe('sandbox-runtime');
        expect(detectSandboxDenial('x-proxy-error: blocked-by-sandbox-runtime')?.reason).toBe(
            'sandbox-runtime'
        );
    });

    it('recognises bubblewrap failing to set up its namespaces', () => {
        for (const line of [
            'bwrap: setting up uid map: Permission denied',
            'bwrap: setting up gid map: Operation not permitted',
            'bwrap: Creating new namespace failed: Operation not permitted',
            "bwrap: No permissions to create new namespace, likely because the kernel does not allow non-privileged user namespaces. On e.g. debian this can be enabled with 'sysctl kernel.unprivileged_userns_clone=1'.",
            'bwrap: Creating new namespace failed: nesting depth or /proc/sys/user/max_*_namespaces exceeded (ENOSPC)',
            'bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted',
            "bwrap: Can't mount proc on /newroot/proc: Operation not permitted",
        ]) {
            // (`line` is clipped at 200 characters.)
            expect(detectSandboxDenial(`${line}\n`), line).toEqual({
                reason: 'bubblewrap',
                line: line.slice(0, 200),
            });
        }
    });

    it("doesn't blame the agent for bwrap errors from a command that runs bwrap itself", () => {
        // On a system that restricts unprivileged user namespaces (Ubuntu
        // 23.10+), these fail the same way in a terminal.
        const out = 'bwrap: setting up uid map: Permission denied\n';
        for (const command of [
            'flatpak run org.example.App',
            'bwrap --ro-bind / / true',
            '/usr/bin/bwrap --dev /dev sh',
            'cd ~/x && flatpak-builder build app.yml',
            'steam -silent',
        ]) {
            expect(detectSandboxDenial(out, { command }), command).toBeNull();
        }
        // Another command's output: the bwrap that failed is the agent's.
        expect(detectSandboxDenial(out, { command: 'ls ~/steam-library' })?.reason).toBe(
            'bubblewrap'
        );
        expect(detectSandboxDenial(out, { command: 'make test' })?.reason).toBe('bubblewrap');
    });

    it("recognises Codex's own sandbox errors", () => {
        expect(
            detectSandboxDenial(
                'sandbox error: sandbox denied exec error, exit code: 1, stdout: , stderr: touch: x: Operation not permitted'
            )?.reason
        ).toBe('codex');
        expect(
            detectSandboxDenial('error: Landlock was not able to fully enforce all sandbox rules')?.reason
        ).toBe('codex');
    });

    it('clips a very long matching line', () => {
        const hit = detectSandboxDenial(`Sandbox: x(1) deny(1) file-read-data /${'a'.repeat(5000)}`);
        expect(hit.reason).toBe('seatbelt');
        expect(hit.line.length).toBe(200);
    });
});

describe('detectSandboxDenial: negatives', () => {
    it('ignores the permission errors a terminal prints too', () => {
        for (const out of [
            'touch: /System/Library/x: Operation not permitted', // SIP
            'ls: /Users/me/Library/Mail: Operation not permitted', // privacy (TCC)
            'mkdir: /usr/local/share/x: Permission denied',
            'bash: ./install.sh: Permission denied',
            'Error: Permission denied @ apply2files - /usr/local/lib/node_modules/x',
            'cp: /etc/hosts: Read-only file system',
            'sudo: a terminal is required to read the password',
            'zsh: operation not permitted: ./run',
            'Bad system call: 31',
            'curl: (6) Could not resolve host: example.com',
            'Error: betterdisplay: Download failed',
        ]) {
            expect(detectSandboxDenial(out), out).toBeNull();
        }
    });

    it('ignores signatures quoted inside other output', () => {
        for (const out of [
            // A test diff and an assertion message.
            '- sandbox-exec: sandbox_apply: Operation not permitted',
            '-sandbox-exec: sandbox_apply: Operation not permitted',
            "msg: 'sandbox-exec: sandbox_apply: Operation not permitted'",
            "expected 'sandbox-exec: sandbox_apply: Operation not permitted' to be null",
            // A grep hit in source code.
            'src/sandbox.c:42:    if (sandbox_apply(profile) != 0) err(1, "sandbox_apply");',
            // The system log's form (timestamp first) — what Homebrew's own
            // build sandbox prints in verbose mode, failing in a terminal too.
            '2026-11-06 10:00:00.123456-0800  localhost kernel[0]: (Sandbox) Sandbox: sh(123) deny(1) file-write-create /tmp/x',
            'Sandbox: disabled',
            // bwrap's own usage errors and a missing program aren't the sandbox.
            'bwrap: execvp brew: No such file or directory',
            'bwrap: Specifying --uid requires --unshare-user or --userns',
            // A tag that isn't a block of its own.
            'echo "<sandbox_violations></sandbox_violations>"',
            '<sandbox_violations>\nnever closed',
        ]) {
            expect(detectSandboxDenial(out), out).toBeNull();
        }
    });

    it('returns null for empty and non-string input', () => {
        expect(detectSandboxDenial('')).toBeNull();
        expect(detectSandboxDenial(null)).toBeNull();
        expect(detectSandboxDenial(undefined)).toBeNull();
        expect(detectSandboxDenial({ stderr: BREW_CASK })).toBeNull();
    });

    it('stays fast on long lines full of `name: ` prefixes', () => {
        const hostile = `${'a b: '.repeat(20000)}\n`.repeat(3);
        expect(detectSandboxDenial(hostile)).toBeNull();
        const colons = `${': sandbox-exec: sandbox_ap'.repeat(20000)}\n`;
        expect(detectSandboxDenial(colons)).toBeNull();
    });

    it('strict mode checks only the nested-Seatbelt line', () => {
        expect(detectSandboxDenial(BREW_CASK, { strict: true })?.reason).toBe('seatbelt');
        for (const out of [
            'Sandbox: curl(48213) deny(1) network-outbound /private/var/run/mDNSResponder',
            '<sandbox_violations>\ntouch(5) deny(1) file-write-data /x\n</sandbox_violations>',
            'bwrap: setting up uid map: Permission denied',
            'sandbox error: sandbox denied exec error, exit code: 1',
        ]) {
            expect(detectSandboxDenial(out), out).not.toBeNull();
            expect(detectSandboxDenial(out, { strict: true }), out).toBeNull();
        }
    });
});

describe('hasUnquotedPipe', () => {
    it('finds pipelines and or-lists', () => {
        for (const c of [
            'brew install --cask betterdisplay 2>&1 | tail -20',
            'make |& tee build.log',
            'brew install x || true',
            "grep -c 'a' log|wc -l",
            'x=$(brew install y 2>&1 | tail -1)',
            "echo 'a|b' | cat",
            'echo hi # note\nls | wc -l',
        ]) {
            expect(hasUnquotedPipe(c), c).toBe(true);
        }
    });

    it('ignores quoted, escaped and commented-out bars', () => {
        for (const c of [
            'brew install x',
            "grep 'a|b' log",
            'grep "a|b" log',
            'grep a\\|b log',
            'echo "it\'s | here"',
            'ls # | wc -l',
            '',
        ]) {
            expect(hasUnquotedPipe(c), c).toBe(false);
        }
        expect(hasUnquotedPipe(null)).toBe(false);
    });
});

// --- Records ----------------------------------------------------------------

function evt(update) {
    return { payload: { params: { sessionId: 's1', update } } };
}

function record(...updates) {
    const state = { toolUsages: [], toolSources: [] };
    let shell = null;
    for (const u of updates) shell = processToolCallUpdate(evt(u), state).shell || shell;
    return shell;
}

const start = (command) => ({
    sessionUpdate: 'tool_call',
    toolCallId: 'c1',
    title: 'shell',
    kind: 'execute',
    status: 'in_progress',
    rawInput: { command },
});

const kiroResult = (exit, stdout, stderr) => ({
    toolCallId: 'c1',
    status: 'completed',
    rawOutput: { items: [{ Json: { exit_status: `exit status: ${exit}`, stdout, stderr } }] },
});

describe('shellSandboxDenial', () => {
    it('flags the failed brew install (kiro-cli output)', () => {
        const rec = record(
            start('brew install --cask betterdisplay'),
            kiroResult(1, '==> Downloading …', BREW_CASK.split('\n').slice(1).join('\n'))
        );
        expect(shellSandboxDenial(rec)).toEqual({
            reason: 'seatbelt',
            line: 'sandbox-exec: sandbox_apply: Operation not permitted',
        });
    });

    it('also looks at stdout', () => {
        const rec = record(start('make'), kiroResult(2, BREW_CASK, ''));
        expect(shellSandboxDenial(rec)?.reason).toBe('seatbelt');
    });

    it('needs the call to have failed', () => {
        expect(shellSandboxDenial(record(start('cat log'), kiroResult(0, BREW_CASK, '')))).toBeNull();
        expect(shellSandboxDenial(record(start('brew install x')))).toBeNull();
        expect(shellSandboxDenial(null)).toBeNull();
    });

    it('flags a piped brew install that exited 0 (no pipefail)', () => {
        const rec = record(
            start('brew install --cask betterdisplay 2>&1 | tail -20'),
            kiroResult(0, BREW_CASK, '')
        );
        expect(shellSandboxDenial(rec)).toEqual({
            reason: 'seatbelt',
            line: 'sandbox-exec: sandbox_apply: Operation not permitted',
        });
        const orTrue = record(start('brew install x || true'), kiroResult(0, '', BREW_CASK));
        expect(shellSandboxDenial(orTrue)?.reason).toBe('seatbelt');
    });

    it('a call that exited 0 counts only on the nested-Seatbelt line, and only when piped', () => {
        const report = 'Sandbox: curl(48213) deny(1) network-outbound /private/var/run/mDNSResponder';
        expect(
            shellSandboxDenial(record(start('curl -s https://example.com | sh'), kiroResult(0, report, '')))
        ).toBeNull();
        // A quoted bar isn't a pipe.
        expect(
            shellSandboxDenial(record(start("grep 'sandbox|exec' log"), kiroResult(0, BREW_CASK, '')))
        ).toBeNull();
    });

    it("skips bwrap errors for a command that runs bwrap itself", () => {
        const out = 'bwrap: No permissions to create new namespace, likely because the kernel does not allow non-privileged user namespaces.';
        expect(shellSandboxDenial(record(start('flatpak run org.example.App'), kiroResult(1, '', out)))).toBeNull();
        expect(shellSandboxDenial(record(start('npm test'), kiroResult(1, '', out)))?.reason).toBe(
            'bubblewrap'
        );
    });

    it('ignores a failure with a plain permission error', () => {
        const rec = record(
            start('touch /System/x'),
            kiroResult(1, '', 'touch: /System/x: Operation not permitted')
        );
        expect(shellSandboxDenial(rec)).toBeNull();
    });

    it("reads kiro's Error shape and content-block agents' output", () => {
        const errorShape = record(start('brew install x'), {
            toolCallId: 'c1',
            status: 'failed',
            rawOutput: { Error: 'sandbox-exec: sandbox_apply: Operation not permitted' },
        });
        expect(shellSandboxDenial(errorShape)?.reason).toBe('seatbelt');

        const claude = record(start('touch ~/.zshrc'), {
            toolCallId: 'c1',
            status: 'failed',
            content: [
                {
                    type: 'content',
                    content: {
                        type: 'text',
                        text: '```console\ntouch: /Users/me/.zshrc: Operation not permitted\n<sandbox_violations>\ntouch(5) deny(1) file-write-data /Users/me/.zshrc\n</sandbox_violations>\n```',
                    },
                },
            ],
        });
        expect(shellSandboxDenial(claude)?.reason).toBe('sandbox-runtime');
    });
});

describe('the notice, in English', () => {
    afterEach(() => {
        delete window.__TAURI__;
    });

    const flush = () => new Promise((r) => setTimeout(r, 0));

    function blockedBlock() {
        return createShellCommandBlock(
            record(start('brew install --cask betterdisplay'), kiroResult(1, '', BREW_CASK))
        );
    }

    it('says what happened and what to do', () => {
        const n = blockedBlock().querySelector('.shell-cmd-sandbox');
        expect(n.querySelector('.shell-cmd-sandbox-title').textContent).toBe(
            "Blocked by the agent's sandbox"
        );
        const body = n.querySelector('.shell-cmd-sandbox-text').textContent;
        expect(body).toContain('its own sandbox');
        expect(body).toContain("can't change that");
        // The notice can't vouch for the command: it asks for a read first.
        expect(body).toContain('Read the whole command first');
        expect(body).toContain('only if you trust all of it');
        expect(n.querySelector('.shell-cmd-sandbox-command').getAttribute('aria-label')).toBe(
            'Command'
        );
        expect(n.querySelector('.shell-cmd-sandbox-warn').hidden).toBe(true);
        const labels = [...n.querySelectorAll('button')].map((b) => b.textContent);
        expect(labels).toEqual(['Copy command', 'Copy & open Terminal']);
    });

    it('warns about hidden characters and several lines', () => {
        const hidden = createShellCommandBlock(
            record(start('brew install x\u202e'), kiroResult(1, '', BREW_CASK))
        );
        expect(hidden.querySelector('.shell-cmd-sandbox-warn').textContent).toBe(
            "It contains hidden or control characters (shown boxed), so it may not do what it seems to. Don't run it unless you know why they're there."
        );
        const lines = createShellCommandBlock(
            record(start('brew update\nbrew install x'), kiroResult(1, '', BREW_CASK))
        );
        expect(lines.querySelector('.shell-cmd-sandbox-warn').textContent).toBe(
            'It spans several lines. Read every line, not just the first.'
        );
    });

    it('shows an AppError from opening the terminal by its message', async () => {
        Object.defineProperty(navigator, 'clipboard', {
            value: { writeText: vi.fn(() => Promise.resolve()) },
            configurable: true,
        });
        window.__TAURI__ = {
            core: {
                invoke: vi.fn(() =>
                    Promise.reject({ kind: 'internal', message: 'Failed to execute: not found' })
                ),
            },
        };
        const block = blockedBlock();
        block.querySelector('.shell-cmd-open-terminal').click();
        await flush();
        await flush();
        expect(block.querySelector('.shell-cmd-sandbox-status').textContent).toBe(
            "The command is copied, but the terminal didn't open: Failed to execute: not found"
        );
    });
});
