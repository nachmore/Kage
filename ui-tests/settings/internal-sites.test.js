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

    it('needs a sample URL before drafting', async () => {
        mount({});
        click('[data-is-action="add"]');
        click('[data-is-action="draft-rules"]');
        await flush();
        expect(tauri.invoke).not.toHaveBeenCalled();
        expect(document.querySelector('.internal-site-status.error')).not.toBeNull();
    });

    it('merges drafted rules and script into the profile', async () => {
        mount({});
        click('[data-is-action="add"]');
        type('[data-is-field="sample_url"]', 'https://wiki.example.test/page');
        click('[data-is-action="draft-rules"]');
        await flush();
        await flush();
        expect(tauri.invoke).toHaveBeenCalledWith('internal_sites_draft_rules', {
            url: 'https://wiki.example.test/page',
        });
        click('[data-is-action="draft-script"]');
        await flush();
        await flush();
        const [p] = mod._profiles;
        expect(p.name).toBe('Wiki');
        expect(p.url_rules[0].pattern).toBe('wiki.example.test');
        expect(p.content_rules[0].target).toBe('final_url');
        expect(p.proxy.script).toBe('Write-Output $env:KAGE_URL');
        expect(document.querySelector('[data-is-field="script"]').value).toBe(
            'Write-Output $env:KAGE_URL'
        );
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
