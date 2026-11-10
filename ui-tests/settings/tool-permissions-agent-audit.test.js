/**
 * Settings → Tool permissions → Audit log, for background agent calls
 * (docs/design/EXTENSION_AGENT_CALLS.md §4.5, §12 "audit viewer"): the two
 * new event kinds render as 🌙 rows that name the extension, the agent
 * (left to right), the purpose and the outcome; extension-supplied text is
 * escaped; and the "Background AI" filter shows both kinds and nothing
 * else.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import * as i18n from '../../ui/js/shared/i18n.js';
import { setSettingsManager } from '../../ui/js/settings/module-registry.js';
import { initEnglish } from '../helpers/sync-i18n.js';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';

const flush = (ms = 0) => new Promise((r) => setTimeout(r, ms));

const ENTRIES = [
    {
        at: '2026-11-09T10:00:05.000Z',
        event: 'extension_agent_call',
        extension_id: 'mindmap',
        call_id: 'c3',
        purpose: 'organize',
        label: '<img src=x onerror=alert(1)>',
        agent: 'Work agent <b>x</b>',
        outcome: 'tool_attempted',
        prompt_chars: 12000,
        response_chars: 0,
        duration_ms: 900,
        redactions: 0,
    },
    {
        at: '2026-11-09T10:00:04.000Z',
        event: 'extension_agent_access_changed',
        extension_id: 'mindmap',
        enabled: true,
        agent: 'Kiro',
    },
    {
        at: '2026-11-09T10:00:03.000Z',
        event: 'extension_agent_access_changed',
        extension_id: 'gone-ext',
        enabled: false,
    },
    { at: '2026-11-09T09:00:00.000Z', event: 'denied', tool: 'shell' },
];

let ToolPermissionsSettingsModule;
beforeAll(async () => {
    await initEnglish(i18n);
    ({ ToolPermissionsSettingsModule } = await import('../../ui/js/settings/tool-permissions.js'));
});

function mount() {
    setupTauriMock({
        get_permission_audit_log: async () => structuredClone(ENTRIES),
        get_permission_audit_log_path: async () => '/tmp/audit.jsonl',
    });
    const mod = new ToolPermissionsSettingsModule();
    document.body.innerHTML = `<div data-section-content="tool-permissions">${mod.render()}</div>`;
    document.getElementById('terminatorMascot')?.remove();
    mod.initialize();
    return mod;
}

const rows = () => [...document.querySelectorAll('#auditLogList .audit-log-row')];

describe('Background AI in the audit log', () => {
    beforeEach(() => {
        vi.spyOn(console, 'error').mockImplementation(() => {});
        // The extension's Settings page gives its display name.
        setSettingsManager({ modules: [{ _extensionId: 'mindmap', title: 'Mind Map' }] });
    });
    afterEach(() => {
        teardownTauriMock();
        setSettingsManager(null);
        vi.restoreAllMocks();
        document.body.innerHTML = '';
    });

    it('renders a call and the switch turning on and off', async () => {
        mount();
        await flush();
        const [call, on, off] = rows();
        expect(call.querySelector('.audit-log-icon').textContent).toBe('🌙');
        const summary = call.querySelector('.audit-log-summary');
        expect(summary.textContent).toBe(
            'Mind Map sent 12,000 characters to Work agent <b>x</b> (organize): Stopped: the agent tried a tool'
        );
        expect(summary.querySelector('strong').textContent).toBe('Mind Map');
        expect(summary.querySelector('bdi[dir="ltr"]').textContent).toBe('Work agent <b>x</b>');
        expect(summary.querySelector('b')).toBeNull();
        // The label is the extension's text: escaped.
        expect(call.querySelector('img')).toBeNull();
        expect(call.querySelector('.audit-log-meta').textContent).toBe(
            '<img src=x onerror=alert(1)>'
        );
        expect(on.querySelector('.audit-log-summary').textContent).toBe(
            'Background AI turned on for Mind Map (Kiro)'
        );
        // An extension without a page here shows its id.
        expect(off.querySelector('.audit-log-summary').textContent).toBe(
            'Background AI turned off for gone-ext'
        );
        expect(rows()).toHaveLength(4);
    });

    it('the Background AI filter shows both kinds and nothing else', async () => {
        mount();
        await flush();
        const filter = document.getElementById('auditLogFilter');
        const option = [...filter.options].find((o) => o.value === 'background_ai');
        expect(option.textContent).toBe('Background AI');
        filter.value = 'background_ai';
        filter.dispatchEvent(new Event('change'));
        expect(rows()).toHaveLength(3);
        expect(rows().every((r) => r.querySelector('.audit-log-icon').textContent === '🌙')).toBe(
            true
        );
        filter.value = 'denied';
        filter.dispatchEvent(new Event('change'));
        expect(rows()).toHaveLength(1);
    });
});
