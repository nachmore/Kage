/**
 * Settings → Tool permissions → "Commands outside the agent's sandbox":
 * the rules ("commands starting with …") the backend keeps, each removable
 * through the audited `remove_host_command_rule`; the allow-all switch,
 * turned on only after a plain warning and only through
 * `set_host_commands_allow_all`; a link to the audit log; and save() never
 * changing any of it.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
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

function configWith(rules, allowAll = false) {
    return {
        tool_permissions: {
            trust_all: false,
            terminator_mode: false,
            trusted_kinds: [],
            trusted_mcp_servers: [],
            host_command_rules: rules.map((prefix) => ({
                prefix,
                created_at: '2026-11-01T10:00:00Z',
            })),
            host_commands_allow_all: allowAll,
            tools: [],
        },
    };
}

function mockBackend(rules, allowAll = false) {
    const backend = configWith([...rules], allowAll);
    const mock = setupTauriMock({
        get_config: async () => structuredClone(backend),
        save_config: async () => undefined,
        set_tool_kind_trusted: async () => undefined,
        set_mcp_server_trusted: async () => undefined,
        remove_host_command_rule: async ({ prefix }) => {
            const tp = backend.tool_permissions;
            const before = tp.host_command_rules.length;
            tp.host_command_rules = tp.host_command_rules.filter((r) => r.prefix !== prefix);
            return tp.host_command_rules.length !== before;
        },
        set_host_commands_allow_all: async ({ enabled }) => {
            backend.tool_permissions.host_commands_allow_all = enabled;
        },
        get_permission_audit_log: async () => [
            {
                at: '2026-11-01T10:00:00Z',
                event: 'host_command',
                action: 'auto_approved',
                by: 'rule',
                rule: 'brew install',
                command: 'brew install jq',
            },
            { at: '2026-11-01T09:00:00Z', event: 'denied', tool: 'shell' },
        ],
        get_permission_audit_log_path: async () => '/tmp/audit.jsonl',
    });
    return { ...mock, backend };
}

function mount(config) {
    const mod = new ToolPermissionsSettingsModule();
    document.body.innerHTML = `<div data-section-content="tool-permissions">${mod.render()}</div>`;
    document.getElementById('terminatorMascot')?.remove();
    mod.initialize();
    mod.load(config);
    return mod;
}

const ruleRows = () => [...document.querySelectorAll('#hostCommandRulesList .host-rule-item')];
const prefixes = () => ruleRows().map((r) => r.querySelector('bdi').textContent);
const allowAllBox = () => document.getElementById('hostCommandsAllowAll');

describe('Settings → commands outside the agent sandbox', () => {
    let mock;
    beforeEach(() => {
        vi.spyOn(console, 'error').mockImplementation(() => {});
        Element.prototype.scrollIntoView = vi.fn();
    });
    afterEach(() => {
        teardownTauriMock();
        vi.restoreAllMocks();
        document.body.innerHTML = '';
    });

    it('lists the rules as LTR text, or an empty state', () => {
        mock = mockBackend(['brew install', 'npm i']);
        mount(configWith(['brew install', 'npm i']));
        expect(prefixes()).toEqual(['brew install', 'npm i']);
        expect(ruleRows()[0].querySelector('bdi').getAttribute('dir')).toBe('ltr');
        expect(ruleRows()[0].textContent).toContain('Commands starting with');

        teardownTauriMock();
        mock = mockBackend([]);
        mount(configWith([]));
        expect(ruleRows()).toHaveLength(0);
        expect(document.getElementById('hostCommandRulesList').textContent).toContain(
            'No rules yet.'
        );
    });

    it('renders a hostile prefix as text', () => {
        mock = mockBackend([]);
        mount(configWith(['<img src=x onerror=alert(1)>']));
        expect(document.querySelector('#hostCommandRulesList img')).toBeNull();
        expect(prefixes()).toEqual(['<img src=x onerror=alert(1)>']);
    });

    it('Remove deletes the rule through the backend, not save_config', async () => {
        mock = mockBackend(['brew install', 'npm i']);
        mount(configWith(['brew install', 'npm i']));
        ruleRows()[0].querySelector('button').click();
        await flush();
        expect(mock.invoke).toHaveBeenCalledWith('remove_host_command_rule', {
            prefix: 'brew install',
        });
        expect(mock.invoke).not.toHaveBeenCalledWith('save_config', expect.anything());
        expect(prefixes()).toEqual(['npm i']);
    });

    it('turns allow-all on only after the warning is accepted', async () => {
        mock = mockBackend([]);
        mount(configWith([]));
        const box = allowAllBox();
        expect(box.checked).toBe(false);
        // The description says plainly what it means.
        expect(document.body.textContent).toContain('full access to your computer');
        expect(document.body.textContent).toContain('AWS');

        const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);
        box.checked = true;
        box.dispatchEvent(new Event('change'));
        await flush();
        expect(confirmSpy).toHaveBeenCalled();
        expect(box.checked).toBe(false);
        expect(mock.invoke).not.toHaveBeenCalledWith(
            'set_host_commands_allow_all',
            expect.anything()
        );

        confirmSpy.mockReturnValue(true);
        box.checked = true;
        box.dispatchEvent(new Event('change'));
        await flush();
        expect(mock.invoke).toHaveBeenCalledWith('set_host_commands_allow_all', { enabled: true });
        expect(mock.backend.tool_permissions.host_commands_allow_all).toBe(true);

        // Turning it off needs no warning.
        confirmSpy.mockClear();
        box.checked = false;
        box.dispatchEvent(new Event('change'));
        await flush();
        expect(confirmSpy).not.toHaveBeenCalled();
        expect(mock.invoke).toHaveBeenCalledWith('set_host_commands_allow_all', {
            enabled: false,
        });
    });

    it('re-syncs on show (a prompt in another window may have added a rule)', async () => {
        mock = mockBackend(['brew install'], true);
        const mod = mount(configWith([]));
        expect(ruleRows()).toHaveLength(0);
        await mod.onShow();
        expect(prefixes()).toEqual(['brew install']);
        expect(allowAllBox().checked).toBe(true);
    });

    it('the audit link shows only these commands', async () => {
        mock = mockBackend([]);
        mount(configWith([]));
        await flush();
        document.getElementById('hostCommandAuditLink').click();
        expect(document.getElementById('auditLogFilter').value).toBe('host_command');
        const rows = document.querySelectorAll('#auditLogList .audit-log-row');
        expect(rows).toHaveLength(1);
        expect(rows[0].textContent).toContain('A command ran without asking');
        expect(rows[0].textContent).toContain('brew install jq');
    });

    it('save() keeps the backend rules and switch whatever this window shows', () => {
        mock = mockBackend([]);
        const mod = mount(configWith(['stale-local'], true));
        const fresh = configWith(['brew install'], false);
        mod.save(fresh);
        expect(fresh.tool_permissions.host_command_rules.map((r) => r.prefix)).toEqual([
            'brew install',
        ]);
        expect(fresh.tool_permissions.host_commands_allow_all).toBe(false);
    });

    it('Reset removes every rule and turns allow-all off', async () => {
        mock = mockBackend(['brew install', 'npm i'], true);
        mount(configWith(['brew install']));
        vi.spyOn(window, 'confirm').mockReturnValue(true);
        document.getElementById('resetPermissionsBtn').click();
        await flush(10);
        expect(mock.backend.tool_permissions.host_command_rules).toEqual([]);
        expect(mock.backend.tool_permissions.host_commands_allow_all).toBe(false);
        expect(ruleRows()).toHaveLength(0);
    });
});
