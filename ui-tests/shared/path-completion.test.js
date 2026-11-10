/**
 * Path completion in the launcher: typing `~/Down` offers `~/Downloads/`
 * from disk (the backend's `complete_path`), ranked first, with Tab filling
 * the completed path in (`name`), and a lone `/word` left to slash commands.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

let unifiedSearch;
let looksLikePath;

beforeEach(async () => {
    vi.resetModules();
    vi.doMock('../../ui/js/shared/commands.js', () => ({
        matchCommands: vi.fn(() => []),
        matchSlashCommands: vi.fn(() => [{ name: '/model', description: 'Switch model' }]),
        matchCommandsByName: vi.fn(() => []),
    }));
    const mod = await import('../../ui/js/shared/search-engine.js');
    unifiedSearch = mod.unifiedSearch;
    looksLikePath = mod.looksLikePath;
});

const COMPLETIONS = [
    { name: 'Downloads', display: '~/Downloads/', path: '/home/alex/Downloads', is_folder: true },
    { name: 'download.pdf', display: '~/download.pdf', path: '/home/alex/download.pdf', is_folder: false },
];

function invokeWith(completions) {
    return vi.fn(async (cmd) => {
        if (cmd === 'complete_path') return completions;
        if (cmd === 'search_files') return [{ name: 'other.pdf', path: '/x/other.pdf', is_folder: false }];
        return [];
    });
}

describe('looksLikePath', () => {
    it('takes home, drive and multi-segment absolute paths', () => {
        for (const q of ['~', '~/', '~/Down', '~\\Docs', 'C:\\Users', 'd:/x', '/Users/', '/Applications/Sl']) {
            expect(looksLikePath(q), q).toBe(true);
        }
    });

    it('leaves slash commands, network paths and prose alone', () => {
        for (const q of ['', '/model', '/', '//server/share', '\\\\server\\share', '~user', 'open ~/x please', 'a\nb']) {
            expect(looksLikePath(q), q).toBe(false);
        }
    });
});

describe('unifiedSearch path completion', () => {
    it('lists completions first, as file rows Tab can fill in', async () => {
        const invoke = invokeWith(COMPLETIONS);
        const results = await unifiedSearch('~/Down', invoke, []);
        expect(invoke).toHaveBeenCalledWith('complete_path', { prefix: '~/Down' });
        const top = results[0];
        expect(top).toMatchObject({
            type: 'file',
            label: '~/Downloads/',
            name: '~/Downloads/',
            icon: '📁',
            data: { path: '/home/alex/Downloads', is_folder: true },
        });
        expect(results[1].label).toBe('~/download.pdf');
    });

    it('skips the indexed file search for path-shaped input', async () => {
        const invoke = invokeWith(COMPLETIONS);
        await unifiedSearch('~/download.pdf', invoke, []);
        expect(invoke.mock.calls.some(([cmd]) => cmd === 'search_files')).toBe(false);
    });

    it('completes an absolute path instead of matching slash commands', async () => {
        const invoke = invokeWith([
            { name: 'Slack.app', display: '/Applications/Slack.app/', path: '/Applications/Slack.app', is_folder: true },
        ]);
        const results = await unifiedSearch('/Applications/Sl', invoke, []);
        expect(results[0].label).toBe('/Applications/Slack.app/');
        expect(results.some((r) => r.type === 'slash')).toBe(false);
    });

    it('keeps slash commands for a lone /word', async () => {
        const invoke = invokeWith(COMPLETIONS);
        const results = await unifiedSearch('/mod', invoke, []);
        expect(invoke.mock.calls.some(([cmd]) => cmd === 'complete_path')).toBe(false);
        expect(results.some((r) => r.type === 'slash')).toBe(true);
    });

    it('drops the duplicate "Open Folder" row for a typed folder, but ranks it first after a separator', async () => {
        const pathRow = JSON.stringify([{ type: 'path', value: '/home/alex/Downloads', pathType: 'folder', score: 87 }]);
        const invoke = vi.fn(async (cmd) => {
            if (cmd === 'complete_path') return COMPLETIONS;
            if (cmd === 'handle_floating_input') return pathRow;
            return [];
        });
        const noSep = await unifiedSearch('~/Downloads', invoke, []);
        expect(noSep.some((r) => r.type === 'path')).toBe(false);
        expect(noSep[0].label).toBe('~/Downloads/');

        const withSep = await unifiedSearch('~/Downloads/', invoke, []);
        expect(withSep[0].type).toBe('path');
    });

    it('shows nothing extra when completion fails', async () => {
        const invoke = vi.fn(async (cmd) => {
            if (cmd === 'complete_path') throw new Error('boom');
            return [];
        });
        const results = await unifiedSearch('~/Down', invoke, []);
        expect(results.filter((r) => r.type === 'file')).toEqual([]);
    });
});
