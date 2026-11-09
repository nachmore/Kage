import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
    BULK_KEY,
    CHANGE_OPS,
    CHANGE_SOURCES,
    deliverToSandbox,
    holdsStorage,
    installExtensionDataEvents,
    MAX_BATCH_CHANGES,
    toSandboxChanges,
} from '../../ui/js/shared/extension-data-events.js';
import { EVT } from '../../ui/js/shared/events.js';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

afterEach(() => vi.restoreAllMocks());

/** A stand-in for Tauri's `listen`: `emit(payload)` calls the handlers. */
function fakeListen() {
    const handlers = new Map();
    const unlisten = vi.fn();
    const listen = vi.fn(async (event, handler) => {
        handlers.set(event, handler);
        return unlisten;
    });
    const emit = (payload) => handlers.get(EVT.EXTENSION_DATA_CHANGED)?.({ payload });
    return { listen, emit, unlisten };
}

const batch = (changes, extension_id = 'mindmap') => ({ extension_id, changes });

describe('toSandboxChanges', () => {
    it('adds self from the writing window and drops the label', () => {
        const out = toSandboxChanges(
            batch([
                { key: 'mm-meta', op: 'save', source: 'window', window: 'floating' },
                { key: 'mm-0', op: 'delete', source: 'window', window: 'ext-view:mindmap:explorer' },
                { key: '*', op: 'bulk', source: 'import' },
            ]),
            'floating'
        );
        expect(out).toEqual({
            extensionId: 'mindmap',
            changes: [
                { key: 'mm-meta', op: 'save', self: true, source: 'window' },
                { key: 'mm-0', op: 'delete', self: false, source: 'window' },
                { key: '*', op: 'bulk', self: false, source: 'import' },
            ],
        });
        for (const c of out.changes) expect(c).not.toHaveProperty('window');
    });

    it('is self in a view window that wrote the key', () => {
        const label = 'ext-view:mindmap:explorer';
        const out = toSandboxChanges(
            batch([{ key: 'mm-view', op: 'save', source: 'window', window: label }]),
            label
        );
        expect(out.changes[0].self).toBe(true);
    });

    it('never says self without a window label on both sides', () => {
        const change = { key: 'k', op: 'save', source: 'sync' };
        expect(toSandboxChanges(batch([change]), 'main').changes[0].self).toBe(false);
        const labelled = { ...change, source: 'window', window: 'main' };
        expect(toSandboxChanges(batch([labelled]), undefined).changes[0].self).toBe(false);
    });

    it('drops malformed changes and keeps the rest', () => {
        const out = toSandboxChanges(
            batch([
                null,
                'k',
                { key: 'ok', op: 'save', source: 'window', window: 'main' },
                { key: 'k', op: 'rename', source: 'window' },
                { key: 'k', op: 'save', source: 'elsewhere' },
                { key: 42, op: 'save', source: 'window' },
                { key: '', op: 'save', source: 'window' },
                { key: '../x', op: 'delete', source: 'window' },
                { key: 'a/b', op: 'save', source: 'window' },
                { key: '*', op: 'save', source: 'window' },
                { key: 'k', op: 'bulk', source: 'import' },
                { key: 'x'.repeat(129), op: 'save', source: 'window' },
            ]),
            'main'
        );
        expect(out.changes).toEqual([{ key: 'ok', op: 'save', self: true, source: 'window' }]);
    });

    it('refuses payloads without a valid extension or any valid change', () => {
        const ok = [{ key: 'k', op: 'save', source: 'window' }];
        for (const payload of [
            null,
            undefined,
            'mindmap',
            {},
            { extension_id: 'mindmap' },
            { extension_id: 'mindmap', changes: 'k' },
            { extension_id: 'mindmap', changes: [] },
            { extension_id: 'mindmap', changes: [{ key: 'k', op: 'nope', source: 'window' }] },
            { extension_id: 'Mind Map', changes: ok },
            { extension_id: '../x', changes: ok },
            { extension_id: 42, changes: ok },
        ]) {
            expect(toSandboxChanges(payload, 'main')).toBeNull();
        }
    });

    it('caps a batch at the size Rust sends', () => {
        const many = Array.from({ length: MAX_BATCH_CHANGES + 10 }, (_, i) => ({
            key: `k${i}`,
            op: 'save',
            source: 'window',
        }));
        expect(toSandboxChanges(batch(many), 'main').changes).toHaveLength(MAX_BATCH_CHANGES);
    });
});

describe('holdsStorage and deliverToSandbox', () => {
    it('reads capabilities as a Set or an array', () => {
        expect(holdsStorage({ capabilities: new Set(['storage']) })).toBe(true);
        expect(holdsStorage({ capabilities: ['urls', 'storage'] })).toBe(true);
        expect(holdsStorage({ capabilities: new Set(['urls']) })).toBe(false);
        expect(holdsStorage({ capabilities: 'storage' })).toBe(false);
        expect(holdsStorage({})).toBe(false);
        expect(holdsStorage(null)).toBe(false);
    });

    it('reaches only the matching sandbox, and only with storage', () => {
        const withStorage = {
            capabilities: new Set(['storage']),
            notifyStorageChanged: vi.fn(),
        };
        const withoutStorage = {
            capabilities: new Set(['urls']),
            notifyStorageChanged: vi.fn(),
        };
        const sandboxes = new Map([
            ['mindmap', withStorage],
            ['todos', withoutStorage],
        ]);
        const deliver = deliverToSandbox((id) => sandboxes.get(id));
        const changes = [{ key: 'k', op: 'save', self: false, source: 'window' }];

        expect(deliver('mindmap', changes)).toBe(true);
        expect(withStorage.notifyStorageChanged).toHaveBeenCalledWith(changes);
        expect(deliver('todos', changes)).toBe(false);
        expect(withoutStorage.notifyStorageChanged).not.toHaveBeenCalled();
        expect(deliver('absent', changes)).toBe(false);
        expect(deliverToSandbox(() => ({ capabilities: ['storage'] }))('x', changes)).toBe(false);
    });
});

