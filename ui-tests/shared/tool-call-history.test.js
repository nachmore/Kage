/**
 * Tool calls of a reopened session: kiro-cli's session log (as
 * `load_session` returns it — see parse_jsonl in
 * src/commands/sessions/crud.rs) rebuilt into the same per-response tool
 * state a live turn leaves, so the chat window draws the same chips and
 * details. The fixtures have the structure of real kiro lines; every value
 * is made up.
 */
import { describe, it, expect } from 'vitest';
import { buildRenderQueue } from '../../ui/js/shared/session-render.js';
import {
    buildHistoryTools,
    HistoryToolCollector,
    historyToolUpdates,
} from '../../ui/js/shared/tool-call-history.js';
import { shellCallState } from '../../ui/js/shared/shell-commands.js';
import { toolCallState, toolOutputOf } from '../../ui/js/shared/tool-calls.js';
import { renderToolSources } from '../../ui/js/shared/tool-call-view.js';

const noImages = () => null;

const prompt = (id, text) => ({ kind: 'Prompt', message_id: id, content: [{ kind: 'text', data: text }] });
const assistant = (id, ...content) => ({ kind: 'AssistantMessage', message_id: id, content });
const text = (s) => ({ kind: 'text', data: s });
const use = (toolUseId, name, input) => ({ kind: 'toolUse', data: { toolUseId, name, input } });
const result = (toolUseId, status, ...content) => ({
    kind: 'toolResult',
    data: { toolUseId, status, content },
});
const textOut = (s) => ({ kind: 'text', data: s });
const jsonOut = (v) => ({ kind: 'json', data: v });
const results = (id, items, toolKinds = {}) => ({
    kind: 'ToolResults',
    message_id: id,
    content: items,
    tool_kinds: toolKinds,
});

function session() {
    return [
        prompt('p1', 'Tidy the config and check the build'),
        assistant(
            'a1',
            { kind: 'thinking', data: { text: '', signature: null, redactedContent: [1, 2, 3] } },
            text('Looking at it.'),
            use('t_sh', 'shell', { command: 'cargo check', __tool_use_purpose: 'Build' }),
            use('t_mcp', 'InternalSearch', { query: 'widgets', pageSize: 5 }),
            use('t_wr', 'write', { command: 'str_replace', path: '/repo/a.toml', oldStr: 'x = 1', newStr: 'x = 2' }),
            use('t_rd', 'read', { operations: [{ mode: 'Line', path: '/repo/b.rs', start_line: 3, end_line: 9 }] }),
            use('t_ws', 'web_search', { query: 'toml spec' }),
            use('t_lost', 'grep', { pattern: 'TODO', path: '/repo' })
        ),
        results(
            'r1',
            [
                result('t_sh', 'error', jsonOut({ exit_status: 'exit status: 101', stdout: '', stderr: 'error[E0425]' })),
                result('t_mcp', 'success', jsonOut({ content: [{ type: 'text', text: '2 results' }], isError: false })),
                result('t_wr', 'success', textOut('Replaced 1 occurrence')),
                result('t_rd', 'success', textOut('fn main() {}')),
                result(
                    't_ws',
                    'success',
                    jsonOut({ results: [{ title: 'TOML', url: 'https://toml.example/spec', domain: 'toml.example' }], query: 'toml spec', error: null })
                ),
            ],
            {
                t_sh: { builtin: 'ExecuteCmd' },
                t_mcp: { mcp_server: 'builder-mcp', mcp_tool: 'InternalSearch' },
                t_wr: { builtin: 'FileWrite' },
                t_rd: { builtin: 'FileRead' },
                t_ws: { builtin: 'WebSearch' },
            }
        ),
        assistant('a2', text('The build fails on an unknown name.')),
        prompt('p2', 'thanks'),
        assistant('a3', text('Any time.')),
    ];
}

const byId = (tools, id) => tools.toolUsages.find((u) => u.toolCallId === id);

