/**
 * "Update all" marks the extensions it updated in the Settings sidebar, and
 * the marks go when Settings is closed (the backend calls
 * window.__kageClearUpdatedExtensions when it shows Settings again).
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';

let mgr;

beforeEach(async () => {
    setupTauriMock();
    document.body.innerHTML = '<div id="extensionsSidebarSection"><div class="sidebar-item" data-section="store">Store</div></div>';
    mgr = await import('../../ui/js/settings/manager.js');
    mgr.clearUpdatedExtensions();
});

afterEach(() => {
    teardownTauriMock();
    document.body.innerHTML = '';
});

const badgeFor = (id) => document.querySelector(`.sidebar-item[data-section="ext-${id}"] .sidebar-item-badge`);

describe('updated marks in the Settings sidebar', () => {
    it('marks exactly the updated extensions, keeps alphabetical order, and clears', () => {
        mgr.addExtensionSidebarItem('ext-weather', '🌤', 'Weather');
        mgr.addExtensionSidebarItem('ext-stocks', '📈', 'Stocks');
        mgr.addExtensionSidebarItem('ext-math', '🧮', 'Math');
        mgr.markUpdatedExtensions(['stocks', 'weather', 42, '']);
        expect(badgeFor('stocks')).toBeTruthy();
        expect(badgeFor('weather')).toBeTruthy();
        expect(badgeFor('math')).toBeNull();
        expect(mgr.updatedExtensionIds().sort()).toEqual(['stocks', 'weather']);

        // A badge must not break the alphabetical insert of later items.
        mgr.addExtensionSidebarItem('ext-todos', '✅', 'Todos');
        const labels = [...document.querySelectorAll('.sidebar-item[data-ext-sidebar="true"] .sidebar-item-label')].map((el) => el.textContent);
        expect(labels).toEqual(['Math', 'Stocks', 'Todos', 'Weather']);

        window.__kageClearUpdatedExtensions();
        expect(document.querySelectorAll('.sidebar-item-badge')).toHaveLength(0);
        expect(document.querySelectorAll('.sidebar-item.is-updated')).toHaveLength(0);
    });

    it('re-applies the mark to an item rebuilt after the update (extensions_changed)', () => {
        mgr.markUpdatedExtensions(['stocks']);
        mgr.addExtensionSidebarItem('ext-stocks', '📈', 'Stocks');
        expect(badgeFor('stocks')).toBeTruthy();
    });

    it('ignores a result with no updated_ids', () => {
        mgr.addExtensionSidebarItem('ext-stocks', '📈', 'Stocks');
        mgr.markUpdatedExtensions(undefined);
        expect(document.querySelectorAll('.sidebar-item-badge')).toHaveLength(0);
    });
});
