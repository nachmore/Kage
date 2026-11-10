/**
 * Field-level saves (docs/design/SYNC.md §9.5): a Settings section writes
 * only what the user changed, so a value another window or another
 * computer changed meanwhile survives the Save; lists are compared by
 * `sync_id`; and every save names the revision it started from.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import {
    applyPaths,
    diffPaths,
    settingPaths,
    valueAt,
} from '../../ui/js/settings/field-save.js';

const clone = (v) => structuredClone(v);

/** Save the way the manager does: diff against what was loaded, apply to fresh. */
function fieldSave(loaded, edit, fresh) {
    const before = clone(loaded);
    const edited = clone(before);
    edit(edited);
    return applyPaths(clone(fresh), edited, diffPaths(before, edited));
}

describe('diffPaths and applyPaths', () => {
    it('keep a remote change to a field the user did not touch', () => {
        const loaded = { ui: { theme: 'dark', font_size: 14 }, hotkey: { modifiers: ['Alt'], key: 'Space' } };
        const fresh = { ...clone(loaded), ui: { theme: 'light', font_size: 14 } };
        const out = fieldSave(loaded, (c) => (c.ui.font_size = 16), fresh);
        expect(out.ui).toEqual({ theme: 'light', font_size: 16 });
        expect(out.hotkey).toEqual(loaded.hotkey);
    });

    it('report nothing when the user changed nothing', () => {
        const loaded = { ui: { theme: 'dark' }, shortcuts: [{ sync_id: 's1', name: 'a' }] };
        expect(diffPaths(loaded, clone(loaded))).toEqual([]);
    });

    it('delete a key the user removed, and add one the user set', () => {
        const loaded = { extensions: { mm: { a: 1, b: 2 } } };
        const out = fieldSave(
            loaded,
            (c) => {
                delete c.extensions.mm.a;
                c.extensions.mm.c = 3;
            },
            { extensions: { mm: { a: 1, b: 5 } } }
        );
        expect(out.extensions.mm).toEqual({ b: 5, c: 3 });
    });

    it('replace an array without ids as one value', () => {
        const loaded = { system: { capture_selection_blocklist: ['a', 'b'] } };
        const out = fieldSave(
            loaded,
            (c) => c.system.capture_selection_blocklist.push('c'),
            { system: { capture_selection_blocklist: ['x'] } }
        );
        expect(out.system.capture_selection_blocklist).toEqual(['a', 'b', 'c']);
    });

    describe('lists by sync_id', () => {
        const list = () => [
            { sync_id: 's1', name: 'one', url: 'u1' },
            { sync_id: 's2', name: 'two', url: 'u2' },
            { sync_id: 's3', name: 'three', url: 'u3' },
        ];

        it('editing one item leaves the others as they are now', () => {
            const loaded = { shortcuts: list() };
            const fresh = { shortcuts: list() };
            fresh.shortcuts[2].name = 'three (renamed elsewhere)';
            const out = fieldSave(loaded, (c) => (c.shortcuts[0].url = 'u1-new'), fresh);
            expect(out.shortcuts.map((s) => [s.sync_id, s.name, s.url])).toEqual([
                ['s1', 'one', 'u1-new'],
                ['s2', 'two', 'u2'],
                ['s3', 'three (renamed elsewhere)', 'u3'],
            ]);
            const changes = diffPaths(loaded, { shortcuts: [{ ...list()[0], url: 'x' }, ...list().slice(1)] });
            expect(changes).toEqual([{ path: ['shortcuts', { id: 's1' }, 'url'], op: 'set' }]);
        });

        it('two fields of one item merge', () => {
            const loaded = { shortcuts: list() };
            const fresh = { shortcuts: list() };
            fresh.shortcuts[0].name = 'renamed elsewhere';
            const out = fieldSave(loaded, (c) => (c.shortcuts[0].url = 'mine'), fresh);
            expect(out.shortcuts[0]).toEqual({ sync_id: 's1', name: 'renamed elsewhere', url: 'mine' });
        });

        it('a removed item is removed by id, even when the list moved', () => {
            const loaded = { shortcuts: list() };
            const fresh = { shortcuts: [{ sync_id: 's0', name: 'new elsewhere' }, ...list()] };
            const out = fieldSave(loaded, (c) => c.shortcuts.splice(1, 1), fresh);
            expect(out.shortcuts.map((s) => s.sync_id)).toEqual(['s0', 's1', 's3']);
        });

        it('a new item goes in after its neighbour; an item added elsewhere stays', () => {
            const loaded = { shortcuts: list() };
            const fresh = { shortcuts: [...list(), { sync_id: 's4', name: 'four' }] };
            const out = fieldSave(
                loaded,
                (c) => c.shortcuts.splice(1, 0, { name: 'mine' }),
                fresh
            );
            expect(out.shortcuts.map((s) => s.sync_id || s.name)).toEqual([
                's1',
                'mine',
                's2',
                's3',
                's4',
            ]);
        });

        it('a reorder moves only the items the user saw', () => {
            const loaded = { shortcuts: list() };
            const fresh = { shortcuts: [list()[0], { sync_id: 'sX', name: 'x' }, list()[1], list()[2]] };
            const out = fieldSave(loaded, (c) => c.shortcuts.reverse(), fresh);
            expect(out.shortcuts.map((s) => s.sync_id)).toEqual(['s3', 'sX', 's2', 's1']);
        });

        it('an item deleted elsewhere while the user edited it comes back with the edit', () => {
            const loaded = { shortcuts: list() };
            const fresh = { shortcuts: list().filter((s) => s.sync_id !== 's2') };
            const out = fieldSave(loaded, (c) => (c.shortcuts[1].url = 'kept'), fresh);
            expect(out.shortcuts.map((s) => [s.sync_id, s.url])).toEqual([
                ['s1', 'u1'],
                ['s2', 'kept'],
                ['s3', 'u3'],
            ]);
        });

        it('agents and internal sites use their `id`', () => {
            const loaded = { acp: { connections: [{ id: 'c1', name: 'a' }, { id: 'c2', name: 'b' }] } };
            const fresh = clone(loaded);
            fresh.acp.connections[1].name = 'b (elsewhere)';
            const out = fieldSave(loaded, (c) => (c.acp.connections[0].name = 'a2'), fresh);
            expect(out.acp.connections.map((c) => c.name)).toEqual(['a2', 'b (elsewhere)']);
        });
    });

    it('settingPaths names single settings, not list items', () => {
        const changes = diffPaths(
            { ui: { theme: 'a' }, hotkey: { key: 'A' }, shortcuts: [{ sync_id: 's1', n: 1 }] },
            { ui: { theme: 'b' }, hotkey: { key: 'B' }, shortcuts: [{ sync_id: 's1', n: 2 }] }
        );
        expect(settingPaths(changes).sort()).toEqual(['hotkey.key', 'ui.theme']);
        expect(valueAt({ a: [{ sync_id: 'x', v: 1 }] }, ['a', { id: 'x' }, 'v'])).toBe(1);
    });
});