describe('buildRenderQueue with tool calls', () => {
    it("attaches a turn's tools to its last reply, rebuilt like a live turn", () => {
        const queue = buildRenderQueue(session(), {}, {}, noImages);
        expect(queue.map((i) => [i.type, i.text, !!i.tools])).toEqual([
            ['user', 'Tidy the config and check the build', false],
            ['assistant', 'Looking at it.', false],
            ['assistant', 'The build fails on an unknown name.', true],
            ['user', 'thanks', false],
            ['assistant', 'Any time.', false],
        ]);
        const tools = queue[2].tools;
        expect(tools.toolUsages.map((u) => u.toolCallId)).toEqual(['t_sh', 't_mcp', 't_wr', 't_rd', 't_ws', 't_lost']);

        const sh = byId(tools, 't_sh').shell;
        expect(sh.command).toBe('cargo check');
        expect(sh.rawOut.stderr.head).toBe('error[E0425]');
        expect(sh.rawOut.exitCode).toBe(101);
        expect(shellCallState(sh)).toBe('failed');

        const mcp = byId(tools, 't_mcp').call;
        expect(mcp.category).toBe('mcp');
        expect(mcp.mcp).toEqual({ server: 'builder-mcp', tool: 'InternalSearch' });
        expect(toolOutputOf(mcp).text.head).toBe('2 results');
        expect(toolCallState(mcp)).toBe('success');

        const wr = byId(tools, 't_wr').call;
        expect(wr.category).toBe('edit');
        expect(wr.diffs[0].path).toBe('/repo/a.toml');
        expect(wr.diffs[0].added).toBe(1);

        const rd = byId(tools, 't_rd').call;
        expect(rd.category).toBe('read');
        expect(toolOutputOf(rd).text.head).toBe('fn main() {}');

        // A search's sources become source chips, as live.
        expect(tools.toolSources.map((s) => s.url)).toEqual(['https://toml.example/spec']);

        // No result was logged: the turn is over, so the status is unknown.
        expect(toolCallState(byId(tools, 't_lost').call)).toBe('unknown');
    });

    it('renders the same chip row a live response gets', () => {
        const queue = buildRenderQueue(session(), {}, {}, noImages);
        const container = document.createElement('div');
        renderToolSources(container, queue[2].tools.toolUsages, queue[2].tools.toolSources);
        const chipLabels = [...container.querySelectorAll('.tool-call-chip .tool-call-chip-label')].map((n) => n.textContent);
        expect(chipLabels).toEqual(['builder-mcp', 'write', 'read', 'web_search', 'grep']);
        expect(container.querySelector('.shell-chip')).not.toBeNull();
        expect(container.querySelector('.shell-chip').dataset.state).toBe('failed');
        expect(container.querySelector('.tool-sources-static a.source-chip[data-url]')).not.toBeNull();
    });

    it('an MCP tool without a logged server is a plain tool, never guessed', () => {
        const msgs = [
            prompt('p', 'q'),
            assistant('a', use('m', 'InternalSearch', { query: 'x' })),
            results('r', [result('m', 'success', textOut('ok'))]),
            assistant('b', text('done')),
        ];
        const call = buildRenderQueue(msgs, {}, {}, noImages).at(-1).tools.toolUsages[0].call;
        expect(call.category).toBe('other');
        expect(call.mcp).toBeNull();
    });

    it('a bogus server name from the log is refused by the title rules', () => {
        const [u] = historyToolUpdates({ toolUseId: 'x', name: 't', input: {} }, null, {
            mcp_server: 'a b',
            mcp_tool: 't',
        });
        const { tools } = buildHistoryTools([{ toolUseId: 'x', name: 't', input: {} }], new Map(), new Map([['x', { mcp_server: 'a b', mcp_tool: 't' }]]));
        expect(u.title).toBe('Running: @a b/t');
        expect(tools.toolUsages[0].call.category).toBe('other');
    });

    it('a shell command spelled like an MCP tool stays a shell command', () => {
        const msgs = [
            prompt('p', 'q'),
            assistant('a', use('s', 'shell', { command: '@builder-mcp/InternalSearch' })),
            results('r', [result('s', 'success', jsonOut({ exit_status: 'exit status: 0', stdout: 'ok', stderr: '' }))], {
                s: { builtin: 'ExecuteCmd' },
            }),
            assistant('b', text('done')),
        ];
        const u = buildRenderQueue(msgs, {}, {}, noImages).at(-1).tools.toolUsages[0];
        expect(u.shell.command).toBe('@builder-mcp/InternalSearch');
        expect(u.call).toBeUndefined();
    });

    it('a turn that only used tools gets an empty reply for its chips', () => {
        const msgs = [
            prompt('p', 'go'),
            assistant('a', use('r', 'read', { path: '/x' })),
            results('rr', [result('r', 'success', textOut('x'))]),
            prompt('p2', 'next'),
            assistant('b', text('ok')),
        ];
        const queue = buildRenderQueue(msgs, {}, {}, noImages);
        expect(queue.map((i) => [i.type, i.text])).toEqual([
            ['user', 'go'],
            ['assistant', ''],
            ['user', 'next'],
            ['assistant', 'ok'],
        ]);
        expect(queue[1].tools.toolUsages).toHaveLength(1);
        expect(queue[3].tools).toBeUndefined();
    });

    it("a steering turn's tools are not shown", () => {
        const msgs = [
            prompt('p', '[KAGE_STEERING_IGNORE] be brief'),
            assistant('a', text('ok'), use('r', 'read', { path: '/x' })),
            prompt('p2', 'hi'),
            assistant('b', text('hello')),
        ];
        const queue = buildRenderQueue(msgs, {}, {}, noImages);
        expect(queue.map((i) => i.type)).toEqual(['steering', 'steering_ack', 'user', 'assistant']);
        expect(queue.some((i) => i.tools)).toBe(false);
    });

    it('sessions without tool calls are unchanged', () => {
        const msgs = [prompt('p', 'hi'), assistant('a', text('hello'))];
        expect(buildRenderQueue(msgs, {}, {}, noImages)).toEqual([
            { type: 'user', text: 'hi', snapshots: null, meta: null },
            { type: 'assistant', text: 'hello', meta: null },
        ]);
    });
});

