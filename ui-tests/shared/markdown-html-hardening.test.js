/**
 * P1.8: marked.js raw-HTML hardening.
 *
 * Pre-fix, markdown.js had a 200-char heuristic that wrapped a *whole*
 * HTML document in an html code fence — but only when <html>/<script>/etc.
 * was the first non-whitespace token. An LLM emitting "Here is the page:
 * \n\n<script>alert(1)</script>" slipped past entirely and marked passed
 * the raw <script> tag straight through to innerHTML.
 *
 * Fix: install a marked renderer.html override at module load that escapes
 * every raw HTML token (block-level and inline) to text. Fenced code
 * blocks (```html ... ```) are routed through renderer.code, not
 * renderer.html, so they still render as syntax-highlighted source.
 *
 * These tests load real `marked` via the test devDependency, set it on the
 * jsdom window so markdown.js's `typeof marked` guard sees it, then arm the
 * hardening by importing the module and calling hardenMarkedOnce().
 */

import { describe, it, expect, beforeAll } from 'vitest';
import { marked } from 'marked';

let hardenMarkedOnce;
let _resetMarkedHardenedFlagForTests;

beforeAll(async () => {
    // marked has to be globally visible before markdown.js loads — its
    // module-init hardening call uses `typeof marked === 'undefined'` as
    // the guard, so a missing global silently disables the override.
    globalThis.marked = marked;
    const mod = await import('../../ui/js/shared/markdown.js');
    hardenMarkedOnce = mod.hardenMarkedOnce;
    _resetMarkedHardenedFlagForTests = mod._resetMarkedHardenedFlagForTests;

    // markdown.js's top-level call may have run before we set the global.
    // Re-arm and run hardening explicitly.
    _resetMarkedHardenedFlagForTests();
    hardenMarkedOnce();
});

describe('marked raw-HTML hardening', () => {
    it('escapes a <script> tag emitted mid-paragraph', () => {
        // The case the pre-fix 200-char heuristic missed: HTML appearing
        // *after* legitimate prose. The script must come out as escaped
        // text, never as a live tag.
        const md = "Here is the page:\n\n<script>alert('xss')</script>";
        const out = marked.parse(md);
        expect(out).not.toContain('<script>');
        expect(out).toContain('&lt;script&gt;');
        // The script's body must appear as plain text — no live tags.
        // (The single quotes don't need entity-escaping; what matters is
        // that the surrounding <script> isn't a live element.)
        expect(out).toContain("alert(");
    });

    it('escapes inline raw HTML mixed with markdown', () => {
        const md = "Hello **world** with an inline <img src=x onerror=alert(1)> tag";
        const out = marked.parse(md);
        expect(out).not.toMatch(/<img\s/i);
        expect(out).toContain('&lt;img');
        // Markdown emphasis is preserved — the override only touches raw HTML.
        expect(out).toContain('<strong>world</strong>');
    });

    it('escapes <style> blocks that would otherwise inject CSS', () => {
        const md = "Para before.\n\n<style>body{display:none}</style>\n\nPara after.";
        const out = marked.parse(md);
        expect(out).not.toMatch(/<style[\s>]/i);
        expect(out).toContain('&lt;style&gt;');
        expect(out).toContain('body{display:none}');
    });

    it('escapes a full HTML document pasted mid-response', () => {
        // Full-document case but not at offset 0 — the legacy 200-char
        // first-token regex would not catch this, the renderer.html
        // override must.
        const md = "I generated:\n\n<!DOCTYPE html><html><body><script>x</script></body></html>";
        const out = marked.parse(md);
        expect(out).not.toMatch(/<script[\s>]/i);
        expect(out).not.toMatch(/<body[\s>]/i);
        expect(out).toContain('&lt;script&gt;');
    });

    it('preserves fenced html code blocks as rendered source (not stripped)', () => {
        // ```html\n<script>...\n``` is a code block, not raw HTML — it
        // routes through renderer.code, not renderer.html. The override
        // must NOT touch it (otherwise we'd ruin syntax-highlighted code
        // listings of HTML examples).
        const md = "```html\n<script>alert(1)</script>\n```";
        const out = marked.parse(md);
        // The script content lives inside a <pre><code> wrapper as
        // escaped text — that's marked's normal code-block behavior, not
        // our override. The wrapper must be present.
        expect(out).toMatch(/<pre>/);
        expect(out).toMatch(/<code/);
        // The literal <script> source appears as escaped text inside the
        // code block, not as a live element.
        expect(out).not.toMatch(/<script[\s>]/i);
        expect(out).toContain('&lt;script&gt;');
    });

    it('handles raw HTML inside a list item', () => {
        const md = "- safe item\n- evil <iframe src=x></iframe> item";
        const out = marked.parse(md);
        expect(out).not.toMatch(/<iframe[\s>]/i);
        expect(out).toContain('&lt;iframe');
        // The list structure itself must survive.
        expect(out).toMatch(/<ul>/);
        expect(out).toMatch(/<li>/);
    });

    it('handles a <script> tag at the very start (pre-fix-also-covered case)', () => {
        // The 200-char fence-wrap heuristic in _doRender catches this for
        // display polish, but the renderer.html override is the security
        // guarantee — verify it works with marked.parse alone, no _doRender.
        const md = "<script>alert(1)</script>";
        const out = marked.parse(md);
        expect(out).not.toMatch(/<script[\s>]/i);
        expect(out).toContain('&lt;script&gt;');
    });

    it('hardenMarkedOnce is idempotent', () => {
        // Calling hardenMarkedOnce a second time must not double-wrap the
        // override (marked.use stacks overrides if applied repeatedly).
        // After two calls, raw HTML should still escape — not produce
        // double-escaped output.
        hardenMarkedOnce();
        hardenMarkedOnce();
        const out = marked.parse("<b>x</b>");
        // Either: "&lt;b&gt;x&lt;/b&gt;" (single escape, correct)
        // Not: "&amp;lt;b&amp;gt;..." (double escape would mean stacked).
        expect(out).toContain('&lt;b&gt;');
        expect(out).not.toContain('&amp;lt;');
    });
});
