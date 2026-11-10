/**
 * Enter on a path typed faster than its suggestions arrived: the launcher
 * opens the entry the input names (or its only completion) instead of
 * sending the path to the agent; with several candidates it shows them.
 */
import { describe, it, expect, vi } from 'vitest';

const { FloatingApp } = await import('../../ui/js/floating/app.js');

function makeApp(completions) {
    const app = Object.create(FloatingApp.prototype);
    app.appWindow = { hide: vi.fn(async () => {}) };
    app.resetUI = vi.fn();
    app.handleInputChange = vi.fn(async () => {});
    app.invoke = vi.fn(async (cmd) => (cmd === 'complete_path' ? completions : null));
    return app;
}

const DOWNLOADS = { name: 'Downloads', display: '~/Downloads/', path: '/home/alex/Downloads', is_folder: true };
const DOCUMENTS = { name: 'Documents', display: '~/Documents/', path: '/home/alex/Documents', is_folder: true };

describe('Enter on a typed path', () => {
    it('opens the folder the input names exactly', async () => {
        const app = makeApp([DOWNLOADS]);
        expect(await app._openTypedPath('~/Downloads')).toBe(true);
        expect(app.invoke).toHaveBeenCalledWith('open_path', { path: '/home/alex/Downloads' });
        expect(app.appWindow.hide).toHaveBeenCalled();
    });

    it('opens the only completion of a partial path', async () => {
        const app = makeApp([DOWNLOADS]);
        expect(await app._openTypedPath('~/Down')).toBe(true);
        expect(app.invoke).toHaveBeenCalledWith('open_path', { path: '/home/alex/Downloads' });
    });

    it('shows the candidates when there are several, opening none', async () => {
        const app = makeApp([DOCUMENTS, DOWNLOADS]);
        expect(await app._openTypedPath('~/Do')).toBe(true);
        expect(app.invoke).not.toHaveBeenCalledWith('open_path', expect.anything());
        expect(app.handleInputChange).toHaveBeenCalled();
    });

    it('falls through (to the agent) when nothing matches', async () => {
        const app = makeApp([]);
        expect(await app._openTypedPath('~/nothing-here')).toBe(false);
        expect(app.invoke).not.toHaveBeenCalledWith('open_path', expect.anything());
    });
});
