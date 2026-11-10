/**
 * The where-it-applies control (docs/design/SYNC.md §9.4): the label for
 * each ScopeView, the menu's scope actions (§9.3), nothing at all while
 * sync is off, and RTL placement.
 */

import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import { initEnglish } from '../helpers/sync-i18n.js';

let chip;
let i18n;
let t;

const view = (over = {}) => ({
    synced: true,
    rule: 'shared',
    applies: 'all',
    reason_key: null,
    held: null,
    values: { all: 14 },
    others: [],
    ...over,
});

const flush = () => new Promise((r) => setTimeout(r, 0));

beforeAll(async () => {
    setupTauriMock({});
    i18n = await import('../../ui/js/shared/i18n.js');
    await initEnglish(i18n);
    t = i18n.t;
    chip = await import('../../ui/js/shared/sync-scope-chip.js');
});

afterEach(() => {
    document.body.innerHTML = '';
    document.documentElement.dir = 'ltr';
});

describe('chipState', () => {
    it('is a quiet icon for a shared value with no overrides', () => {
        const s = chip.chipState(view(), 'macos', { t });
        expect(s.tone).toBe('quiet');
        expect(s.label).toBe('All computers');
        expect(s.interactive).toBe(true);
    });

    it('says "Macs only" with the shared value under it when this OS has its own', () => {
        const s = chip.chipState(
            view({ applies: 'os', values: { all: 14, macos: 16 } }),
            'macos',
            { t }
        );
        expect(s.tone).toBe('override');
        expect(s.label).toBe('Macs only');
        expect(s.sub).toBe('All computers: 14');
        expect(chip.chipState(view({ applies: 'os' }), 'windows', { t }).label).toBe(
            'Windows PCs only'
        );
        expect(chip.chipState(view({ applies: 'os' }), 'linux', { t }).label).toBe(
            'Linux PCs only'
        );
    });

    it('says "This computer" when pinned', () => {
        const s = chip.chipState(view({ applies: 'this_computer', synced: false }), 'macos', {
            t,
        });
        expect(s.tone).toBe('pinned');
        expect(s.label).toBe('This computer');
    });

    it('notes another OS with its own value, with the value on hover', () => {
        const s = chip.chipState(view({ others: [{ os: 'windows', preview: '12' }] }), 'macos', {
            t,
        });
        expect(s.tone).toBe('differs');
        expect(s.label).toBe('Differs on Windows');
        expect(s.title).toBe('Windows PCs use 12');
    });

    it('says "Each kind of computer" for a per-OS row, with the other OS as a hint', () => {
        const s = chip.chipState(
            view({
                rule: 'per_os',
                applies: 'os',
                values: { macos: 'Alt+Space', windows: 'Ctrl+Space' },
                others: [{ os: 'windows', preview: 'Ctrl+Space' }],
            }),
            'macos',
            { t }
        );
        expect(s.tone).toBe('per_os');
        expect(s.label).toBe('Each kind of computer');
        expect(s.sub).toBe('Windows PCs use Ctrl+Space');
    });

    it('is a static "This computer" with the reason for a row that never syncs', () => {
        const s = chip.chipState(
            view({
                synced: false,
                rule: 'this_computer',
                applies: 'not_synced',
                reason_key: 'settings.sync_scope.reason.login_item',
                values: {},
            }),
            'macos',
            { t }
        );
        expect(s.tone).toBe('local');
        expect(s.interactive).toBe(false);
        expect(s.title).toBe('Not synced. Start at login is set for each computer.');
    });

    it('says "Not synced here" for a held value, with why', () => {
        const s = chip.chipState(
            view({
                applies: 'not_synced',
                held: 'held_back',
                reason_key: 'settings.sync_scope.reason.looks_like_key',
            }),
            'macos',
            { t }
        );
        expect(s.tone).toBe('held');
        expect(s.label).toBe('Not synced here');
        expect(s.title).toBe('It looks like it contains a key.');
    });

    it('shows nothing for caches and geometry, or a list item on every computer', () => {
        expect(chip.chipState(view({ rule: 'never', synced: false }), 'macos', { t })).toBeNull();
        expect(chip.chipState(view({ values: { all: 'all' } }), 'macos', { item: true, t })).toBe(
            null
        );
        expect(
            chip.chipState(view({ applies: 'os', values: { all: 'macos' } }), 'macos', {
                item: true,
                t,
            }).label
        ).toBe('Macs only');
    });
});

