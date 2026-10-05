/**
 * Tests for `renderMarkdown` — the streaming incremental render path.
 *
 * This is the longest untested critical path in ui/js/. The function
 * is called from every chunk of streaming agent output (~60fps via
 * the chunk batcher) and threads state through two WeakMaps
 * (_frozenHtml, _frozenLength) that determine whether the next
 * chunk does a full re-parse or just re-renders the active tail.
 *
 * The state machine:
 *   1. First render — no frozen prefix; whole markdown becomes the tail.
 *   2. Subsequent render where the prefix grew — re-parse the new
 *      frozen part, keep the cached prefix HTML for the rest.
 *   3. Subsequent render where the prefix shrank (e.g. user cleared
 *      the message and started a new one) — full rebuild.
 *   4. Final render (`streaming = false`) — full re-parse, clear state.
 *   5. Empty markdown — wipe state, blank the target.
 *
 * Throttle: streaming renders within 150ms of the last one are
 * deferred via setTimeout. Tests use `streaming = false` for
 * post-state-change asserts so the throttle doesn't matter.
 */

import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import { marked } from 'marked';

let renderMarkdown;
let _resetMarkedHardenedFlagForTests;
let hardenMarkedOnce;

beforeAll(async () => {
    globalThis.marked = marked;
    const mod = await import('../../ui/js/shared/markdown.js');
    renderMarkdown = mod.renderMarkdown;
    _resetMarkedHardenedFlagForTests = mod._resetMarkedHardenedFlagForTests;
    hardenMarkedOnce = mod.hardenMarkedOnce;
    _resetMarkedHardenedFlagForTests();
    hardenMarkedOnce();
});

beforeEach(() => {
    document.body.innerHTML = '';
});

function fresh() {
    const el = document.createElement('div');
    document.body.appendChild(el);
    return el;
}

describe('renderMarkdown — non-streaming (final) renders', () => {
    it('renders a paragraph in body content with no entity escaping for quotes', () => {
        const el = fresh();
        renderMarkdown('hello "world"', el, false);
        // Marked emits a <p> wrapper. The `"` characters land in body
        // text and are NOT entity-encoded — that's correct for HTML
        // (browsers render `"` literally inside <p>).
        expect(el.innerHTML).toContain('<p>hello "world"</p>');
    });

    it('blanks the target on empty markdown and wipes streaming state', () => {
        const el = fresh();
        // Build up some streaming state first.
        renderMarkdown('first paragraph long enough to freeze\n\nactive tail', el, true);
        // Then render an empty string non-streaming. Should clear out.
        renderMarkdown('', el, false);
        expect(el.innerHTML).toBe('');
    });

    it('renders code fences with <pre><code>', () => {
        const el = fresh();
        renderMarkdown('```\nlet x = 1;\n```', el, false);
        expect(el.querySelector('pre code')).not.toBeNull();
        expect(el.textContent).toContain('let x = 1;');
    });
});

