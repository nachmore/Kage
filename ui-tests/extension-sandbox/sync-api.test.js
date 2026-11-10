/**
 * `context.sync` (docs/design/SYNC.md §12.6, §18.5): the runtime object,
 * its argument checks and `onChanges` delivery; and the host side: the
 * capability gate, identity injection, the host features, and the
 * `sync-changes` forwarding.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
    COMMAND_CAPABILITIES,
    CAPABILITIES,
    decideInvoke,
    grantCovers,
    KNOWN_CAPABILITIES,
} from '../../ui/js/shared/extension-permissions.js';
import {
    deliverSyncChanges,
    ExtensionSandbox,
    HOST_FEATURE_SYNC_KEYS,
    HOST_FEATURE_SYNC_LISTS,
    HOST_FEATURE_SYNC_RECORDS,
    HOST_FEATURE_VIEWS,
    hostFeaturesFor,
    IDENTITY_SCOPED_COMMANDS,
    installExtensionSyncEvents,
    SYNC_HOST_FEATURES,
} from '../../ui/js/shared/extension-sandbox-host.js';
import {
    createSyncApi,
    SYNC_CHANGES_DEBOUNCE_MS,
} from '../../ui/js/extension-sandbox/runtime/sync-api.js';

const UI_ROOT = resolve(__dirname, '../../ui');

afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
    document.body.innerHTML = '';
});

/** A sync api on a fake invoke and port. */
function makeApi(answers = {}) {
    const calls = [];
    const handlers = new Set();
    const invoke = vi.fn(async (command, args) => {
        calls.push([command, args]);
        const a = answers[command];
        if (a instanceof Error) throw a;
        return typeof a === 'function' ? a(args) : a;
    });
    const api = createSyncApi({
        invoke,
        onPortMessage: (fn) => {
            handlers.add(fn);
            return () => handlers.delete(fn);
        },
    });
    const post = (msg) => {
        for (const fn of handlers) fn(msg);
    };
    return { api, calls, invoke, post };
}

describe('context.sync: the runtime object', () => {
    it('asks for its status at start and follows it', async () => {
        const { api, calls } = makeApi({
            ext_sync_status: { available: true, needsInitialPush: true },
        });
        expect(api.available).toBe(false);
        expect(calls[0]).toEqual(['ext_sync_status', {}]);
        await Promise.resolve();
        await Promise.resolve();
        expect(api.available).toBe(true);
        expect(Object.isFrozen(api)).toBe(true);
        expect(() => {
            api.available = false;
        }).toThrow();
    });

    it('sends the four commands without an extension id', async () => {
        const { api, calls } = makeApi({
            ext_sync_status: { available: true },
            ext_sync_push: { accepted: 1, unchanged: 0, rejected: [], off: false },
            ext_sync_pull: { cursor: '7', more: false, changes: [] },
            ext_sync_ack: null,
        });
        await api.push('items', [{ id: 'n1', title: 'Milk' }], { deleted: ['n2'], initial: true });
        await api.pull({ max: 10 });
        await api.pull();
        await api.ack('7');
        expect(calls.slice(1)).toEqual([
            [
                'ext_sync_push',
                { collection: 'items', records: [{ id: 'n1', title: 'Milk' }], deleted: ['n2'], initial: true },
            ],
            ['ext_sync_pull', { max: 10 }],
            ['ext_sync_pull', { max: 500 }],
            ['ext_sync_ack', { cursor: '7' }],
        ]);
        for (const [, args] of calls) expect(args).not.toHaveProperty('extensionId');
    });

    it('checks its arguments before the host sees them', async () => {
        const { api, calls } = makeApi({ ext_sync_status: {} });
        const bad = [
            () => api.push('Items', []),
            () => api.push('items', 'x'),
            () => api.push('items', [{ title: 'no id' }]),
            () => api.push('items', [{ id: 'a b' }]),
            () => api.push('items', [], { deleted: [3] }),
            () => api.push('items', Array.from({ length: 501 }, (_, i) => ({ id: `r${i}` }))),
            () => api.pull({ max: 0 }),
            () => api.pull({ max: 501 }),
            () => api.ack(7),
            () => api.ack('12a'),
        ];
        for (const f of bad) {
            await expect(f()).rejects.toMatchObject({ kind: 'invalid_input' });
        }
        expect(() => api.onChanges('nope')).toThrow();
        expect(calls.map((c) => c[0])).toEqual(['ext_sync_status']);
    });

    it('a push that comes back off makes it unavailable', async () => {
        const { api } = makeApi({
            ext_sync_status: { available: true },
            ext_sync_push: { accepted: 0, unchanged: 0, rejected: [], off: true },
        });
        await Promise.resolve();
        await Promise.resolve();
        expect(api.available).toBe(true);
        expect((await api.push('items', [{ id: 'a' }])).off).toBe(true);
        expect(api.available).toBe(false);
    });

    it('delivers onChanges once per burst, with the collections and the strongest reason', () => {
        vi.useFakeTimers();
        const { api, post } = makeApi({ ext_sync_status: {} });
        const seen = [];
        const off = api.onChanges((e) => seen.push(e));
        const other = vi.fn(() => {
            throw new Error('a bad listener');
        });
        api.onChanges(other);
        post({ type: 'sync-changes', collections: ['items'], reason: 'remote' });
        post({ type: 'sync-changes', collections: ['links', 'Bad!'], reason: 'reset' });
        post({ type: 'sync-changes', collections: ['items'], reason: 'remote' });
        post({ type: 'storage-changed', changes: [] });
        post({ type: 'sync-changes', collections: [], reason: 'nonsense' });
        vi.advanceTimersByTime(SYNC_CHANGES_DEBOUNCE_MS - 1);
        expect(seen).toEqual([]);
        vi.advanceTimersByTime(1);
        expect(seen).toEqual([{ collections: ['items', 'links'], reason: 'reset' }]);
        expect(Object.isFrozen(seen[0])).toBe(true);
        expect(other).toHaveBeenCalledTimes(1);
        off();
        post({ type: 'sync-changes', collections: ['items'], reason: 'remote' });
        vi.advanceTimersByTime(SYNC_CHANGES_DEBOUNCE_MS);
        expect(seen).toHaveLength(1);
    });

    it('enabled and purged turn available on and off', () => {
        vi.useFakeTimers();
        const { api, post } = makeApi({ ext_sync_status: new Error('off') });
        post({ type: 'sync-changes', collections: [], reason: 'enabled' });
        expect(api.available).toBe(true);
        post({ type: 'sync-changes', collections: [], reason: 'purged' });
        expect(api.available).toBe(false);
    });
});

