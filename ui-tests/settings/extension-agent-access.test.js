/**
 * The Background AI panel on an extension's Settings page and its consent
 * dialog (docs/design/EXTENSION_AGENT_CALLS.md §4.2, §4.3, §12 "host
 * panel"): it shows only with the grant and the manifest block; the dialog
 * names the host's agent and where it sends text; the extension's `reason`
 * is escaped and quoted; the consent token goes back to `set`; the
 * allowance select, Stop now and the log work; labels are escaped; "Show
 * what was sent" is offered only for this session's calls; and the
 * `agent_access` settings host effect opens the dialog for the page's own
 * extension only.
 */

import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import * as i18n from '../../ui/js/shared/i18n.js';
import { initEnglish } from '../helpers/sync-i18n.js';
import {
    agentOutcomeLabel,
    agentWhere,
    callRows,
    cleanText,
    consentModel,
    panelStatus,
    renderAgentAccessPanel,
    requestAgentAccess,
    resetAgentAccessPanels,
    resolveReason,
    showConsentDialog,
    showsAgentPanel,
} from '../../ui/js/settings/extension-agent-access.js';
import { renderCapabilityBadges } from '../../ui/js/settings/extension-capabilities.js';
import { runSettingsHostSideEffect } from '../../ui/js/shared/settings-host-effects.js';

const t = i18n.t;
const REASON_KEY = 'manifest.agent_background_reason';
const CATALOG = {
    [REASON_KEY]: { message: 'Mind Map sends <b>new notes</b> to organise them.' },
};

function panelState(over = {}) {
    return {
        extension_id: 'mindmap',
        name: 'Mind Map',
        available: true,
        granted: true,
        declared: true,
        extension_enabled: true,
        enabled: false,
        access: 'off',
        connected: true,
        agent: { label: 'Work agent (Claude Code)', location: { kind: 'cloud' } },
        consent_token: 'tok-1',
        consent_agent: null,
        allowance: 'normal',
        allowances: {
            low: { calls: 10, chars: 100000 },
            normal: { calls: 40, chars: 400000 },
            high: { calls: 150, chars: 1500000 },
        },
        usage: {
            calls_today: 3,
            calls_limit: 40,
            chars_today: 12345,
            chars_limit: 400000,
            resets_at: '2026-11-10T00:00:00+01:00',
        },
        pause: null,
        next_start_at: null,
        running: false,
        waiting: 0,
        reason: `__MSG_${REASON_KEY}__`,
        purposes: ['organize'],
        reads: [],
        viewable: [],
        log: [],
        ...over,
    };
}

function settingsModule(over = {}) {
    return {
        _extensionId: 'mindmap',
        title: 'Mind Map',
        icon: '🧠',
        _capabilities: ['storage', 'agent_background'],
        _manifest: {
            id: 'mindmap',
            name: 'Mind Map',
            permissions: ['storage', 'agent_background'],
            agentBackground: { reason: `__MSG_${REASON_KEY}__`, purposes: ['organize'] },
        },
        _sandbox: { i18nCatalog: CATALOG, i18nFallback: {} },
        ...over,
    };
}

const flush = async (n = 4) => {
    for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0));
};

/** A backend that answers the four host commands like Rust does. */
function backendFor(state, overrides = {}) {
    const backend = { state: structuredClone(state), calls: [] };
    backend.invoke = vi.fn(async (cmd, args) => {
        backend.calls.push([cmd, args]);
        if (overrides[cmd]) return overrides[cmd](args, backend);
        switch (cmd) {
            case 'get_extension_agent_access':
                return structuredClone(backend.state);
            case 'set_extension_agent_access': {
                const s = backend.state;
                if (args.enabled) {
                    s.enabled = true;
                    s.access = 'on';
                    s.consent_agent = s.agent.label;
                } else {
                    s.enabled = false;
                    s.access = 'off';
                    s.consent_agent = null;
                }
                if (args.allowance) s.allowance = args.allowance;
                return null;
            }
            case 'stop_extension_agent_calls':
                backend.state.running = false;
                backend.state.waiting = 0;
                backend.state.pause = { until: '2026-11-09T11:00:00+01:00', reason: 'stopped' };
                return null;
            case 'get_extension_agent_request_detail':
                return {
                    call_id: args.callId,
                    extension_id: 'mindmap',
                    purpose: 'organize',
                    at: '2026-11-09T10:00:00Z',
                    prompt: '[Kage background task]\n<script>alert(1)</script>',
                    reply: '{"items": []}',
                };
            case 'read_extension_locale':
                return null;
            default:
                throw new Error(`unexpected ${cmd}`);
        }
    });
    backend.listeners = {};
    backend.listen = vi.fn(async (event, fn) => {
        (backend.listeners[event] ||= []).push(fn);
        return () => {};
    });
    return backend;
}

