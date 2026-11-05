/**
 * Tool call blocks and chips in the DOM: every tool chip is a toggle for
 * that response's calls of the tool (MCP calls by server), each call an
 * expandable block with per-kind details (diff, path + lines, query +
 * sources, URL, MCP arguments + result). Untrusted data stays text (no
 * markup, no links, hidden characters marked), aria wiring is real, and
 * open lists / panels / JSON values survive updates and re-renders.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
    createToolCallBlock,
    isToolCallExpanded,
    renderToolSources,
    setToolCallExpanded,
    syncToolCallLists,
    syncToolSources,
    toolResultText,
    updateToolCallBlock,
} from '../../ui/js/shared/tool-call-view.js';
import { SHELL_LAYOUT_EVENT } from '../../ui/js/shared/shell-command-view.js';
import { processToolCallUpdate } from '../../ui/js/shared/streaming-utils.js';

const XSS = '<img src=x onerror="window.__pwned=1">';
const ESC = '\x1b';

function evt(update) {
    return { payload: { params: { sessionId: 's1', update } } };
}
function newState() {
    return { toolUsages: [], toolSources: [] };
}
function feed(state, ...updates) {
    for (const u of updates) processToolCallUpdate(evt(u), state);
    return state;
}
const recOf = (state, id) => state.toolUsages.find((u) => u.toolCallId === id).call;

function row(state, sources = []) {
    const container = document.createElement('div');
    container.className = 'tool-sources';
    document.body.append(container);
    renderToolSources(container, state.toolUsages, sources);
    return container;
}

const chips = (c) => [...c.querySelectorAll('.tool-call-chips > .tool-call-chip')];
const chipText = (chip) => chip.textContent.replace(/\s+/g, ' ').trim();
const listOf = (chip) => document.getElementById(chip.getAttribute('aria-controls'));
const blocksIn = (list) => [...list.querySelectorAll(':scope > .tool-call')];
const header = (b) => b.querySelector('.tool-call-header');
const panel = (b) => b.querySelector('.tool-call-panel');
const open = (b) => {
    setToolCallExpanded(b, true);
    return panel(b);
};
const noMarkup = (root) => {
    expect(root.querySelector('img, script, a, iframe')).toBeNull();
    expect(window.__pwned).toBeUndefined();
};

beforeEach(() => {
    document.body.innerHTML = '';
    delete window.__pwned;
});

describe('chips', () => {
    function setup() {
        const state = feed(
            newState(),
            { toolCallId: 'r1', title: 'read', kind: 'read', rawInput: { operations: [{ path: '/a' }] } },
            { toolCallId: 'm1', title: 'Running: @builder-mcp/InternalSearch', rawInput: { query: 'q' } },
            { toolCallId: 'r2', title: 'read', kind: 'read', rawInput: { operations: [{ path: '/b' }] } },
            { toolCallId: 'm2', title: 'Running: @builder-mcp/ReadInternalWebsites', status: 'failed' }
        );
        state.toolUsages.push({ toolCallId: 'x', title: 'ext:todos/add', kind: 'extension' });
        return { state, container: row(state) };
    }

    it('one toggle chip per kind / MCP server with a count; extension chips stay static', () => {
        const { container } = setup();
        const cs = chips(container);
        // A built-in (kiro `read`) is listed by its kind, translated.
        expect(cs.map(chipText)).toEqual(['shared.tool.kind.read×2', 'builder-mcp×2']);
        for (const c of cs) {
            expect(c.tagName).toBe('BUTTON');
            expect(c.type).toBe('button');
            expect(c.getAttribute('aria-expanded')).toBe('false');
        }
        expect(cs[0].getAttribute('aria-label')).toBe('shared.tool.chip_state.running');
        expect(cs[1].getAttribute('aria-label')).toBe('shared.tool.chip_state.running');
        expect(cs[1].dataset.state).toBe('running');
        expect(container.querySelector('.tool-sources-static').textContent).toContain('Todos');
        expect(container.querySelector('.tool-sources-static .tool-call-chip')).toBeNull();
        // Tool names are identifiers: LTR inside the chip.
        expect(cs[1].querySelector('.tool-call-chip-label').dir).toBe('ltr');
    });

    it('a chip toggles the ordered list of its calls, all collapsed', () => {
        const { container } = setup();
        const chip = chips(container)[0];
        const list = listOf(chip);
        expect(list.hidden).toBe(true);
        const onLayout = vi.fn();
        container.addEventListener(SHELL_LAYOUT_EVENT, onLayout);
        chip.click();
        expect(list.hidden).toBe(false);
        expect(chip.getAttribute('aria-expanded')).toBe('true');
        expect(onLayout).toHaveBeenCalledTimes(1);
        expect(blocksIn(list).map((b) => b.querySelector('.tool-call-summary').textContent)).toEqual(['/a', '/b']);
        expect(blocksIn(list).map((b) => header(b).getAttribute('aria-expanded'))).toEqual(['false', 'false']);
        chip.click();
        expect(list.hidden).toBe(true);
    });

    it('a failed call marks its chip, for screen readers too', () => {
        const state = feed(
            newState(),
            { toolCallId: 'a', title: 'read', kind: 'read', status: 'failed' },
            { toolCallId: 'b', title: 'read', kind: 'read', status: 'completed' }
        );
        const container = row(state);
        const chip = chips(container)[0];
        expect(chip.dataset.state).toBe('failed');
        // The red dot is decorative; the accessible name says it.
        expect(chip.querySelector('.shell-chip-dot').getAttribute('aria-hidden')).toBe('true');
        expect(chip.getAttribute('aria-label')).toBe('shared.tool.chip_state.failed');
        expect(chip.title).toBe('shared.tool.chip_state.failed');
        // The list keeps the plain name.
        expect(listOf(chip).getAttribute('aria-label')).toBe('shared.tool.chip_title');
        // All done, none failed: just the name.
        feed(state, { toolCallId: 'c', title: 'Running: @srv/tool', status: 'completed' });
        renderToolSources(container, state.toolUsages, []);
        expect(chips(container)[1].getAttribute('aria-label')).toBe('shared.tool.chip_title_mcp');
    });

    it('an unknown plain tool name keeps a chip of its own', () => {
        const state = feed(newState(), { toolCallId: 'a', title: 'deploy', kind: 'other', status: 'completed' });
        const chip = chips(row(state))[0];
        expect(chipText(chip)).toBe('deploy');
        expect(chip.querySelector('.tool-call-chip-label').dir).toBe('ltr');
    });

    it('groups by kind with a translated label when the title is not a tool name', () => {
        const state = feed(newState(), { toolCallId: 'a', title: 'Read /Users/me/notes.md', kind: 'read' });
        const chip = chips(row(state))[0];
        expect(chipText(chip)).toBe('shared.tool.kind.read');
        expect(chip.querySelector('.tool-call-chip-label').dir).toBe('auto');
    });

    it('open lists and expanded calls survive re-renders and new calls', () => {
        const { state, container } = setup();
        const chip = chips(container)[1];
        chip.click();
        const list = listOf(chip);
        const block = blocksIn(list)[0];
        open(block);
        const details = block.querySelector('.tool-call-args');
        expect(details).not.toBeNull();

        feed(state, { toolCallId: 'm3', title: 'Running: @builder-mcp/InternalSearch' });
        feed(state, { toolCallId: 'g1', title: 'grep', kind: 'search' });
        renderToolSources(container, state.toolUsages, []);

        const again = chips(container);
        expect(again[1]).toBe(chip);
        expect(chipText(again[1])).toBe('builder-mcp×3');
        expect(again.map(chipText)).toEqual([
            'shared.tool.kind.read×2',
            'builder-mcp×3',
            'shared.tool.kind.search',
        ]);
        expect(listOf(chip)).toBe(list);
        expect(list.hidden).toBe(false);
        expect(blocksIn(list)[0]).toBe(block);
        expect(isToolCallExpanded(block)).toBe(true);
    });

    it('later status and output refresh an open call in place', () => {
        const { state, container } = setup();
        chips(container)[0].click();
        const block = blocksIn(listOf(chips(container)[0]))[0];
        open(block);
        expect(block.dataset.state).toBe('running');
        expect(panel(block).textContent).toContain('shared.tool.still_running');
        feed(state, { toolCallId: 'r1', status: 'completed', rawOutput: { items: [{ Text: 'file body' }] } });
        syncToolSources(container, state.toolUsages);
        expect(block.dataset.state).toBe('success');
        expect(panel(block).querySelector('.tool-call-text').textContent).toBe('file body');
    });

    it('moves a call to its new chip without rebuilding it', () => {
        const state = feed(newState(), { toolCallId: 'a', title: 'Running: @srv/tool' });
        const container = row(state);
        chips(container)[0].click();
        const block = blocksIn(listOf(chips(container)[0]))[0];
        open(block);
        // The title changes so it's no longer unambiguous MCP.
        feed(state, { toolCallId: 'a', title: 'Running: @other/tool' });
        syncToolCallLists(container, state.toolUsages);
        const cs = chips(container);
        expect(cs).toHaveLength(1);
        expect(cs[0].dataset.group).toBe('kind:other');
        expect(blocksIn(listOf(cs[0]))[0]).toBe(block);
        expect(isToolCallExpanded(block)).toBe(true);
    });

    it('drops chips and lists when the calls are gone', () => {
        const { container } = setup();
        renderToolSources(container, [], []);
        expect(container.querySelector('.tool-call-chips')).toBeNull();
        expect(container.querySelector('.tool-call-lists')).toBeNull();
    });
});

describe('blocks', () => {
    it('header button with aria wiring; the panel is built on open and emptied on close', () => {
        const state = feed(newState(), { toolCallId: 'a', title: 'read', kind: 'read', rawInput: { path: '/p' } });
        const block = createToolCallBlock(recOf(state, 'a'));
        document.body.append(block);
        const h = header(block);
        expect(h.tagName).toBe('BUTTON');
        expect(h.getAttribute('aria-controls')).toBe(panel(block).id);
        expect(panel(block).getAttribute('aria-labelledby')).toBe(h.id);
        expect(panel(block).getAttribute('role')).toBe('region');
        expect(block.dir).toBe('ltr');
        expect(panel(block).hidden).toBe(true);
        expect(panel(block).childElementCount).toBe(0);
        const onLayout = vi.fn();
        document.body.addEventListener(SHELL_LAYOUT_EVENT, onLayout);
        h.click();
        expect(h.getAttribute('aria-expanded')).toBe('true');
        expect(panel(block).hidden).toBe(false);
        expect(panel(block).childElementCount).toBe(1);
        h.click();
        expect(panel(block).hidden).toBe(true);
        expect(panel(block).childElementCount).toBe(0);
        expect(onLayout).toHaveBeenCalledTimes(2);
        expect(block.querySelector('.shell-cmd-status').getAttribute('role')).toBe('img');
        expect(block.querySelector('.shell-cmd-status').getAttribute('aria-label')).toBe('shared.tool.status.running');
    });

    it('edit: path, +/− note, a unified diff with numbers and folded gaps', () => {
        const old = Array.from({ length: 20 }, (_, i) => `line ${i + 1}`).join('\n');
        const neu = old.replace('line 10', 'line ten');
        const state = feed(newState(), {
            toolCallId: 'e',
            title: 'write',
            kind: 'edit',
            status: 'completed',
            rawInput: { command: 'str_replace', path: '/src/app.js' },
            content: [{ type: 'diff', path: '/src/app.js', oldText: old, newText: neu }],
        });
        const block = createToolCallBlock(recOf(state, 'e'));
        expect(block.querySelector('.tool-call-summary').textContent).toBe('/src/app.js');
        const meta = block.querySelector('.tool-call-meta');
        expect(meta.querySelector('[aria-hidden="true"]').textContent).toBe('+1 −1');
        expect(meta.querySelector('.tool-sr-only').textContent).toBe('shared.tool.diff.stat');
        const p = open(block);
        expect(p.querySelector('.tool-call-value').textContent).toBe('/src/app.js');
        const rows = [...p.querySelectorAll('.tool-diff-line')];
        expect(rows.map((r) => r.dataset.op)).toEqual(['ctx', 'ctx', 'ctx', 'del', 'add', 'ctx', 'ctx', 'ctx']);
        const del = rows[3];
        expect(del.querySelector('.tool-diff-num').textContent).toBe('10');
        expect(del.querySelector('.tool-diff-sign').textContent).toBe('−');
        expect(del.querySelector('.tool-sr-only').textContent).toBe('shared.tool.diff.removed_line');
        expect(rows[4].querySelector('.tool-diff-text').textContent).toBe('line ten');
        expect(p.querySelectorAll('.tool-diff-gap')).toHaveLength(2);
        expect(p.querySelector('.tool-diff').getAttribute('role')).toBe('group');
        // The footer copies the path.
        expect(p.querySelector('.shell-cmd-copy').textContent).toContain('shared.tool.copy_path');
    });

    it('edit: a new file is badged; too many lines say how many are left out', () => {
        const big = Array.from({ length: 500 }, (_, i) => `n${i}`).join('\n');
        const state = feed(newState(), {
            toolCallId: 'n',
            title: 'Write',
            kind: 'edit',
            content: [{ type: 'diff', path: '/new.txt', oldText: null, newText: big }],
        });
        const p = open(createToolCallBlock(recOf(state, 'n')));
        expect(p.querySelector('.tool-diff-badge').textContent).toBe('shared.tool.diff.new_file');
        expect(p.querySelector('.tool-diff-more').textContent).toBe('shared.tool.diff.more_changes');
    });

    it('read: path and line range in the header and the panel, content as text', () => {
        const state = feed(newState(), {
            toolCallId: 'r',
            title: 'read',
            kind: 'read',
            status: 'completed',
            rawInput: { operations: [{ mode: 'Line', path: '/etc/hosts', start_line: 1, end_line: 5 }] },
            rawOutput: { items: [{ Text: '127.0.0.1 localhost' }] },
        });
        const block = createToolCallBlock(recOf(state, 'r'));
        expect(block.querySelector('.tool-call-summary').textContent).toBe('/etc/hosts');
        expect(block.querySelector('.tool-call-meta [aria-hidden]').textContent).toBe('1–5');
        const p = open(block);
        const values = [...p.querySelectorAll('.tool-call-field .tool-call-value')].map((n) => n.textContent);
        expect(values).toEqual(['/etc/hosts', '1–5']);
        // `operations` only said that: it isn't listed again.
        expect(p.querySelector('.tool-call-args')).toBeNull();
        expect(p.querySelector('.tool-call-output').getAttribute('aria-label')).toBe('shared.tool.result_label');
        expect(p.querySelector('.tool-call-output').tabIndex).toBe(0);
        expect(p.querySelector('.tool-call-text').textContent).toBe('127.0.0.1 localhost');
    });

    it('search: query, scope and result sources as plain text (no links)', () => {
        const state = feed(newState(), {
            toolCallId: 's',
            title: 'web_search',
            kind: 'search',
            status: 'completed',
            rawInput: { query: 'kage tauri' },
            rawOutput: {
                items: [{ Json: { results: [{ title: XSS, url: 'javascript:alert(1)' }, { title: 'B', url: 'https://b.example/' }] } }],
            },
        });
        const block = createToolCallBlock(recOf(state, 's'));
        expect(block.querySelector('.tool-call-summary').textContent).toBe('kage tauri');
        const p = open(block);
        expect([...p.querySelectorAll('.tool-call-field .tool-call-value')].map((n) => n.textContent)).toEqual([
            'kage tauri',
        ]);
        // The query is a field, not repeated as an argument.
        expect(p.querySelector('.tool-call-args')).toBeNull();
        const items = [...p.querySelectorAll('.tool-call-source')];
        expect(items).toHaveLength(2);
        expect(items[0].querySelector('.tool-call-source-title').textContent).toBe(XSS);
        expect(items[0].querySelector('.tool-call-source-url').textContent).toBe('javascript:alert(1)');
        noMarkup(p);
    });

    it('fetch: the URL is text, never a link', () => {
        const state = feed(newState(), {
            toolCallId: 'f',
            title: 'web_fetch',
            kind: 'fetch',
            rawInput: { url: 'https://evil.example/?q=<b>x</b>', mode: 'selective' },
            rawOutput: 'Visit https://phish.example now',
        });
        const p = open(createToolCallBlock(recOf(state, 'f')));
        expect(p.querySelector('.tool-call-field .tool-call-value').textContent).toBe('https://evil.example/?q=<b>x</b>');
        expect(p.querySelector('.tool-call-text').textContent).toBe('Visit https://phish.example now');
        // url is a field; the remaining argument is listed.
        expect([...p.querySelectorAll('.tool-call-arg-key')].map((k) => k.textContent)).toEqual(['mode']);
        noMarkup(p);
    });

    it('MCP: server and tool, key/value arguments with nested JSON collapsed, JSON result collapsible', () => {
        const state = feed(newState(), {
            toolCallId: 'm',
            title: 'Running: @builder-mcp/InternalSearch',
            status: 'completed',
            rawInput: { query: 'widgets', isDeep: true, filters: { team: 'a', tags: ['x', 'y'] } },
            rawOutput: { items: [{ Json: { hits: [{ id: 1 }], total: 1 } }] },
        });
        const block = createToolCallBlock(recOf(state, 'm'));
        expect(block.dataset.category).toBe('mcp');
        expect(block.querySelector('.tool-call-summary').textContent).toBe('InternalSearch');
        expect(block.querySelector('.tool-call-meta [aria-hidden]').textContent).toBe('widgets');
        const p = open(block);
        const fields = [...p.querySelectorAll('.tool-call-field .tool-call-value')].map((n) => n.textContent);
        expect(fields).toEqual(['builder-mcp', 'InternalSearch']);
        const keys = [...p.querySelectorAll('.tool-call-arg-key')].map((k) => k.textContent);
        expect(keys).toEqual(['query', 'isDeep', 'filters']);
        const nested = p.querySelector('.tool-call-args details.tool-json');
        expect(nested.open).toBe(false);
        expect(nested.querySelector('.tool-json-body').textContent).toContain('"team": "a"');
        const result = p.querySelector('.tool-call-output details.tool-json');
        expect(result.open).toBe(true); // short: starts open
        expect(JSON.parse(result.querySelector('.tool-json-body').textContent)).toEqual({ hits: [{ id: 1 }], total: 1 });
        expect(toolResultText(recOf(state, 'm'))).toContain('"total": 1');
    });

    it('MCP: text result; an isError result shows as failed with an Error section', () => {
        const state = feed(newState(), {
            toolCallId: 'm',
            title: 'mcp__github__create_issue',
            kind: 'other',
            status: 'completed',
            rawInput: { title: 'Bug' },
            rawOutput: { content: [{ type: 'text', text: 'permission denied' }], isError: true },
        });
        const block = createToolCallBlock(recOf(state, 'm'));
        expect(block.dataset.state).toBe('failed');
        const p = open(block);
        expect(p.querySelector('.tool-call-section-label:last-of-type')).not.toBeNull();
        const labels = [...p.querySelectorAll('.tool-call-section-label')].map((n) => n.textContent);
        expect(labels).toContain('shared.tool.error_label');
        expect(p.querySelector('.tool-call-text').textContent).toBe('permission denied');
        expect(p.querySelector('.shell-cmd-exit').hidden).toBe(false);
    });

    it('open nested JSON values stay open when the panel refreshes', () => {
        const state = feed(newState(), {
            toolCallId: 'm',
            title: 'Running: @s/t',
            rawInput: { opts: { a: 1 } },
        });
        const block = createToolCallBlock(recOf(state, 'm'));
        open(block);
        block.querySelector('.tool-call-args details').open = true;
        feed(state, { toolCallId: 'm', status: 'completed', rawOutput: 'done' });
        updateToolCallBlock(block, recOf(state, 'm'));
        expect(block.querySelector('.tool-call-args details').open).toBe(true);
        expect(block.querySelector('.tool-call-text').textContent).toBe('done');
    });

    it('think / other: purpose, arguments and output', () => {
        const state = feed(newState(), {
            toolCallId: 't',
            title: 'todo_list',
            status: 'completed',
            rawInput: { command: 'create', tasks: [{ task_description: 'a' }], __tool_use_purpose: 'Plan the work' },
            rawOutput: { items: [{ Json: { tasks: [{ id: '1', completed: false }] } }] },
        });
        const rec = recOf(state, 't');
        expect(rec.category).toBe('think');
        const block = createToolCallBlock(rec);
        expect(block.querySelector('.tool-call-summary').textContent).toBe('todo_list');
        expect(block.querySelector('.tool-call-meta [aria-hidden]').textContent).toBe('create');
        const p = open(block);
        expect(p.querySelector('.tool-call-purpose').textContent).toBe('Plan the work');
        expect(p.querySelector('.tool-call-purpose').dir).toBe('auto');
        expect([...p.querySelectorAll('.tool-call-arg-key')].map((k) => k.textContent)).toEqual(['command', 'tasks']);
    });

    it('notes: no result, dropped result, still running', () => {
        const state = feed(newState(), { toolCallId: 'a', title: 'read', status: 'completed' });
        expect(open(createToolCallBlock(recOf(state, 'a'))).textContent).toContain('shared.tool.no_output');
        const rec = recOf(state, 'a');
        rec.rawOut = { text: null, json: undefined, failed: false, dropped: true, size: 0 };
        rec.rev += 1;
        expect(open(createToolCallBlock(rec)).textContent).toContain('shared.tool.output_dropped');
    });

    it('unknown subject shows a placeholder', () => {
        const block = createToolCallBlock({
            toolCallId: 'z',
            title: '',
            firstTitle: '',
            category: 'other',
            kind: 'other',
            diffs: [],
            rev: 1,
        });
        const s = block.querySelector('.tool-call-summary');
        expect(s.textContent).toBe('shared.tool.unknown_call');
        expect(s.classList.contains('is-placeholder')).toBe(true);
    });
});

describe('untrusted data stays text', () => {
    it('titles, paths, arguments, outputs, diff lines and JSON keys never become markup', () => {
        const state = feed(
            newState(),
            {
                toolCallId: 'a',
                title: XSS,
                kind: 'edit',
                rawInput: { path: XSS, [XSS]: XSS, nested: { [XSS]: [XSS] } },
                content: [{ type: 'diff', path: XSS, oldText: XSS, newText: `${XSS}\n<script>x</script>` }],
                rawOutput: { items: [{ Text: XSS }, { Json: { [XSS]: XSS } }] },
            },
            { toolCallId: 'b', title: `Running: @srv/tool`, rawInput: { q: XSS }, rawOutput: XSS },
            { toolCallId: 'c', title: XSS, kind: 'search', rawInput: { query: XSS }, rawOutput: { results: [{ title: XSS, url: XSS }] } }
        );
        const container = row(state);
        for (const chip of chips(container)) chip.click();
        for (const b of container.querySelectorAll('.tool-call')) open(b);
        for (const d of container.querySelectorAll('details')) d.open = true;
        noMarkup(container);
        expect(container.textContent).toContain(XSS);
        expect(container.querySelector('.tool-diff-text').textContent).toBe(XSS);
    });

    it('hidden characters in what a call operates on are marked, not applied', () => {
        const path = '/tmp/‮txt.exe';
        const state = feed(newState(), {
            toolCallId: 'a',
            title: 'read',
            kind: 'read',
            rawInput: { path, note: `a${ESC}[2Jb` },
        });
        const block = createToolCallBlock(recOf(state, 'a'));
        const marks = block.querySelectorAll('.tool-call-summary .shell-cmd-mark');
        expect(marks).toHaveLength(1);
        expect(marks[0].textContent).toBe('<U+202E>');
        const p = open(block);
        expect(p.querySelector('.tool-call-value .shell-cmd-mark').textContent).toBe('<U+202E>');
        expect(p.querySelector('.tool-call-arg-value .shell-cmd-mark').textContent).toBe('␛');
    });

    it('output text is sanitised like shell output (escapes and bidi stripped)', () => {
        const state = feed(newState(), {
            toolCallId: 'a',
            title: 'web_fetch',
            kind: 'fetch',
            rawOutput: { items: [{ Text: `${ESC}[31mred${ESC}[0m ‮ok` }] },
        });
        const p = open(createToolCallBlock(recOf(state, 'a')));
        expect(p.querySelector('.tool-call-text').textContent).toBe('red ok');
    });
});

describe('keyboard and screen readers', () => {
    const argRows = (p) =>
        [...p.querySelectorAll('.tool-call-arg-key')].map((k) => k.textContent);

    it('a `__proto__` argument is listed for every kind of call', () => {
        const proto = (extra) =>
            JSON.parse(`{${extra}"__proto__":{"hidden":"rm -rf ~"},"extra":"shown"}`);
        const cases = [
            { title: 'write', kind: 'edit', rawInput: proto('"path":"/tmp/a","oldStr":"a","newStr":"b",') },
            { title: 'grep', kind: 'search', rawInput: proto('"query":"widgets",') },
            { title: 'read', kind: 'read', rawInput: proto('"path":"/a",') },
            { title: 'web_fetch', kind: 'fetch', rawInput: proto('"url":"https://x.example",') },
            { title: 'Running: @srv/tool', rawInput: proto('"q":"x",') },
        ];
        for (const [i, c] of cases.entries()) {
            const state = feed(newState(), { toolCallId: `c${i}`, ...c });
            const p = open(createToolCallBlock(recOf(state, `c${i}`)));
            expect(argRows(p)).toContain('__proto__');
            expect(argRows(p)).toContain('extra');
        }
    });

    it('argument values and argument JSON scroll boxes take focus and are named by their key', () => {
        const state = feed(newState(), {
            toolCallId: 'm',
            title: 'Running: @srv/tool',
            rawInput: { query: 'x\n'.repeat(40), filters: { a: [1, 2, 3] } },
        });
        const p = open(createToolCallBlock(recOf(state, 'm')));
        const [qKey, fKey] = p.querySelectorAll('.tool-call-arg-key');
        const value = p.querySelector('.tool-call-arg-value');
        expect(value.tabIndex).toBe(0);
        expect(value.getAttribute('role')).toBe('group');
        expect(value.getAttribute('aria-labelledby')).toBe(qKey.id);
        const body = p.querySelector('.tool-call-args .tool-json-body');
        expect(body.tabIndex).toBe(0);
        expect(body.getAttribute('aria-labelledby')).toBe(fKey.id);
        expect(qKey.id).not.toBe(fKey.id);

        // A result's JSON scrolls inside its (focusable) result box instead.
        feed(state, { toolCallId: 'm', status: 'completed', rawOutput: { items: [{ Json: { ok: true } }] } });
        const block = createToolCallBlock(recOf(state, 'm'));
        const p2 = open(block);
        expect(p2.querySelector('.tool-call-output .tool-json-body').hasAttribute('tabindex')).toBe(false);
        expect(p2.querySelector('.tool-call-output').tabIndex).toBe(0);
    });

    it('focus survives a panel rebuild while the call streams', () => {
        const state = feed(newState(), {
            toolCallId: 'm',
            title: 'Running: @srv/tool',
            rawInput: { q: 'x', filters: { a: 1 } },
        });
        const container = row(state);
        chips(container)[0].click();
        const block = blocksIn(listOf(chips(container)[0]))[0];
        const p = open(block);

        // On "Copy arguments".
        p.querySelector('[data-focus="copy:arguments"]').focus();
        feed(state, { toolCallId: 'm', status: 'in_progress', rawOutput: 'partial' });
        syncToolSources(container, state.toolUsages);
        expect(document.activeElement.dataset.focus).toBe('copy:arguments');
        expect(p.contains(document.activeElement)).toBe(true);

        // On a JSON value's summary (the arguments' nested value).
        p.querySelector('.tool-call-args summary').focus();
        feed(state, { toolCallId: 'm', rawOutput: 'more' });
        syncToolSources(container, state.toolUsages);
        expect(document.activeElement.tagName).toBe('SUMMARY');
        expect(document.activeElement.dataset.focus).toBe('json:arg:filters');

        // On the result box.
        p.querySelector('.tool-call-output').focus();
        feed(state, { toolCallId: 'm', status: 'completed', rawOutput: 'done' });
        syncToolSources(container, state.toolUsages);
        expect(document.activeElement.classList.contains('tool-call-output')).toBe(true);

        // What had focus is gone: the block's header takes it, not <body>.
        p.querySelector('[data-focus="copy:result"]').focus();
        feed(state, { toolCallId: 'm', rawOutput: null });
        syncToolSources(container, state.toolUsages);
        expect(p.querySelector('[data-focus="copy:result"]')).toBeNull();
        expect(document.activeElement).toBe(header(block));
    });

    it('a block moved to another chip keeps focus', () => {
        const state = feed(newState(), { toolCallId: 'a', title: 'Running: @srv/tool' });
        const container = row(state);
        chips(container)[0].click();
        const block = blocksIn(listOf(chips(container)[0]))[0];
        header(block).focus();
        feed(state, { toolCallId: 'a', title: 'Running: @other/tool' });
        syncToolCallLists(container, state.toolUsages);
        expect(chips(container)[0].dataset.group).toBe('kind:other');
        expect(document.activeElement).toBe(header(block));
    });
});
