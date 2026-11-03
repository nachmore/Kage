/**
 * Settings → Agents: the "Use my shell's environment for agent tools"
 * toggle (`acp.agent.import_login_shell_env`). Defaults on for configs
 * written before it existed, survives re-renders and the edit view
 * (the list view's DOM is gone while a connection is edited), and is
 * hidden on Windows, where the backend never probes a login shell.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';

const flush = () => new Promise((r) => setTimeout(r, 0));

function setPlatform(value) {
    Object.defineProperty(navigator, 'platform', { value, configurable: true });
}

// shortcuts.js caches the platform on first use; re-import per case.
async function freshModule() {
    vi.resetModules();
    const { ConnectionSettingsModule } = await import('../../ui/js/settings/connection.js');
    return new ConnectionSettingsModule();
}

function configWith(agent) {
    return {
        acp: {
            connections: [
                {
                    id: 'c1',
                    name: 'Remote',
                    preset_id: null,
                    mode: { type: 'remote', host: '127.0.0.1', port: 8765, timeout_ms: 30000 },
                    sessions_directory: null,
                },
            ],
            active_connection_id: 'c1',
            agent,
        },
    };
}

async function mount(agent) {
    const mod = await freshModule();
    document.body.innerHTML = `<div data-section-content="connection">${mod.render()}</div>`;
    mod.initialize();
    mod.load(configWith(agent));
    await flush();
    return mod;
}

const checkbox = () => document.getElementById('importLoginShellEnv');

function saved(mod) {
    const config = configWith({});
    mod.save(config);
    return config.acp.agent.import_login_shell_env;
}

describe('ConnectionSettingsModule login-shell environment toggle', () => {
    beforeEach(() => {
        setPlatform('MacIntel');
        setupTauriMock({
            validate_agent_connection: async () => ({ ok: true, issues: [], resolved_path: null }),
            probe_connection_version: async () => null,
            detect_agents: async () => [],
            list_agent_presets: async () => [],
        });
    });

    afterEach(() => {
        teardownTauriMock();
        document.body.innerHTML = '';
    });

    it('defaults to on when the config predates the setting', async () => {
        const mod = await mount({ start_session_on_launch: true });
        expect(checkbox()).not.toBeNull();
        expect(checkbox().checked).toBe(true);
        expect(saved(mod)).toBe(true);
    });

    it('loads off and saves a toggle back on, across re-renders', async () => {
        const mod = await mount({ import_login_shell_env: false });
        expect(checkbox().checked).toBe(false);
        expect(saved(mod)).toBe(false);

        checkbox().checked = true;
        checkbox().dispatchEvent(new Event('change', { bubbles: true }));
        // Async results (validation, detection) re-render the list view.
        mod._renderRoot();
        expect(checkbox().checked).toBe(true);
        expect(saved(mod)).toBe(true);
    });

    it('keeps the toggle while a connection is being edited', async () => {
        const mod = await mount({ import_login_shell_env: true });
        checkbox().checked = false;
        checkbox().dispatchEvent(new Event('change', { bubbles: true }));
        mod._enterEdit(mod._connections[0], { isNew: false });
        expect(checkbox()).toBeNull();
        expect(saved(mod)).toBe(false);
    });

    it('is not shown on Windows', async () => {
        setPlatform('Win32');
        const mod = await mount({ import_login_shell_env: false });
        expect(checkbox()).toBeNull();
        // Saving from Windows doesn't flip the stored preference.
        expect(saved(mod)).toBe(false);
    });
});