describe('saveConfig and getConfigSnapshot', () => {
    let cache;
    let tauri;

    beforeEach(async () => {
        vi.resetModules();
        tauri = setupTauriMock({
            get_config_snapshot: async () => ({ revision: 7, config: { a: 1 } }),
            save_config: async () => ({ revision: 8 }),
        });
        cache = await import('../../ui/js/shared/config-cache.js');
    });

    afterEach(() => teardownTauriMock());

    it('saveConfig passes baseRevision and returns the new revision', async () => {
        const { revision, config } = await cache.getConfigSnapshot(tauri.invoke);
        expect(revision).toBe(7);
        const out = await cache.saveConfig(config, { baseRevision: revision, invoke: tauri.invoke });
        expect(tauri.invoke).toHaveBeenLastCalledWith('save_config', {
            config: { a: 1 },
            baseRevision: 7,
        });
        expect(out.revision).toBe(8);
    });

    it('without a revision it saves as before, and an older backend answer still works', async () => {
        tauri.invoke.mockImplementationOnce(async () => null);
        const out = await cache.saveConfig({ a: 2 });
        expect(tauri.invoke).toHaveBeenLastCalledWith('save_config', { config: { a: 2 } });
        expect(out.revision).toBeNull();
    });
});

describe('SettingsManager field-level save', () => {
    let backend;
    let revision;
    let saves;
    let SettingsManager;
    let SettingsModule;

    function listModule() {
        class ListModule extends SettingsModule {
            constructor() {
                super('lists', 'lists', '');
            }
            render() {
                return '<input id="font"><button id="add">add</button>';
            }
            load(config) {
                this.items = config.shortcuts;
                document.getElementById('font').value = String(config.ui.font_size);
            }
            save(config) {
                config.ui.font_size = Number(document.getElementById('font').value);
                config.shortcuts = this.items;
            }
        }
        return new ListModule();
    }

    beforeEach(async () => {
        vi.resetModules();
        document.body.innerHTML = '<div id="settingsModules"></div>';
        revision = 1;
        saves = [];
        backend = {
            ui: { theme: 'dark', font_size: 14 },
            shortcuts: [{ sync_id: 's1', name: 'one' }],
        };
        setupTauriMock({
            get_config: async () => clone(backend),
            get_config_snapshot: async () => ({ revision, config: clone(backend) }),
            save_config: async ({ config, baseRevision }) => {
                saves.push({ config: clone(config), baseRevision });
                // The backend gives new list items their id.
                for (const s of config.shortcuts || []) if (!s.sync_id) s.sync_id = `s${Math.random()}`;
                backend = clone(config);
                revision += 1;
                return { revision };
            },
        });
        ({ SettingsManager } = await import('../../ui/js/settings/manager.js'));
        ({ SettingsModule } = await import('../../ui/js/settings/base.js'));
    });

    afterEach(() => teardownTauriMock());

    async function setup() {
        const m = listModule();
        const mgr = new SettingsManager();
        mgr.registerModule(m);
        mgr.render();
        await mgr.load();
        return { mgr, m };
    }

    it('a stale section keeps a change made elsewhere (scenario I5)', async () => {
        const { mgr } = await setup();
        const font = document.getElementById('font');
        font.value = '18';
        font.dispatchEvent(new Event('input', { bubbles: true }));
        // Another computer changes the theme and renames the shortcut.
        backend.ui.theme = 'light';
        backend.shortcuts[0].name = 'one (elsewhere)';
        await mgr.save();
        expect(saves).toHaveLength(1);
        expect(saves[0].baseRevision).toBe(1);
        expect(saves[0].config.ui).toEqual({ theme: 'light', font_size: 18 });
        expect(saves[0].config.shortcuts).toEqual([{ sync_id: 's1', name: 'one (elsewhere)' }]);
    });

    it('reloads after a save, so a new item carries its id and a second save adds nothing', async () => {
        const { mgr, m } = await setup();
        m.items.push({ name: 'two' });
        document.getElementById('add').click();
        await mgr.save();
        expect(backend.shortcuts).toHaveLength(2);
        const id = backend.shortcuts[1].sync_id;
        expect(m.items[1].sync_id).toBe(id);
        document.getElementById('add').click();
        await mgr.save();
        expect(backend.shortcuts).toHaveLength(2);
        expect(saves[1].baseRevision).toBe(2);
    });
});

