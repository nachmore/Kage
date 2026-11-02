/**
 * Tests for ui/js/settings/permissions.js — the macOS Permissions settings
 * section. Rendering lives in shared/mac-permissions.js (covered by
 * shared/mac-permissions.test.js); this locks in the section lifecycle:
 * initialize() renders the cards, load() (called each time the section is
 * shown) re-reads live status, destroy() drops the focus listeners, and
 * the section never holds unsaved edits.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import { MacPermissionsSettingsModule } from '../../ui/js/settings/permissions.js';

const flush = async () => {
    for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
};

let tauri;
let statuses;
let mod;

const statusCalls = () =>
    tauri.invoke.mock.calls.filter(([cmd]) => cmd === 'get_privacy_permissions').length;

beforeEach(() => {
    statuses = { accessibility: 'not_granted', 'screen-recording': 'granted' };
    tauri = setupTauriMock({
        get_privacy_permissions: async () =>
            Object.entries(statuses).map(([id, status]) => ({ id, status })),
    });
    mod = new MacPermissionsSettingsModule();
    document.body.innerHTML = `<div data-section-content="mac-permissions">${mod.render()}</div>`;
});

afterEach(() => {
    mod.destroy();
    teardownTauriMock();
    document.body.innerHTML = '';
});

describe('MacPermissionsSettingsModule', () => {
    it('is a read-only pane the manager never treats as dirty', () => {
        expect(mod.persistsImmediately).toBe(true);
        expect(mod.validate()).toEqual({ valid: true });
        expect(() => mod.save({})).not.toThrow();
    });

    it('renders the cards with live status on initialize()', async () => {
        mod.initialize();
        await flush();
        const ax = document.querySelector('.mac-perm-card[data-perm-id="accessibility"]');
        expect(ax).not.toBeNull();
        expect(ax.dataset.status).toBe('not_granted');
        expect(document.querySelector('#macPermSetting-accessibility-btn')).not.toBeNull();
    });

    it('re-reads status when the section is shown again (load)', async () => {
        mod.initialize();
        await flush();
        statuses.accessibility = 'granted';
        const before = statusCalls();
        mod.load({});
        await flush();
        expect(statusCalls()).toBe(before + 1);
        const ax = document.querySelector('.mac-perm-card[data-perm-id="accessibility"]');
        expect(ax.dataset.status).toBe('granted');
    });

    it('load() before initialize() is a harmless no-op', () => {
        expect(() => mod.load({})).not.toThrow();
        expect(statusCalls()).toBe(0);
    });

    it('stops refreshing on window focus after destroy()', async () => {
        mod.initialize();
        await flush();
        mod.destroy();
        const before = statusCalls();
        window.dispatchEvent(new Event('focus'));
        await flush();
        expect(statusCalls()).toBe(before);
    });
});
