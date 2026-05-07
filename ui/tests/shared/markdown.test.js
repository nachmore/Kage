/**
 * Coverage for the streaming-incremental and taskplan logic in markdown.js
 * — the bits the audit (P3.6) called out as zero-coverage.
 *
 * The 2026-04 OOM regression came from `_findStableSplitPoint` returning a
 * split inside a code fence; this file pins the safe-split contract so a
 * future refactor can't regress it silently. `_keepLastTaskPlan` and
 * `parseTaskPlan` are also covered here — they're the source-level
 * deduplication and parsing layer for the taskplan UI.
 *
 * Loads marked via the test devDependency and pre-arms the renderer.html
 * hardening so renderMarkdown's hot path works the same way as in the
 * browser (matches the setup in markdown-html-hardening.test.js).
 */

import { describe, it, expect, beforeAll } from 'vitest';
import { marked } from 'marked';

let _findStableSplitPoint;
let _keepLastTaskPlan;
let parseTaskPlan;
let createTaskPlanElement;
let _resetMarkedHardenedFlagForTests;
let hardenMarkedOnce;

beforeAll(async () => {
    globalThis.marked = marked;
    const mod = await import('../../js/shared/markdown.js');
    _findStableSplitPoint = mod._findStableSplitPoint;
    _keepLastTaskPlan = mod._keepLastTaskPlan;
    parseTaskPlan = mod.parseTaskPlan;
    createTaskPlanElement = mod.createTaskPlanElement;
    _resetMarkedHardenedFlagForTests = mod._resetMarkedHardenedFlagForTests;
    hardenMarkedOnce = mod.hardenMarkedOnce;

    _resetMarkedHardenedFlagForTests();
    hardenMarkedOnce();
});

// ---- _findStableSplitPoint --------------------------------------------------

