/**
 * Shell command tool calls: recognition, command extraction, output parsing
 * for every agent shape we know of, sanitising, caps, and the record merge
 * that follows a call across its ACP updates.
 */
import { describe, it, expect } from 'vitest';
import {
    applyShellUpdate,
    capText,
    cleanShellCommand,
    extractShellCommand,
    getShellCalls,
    isShellToolCall,
    joinCapped,
    parseExitStatus,
    parseShellContent,
    parseShellRawOutput,
    sanitizeAndCap,
    sanitizeTerminalText,
    settleShellCalls,
    shellCallState,
    shellOutputOf,
    shellOutputSize,
    SHELL_COMMAND_CAP,
    SHELL_OUTPUT_CAP,
} from '../../ui/js/shared/shell-commands.js';
import { processToolCallUpdate } from '../../ui/js/shared/streaming-utils.js';

const ESC = '\x1b';

function evt(update, sessionId = 's1') {
    return { payload: { params: { sessionId, update } } };
}

/** Run a sequence of updates for one call through processToolCallUpdate. */
function run(updates, state = { toolUsages: [], toolSources: [] }) {
    let last = null;
    for (const u of updates) last = processToolCallUpdate(evt(u), state);
    return { state, last, rec: state.toolUsages[0]?.shell || null };
}

describe('isShellToolCall', () => {
    it('is true for ACP kind execute whatever the title', () => {
        expect(isShellToolCall('execute', 'shell')).toBe(true);
        expect(isShellToolCall('execute', '`ls -la`')).toBe(true);
        expect(isShellToolCall('execute', undefined)).toBe(true);
    });

    it("falls back to kiro's `Running: <command>` title when the kind is missing or other", () => {
        expect(isShellToolCall(undefined, 'Running: git status')).toBe(true);
        expect(isShellToolCall('other', 'Running: ls')).toBe(true);
    });

    it('never treats kiro MCP titles or other kinds as shell', () => {
        expect(isShellToolCall(undefined, 'Running: @builder-mcp/InternalSearch')).toBe(false);
        expect(isShellToolCall('other', 'Running: @kage-computer-control/click')).toBe(false);
        expect(isShellToolCall('read', 'Running: cat x')).toBe(false);
        expect(isShellToolCall('edit', 'Running: ls')).toBe(false);
        expect(isShellToolCall(undefined, 'shell')).toBe(false);
        expect(isShellToolCall(undefined, 'Running: ')).toBe(false);
        expect(isShellToolCall(undefined, null)).toBe(false);
    });
});

describe('extractShellCommand', () => {
    it('prefers rawInput.command', () => {
        expect(extractShellCommand({ command: 'ls -la' }, 'Running: something else')).toBe('ls -la');
    });

    it('unwraps a shell -c argv array, quotes other argv arrays', () => {
        expect(extractShellCommand({ command: ['bash', '-lc', 'echo hi && ls'] })).toBe(
            'echo hi && ls'
        );
        expect(extractShellCommand({ command: ['/bin/zsh', '-c', 'pwd'] })).toBe('pwd');
        expect(extractShellCommand({ command: ['git', 'commit', '-m', "it's done"] })).toBe(
            "git commit -m 'it'\\''s done'"
        );
        expect(extractShellCommand({ command: ['ls', 42] }, 'Running: ls')).toBe('ls');
    });

    it("falls back to kiro's title minus `Running: `", () => {
        expect(extractShellCommand(undefined, 'Running: F=/tmp/a.zip; echo "e6"')).toBe(
            'F=/tmp/a.zip; echo "e6"'
        );
        expect(extractShellCommand({}, 'Running: @builder-mcp/InternalSearch')).toBe('');
    });

    it("unwraps Claude Code's backticked title", () => {
        expect(extractShellCommand(undefined, '`npm test`')).toBe('npm test');
        expect(extractShellCommand(undefined, '`echo \\`date\\``')).toBe('echo `date`');
    });

    it('is empty for a bare tool name', () => {
        expect(extractShellCommand(undefined, 'shell')).toBe('');
        expect(extractShellCommand({ command: '   ' }, 'Terminal')).toBe('');
    });
});

