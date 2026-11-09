/**
 * Settings → Sync (docs/design/SYNC.md §13) against mocked §19.5 commands:
 * every status line of §13.4, the setup steps, conflicts, reviews, the
 * computers, destructive actions with a confirm token, the sidebar item,
 * the extension chip and the Store's uninstall note.
 */

import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import { initEnglish, repoRoot } from '../helpers/sync-i18n.js';

let sync;
let t;

const NOW = Date.parse('2026-11-09T12:00:00Z');
const flush = () => new Promise((r) => setTimeout(r, 0));

const onStatus = (over = {}) => ({
    state: 'up_to_date',
    folder: { display: 'Dropbox › Kage Sync', provider: 'dropbox' },
    last_sync_at: NOW - 120_000,
    last_publish_at: NOW - 120_000,
    paused_until: null,
    pending_local: 0,
    waiting_files: 0,
    categories: { settings: true, automations: true, extensions: true, chats: false },
    devices: [
        { id: 'd1', name: 'MacBook Air', os: 'macos', kage_version: '1', last_seen_at: NOW, this: true, state: 'ok' },
        { id: 'd2', name: 'Desktop <PC>', os: 'windows', kage_version: '1', last_seen_at: NOW - 3 * 3_600_000, this: false, state: 'ok' },
        { id: 'd3', name: 'Old laptop', os: 'windows', kage_version: '1', last_seen_at: NOW - 200 * 86_400_000, this: false, state: 'inactive' },
    ],
    conflicts_open: 0,
    held: 0,
    reviews: 0,
    suggestions: 0,
    problems: [],
    key_store: 'keychain',
    undo_join_until: null,
    used_bytes: 0,
    ext_data_off: [],
    ...over,
});

beforeAll(async () => {
    vi.useFakeTimers({ toFake: ['Date'], now: NOW });
    window.matchMedia ??= () => ({
        matches: false,
        addEventListener() {},
        removeEventListener() {},
        addListener() {},
        removeListener() {},
    });
    setupTauriMock({});
    const i18n = await import('../../ui/js/shared/i18n.js');
    await initEnglish(i18n);
    t = i18n.t;
    sync = await import('../../ui/js/settings/sync.js');
});

describe('statusView: every state of §13.4', () => {
    const cases = [
        [{ state: 'up_to_date', last_sync_at: NOW - 120_000 }, 'Up to date · Last synced 2 minutes ago', []],
        [{ state: 'syncing' }, 'Syncing…', []],
        [{ state: 'waiting', waiting_files: 4 }, 'Waiting for 4 files to download', []],
        [{ state: 'placeholders' }, "Your sync folder hasn't downloaded some files yet", ['keep-downloaded']],
        [{ state: 'paused', paused_until: 'forever' }, 'Paused', ['resume']],
        [{ state: 'locked' }, 'Enter your sync passphrase to keep syncing', ['unlock-start']],
        [{ state: 'folder_missing' }, "Can't find your sync folder", ['find-folder', 'change-folder']],
        [{ state: 'folder_denied' }, "Kage isn't allowed to open your sync folder", ['change-folder']],
        [{ state: 'folder_empty' }, 'Your sync folder no longer has Kage data', ['start-over-start', 'leave']],
        [{ state: 'reset' }, 'Sync was started over on another computer', ['join-again']],
        [{ state: 'removed' }, 'This computer was removed from sync on another computer', ['join-again', 'ok-removed']],
        [{ state: 'update_required' }, 'Update Kage on this computer to keep syncing', ['open-updates']],
        [{ state: 'disk_full' }, 'Your disk is full. Sync continues when there is space.', []],
        [{ state: 'write_failed' }, "Kage can't write to your sync folder", []],
        [{ state: 'folder_reverting' }, "Your sync app keeps undoing Kage's files", []],
        [{ state: 'other_engine' }, 'Another copy of Kage is syncing this profile', []],
        [{ state: 'off' }, 'Off', []],
        [{ state: 'setting_up' }, 'Setting up…', []],
    ];
    for (const [status, text, actions] of cases) {
        it(`${status.state}`, () => {
            const v = sync.statusView(status, NOW);
            expect(v.text).toBe(text);
            expect(v.actions).toEqual(actions);
        });
    }

    it('paused until a time names the time', () => {
        const v = sync.statusView({ state: 'paused', paused_until: '2026-11-09T13:00:00Z' }, NOW);
        expect(v.text.startsWith('Paused until ')).toBe(true);
    });

    it('renders problems with bidi-isolated computer names and plural counts', () => {
        const v = sync.statusView(
            {
                state: 'up_to_date',
                problems: [
                    { kind: 'clock_warning', key: 'sync.problem.clock_warning', params: { computer: 'Home Mac', minutes: '42' } },
                    { kind: 'unreadable', key: 'sync.problem.unreadable', params: { computer: 'B', count: '1' } },
                    { kind: 'held_back', key: 'sync.problem.held_back', params: { count: '2' } },
                ],
            },
            NOW
        );
        expect(v.problems).toHaveLength(2);
        expect(v.problems[0]).toContain('⁨Home Mac⁩');
        expect(v.problems[0]).toContain('42 minutes');
        expect(v.problems[1]).toContain('1 file from');
    });
});

