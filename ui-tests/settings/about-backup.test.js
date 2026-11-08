/**
 * Settings → About → Backup: the two kinds of export, and import.
 *
 * The contract under test: export is either everything, encrypted with a
 * passphrase typed twice (the default), or an unencrypted settings-only
 * file whose left-out list is on screen before anything is saved. There is
 * no third, plain full backup.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { initI18n } from '../../ui/js/shared/i18n.js';
import {
    installAboutBackupMethods,
    MODE_ENCRYPTED,
    MODE_SETTINGS_ONLY,
    renderBackupSection,
} from '../../ui/js/settings/about-backup.js';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const en = JSON.parse(
    readFileSync(path.resolve(here, '../../locales/en/messages.json'), 'utf8')
);

class FakeAbout {
    _formatError(e) {
        return e?.message || String(e);
    }
}
installAboutBackupMethods(FakeAbout);

const PREVIEW = {
    left_out: [
        { kind: 'steering', count: 2 },
        { kind: 'shortcuts', count: 3 },
        { kind: 'agents', count: 1 },
        // A kind a newer backend sends is shown by name, as text.
        { kind: '<img src=x onerror="window.__pwned = true">', count: 1 },
    ],
    // Labels come from the backend: must render as text.
    flagged: ['Setting “ui › theme”', '<img src=x onerror="window.__pwned = true">'],
    settings: 42,
};

const msg = (key) => en[key].message;

const $ = (id) => document.getElementById(id);
/** Text without the FSI/PDI bidi isolates the UI puts around paths and names. */
const plain = (text) => text.replace(/[\u2068\u2069]/g, '');
const status = () => plain($('backupStatus').textContent);
const flush = () => new Promise((r) => setTimeout(r, 0));

/** Flush until `fn` returns something truthy, and return it. */
async function waitFor(fn, what = 'condition') {
    for (let i = 0; i < 50; i++) {
        const value = fn();
        if (value) return value;
        await flush();
    }
    throw new Error(`timed out waiting for ${what}`);
}

function mount(handlers = {}) {
    document.body.innerHTML = renderBackupSection();
    const { invoke } = setupTauriMock({
        export_config_default_filename: ({ encrypted }) =>
            encrypted ? 'kage-backup.kage.enc' : 'kage-settings.kage',
        export_config_bundle: () => 2048,
        preview_config_export: () => PREVIEW,
        ...handlers,
    });
    const save = vi.fn(async () => '/tmp/backup.kage');
    window.__TAURI__.dialog = { save, open: vi.fn(), ask: vi.fn(async () => true) };
    const module = new FakeAbout();
    module._wireBackupSection();
    return { module, invoke, save };
}

const calls = (invoke, cmd) => invoke.mock.calls.filter(([c]) => c === cmd);

async function clickExport() {
    $('backupExportBtn').click();
    for (let i = 0; i < 4; i++) await flush();
}

function choose(id) {
    const radio = $(id);
    radio.checked = true;
    radio.dispatchEvent(new Event('change', { bubbles: true }));
}

function typePassphrase(a, b = a) {
    $('backupPassphrase').value = a;
    $('backupPassphraseConfirm').value = b;
}

beforeAll(async () => {
    await initI18n(async () => ({ catalog: en, fallback: en, language: 'en' }));
});

beforeEach(() => {
    delete window.__pwned;
});

afterEach(() => {
    teardownTauriMock();
    document.body.innerHTML = '';
});