describe('sanitizeTerminalText', () => {
    it('strips CSI colour/cursor sequences', () => {
        expect(sanitizeTerminalText(`${ESC}[31mred${ESC}[0m ${ESC}[1;32mok${ESC}[K`)).toBe(
            'red ok'
        );
        expect(sanitizeTerminalText(`a${ESC}[2Jb${ESC}[?25lc`)).toBe('abc');
        expect(sanitizeTerminalText('a\x9b31mb')).toBe('ab');
    });

    it('strips OSC sequences (BEL or ST terminated, and unterminated)', () => {
        expect(sanitizeTerminalText(`${ESC}]0;title\x07after`)).toBe('after');
        expect(
            sanitizeTerminalText(`${ESC}]8;;https://evil.example${ESC}\\link${ESC}]8;;${ESC}\\`)
        ).toBe('link');
        // Unterminated OSC stops at the next ESC instead of eating the rest.
        expect(sanitizeTerminalText(`x${ESC}]0;oops${ESC}[0mrest`)).toBe('xrest');
    });

    it('strips DCS/APC strings and other two-byte escapes', () => {
        expect(sanitizeTerminalText(`a${ESC}Pq#0;2;0;0;0${ESC}\\b`)).toBe('ab');
        expect(sanitizeTerminalText(`a${ESC}(Bb${ESC}7c${ESC}=d`)).toBe('abcd');
    });

    it('drops control characters but keeps newlines and tabs', () => {
        expect(sanitizeTerminalText('a\x00b\x07c\x08d\x7fe\x85f')).toBe('abcdef');
        expect(sanitizeTerminalText('col1\tcol2\nline2')).toBe('col1\tcol2\nline2');
    });

    it('resolves carriage returns like a terminal', () => {
        expect(sanitizeTerminalText('a\r\nb')).toBe('a\nb');
        expect(sanitizeTerminalText('10%\r55%\r100%\ndone')).toBe('100%\ndone');
        expect(sanitizeTerminalText('abcdef\rXY')).toBe('XYcdef');
        expect(sanitizeTerminalText('progress\r')).toBe('progress');
    });

    it('drops bidi override/isolate controls (Trojan Source)', () => {
        expect(sanitizeTerminalText('rm ‮evil‬ -rf ⁦x⁩')).toBe('rm evil -rf x');
    });

    it('returns empty for non-strings', () => {
        expect(sanitizeTerminalText(null)).toBe('');
        expect(sanitizeTerminalText({})).toBe('');
    });
});

describe('capText / sanitizeAndCap', () => {
    it('keeps short text whole', () => {
        expect(capText('hello', 10)).toEqual({ head: 'hello', tail: '', omitted: 0 });
    });

    it('keeps head and tail of long text and counts the omission', () => {
        const s = 'a'.repeat(100) + 'b'.repeat(100);
        const c = capText(s, 40);
        expect(c.head).toBe('a'.repeat(20));
        expect(c.tail).toBe('b'.repeat(20));
        expect(c.omitted).toBe(160);
        expect(c.head.length + c.tail.length + c.omitted).toBe(s.length);
    });

    it('snaps cuts to nearby line breaks', () => {
        const lines = Array.from({ length: 50 }, (_, i) => `line ${String(i).padStart(2, '0')}`);
        const c = capText(lines.join('\n'), 200);
        expect(c.head.endsWith('\n')).toBe(true);
        expect(c.tail.startsWith('line')).toBe(true);
    });

    it('never splits a surrogate pair', () => {
        const s = '😀'.repeat(50);
        const c = capText(s, 21);
        expect(c.head).toMatch(/^(?:😀)*$/u);
        expect(c.tail).toMatch(/^(?:😀)*$/u);
    });

    it('sanitises before capping, and trims huge input before sanitising', () => {
        expect(sanitizeAndCap(`${ESC}[31m${ESC}[0m`, 100)).toBeNull();
        const huge = `${ESC}[1mSTART${ESC}[0m` + 'x'.repeat(2_000_000) + 'END';
        const c = sanitizeAndCap(huge, 1000);
        expect(c.head.startsWith('START')).toBe(true);
        expect(c.tail.endsWith('END')).toBe(true);
        expect(c.head.length + c.tail.length).toBeLessThanOrEqual(1000);
        expect(c.omitted).toBeGreaterThan(1_990_000);
        expect(c.head).not.toContain(ESC);
    });

    it('joins with an omission marker', () => {
        const c = capText('a'.repeat(30), 10);
        expect(joinCapped(c, (n) => `[${n} cut]`)).toBe('aaaaa\n[20 cut]\naaaaa');
        expect(joinCapped({ head: 'x', tail: '', omitted: 0 })).toBe('x');
        expect(joinCapped(null)).toBe('');
    });

    it('caps commands for display', () => {
        const long = 'echo ' + 'y'.repeat(SHELL_COMMAND_CAP * 2);
        expect(cleanShellCommand(long).length).toBeLessThan(SHELL_COMMAND_CAP + 50);
        expect(cleanShellCommand(`ls ${ESC}[31m-la`)).toBe('ls -la');
    });
});

