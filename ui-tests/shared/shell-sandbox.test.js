/**
 * Recognising a command the agent's own sandbox refused: high precision —
 * every positive is a signature a terminal doesn't print, and the plain
 * permission errors a terminal does print (SIP, privacy prompts, file
 * modes, root-owned paths) never match, nor does the same text quoted in
 * other output. Only a failed call counts.
 */
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { initI18n } from '../../ui/js/shared/i18n.js';
import { createShellCommandBlock } from '../../ui/js/shared/shell-command-view.js';
import { detectSandboxDenial, shellSandboxDenial } from '../../ui/js/shared/shell-commands.js';
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
        expect(body).toContain('run the command yourself in a terminal');
        const labels = [...n.querySelectorAll('button')].map((b) => b.textContent);
        expect(labels).toEqual(['Copy command', 'Copy & open Terminal']);
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