describe('_findStableSplitPoint', () => {
    it('returns 0 when input is short — not worth freezing', () => {
        // Cutoff in the impl: don't freeze if the tail would be shorter
        // than 50 chars. A small message has nothing to freeze.
        expect(_findStableSplitPoint('hi')).toBe(0);
        expect(_findStableSplitPoint('one\n\ntwo')).toBe(0);
    });

    it('returns 0 when there is no double-newline', () => {
        const md = 'a single paragraph that just keeps going '.repeat(5);
        expect(_findStableSplitPoint(md)).toBe(0);
    });

    it('splits at the last \\n\\n outside of a fence', () => {
        const prefix = 'first paragraph done\n\n';
        const tail = 'this is the active tail and it is at least fifty characters long for sure';
        const md = prefix + tail;
        const idx = _findStableSplitPoint(md);
        expect(idx).toBe(prefix.length);
        expect(md.substring(idx)).toBe(tail);
    });

    it('does NOT split inside an open code fence', () => {
        // The whole point: \n\n inside an unclosed ``` block is not a safe
        // freeze point — re-parsing the tail alone would render the
        // language label as ordinary text mid-stream.
        const md = [
            'intro paragraph',
            '',
            '```python',
            'def foo():',
            '',  // blank line *inside* the fence
            '    pass',
        ].join('\n');
        const idx = _findStableSplitPoint(md);
        // Only the safe \n\n before the fence is allowed. The inside-fence
        // \n\n must be ignored.
        const safeBefore = md.indexOf('\n\n```python') + 2;
        // If the impl freezes anywhere ≥ safeBefore + 'python' fence open, that's bad.
        expect(idx).toBeLessThanOrEqual(safeBefore);
    });

    it('allows splitting AFTER a closed fence', () => {
        // A complete fence is a stable block — markdown after it can be
        // safely frozen with the fence in the prefix.
        const md = [
            'intro',
            '',
            '```python',
            'pass',
            '```',
            '',
            'paragraph after the fence is long enough not to trip the 50-char cutoff at the end',
        ].join('\n');
        const idx = _findStableSplitPoint(md);
        expect(idx).toBeGreaterThan(0);
        // Everything before idx contains a complete fenced block.
        const prefix = md.substring(0, idx);
        const fenceCount = (prefix.match(/```/g) || []).length;
        expect(fenceCount % 2).toBe(0); // balanced
    });

    it('returns 0 when the only \\n\\n is too close to the end', () => {
        // 50-char cutoff means even a real \n\n won't trigger if the tail
        // is too short — pointless to freeze for that little.
        const md = 'big chunky frozen prefix going on and on and on\n\nshort';
        expect(_findStableSplitPoint(md)).toBe(0);
    });
});

// ---- _keepLastTaskPlan ------------------------------------------------------

describe('_keepLastTaskPlan', () => {
    it('passes through markdown without taskplan blocks unchanged', () => {
        const md = '## Just a heading\n\nWith a paragraph.';
        expect(_keepLastTaskPlan(md)).toBe(md);
    });

    it('removes all but the last taskplan block', () => {
        // Agent re-emits the full block on every update — older blocks
        // are stale and must be stripped at the source level.
        const md = [
            '```taskplan',
            '[pending] step one',
            '[pending] step two',
            '```',
            '',
            'some chatter',
            '',
            '```taskplan',
            '[done] step one | finished',
            '[active] step two',
            '```',
        ].join('\n');
        const out = _keepLastTaskPlan(md);
        const blockMatches = out.match(/```taskplan/g) || [];
        expect(blockMatches.length).toBe(1);
        expect(out).toContain('[done] step one');
        expect(out).toContain('[active] step two');
        // The earlier "[pending] step one" line came from the stale block,
        // so it should be gone.
        expect(out).not.toContain('[pending] step one');
    });

    it('applies inline step markers to the surviving block', () => {
        // The agent emits `[step N status]` markers between rebuilds; this
        // helper folds those into the block so the rendered taskplan
        // updates in place without waiting for the next full re-emit.
        const md = [
            '```taskplan',
            '[pending] build the thing',
            '[pending] ship the thing',
            '```',
            '',
            '`[step 1 done]` shipped successfully',
        ].join('\n');
        const out = _keepLastTaskPlan(md);
        expect(out).toContain('[done] build the thing | shipped successfully');
        // The marker itself is stripped from the output (it's not user-facing).
        expect(out).not.toContain('[step 1 done]');
    });

    it('handles same-line active+done markers (latest wins)', () => {
        // The doc-string for this helper specifically calls out same-line
        // dual markers like `[step 1 active]` Launching...`[step 1 done]`.
        const md = [
            '```taskplan',
            '[pending] launch app',
            '```',
            '',
            '`[step 1 active]` Launching...`[step 1 done]` Word launched',
        ].join('\n');
        const out = _keepLastTaskPlan(md);
        // Latest wins → done, with the trailing detail.
        expect(out).toContain('[done] launch app');
        expect(out).toMatch(/\[done\] launch app \| Word launched/);
    });

    it('strips a leading "ack" leakage from steering responses', () => {
        // Steering response sometimes leaks an "ack" prefix into the stream;
        // the helper drops it so the leading taskplan fence is at column 0.
        const md = 'ack```taskplan\n[pending] go\n```';
        const out = _keepLastTaskPlan(md);
        expect(out.startsWith('```taskplan')).toBe(true);
    });

    it('returns input unchanged when there is no markers and only one block', () => {
        const md = '```taskplan\n[pending] only step\n```';
        expect(_keepLastTaskPlan(md)).toBe(md);
    });
});

// ---- parseTaskPlan ----------------------------------------------------------

describe('parseTaskPlan', () => {
    it('parses a basic three-status plan', () => {
        const text = [
            '[done] first | done detail',
            '[active] second',
            '[pending] third',
        ].join('\n');
        const tasks = parseTaskPlan(text);
        expect(tasks).toEqual([
            { status: 'done', description: 'first', detail: 'done detail' },
            { status: 'active', description: 'second', detail: '' },
            { status: 'pending', description: 'third', detail: '' },
        ]);
    });

    it('skips lines that do not match the expected shape', () => {
        const text = 'this is not a valid line\n[pending] this one is';
        const tasks = parseTaskPlan(text);
        expect(tasks).toHaveLength(1);
        expect(tasks[0].description).toBe('this one is');
    });

    it('ignores empty lines and surrounding whitespace', () => {
        const text = '\n  \n[pending] alpha\n\n[done] beta | with detail\n  \n';
        const tasks = parseTaskPlan(text);
        expect(tasks).toHaveLength(2);
        expect(tasks[0].status).toBe('pending');
        expect(tasks[1].detail).toBe('with detail');
    });

    it('returns an empty array for blank or no-matches input', () => {
        expect(parseTaskPlan('')).toEqual([]);
        expect(parseTaskPlan('not a single match here')).toEqual([]);
    });
});

// ---- createTaskPlanElement --------------------------------------------------

describe('createTaskPlanElement', () => {
    it('renders one item per task with the right status class', () => {
        const wrapper = createTaskPlanElement([
            { status: 'done', description: 'first', detail: '' },
            { status: 'active', description: 'second', detail: '' },
            { status: 'pending', description: 'third', detail: '' },
        ]);
        const items = wrapper.querySelectorAll('.taskplan-item');
        expect(items).toHaveLength(3);
        expect(items[0].classList.contains('taskplan-done')).toBe(true);
        expect(items[1].classList.contains('taskplan-active')).toBe(true);
        expect(items[2].classList.contains('taskplan-pending')).toBe(true);
    });

    it('marks done-with-detail items as collapsible and collapsed by default', () => {
        const wrapper = createTaskPlanElement([
            { status: 'done', description: 'first', detail: 'collapsed by default' },
            { status: 'done', description: 'second', detail: '' }, // no detail → not collapsible
        ]);
        const items = wrapper.querySelectorAll('.taskplan-item');
        expect(items[0].classList.contains('taskplan-collapsible')).toBe(true);
        expect(items[0].classList.contains('taskplan-collapsed')).toBe(true);
        expect(items[1].classList.contains('taskplan-collapsible')).toBe(false);
    });

    it('records progress as "done/total" on the wrapper dataset', () => {
        const wrapper = createTaskPlanElement([
            { status: 'done', description: 'a', detail: '' },
            { status: 'done', description: 'b', detail: '' },
            { status: 'pending', description: 'c', detail: '' },
        ]);
        expect(wrapper.dataset.progress).toBe('2/3');
    });

    it('escapes HTML in descriptions and details', () => {
        // Defensive — the agent's description text shouldn't break out of
        // the wrapper. Without escaping, an inline <img onerror> would fire.
        const wrapper = createTaskPlanElement([
            { status: 'pending', description: '<img src=x onerror=alert(1)>', detail: '' },
        ]);
        const html = wrapper.innerHTML;
        expect(html).not.toMatch(/<img\s/i);
        expect(html).toContain('&lt;img');
    });

    it('renders cancelled flag with a "Cancelled by user" tag', () => {
        const wrapper = createTaskPlanElement([
            { status: 'stopped', description: 'aborted', detail: '', cancelled: true },
        ]);
        expect(wrapper.querySelector('.taskplan-cancelled')).not.toBeNull();
        expect(wrapper.querySelector('.taskplan-cancelled').textContent).toContain('Cancelled');
    });
});