describe('parseExitStatus', () => {
    it("parses kiro's `exit status: N` and plain numbers", () => {
        expect(parseExitStatus('exit status: 0')).toEqual({ code: 0 });
        expect(parseExitStatus('exit status: 127')).toEqual({ code: 127 });
        expect(parseExitStatus('exit code 2')).toEqual({ code: 2 });
        expect(parseExitStatus(1)).toEqual({ code: 1 });
        expect(parseExitStatus('3')).toEqual({ code: 3 });
    });

    it('reports signals as a label', () => {
        expect(parseExitStatus('signal: 9 (SIGKILL)')).toEqual({ signal: 'signal: 9 (SIGKILL)' });
    });

    it('ignores anything else', () => {
        expect(parseExitStatus('ok')).toBeNull();
        expect(parseExitStatus(1.5)).toBeNull();
        expect(parseExitStatus(null)).toBeNull();
    });
});

describe('parseShellRawOutput', () => {
    it("parses kiro's items[].Json { exit_status, stdout, stderr }", () => {
        const p = parseShellRawOutput({
            items: [{ Json: { exit_status: 'exit status: 1', stdout: 'out', stderr: 'err' } }],
        });
        expect(p).toMatchObject({ stdout: 'out', stderr: 'err', exit: { code: 1 } });
    });

    it('parses the Success wrapper from kiro session logs', () => {
        const p = parseShellRawOutput({
            Success: { items: [{ Json: { exit_status: 'exit status: 0', stdout: 'hi', stderr: '' } }] },
        });
        expect(p).toMatchObject({ stdout: 'hi', stderr: '', exit: { code: 0 } });
    });

    it('parses items[].Text and joins several items', () => {
        expect(parseShellRawOutput({ items: [{ Text: 'one' }, { Text: 'two' }] })).toMatchObject({
            stdout: 'one\ntwo',
        });
    });

    it('parses flat objects with stdout/stderr/exit_code', () => {
        expect(parseShellRawOutput({ stdout: 'a', stderr: 'b', exit_code: 2 })).toMatchObject({
            stdout: 'a',
            stderr: 'b',
            exit: { code: 2 },
        });
        expect(parseShellRawOutput({ exitCode: 0, output: 'combined' })).toMatchObject({
            stdout: 'combined',
            exit: { code: 0 },
        });
        expect(parseShellRawOutput({ aggregated_output: 'agg', exit_code: 0 })).toMatchObject({
            stdout: 'agg',
        });
    });

    it('treats a plain string as output, and a JSON string by its shape', () => {
        expect(parseShellRawOutput('plain text')).toMatchObject({ stdout: 'plain text' });
        expect(parseShellRawOutput('{"stdout":"x","exit_code":0}')).toMatchObject({
            stdout: 'x',
            exit: { code: 0 },
        });
        // A JSON string we don't recognise is not dumped as output.
        expect(parseShellRawOutput('{"foo": 1}')).toBeNull();
    });

    it('marks Error results and MCP-style isError content as failed', () => {
        expect(parseShellRawOutput({ Error: 'boom' })).toMatchObject({ stderr: 'boom', failed: true });
        expect(
            parseShellRawOutput({ content: [{ type: 'text', text: 'denied' }], isError: true })
        ).toMatchObject({ stdout: 'denied', failed: true });
    });

    it('returns null for unknown shapes instead of dumping JSON', () => {
        expect(parseShellRawOutput({ foo: 'bar', nested: { a: 1 } })).toBeNull();
        expect(parseShellRawOutput({ items: [{ Json: { results: [] } }] })).toBeNull();
        expect(parseShellRawOutput(42)).toBeNull();
        expect(parseShellRawOutput(null)).toBeNull();
        expect(parseShellRawOutput('   ')).toBeNull();
    });
});