describe('backup export: the two kinds', () => {
    it('offers exactly two, with the encrypted one chosen and its passphrase fields shown', () => {
        mount();
        const radios = [...document.querySelectorAll('input[name="backupMode"]')];
        expect(radios.map((r) => r.value)).toEqual([MODE_ENCRYPTED, MODE_SETTINGS_ONLY]);
        expect($('backupModeEncrypted').checked).toBe(true);
        expect($('backupPassphraseRow').hidden).toBe(false);
        expect($('backupLeftOut').hidden).toBe(true);
        expect($('backupMode').querySelector('legend').textContent).toBe(
            msg('settings.about.backup.mode.title')
        );
        expect(document.querySelector('label[for="backupModeEncrypted"]').textContent).toBe(
            msg('settings.about.backup.mode.encrypted.label')
        );
        expect(document.querySelector('label[for="backupModeSettings"]').textContent).toBe(
            msg('settings.about.backup.mode.settings_only.label')
        );
        // The settings-only option always says what it never carries.
        expect($('backupLeftOutGeneric').textContent).toBe(
            msg('settings.about.backup.settings_only.left_out_generic')
        );
    });

    it('an encrypted export needs a matching passphrase and sends mode encrypted', async () => {
        const { invoke, save } = mount();
        await clickExport();
        expect(status()).toBe(msg('settings.about.backup.passphrase_required'));
        typePassphrase('one', 'two');
        await clickExport();
        expect(status()).toBe(msg('settings.about.backup.passphrase_mismatch'));
        expect(save).not.toHaveBeenCalled();
        typePassphrase('correct horse');
        await clickExport();
        expect(calls(invoke, 'export_config_bundle')).toEqual([
            [
                'export_config_bundle',
                { path: '/tmp/backup.kage', mode: MODE_ENCRYPTED, passphrase: 'correct horse' },
            ],
        ]);
        expect(status()).toBe('✓ Saved 2.0 KB to /tmp/backup.kage');
        expect($('backupPassphrase').value).toBe('');
        expect(calls(invoke, 'preview_config_export')).toHaveLength(0);
    });

    it('refuses a passphrase of only spaces without invoking anything', async () => {
        const { invoke, save } = mount();
        typePassphrase('   ');
        await clickExport();
        expect(status()).toBe(msg('settings.about.backup.passphrase_blank'));
        expect(save).not.toHaveBeenCalled();
        expect(calls(invoke, 'export_config_bundle')).toHaveLength(0);
    });

    it('sends a passphrase with surrounding spaces unchanged', async () => {
        const { invoke } = mount();
        typePassphrase('  pw  ');
        await clickExport();
        expect(calls(invoke, 'export_config_bundle')[0][1].passphrase).toBe('  pw  ');
    });

    it('shows a backend refusal as an export failure', async () => {
        const { invoke } = mount({
            export_config_bundle: () => {
                throw {
                    kind: 'internal',
                    key: 'errors.config.export_passphrase_blank',
                    message: msg('errors.config.export_passphrase_blank'),
                };
            },
        });
        typePassphrase('pw');
        await clickExport();
        expect(status()).toBe(
            `Export failed: ${msg('errors.config.export_passphrase_blank')}`
        );
        expect(calls(invoke, 'export_config_bundle')).toHaveLength(1);
    });

    it('settings only: lists what it leaves out, as text, before saving, and sends no passphrase', async () => {
        const { invoke, save } = mount();
        typePassphrase('typed then abandoned');
        choose('backupModeSettings');
        expect($('backupPassphraseRow').hidden).toBe(true);
        expect($('backupPassphrase').value).toBe('', 'a half-typed passphrase is wiped');
        await waitFor(() => !$('backupLeftOut').hidden, 'the left-out list');
        const items = [...$('backupLeftOutList').querySelectorAll('li')].map((li) => li.textContent);
        expect(items).toEqual([
            'Your steering docs',
            '3 shortcuts',
            '1 agent setup',
            PREVIEW.left_out[3].kind,
        ]);
        expect(items.join(' ')).not.toContain('{');
        expect(window.__pwned).toBeUndefined();
        expect($('backupLeftOutList').querySelector('img')).toBeNull();
        expect($('backupFlaggedList').querySelector('img')).toBeNull();
        expect($('backupFlaggedIntro').hidden).toBe(false);
        expect([...$('backupFlaggedList').querySelectorAll('li')].map((li) => li.textContent)).toEqual(
            PREVIEW.flagged
        );
        expect($('backupLeftOutOutro').textContent).toBe(
            msg('settings.about.backup.settings_only.left_out_outro')
        );
        await clickExport();
        expect(save.mock.calls[0][0].filters[0].extensions).toEqual(['kage']);
        expect(save.mock.calls[0][0].defaultPath).toBe('kage-settings.kage');
        expect(calls(invoke, 'export_config_bundle')).toEqual([
            [
                'export_config_bundle',
                { path: '/tmp/backup.kage', mode: MODE_SETTINGS_ONLY, passphrase: null },
            ],
        ]);
        expect(status()).toContain('Settings only');
    });

    it('settings only: the list is on screen before the save dialog opens', async () => {
        let answer;
        const { save } = mount({
            preview_config_export: () =>
                new Promise((resolve) => {
                    answer = resolve;
                }),
        });
        choose('backupModeSettings');
        $('backupExportBtn').click();
        await flush();
        expect(save).not.toHaveBeenCalled();
        answer(PREVIEW);
        await waitFor(() => save.mock.calls.length, 'the save dialog');
        expect($('backupLeftOut').hidden).toBe(false);
    });

    it('settings only: a failed list still exports, saying it could not list them', async () => {
        const { invoke } = mount({
            preview_config_export: () => {
                throw { kind: 'internal', message: 'disk on fire' };
            },
        });
        choose('backupModeSettings');
        await waitFor(() => !$('backupLeftOut').hidden, 'the failure note');
        expect(plain($('backupLeftOutList').textContent)).toBe(
            "Couldn't list what this machine has: disk on fire"
        );
        await clickExport();
        expect(calls(invoke, 'export_config_bundle')[0][1].mode).toBe(MODE_SETTINGS_ONLY);
    });

    it('switching back to encrypted hides the list and shows the passphrase fields', async () => {
        mount();
        choose('backupModeSettings');
        await waitFor(() => !$('backupLeftOut').hidden, 'the left-out list');
        choose('backupModeEncrypted');
        expect($('backupLeftOut').hidden).toBe(true);
        expect($('backupPassphraseRow').hidden).toBe(false);
    });

    it('the encrypted save filter offers only .enc', async () => {
        const { save } = mount();
        typePassphrase('pw');
        await clickExport();
        expect(save.mock.calls[0][0].filters[0].extensions).toEqual(['enc']);
        expect(save.mock.calls[0][0].defaultPath).toBe('kage-backup.kage.enc');
    });

    it('falls back to a .kage.enc name when the default name is unavailable', async () => {
        const { save } = mount({
            export_config_default_filename: () => {
                throw new Error('no');
            },
        });
        typePassphrase('pw');
        await clickExport();
        expect(save.mock.calls[0][0].defaultPath).toBe('kage-backup.kage.enc');
    });

    it('a double click runs one export', async () => {
        let pick;
        const { invoke, save } = mount();
        save.mockImplementation(
            () =>
                new Promise((resolve) => {
                    pick = resolve;
                })
        );
        typePassphrase('pw');
        $('backupExportBtn').click();
        $('backupExportBtn').click();
        await flush();
        expect(save).toHaveBeenCalledTimes(1);
        pick('/tmp/x.kage.enc');
        await waitFor(() => calls(invoke, 'export_config_bundle').length, 'the export');
        expect(calls(invoke, 'export_config_bundle')).toHaveLength(1);
    });
});

