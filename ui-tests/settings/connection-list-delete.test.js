/**
 * Settings → Agents → Connections: the delete icon button on each
 * saved-connection row.
 *
 * The row for the ACTIVE connection (and the row of the single
 * remaining connection) renders a VISIBLE but DISABLED delete button
 * with a tooltip explaining why. Previously the button was simply
 * omitted, which left users wondering whether their delete action had
 * silently failed. See docs/… / commit message for context.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import { initEnglish } from '../helpers/sync-i18n.js';
import * as i18n from '../../ui/js/shared/i18n.js';

const flush = () => new Promise((r) => setTimeout(r, 0));

function setPlatform(value) {
    Object.defineProperty(navigator, 'platform', { value, configurable: true });
}

async function freshModule() {
    vi.resetModules();
    // Re-initialise i18n in the fresh module graph — i18n.js caches the
    // catalog in module scope, and resetModules gives us a new copy.
    const freshI18n = await import('../../ui/js/shared/i18n.js');
    await initEnglish(freshI18n);
    const { ConnectionSettingsModule } = await import('../../ui/js/settings/connection.js');
    return { mod: new ConnectionSettingsModule(), t: freshI18n.t };
}

function configWithConnections(connections, activeId) {
    return {
        acp: {
            connections,
            active_connection_id: activeId,
            agent: { start_session_on_launch: true, import_login_shell_env: true },
        },
    };
}

function makeConn(id, name) {
    return {
        id,
        name,
        preset_id: null,
        mode: { type: 'remote', host: '127.0.0.1', port: 8765, timeout_ms: 30000 },
        sessions_directory: null,
    };
}

async function mount(connections, activeId) {
    const { mod, t } = await freshModule();
    document.body.innerHTML = `<div data-section-content="connection">${mod.render()}</div>`;
    mod.initialize();
    mod.load(configWithConnections(connections, activeId));
    await flush();
    return { mod, t };
}

function deleteButtons() {
    return [...document.querySelectorAll('.conn-delete-btn')];
}

function deleteButtonOnActiveRow() {
    const activeRow = document.querySelector('.conn-row.conn-row-active');
    return activeRow?.querySelector('.conn-delete-btn') || null;
}

function deleteButtonOnRow(id) {
    const row = document.querySelector(`.conn-row[data-id="${id}"]`);
    return row?.querySelector('.conn-delete-btn') || null;
}

describe('ConnectionSettingsModule delete button gating', () => {
    beforeEach(async () => {
        setPlatform('MacIntel');
        setupTauriMock({
            validate_agent_connection: async () => ({ ok: true, issues: [], resolved_path: null }),
            probe_connection_version: async () => null,
            detect_agents: async () => [],
            list_agent_presets: async () => [],
        });
        await initEnglish(i18n);
    });

    afterEach(() => {
        teardownTauriMock();
        document.body.innerHTML = '';
    });

    it('renders a visible but disabled delete button on the active row (two connections)', async () => {
        const { t } = await mount([makeConn('c1', 'Primary'), makeConn('c2', 'Spare')], 'c1');

        // One delete button per row — no longer conditionally omitted.
        expect(deleteButtons()).toHaveLength(2);

        const btn = deleteButtonOnActiveRow();
        expect(btn).not.toBeNull();
        expect(btn.hasAttribute('disabled')).toBe(true);
        expect(btn.getAttribute('aria-disabled')).toBe('true');
        expect(btn.getAttribute('tabindex')).toBe('-1');

        const disabledTooltip = t('settings.connection.row.delete_disabled_title');
        expect(disabledTooltip).toContain('Add a different agent');
        expect(btn.getAttribute('title')).toBe(disabledTooltip);
        expect(btn.getAttribute('aria-label')).toBe(disabledTooltip);
    });

    it('clicking the disabled delete button is a no-op', async () => {
        const { mod } = await mount(
            [makeConn('c1', 'Primary'), makeConn('c2', 'Spare')],
            'c1'
        );
        const spy = vi.spyOn(mod, '_deleteConnection');
        const btn = deleteButtonOnActiveRow();
        btn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
        expect(spy).not.toHaveBeenCalled();
    });

    it('clicking the enabled delete button on a non-active row deletes it', async () => {
        const { mod } = await mount(
            [makeConn('c1', 'Primary'), makeConn('c2', 'Spare')],
            'c1'
        );
        const spy = vi.spyOn(mod, '_deleteConnection');
        const btn = deleteButtonOnRow('c2');
        expect(btn).not.toBeNull();
        expect(btn.hasAttribute('disabled')).toBe(false);
        btn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
        expect(spy).toHaveBeenCalledWith('c2');
    });

    it('single connection: the sole row still shows a disabled delete button', async () => {
        await mount([makeConn('c1', 'Only')], 'c1');
        // Regression guard for the `length > 1` branch: with only one
        // connection, the button is present-but-disabled rather than
        // omitted.
        const btns = deleteButtons();
        expect(btns).toHaveLength(1);
        expect(btns[0].hasAttribute('disabled')).toBe(true);
        expect(btns[0].getAttribute('aria-disabled')).toBe('true');
    });

    it('aria-label on the disabled button resolves via the i18n key', async () => {
        const { t } = await mount(
            [makeConn('c1', 'Primary'), makeConn('c2', 'Spare')],
            'c1'
        );
        const btn = deleteButtonOnActiveRow();
        // The exact catalog message threads through — translators get a
        // single hook to localise.
        expect(btn.getAttribute('aria-label')).toBe(
            t('settings.connection.row.delete_disabled_title')
        );
    });
});
