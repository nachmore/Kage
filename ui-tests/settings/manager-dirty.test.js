/**
 * SettingsManager unsaved-edit tracking:
 *   - a section with unsaved edits keeps them across tab switches and
 *     config_updated broadcasts (it is never reloaded from config);
 *   - a clean section is reloaded from fresh config when shown and when
 *     config_updated arrives while it's visible, so changes made in other
 *     windows show up;
 *   - Save writes only dirty sections, so a clean section's stale DOM can't
 *     overwrite newer config, and clears dirty state only on success.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';

let SettingsManager;
let SettingsModule;
let backend;
let saved;
let tauri;
let saveGate;

function makeModule(id, opts = {}) {
    class FieldModule extends SettingsModule {
        constructor() {
            super(id, id, '');
            this.loads = 0;
            this.saves = 0;
            if (opts.persistsImmediately) this.persistsImmediately = true;
        }
        render() {
            return `<input id="${id}-field"><button id="${id}-btn">b</button><div id="${id}-plain">p</div>`;
        }
        load(config) {
            this.loads++;
            document.getElementById(`${id}-field`).value = config[id] ?? '';
        }
        save(config) {
            this.saves++;
            config[id] = document.getElementById(`${id}-field`).value;
        }
    }
    return new FieldModule();
}

const flush = () => new Promise((r) => setTimeout(r, 0));
const field = (id) => document.getElementById(`${id}-field`);

function type(id, value) {
    const el = field(id);
    el.value = value;
    el.dispatchEvent(new Event('input', { bubbles: true }));
}

async function setup(modules) {
    const mgr = new SettingsManager();
    modules.forEach((m) => mgr.registerModule(m));
    mgr.render();
    await mgr.load();
    await flush();
    return mgr;
}

beforeEach(async () => {
    vi.resetModules();
    document.body.innerHTML = '<div id="settingsModules"></div>';
    backend = { a: 'a0', b: 'b0', c: 'c0' };
    saved = [];
    saveGate = null;
    tauri = setupTauriMock({
        get_config: async () => structuredClone(backend),
        save_config: async ({ config }) => {
            if (saveGate) await saveGate;
            saved.push(structuredClone(config));
            backend = structuredClone(config);
        },
    });
    ({ SettingsManager } = await import('../../ui/js/settings/manager.js'));
    ({ SettingsModule } = await import('../../ui/js/settings/base.js'));
});

afterEach(() => {
    teardownTauriMock();
});

describe('tab switches', () => {
    it('keep unsaved edits and refresh clean sections from fresh config', async () => {
        const a = makeModule('a');
        const b = makeModule('b');
        const mgr = await setup([a, b]);

        type('a', 'edited');
        expect(mgr.isDirty('a')).toBe(true);

        // Another window changes both values.
        backend = { ...backend, a: 'a-ext', b: 'b-ext' };

        mgr.switchSection('b'); // first reveal: initialize + load
        await flush();
        expect(field('b').value).toBe('b-ext');

        mgr.switchSection('a');
        await flush();
        expect(field('a').value).toBe('edited');

        backend = { ...backend, b: 'b-ext2' };
        mgr.switchSection('b'); // later reveal of a clean section reloads it
        await flush();
        expect(field('b').value).toBe('b-ext2');
    });

    it('call onShow instead of reloading a dirty section', async () => {
        const a = makeModule('a');
        const b = makeModule('b');
        b.onShow = vi.fn();
        const mgr = await setup([a, b]);
        mgr.switchSection('b');
        await flush();
        type('b', 'mine');
        mgr.switchSection('a');
        mgr.switchSection('b');
        await flush();
        expect(b.onShow).toHaveBeenCalledTimes(1);
        expect(field('b').value).toBe('mine');
    });
});

describe('config_updated', () => {
    it('refreshes the visible clean section but not a dirty one', async () => {
        const a = makeModule('a');
        const b = makeModule('b');
        const mgr = await setup([a, b]);

        backend = { ...backend, a: 'a-ext' };
        tauri.emit('config_updated', null);
        await new Promise((r) => setTimeout(r, 200));
        expect(field('a').value).toBe('a-ext');

        type('a', 'edited');
        backend = { ...backend, a: 'a-ext2' };
        tauri.emit('config_updated', null);
        await new Promise((r) => setTimeout(r, 200));
        expect(field('a').value).toBe('edited');
        expect(mgr.isDirty('a')).toBe(true);
    });
});

describe('dirty detection', () => {
    it('counts button clicks but not clicks on plain content', async () => {
        const a = makeModule('a');
        const mgr = await setup([a]);
        document.getElementById('a-plain').click();
        expect(mgr.isDirty('a')).toBe(false);
        document.getElementById('a-btn').click();
        expect(mgr.isDirty('a')).toBe(true);
    });

    it('honours markDirty() from the module', async () => {
        const a = makeModule('a');
        const mgr = await setup([a]);
        a.markDirty();
        expect(mgr.isDirty('a')).toBe(true);
    });

    it('never marks persistsImmediately modules', async () => {
        const a = makeModule('a', { persistsImmediately: true });
        const mgr = await setup([a]);
        type('a', 'x');
        expect(mgr.isDirty('a')).toBe(false);
    });

    it('a full load() clears dirty state', async () => {
        const a = makeModule('a');
        const mgr = await setup([a]);
        type('a', 'x');
        await mgr.load();
        expect(mgr.isDirty('a')).toBe(false);
        expect(field('a').value).toBe('a0');
    });
});

describe('save', () => {
    it('writes only dirty sections, keeping newer config for clean ones', async () => {
        const a = makeModule('a');
        const b = makeModule('b');
        const mgr = await setup([a, b]);
        mgr.switchSection('b');
        await flush();
        mgr.switchSection('a');
        await flush();

        type('a', 'edited');
        // Changed elsewhere while b's DOM still shows b0.
        backend = { ...backend, b: 'b-ext' };

        await mgr.save();
        expect(saved).toHaveLength(1);
        expect(saved[0].a).toBe('edited');
        expect(saved[0].b).toBe('b-ext');
        expect(b.saves).toBe(0);
        expect(mgr.isDirty('a')).toBe(false);
    });

    it('still calls save_config when nothing is dirty', async () => {
        const mgr = await setup([makeModule('a')]);
        await mgr.save();
        expect(saved).toEqual([{ a: 'a0', b: 'b0', c: 'c0' }]);
    });

    it('keeps sections edited during an in-flight save dirty', async () => {
        const a = makeModule('a');
        const mgr = await setup([a]);
        type('a', 'first');
        let release;
        saveGate = new Promise((r) => {
            release = r;
        });
        const p = mgr.save();
        await flush();
        type('a', 'second');
        release();
        await p;
        expect(saved[0].a).toBe('first');
        expect(mgr.isDirty('a')).toBe(true);
    });

    it('keeps dirty state when save_config fails', async () => {
        const a = makeModule('a');
        const mgr = await setup([a]);
        type('a', 'edited');
        tauri.invoke.mockImplementationOnce(async () => structuredClone(backend));
        tauri.invoke.mockImplementationOnce(async () => {
            throw new Error('disk full');
        });
        const ok = await mgr.save();
        expect(ok).toBe(false);
        expect(mgr.isDirty('a')).toBe(true);
    });

    it('skips validation of clean sections', async () => {
        const a = makeModule('a');
        const b = makeModule('b');
        b.validate = () => ({ valid: false, error: 'nope' });
        const mgr = await setup([a, b]);
        type('a', 'edited');
        await mgr.save();
        expect(saved).toHaveLength(1);
    });
});
