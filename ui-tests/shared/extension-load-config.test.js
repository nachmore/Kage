import { describe, expect, it, vi } from 'vitest';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

function fixture() {
    const config = {
        extensions: { ext: { label: 'original' } },
        extension_grants: { ext: { granted: [] } },
    };
    const mgr = new ExtensionManager(vi.fn(async () => structuredClone(config)));
    mgr._configCache = structuredClone(config);
    mgr._fetchProviderSources = vi.fn(async () => ({ searchProvider: 'export default class {}' }));
    mgr._fetchVendorSources = vi.fn(async () => undefined);
    mgr._fetchExtensionLocale = vi.fn(async () => ({ catalog: {}, fallback: {} }));
    mgr._loadExtensionCss = vi.fn(async () => {});
    const sandbox = { updateConfig: vi.fn(async () => {}) };
    mgr._pool.load = vi.fn(async () => sandbox);
    const item = { manifest: { id: 'ext' }, enabled: true };
    return { mgr, config, sandbox, item };
}

describe('_loadExtension config reconciliation', () => {
    it('boots with the latest source-fetch snapshot and notices a subsequent revert', async () => {
        const { mgr, config, sandbox, item } = fixture();
        let release;
        mgr._fetchProviderSources.mockImplementationOnce(() => new Promise((resolve) => { release = resolve; }));
        const load = mgr._loadExtension(item);
        config.extensions.ext.label = 'saved';
        await mgr.onConfigUpdate();
        release({ searchProvider: 'export default class {}' });
        await load;
        expect(mgr._pool.load.mock.calls[0][0].config.label).toBe('saved');
        expect(JSON.parse(mgr.extensions.get('ext').configSignature).label).toBe('saved');
        config.extensions.ext.label = 'original';
        await mgr.onConfigUpdate();
        expect(sandbox.updateConfig).toHaveBeenCalledExactlyOnceWith({
            label: 'original', enabled: true,
        });
    });

    it.each(['boot', 'css'])('reconciles saves while %s is pending before returning', async (phase) => {
        const { mgr, config, sandbox, item } = fixture();
        let release;
        const gate = phase === 'boot' ? mgr._pool.load : mgr._loadExtensionCss;
        gate.mockImplementationOnce(() => new Promise((resolve) => {
            release = () => resolve(phase === 'boot' ? sandbox : undefined);
        }));
        const load = mgr._loadExtension(item);
        await vi.waitFor(() => expect(release).toBeTypeOf('function'));
        expect(mgr.extensions.has('ext')).toBe(false);
        config.extensions.ext.label = 'saved during load';
        await mgr.onConfigUpdate();
        release();
        await load;
        expect(mgr._pool.load.mock.calls[0][0].config.label).toBe('original');
        expect(sandbox.updateConfig).toHaveBeenCalledExactlyOnceWith({
            label: 'saved during load', enabled: true,
        });
        expect(JSON.parse(mgr.extensions.get('ext').configSignature).label).toBe('saved during load');
    });
});