describe('menuItems', () => {
    const actions = (items) => items.filter((i) => i.action).map((i) => i.action);

    it('offers Macs only and This computer for a shared value', () => {
        const items = chip.menuItems(view(), 'macos', { t });
        expect(actions(items)).toEqual(['all', 'os', 'this_computer']);
        expect(items[0].checked).toBe(true);
        expect(items[0].noop).toBe(true);
        expect(items[1].label).toBe('Only on Macs');
    });

    it('offers the shared value back, or this value everywhere, for an override', () => {
        const items = chip.menuItems(
            view({ applies: 'os', values: { all: 14, macos: 16 } }),
            'macos',
            { t }
        );
        expect(actions(items)).toEqual(['os', 'use_shared', 'all', 'this_computer']);
        expect(items[1].label).toBe('Use the value for all computers (14)');
        expect(items[2].label).toBe('Use 16 on all computers');
    });

    it('offers to stop keeping a value only here when pinned', () => {
        expect(
            actions(chip.menuItems(view({ applies: 'this_computer', synced: false }), 'macos', { t }))
        ).toEqual(['this_computer', 'stop_this_computer']);
    });

    it('lists what other OSes use', () => {
        const items = chip.menuItems(view({ others: [{ os: 'windows', preview: '12' }] }), 'macos', {
            t,
        });
        expect(items.some((i) => i.separator)).toBe(true);
        expect(items.find((i) => i.info).info).toBe('Windows PCs use 12');
    });

    it('offers availability for a list item, confirming before keeping it only here', () => {
        const items = chip.menuItems(view({ values: { all: 'all' } }), 'windows', {
            item: true,
            t,
        });
        expect(actions(items)).toEqual(['avail_all', 'avail_os', 'avail_this_computer']);
        expect(items[1].label).toBe('Only on Windows PCs');
        expect(items[2].confirm).toBe(true);
    });

    it('offers nothing for a row that never syncs', () => {
        expect(
            chip.menuItems(
                view({ synced: false, rule: 'this_computer', applies: 'not_synced' }),
                'macos',
                { t }
            )
        ).toEqual([]);
    });
});

describe('helpers', () => {
    it('pathsOverlap matches a path, what it holds, and what holds it', () => {
        expect(chip.pathsOverlap('ui.theme', 'ui.theme')).toBe(true);
        expect(chip.pathsOverlap('hotkey', 'hotkey.key')).toBe(true);
        expect(chip.pathsOverlap('shortcuts/s1', 'shortcuts')).toBe(true);
        expect(chip.pathsOverlap('shortcuts/s1/path', 'shortcuts/s1')).toBe(true);
        expect(chip.pathsOverlap('ui.theme', 'ui.themes')).toBe(false);
        expect(chip.pathsOverlap('', 'ui')).toBe(false);
    });

    it('formats values for the control', () => {
        expect(chip.formatScopeValue(true, t)).toBe('On');
        expect(chip.formatScopeValue({ modifiers: ['Alt'], key: 'Space' }, t)).toBe('Alt+Space');
        expect(chip.formatScopeValue(['a', 'b'], t)).toBe('a, b');
        expect(chip.formatScopeValue(null, t)).toBe('');
    });

    it('syncIsOn is false while off, unavailable or setting up', () => {
        for (const state of ['off', 'unavailable', 'setting_up']) {
            expect(chip.syncIsOn({ state })).toBe(false);
        }
        for (const state of ['up_to_date', 'paused', 'locked', 'folder_missing']) {
            expect(chip.syncIsOn({ state })).toBe(true);
        }
        expect(chip.syncIsOn(null)).toBe(false);
    });

    it('an extension data syncs only with sync on, the category on, the grant and its switch', () => {
        const on = { state: 'up_to_date', categories: { extensions: true }, ext_data_off: [] };
        const grant = { granted: ['storage', 'sync'] };
        expect(chip.extensionDataSyncs(on, grant, 'mm')).toBe(true);
        expect(chip.extensionDataSyncs({ ...on, state: 'off' }, grant, 'mm')).toBe(false);
        expect(
            chip.extensionDataSyncs({ ...on, categories: { extensions: false } }, grant, 'mm')
        ).toBe(false);
        expect(chip.extensionDataSyncs(on, { granted: ['storage'] }, 'mm')).toBe(false);
        expect(chip.extensionDataSyncs({ ...on, ext_data_off: ['mm'] }, grant, 'mm')).toBe(false);
    });
});