async function mount(state, overrides, module = settingsModule()) {
    const backend = backendFor(state, overrides);
    document.body.innerHTML = renderAgentAccessPanel(module, {
        invoke: backend.invoke,
        listen: backend.listen,
    });
    await flush();
    return { backend, root: document.querySelector('[data-ext-agent="mindmap"]') };
}

const dialog = () => document.querySelector('.ext-agent-consent');
const callsOf = (backend, cmd) => backend.calls.filter(([c]) => c === cmd).map(([, a]) => a);
const statusKind = (root) => root.querySelector('.ext-agent-status-text')?.dataset.status;

beforeAll(async () => {
    await initEnglish(i18n);
});
beforeEach(() => {
    resetAgentAccessPanels();
    document.body.innerHTML = '';
    vi.spyOn(console, 'warn').mockImplementation(() => {});
});
afterEach(() => {
    vi.restoreAllMocks();
    resetAgentAccessPanels();
    document.body.innerHTML = '';
    delete window.__TAURI__;
});

describe('when the panel shows', () => {
    it('needs the grant and the manifest block', () => {
        expect(showsAgentPanel(settingsModule())).toBe(true);
        expect(showsAgentPanel(settingsModule({ _capabilities: new Set(['agent_background']) }))).toBe(
            true
        );
        expect(showsAgentPanel(settingsModule({ _capabilities: ['storage', 'agent'] }))).toBe(false);
        expect(showsAgentPanel(settingsModule({ _manifest: { id: 'mindmap' } }))).toBe(false);
        expect(
            showsAgentPanel(settingsModule({ _manifest: { id: 'mindmap', agentBackground: [] } }))
        ).toBe(false);
        expect(showsAgentPanel(settingsModule({ _extensionId: '' }))).toBe(false);
        expect(showsAgentPanel(null)).toBe(false);
    });

    it('renders nothing and asks nothing without them', () => {
        const invoke = vi.fn();
        expect(
            renderAgentAccessPanel(settingsModule({ _capabilities: ['storage'] }), { invoke })
        ).toBe('');
        expect(renderAgentAccessPanel(settingsModule({ _manifest: {} }), { invoke })).toBe('');
        expect(invoke).not.toHaveBeenCalled();
    });

    it('the capability badge has its icon and label', () => {
        const html = renderCapabilityBadges(['agent_background']);
        expect(html).toContain('🌙');
        expect(html).toContain(t('settings.manager.cap.agent_background.label'));
    });

    it('is host DOM that the dirty tracker skips', async () => {
        const { root } = await mount(panelState());
        expect(root.hasAttribute('data-sync-ui')).toBe(true);
        expect(root.getAttribute('role')).toBe('group');
    });
});

describe('the panel, switched off', () => {
    it('shows the switch, status, usage, allowance and an empty log', async () => {
        const { root, backend } = await mount(panelState());
        expect(callsOf(backend, 'get_extension_agent_access')).toEqual([{ extensionId: 'mindmap' }]);
        const sw = root.querySelector('[data-agent-access="switch"]');
        expect(sw.checked).toBe(false);
        expect(sw.getAttribute('role')).toBe('switch');
        expect(sw.disabled).toBe(false);
        expect(root.textContent).toContain('Let Mind Map use your agent in the background');
        expect(statusKind(root)).toBe('off');
        expect(root.querySelector('.ext-agent-usage').textContent).toMatch(
            /^3 of 40 requests today on this computer · 12,345 characters · Resets at /
        );
        const select = root.querySelector('[data-agent-access="allowance"]');
        expect(select.value).toBe('normal');
        expect([...select.options].map((o) => o.textContent)).toEqual([
            'Low (10 requests a day)',
            'Normal (40 requests a day)',
            'High (150 requests a day)',
        ]);
        expect(select.disabled).toBe(false);
        expect(root.querySelector('[data-agent-access="stop"]')).toBeNull();
        expect(root.querySelector('.ext-agent-log-empty').textContent).toBe(
            t('settings.manager.ext.agent.log.empty')
        );
    });
});