describe('context.sync: the runtime offers it only with sync.records', () => {
    const runtime = readFileSync(resolve(UI_ROOT, 'js/extension-sandbox/runtime.js'), 'utf8');

    it('builds context.sync only when the host offered sync.records', () => {
        expect(runtime).toMatch(/hostFeatures\?\.includes\('sync\.records'\)/);
        expect(runtime).toMatch(/context\.sync = syncApi/);
        expect(runtime).toMatch(/case 'sync-changes':/);
    });

    it('is inlined into the runtime before runtime.js', async () => {
        const { RUNTIME_SOURCE_PATHS } = await import('../../ui/js/shared/extension-sandbox-host.js');
        const sync = RUNTIME_SOURCE_PATHS.indexOf('js/extension-sandbox/runtime/sync-api.js');
        expect(sync).toBeGreaterThanOrEqual(0);
        expect(sync).toBeLessThan(RUNTIME_SOURCE_PATHS.indexOf('js/extension-sandbox/runtime.js'));
    });
});

describe('the sync capability and its commands', () => {
    it('is a capability with its icon and label', () => {
        expect(KNOWN_CAPABILITIES).toContain('sync');
        expect(CAPABILITIES.sync).toMatchObject({ icon: '🔄', label: 'Sync between your computers' });
    });

    it('maps ext_sync_* to sync and every sync_* command to null', () => {
        const ext = ['ext_sync_status', 'ext_sync_push', 'ext_sync_pull', 'ext_sync_ack'];
        for (const c of ext) {
            expect(COMMAND_CAPABILITIES[c], c).toBe('sync');
            expect(decideInvoke(c, new Set(['storage'])).allow, c).toBe(false);
            expect(decideInvoke(c, new Set(['sync'])).allow, c).toBe(true);
            expect(decideInvoke(c, new Set(['sync']), { surface: 'view' }).allow, c).toBe(true);
        }
        const host = Object.keys(COMMAND_CAPABILITIES).filter(
            (c) => c.startsWith('sync_') || c === 'get_config_snapshot'
        );
        expect(host.length).toBe(34);
        for (const c of host) {
            expect(COMMAND_CAPABILITIES[c], c).toBeNull();
            expect(decideInvoke(c, new Set(KNOWN_CAPABILITIES)).allow, c).toBe(false);
        }
    });

    it('the host force-injects the extension id into every ext_sync_* call', async () => {
        for (const c of ['ext_sync_status', 'ext_sync_push', 'ext_sync_pull', 'ext_sync_ack']) {
            expect(IDENTITY_SCOPED_COMMANDS[c], c).toBe('extensionId');
        }
        const rawInvoke = vi.fn().mockResolvedValue({ cursor: '0', more: false, changes: [] });
        const sb = new ExtensionSandbox(
            { extensionId: 'mindmap', capabilities: ['storage', 'sync'] },
            rawInvoke,
            document.body
        );
        const sent = [];
        sb._port = { postMessage: (m) => sent.push(m), close: () => {} };
        sb._ready = true;
        await sb._handleInvoke({
            id: 1,
            command: 'ext_sync_pull',
            args: { extensionId: 'other', max: 5 },
        });
        expect(rawInvoke).toHaveBeenCalledWith('ext_sync_pull', { extensionId: 'mindmap', max: 5 });
        // Without the grant the call never reaches the backend.
        const noGrant = new ExtensionSandbox(
            { extensionId: 'mindmap', capabilities: ['storage'] },
            rawInvoke,
            document.body
        );
        noGrant._port = { postMessage: (m) => sent.push(m), close: () => {} };
        noGrant._ready = true;
        rawInvoke.mockClear();
        await noGrant._handleInvoke({ id: 2, command: 'ext_sync_push', args: {} });
        expect(rawInvoke).not.toHaveBeenCalled();
        expect(sent.at(-1).error).toMatch(/missing capability 'sync'/);
    });

    it('an update that adds sync needs approval again', () => {
        const before = { granted: ['storage'] };
        expect(grantCovers(before, ['storage'], 'mindmap')).toBe(true);
        expect(grantCovers(before, ['storage', 'sync'], 'mindmap')).toBe(false);
        expect(grantCovers({ granted: ['storage', 'sync'] }, ['storage', 'sync'], 'mindmap')).toBe(true);
    });
});

