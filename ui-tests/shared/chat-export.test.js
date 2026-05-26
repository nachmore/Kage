import { describe, expect, it } from 'vitest';
import { buildChatMarkdown, defaultExportFilename } from '../../ui/js/shared/chat-export.js';

describe('buildChatMarkdown', () => {
    it('renders a basic two-message exchange', () => {
        const md = buildChatMarkdown({
            messages: [
                { role: 'user', content: 'hello' },
                { role: 'assistant', content: 'hi' },
            ],
            title: 'My chat',
            model: 'gpt-test',
            sessionId: 'abcd1234efgh',
            exportedAt: '2026-05-26',
        });
        expect(md).toContain('# My chat');
        expect(md).toContain('_Exported on 2026-05-26 · gpt-test · session abcd1234_');
        expect(md).toContain('## You');
        expect(md).toContain('hello');
        expect(md).toContain('## Kage');
        expect(md).toContain('hi');
    });

    it('falls back to "Untitled chat" when title is empty', () => {
        const md = buildChatMarkdown({ messages: [], title: '   ', exportedAt: '2026-05-26' });
        expect(md).toContain('# Untitled chat');
    });

    it('skips empty / whitespace-only messages', () => {
        const md = buildChatMarkdown({
            messages: [
                { role: 'user', content: '' },
                { role: 'assistant', content: '   ' },
                { role: 'user', content: 'real' },
            ],
            exportedAt: '2026-05-26',
        });
        // Only one heading should appear since the first two are dropped.
        const headings = md.match(/^## /gm) || [];
        expect(headings.length).toBe(1);
        expect(md).toContain('real');
    });

    it('handles unknown roles with a generic header', () => {
        const md = buildChatMarkdown({
            messages: [{ role: 'tool', content: 'output' }],
            exportedAt: '2026-05-26',
        });
        expect(md).toContain('## tool');
    });

    it('preserves markdown in assistant content as-is', () => {
        const md = buildChatMarkdown({
            messages: [{ role: 'assistant', content: '```js\nconst x = 1;\n```' }],
            exportedAt: '2026-05-26',
        });
        expect(md).toContain('```js\nconst x = 1;\n```');
    });

    it('omits model + session when not provided', () => {
        const md = buildChatMarkdown({
            messages: [{ role: 'user', content: 'hi' }],
            title: 'T',
            exportedAt: '2026-05-26',
        });
        expect(md).toContain('_Exported on 2026-05-26_');
        expect(md).not.toContain('session');
        expect(md).not.toContain('·');
    });

    it('terminates with exactly one trailing newline', () => {
        const md = buildChatMarkdown({
            messages: [{ role: 'user', content: 'hi' }],
            title: 'T',
            exportedAt: '2026-05-26',
        });
        expect(md.endsWith('\n')).toBe(true);
        expect(md.endsWith('\n\n')).toBe(false);
    });

    it('handles a missing/non-array messages input', () => {
        const md = buildChatMarkdown({ title: 'T', exportedAt: '2026-05-26' });
        expect(md).toContain('# T');
        expect(md).not.toContain('## You');
    });
});

describe('defaultExportFilename', () => {
    it('builds a kebab-friendly filename with the .md extension', () => {
        expect(defaultExportFilename('My Chat About Rust')).toBe('My Chat About Rust.md');
    });

    it('falls back to "kage-chat.md" for empty / blank input', () => {
        expect(defaultExportFilename('')).toBe('kage-chat.md');
        expect(defaultExportFilename('   ')).toBe('kage-chat.md');
    });

    it('strips Windows-reserved characters', () => {
        const out = defaultExportFilename('what / is "my" <stuff>?');
        expect(out).not.toMatch(/[<>:"/\\|?*]/);
        expect(out.endsWith('.md')).toBe(true);
    });

    it('truncates very long titles', () => {
        const longTitle = 'x'.repeat(200);
        const out = defaultExportFilename(longTitle);
        // 80-char base + ".md"
        expect(out.length).toBeLessThanOrEqual(83);
        expect(out.endsWith('.md')).toBe(true);
    });

    it('keeps unicode letters', () => {
        // Japanese / Chinese / Arabic titles should round-trip — only
        // OS-reserved punctuation gets stripped.
        expect(defaultExportFilename('日本語チャット')).toBe('日本語チャット.md');
    });
});
