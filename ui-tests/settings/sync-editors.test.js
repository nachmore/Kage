/**
 * List editors keep each item's id when they rebuild it (docs/design/
 * SYNC.md §4.4, §20 WP-S6). Folder sync matches items by `sync_id` (or
 * `id` for agents and internal sites); an editor that drops it makes an
 * edit look like a delete and a new item on every computer.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import { initEnglish } from '../helpers/sync-i18n.js';

const clone = (v) => structuredClone(v);

function mount(module) {
    document.body.innerHTML = `<div data-section-content="${module.id}">${module.render()}</div>`;
}

beforeAll(async () => {
    Object.defineProperty(navigator, 'platform', { value: 'MacIntel', configurable: true });
    window.matchMedia ??= () => ({
        matches: false,
        addEventListener() {},
        removeEventListener() {},
        addListener() {},
        removeListener() {},
    });
    setupTauriMock(new Proxy({}, { get: () => async () => undefined }));
    await initEnglish(await import('../../ui/js/shared/i18n.js'));
});

afterAll(() => teardownTauriMock());

describe('editors keep sync_id', () => {
    it('Commands & Prompts: editing a shortcut keeps its id and marks its row', async () => {
        const { ShortcutsSettingsModule } = await import('../../ui/js/settings/shortcuts.js');
        const m = new ShortcutsSettingsModule();
        mount(m);
        m.load({
            shortcuts: [
                { sync_id: 's1', name: 'gh', shortcut: 'gh', action_type: 'open_url', url: 'https://a' },
            ],
        });
        expect(document.querySelector('[data-sync-path="shortcuts/s1"]')).not.toBeNull();
        m.editShortcut(0);
        expect(
            document.querySelector('[data-sync-field="path"]').dataset.syncPath
        ).toBe('shortcuts/s1/path');
        expect(document.querySelector('[data-sync-item-avail]').dataset.syncAvail).toBe(
            'shortcuts/s1'
        );
        document.getElementById('shortcutUrl').value = 'https://b';
        m.saveShortcut();
        const config = {};
        m.save(config);
        expect(config.shortcuts[0]).toMatchObject({ sync_id: 's1', url: 'https://b' });
        // A new shortcut has no id yet, so its editor names no item.
        m.showAddDialog();
        expect(document.querySelector('[data-sync-field="path"]').dataset.syncPath).toBeUndefined();
    });

    it('Personalization: custom actions and app modes keep their ids', async () => {
        const { AssistantSettingsModule } = await import('../../ui/js/settings/assistant.js');
        const m = new AssistantSettingsModule();
        mount(m);
        m._renderCustomActions([{ sync_id: 'q1', label: 'Fix', prompt: 'Fix {text}', icon: '🛠' }]);
        const row = document.querySelector('.custom-action-row');
        expect(row.dataset.syncPath).toBe('quick_actions.custom_actions/q1');
        row.querySelector('.ca-prompt').value = 'Fix this: {text}';
        expect(m._collectCustomActions()).toEqual([
            { sync_id: 'q1', label: 'Fix', icon: '🛠', prompt: 'Fix this: {text}', content_types: [] },
        ]);

        document.getElementById('personalization-app-modes').hidden = false;
        m._addAppModeRow({ sync_id: 'm1', friendly_name: 'Code', executable: 'code', steering: 's', enabled: true });
        const am = document.querySelector('.app-mode-row');
        expect(am.dataset.syncPath).toBe('context_rules/m1');
        am.querySelector('.am-name').value = 'VS Code';
        expect(m._collectAppModes()[0]).toMatchObject({ sync_id: 'm1', friendly_name: 'VS Code' });
    });

    it('Extension Store: store sources keep their ids', async () => {
        const { StoreSettingsModule } = await import('../../ui/js/settings/store.js');
        const m = new StoreSettingsModule();
        mount(m);
        m.load({ store_sources: [{ sync_id: 'x1', name: 'Mine', url: 'https://s', enabled: true }] });
        const row = document.querySelector('.store-source-row');
        expect(row.dataset.syncPath).toBe('store_sources/x1');
        row.querySelector('.source-name').value = 'Mine 2';
        const config = {};
        m.save(config);
        expect(config.store_sources).toEqual([
            { sync_id: 'x1', name: 'Mine 2', url: 'https://s', enabled: true },
        ]);
    });

    it('Automations keep their ids through an edit', async () => {
        const { AutomationsSettingsModule } = await import('../../ui/js/settings/automations.js');
        const m = new AutomationsSettingsModule();
        mount(m);
        m.load({
            macros: [
                { sync_id: 'a1', name: 'Daily', icon: '🔄', steps: [], trigger: { type: 'manual' } },
            ],
        });
        expect(document.querySelector('[data-sync-path="macros/a1"]')).not.toBeNull();
        const config = {};
        m.save(config);
        expect(config.macros[0].sync_id).toBe('a1');
    });

    it('Internal Sites and Agents keep their `id`', async () => {
        const { InternalSitesSettingsModule } = await import(
            '../../ui/js/settings/internal-sites.js'
        );
        const s = new InternalSitesSettingsModule();
        mount(s);
        s.load({ internal_sites: [{ id: 'site1', name: 'Wiki', url_rules: [], content_rules: [] }] });
        expect(document.querySelector('[data-sync-path="internal_sites/site1"]')).not.toBeNull();
        const config = {};
        s.save(config);
        expect(config.internal_sites[0].id).toBe('site1');

        const { ConnectionSettingsModule } = await import('../../ui/js/settings/connection.js');
        const c = new ConnectionSettingsModule();
        mount(c);
        const conn = {
            id: 'c1',
            name: 'Kiro',
            preset_id: null,
            mode: { type: 'local', spawn_command: 'kiro-cli acp' },
            sessions_directory: null,
        };
        c.load({ acp: { connections: [clone(conn)], active_connection_id: 'c1', agent: {} } });
        expect(document.querySelector('[data-sync-path="acp.connections/c1"]')).not.toBeNull();
        const out = {};
        c.save(out);
        expect(out.acp.connections[0].id).toBe('c1');
    });
});