describe('parseShellContent', () => {
    it('reads ACP text content blocks and strips one wrapping ```console fence', () => {
        const p = parseShellContent([
            { type: 'content', content: { type: 'text', text: '```console\ntotal 0\nfile\n```' } },
        ]);
        expect(p).toMatchObject({ stdout: 'total 0\nfile', terminal: false });
    });

    it('keeps text that merely contains a fence', () => {
        const text = 'see:\n```\ncode\n```\nend';
        expect(parseShellContent([{ type: 'content', content: { type: 'text', text } }]).stdout).toBe(
            text
        );
    });

    it('flags terminal blocks (output Kage cannot show)', () => {
        expect(parseShellContent([{ type: 'terminal', terminalId: 't1' }])).toMatchObject({
            stdout: '',
            terminal: true,
        });
    });

    it('ignores diffs, images and junk', () => {
        expect(
            parseShellContent([
                { type: 'diff', path: '/x', oldText: 'a', newText: 'b' },
                { type: 'content', content: { type: 'image', data: '...' } },
                null,
                'str',
            ])
        ).toBeNull();
        expect(parseShellContent('nope')).toBeNull();
    });
});

describe('applyShellUpdate via processToolCallUpdate (kiro flow)', () => {
    it('follows a call from tool_call to completion', () => {
        const state = { toolUsages: [], toolSources: [] };
        const r1 = processToolCallUpdate(
            evt({
                sessionUpdate: 'tool_call',
                toolCallId: 'k1',
                title: 'shell',
                kind: 'execute',
                status: 'pending',
                rawInput: { command: 'echo hi' },
            }),
            state
        );
        expect(r1.updated).toBe(true);
        expect(r1.shell).toMatchObject({ toolCallId: 'k1', command: 'echo hi', status: 'pending' });
        expect(shellCallState(r1.shell)).toBe('running');

        const r2 = processToolCallUpdate(
            evt({ sessionUpdate: 'tool_call_update', toolCallId: 'k1', title: 'Running: echo hi' }),
            state
        );
        expect(r2.updated).toBe(false);
        expect(r2.shell.command).toBe('echo hi');

        const r3 = processToolCallUpdate(
            evt({
                sessionUpdate: 'tool_call_update',
                toolCallId: 'k1',
                status: 'completed',
                rawOutput: { items: [{ Json: { exit_status: 'exit status: 0', stdout: 'hi\n', stderr: '' } }] },
            }),
            state
        );
        expect(r3.updated).toBe(false);
        expect(r3.shell).toBe(r1.shell);
        expect(shellCallState(r3.shell)).toBe('success');
        expect(shellOutputOf(r3.shell)).toMatchObject({
            stdout: { head: 'hi\n', tail: '', omitted: 0 },
            stderr: null,
            exitCode: 0,
        });
        // The usage chip entry itself is unchanged apart from the record.
        expect(state.toolUsages).toHaveLength(1);
        expect(state.toolUsages[0].title).toBe('shell');
    });

    it('maps non-zero exit to failed even when the status says completed', () => {
        const { rec } = run([
            { toolCallId: 'k', title: 'shell', kind: 'execute', rawInput: { command: 'false' } },
            {
                toolCallId: 'k',
                status: 'completed',
                rawOutput: { items: [{ Json: { exit_status: 'exit status: 1', stdout: '', stderr: 'nope' } }] },
            },
        ]);
        expect(shellCallState(rec)).toBe('failed');
        expect(shellOutputOf(rec).exitCode).toBe(1);
    });

    it('maps status failed without output to failed', () => {
        const { rec } = run([
            { toolCallId: 'k', title: 'Running: x', kind: 'execute' },
            { toolCallId: 'k', status: 'failed' },
        ]);
        expect(shellCallState(rec)).toBe('failed');
        expect(rec.command).toBe('x');
    });

    it('takes the command from a later title when rawInput never names it', () => {
        const { rec } = run([
            { toolCallId: 'k', title: 'shell', kind: 'execute' },
            { toolCallId: 'k', title: 'Running: ls -la' },
        ]);
        expect(rec.command).toBe('ls -la');
    });

    it('lets a kind that arrives later make the call a shell call', () => {
        const state = { toolUsages: [], toolSources: [] };
        const first = processToolCallUpdate(evt({ toolCallId: 'k', title: 'make' }), state);
        expect(first.shell).toBeNull();
        const later = processToolCallUpdate(
            evt({ toolCallId: 'k', kind: 'execute', status: 'in_progress' }),
            state
        );
        expect(state.toolUsages[0].kind).toBe('execute');
        expect(later.shell).toMatchObject({ title: 'make', command: '', status: 'in_progress' });
        // The first title seeds the command when it names one.
        const s2 = { toolUsages: [], toolSources: [] };
        processToolCallUpdate(evt({ toolCallId: 'm', title: 'Running: make' , kind: 'other' }), s2);
        expect(s2.toolUsages[0].shell.command).toBe('make');
    });

    it('does not create records for non-shell tools', () => {
        const { state, last } = run([
            { toolCallId: 'r', title: 'read_file', kind: 'read', rawInput: { command: 'create' } },
            { toolCallId: 'r', status: 'completed', rawOutput: { stdout: 'x' } },
        ]);
        expect(state.toolUsages[0].shell).toBeUndefined();
        expect(last.shell).toBeNull();
        expect(getShellCalls(state.toolUsages)).toEqual([]);
    });

    it('settles calls the turn left without a terminal status', () => {
        const { rec, state } = run([
            { toolCallId: 'k', title: 'shell', kind: 'execute', status: 'in_progress' },
        ]);
        expect(shellCallState(rec)).toBe('running');
        expect(settleShellCalls(state.toolUsages)).toBe(true);
        expect(shellCallState(rec)).toBe('unknown');
        expect(settleShellCalls(state.toolUsages)).toBe(false);
    });

    it('bumps rev on each change', () => {
        const state = { toolUsages: [], toolSources: [] };
        const a = processToolCallUpdate(evt({ toolCallId: 'k', title: 'shell', kind: 'execute' }), state);
        const r0 = a.shell.rev;
        processToolCallUpdate(evt({ toolCallId: 'k', status: 'in_progress' }), state);
        expect(a.shell.rev).toBeGreaterThan(r0);
    });
});