describe('installExtensionDataEvents', () => {
    it('listens for extension_data_changed and delivers each valid batch', async () => {
        const { listen, emit, unlisten } = fakeListen();
        const deliver = vi.fn();
        const stop = await installExtensionDataEvents({ listen, myLabel: 'settings', deliver });
        expect(listen).toHaveBeenCalledWith('extension_data_changed', expect.any(Function));

        emit(batch([{ key: 'mm-meta', op: 'save', source: 'window', window: 'settings' }]));
        emit(batch([{ key: '*', op: 'bulk', source: 'import' }], 'todos'));
        emit({ extension_id: 'mindmap', changes: [] });
        emit('garbage');

        expect(deliver.mock.calls).toEqual([
            ['mindmap', [{ key: 'mm-meta', op: 'save', self: true, source: 'window' }]],
            ['todos', [{ key: '*', op: 'bulk', self: false, source: 'import' }]],
        ]);
        stop();
        expect(unlisten).toHaveBeenCalled();
    });

    it('reads the label when each batch arrives', async () => {
        const { listen, emit } = fakeListen();
        const deliver = vi.fn();
        let label = 'floating';
        await installExtensionDataEvents({ listen, myLabel: () => label, deliver });
        const payload = batch([{ key: 'k', op: 'save', source: 'window', window: 'main' }]);
        emit(payload);
        label = 'main';
        emit(payload);
        expect(deliver.mock.calls.map(([, changes]) => changes[0].self)).toEqual([false, true]);
    });

    it('end to end with deliverToSandbox: one extension, one sandbox', async () => {
        const { listen, emit } = fakeListen();
        const mindmap = { capabilities: new Set(['storage']), notifyStorageChanged: vi.fn() };
        const todos = { capabilities: new Set(['storage']), notifyStorageChanged: vi.fn() };
        const sandboxes = new Map([
            ['mindmap', mindmap],
            ['todos', todos],
        ]);
        await installExtensionDataEvents({
            listen,
            myLabel: 'floating',
            deliver: deliverToSandbox((id) => sandboxes.get(id)),
        });
        emit(batch([{ key: 'mm-meta', op: 'save', source: 'window', window: 'main' }]));
        expect(mindmap.notifyStorageChanged).toHaveBeenCalledTimes(1);
        expect(todos.notifyStorageChanged).not.toHaveBeenCalled();
    });

    it('keeps listening when a delivery throws', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const { listen, emit } = fakeListen();
        const deliver = vi.fn(() => {
            throw new Error('boom');
        });
        await installExtensionDataEvents({ listen, myLabel: 'main', deliver });
        const payload = batch([{ key: 'k', op: 'save', source: 'window' }]);
        emit(payload);
        emit(payload);
        expect(deliver).toHaveBeenCalledTimes(2);
        expect(warn).toHaveBeenCalled();
    });

    it('needs listen and deliver', async () => {
        await expect(installExtensionDataEvents({ myLabel: 'main', deliver() {} })).rejects.toThrow(
            TypeError
        );
        await expect(
            installExtensionDataEvents({ listen: async () => () => {}, myLabel: 'main' })
        ).rejects.toThrow(TypeError);
    });

    it('returns a no-op stop when listen gives none', async () => {
        const stop = await installExtensionDataEvents({
            listen: async () => undefined,
            myLabel: 'main',
            deliver() {},
        });
        expect(() => stop()).not.toThrow();
    });
});

describe('Rust alignment', () => {
    const rust = readFileSync(path.join(repoRoot, 'src/extension_data_events.rs'), 'utf8');

    /** Variant names of a `#[serde(rename_all = "snake_case")]` enum. */
    function snakeVariants(name) {
        const m = rust.match(new RegExp(`pub enum ${name}\\s*\\{([\\s\\S]*?)\\}`));
        expect(m, `${name} not found`).not.toBeNull();
        return m[1]
            .split('\n')
            .map((line) => line.replace(/\/\/.*$/, '').trim())
            .filter((line) => /^[A-Z]\w*,?$/.test(line))
            .map((line) => line.replace(/,$/, ''))
            .map((v) => v.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase());
    }

    it('names the same ops and sources', () => {
        expect(snakeVariants('ChangeOp')).toEqual([...CHANGE_OPS]);
        expect(snakeVariants('ChangeSource')).toEqual([...CHANGE_SOURCES]);
    });

    it('uses the same bulk key and batch cap', () => {
        expect(rust).toContain(`pub const BULK_KEY: &str = "${BULK_KEY}";`);
        expect(rust).toContain(`pub const MAX_BATCH_KEYS: usize = ${MAX_BATCH_CHANGES};`);
    });
});
