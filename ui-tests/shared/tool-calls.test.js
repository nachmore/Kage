/**
 * Tool calls other than shell commands: MCP classification (parity with the
 * Rust rules the permission handler uses — read from src/config.rs), kind
 * categories and chip groups, output parsing for every shape we know,
 * sanitising, caps (per call, per response together with shell output),
 * diff building, and the record merge across a call's ACP updates.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, it, expect } from 'vitest';
import {
    applyToolUpdate,
    buildDiff,
    capJson,
    classifyMcpCall,
    classifyToolCall,
    diffsFromEditInput,
    DIFF_MAX_SHOWN_LINES,
    fetchUrlsOf,
    getToolCalls,
    groupToolCalls,
    JSON_CUT,
    parseMcpTitle,
    parseToolOutput,
    readRanges,
    searchQueryOf,
    searchSourcesOf,
    settleToolCalls,
    toolCallPaths,
    toolCallSize,
    toolCallState,
    toolChipGroup,
    toolOutputOf,
    TOOL_INPUT_CAP,
    TOOL_OUTPUT_CAP,
} from '../../ui/js/shared/tool-calls.js';
import { SHELL_RESPONSE_OUTPUT_CAP } from '../../ui/js/shared/shell-commands.js';
import { processToolCallUpdate } from '../../ui/js/shared/streaming-utils.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const ESC = '\x1b';

function evt(update) {
    return { payload: { params: { sessionId: 's1', update } } };
}

function feed(state, ...updates) {
    let last = null;
    for (const u of updates) last = processToolCallUpdate(evt(u), state);
    return last;
}

function newState() {
    return { toolUsages: [], toolSources: [] };
}

/** One call through processToolCallUpdate; its record. */
function call(...updates) {
    const state = newState();
    feed(state, ...updates);
    return { state, rec: state.toolUsages[0]?.call ?? null };
}

// --- Rust parity ----------------------------------------------------------

/** Rust string literal body → JS string (the escapes the tests use). */
function unescapeRust(s) {
    return s.replace(/\\(n|r|t|"|\\|u\{([0-9a-fA-F]+)\})/g, (_, c, hex) => {
        if (hex) return String.fromCodePoint(Number.parseInt(hex, 16));
        return { n: '\n', r: '\r', t: '\t', '"': '"', '\\': '\\' }[c];
    });
}

