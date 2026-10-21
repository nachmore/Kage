/**
 * Tests for ui/js/settings/internal-sites.js — the Internal Sites settings
 * module: list/editor rendering from state, delegated edits, the persisted
 * shape, and the agent-draft / test round trips.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import {
    InternalSitesSettingsModule,
    cleanProfile,
    normalizeSampleUrl,
} from '../../ui/js/settings/internal-sites.js';

let mod;
let tauri;

function mount(config = {}) {
    mod = new InternalSitesSettingsModule();
    document.body.innerHTML = `<div data-section-content="internal-sites">${mod.render()}</div>`;
    mod.initialize();
    mod.load(config);
}

function click(selector) {
    document.querySelector(selector).click();
}

function type(selector, value) {
    const el = document.querySelector(selector);
    el.value = value;
    el.dispatchEvent(new Event('input', { bubbles: true }));
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe('InternalSitesSettingsModule', () => {
    beforeEach(() => {
        tauri = setupTauriMock({
            internal_sites_draft_rules: async () => ({
                name: 'Wiki',
                url_rules: [{ kind: 'domain_suffix', pattern: 'wiki.example.test' }],
                content_rules: [{ target: 'final_url', kind: 'contains', pattern: 'login.' }],
            }),
            internal_sites_draft_script: async () => 'Write-Output $env:KAGE_URL',
            internal_sites_test: async () => ({
                url_match: true,
                content_match: null,
                probe: { ok: false, error: 'Fetch error: <dns>' },
                proxy: { ok: true, exit_code: 0, timed_out: false, stdout_bytes: 42, stderr_tail: '' },
                metadata: { title: 'Wiki <home>', description: 'Docs', favicon: 'data:image/png;base64,AA==' },
            }),
        });
        globalThis.confirm = () => true;
    });

    afterEach(() => {
        teardownTauriMock();
        document.body.innerHTML = '';
    });

    it('lists profiles from config and saves them back unchanged', () => {
        const profile = {
            id: 'a',
            name: 'Wiki',
            enabled: true,
            url_rules: [{ kind: 'regex', pattern: '^https://w\\.' }],
            content_rules: [],
            proxy: { lang: 'bash', script: 'cat', timeout_ms: 5000 },
            sample_url: 'https://w.example.test/',
        };
        mount({ internal_sites: [profile] });
        expect(document.querySelector('.internal-site-row-name').textContent).toBe('Wiki');
        const config = {};
        mod.save(config);
        expect(config.internal_sites).toEqual([profile]);
    });

    it('adds a profile, edits fields, and drops blank rule rows on save', () => {
        mount({});
        click('[data-is-action="add"]');
        type('[data-is-field="name"]', 'Docs');
        click('[data-is-action="add-url-rule"]');
        type('[data-is-field="url-pattern"]', 'docs.example.test');
        click('[data-is-action="add-content-rule"]'); // left blank
        type('[data-is-field="script"]', 'echo hi');
        type('[data-is-field="timeout"]', '7');

        const config = {};
        mod.save(config);
        const [p] = config.internal_sites;
        expect(p.name).toBe('Docs');
        expect(p.url_rules).toEqual([{ kind: 'domain_suffix', pattern: 'docs.example.test' }]);
        expect(p.content_rules).toEqual([]);
        expect(p.proxy.script).toBe('echo hi');
        expect(p.proxy.timeout_ms).toBe(7000);
        expect(mod.validate().valid).toBe(true);
    });

    it('requires a name on every profile', () => {
        mount({});
        click('[data-is-action="add"]');
        expect(mod.validate().valid).toBe(false);
    });

    it('needs a sample URL before drafting, and reports it beside the button', async () => {
        mount({});
        click('[data-is-action="add"]');
        click('[data-is-action="draft-rules"]');
        await flush();
        expect(tauri.invoke).not.toHaveBeenCalled();
        const status = document.querySelector('.internal-site-status.error');
        expect(status).not.toBeNull();
        // Rendered in the rules row, right under its button.
        expect(
            status.parentElement.querySelector('[data-is-action="draft-rules"]')
        ).not.toBeNull();
        expect(document.querySelector('.isd-overlay')).toBeNull();
    });

    it('assumes https:// when the sample URL has no scheme', async () => {
        mount({});
        click('[data-is-action="add"]');
        type('[data-is-field="sample_url"]', 'wiki.example.test/page');
        click('[data-is-action="test"]');
        await flush();
        await flush();
        const [, args] = tauri.invoke.mock.calls.find(([cmd]) => cmd === 'internal_sites_test');
        expect(args.url).toBe('https://wiki.example.test/page');
        expect(mod._profiles[0].sample_url).toBe('https://wiki.example.test/page');
    });

    it('drafts through the dialog, passing guidance, and merges the results', async () => {
        mount({});
        click('[data-is-action="add"]');
        type('[data-is-field="sample_url"]', 'https://wiki.example.test/page');
        click('[data-is-action="draft-rules"]');
        await flush();
        expect(document.querySelector('.isd-overlay')).not.toBeNull();
        expect(tauri.invoke).not.toHaveBeenCalled(); // waits for Start
        type('.isd-guidance', 'use the docs CLI');
        click('[data-isd-action="start"]');
        await flush();
        await flush();
        const [, args] = tauri.invoke.mock.calls.find(
            ([cmd]) => cmd === 'internal_sites_draft_rules'
        );
        expect(args.url).toBe('https://wiki.example.test/page');
        expect(args.guidance).toBe('use the docs CLI');
        expect(typeof args.requestId).toBe('string');
        expect(document.querySelector('.isd-overlay')).toBeNull();

        click('[data-is-action="draft-script"]');
        await flush();
        click('[data-isd-action="start"]'); // no guidance
        await flush();
        await flush();
        const [, sargs] = tauri.invoke.mock.calls.find(
            ([cmd]) => cmd === 'internal_sites_draft_script'
        );
        expect(sargs.guidance).toBeNull();
        const [p] = mod._profiles;
        expect(p.name).toBe('Wiki');
        expect(p.url_rules[0].pattern).toBe('wiki.example.test');
        expect(p.content_rules[0].target).toBe('final_url');
        expect(p.proxy.script).toBe('Write-Output $env:KAGE_URL');
        expect(document.querySelector('[data-is-field="script"]').value).toBe(
            'Write-Output $env:KAGE_URL'
        );
    });

    it('shows live agent activity and answers its permission prompt', async () => {
        let finish;
        tauri = setupTauriMock({
            internal_sites_draft_script: () =>
                new Promise((resolve) => {
                    finish = resolve;
                }),
            send_permission_response: async () => null,
        });
        mount({});
        click('[data-is-action="add"]');
        type('[data-is-field="sample_url"]', 'https://wiki.example.test/');
        click('[data-is-action="draft-script"]');
        await flush();
        click('[data-isd-action="start"]');
        await flush();
        await flush();
        const [, args] = tauri.invoke.mock.calls.find(
            ([cmd]) => cmd === 'internal_sites_draft_script'
        );
        tauri.emit('internal_sites_draft_session', { requestId: args.requestId, sessionId: 's1' });
        tauri.emit('internal_sites_draft_session', { requestId: 'other', sessionId: 's2' });
        tauri.emit('tool_call_update', {
            params: { sessionId: 's1', update: { toolCallId: 't1', title: 'Read config', status: 'completed' } },
        });
        tauri.emit('message_chunk', { sessionId: 's1', text: 'Looking at <cookies>' });
        tauri.emit('message_chunk', { sessionId: 's2', text: 'not mine' });
        tauri.emit('permission_request', {
            toolName: 'shell',
            notification: {
                id: 7,
                params: {
                    sessionId: 's1',
                    toolCall: { title: 'shell' },
                    options: [
                        { optionId: 'allow_once', name: 'Allow once', kind: 'allow_once' },
                        { optionId: 'reject_once', name: 'Deny', kind: 'reject_once' },
                    ],
                },
            },
        });
        const live = document.querySelector('.isd-live');
        expect(live.textContent).toContain('Read config');
        expect(live.querySelector('.isd-text').textContent).toBe('Looking at <cookies>');
        expect(live.textContent).not.toContain('not mine');
        click('[data-isd-action="permission"][data-option="allow_once"]');
        expect(tauri.invoke).toHaveBeenCalledWith('send_permission_response', {
            sessionId: 's1',
            requestId: 7,
            optionId: 'allow_once',
            toolTitle: 'shell',
        });
        expect(document.querySelector('.isd-permission')).toBeNull();

        finish('echo ok');
        await flush();
        await flush();
        expect(mod._profiles[0].proxy.script).toBe('echo ok');
    });

    it('cancels a running draft and discards its late result', async () => {
        let finish;
        tauri = setupTauriMock({
            internal_sites_draft_script: () =>
                new Promise((resolve) => {
                    finish = resolve;
                }),
            cancel_generation: async () => null,
            dismiss_pending_permission: async () => null,
        });
        mount({});
        click('[data-is-action="add"]');
        type('[data-is-field="sample_url"]', 'https://wiki.example.test/');
        click('[data-is-action="draft-script"]');
        await flush();
        click('[data-isd-action="start"]');
        await flush();
        await flush();
        const [, args] = tauri.invoke.mock.calls.find(
            ([cmd]) => cmd === 'internal_sites_draft_script'
        );
        tauri.emit('internal_sites_draft_session', { requestId: args.requestId, sessionId: 's9' });
        click('[data-isd-action="cancel"]');
        await flush();
        expect(tauri.invoke).toHaveBeenCalledWith('cancel_generation', { sessionId: 's9' });
        expect(tauri.invoke).toHaveBeenCalledWith('dismiss_pending_permission', { sessionId: 's9' });
        expect(document.querySelector('.isd-overlay')).toBeNull();
        finish('late script');
        await flush();
        expect(mod._profiles[0].proxy.script).toBe('');
        expect(document.querySelector('.internal-site-status')).not.toBeNull();
    });

    it('keeps a draft failure visible in the dialog until closed', async () => {
        tauri = setupTauriMock({
            internal_sites_draft_rules: async () => {
                throw { kind: 'internal', message: 'agent offline' };
            },
        });
        mount({});
        click('[data-is-action="add"]');
        type('[data-is-field="sample_url"]', 'https://wiki.example.test/');
        click('[data-is-action="draft-rules"]');
        await flush();
        click('[data-isd-action="start"]');
        await flush();
        await flush();
        expect(document.querySelector('.isd-overlay').textContent).toContain('agent offline');
        click('[data-isd-action="close"]');
        await flush();
        expect(document.querySelector('.isd-overlay')).toBeNull();
        expect(document.querySelector('.internal-site-status.error').textContent).toContain(
            'agent offline'
        );
    });

    it('normalizeSampleUrl assumes https and rejects non-web URLs', () => {
        expect(normalizeSampleUrl('wiki.example.test/a?b=1')).toBe('https://wiki.example.test/a?b=1');
        expect(normalizeSampleUrl(' http://x.example.test ')).toBe('http://x.example.test/');
        expect(normalizeSampleUrl('ftp://x.example.test')).toBeNull();
        expect(normalizeSampleUrl('')).toBeNull();
    });

    it('renders the test result with escaped metadata', async () => {
        mount({});
        click('[data-is-action="add"]');
        type('[data-is-field="sample_url"]', 'https://wiki.example.test/page');
        click('[data-is-action="add-url-rule"]'); // blank row isn't sent
        click('[data-is-action="test"]');
        await flush();
        await flush();
        const [, args] = tauri.invoke.mock.calls.find(([cmd]) => cmd === 'internal_sites_test');
        expect(args.profile.url_rules).toEqual([]);
        const preview = document.querySelector('.internal-site-preview');
        expect(preview.textContent).toContain('Wiki <home>');
        expect(preview.querySelector('img').getAttribute('src')).toBe('data:image/png;base64,AA==');
        expect(document.querySelector('.internal-site-test').innerHTML).not.toContain('<home>');
    });

    it('lists each favicon attempt and why it failed', async () => {
        tauri = setupTauriMock({
            internal_sites_test: async () => ({
                url_match: true,
                content_match: null,
                probe: { ok: true, status: 302, final_url: 'https://login.example.test/' },
                proxy: { ok: true, exit_code: 0, timed_out: false, stdout_bytes: 10, stderr_tail: '' },
                metadata: { title: 'Wiki', description: '', favicon: null },
                favicon_attempts: [
                    { url: 'https://wiki.example.test/a.svg', ok: false, exit_code: 0, timed_out: false, rejection: 'not_base64' },
                    { url: 'https://wiki.example.test/favicon.ico', ok: false, exit_code: 3, timed_out: false, rejection: null },
                ],
            }),
        });
        mount({});
        click('[data-is-action="add"]');
        type('[data-is-field="sample_url"]', 'wiki.example.test');
        click('[data-is-action="test"]');
        await flush();
        await flush();
        const items = [...document.querySelectorAll('.internal-site-icon-attempts li')];
        expect(items).toHaveLength(2);
        expect(items[0].textContent).toContain('a.svg');
        expect(items[0].textContent).toContain('settings.internal_sites.test.icon.not_base64');
        expect(items[1].classList.contains('failed')).toBe(true);
    });

    it('deletes a profile after confirmation', () => {
        mount({ internal_sites: [{ id: 'x', name: 'X' }] });
        click('[data-is-action="delete"]');
        const config = {};
        mod.save(config);
        expect(config.internal_sites).toEqual([]);
    });

    it('cleanProfile keeps everything but blank rules', () => {
        const p = {
            id: 'a',
            url_rules: [{ pattern: ' ' }, { pattern: 'a.test' }],
            content_rules: [{ pattern: '' }],
        };
        expect(cleanProfile(p)).toEqual({
            id: 'a',
            url_rules: [{ pattern: 'a.test' }],
            content_rules: [],
        });
    });
});