describe('every save goes through saveConfig', () => {
    const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
    const uiRoot = path.join(repoRoot, 'ui');
    // config-cache.js owns the one call. extension-permissions.js only maps
    // the command name for the sandbox (WP-S5 moves its own save).
    const ALLOWED = new Set([
        path.join('js', 'shared', 'config-cache.js'),
        path.join('js', 'shared', 'extension-permissions.js'),
    ]);

    function files(dir) {
        const out = [];
        for (const e of readdirSync(dir, { withFileTypes: true })) {
            const full = path.join(dir, e.name);
            if (e.isDirectory()) {
                if (e.name === 'vendor') continue;
                out.push(...files(full));
            } else if (/\.(js|html)$/.test(e.name)) {
                out.push(full);
            }
        }
        return out;
    }

    it("no file but config-cache.js invokes 'save_config'", () => {
        const offenders = [];
        for (const f of files(uiRoot)) {
            const rel = path.relative(uiRoot, f);
            if (ALLOWED.has(rel)) continue;
            if (/invoke\s*\(\s*['"]save_config['"]/.test(readFileSync(f, 'utf8'))) offenders.push(rel);
        }
        expect(offenders, `Use saveConfig() from ui/js/shared/config-cache.js:\n${offenders.join('\n')}`).toEqual([]);
    });
});