describe('turning it on', () => {
    it('asks in the host dialog, then passes the token to set', async () => {
        const { root, backend } = await mount(panelState());
        const sw = root.querySelector('[data-agent-access="switch"]');
        sw.checked = true;
        sw.dispatchEvent(new Event('change', { bubbles: true }));
        await flush();
        const d = dialog();
        expect(d).toBeTruthy();
        expect(d.getAttribute('role')).toBe('dialog');
        expect(d.getAttribute('aria-modal')).toBe('true');
        // The switch waits, checked and locked, while the dialog is open.
        const pending = root.querySelector('[data-agent-access="switch"]');
        expect(pending.checked).toBe(true);
        expect(pending.disabled).toBe(true);
        expect(callsOf(backend, 'set_extension_agent_access')).toEqual([]);

        d.querySelector('[data-action="allow"]').click();
        await flush();
        expect(callsOf(backend, 'set_extension_agent_access')).toEqual([
            { extensionId: 'mindmap', enabled: true, consentToken: 'tok-1' },
        ]);
        expect(dialog()).toBeNull();
        expect(root.querySelector('[data-agent-access="switch"]').checked).toBe(true);
        expect(statusKind(root)).toBe('on');
        expect(root.querySelector('.ext-agent-status-text').textContent).toBe(
            'On · sends to Work agent (Claude Code)'
        );
    });

    it('Not now sends nothing and leaves it off', async () => {
        const { root, backend } = await mount(panelState());
        const sw = root.querySelector('[data-agent-access="switch"]');
        sw.checked = true;
        sw.dispatchEvent(new Event('change', { bubbles: true }));
        await flush();
        // Not now has the focus, so Enter alone never allows.
        expect(document.activeElement?.dataset.action).toBe('cancel');
        dialog().querySelector('[data-action="cancel"]').click();
        await flush();
        expect(callsOf(backend, 'set_extension_agent_access')).toEqual([]);
        expect(root.querySelector('[data-agent-access="switch"]').checked).toBe(false);
        expect(statusKind(root)).toBe('off');
    });

    it('Escape and a click outside close it as Not now', async () => {
        const { backend } = await mount(panelState());
        let p = requestAgentAccess('mindmap');
        await flush();
        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        expect(await p).toEqual({ ok: false, cancelled: true });
        p = requestAgentAccess('mindmap');
        await flush();
        document.querySelector('.kage-confirm-backdrop').click();
        expect(await p).toEqual({ ok: false, cancelled: true });
        expect(callsOf(backend, 'set_extension_agent_access')).toEqual([]);
    });

    it('opens one dialog at a time', async () => {
        await mount(panelState());
        const first = requestAgentAccess('mindmap');
        await flush();
        expect(await requestAgentAccess('mindmap')).toEqual({ ok: false, busy: true });
        expect(document.querySelectorAll('.ext-agent-consent')).toHaveLength(1);
        dialog().querySelector('[data-action="cancel"]').click();
        await first;
    });

    it('an agent change while it was open is reported, not hidden', async () => {
        const { root } = await mount(panelState(), {
            set_extension_agent_access: async () => {
                throw {
                    kind: 'not_permitted',
                    key: 'errors.agent_call.agent_changed',
                    message: t('errors.agent_call.agent_changed'),
                };
            },
        });
        const sw = root.querySelector('[data-agent-access="switch"]');
        sw.checked = true;
        sw.dispatchEvent(new Event('change', { bubbles: true }));
        await flush();
        dialog().querySelector('[data-action="allow"]').click();
        await flush();
        const note = root.querySelector('.ext-agent-feedback');
        expect(note.textContent).toBe(t('errors.agent_call.agent_changed'));
        expect(note.getAttribute('role')).toBe('alert');
        expect(root.querySelector('[data-agent-access="switch"]').checked).toBe(false);
    });

    it("without a connected agent it says so and doesn't open the dialog", async () => {
        await mount(panelState({ agent: null, consent_token: null }));
        const result = await requestAgentAccess('mindmap');
        expect(result).toEqual({ ok: false, message: t('errors.agent_call.agent_unavailable') });
        expect(dialog()).toBeNull();
    });

    it('is refused when the extension may not use it', async () => {
        await mount(panelState({ available: false, declared: false }));
        const result = await requestAgentAccess('mindmap');
        expect(result.ok).toBe(false);
        expect(result.message).toBe(t('errors.agent_call.not_available', { extension: 'Mind Map' }));
        expect(dialog()).toBeNull();
    });
});