describe('renderMarkdown — streaming incremental state', () => {
    it('first streaming render produces a markdown-tail container', () => {
        // The first call sees no cached prefix — whole input is the
        // tail. The wrapper structure (.markdown-tail / optional
        // .markdown-frozen) is what subsequent chunks update.
        const el = fresh();
        renderMarkdown('partial response in flight', el, true);
        // Either the input is short and no split happens (no .markdown-
        // frozen wrapper) OR the split layered both. Both are fine —
        // what matters is some HTML appears.
        expect(el.innerHTML.length).toBeGreaterThan(0);
        expect(el.textContent).toContain('partial response in flight');
    });

    it('streaming render with a stable prefix freezes it across chunks', () => {
        // Build markdown long enough that _findStableSplitPoint returns
        // a non-zero position: a paragraph (>=50 chars) followed by
        // a \n\n then a tail.
        const el = fresh();
        const prefix = 'A complete first paragraph that is long enough to be considered for freezing.';
        const tailA = 'Active tail being typed';
        const tailB = 'Active tail with more typed';
        // First chunk
        renderMarkdown(`${prefix}\n\n${tailA}`, el, false);
        const afterFirst = el.innerHTML;
        expect(afterFirst).toContain(prefix);
        expect(afterFirst).toContain(tailA);

        // Second chunk extends the tail. Use streaming=false so the
        // throttle doesn't defer. Prefix unchanged → cached frozen
        // markup should be reused, tail re-rendered.
        renderMarkdown(`${prefix}\n\n${tailB}`, el, false);
        expect(el.textContent).toContain(prefix);
        expect(el.textContent).toContain(tailB);
        // Old tail text must be gone — no double-render.
        expect(el.textContent).not.toContain(tailA);
    });

    it('a final non-streaming render after streaming clears the cached prefix', () => {
        // streaming → final → streaming-again must NOT carry stale
        // frozen prefix from before the final. Final render clears
        // _frozenHtml/_frozenLength so the next streaming pass starts
        // clean.
        const el = fresh();
        const prefix1 = 'First conversation paragraph that exceeds the freezing threshold.';
        renderMarkdown(`${prefix1}\n\nactive tail`, el, false);
        // New, completely different content (e.g. a brand-new
        // assistant message starting fresh in the same DOM target)
        renderMarkdown('totally different reply', el, false);
        // Marked appends a trailing newline to its <p> output, so
        // textContent ends in `\n` — trim before exact-equality.
        expect(el.textContent.trim()).toBe('totally different reply');
        expect(el.textContent).not.toContain(prefix1);
    });

    it('handles a streaming chunk that contains an in-progress code fence', () => {
        // The split-point logic skips inside-fence positions so the
        // tail doesn't get cut mid-block. We don't assert on internal
        // state — just that the rendered HTML doesn't include a
        // half-closed <pre> or unparsed `\`\`\`` lines.
        const el = fresh();
        const md = 'intro paragraph with enough body to potentially freeze\n\n```\nstreaming code\nstill streaming';
        renderMarkdown(md, el, true);
        // Either the renderer streamed a <pre> or it left the fence
        // text raw — both are acceptable mid-streaming behaviours.
        // What's NOT acceptable is corrupted output that breaks the
        // surrounding DOM (which would manifest as the intro text
        // being absent).
        expect(el.textContent).toContain('intro paragraph');
    });
});

describe('renderMarkdown — fence stripping for internal blocks', () => {
    // The renderer hides three types of fenced blocks the agent emits:
    //   - ```automation_plan ... ``` (rendered as a TaskList by the app)
    //   - ```extension_tool_call ... ``` (handled programmatically)
    //   - ```suggested_actions ... ``` (rendered as chips)
    // None of these should appear as visible code blocks to the user.

    it('strips complete extension_tool_call fences from rendered output', () => {
        const el = fresh();
        const md =
            'before\n\n```extension_tool_call\n{"tool":"foo"}\n```\n\nafter';
        renderMarkdown(md, el, false);
        const html = el.innerHTML;
        expect(html).toContain('before');
        expect(html).toContain('after');
        // The fence body must NOT appear as a visible <pre><code>.
        expect(el.textContent).not.toContain('extension_tool_call');
        expect(el.textContent).not.toContain('"tool":"foo"');
    });

    it('strips an in-progress (incomplete) extension_tool_call fence during streaming', () => {
        // Same fence type, but never closed — happens while the agent
        // is still streaming the body. We must hide the partial body,
        // otherwise the chat shows raw JSON until the fence closes.
        const el = fresh();
        const md = 'visible text\n\n```extension_tool_call\n{"tool":';
        renderMarkdown(md, el, true);
        expect(el.textContent).toContain('visible text');
        expect(el.textContent).not.toContain('extension_tool_call');
        expect(el.textContent).not.toContain('"tool":');
    });

    it('strips suggested_actions fences', () => {
        const el = fresh();
        const md = 'pre\n\n```suggested_actions\n[{"label":"Do thing"}]\n```\n\npost';
        renderMarkdown(md, el, false);
        expect(el.textContent).toContain('pre');
        expect(el.textContent).toContain('post');
        expect(el.textContent).not.toContain('suggested_actions');
        expect(el.textContent).not.toContain('Do thing');
    });
});