describe('attachScopeChips', () => {
    let tauri;
    let scopes;

    beforeEach(() => {
        scopes = null;
        tauri = setupTauriMock({
            sync_get_scopes: async ({ paths }) => {
                if (!scopes) throw { kind: 'sync_unavailable', message: 'off' };
                return Object.fromEntries(paths.filter((p) => scopes[p]).map((p) => [p, scopes[p]]));
            },
            sync_set_scope: async ({ path, action }) => {
                if (action === 'os') scopes[path] = view({ applies: 'os', values: { macos: 14 } });
                return scopes[path];
            },
        });
        document.body.innerHTML = `<div id="root">
            <div class="setting-row" data-sync-path="ui.font_size"><div class="setting-label">Font size</div><input id="f"></div>
            <div class="setting-row" data-sync-path="system.auto_start"><div class="setting-label">Start at login</div></div>
            <div data-sync-avail="shortcuts/s1" data-sync-name="gh" hidden></div>
            <div class="setting-row"><span data-sync-local="sign_in" hidden></span></div>
        </div>`;
    });

    afterEach(() => teardownTauriMock());

    const attach = () =>
        chip.attachScopeChips(document.getElementById('root'), {
            invoke: tauri.invoke,
            listen: tauri.listen,
            t,
            os: 'macos',
        });

    it('adds nothing while sync is off: the page is unchanged', async () => {
        const before = document.getElementById('root').innerHTML;
        const detach = attach();
        await flush();
        expect(tauri.invoke).toHaveBeenCalledWith('sync_get_scopes', expect.anything());
        expect(document.querySelector('.sync-scope-chip')).toBeNull();
        expect(document.getElementById('root').innerHTML).toBe(before);
        detach();
    });

    it('asks once for every marked path and labels each row', async () => {
        scopes = {
            'ui.font_size': view(),
            'system.auto_start': view({
                synced: false,
                rule: 'this_computer',
                applies: 'not_synced',
                reason_key: 'settings.sync_scope.reason.login_item',
            }),
            'shortcuts/s1': view({ values: { all: 'all' } }),
        };
        const detach = attach();
        await flush();
        const calls = tauri.invoke.mock.calls.filter(([c]) => c === 'sync_get_scopes');
        expect(calls).toHaveLength(1);
        expect(calls[0][1].paths.sort()).toEqual(
            ['shortcuts/s1', 'system.auto_start', 'ui.font_size'].sort()
        );
        const font = document.querySelector('[data-sync-chip-for="ui.font_size"]');
        expect(font.closest('.setting-label')).not.toBeNull();
        expect(font.getAttribute('aria-label')).toBe('Font size: All computers. Change');
        const login = document.querySelector('[data-sync-chip-for="system.auto_start"]');
        expect(login.tagName).toBe('SPAN');
        expect(login.title).toContain('Start at login is set for each computer.');
        // The availability line and the sign-in chip.
        const radios = document.querySelectorAll('[data-sync-avail] input[type="radio"]');
        expect(radios).toHaveLength(3);
        expect(radios[0].checked).toBe(true);
        expect(document.querySelector('[data-sync-local]').textContent).toContain(
            "sign-ins don't sync"
        );
        detach();
    });

    it('each menu item calls sync_set_scope with its action, and the label follows', async () => {
        scopes = { 'ui.font_size': view() };
        const detach = attach();
        await flush();
        document.querySelector('[data-sync-chip-for="ui.font_size"]').click();
        const menu = document.querySelector('.sync-scope-menu');
        expect(menu.getAttribute('role')).toBe('menu');
        const item = [...menu.querySelectorAll('[role="menuitemradio"]')].find(
            (b) => b.textContent.includes('Only on Macs')
        );
        item.click();
        await flush();
        expect(tauri.invoke).toHaveBeenCalledWith('sync_set_scope', {
            path: 'ui.font_size',
            action: 'os',
        });
        expect(document.querySelector('.sync-scope-menu')).toBeNull();
        expect(
            document.querySelector('[data-sync-chip-for="ui.font_size"]').textContent
        ).toContain('Macs only');
        detach();
    });

    it('the menu works from the keyboard and closes on Escape', async () => {
        scopes = { 'ui.font_size': view() };
        const detach = attach();
        await flush();
        const c = document.querySelector('[data-sync-chip-for="ui.font_size"]');
        c.click();
        const menu = document.querySelector('.sync-scope-menu');
        const items = [...menu.querySelectorAll('button')];
        expect(document.activeElement).toBe(items[0]);
        menu.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
        expect(document.activeElement).toBe(items[1]);
        menu.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        expect(document.querySelector('.sync-scope-menu')).toBeNull();
        expect(document.activeElement).toBe(c);
        detach();
    });

    it('opens the menu from the inline end in RTL', async () => {
        document.documentElement.dir = 'rtl';
        scopes = { 'ui.font_size': view() };
        const detach = attach();
        await flush();
        document.querySelector('[data-sync-chip-for="ui.font_size"]').click();
        const menu = document.querySelector('.sync-scope-menu');
        expect(menu.style.right).not.toBe('');
        expect(menu.style.left).toBe('');
        detach();
    });

    it('confirms before keeping a list item only on this computer', async () => {
        scopes = { 'shortcuts/s1': view({ values: { all: 'all' } }) };
        const detach = attach();
        await flush();
        const radios = document.querySelectorAll('[data-sync-avail] input[type="radio"]');
        radios[2].checked = true;
        radios[2].dispatchEvent(new Event('change', { bubbles: true }));
        await flush();
        const dialog = document.querySelector('.kage-confirm-backdrop');
        expect(dialog.textContent).toContain('gh');
        dialog.querySelector('[data-action="cancel"]').click();
        await flush();
        expect(tauri.invoke).not.toHaveBeenCalledWith('sync_set_scope', expect.anything());
        detach();
    });

    it('gives new rows a control and removes everything on detach', async () => {
        scopes = { 'ui.font_size': view(), 'ui.theme': view() };
        const detach = attach();
        await flush();
        const row = document.createElement('div');
        row.className = 'setting-row';
        row.dataset.syncPath = 'ui.theme';
        row.innerHTML = '<div class="setting-label">Theme</div>';
        document.getElementById('root').appendChild(row);
        await new Promise((r) => setTimeout(r, 120));
        expect(document.querySelector('[data-sync-chip-for="ui.theme"]')).not.toBeNull();
        detach();
        expect(document.querySelector('.sync-scope-chip')).toBeNull();
    });

    it('markSyncItem points an editor at the item it edits, or at none', async () => {
        scopes = { 'shortcuts/s9': view({ values: { all: 'all' } }), 'shortcuts/s9/path': view() };
        document.body.innerHTML = `<div id="root"><div id="dlg">
            <div data-sync-item-avail hidden></div>
            <div class="dialog-field" data-sync-field="path"><label>Path</label><span data-sync-chip-slot></span></div>
        </div></div>`;
        const detach = attach();
        await flush();
        const dlg = document.getElementById('dlg');
        chip.markSyncItem(dlg, 'shortcuts/s9', 'gh');
        expect(dlg.querySelector('[data-sync-field]').dataset.syncPath).toBe('shortcuts/s9/path');
        await new Promise((r) => setTimeout(r, 120));
        expect(dlg.querySelector('[data-sync-chip-for="shortcuts/s9/path"]')).not.toBeNull();
        expect(dlg.querySelectorAll('[data-sync-item-avail] input[type="radio"]')).toHaveLength(3);
        chip.markSyncItem(dlg, null);
        expect(dlg.querySelector('[data-sync-field]').dataset.syncPath).toBeUndefined();
        expect(dlg.querySelector('[data-sync-item-avail]').hidden).toBe(true);
        detach();
    });
});