describe('applyShellUpdate (content-block agents)', () => {
    it("ignores Claude Code's in-progress description, shows the completed output", () => {
        const { rec, state } = run([
            {
                toolCallId: 'c',
                title: '`ls`',
                kind: 'execute',
                status: 'pending',
                rawInput: { command: 'ls', description: 'List files' },
                content: [{ type: 'content', content: { type: 'text', text: 'List files' } }],
            },
        ]);
        expect(shellOutputOf(rec)).toBeNull();
        processToolCallUpdate(
            evt({
                toolCallId: 'c',
                status: 'completed',
                content: [{ type: 'content', content: { type: 'text', text: '```console\na\nb\n```' } }],
            }),
            state
        );
        expect(joinCapped(shellOutputOf(rec).stdout)).toBe('a\nb');
        expect(shellCallState(rec)).toBe('success');
    });

    it('keeps live content output when the completion update carries none', () => {
        const { rec } = run([
            { toolCallId: 'c', title: '`npm i`', kind: 'execute', status: 'in_progress' },
            {
                toolCallId: 'c',
                content: [{ type: 'content', content: { type: 'text', text: 'added 3 packages' } }],
            },
            { toolCallId: 'c', status: 'completed' },
        ]);
        expect(joinCapped(shellOutputOf(rec).stdout)).toBe('added 3 packages');
    });

    it('reports terminal content as unavailable output', () => {
        const { rec } = run([
            {
                toolCallId: 't',
                title: '`top`',
                kind: 'execute',
                content: [{ type: 'terminal', terminalId: 'term-1' }],
            },
            { toolCallId: 't', status: 'completed' },
        ]);
        const out = shellOutputOf(rec);
        expect(out.terminal).toBe(true);
        expect(out.stdout).toBeNull();
    });

    it('prefers rawOutput (with exit status) over content text', () => {
        const { rec } = run([
            { toolCallId: 'x', title: '`ls`', kind: 'execute' },
            {
                toolCallId: 'x',
                status: 'completed',
                rawOutput: { stdout: 'raw', exit_code: 0 },
                content: [{ type: 'content', content: { type: 'text', text: 'content' } }],
            },
        ]);
        expect(joinCapped(shellOutputOf(rec).stdout)).toBe('raw');
    });

    it('uses content output with an exit code from rawOutput', () => {
        const { rec } = run([
            { toolCallId: 'x', title: '`ls`', kind: 'execute' },
            {
                toolCallId: 'x',
                status: 'completed',
                rawOutput: { exit_code: 3 },
                content: [{ type: 'content', content: { type: 'text', text: 'oops' } }],
            },
        ]);
        const out = shellOutputOf(rec);
        expect(joinCapped(out.stdout)).toBe('oops');
        expect(out.exitCode).toBe(3);
        expect(shellCallState(rec)).toBe('failed');
    });

    it('a later rawOutput/content replaces the earlier one (ACP semantics)', () => {
        const { rec } = run([
            { toolCallId: 'x', title: 'Running: tail -f log', kind: 'execute', rawOutput: 'first' },
            { toolCallId: 'x', rawOutput: 'second' },
        ]);
        expect(joinCapped(shellOutputOf(rec).stdout)).toBe('second');
    });
});

