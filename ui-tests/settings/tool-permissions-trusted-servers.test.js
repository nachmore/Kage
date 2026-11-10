/**
 * Settings → Tool Permissions: trusted MCP servers. The list reflects the
 * backend's `trusted_mcp_servers`, each server can be revoked through the
 * audited `set_mcp_server_trusted` command (never by editing config in JS),
 * Reset revokes every server the backend trusts, and save() never changes
 * the list.
 */
import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import { initI18n } from '../../ui/js/shared/i18n.js';

const EN_CATALOG = JSON.parse(
    readFileSync(
        join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'locales', 'en', 'messages.json'),
        'utf-8'
    )
);

const flush = (ms = 0) => new Promise((r) => setTimeout(r, ms));

let ToolPermissionsSettingsModule;
beforeAll(async () => {
    await initI18n(async () => ({
        language: 'en',
        rtl: false,
        catalog: EN_CATALOG,
        fallback: EN_CATALOG,
    }));
    ({ ToolPermissionsSettingsModule } = await import('../../ui/js/settings/tool-permissions.js'));
});

function configWith(servers, extra = {}) {
    return {
        tool_permissions: {
            trust_all: false,
            terminator_mode: false,
            trusted_kinds: [],
            trusted_mcp_servers: servers,
            tools: [],
            ...extra,
        },
    };
}

/** Backend stand-in: get_config returns `backend`, set_mcp_server_trusted edits it. */
function mockBackend(servers, { failFor = null } = {}) {
    const backend = configWith([...servers]);
    const mock = setupTauriMock({
        get_config: async () => structuredClone(backend),
        get_config_snapshot: async () => ({ revision: 0, config: structuredClone(backend) }),
        save_config: async () => undefined,
        set_tool_kind_trusted: async () => undefined,
        set_mcp_server_trusted: async ({ server, trusted }) => {
            if (server === failFor) throw { kind: 'internal', message: 'disk full' };
            const list = backend.tool_permissions.trusted_mcp_servers;
            backend.tool_permissions.trusted_mcp_servers = trusted
                ? [...new Set([...list, server])]
                : list.filter((s) => s !== server);
        },
        get_permission_audit_log: async () => [],
        get_permission_audit_log_path: async () => '/tmp/audit.jsonl',
    });
    return { ...mock, backend };
}

function mount(config) {
    const mod = new ToolPermissionsSettingsModule();
    document.body.innerHTML = `<div data-section-content="tool-permissions">${mod.render()}</div>`;
    // The mascot loads an SVG over fetch; not under test here.
    document.getElementById('terminatorMascot')?.remove();
    mod.initialize();
    mod.load(config);
    return mod;
}

const rows = () => [...document.querySelectorAll('#trustedMcpServersList .trusted-server-item')];
const names = () => rows().map((r) => r.querySelector('bdi').textContent);
const revokeBtn = (server) =>
    rows()
        .find((r) => r.querySelector('bdi').textContent === server)
        ?.querySelector('button');

describe('Settings → trusted MCP servers', () => {
    let mock;
    beforeEach(() => {
        vi.spyOn(console, 'error').mockImplementation(() => {});
    });
    afterEach(() => {
        teardownTauriMock();
        vi.restoreAllMocks();
        document.body.innerHTML = '';
    });

    it('lists each trusted server as LTR text, or an empty state', async () => {
        mock = mockBackend(['builder-mcp', 'kage-computer-control']);
        mount(configWith(['builder-mcp', 'kage-computer-control']));
        expect(names()).toEqual(['builder-mcp', 'kage-computer-control']);
        expect(rows()[0].querySelector('bdi').getAttribute('dir')).toBe('ltr');
        expect(revokeBtn('builder-mcp').textContent).toBe('Stop trusting');

        teardownTauriMock();
        mock = mockBackend([]);
        mount(configWith([]));
        expect(rows()).toHaveLength(0);
        expect(document.getElementById('trustedMcpServersList').textContent).toContain(
            'No MCP servers are trusted.'
        );
    });

    it('renders a hostile name as text', () => {
        mock = mockBackend([]);
        mount(configWith(['<img src=x onerror=alert(1)>']));
        expect(document.querySelector('#trustedMcpServersList img')).toBeNull();
        expect(names()).toEqual(['<img src=x onerror=alert(1)>']);
    });

    it('Stop trusting revokes through the backend command and drops the row', async () => {
        mock = mockBackend(['builder-mcp', 'kage-computer-control']);
        mount(configWith(['builder-mcp', 'kage-computer-control']));
        revokeBtn('builder-mcp').click();
        await flush();
        expect(mock.invoke).toHaveBeenCalledWith('set_mcp_server_trusted', {
            server: 'builder-mcp',
            trusted: false,
        });
        expect(mock.invoke).not.toHaveBeenCalledWith('save_config', expect.anything());
        expect(names()).toEqual(['kage-computer-control']);
    });

    it('keeps the row and says so when revoking fails', async () => {
        mock = mockBackend(['builder-mcp'], { failFor: 'builder-mcp' });
        mount(configWith(['builder-mcp']));
        revokeBtn('builder-mcp').click();
        await flush();
        expect(names()).toEqual(['builder-mcp']);
        expect(revokeBtn('builder-mcp').disabled).toBe(false);
        const status = document.getElementById('trustedMcpServersStatus');
        expect(status.textContent).toBe("Couldn't stop trusting builder-mcp. Try again.");
        expect(status.querySelector('bdi').textContent).toBe('builder-mcp');
    });

    it('re-syncs on show (a prompt in another window may have trusted one)', async () => {
        mock = mockBackend(['builder-mcp']);
        const mod = mount(configWith([]));
        expect(rows()).toHaveLength(0);
        await mod.onShow();
        expect(names()).toEqual(['builder-mcp']);
    });

    it('Reset revokes every server the backend trusts, not just the listed ones', async () => {
        // The window loaded before "other-mcp" was trusted from a prompt.
        mock = mockBackend(['builder-mcp', 'other-mcp']);
        mount(configWith(['builder-mcp']));
        vi.spyOn(window, 'confirm').mockReturnValue(true);
        document.getElementById('resetPermissionsBtn').click();
        await flush(10);
        const revoked = mock.invoke.mock.calls
            .filter(([cmd]) => cmd === 'set_mcp_server_trusted')
            .map(([, a]) => [a.server, a.trusted]);
        expect(revoked).toEqual([
            ['builder-mcp', false],
            ['other-mcp', false],
        ]);
        expect(mock.backend.tool_permissions.trusted_mcp_servers).toEqual([]);
        const saved = mock.invoke.mock.calls.find(([cmd]) => cmd === 'save_config')?.[1].config;
        expect(saved.tool_permissions.tools).toEqual([]);
        expect(saved.tool_permissions.trusted_mcp_servers).toEqual([]);
        expect(rows()).toHaveLength(0);
    });

    it('save() keeps the backend list whatever this window shows', () => {
        mock = mockBackend([]);
        const mod = mount(configWith(['stale-local']));
        const fresh = configWith(['builder-mcp']);
        mod.save(fresh);
        expect(fresh.tool_permissions.trusted_mcp_servers).toEqual(['builder-mcp']);
        const old = { tool_permissions: { tools: [] } };
        mod.save(old);
        expect(old.tool_permissions.trusted_mcp_servers).toEqual([]);
    });
});