describe('the consent dialog', () => {
    it("uses the host's agent label, and isolates it left to right", async () => {
        const p = showConsentDialog(consentModel(panelState(), { reason: 'Why' }));
        const d = dialog();
        expect(d.querySelector('.kage-confirm-title').textContent).toBe(
            'Let Mind Map use Work agent (Claude Code) in the background?'
        );
        const agents = [...d.querySelectorAll('bdi[dir="ltr"]')].map((b) => b.textContent);
        expect(agents).toContain('Work agent (Claude Code)');
        expect(d.querySelector('.ext-agent-consent-where').textContent).toBe(
            'Work agent (Claude Code) sends what it receives to its online service.'
        );
        expect(d.textContent).toContain('up to 40 requests a day');
        for (const key of ['when', 'tools', 'history', 'log']) {
            expect(d.textContent).toContain(t(`settings.manager.ext.agent.consent.${key}`));
        }
        d.querySelector('[data-action="cancel"]').click();
        expect(await p).toBe(false);
    });

    it.each([
        [{ kind: 'local' }, 'Ollama runs on this computer.'],
        [{ kind: 'remote', host: 'gpu.example.net' }, 'Ollama runs on another computer (gpu.example.net).'],
        [{ kind: 'remote' }, "Kage can't tell where Ollama sends what it receives."],
        [{ kind: 'unknown' }, "Kage can't tell where Ollama sends what it receives."],
        [{ kind: 'mystery' }, "Kage can't tell where Ollama sends what it receives."],
    ])('says where the agent sends text: %o', async (location, text) => {
        const panel = panelState({ agent: { label: 'Ollama', location } });
        const p = showConsentDialog(consentModel(panel, {}));
        const where = dialog().querySelector('.ext-agent-consent-where');
        expect(where.textContent.startsWith(text)).toBe(true);
        if (location.host) {
            const ltr = [...where.querySelectorAll('bdi[dir="ltr"]')].map((b) => b.textContent);
            expect(ltr).toEqual(['Ollama', 'gpu.example.net']);
        }
        dialog().querySelector('[data-action="cancel"]').click();
        await p;
    });

    it("quotes the extension's reason as text, never as HTML", async () => {
        await mount(panelState());
        const p = requestAgentAccess('mindmap');
        await flush();
        const quote = dialog().querySelector('.ext-agent-consent-reason');
        expect(quote.textContent).toBe(
            'Mind Map says: "Mind Map sends <b>new notes</b> to organise them."'
        );
        expect(quote.querySelector('b')).toBeNull();
        expect(dialog().querySelector('b, img, script')).toBeNull();
        dialog().querySelector('[data-action="cancel"]').click();
        await p;
    });

    it('lists the data capabilities it can also read, only when it holds some', async () => {
        let p = showConsentDialog(consentModel(panelState({ reads: ['clipboard', 'calendar'] })));
        const reads = dialog().querySelector('[data-reads]');
        expect(reads.dataset.reads).toBe('clipboard calendar');
        expect(reads.textContent).toContain(t('settings.manager.cap.clipboard.label'));
        expect(reads.textContent).toContain(t('settings.manager.cap.calendar.label'));
        dialog().querySelector('[data-action="cancel"]').click();
        await p;
        p = showConsentDialog(consentModel(panelState({ reads: ['storage', 'agent'] })));
        expect(dialog().querySelector('[data-reads]')).toBeNull();
        dialog().querySelector('[data-action="cancel"]').click();
        await p;
    });

    it('a manifest name is text too', async () => {
        const p = showConsentDialog(
            consentModel(panelState({ name: '<img src=x onerror=alert(1)>' }), {})
        );
        expect(dialog().querySelector('img')).toBeNull();
        expect(dialog().querySelector('.kage-confirm-title').textContent).toContain(
            '<img src=x onerror=alert(1)>'
        );
        dialog().querySelector('[data-action="cancel"]').click();
        await p;
    });
});