describe('output caps', () => {
    it('caps each call at SHELL_OUTPUT_CAP (head + tail kept)', () => {
        const big = 'A'.repeat(SHELL_OUTPUT_CAP) + 'MIDDLE' + 'Z'.repeat(SHELL_OUTPUT_CAP);
        const { rec } = run([
            { toolCallId: 'k', title: 'shell', kind: 'execute' },
            { toolCallId: 'k', status: 'completed', rawOutput: { stdout: big, stderr: '' } },
        ]);
        const out = shellOutputOf(rec);
        expect(shellOutputSize(rec)).toBeLessThanOrEqual(SHELL_OUTPUT_CAP);
        expect(out.stdout.head.startsWith('AAA')).toBe(true);
        expect(out.stdout.tail.endsWith('ZZZ')).toBe(true);
        expect(out.stdout.omitted).toBeGreaterThan(SHELL_OUTPUT_CAP);
        expect(joinCapped(out.stdout)).not.toContain('MIDDLE');
    });

    it('splits the budget between stdout and stderr', () => {
        const { rec } = run([
            { toolCallId: 'k', title: 'shell', kind: 'execute' },
            {
                toolCallId: 'k',
                status: 'completed',
                rawOutput: { stdout: 'o'.repeat(SHELL_OUTPUT_CAP), stderr: 'e'.repeat(SHELL_OUTPUT_CAP) },
            },
        ]);
        const out = shellOutputOf(rec);
        expect(out.stdout.head.length + out.stdout.tail.length).toBe(SHELL_OUTPUT_CAP / 2);
        expect(out.stderr.head.length + out.stderr.tail.length).toBe(SHELL_OUTPUT_CAP / 2);
    });

    it('caps the whole response and says when output was not kept', () => {
        const state = { toolUsages: [], toolSources: [] };
        for (let i = 0; i < 10; i++) {
            processToolCallUpdate(evt({ toolCallId: `c${i}`, title: 'shell', kind: 'execute' }), state);
            processToolCallUpdate(
                evt({
                    toolCallId: `c${i}`,
                    status: 'completed',
                    rawOutput: { stdout: 'x'.repeat(SHELL_OUTPUT_CAP * 2), stderr: '' },
                }),
                state
            );
        }
        const calls = getShellCalls(state.toolUsages);
        const total = calls.reduce((n, r) => n + shellOutputSize(r), 0);
        expect(total).toBeLessThanOrEqual(512 * 1024);
        const last = shellOutputOf(calls[9]);
        expect(last.dropped).toBe(true);
        expect(last.stdout).toBeNull();
        // Re-applying an update to an early call still fits its own budget.
        expect(shellOutputSize(calls[0])).toBe(SHELL_OUTPUT_CAP);
    });

    it('stores sanitised output (escapes and controls gone)', () => {
        const { rec } = run([
            { toolCallId: 'k', title: 'shell', kind: 'execute' },
            {
                toolCallId: 'k',
                status: 'completed',
                rawOutput: { stdout: `${ESC}[32mPASS${ESC}[0m\x00 ok\r\n`, stderr: '' },
            },
        ]);
        expect(joinCapped(shellOutputOf(rec).stdout)).toBe('PASS ok\n');
    });
});