describe('backup import', () => {
    const SUMMARY = { shortcuts: 2, extensions: 1, steering_bytes: 0, exported_at: '' };
    const required = () => {
        throw { kind: 'invalid_input', key: 'errors.config.import_passphrase_required', message: 'encrypted' };
    };
    const wrong = () => {
        throw { kind: 'invalid_input', key: 'errors.config.import_wrong_passphrase', message: 'wrong' };
    };

    /** Mount with `file` as the open dialog's answer and `handler` for the import. */
    function mountImport(file, handler) {
        const ctx = mount({ import_config_bundle: handler, restart_app: () => null });
        window.__TAURI__.dialog.open.mockResolvedValue(file);
        return { ...ctx, ask: window.__TAURI__.dialog.ask };
    }

    const imports = (invoke) => calls(invoke, 'import_config_bundle').map(([, args]) => args);
    const overlay = () => document.querySelector('.backup-passphrase-overlay');

    /** Wait for the passphrase prompt; answer it (null = Cancel). Returns its body text. */
    async function answerPrompt(value) {
        const box = await waitFor(overlay, 'the passphrase prompt');
        const body = box.querySelector('#backupUnlockDesc').textContent;
        if (value === null) {
            box.querySelector('.backup-passphrase-cancel').click();
        } else {
            box.querySelector('input').value = value;
            box.querySelector('.backup-passphrase-ok').click();
        }
        return body;
    }

    const imported = () => waitFor(() => status().startsWith('✓ Restored'), 'the import summary');

    it('asks for the passphrase when a .kage file turns out to be encrypted', async () => {
        const { invoke, ask } = mountImport('/tmp/backup.kage', ({ passphrase }) =>
            passphrase === null ? required() : SUMMARY
        );
        $('backupImportBtn').click();
        expect(await answerPrompt('pw')).toBe(msg('settings.about.backup.unlock.description'));
        await imported();
        expect(imports(invoke)).toEqual([
            { path: '/tmp/backup.kage', passphrase: null },
            { path: '/tmp/backup.kage', passphrase: 'pw' },
        ]);
        expect(ask.mock.calls[0][0]).toBe(msg('settings.about.dialog.import.message'));
    });

    it('a plain .kage file imports without a prompt', async () => {
        const { invoke } = mountImport('/tmp/backup.kage', () => SUMMARY);
        $('backupImportBtn').click();
        await imported();
        expect(overlay()).toBeNull();
        expect(imports(invoke)).toEqual([{ path: '/tmp/backup.kage', passphrase: null }]);
    });

    it('a .enc file asks for the passphrase after the overwrite check, before importing', async () => {
        const { invoke, ask } = mountImport('/tmp/backup.kage.enc', () => SUMMARY);
        $('backupImportBtn').click();
        await waitFor(overlay, 'the passphrase prompt');
        expect(ask).toHaveBeenCalledTimes(1);
        expect(imports(invoke)).toEqual([]);
        await answerPrompt('pw');
        await imported();
        expect(imports(invoke)).toEqual([{ path: '/tmp/backup.kage.enc', passphrase: 'pw' }]);
    });

    it('declining the overwrite check stops before any prompt or import', async () => {
        const { invoke, ask } = mountImport('/tmp/backup.kage.enc', () => SUMMARY);
        ask.mockResolvedValue(false);
        $('backupImportBtn').click();
        await flush();
        await flush();
        expect(overlay()).toBeNull();
        expect(imports(invoke)).toEqual([]);
    });

    it('a wrong passphrase asks again, saying it did not work', async () => {
        const { invoke } = mountImport('/tmp/backup.kage.enc', ({ passphrase }) =>
            passphrase === 'right' ? SUMMARY : wrong()
        );
        $('backupImportBtn').click();
        expect(await answerPrompt('nope')).toBe(msg('settings.about.backup.unlock.description'));
        expect(await answerPrompt('right')).toBe(msg('settings.about.backup.unlock.wrong'));
        await imported();
        expect(imports(invoke).map((a) => a.passphrase)).toEqual(['nope', 'right']);
    });

    it('Cancel at the re-prompt stops quietly without another import', async () => {
        const { invoke } = mountImport('/tmp/backup.kage', ({ passphrase }) =>
            passphrase === null ? required() : wrong()
        );
        $('backupImportBtn').click();
        await answerPrompt('nope');
        expect(await answerPrompt(null)).toBe(msg('settings.about.backup.unlock.wrong'));
        await waitFor(() => !$('backupImportBtn').hasAttribute('aria-busy'), 'the import to end');
        expect(imports(invoke).map((a) => a.passphrase)).toEqual([null, 'nope']);
        expect(overlay()).toBeNull();
        expect(status()).toBe('');
    });

    it('Cancel at the first prompt of a .enc file imports nothing', async () => {
        const { invoke } = mountImport('/tmp/backup.enc', () => SUMMARY);
        $('backupImportBtn').click();
        await answerPrompt(null);
        await waitFor(() => !$('backupImportBtn').hasAttribute('aria-busy'), 'the import to end');
        expect(imports(invoke)).toEqual([]);
    });

    it('other errors are reported without a prompt', async () => {
        const { invoke } = mountImport('/tmp/backup.kage', () => {
            throw { kind: 'internal', key: 'errors.config.import_build_failed', message: 'bad zip' };
        });
        $('backupImportBtn').click();
        await waitFor(() => status().startsWith('Import failed'), 'the failure');
        expect(status()).toBe('Import failed: bad zip');
        expect(overlay()).toBeNull();
        expect(imports(invoke)).toHaveLength(1);
    });

    it('a double click opens one file dialog', async () => {
        mountImport(null, () => SUMMARY);
        let pick;
        const open = vi.fn(
            () =>
                new Promise((resolve) => {
                    pick = resolve;
                })
        );
        window.__TAURI__.dialog.open = open;
        const btn = $('backupImportBtn');
        btn.click();
        btn.click();
        await flush();
        expect(open).toHaveBeenCalledTimes(1);
        expect(btn.getAttribute('aria-busy')).toBe('true');
        pick(null); // the user closes the dialog
        await waitFor(() => !btn.hasAttribute('aria-busy'), 'the import to end');
        btn.click();
        await flush();
        expect(open).toHaveBeenCalledTimes(2);
    });
});