describe('the Sync sidebar item', () => {
    afterEach(() => teardownTauriMock());

    async function wire(status) {
        document.body.innerHTML = `<div class="sidebar-item hidden" data-section="sync"><span>Sync</span></div>
            <p data-sync-available-only hidden>pointer</p>`;
        const tauri = setupTauriMock({ sync_get_status: async () => status });
        sync.wireSyncSidebar({ invoke: tauri.invoke, listen: tauri.listen });
        await flush();
        await flush();
        return tauri;
    }

    it('stays hidden while sync is unavailable', async () => {
        await wire({ state: 'unavailable' });
        expect(document.querySelector('.sidebar-item').classList.contains('hidden')).toBe(true);
        expect(document.querySelector('[data-sync-available-only]').hidden).toBe(true);
    });

    it('shows while off, and counts what needs attention once on', async () => {
        await wire({ state: 'off' });
        expect(document.querySelector('.sidebar-item').classList.contains('hidden')).toBe(false);
        expect(document.querySelector('.sync-sidebar-badge')).toBeNull();
        expect(document.querySelector('[data-sync-available-only]').hidden).toBe(false);
        const tauri = await wire(onStatus({ conflicts_open: 2, reviews: 1 }));
        expect(document.querySelector('.sync-sidebar-badge').textContent).toBe('3');
        tauri.emit('sync_conflicts_changed', { open: 0 });
        expect(document.querySelector('.sync-sidebar-badge').textContent).toBe('1');
    });
});

/** A mocked backend for the page. */
function backend(over = {}) {
    const state = {
        status: onStatus(),
        reviews: { runnables: [], suggestions: [], removals: [], mass_changes: [], approvals: [] },
        conflicts: { replaced: [], deleted: [], resolved: [] },
        detail: {},
        ...over,
    };
    const handlers = {
        sync_get_status: async () => structuredClone(state.status),
        sync_list_reviews: async () => structuredClone(state.reviews),
        sync_suggest_folders: async () => [
            { provider: 'dropbox', label: 'Dropbox', path: '/home/u/Dropbox' },
        ],
        sync_inspect_folder: async ({ path }) =>
            path.includes('existing')
                ? { kind: 'existing', provider: 'dropbox', computer_count: 2 }
                : path.includes('tmp')
                  ? { kind: 'not_usable', provider: 'other', computer_count: 0, reason: 'settings.sync.folder.reason.temp' }
                  : path.includes('newer')
                    ? { kind: 'newer_format', provider: 'other', computer_count: 1 }
                    : { kind: 'new', provider: 'dropbox', computer_count: 0 },
        sync_generate_passphrase: async () => 'ABCD-EFGH-JKMN-PQRS',
        sync_create: async () => {
            state.status = onStatus();
            return onStatus();
        },
        sync_join: async ({ passphrase }) => {
            if (passphrase !== 'the right passphrase') {
                throw { kind: 'invalid_input', message: "That passphrase doesn't unlock this folder." };
            }
            return {
                computers: [{ name: 'Work <b>PC</b>', os: 'windows', last_seen_at: NOW }],
                same: { settings: 3, automations: 0, extensions: 0, chats: 0 },
                different: { settings: 9, automations: 0, extensions: 0, chats: 0 },
                only_here: { settings: 0, automations: 2, extensions: 0, chats: 0 },
                only_synced: { settings: 0, automations: 0, extensions: 0, chats: 0 },
            };
        },
        sync_confirm_join: async () => {
            state.status = onStatus({ conflicts_open: 9 });
            return onStatus({ conflicts_open: 9 });
        },
        sync_list_conflicts: async ({ tab }) => structuredClone(state.conflicts[tab]),
        sync_conflict_detail: async ({ id }) => structuredClone(state.detail[id]),
        sync_resolve_conflict: async () => null,
        sync_dismiss_all: async () => null,
        sync_approve_runnable: async () => null,
        sync_keep_off: async () => null,
        sync_answer_mass_change: async () => null,
        sync_prepare_destructive: async ({ action }) => `token-${action}`,
        sync_leave: async () => null,
        sync_remove_device: async () => null,
        sync_unlock: async () => onStatus(),
        sync_set_paused: async () => null,
        sync_now: async () => null,
        sync_set_category: async () => null,
        sync_rename_device: async () => null,
        sync_change_passphrase: async () => null,
        list_extensions: async () => [],
        get_config: async () => ({}),
    };
    return { state, handlers };
}

