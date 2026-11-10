/**
 * The Settings window's side of reading a file the user picks
 * (docs/design/EXTENSION_FILE_TEXT.md §3.3, §3.9, §4.5):
 *   - the "📄 Files you choose" pill on an extension's page, with its
 *     description as the tooltip;
 *   - the settings sandbox pool offers `files.pick_text` (kept only with
 *     the `file_text` grant) and names the extension with its manifest
 *     name and icon, `__MSG_*__` resolved, for the dialog and the ask bar.
 */
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import { initI18n } from '../../ui/js/shared/i18n.js';
import { CAPABILITIES } from '../../ui/js/shared/extension-permissions.js';
import {
    HOST_FEATURE_FILE_TEXT,
    HOST_FEATURE_VIEWS,
    hostFeaturesFor,
} from '../../ui/js/shared/extension-sandbox-host.js';
import { renderCapabilityBadges } from '../../ui/js/settings/extension-capabilities.js';

const EN_CATALOG = JSON.parse(
    readFileSync(
        join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'locales', 'en', 'messages.json'),
        'utf-8'
    )
);

let manager;
beforeAll(async () => {
    await initI18n(async () => ({
        language: 'en',
        rtl: false,
        catalog: EN_CATALOG,
        fallback: EN_CATALOG,
    }));
    setupTauriMock({});
    manager = await import('../../ui/js/settings/manager.js');
});

afterAll(() => {
    teardownTauriMock();
});

describe('the file_text capability pill', () => {
    it('shows "📄 Files you choose" with its description as the tooltip', () => {
        const box = document.createElement('div');
        box.innerHTML = renderCapabilityBadges(['storage', 'file_text'], false);
        const pill = [...box.querySelectorAll('.ext-capability-pill')].find((p) =>
            p.textContent.includes('Files you choose')
        );
        expect(pill).toBeTruthy();
        expect(pill.textContent).toBe('📄 Files you choose');
        expect(pill.getAttribute('title')).toBe(
            EN_CATALOG['settings.manager.cap.file_text.desc'].message
        );
    });

    it('matches the capability table the install prompt uses', () => {
        const box = document.createElement('div');
        box.innerHTML = renderCapabilityBadges(['file_text'], false);
        const pill = box.querySelector('.ext-capability-pill');
        expect(pill.textContent).toBe(
            `${CAPABILITIES.file_text.icon} ${EN_CATALOG['settings.manager.cap.file_text.label'].message}`
        );
        expect(pill.getAttribute('title')).toBe(CAPABILITIES.file_text.description);
    });
});

describe('the settings sandbox pool', () => {
    const schema = { sections: [{ controls: [{ type: 'checkbox', id: 'on', label: 'On' }] }] };

    async function build(manifest, { invoke, capabilities = ['file_text'], hostFeatures } = {}) {
        const sandbox = {
            hasSettings: true,
            call: vi.fn(async (method) => (method === 'getSettings' ? schema : undefined)),
            updateConfig: vi.fn(async () => {}),
        };
        const pool = { load: vi.fn(async () => sandbox), unload: vi.fn() };
        await manager.buildSandboxedSettingsModule({
            invoke,
            pool,
            manifest,
            capabilities,
            settingsProviderSource: 'export default class {}',
            currentConfig: {},
            hostFeatures,
        });
        return pool.load.mock.calls[0][0];
    }

    it('offers files.pick_text next to what the window offers', async () => {
        const spec = await build(
            { id: 'mindmap', name: 'Mind Map', icon: '🧠', version: '1.0.0' },
            { hostFeatures: [HOST_FEATURE_VIEWS] }
        );
        expect(spec.hostFeatures).toEqual([HOST_FEATURE_VIEWS, HOST_FEATURE_FILE_TEXT]);
        // Kept only with the grant (the sandbox filters it the same way).
        expect(hostFeaturesFor(spec.hostFeatures, ['file_text'])).toEqual([
            HOST_FEATURE_FILE_TEXT,
        ]);
        expect(hostFeaturesFor(spec.hostFeatures, ['storage'])).toEqual([]);
    });

    it('names the extension with its manifest name and icon', async () => {
        const spec = await build({ id: 'mindmap', name: 'Mind Map', icon: '🧠', version: '1.0.0' });
        expect(spec.displayName).toBe('Mind Map');
        expect(spec.displayIcon).toBe('🧠');
        expect(spec.hostFeatures).toEqual([HOST_FEATURE_FILE_TEXT]);
    });

    it('resolves a __MSG_ name through the extension catalog', async () => {
        const invoke = vi.fn(async (cmd) =>
            cmd === 'read_extension_locale' ? { manifest_name: { message: 'Carte mentale' } } : null
        );
        const spec = await build(
            { id: 'mindmap', name: '__MSG_manifest_name__', icon: '🧠', version: '1.0.0' },
            { invoke }
        );
        expect(spec.displayName).toBe('Carte mentale');
    });

    it('uses the id, never a raw token, when the name has no translation', async () => {
        const spec = await build({ id: 'mindmap', name: '__MSG_manifest_name__', version: '1.0.0' });
        expect(spec.displayName).toBe('mindmap');
        expect(spec.displayIcon).toBe('');
    });
});