describe('dirty tracking', () => {
    it("sync's own controls never mark a Settings section dirty", async () => {
        const { dirtySectionForEvent } = await import('../../ui/js/settings/dirty-tracker.js');
        document.body.innerHTML = `<div data-section-content="appearance">
            <button id="plain">x</button>
            <span data-sync-ui><button id="chip">c</button></span>
        </div>`;
        const seen = [];
        document.addEventListener('click', (e) => seen.push(dirtySectionForEvent(e)), {
            capture: true,
            once: false,
        });
        document.getElementById('plain').click();
        document.getElementById('chip').click();
        expect(seen).toEqual(['appearance', null]);
    });
});

describe('the changed-everywhere note', () => {
    it('offers Undo and Only on this computer', async () => {
        const onUndo = vi.fn(async () => {});
        const onOnlyHere = vi.fn(async () => {});
        const toast = chip.showChangedEverywhereToast({ t, onUndo, onOnlyHere });
        expect(toast.textContent).toContain('Changed on all your computers.');
        const [undo, here] = toast.querySelectorAll('button');
        expect(here.textContent).toBe('Only on this computer');
        undo.click();
        await flush();
        expect(onUndo).toHaveBeenCalled();
        expect(document.querySelector('.sync-toast')).toBeNull();
    });
});