describe('caps', () => {
    it('allocates the session budget newest response first', () => {
        const big = 'b'.repeat(60 * 1024);
        const turn = (n) => [
            prompt(`p${n}`, `q${n}`),
            assistant(`a${n}`, use(`r${n}`, 'read', { path: `/f${n}` })),
            results(`x${n}`, [result(`r${n}`, 'success', textOut(big))]),
            assistant(`b${n}`, text(`done ${n}`)),
        ];
        const collector = new HistoryToolCollector();
        const items = [];
        for (const n of [1, 2, 3]) {
            const [p, a, r, b] = turn(n);
            collector.startTurn(false);
            collector.addAssistant(a.content);
            collector.addResults(r);
            const item = { type: 'assistant', text: b.content[0].data };
            items.push(item);
            collector.setItem(item);
            expect(p.kind).toBe('Prompt');
        }
        // Room for a bit more than one response's output.
        collector.finish(80 * 1024);
        const out = (i) => toolOutputOf(items[i].tools.toolUsages[0].call);
        expect(out(2).text.head.length + out(2).text.tail.length).toBe(60 * 1024);
        expect(out(1).text.omitted).toBeGreaterThan(0);
        expect(out(0).dropped).toBe(true);
    });

    it('output per call is capped like a live call', () => {
        const msgs = [
            prompt('p', 'q'),
            assistant('a', use('f', 'web_fetch', { url: 'https://x.example' })),
            results('r', [result('f', 'success', textOut('w'.repeat(1024 * 1024)))]),
            assistant('b', text('done')),
        ];
        const call = buildRenderQueue(msgs, {}, {}, noImages).at(-1).tools.toolUsages[0].call;
        const t = toolOutputOf(call).text;
        expect(t.head.length + t.tail.length).toBeLessThanOrEqual(64 * 1024);
    });
});

describe('historyToolUpdates', () => {
    it("synthesises kiro's live updates", () => {
        expect(
            historyToolUpdates(
                { toolUseId: 'a', name: 'shell', input: { command: 'ls' } },
                { toolUseId: 'a', status: 'success', content: [jsonOut({ stdout: 'x' }), textOut('t'), { kind: 'image' }] },
                null
            )
        ).toEqual([
            { sessionUpdate: 'tool_call', toolCallId: 'a', title: 'shell', kind: 'execute', status: 'pending', rawInput: { command: 'ls' } },
            {
                sessionUpdate: 'tool_call_update',
                toolCallId: 'a',
                status: 'completed',
                rawOutput: { items: [{ Json: { stdout: 'x' } }, { Text: 't' }] },
            },
        ]);
        const [mcp] = historyToolUpdates({ toolUseId: 'm', name: 'Search', input: {} }, null, {
            mcp_server: 'srv',
            mcp_tool: 'Search',
        });
        expect(mcp.title).toBe('Running: @srv/Search');
        expect(mcp.kind).toBe('other');
        // Unknown names fall back to the built-in variant.
        expect(historyToolUpdates({ toolUseId: 'b', name: 'mystery' }, null, { builtin: 'FileRead' })[0].kind).toBe('read');
        expect(historyToolUpdates({ toolUseId: 'b', name: 'mystery' }, null, null)[0].kind).toBe('other');
        expect(historyToolUpdates({ toolUseId: 'b', name: 'x' }, { status: 'error', content: [] }, null)[1]).toEqual({
            sessionUpdate: 'tool_call_update',
            toolCallId: 'b',
            status: 'failed',
        });
    });
});