describe('Settings → Sync page', () => {
    let tauri;
    let be;
    let page;

    async function open(over = {}, opts = {}) {
        be = backend(over);
        tauri = setupTauriMock(be.handlers);
        page = new sync.SyncSettingsModule({
            invoke: tauri.invoke,
            listen: tauri.listen,
            pickFolder: async () => opts.pick ?? '/home/u/other',
        });
        document.body.innerHTML = `<div data-section-content="sync">${page.render()}</div>`;
        await page.initialize();
        return page;
    }

    const root = () => document.getElementById('syncPage');
    const click = async (act, match) => {
        const btns = [...root().querySelectorAll(`[data-sync-act="${act}"]`)];
        const btn = match ? btns.find(match) : btns[0];
        if (!btn) throw new Error(`no button ${act}`);
        btn.click();
        for (let i = 0; i < 6; i++) await flush();
    };

    afterEach(() => {
        page?.destroy();
        teardownTauriMock();
        document.body.innerHTML = '';
    });

    it('while off: the intro and Set up sync…', async () => {
        await open({ status: { state: 'off' } });
        const text = root().textContent;
        expect(text).toContain('Keep your settings, shortcuts and extensions the same');
        expect(text).toContain('Nothing goes to a Kage server');
        expect(root().querySelector('[data-sync-act="setup-start"]').textContent).toBe('Set up sync…');
    });

    it('while on: status, folder, this computer, categories, computers and Advanced', async () => {
        await open({ status: onStatus({ undo_join_until: '2026-12-02T10:00:00Z' }) });
        const text = root().textContent;
        expect(text).toContain('Up to date · Last synced 2 minutes ago');
        expect(text).toContain('Dropbox › Kage Sync');
        expect(text).toContain('MacBook Air');
        expect(root().querySelectorAll('[data-sync-category]')).toHaveLength(4);
        expect(root().querySelector('[data-sync-category="chats"]').checked).toBe(false);
        // User text is escaped and isolated; the folder reads LTR in any
        // language (RTL, §17).
        expect(root().innerHTML).toContain('<bdi>Desktop &lt;PC&gt;</bdi>');
        expect(root().querySelector('.sync-fact-value [dir="ltr"]').textContent).toBe(
            'Dropbox › Kage Sync'
        );
        expect(text).toContain('last synced');
        expect(text).toContain('inactive');
        expect(text).toContain('Where the key is kept: Keychain');
        expect(text).toContain('Undo joining (until');
        // No "Needs your attention" when nothing does.
        expect(text).not.toContain('Needs your attention');
    });

    it('each state shows its buttons; locked asks for the passphrase', async () => {
        await open({ status: onStatus({ state: 'locked' }) });
        await click('unlock-start');
        const input = root().querySelector('#syncUnlockPass');
        expect(input.getAttribute('dir')).toBe('ltr');
        input.value = 'my passphrase here';
        await click('unlock');
        expect(tauri.invoke).toHaveBeenCalledWith('sync_unlock', { passphrase: 'my passphrase here' });
    });

    it('Pause offers three lengths and Resume ends it', async () => {
        await open();
        await click('pause-menu');
        await click('pause', (b) => b.dataset.until === 'forever');
        expect(tauri.invoke).toHaveBeenCalledWith('sync_set_paused', { until: 'forever' });
        be.state.status = onStatus({ state: 'paused', paused_until: 'forever' });
        await page.refresh();
        await click('resume');
        expect(tauri.invoke).toHaveBeenCalledWith('sync_set_paused', { until: null });
    });

    it('a category switch calls sync_set_category', async () => {
        await open();
        const box = root().querySelector('[data-sync-category="chats"]');
        box.checked = true;
        box.dispatchEvent(new Event('change', { bubbles: true }));
        for (let i = 0; i < 4; i++) await flush();
        expect(tauri.invoke).toHaveBeenCalledWith('sync_set_category', { category: 'chats', on: true });
    });

    it('Needs your attention lists conflicts and reviews', async () => {
        await open({
            status: onStatus({ conflicts_open: 2 }),
            reviews: {
                runnables: [{ kind: 'runnable', id: 'g1', label_key: 'sync.conflicts.label.item', label_params: { name: 'deploy' }, detail: 'rm -rf <x>' }],
                suggestions: [],
                removals: [],
                mass_changes: [],
                approvals: [{ kind: 'consent', id: 'stocks', label_key: 'sync.review.consent', label_params: { extension: 'Stocks' } }],
            },
        });
        const text = root().textContent;
        expect(text).toContain('Needs your attention');
        expect(text).toContain('2 conflicts');
        expect(text).toContain('1 synced item can run programs on this computer');
        expect(text).toContain('Stocks');
    });

    describe('setup', () => {
        it('creates a sync folder: folder, passphrase rules, name, categories', async () => {
            await open({ status: { state: 'off' } });
            await click('setup-start');
            expect(root().textContent).toContain('Choose a folder');
            expect(root().textContent).toContain('Dropbox');
            // Nothing chosen yet: Continue is disabled.
            expect(root().querySelector('[data-sync-act="setup-next"]').disabled).toBe(true);
            await click('setup-pick');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_inspect_folder', { path: '/home/u/Dropbox' });
            expect(root().textContent).toContain('Kage will create a folder named “Kage Sync” here.');
            await click('setup-next');

            // 12-character rule, then the confirm must match.
            const pass = () => root().querySelector('#syncPassphrase');
            const conf = () => root().querySelector('#syncPassphraseConfirm');
            pass().value = 'short';
            conf().value = 'short';
            await click('setup-next');
            expect(root().querySelector('[role="alert"]').textContent).toBe(
                'Use a passphrase of at least 12 characters.'
            );
            pass().value = 'a long enough passphrase';
            conf().value = 'a long enough passphrasE';
            await click('setup-next');
            expect(root().querySelector('[role="alert"]').textContent).toBe(
                "The two passphrases aren't the same."
            );
            // Make one for me fills both fields and shows it, LTR, to copy.
            await click('setup-generate');
            expect(root().querySelector('.sync-generated-value').textContent).toBe('ABCD-EFGH-JKMN-PQRS');
            expect(root().querySelector('.sync-generated-value').getAttribute('dir')).toBe('ltr');
            expect(pass().value).toBe('ABCD-EFGH-JKMN-PQRS');
            await click('setup-next');

            // Name.
            expect(root().textContent).toContain('Name this computer');
            root().querySelector('#syncDeviceName').value = '';
            await click('setup-next');
            expect(root().querySelector('[role="alert"]').textContent).toBe('Give this computer a name.');
            root().querySelector('#syncDeviceName').value = 'Studio';
            await click('setup-next');

            // Categories, with chat history off by default.
            expect(root().querySelector('[data-sync-category="chats"]').checked).toBe(false);
            expect(root().querySelector('[data-sync-category="settings"]').checked).toBe(true);
            await click('setup-create');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_create', {
                path: '/home/u/Dropbox',
                passphrase: 'ABCD-EFGH-JKMN-PQRS',
                deviceName: 'Studio',
                categories: { settings: true, automations: true, extensions: true, chats: false },
            });
            expect(root().textContent).toContain('Sync is on');
            await click('setup-finish');
            expect(root().textContent).toContain('Up to date');
        });

        it('joins: one passphrase field, a wrong one goes back, then the join choice', async () => {
            await open({ status: { state: 'off' } }, { pick: '/home/u/existing' });
            await click('setup-start');
            await click('setup-browse');
            expect(root().textContent).toContain('already has Kage sync from 2 computers');
            await click('setup-next');
            expect(root().querySelector('#syncPassphraseConfirm')).toBeNull();
            root().querySelector('#syncPassphrase').value = 'wrong';
            await click('setup-next');
            await click('setup-next'); // name
            await click('setup-join');
            // Back at the passphrase, with the reason.
            expect(root().querySelector('#syncPassphrase')).not.toBeNull();
            expect(root().querySelector('[role="alert"]').textContent).toBe(
                "That passphrase doesn't unlock this folder."
            );
            root().querySelector('#syncPassphrase').value = 'the right passphrase';
            await click('setup-next');
            await click('setup-next');
            await click('setup-join');
            const html = root().innerHTML;
            expect(html).not.toContain('<b>PC</b>');
            expect(root().textContent).toContain('This folder has Kage sync from');
            expect(root().textContent).toContain('\u2068Work <b>PC</b>\u2069 (Windows)');
            expect(root().textContent).toContain('9 settings differ.');
            expect(root().textContent).toContain('Nothing is lost.');
            const mine = root().querySelector('[data-sync-choice][value="mine"]');
            mine.checked = true;
            await click('setup-confirm');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_confirm_join', { choice: 'mine' });
            expect(root().textContent).toContain('9 settings on this computer were replaced by synced ones');
        });

        it("explains a folder it can't use, and a newer format", async () => {
            await open({ status: { state: 'off' } }, { pick: '/tmp/x' });
            await click('setup-start');
            await click('setup-browse');
            expect(root().textContent).toContain('This is a temporary folder.');
            expect(root().querySelector('[data-sync-act="setup-next"]').disabled).toBe(true);
            page._deps.pickFolder = async () => '/home/u/newer';
            await click('setup-browse');
            expect(root().textContent).toContain('needs a newer Kage');
            expect(root().querySelector('[data-sync-act="open-updates"]')).not.toBeNull();
        });

        it('moves focus to each step title, for keyboard use', async () => {
            await open({ status: { state: 'off' } });
            await click('setup-start');
            expect(document.activeElement.classList.contains('sync-step-title')).toBe(true);
        });
    });

    describe('conflicts', () => {
        const row = (over) => ({
            id: 'c1',
            kind: 'concurrent_edit',
            category: 'settings',
            label_key: 'settings.appearance.theme.label',
            label_params: {},
            loser: { computer: 'Home Mac', at: NOW - 60_000, deleted: false, preview: 'Light' },
            winner: { computer: 'Work PC', at: NOW - 30_000 },
            state: 'open',
            group: null,
            ...over,
        });

        it('groups joins and generated documents; Restore and Dismiss call the commands', async () => {
            await open({
                status: onStatus({ conflicts_open: 4 }),
                conflicts: {
                    replaced: [
                        row({}),
                        row({ id: 'j1', kind: 'replaced_on_join', group: 'join-1' }),
                        row({ id: 'j2', kind: 'replaced_on_join', group: 'join-1' }),
                        row({ id: 'g1', group: 'generated', label_key: 'sync.label.steering.auto' }),
                    ],
                    deleted: [],
                    resolved: [],
                },
            });
            await click('open-conflicts');
            const text = root().textContent;
            expect(text).toContain('When two computers changed the same thing');
            expect(text).toContain('Today');
            expect(text).toContain('Replaced: Light');
            expect(text).toContain('2 settings differed');
            expect(text).toContain('Generated by Kage');
            // The generated group starts collapsed.
            expect(text).not.toContain('What Kage learned about you');
            await click('conflict-restore', (b) => b.dataset.id === 'c1');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_resolve_conflict', { id: 'c1', action: 'restore' });
            await click('conflict-dismiss', (b) => b.dataset.id === 'c1');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_resolve_conflict', { id: 'c1', action: 'dismiss' });
            await click('conflict-keep-all');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_dismiss_all', { group: 'join-1' });
            await click('conflict-dismiss-all');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_dismiss_all', { group: null });
        });

        it('Compare shows both versions as escaped text', async () => {
            await open({
                status: onStatus({ conflicts_open: 1 }),
                conflicts: {
                    replaced: [row({ label_key: 'sync.conflicts.label.item', label_params: { name: '<i>gh</i>' } })],
                    deleted: [],
                    resolved: [],
                },
                detail: {
                    c1: {
                        ...row({}),
                        fields: [{ field: 'url', now: '<img src=x onerror=alert(1)>', replaced: 'https://old' }],
                    },
                },
            });
            await click('open-conflicts');
            expect(root().innerHTML).not.toContain('<i>gh</i>');
            await click('conflict-compare');
            const cell = root().querySelector('.sync-compare td');
            expect(cell.textContent).toBe('<img src=x onerror=alert(1)>');
            expect(root().querySelector('.sync-compare img')).toBeNull();
        });

        it('a delete against an edit offers Delete anyway; Recently deleted offers Restore', async () => {
            await open({
                status: onStatus({ conflicts_open: 1 }),
                conflicts: {
                    replaced: [
                        row({
                            kind: 'delete_vs_edit',
                            label_key: 'sync.conflicts.label.item',
                            label_params: { name: 'jira' },
                            loser: { computer: 'Work PC', at: NOW, deleted: true, preview: '' },
                            winner: { computer: 'Home Mac', at: NOW },
                        }),
                    ],
                    deleted: [row({ id: 'd|list.shortcuts|s1', kind: 'edit_vs_delete', winner: null })],
                    resolved: [],
                },
            });
            await click('open-conflicts');
            expect(root().textContent).toContain('You deleted');
            await click('conflict-delete_anyway');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_resolve_conflict', { id: 'c1', action: 'delete_anyway' });
            await click('conflict-tab', (b) => b.dataset.tab === 'deleted');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_list_conflicts', { tab: 'deleted' });
            expect(root().textContent).toContain('Deleted on');
            await click('conflict-restore');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_resolve_conflict', {
                id: 'd|list.shortcuts|s1',
                action: 'restore',
            });
        });
    });

    describe('reviews', () => {
        it('shows what a held item runs, LTR, and Allow or Keep off', async () => {
            await open({
                reviews: {
                    runnables: [{ kind: 'runnable', id: 'guard1', label_key: 'sync.conflicts.label.item', label_params: { name: 'deploy' }, computer: 'Work PC', detail: 'deploy.sh <prod>' }],
                    suggestions: [],
                    removals: [],
                    mass_changes: [{ kind: 'mass_delete', id: 'list.shortcuts', label_key: 'sync.review.mass_delete', label_params: { computer: 'Work PC', count: '300', what: 'list.shortcuts' }, count: 300 }],
                    approvals: [],
                },
            });
            await click('open-reviews');
            const pre = root().querySelector('pre.sync-runnable');
            expect(pre.textContent).toBe('deploy.sh <prod>');
            expect(pre.getAttribute('dir')).toBe('ltr');
            expect(root().textContent).toContain('From ⁨Work PC⁩');
            expect(root().textContent).toContain('deleted 300 items from Commands & Prompts');
            await click('review-approve');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_approve_runnable', { key: 'guard1' });
            await click('review-mass-refuse');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_answer_mass_change', {
                ns: 'list.shortcuts',
                accept: false,
            });
        });
    });

    describe('computers and destructive actions', () => {
        it('removes another computer with a confirm token; lost needs a new passphrase', async () => {
            await open();
            await click('device-remove-start', (b) => b.dataset.id === 'd2');
            expect(root().textContent).toContain('Remove');
            const lost = root().querySelector('#syncRemoveLost');
            lost.checked = true;
            lost.dispatchEvent(new Event('change', { bubbles: true }));
            await click('device-remove-confirm');
            expect(root().querySelector('[role="alert"]').textContent).toContain('Enter the passphrase.');
            expect(tauri.invoke).not.toHaveBeenCalledWith('sync_remove_device', expect.anything());
            await click('device-remove-start', (b) => b.dataset.id === 'd2');
            const box = root().querySelector('#syncRemoveLost');
            box.checked = true;
            box.dispatchEvent(new Event('change', { bubbles: true }));
            root().querySelector('#syncRemovePass').value = 'a brand new passphrase';
            root().querySelector('#syncRemovePass2').value = 'a brand new passphrase';
            await click('device-remove-confirm');
            expect(tauri.invoke).toHaveBeenCalledWith('sync_prepare_destructive', { action: 'remove_device' });
            expect(tauri.invoke).toHaveBeenCalledWith('sync_remove_device', {
                deviceId: 'd2',
                lost: true,
                newPassphrase: 'a brand new passphrase',
                confirm: 'token-remove_device',
            });
        });

        it('Turn off sync asks first, then leaves with a token', async () => {
            await open();
            await click('leave');
            const dialog = document.querySelector('.kage-confirm-backdrop');
            expect(dialog.textContent).toContain('This computer keeps its settings as they are now.');
            dialog.querySelector('[data-action="confirm"]').click();
            for (let i = 0; i < 6; i++) await flush();
            expect(tauri.invoke).toHaveBeenCalledWith('sync_leave', { confirm: 'token-leave' });
        });

        it('Start over needs a new passphrase and the box ticked', async () => {
            be = null;
            await open();
            await click('start-over-start');
            root().querySelector('#syncNewPass').value = 'a brand new passphrase';
            root().querySelector('#syncNewPass2').value = 'a brand new passphrase';
            await click('start-over');
            expect(root().querySelector('[role="alert"]').textContent).toBe('Tick the box to confirm first.');
            expect(tauri.invoke).not.toHaveBeenCalledWith('sync_start_over', expect.anything());
        });
    });
});