describe('backup section keyboard access', () => {
    it('the header is a button that toggles with Enter and Space', () => {
        mount();
        const header = $('backupToggle');
        expect(header.getAttribute('role')).toBe('button');
        expect(header.getAttribute('tabindex')).toBe('0');
        expect(header.getAttribute('aria-expanded')).toBe('false');
        header.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
        expect(header.getAttribute('aria-expanded')).toBe('true');
        expect($('backupBody').style.display).toBe('');
        header.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }));
        expect(header.getAttribute('aria-expanded')).toBe('false');
        expect($('backupBody').style.display).toBe('none');
    });

    it('every export choice and passphrase field has an accessible name', () => {
        mount();
        for (const id of ['backupModeEncrypted', 'backupModeSettings']) {
            expect(document.querySelector(`label[for="${id}"]`)).not.toBeNull();
        }
        expect($('backupPassphrase').getAttribute('aria-label')).toBe(
            msg('settings.about.backup.passphrase.placeholder')
        );
    });
});

describe('import summary', () => {
    const base = {
        shortcuts: 2,
        extensions: 3,
        steering_bytes: 0,
        kage_version: '0.9.0',
        exported_at: '',
    };

    it('names the extensions that need signing in again and what to re-enter', () => {
        const module = new FakeAbout();
        const text = module._importSummaryText({
            ...base,
            left_out: 3,
            sign_in_again: ['Spotify', 'GitHub'],
            reenter: ['Agent connection “Claude”: a key or password in its command'],
        });
        expect(plain(text)).toContain('Sign in again to Spotify and GitHub.');
        expect(plain(text)).toContain(
            'Re-enter: Agent connection “Claude”: a key or password in its command.'
        );
        // Names are bidi-isolated for RTL sentences.
        expect(text).toContain('\u2068Spotify\u2069');
    });

    it('says local values were kept when nothing is missing', () => {
        const module = new FakeAbout();
        const text = module._importSummaryText({ ...base, left_out: 2, sign_in_again: [], reenter: [] });
        expect(text).toContain("Kept this machine's values for 2 sign-ins and keys the backup left out.");
        expect(text).not.toContain('Sign in again');
    });

    it('a settings-only import says everything else was left as it was', () => {
        const module = new FakeAbout();
        const text = module._importSummaryText({ ...base, kind: 'settings_only', settings: 12 });
        expect(text).toBe(
            '✓ Restored 12 settings. Everything else on this machine was left as it was.'
        );
    });

    it('names the extensions that are off until approved, for either kind', () => {
        const module = new FakeAbout();
        for (const summary of [
            { ...base, needs_approval: ['Fetcher', 'Todos'] },
            { ...base, kind: 'settings_only', settings: 3, needs_approval: ['Fetcher', 'Todos'] },
        ]) {
            expect(plain(module._importSummaryText(summary))).toContain(
                '2 extensions need your approval again: Fetcher and Todos. They stay off until you approve them in the Store.'
            );
        }
        const one = module._importSummaryText({ ...base, needs_approval: ['Fetcher'] });
        expect(plain(one)).toContain('1 extension needs your approval again: Fetcher.');
        expect(one).toContain('\u2068Fetcher\u2069');
    });

    it('names the extensions that kept this machine’s settings whole', () => {
        const module = new FakeAbout();
        const text = module._importSummaryText({ ...base, kept_local: ['Todos'] });
        expect(plain(text)).toContain(
            "Kept this machine's settings and stored data for Todos: its sign-in here stays where it is"
        );
        expect(text).not.toContain('Sign in again');
    });

    it('old summaries without the new fields still render', () => {
        const module = new FakeAbout();
        const text = module._importSummaryText(base);
        expect(text).toContain('Restored 2 shortcuts');
        expect(text).not.toContain('Sign in again');
        expect(text).not.toContain('approval');
    });

    it('the passphrase prompt still closes on Escape after a click on the backdrop', async () => {
        mount();
        const module = new FakeAbout();
        const pending = module._promptForPassphrase();
        await flush();
        const overlay = document.querySelector('.backup-passphrase-overlay');
        overlay.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
        expect(document.activeElement).toBe(overlay.querySelector('input'));
        document.body.focus();
        document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        await expect(pending).resolves.toBeNull();
        expect(document.querySelector('.backup-passphrase-overlay')).toBeNull();
    });

    it('the passphrase prompt is a labelled modal dialog that traps focus', async () => {
        mount();
        const importBtn = $('backupImportBtn');
        importBtn.focus();
        const module = new FakeAbout();
        const pending = module._promptForPassphrase();
        await flush();
        const dialog = document.querySelector('[role="dialog"]');
        expect(dialog.getAttribute('aria-modal')).toBe('true');
        expect($(dialog.getAttribute('aria-labelledby')).textContent).toBe('Enter passphrase');
        const input = dialog.querySelector('input');
        expect(document.activeElement).toBe(input);
        const ok = dialog.querySelector('.backup-passphrase-ok');
        ok.focus();
        dialog.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }));
        expect(document.activeElement).toBe(input);
        input.value = 'secret';
        input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
        await expect(pending).resolves.toBe('secret');
        expect(document.querySelector('[role="dialog"]')).toBeNull();
        expect(document.activeElement).toBe(importBtn);
    });
});