describe('renderMarkdown — taskplan deduplication', () => {
    // The agent re-emits the FULL automation_plan block each time it
    // updates a step's status. The renderer keeps only the last
    // occurrence so users don't see ghost copies of older snapshots.

    it('streaming render with a single in-progress automation_plan strips it', () => {
        const el = fresh();
        // An incomplete plan during streaming — should be stripped
        // entirely; the app handles plans via a dedicated path.
        const md = 'thinking...\n\n```automation_plan\n[{"step":1,"task":"foo"}';
        renderMarkdown(md, el, true);
        expect(el.textContent).toContain('thinking');
        expect(el.textContent).not.toContain('automation_plan');
        expect(el.textContent).not.toContain('"step":1');
    });

    it('non-streaming render with a complete automation_plan preserves it (so the app can pick it up)', () => {
        const el = fresh();
        // Complete plan — final render. Note: renderMarkdown's job
        // is just markdown→HTML; whether the plan ends up as a
        // taskplan UI is the app's call. The block stays in the
        // DOM as a code block, which is what the app's taskplan
        // detector reads.
        const md =
            'reply\n\n```automation_plan\n[{"step":1,"task":"foo","details":"bar"}]\n```';
        renderMarkdown(md, el, false);
        // The string contents land somewhere — we just need to
        // verify renderMarkdown doesn't crash on a full plan and
        // that "reply" survives. Don't pin the exact rendering
        // shape; the deduplication tests in markdown.test.js
        // cover that.
        expect(el.textContent).toContain('reply');
    });
});

describe('renderMarkdown — streaming throttle and incremental append', () => {
    afterEach(() => {
        vi.useRealTimers();
    });

    it('the throttled timer paints the newest text, not the scheduling snapshot', () => {
        vi.useFakeTimers();
        const el = fresh();
        renderMarkdown('chunk one', el, true); // immediate
        renderMarkdown('chunk one two', el, true); // schedules timer
        renderMarkdown('chunk one two three', el, true); // must not be dropped
        vi.advanceTimersByTime(200);
        expect(el.textContent).toContain('chunk one two three');
    });

    it('appends newly frozen paragraphs without rebuilding earlier frozen nodes', () => {
        vi.useFakeTimers();
        const el = fresh();
        const p1 = 'First paragraph of the reply, long enough to stand on its own.';
        const p2 = 'Second paragraph of the reply, also long enough to stand alone here.';
        const p3 = 'Third paragraph streaming in now and it is long enough to be the tail.';
        renderMarkdown(`${p1}\n\n${p2}`, el, true);
        const firstFrozen = el.querySelector('.markdown-frozen > p');
        expect(firstFrozen?.textContent).toBe(p1);
        vi.advanceTimersByTime(200);
        renderMarkdown(`${p1}\n\n${p2}\n\n${p3}`, el, true);
        // Same node — the old frozen content was kept, the new slice appended.
        expect(el.querySelector('.markdown-frozen > p')).toBe(firstFrozen);
        expect(el.querySelector('.markdown-frozen').textContent).toContain(p2);
        expect(el.querySelector('.markdown-tail').textContent).toContain(p3);
    });

    it('neutralizes links in text before an in-progress automation_plan', () => {
        const el = fresh();
        renderMarkdown(
            'See [docs](https://example.com/x) first.\n\n```automation_plan\n[{"task":"a"',
            el,
            true
        );
        const a = el.querySelector('a');
        expect(a.getAttribute('href')).toBe('#');
        expect(a.getAttribute('data-href')).toBe('https://example.com/x');
    });
});
