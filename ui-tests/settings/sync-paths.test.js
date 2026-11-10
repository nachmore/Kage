/**
 * Every config path Settings shows carries `data-sync-path`, so the
 * where-it-applies control can reach it (docs/design/SYNC.md §9.4).
 *
 * The list of paths is read from the Rust catalog (`src/sync/catalog.rs`,
 * the rows `sync_get_scopes(None)` answers for), so a new catalog row
 * without a Settings mark fails here. Rows Settings doesn't show are
 * listed below with the reason.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import { initEnglish, repoRoot } from '../helpers/sync-i18n.js';

/** Catalog rows with no control in Settings, and why. */
const NOT_SHOWN = {
    'acp.agent.user_steering_path': 'set by the steering editor, never typed in Settings',
    'ui.macos_overlay_panels': 'an escape hatch with no Settings control',
    debug_mode: 'set by the /dev launch flag',
    first_run_completed: 'set by the welcome window',
    extension_grants: 'shown as capability badges, managed by the install prompt',
    extension_agent_access: 'shown by the Background AI panel (EXTENSION_AGENT_CALLS)',
    'pocket_tts.python_path': 'set by the local voice installer',
    'pocket_tts.installed': 'set by the local voice installer',
    'automation_power.battery_multiplier': 'no control; shown as text under the mode',
    'automation_power.low_battery_multiplier': 'no control; shown as text under the mode',
    'automation_power.disable_signals_on_low_battery': 'no Settings control',
};

/** Rows only drawn on one OS: checked in the module source instead. */
const PLATFORM_ONLY = {
    clipboard_hotkey: 'hotkey.js',
};

/** The `Shared`, `PerOs` and `ThisComputer` rows of the catalog. */
function catalogPaths() {
    const src = readFileSync(path.join(repoRoot, 'src/sync/catalog.rs'), 'utf8');
    const body = src.slice(src.indexOf('pub const SYNC_FIELDS'), src.indexOf('pub enum Avail'));
    const out = new Set();
    for (const m of body.matchAll(/\b(?:shared|per_os|here)\(\s*"([^"]+)"/g)) out.add(m[1]);
    out.delete('extension_states.*');
    return [...out];
}

let rendered = [];

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
    const i18n = await import('../../ui/js/shared/i18n.js');
    await initEnglish(i18n);
    const loaders = [
        [() => import('../../ui/js/settings/appearance.js'), 'AppearanceSettingsModule'],
        [() => import('../../ui/js/settings/hotkey.js'), 'HotkeySettingsModule'],
        [() => import('../../ui/js/settings/system.js'), 'SystemSettingsModule'],
        [() => import('../../ui/js/settings/assistant.js'), 'AssistantSettingsModule'],
        [() => import('../../ui/js/settings/connection.js'), 'ConnectionSettingsModule'],
        [() => import('../../ui/js/settings/model.js'), 'ModelSettingsModule'],
        [() => import('../../ui/js/settings/mcp.js'), 'McpSettingsModule'],
        [() => import('../../ui/js/settings/tool-permissions.js'), 'ToolPermissionsSettingsModule'],
        [() => import('../../ui/js/settings/integration.js'), 'IntegrationSettingsModule'],
        [() => import('../../ui/js/settings/automations.js'), 'AutomationsSettingsModule'],
        [() => import('../../ui/js/settings/speech.js'), 'SpeechSettingsModule'],
        [() => import('../../ui/js/settings/store.js'), 'StoreSettingsModule'],
        [() => import('../../ui/js/settings/privacy.js'), 'PrivacySettingsModule'],
        [() => import('../../ui/js/settings/updates.js'), 'UpdatesSettingsModule'],
        [() => import('../../ui/js/settings/about.js'), 'AboutSettingsModule'],
    ];
    const modules = await Promise.all(
        loaders.map(async ([load, name]) => new (await load())[name]())
    );
    document.body.innerHTML = modules
        .map((m) => `<div data-section-content="${m.id}">${m.render()}</div>`)
        .join('');
    // Rows drawn after render: the agents list and the power mode row.
    const conn = modules.find((m) => m.id === 'connection');
    conn._renderListView(document.getElementById('connPageRoot'));
    const autos = modules.find((m) => m.id === 'macros');
    autos._powerConfig = { mode: 'auto', battery_multiplier: 2, low_battery_multiplier: 4 };
    autos._renderPowerSection();
    rendered = [...document.querySelectorAll('[data-sync-path]')].map((el) => el.dataset.syncPath);
});

afterAll(() => teardownTauriMock());

describe('sync paths in Settings', () => {
    it('every catalog row Settings shows has a data-sync-path', () => {
        const missing = catalogPaths()
            .filter((p) => !(p in NOT_SHOWN) && !(p in PLATFORM_ONLY))
            .filter((p) => !rendered.some((r) => r === p || r.startsWith(`${p}.`)));
        expect(missing, `Add { syncPath } to the Settings row for:\n${missing.join('\n')}`).toEqual(
            []
        );
    });

    it('rows drawn on one OS carry their path in the source', () => {
        for (const [p, file] of Object.entries(PLATFORM_ONLY)) {
            const src = readFileSync(path.join(repoRoot, 'ui/js/settings', file), 'utf8');
            expect(src).toContain(`syncPath: '${p}'`);
        }
    });

    it('every data-sync-path names a catalog row or a list', () => {
        const rows = catalogPaths();
        const lists = [
            'shortcuts',
            'macros',
            'quick_actions.custom_actions',
            'context_rules',
            'internal_sites',
            'acp.connections',
            'store_sources',
        ];
        const unknown = rendered.filter(
            (r) =>
                !rows.some((p) => r === p || r.startsWith(`${p}.`)) &&
                !lists.some((l) => r.startsWith(`${l}/`))
        );
        expect(unknown).toEqual([]);
    });

    it('the not-shown list stays true: none of them is in the catalog twice or gone', () => {
        const rows = catalogPaths();
        for (const p of Object.keys(NOT_SHOWN)) expect(rows).toContain(p);
    });
});