function rustCases() {
    const src = readFileSync(path.join(repo, 'src/config.rs'), 'utf8');
    const positive = [];
    const re = /mcp_server_from_title\("((?:[^"\\]|\\.)*)"\),\s*Some\("((?:[^"\\]|\\.)*)"\)/g;
    for (const m of src.matchAll(re)) positive.push([unescapeRust(m[1]), unescapeRust(m[2])]);
    const start = src.indexOf('fn mcp_server_refuses_anything_ambiguous');
    const block = src.slice(start, src.indexOf('] {', start));
    const negative = [...block.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => unescapeRust(m[1]));
    return { positive, negative };
}

describe('MCP titles: parity with src/config/permissions.rs', () => {
    const { positive, negative } = rustCases();

    it('found the Rust cases', () => {
        expect(positive.length).toBeGreaterThanOrEqual(6);
        expect(negative.length).toBeGreaterThanOrEqual(25);
        expect(negative).toContain('Running: @foo/bar; rm -rf ~');
        expect(negative).toContain('mcp__acp__Bash');
    });

    it('names the same server for every title Rust accepts', () => {
        for (const [title, server] of positive) {
            expect(parseMcpTitle(title)?.server, title).toBe(server);
        }
    });

    it('refuses every title Rust refuses', () => {
        for (const title of negative) expect(parseMcpTitle(title), JSON.stringify(title)).toBeNull();
        expect(parseMcpTitle(`Running: @${'s'.repeat(65)}/tool`)).toBeNull();
        expect(parseMcpTitle(`Running: @${'s'.repeat(64)}/tool`)?.server).toHaveLength(64);
        expect(parseMcpTitle(`Running: @s/${'t'.repeat(129)}`)).toBeNull();
    });

    it('returns the tool too', () => {
        expect(parseMcpTitle('Running: @builder-mcp/InternalSearch')).toEqual({
            server: 'builder-mcp',
            tool: 'InternalSearch',
        });
        expect(parseMcpTitle('mcp__my_server__do_thing')).toEqual({ server: 'my_server', tool: 'do_thing' });
    });

    // Mirrors `mcp_server_tests` in src/commands/messaging/notifications.rs.
    it('classifies calls like classify_mcp_server', () => {
        const t = 'Running: @builder-mcp/InternalSearch';
        expect(classifyMcpCall(t, t, undefined)?.server).toBe('builder-mcp');
        expect(classifyMcpCall(t, t, 'other')?.server).toBe('builder-mcp');
        expect(classifyMcpCall('Running: @kage-computer-control/launch_app', undefined, undefined)?.server).toBe(
            'kage-computer-control'
        );
        expect(classifyMcpCall('mcp__builder-mcp__InternalSearch', null, 'other')?.server).toBe('builder-mcp');
        // A shell command is never an MCP call.
        expect(classifyMcpCall(t, t, 'execute')).toBeNull();
        expect(classifyMcpCall('Running: cd x; @foo/bar', null, undefined)).toBeNull();
        for (const kind of ['edit', 'read', 'delete', 'move', 'search', 'fetch', 'think']) {
            expect(classifyMcpCall('mcp__fs__write', 'mcp__fs__write', kind), kind).toBeNull();
        }
        // Titles that disagree are ambiguous.
        expect(classifyMcpCall(t, 'Running: @other/InternalSearch', undefined)).toBeNull();
        expect(classifyMcpCall(t, 'Running: rm -rf ~', undefined)).toBeNull();
        expect(classifyMcpCall(t, 'mcp__builder-mcp__InternalSearch', undefined)?.server).toBe('builder-mcp');
        expect(classifyMcpCall(t, '', undefined)?.server).toBe('builder-mcp');
    });
});

describe('shell vs MCP through processToolCallUpdate', () => {
    it('an execute call titled like an MCP tool is a shell command, never MCP', () => {
        const state = newState();
        const r = feed(state, {
            toolCallId: 'x',
            title: 'Running: @builder-mcp/InternalSearch',
            kind: 'execute',
            rawInput: { command: '@builder-mcp/InternalSearch' },
        });
        expect(r.shell).not.toBeNull();
        expect(r.call).toBeNull();
        expect(state.toolUsages[0].call).toBeUndefined();
    });

    it('an `other` call with shell text after the MCP title is not MCP', () => {
        const { rec } = call({ toolCallId: 'x', title: 'Running: @foo/bar; rm -rf ~', kind: 'other' });
        expect(rec.category).toBe('other');
        expect(rec.mcp).toBeNull();
        expect(toolChipGroup(rec).key).toBe('kind:other');
    });

    it('a call whose kind turns out to be execute moves to the shell record', () => {
        const state = newState();
        feed(state, { toolCallId: 'k', title: 'shell' });
        expect(state.toolUsages[0].call).not.toBeNull();
        const r = feed(state, { toolCallId: 'k', kind: 'execute', rawInput: { command: 'ls' } });
        expect(r.updated).toBe(true);
        expect(state.toolUsages[0].shell.command).toBe('ls');
        expect(state.toolUsages[0].call).toBeNull();
        expect(getToolCalls(state.toolUsages)).toEqual([]);
    });

    it('a call retitled into an MCP tool reports the chip change', () => {
        const state = newState();
        feed(state, { toolCallId: 'm', title: 'Running: @builder-mcp/InternalSearch' });
        const r = feed(state, { toolCallId: 'm', title: 'Running: @other/InternalSearch' });
        // Titles disagree now: not MCP any more, the chip changes.
        expect(r.updated).toBe(true);
        expect(state.toolUsages[0].call.category).toBe('other');
    });
});

describe('categories and chip groups', () => {
    it('maps ACP kinds to categories', () => {
        const cat = (kind, title = 'x y') => classifyToolCall(kind, title, title).category;
        expect(cat('read')).toBe('read');
        expect(cat('edit')).toBe('edit');
        expect(cat('delete')).toBe('edit');
        expect(cat('move')).toBe('edit');
        expect(cat('search')).toBe('search');
        expect(cat('fetch')).toBe('fetch');
        expect(cat('think')).toBe('think');
        expect(cat('switch_mode')).toBe('other');
        expect(cat('other')).toBe('other');
        expect(cat(undefined)).toBe('other');
    });

    it('infers the kind from a built-in tool name when the agent leaves it out', () => {
        expect(classifyToolCall(undefined, 'read', 'read').category).toBe('read');
        expect(classifyToolCall('other', 'write', 'write').category).toBe('edit');
        expect(classifyToolCall(undefined, 'web_fetch', 'web_fetch').category).toBe('fetch');
        expect(classifyToolCall(undefined, 'grep', 'grep').kind).toBe('search');
        // An explicit kind wins.
        expect(classifyToolCall('read', 'write', 'write').category).toBe('read');
        // Inherited Object properties are not tool names.
        expect(classifyToolCall(undefined, 'constructor', 'constructor').category).toBe('other');
    });

    it('groups MCP calls by server, plain tool names by name, the rest by kind', () => {
        const state = newState();
        feed(
            state,
            { toolCallId: '1', title: 'read', kind: 'read' },
            { toolCallId: '2', title: 'Running: @builder-mcp/InternalSearch' },
            { toolCallId: '3', title: 'read', kind: 'read' },
            { toolCallId: '4', title: 'Running: @builder-mcp/ReadInternalWebsites', kind: 'other' },
            { toolCallId: '5', title: 'Read /Users/me/a.txt', kind: 'read' },
            { toolCallId: '6', title: 'Edit `src/main.rs`', kind: 'edit' }
        );
        const groups = groupToolCalls(state.toolUsages);
        expect(groups.map((g) => [g.key, g.label, g.calls.length])).toEqual([
            ['tool:read', 'read', 2],
            ['mcp:builder-mcp', 'builder-mcp', 2],
            ['kind:read', null, 1],
            ['kind:edit', null, 1],
        ]);
        expect(groups[1].mcp).toBe(true);
        expect(groups[1].calls.map((c) => c.mcp.tool)).toEqual(['InternalSearch', 'ReadInternalWebsites']);
    });
});

describe('parseToolOutput', () => {
    it('reads kiro items: Text and Json, in a Success wrapper', () => {
        expect(parseToolOutput({ Success: { items: [{ Text: 'hello' }] } })).toEqual({
            text: 'hello',
            json: undefined,
            failed: false,
        });
        const p = parseToolOutput({ items: [{ Json: { numMatches: 2, results: [] } }] });
        expect(p.text).toBe('');
        expect(p.json).toEqual({ numMatches: 2, results: [] });
    });

    it('reads MCP results: text content, isError, structuredContent', () => {
        const p = parseToolOutput({
            items: [{ Json: { content: [{ type: 'text', text: '3 hits' }], isError: true } }],
        });
        expect(p).toEqual({ text: '3 hits', json: undefined, failed: true });
        const s = parseToolOutput({
            content: [{ type: 'text', text: 'ok' }],
            structuredContent: { n: 1 },
        });
        expect(s.text).toBe('ok');
        expect(s.json).toEqual({ n: 1 });
    });

    it('reads kiro errors', () => {
        expect(parseToolOutput({ Error: { Custom: 'no such file' } })).toEqual({
            text: 'no such file',
            json: undefined,
            failed: true,
        });
        expect(parseToolOutput({ Error: 'boom' }).failed).toBe(true);
        // A null `error` field is not a failure.
        expect(parseToolOutput({ results: [], error: null }).failed).toBe(false);
    });

    it('reads strings, JSON strings, ACP content blocks; ignores images', () => {
        expect(parseToolOutput('plain').text).toBe('plain');
        expect(parseToolOutput('{"a":1}').json).toEqual({ a: 1 });
        expect(parseToolOutput('{not json').text).toBe('{not json');
        expect(
            parseToolOutput([{ type: 'content', content: { type: 'text', text: 'x' } }, { type: 'image', data: 'AAAA' }])
                .text
        ).toBe('x');
        expect(parseToolOutput(null)).toBeNull();
        expect(parseToolOutput('   ')).toBeNull();
        expect(parseToolOutput([1, 2]).json).toEqual([1, 2]);
    });

    it('stops at hostile nesting', () => {
        let v = 'deep';
        for (let i = 0; i < 50; i++) v = { Success: v };
        expect(parseToolOutput(v)).toBeNull();
    });
});

describe('capJson', () => {
    it('copies, caps strings, entries and depth, and marks cuts', () => {
        const big = { s: 'x'.repeat(20000), arr: Array.from({ length: 500 }, (_, i) => i) };
        const c = capJson(big, 100000);
        expect(c.truncated).toBe(true);
        expect(c.value.s.endsWith(JSON_CUT)).toBe(true);
        expect(c.value.s.length).toBeLessThan(9000);
        expect(c.value.arr).toHaveLength(201);
        expect(c.value.arr[200]).toBe(JSON_CUT);
        let deep = { v: 1 };
        for (let i = 0; i < 20; i++) deep = { d: deep };
        expect(JSON.stringify(capJson(deep, 1000).value)).toContain(JSON_CUT);
    });

    it('respects the character budget', () => {
        const c = capJson({ a: 'y'.repeat(5000), b: 'z'.repeat(5000) }, 3000);
        expect(c.size).toBeLessThanOrEqual(3000 + 64);
        expect(c.truncated).toBe(true);
    });

    it('treats __proto__ as a plain key', () => {
        const v = JSON.parse('{"__proto__": {"polluted": true}, "a": 1}');
        const c = capJson(v, 1000).value;
        expect(Object.getPrototypeOf(c)).toBeNull();
        expect(Object.keys(c)).toEqual(['__proto__', 'a']);
        expect(c.polluted).toBeUndefined();
        expect({}.polluted).toBeUndefined();
    });

    it('sanitises strings (and keys) only when asked', () => {
        const v = { [`k${ESC}[1m`]: `red${ESC}[31m‮evil` };
        expect(Object.entries(capJson(v, 1000, { clean: true }).value)).toEqual([['k', 'redevil']]);
        expect(capJson(v, 1000).value[`k${ESC}[1m`]).toBe(`red${ESC}[31m‮evil`);
    });
});

/** Apply a diff's shown lines to check they're consistent. */
function lines(diff) {
    return diff.hunks.flatMap((h) => h.lines.map((l) => `${l.op}${l.text}`));
}

describe('buildDiff', () => {
    it('shows changed lines with context, numbered', () => {
        const old = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j'].join('\n');
        const neu = ['a', 'b', 'c', 'd', 'E', 'f', 'g', 'h', 'i', 'j'].join('\n');
        const d = buildDiff('/x.txt', old, neu);
        expect(d.added).toBe(1);
        expect(d.removed).toBe(1);
        expect(d.hunks).toHaveLength(1);
        expect(lines(d)).toEqual(['=b', '=c', '=d', '-e', '+E', '=f', '=g', '=h']);
        expect(d.hunks[0].gapBefore).toBe(1);
        expect(d.gapAfter).toBe(2);
        expect(d.hunks[0].lines.find((l) => l.op === '-').oldNo).toBe(5);
        expect(d.hunks[0].lines.find((l) => l.op === '+').newNo).toBe(5);
        expect(d.isNew).toBe(false);
    });

    it('folds long unchanged stretches between hunks', () => {
        const base = Array.from({ length: 100 }, (_, i) => `line ${i}`);
        const neu = [...base];
        neu[5] = 'changed 5';
        neu[90] = 'changed 90';
        const d = buildDiff('f', base.join('\n'), neu.join('\n'));
        expect(d.hunks).toHaveLength(2);
        expect(d.hunks[1].gapBefore).toBe(90 - 3 - (5 + 4));
        expect(d.hunks[1].oldStart).toBe(88);
    });

    it('merges nearby changes into one hunk', () => {
        const base = Array.from({ length: 30 }, (_, i) => `l${i}`);
        const neu = [...base];
        neu[10] = 'x';
        neu[16] = 'y';
        expect(buildDiff('f', base.join('\n'), neu.join('\n')).hunks).toHaveLength(1);
    });

    it('finds insertions and deletions in the middle (Myers)', () => {
        const old = 'a\nb\nc\nd\ne';
        const neu = 'a\nX\nb\nd\ne\nY';
        const d = buildDiff('f', old, neu);
        expect(lines(d)).toEqual(['=a', '+X', '=b', '-c', '=d', '=e', '+Y']);
    });

    it('treats a null old text as a new file and CRLF like LF', () => {
        const d = buildDiff('n', null, 'one\r\ntwo\r\n');
        expect(d.isNew).toBe(true);
        expect(lines(d)).toEqual(['+one', '+two']);
        expect(buildDiff('f', 'a\r\nb', 'a\nb').hunks).toEqual([]);
    });

    it('caps the lines shown and counts the rest', () => {
        const neu = Array.from({ length: 1000 }, (_, i) => `n${i}`).join('\n');
        const d = buildDiff('big', '', neu);
        expect(lines(d)).toHaveLength(DIFF_MAX_SHOWN_LINES);
        expect(d.omittedLines).toBe(1000 - DIFF_MAX_SHOWN_LINES);
        expect(d.added).toBe(1000);
    });

    it('respects the character budget and long lines', () => {
        const d = buildDiff('f', '', `${'q'.repeat(5000)}\nshort`, { budget: 3000 });
        expect(d.hunks[0].lines[0].cut).toBe(true);
        expect(d.hunks[0].lines[0].text.length).toBe(2000);
        expect(d.size).toBeLessThanOrEqual(3000 + 2001);
    });

    it('gives up on huge inputs and very different texts gracefully', () => {
        expect(buildDiff('f', 'x'.repeat(3 * 1024 * 1024), 'y').tooLarge).toBe(true);
        // More than the Myers depth: shown as one replaced block.
        const a = Array.from({ length: 1200 }, (_, i) => `a${i}`).join('\n');
        const b = Array.from({ length: 1200 }, (_, i) => `b${i}`).join('\n');
        const d = buildDiff('f', a, b);
        expect(d.added).toBe(1200);
        expect(d.removed).toBe(1200);
        expect(d.tooLarge).toBe(false);
    });
});

describe('diffsFromEditInput', () => {
    it('kiro write: str_replace, create, insert, append', () => {
        const [r] = diffsFromEditInput({ command: 'str_replace', path: '/a.rs', oldStr: 'let x = 1;', newStr: 'let x = 2;' });
        expect(r.path).toBe('/a.rs');
        expect(r.numbered).toBe(false);
        expect(lines(r)).toEqual(['-let x = 1;', '+let x = 2;']);
        const [c] = diffsFromEditInput({ command: 'create', path: '/n.md', content: '# Title\nbody' });
        expect(lines(c)).toEqual(['+# Title', '+body']);
        expect(c.numbered).toBe(true);
        const [i] = diffsFromEditInput({ command: 'insert', path: '/a', insertLine: 3, newStr: 'z' });
        expect(lines(i)).toEqual(['+z']);
        const [ap] = diffsFromEditInput({ command: 'append', path: '/a', content: 'tail' });
        expect(ap.numbered).toBe(false);
        // Older snake_case names.
        expect(lines(diffsFromEditInput({ command: 'str_replace', path: 'p', old_str: 'a', new_str: 'b' })[0])).toEqual([
            '-a',
            '+b',
        ]);
    });

    it('Claude Code: Edit, MultiEdit, Write', () => {
        const [e] = diffsFromEditInput({ file_path: '/f', old_string: 'a', new_string: '' });
        expect(lines(e)).toEqual(['-a']);
        const m = diffsFromEditInput({
            file_path: '/f',
            edits: [
                { old_string: 'a', new_string: 'b' },
                { old_string: 'c', new_string: 'd' },
            ],
        });
        expect(m.map(lines)).toEqual([
            ['-a', '+b'],
            ['-c', '+d'],
        ]);
        expect(lines(diffsFromEditInput({ file_path: '/w', content: 'all' })[0])).toEqual(['+all']);
        expect(diffsFromEditInput({ file_path: '/w' })).toEqual([]);
        expect(diffsFromEditInput(null)).toEqual([]);
    });
});

describe('applyToolUpdate records', () => {
    it('follows an edit: ACP diff content wins over the arguments; text args are not repeated', () => {
        const { rec } = call(
            {
                sessionUpdate: 'tool_call',
                toolCallId: 'e',
                title: 'write',
                kind: 'edit',
                status: 'pending',
                rawInput: { command: 'str_replace', path: '/a.txt', oldStr: 'one', newStr: 'two', __tool_use_purpose: 'Fix it' },
            },
            {
                sessionUpdate: 'tool_call_update',
                toolCallId: 'e',
                status: 'completed',
                content: [{ type: 'diff', path: '/a.txt', oldText: 'zero\none\nthree', newText: 'zero\ntwo\nthree' }],
            }
        );
        expect(rec.category).toBe('edit');
        expect(rec.purpose).toBe('Fix it');
        expect(rec.diffs).toHaveLength(1);
        expect(rec.diffs[0].numbered).toBe(true);
        expect(lines(rec.diffs[0])).toEqual(['=zero', '-one', '+two', '=three']);
        expect(Object.keys(rec.input)).toEqual(['command', 'path']);
        expect(toolCallState(rec)).toBe('success');
        expect(toolCallPaths(rec)).toEqual(['/a.txt']);
    });

    it('builds the diff from the arguments when there is no diff content', () => {
        const { rec } = call({
            toolCallId: 'e',
            title: 'Edit',
            kind: 'edit',
            rawInput: { file_path: '/b', old_string: 'x', new_string: 'y' },
        });
        expect(lines(rec.diffs[0])).toEqual(['-x', '+y']);
        expect(rec.diffs[0].numbered).toBe(false);
    });

    it('keeps output: rawOutput over content text, sanitised', () => {
        const { rec } = call(
            { toolCallId: 'r', title: 'read', kind: 'read', rawInput: { operations: [{ mode: 'Line', path: '/etc/hosts', start_line: 2, end_line: 9 }] } },
            { toolCallId: 'r', content: [{ type: 'content', content: { type: 'text', text: 'from content' } }] },
            { toolCallId: 'r', status: 'completed', rawOutput: { items: [{ Text: `127.0.0.1 ${ESC}[1mlocalhost‮` }] } }
        );
        const out = toolOutputOf(rec);
        expect(out.text.head).toBe('127.0.0.1 localhost');
        expect(rec.contentOut).toBeNull();
        expect(readRanges(rec)).toEqual([{ path: '/etc/hosts', start: 2, end: 9 }]);
        expect(toolCallPaths(rec)).toEqual(['/etc/hosts']);
    });

    it('falls back to content text and ACP locations', () => {
        const { rec } = call({
            toolCallId: 'r',
            title: 'Read /x',
            kind: 'read',
            locations: [{ path: '/x', line: 40 }, { path: 42 }],
            content: [{ type: 'content', content: { type: 'text', text: '```\nbody\n```' } }],
        });
        expect(toolOutputOf(rec).text.head).toBe('body');
        expect(rec.locations).toEqual([{ path: '/x', line: 40 }]);
        expect(readRanges(rec)).toEqual([{ path: '/x', start: 40, end: null }]);
    });

    it('keeps arguments as sent (untrusted text is not altered), capped', () => {
        const { rec } = call({
            toolCallId: 'm',
            title: 'Running: @srv/tool',
            rawInput: { q: `a${ESC}[2Jb`, big: 'x'.repeat(TOOL_INPUT_CAP * 2) },
        });
        expect(rec.input.q).toBe(`a${ESC}[2Jb`);
        expect(rec.inputTruncated).toBe(true);
        expect(rec.inputSize).toBeLessThanOrEqual(TOOL_INPUT_CAP + 64);
    });

    it('caps output per call', () => {
        const { rec } = call({
            toolCallId: 'f',
            title: 'web_fetch',
            kind: 'fetch',
            rawInput: { url: 'https://example.com' },
            rawOutput: { items: [{ Text: 'y'.repeat(TOOL_OUTPUT_CAP * 4) }] },
        });
        expect(toolCallSize(rec)).toBeLessThanOrEqual(TOOL_OUTPUT_CAP);
        expect(toolOutputOf(rec).text.omitted).toBeGreaterThan(0);
        expect(fetchUrlsOf(rec)).toEqual(['https://example.com']);
    });

    it('caps output per response, together with shell output', () => {
        const state = newState();
        const n = Math.ceil(SHELL_RESPONSE_OUTPUT_CAP / TOOL_OUTPUT_CAP) + 2;
        for (let i = 0; i < n; i++) {
            feed(state, {
                toolCallId: `t${i}`,
                title: 'read',
                kind: 'read',
                rawOutput: { items: [{ Text: 'z'.repeat(TOOL_OUTPUT_CAP) }] },
            });
        }
        feed(
            state,
            { toolCallId: 'sh', title: 'shell', kind: 'execute', rawInput: { command: 'cat big' } },
            { toolCallId: 'sh', status: 'completed', rawOutput: { items: [{ Json: { exit_status: '0', stdout: 'o'.repeat(1000), stderr: '' } }] } }
        );
        let total = 0;
        for (const u of state.toolUsages) total += u.call ? toolCallSize(u.call) : 0;
        expect(total).toBeLessThanOrEqual(SHELL_RESPONSE_OUTPUT_CAP);
        const last = state.toolUsages[n - 1].call;
        expect(toolOutputOf(last).dropped).toBe(true);
        expect(state.toolUsages.at(-1).shell.rawOut.dropped).toBe(true);
    });

    it('JSON output is kept as a clean capped copy', () => {
        const { rec } = call({
            toolCallId: 'g',
            title: 'grep',
            kind: 'search',
            rawInput: { pattern: 'TODO', path: '/src' },
            rawOutput: {
                items: [
                    {
                        Json: {
                            numMatches: 3,
                            results: [
                                { file: '/src/a.js', count: 2, matches: ['1: TODO'] },
                                { file: `/src/b${ESC}[0m.js`, count: 1 },
                            ],
                        },
                    },
                ],
            },
        });
        expect(searchQueryOf(rec)).toEqual({ query: 'TODO', scope: '/src', keys: ['pattern', 'path'] });
        expect(searchSourcesOf(rec).sources).toEqual([
            { path: '/src/a.js', count: 2 },
            { path: '/src/b.js', count: 1 },
        ]);
    });

    it('finds web search sources', () => {
        const { rec } = call({
            toolCallId: 'w',
            title: 'web_search',
            kind: 'search',
            rawInput: { query: 'kage' },
            rawOutput: { items: [{ Json: { results: [{ title: 'Kage', url: 'https://kage.example/' }], query: 'kage', error: null } }] },
        });
        expect(searchSourcesOf(rec)).toEqual({ sources: [{ url: 'https://kage.example/', title: 'Kage' }], more: 0 });
        expect(toolCallState(rec)).toBe('running');
    });

    it('status: failed from the status, kiro Error wrapper or MCP isError; settles to unknown', () => {
        expect(toolCallState(call({ toolCallId: 'a', title: 'read', status: 'failed' }).rec)).toBe('failed');
        expect(
            toolCallState(call({ toolCallId: 'a', title: 'read', rawOutput: { Error: { Custom: 'x' } } }).rec)
        ).toBe('failed');
        expect(
            toolCallState(
                call({
                    toolCallId: 'a',
                    title: 'Running: @s/t',
                    status: 'completed',
                    rawOutput: { items: [{ Json: { content: [{ type: 'text', text: 'denied' }], isError: true } }] },
                }).rec
            )
        ).toBe('failed');
        const { state, rec } = call({ toolCallId: 'a', title: 'read', status: 'in_progress' });
        expect(toolCallState(rec)).toBe('running');
        const rev = rec.rev;
        expect(settleToolCalls(state.toolUsages)).toBe(true);
        expect(toolCallState(rec)).toBe('unknown');
        expect(rec.rev).toBe(rev + 1);
        expect(settleToolCalls(state.toolUsages)).toBe(false);
    });

    it('bumps rev only on change', () => {
        const state = newState();
        feed(state, { toolCallId: 'a', title: 'read', kind: 'read' });
        const rec = state.toolUsages[0].call;
        const rev = rec.rev;
        feed(state, { toolCallId: 'a' });
        expect(rec.rev).toBe(rev);
        feed(state, { toolCallId: 'a', status: 'completed' });
        expect(rec.rev).toBe(rev + 1);
    });

    it('ignores non-entries and keeps the record when applied directly', () => {
        expect(applyToolUpdate(null, {})).toBeNull();
        const entry = { toolCallId: 'z', title: 'think', kind: 'think' };
        const rec = applyToolUpdate(entry, { toolCallId: 'z', rawInput: { thought: 'hmm' } });
        expect(entry.call).toBe(rec);
        expect(rec.category).toBe('think');
    });
});
