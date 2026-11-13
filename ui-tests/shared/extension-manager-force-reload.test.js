/**
 * reload({ force: true }) tears down and reloads every already-loaded
 * extension even when its version and capability set are unchanged.
 *
 * This exists because after a full-backup import an extension's version
 * and capabilities usually look identical to what's already loaded — the
 * fast-path short-circuit would leave the sandbox holding stale
 * pre-import state (spotify "no creds" cache, calendar `_events` cache).
 * The ordinary `config_updated` reload keeps hitting the fast-path; the
 * dedicated `config_imported` event triggers the force variant.
 */

import { describe, it, expect, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

function stubSandbox() {
    return {
        hasSearch: false,
        hasTools: false,
        hasTriggers: false,
        hasSettings: false,
        hasToolbar: false,
        hasFormatter: false,
        widgetIds: [],
        async call() {},
        updateConfig: vi.fn(),
        destroy: vi.fn(),
    };
}

function makeHarness({ stallList = false } = {}) {
    const manifest = {
        id: 'spotify',
        version: '1.2.3',
        permissions: ['urls'],
    };
    const grants = { spotify: { granted: ['urls'], approved_version: '1.2.3' } };

    let releaseList;
    const listGate = stallList
        ? new Promise((r) => { releaseList = r; })
        : Promise.resolve();

    const counts = { list_extensions: 0 };
    const invoke = async (cmd) => {
        if (cmd === 'get_config') {
            return {
                extension_states: { spotify: true },
                extension_grants: grants,
                extensions: {},
            };
        }
        if (cmd === 'list_extensions') {
            counts.list_extensions += 1;
            await listGate;
            return [{ manifest, enabled: true }];
        }
        return undefined;
    };

    const mgr = new ExtensionManager(invoke);
    // Pre-populate the loaded state at EXACTLY the same version + caps the
    // "list_extensions" response advertises — this is the fast-path scenario.
    mgr.extensions.set('spotify', {
        manifest: { ...manifest },
        capabilities: ['urls'],
        sandbox: stubSandbox(),
    });

    const unload = vi.spyOn(mgr, '_unloadExtension').mockImplementation((id) => {
        mgr.extensions.delete(id);
    });
    const load = vi.spyOn(mgr, '_loadExtension').mockImplementation(async (item) => {
        mgr.extensions.set(item.manifest.id, {
            manifest: item.manifest,
            capabilities: ['urls'],
            sandbox: stubSandbox(),
        });
    });

    return { mgr, unload, load, counts, releaseList };
}

describe('ExtensionManager.reload({ force: true })', () => {
    it('without force, a same-version + same-caps roster hits the fast-path (regression guard)', async () => {
        const { mgr, unload, load } = makeHarness();
        await mgr.reload();
        expect(unload).not.toHaveBeenCalled();
        expect(load).not.toHaveBeenCalled();
    });

    it('with force, unload + load are both called even for same-version + same-caps', async () => {
        const { mgr, unload, load } = makeHarness();
        await mgr.reload({ force: true });
        expect(unload).toHaveBeenCalledTimes(1);
        expect(unload).toHaveBeenCalledWith('spotify');
        expect(load).toHaveBeenCalledTimes(1);
    });

    it('chat/main.js and floating/app/lifecycle-init.js listen on EVT.CONFIG_IMPORTED and call reload({ force: true })', () => {
        // Static check: without this wiring the Rust-side event is a no-op
        // on the frontend and the fix would silently regress.
        const chatMain = readFileSync(path.join(repoRoot, 'ui/js/chat/main.js'), 'utf8');
        const floatingInit = readFileSync(
            path.join(repoRoot, 'ui/js/floating/app/lifecycle-init.js'),
            'utf8'
        );
        // Permit either `listen(...)` or `this.listen(...)` surrounding the
        // handler; search for the two anchors near each other.
        const pattern = /listen\s*\(\s*EVT\.CONFIG_IMPORTED[\s\S]{0,400}?reload\s*\(\s*\{\s*force:\s*true\s*\}\s*\)/;
        expect(pattern.test(chatMain), 'ui/js/chat/main.js wiring').toBe(true);
        expect(pattern.test(floatingInit), 'ui/js/floating/app/lifecycle-init.js wiring').toBe(true);
    });

    it('a force request during an in-flight non-force reload promotes the trailing run', async () => {
        const { mgr, unload, load, releaseList } = makeHarness({ stallList: true });

        // First reload is stalled on list_extensions. While it's parked,
        // enqueue a force reload — the trailing rerun must honour it,
        // otherwise the fast-path would silently drop the force request.
        const p1 = mgr.reload();
        // Yield so p1 passes get_config and reaches the stalled list_extensions
        await new Promise((r) => setTimeout(r, 0));
        const p2 = mgr.reload({ force: true });

        releaseList();
        await Promise.all([p1, p2]);

        // Trailing run was a force run — unload + load were called once.
        expect(unload).toHaveBeenCalledWith('spotify');
        expect(unload).toHaveBeenCalledTimes(1);
        expect(load).toHaveBeenCalledTimes(1);
        expect(mgr._reloadInFlight).toBeNull();
        expect(mgr._reloadForcePending).toBe(false);
    });
});