describe('the sync host features', () => {
    it('need the sync grant', () => {
        expect(SYNC_HOST_FEATURES).toEqual(['sync.keys', 'sync.lists', 'sync.records']);
        const offered = [HOST_FEATURE_VIEWS, ...SYNC_HOST_FEATURES];
        expect(hostFeaturesFor(offered, ['storage'])).toEqual([]);
        expect(hostFeaturesFor(offered, ['storage', 'sync'])).toEqual([
            HOST_FEATURE_SYNC_KEYS,
            HOST_FEATURE_SYNC_LISTS,
            HOST_FEATURE_SYNC_RECORDS,
        ]);
    });

    it('every window with extension instances offers them', () => {
        const files = {
            'js/shared/extension-manager.js': /SYNC_HOST_FEATURES/,
            'js/settings/main.js': /\.\.\.SYNC_HOST_FEATURES/,
            'js/extension-view/main.js': /'sync\.keys',\s*'sync\.lists',\s*'sync\.records'/,
        };
        for (const [p, re] of Object.entries(files)) {
            expect(readFileSync(resolve(UI_ROOT, p), 'utf8'), p).toMatch(re);
        }
    });
});

describe('sync-changes forwarding', () => {
    function sandbox({ capabilities = ['storage', 'sync'], hostFeatures = SYNC_HOST_FEATURES } = {}) {
        const sb = new ExtensionSandbox(
            { extensionId: 'mindmap', capabilities, hostFeatures },
            vi.fn(),
            document.body
        );
        const sent = [];
        sb._port = { postMessage: (m) => sent.push(m), close: () => {} };
        sb._ready = true;
        return { sb, sent };
    }

    it('reaches a sandbox with sync.records, cleaned', () => {
        const { sb, sent } = sandbox();
        expect(sb.notifySyncChanges(['items', 'Bad!', 3], 'remote')).toBe(true);
        expect(sent).toEqual([{ type: 'sync-changes', collections: ['items'], reason: 'remote' }]);
        expect(sb.notifySyncChanges(['items'], 'whatever')).toBe(false);
        sb.destroy();
        expect(sb.notifySyncChanges(['items'], 'remote')).toBe(false);
    });

    it('never reaches a sandbox without the feature or the grant', () => {
        const { sb: noFeature, sent: a } = sandbox({ hostFeatures: [HOST_FEATURE_VIEWS] });
        expect(noFeature.notifySyncChanges(['items'], 'remote')).toBe(false);
        const { sb: noGrant, sent: b } = sandbox({ capabilities: ['storage'] });
        expect(noGrant.hostFeatures).toEqual([]);
        expect(noGrant.notifySyncChanges(['items'], 'remote')).toBe(false);
        expect([...a, ...b]).toEqual([]);
    });

    it('a payload goes to that extension only', () => {
        const { sb, sent } = sandbox();
        const other = new ExtensionSandbox(
            { extensionId: 'todo', capabilities: ['sync'], hostFeatures: SYNC_HOST_FEATURES },
            vi.fn(),
            document.body
        );
        const otherSent = [];
        other._port = { postMessage: (m) => otherSent.push(m), close: () => {} };
        other._ready = true;
        const n = deliverSyncChanges(
            { extension_id: 'mindmap', collections: ['items'], reason: 'purged' },
            [sb, other]
        );
        expect(n).toBe(1);
        expect(sent).toEqual([{ type: 'sync-changes', collections: ['items'], reason: 'purged' }]);
        expect(otherSent).toEqual([]);
        expect(deliverSyncChanges(null, [sb])).toBe(0);
        expect(deliverSyncChanges({ extension_id: 'mindmap', reason: 'nope' }, [sb])).toBe(0);
    });

    it('listens for extension_sync_changes once per window', async () => {
        const listen = vi.fn(async () => () => {});
        expect(await installExtensionSyncEvents(listen)).toBe(true);
        expect(await installExtensionSyncEvents(listen)).toBe(true);
        expect(listen).toHaveBeenCalledTimes(1);
        expect(listen.mock.calls[0][0]).toBe('extension_sync_changes');
    });
});