describe('reason and text helpers', () => {
    it('resolves a __MSG_ token from the given catalog, else fetches it', async () => {
        expect(await resolveReason(`__MSG_${REASON_KEY}__`, { catalog: CATALOG })).toBe(
            CATALOG[REASON_KEY].message
        );
        const invoke = vi.fn(async (cmd, args) =>
            cmd === 'read_extension_locale' && args.language === 'en'
                ? { [REASON_KEY]: { message: 'Fetched reason' } }
                : null
        );
        expect(
            await resolveReason(`__MSG_${REASON_KEY}__`, { invoke, extensionId: 'mindmap' })
        ).toBe('Fetched reason');
        expect(await resolveReason('__MSG_missing__', {})).toBe('');
        expect(await resolveReason('Plain words', {})).toBe('Plain words');
        expect(await resolveReason(42, {})).toBe('');
    });

    it('drops control, bidi and private-use characters and caps the length', () => {
        const rlo = String.fromCharCode(0x202e);
        const slot = String.fromCharCode(0xe021);
        expect(cleanText(`a${rlo}b${slot}c\u0007`, 10)).toBe('abc');
        expect(Array.from(cleanText('x'.repeat(400), 300))).toHaveLength(300);
        expect(cleanText('x'.repeat(400), 300).endsWith('…')).toBe(true);
        expect(cleanText(null, 5)).toBe('');
    });

    it('maps locations', () => {
        expect(agentWhere({ location: { kind: 'cloud' } })).toEqual({ kind: 'cloud' });
        expect(agentWhere({ location: { kind: 'remote', host: 'h' } })).toEqual({
            kind: 'remote',
            host: 'h',
        });
        expect(agentWhere(null)).toEqual({ kind: 'unknown' });
    });

    it('names every outcome', () => {
        for (const o of [
            'ok',
            'invalid_response',
            'not_json',
            'response_too_large',
            'timeout',
            'cancelled',
            'tool_attempted',
            'agent_error',
            'connection_lost',
        ]) {
            expect(agentOutcomeLabel(o)).toBe(t(`settings.manager.ext.agent.outcome.${o}`));
        }
    });
});