describe('the extension chip', () => {
    it('says what syncs for an extension, only while sync is on', async () => {
        const { syncChipKinds } = await import('../../ui/js/settings/sync-ext-chip.js');
        const manifest = { id: 'mm', config: { token: { type: 'string', sensitive: true } } };
        expect(syncChipKinds({ status: { state: 'off' }, manifest, capabilities: ['sync'] })).toEqual([]);
        expect(
            syncChipKinds({ status: onStatus(), manifest, capabilities: ['storage', 'sync'] })
        ).toEqual(['settings_and_data', 'sign_in_local']);
        expect(syncChipKinds({ status: onStatus(), manifest: { id: 'x' }, capabilities: [] })).toEqual([
            'settings_only',
        ]);
        expect(
            syncChipKinds({
                status: onStatus({ categories: { extensions: false } }),
                manifest,
                capabilities: ['sync'],
            })
        ).toEqual([]);
    });

    it('fills its placeholder once the status is known', async () => {
        const mod = await import('../../ui/js/settings/sync-ext-chip.js');
        mod.resetSyncChipCache();
        const invoke = vi.fn(async () => onStatus());
        document.body.innerHTML = mod.renderSyncChip('mm', {
            invoke,
            t,
            manifest: { id: 'mm' },
            capabilities: ['sync'],
        });
        const el = document.querySelector('[data-sync-ext-chip="mm"]');
        expect(el.hidden).toBe(true);
        await flush();
        await flush();
        expect(el.hidden).toBe(false);
        expect(el.textContent).toBe('Syncs settings and data');
    });
});

describe("the Store's uninstall confirm", () => {
    afterEach(() => teardownTauriMock());

    it('adds the sync note only when the extension data syncs', async () => {
        let status = onStatus();
        setupTauriMock(
            new Proxy(
                {
                    sync_get_status: async () => status,
                    get_config: async () => ({
                        extension_grants: { mm: { granted: ['storage', 'sync'] } },
                    }),
                },
                { get: (h, k) => h[k] || (async () => []) }
            )
        );
        // The Store page boots on import; give it its own markup.
        const html = readFileSync(path.join(repoRoot, 'ui/store.html'), 'utf8');
        document.body.innerHTML = html.slice(html.indexOf('<body'), html.indexOf('</body>'));
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const store = await import('../../ui/js/store.js');
        for (let i = 0; i < 10; i++) await flush();
        expect(await store.uninstallMessage('mm')).toContain(
            'Its data stays in your sync folder and on your other computers.'
        );
        status = { state: 'unavailable' };
        expect(await store.uninstallMessage('mm')).not.toContain('sync folder');
    });
});