describe('the status line', () => {
    it.each([
        [{ access: 'on', enabled: true, consent_agent: 'Kiro' }, 'on', 'On · sends to Kiro'],
        [{ access: 'needs_consent', enabled: true }, 'needs_consent', 'Needs your OK'],
        [
            { access: 'consent_stale', enabled: true, agent: { label: 'Ollama', location: {} } },
            'stale',
            'Paused: your agent changed to Ollama.',
        ],
        [
            {
                access: 'on',
                enabled: true,
                pause: { until: '2026-11-09T10:30:00Z', reason: 'failures' },
            },
            'paused_failures',
            /^Paused until .+ after 3 failed requests\.$/,
        ],
        [
            {
                access: 'on',
                enabled: true,
                pause: { until: '2026-11-10T00:00:00Z', reason: 'tool_attempt' },
            },
            'paused_tools',
            /your agent tried to use a tool twice today/,
        ],
        [
            {
                access: 'on',
                enabled: true,
                pause: { until: '2026-11-09T11:00:00Z', reason: 'stopped' },
            },
            'paused_stopped',
            /because you stopped it/,
        ],
        [{ available: false, declared: false }, 'unavailable', /manifest doesn't declare/],
    ])('%o → %s', async (over, kind, text) => {
        const { root } = await mount(panelState(over));
        expect(statusKind(root)).toBe(kind);
        const shown = root.querySelector('.ext-agent-status-text').textContent;
        if (typeof text === 'string') expect(shown).toBe(text);
        else expect(shown).toMatch(text);
        const again = root.querySelector('[data-agent-access="allow"]');
        expect(Boolean(again)).toBe(kind === 'stale' || kind === 'needs_consent');
    });

    it('a stale consent offers Allow again, which opens the dialog', async () => {
        const { root, backend } = await mount(
            panelState({ access: 'consent_stale', enabled: true, consent_token: 'tok-2' })
        );
        root.querySelector('[data-agent-access="allow"]').click();
        await flush();
        expect(dialog()).toBeTruthy();
        dialog().querySelector('[data-action="allow"]').click();
        await flush();
        expect(callsOf(backend, 'set_extension_agent_access')).toEqual([
            { extensionId: 'mindmap', enabled: true, consentToken: 'tok-2' },
        ]);
    });

    it("can't be switched on when the manifest block isn't valid", async () => {
        const { root } = await mount(panelState({ available: false, declared: false }));
        expect(root.querySelector('[data-agent-access="switch"]').disabled).toBe(true);
        expect(root.querySelector('[data-agent-access="allowance"]')).toBeNull();
    });

    it('panelStatus is pure', () => {
        expect(panelStatus(null)).toEqual({ kind: 'unavailable' });
        expect(panelStatus(panelState())).toEqual({ kind: 'off' });
    });
});

describe('turning it off, the allowance and Stop now', () => {
    it('turning off goes straight to set', async () => {
        const { root, backend } = await mount(
            panelState({ access: 'on', enabled: true, consent_agent: 'Kiro' })
        );
        const sw = root.querySelector('[data-agent-access="switch"]');
        sw.checked = false;
        sw.dispatchEvent(new Event('change', { bubbles: true }));
        await flush();
        expect(dialog()).toBeNull();
        expect(callsOf(backend, 'set_extension_agent_access')).toEqual([
            { extensionId: 'mindmap', enabled: false },
        ]);
        expect(statusKind(root)).toBe('off');
    });

    it('the allowance keeps the switch as it is', async () => {
        const on = await mount(panelState({ access: 'on', enabled: true }));
        const select = on.root.querySelector('[data-agent-access="allowance"]');
        select.value = 'high';
        select.dispatchEvent(new Event('change', { bubbles: true }));
        await flush();
        expect(callsOf(on.backend, 'set_extension_agent_access')).toEqual([
            { extensionId: 'mindmap', enabled: true, allowance: 'high' },
        ]);
        expect(on.root.querySelector('[data-agent-access="allowance"]').value).toBe('high');

        resetAgentAccessPanels();
        const off = await mount(panelState());
        const sel = off.root.querySelector('[data-agent-access="allowance"]');
        sel.value = 'low';
        sel.dispatchEvent(new Event('change', { bubbles: true }));
        await flush();
        expect(callsOf(off.backend, 'set_extension_agent_access')).toEqual([
            { extensionId: 'mindmap', enabled: false, allowance: 'low' },
        ]);
    });

    it('waits for Allow again while the consent is stale', async () => {
        const { root } = await mount(panelState({ access: 'consent_stale', enabled: true }));
        expect(root.querySelector('[data-agent-access="allowance"]').disabled).toBe(true);
    });

    it('Stop now shows while a call runs or waits, and stops them', async () => {
        const idle = await mount(panelState({ access: 'on', enabled: true }));
        expect(idle.root.querySelector('[data-agent-access="stop"]')).toBeNull();
        resetAgentAccessPanels();
        const { root, backend } = await mount(
            panelState({ access: 'on', enabled: true, running: true, waiting: 2 })
        );
        root.querySelector('[data-agent-access="stop"]').click();
        await flush();
        expect(callsOf(backend, 'stop_extension_agent_calls')).toEqual([{ extensionId: 'mindmap' }]);
        expect(root.querySelector('.ext-agent-feedback').textContent).toBe(
            t('settings.manager.ext.agent.stop.done')
        );
        expect(root.querySelector('[data-agent-access="stop"]')).toBeNull();
        expect(statusKind(root)).toBe('paused_stopped');
    });

    it('redraws when calls start or end, and when the config changes', async () => {
        const { root, backend } = await mount(panelState({ access: 'on', enabled: true }));
        expect(backend.listen).toHaveBeenCalledWith('agent_calls_activity', expect.any(Function));
        backend.state.running = true;
        for (const fn of backend.listeners.agent_calls_activity) fn({ payload: { running: [] } });
        await vi.waitFor(() =>
            expect(root.querySelector('[data-agent-access="stop"]')).not.toBeNull()
        );
    });
});

describe('recent requests', () => {
    const log = [
        {
            at: '2026-11-09T10:00:05.000Z',
            event: 'extension_agent_call',
            extension_id: 'mindmap',
            call_id: 'c3',
            purpose: 'organize',
            label: '<img src=x onerror=alert(1)>',
            agent: 'Kiro',
            outcome: 'ok',
            prompt_chars: 1200,
            response_chars: 300,
        },
        {
            at: '2026-11-09T10:00:04.000Z',
            event: 'extension_agent_access_changed',
            extension_id: 'mindmap',
            enabled: true,
            agent: 'Kiro',
        },
        {
            at: '2026-11-08T09:00:00.000Z',
            event: 'extension_agent_call',
            extension_id: 'mindmap',
            call_id: 'c1',
            purpose: 'organize',
            agent: 'Kiro',
            outcome: 'timeout',
            prompt_chars: 900,
        },
    ];

    it('lists calls only, with escaped labels', async () => {
        const { root } = await mount(panelState({ log, viewable: ['c3'] }));
        const rows = [...root.querySelectorAll('.ext-agent-log li')];
        expect(rows.map((r) => r.dataset.callId)).toEqual(['c3', 'c1']);
        expect(rows[0].querySelector('img')).toBeNull();
        expect(rows[0].querySelector('.ext-agent-log-label').textContent).toBe(
            '<img src=x onerror=alert(1)>'
        );
        expect(rows[0].querySelector('.ext-agent-log-text').textContent).toMatch(
            / · organize · Done · 1,200 characters · Kiro$/
        );
        expect(rows[0].querySelector('bdi[dir="ltr"]').textContent).toBe('Kiro');
        expect(rows[1].querySelector('.ext-agent-log-text').textContent).toContain('Timed out');
    });

    it('offers "Show what was sent" only for this session\'s calls', async () => {
        const { root, backend } = await mount(panelState({ log, viewable: ['c3'] }));
        const rows = [...root.querySelectorAll('.ext-agent-log li')];
        expect(rows[0].querySelector('.ext-agent-link')).toBeTruthy();
        expect(rows[1].querySelector('.ext-agent-link')).toBeNull();
        expect(root.querySelector('.ext-agent-note').textContent).toBe(
            t('settings.manager.ext.agent.log.sent_unavailable')
        );
        rows[0].querySelector('.ext-agent-link').click();
        await flush();
        expect(callsOf(backend, 'get_extension_agent_request_detail')).toEqual([
            { extensionId: 'mindmap', callId: 'c3' },
        ]);
        const view = document.querySelector('.ext-agent-detail-modal');
        expect(view.querySelector('.kage-confirm-title').textContent).toBe(
            t('settings.manager.ext.agent.log.sent_title')
        );
        expect(view.querySelector('[data-part="prompt"]').textContent).toBe(
            '[Kage background task]\n<script>alert(1)</script>'
        );
        expect(view.querySelector('script')).toBeNull();
        expect(view.querySelector('[data-part="reply"]').textContent).toBe('{"items": []}');
        view.querySelector('[data-action="close"]').click();
        expect(document.querySelector('.ext-agent-detail-modal')).toBeNull();
    });

    it('no note when every call can be shown', async () => {
        const { root } = await mount(panelState({ log: [log[0]], viewable: ['c3'] }));
        expect(root.querySelector('.ext-agent-note')).toBeNull();
    });

    it('callRows caps at 20 and ignores other events', () => {
        const many = Array.from({ length: 30 }, (_, i) => ({ ...log[0], call_id: `c${i}` }));
        expect(callRows({ log: many })).toHaveLength(20);
        expect(callRows({ log: [log[1]] })).toEqual([]);
        expect(callRows({})).toEqual([]);
    });
});

describe('the agent_access settings host effect', () => {
    function renderer(over = {}) {
        return {
            extensionId: 'mindmap',
            manifest: { id: 'mindmap', name: '__MSG_manifest.name__' },
            sandbox: {
                i18nCatalog: { ...CATALOG, 'manifest.name': { message: 'Mind Map' } },
                i18nFallback: {},
                capabilities: ['storage', 'agent_background'],
            },
            refresh: vi.fn(async () => {}),
            ...over,
        };
    }

    it("opens the dialog for the page's own extension only", async () => {
        const backend = backendFor(panelState());
        window.__TAURI__ = { core: { invoke: backend.invoke } };
        const r = renderer();
        const setStatus = vi.fn();
        // The effect names another extension; the host ignores it.
        const run = runSettingsHostSideEffect(
            r,
            { type: 'agent_access', extensionId: 'weather' },
            { setStatus, sourceAction: 'organize_allow', status: '' }
        );
        await flush();
        expect(callsOf(backend, 'get_extension_agent_access')).toEqual([{ extensionId: 'mindmap' }]);
        expect(dialog().querySelector('.ext-agent-consent-reason').textContent).toContain(
            'Mind Map sends <b>new notes</b>'
        );
        dialog().querySelector('[data-action="allow"]').click();
        await run;
        expect(callsOf(backend, 'set_extension_agent_access')).toEqual([
            { extensionId: 'mindmap', enabled: true, consentToken: 'tok-1' },
        ]);
        expect(backend.calls.every(([, a]) => a?.extensionId !== 'weather')).toBe(true);
        expect(r.refresh).toHaveBeenCalledWith({ preserveStatusFor: 'organize_allow', status: '' });
        expect(setStatus).not.toHaveBeenCalled();
    });

    it('says why on the action row when it may not', async () => {
        const backend = backendFor(panelState({ available: false }));
        window.__TAURI__ = { core: { invoke: backend.invoke } };
        const setStatus = vi.fn();
        await runSettingsHostSideEffect(renderer(), { type: 'agent_access' }, { setStatus });
        expect(dialog()).toBeNull();
        expect(setStatus).toHaveBeenCalledWith(
            `❌ ${t('errors.agent_call.not_available', { extension: 'Mind Map' })}`
        );
    });

    it('does nothing more when access is already on', async () => {
        const backend = backendFor(panelState({ access: 'on', enabled: true }));
        window.__TAURI__ = { core: { invoke: backend.invoke } };
        const r = renderer();
        const setStatus = vi.fn();
        await runSettingsHostSideEffect(r, { type: 'agent_access' }, { setStatus });
        expect(dialog()).toBeNull();
        expect(callsOf(backend, 'set_extension_agent_access')).toEqual([]);
        expect(r.refresh).not.toHaveBeenCalled();
    });

    it('Not now leaves the page as it was', async () => {
        const backend = backendFor(panelState());
        window.__TAURI__ = { core: { invoke: backend.invoke } };
        const r = renderer();
        const setStatus = vi.fn();
        const run = runSettingsHostSideEffect(r, { type: 'agent_access' }, { setStatus });
        await flush();
        dialog().querySelector('[data-action="cancel"]').click();
        await run;
        expect(callsOf(backend, 'set_extension_agent_access')).toEqual([]);
        expect(r.refresh).not.toHaveBeenCalled();
        expect(setStatus).not.toHaveBeenCalled();
    });
});

describe('placement on the extension page', () => {
    it('comes after the badges and the sync chip, before the Windows block', async () => {
        const invoke = vi.fn(async (cmd) =>
            cmd === 'get_extension_agent_access' ? panelState() : {}
        );
        window.__TAURI__ = {
            core: { invoke },
            webviewWindow: { getCurrentWebviewWindow: () => ({ label: 'settings', close() {} }) },
            event: { listen: vi.fn(async () => () => {}) },
        };
        const { SettingsManager, SandboxedExtensionSettingsModule } = await import(
            '../../ui/js/settings/manager.js'
        );
        const container = document.createElement('div');
        container.id = 'settingsModules';
        document.body.appendChild(container);
        const base = settingsModule({
            _capabilities: ['storage', 'agent_background', 'views'],
        });
        base._manifest.contributes = { views: [{ id: 'board', title: 'Board', module: './b.js' }] };
        const module = new SandboxedExtensionSettingsModule({
            extensionId: 'mindmap',
            manifest: base._manifest,
            sandbox: base._sandbox,
            rendered: { container: document.createElement('div') },
            capabilities: base._capabilities,
        });
        const mgr = new SettingsManager();
        mgr.modules = [module];
        mgr._initializeModule = () => true;
        mgr.render();
        const section = container.querySelector('[data-section-content="ext-mindmap"]');
        const badges = section.querySelector('.ext-capabilities');
        const chip = section.querySelector('[data-sync-ext-chip]');
        const panel = section.querySelector('[data-ext-agent="mindmap"]');
        const views = section.querySelector('.ext-views-block');
        const slot = section.querySelector('#ext-sandbox-slot-mindmap');
        expect(badges && chip && panel && views && slot).toBeTruthy();
        const follows = (a, b) =>
            Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
        expect(follows(badges, chip)).toBe(true);
        expect(follows(chip, panel)).toBe(true);
        expect(follows(panel, views)).toBe(true);
        expect(follows(views, slot)).toBe(true);
        expect(panel.closest('#ext-content-mindmap')).toBeTruthy();
        await vi.waitFor(() => expect(panel.querySelector('[data-agent-access="switch"]')).toBeTruthy());
    });
});
