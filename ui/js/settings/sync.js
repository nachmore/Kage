import { confirmDialog } from '../shared/confirm-dialog.js';
import { errMessage } from '../shared/error-message.js';
import { EVT } from '../shared/events.js';
import { sensitiveConfigFields } from '../shared/extension-sensitive-data.js';
import { t } from '../shared/i18n.js';
import { syncIsOn } from '../shared/sync-scope-chip.js';
import { SettingsModule } from './base.js';
import { SyncConflicts } from './sync-conflicts.js';
import { devicesHtml, removeDevice } from './sync-devices.js';
import {
    dateText,
    escapeAttr,
    escapeHtml,
    icuParams,
    isolate,
    labelHtml,
    ltrText,
    relativeTime,
    shortTime,
    userText,
} from './sync-format.js';
import { SyncReviews } from './sync-review.js';
import { CATEGORIES, passphraseError, providerTip, SyncSetup } from './sync-setup.js';

/**
 * Settings → Sync (docs/design/SYNC.md §13): set up sync, see how it's
 * doing, choose what syncs here, review conflicts and synced items that
 * need approval, and manage the computers.
 *
 * Every control calls its own `sync_*` command at once, so the module
 * `persistsImmediately` and save() is a no-op. The sidebar item stays
 * hidden while `sync_get_status` reports `unavailable` (`wireSyncSidebar`).
 * The page says "computer", never "device".
 */

/** States shown with a warning dot. */
const WARN = new Set([
    'waiting',
    'placeholders',
    'paused',
    'locked',
    'folder_missing',
    'folder_denied',
    'folder_empty',
    'reset',
    'removed',
    'update_required',
    'disk_full',
    'write_failed',
    'folder_reverting',
    'other_engine',
]);

/** The buttons of each state (§13.4). */
const STATE_ACTIONS = {
    placeholders: ['keep-downloaded'],
    paused: ['resume'],
    locked: ['unlock-start'],
    folder_missing: ['find-folder', 'change-folder'],
    folder_denied: ['change-folder'],
    folder_empty: ['start-over-start', 'leave'],
    reset: ['join-again'],
    removed: ['join-again', 'ok-removed'],
    update_required: ['open-updates'],
};

const ACTION_KEYS = {
    'keep-downloaded': 'settings.sync.action.keep_downloaded',
    resume: 'settings.sync.action.resume',
    'unlock-start': 'settings.sync.action.unlock',
    'find-folder': 'settings.sync.action.find_folder',
    'change-folder': 'settings.sync.action.choose_folder',
    'start-over-start': 'settings.sync.action.start_over_here',
    leave: 'settings.sync.action.turn_off',
    'join-again': 'settings.sync.action.join_again',
    'ok-removed': 'settings.sync.action.ok',
    'open-updates': 'settings.sync.check_updates',
};

function problemText(p) {
    const params = {};
    for (const [k, v] of Object.entries(icuParams(p?.params))) {
        params[k] = typeof v === 'string' ? isolate(v) : v;
    }
    return t(p.key, params);
}

/**
 * The status line of a `SyncStatus` (§13.4): its text, a tone for the
 * dot, the buttons it offers and the problem lines under it. Pure.
 *
 * @returns {{ state: string, text: string, tone: string, actions: string[],
 *   problems: string[] }}
 */
export function statusView(status, now = Date.now()) {
    const s = status || {};
    const state = typeof s.state === 'string' ? s.state : 'unavailable';
    let text;
    switch (state) {
        case 'up_to_date': {
            const at = s.last_sync_at ?? s.last_publish_at;
            text = t('sync.status.up_to_date', {
                time: Number.isFinite(at) ? relativeTime(at, now) : relativeTime(now, now),
            });
            break;
        }
        case 'waiting':
            text = t('sync.status.waiting_any', { count: Number(s.waiting_files) || 0 });
            break;
        case 'paused':
            text =
                !s.paused_until || s.paused_until === 'forever'
                    ? t('sync.status.paused_forever')
                    : t('sync.status.paused', {
                          time: shortTime(Date.parse(s.paused_until), now),
                      });
            break;
        case 'reset':
            text = t('sync.status.reset_elsewhere');
            break;
        case 'removed':
            text = t('sync.status.removed_elsewhere');
            break;
        case 'unavailable':
            text = t('settings.sync.unavailable');
            break;
        default:
            text = t(`sync.status.${state}`);
    }
    const tone =
        state === 'up_to_date'
            ? 'ok'
            : state === 'syncing'
              ? 'busy'
              : WARN.has(state)
                ? 'warn'
                : 'idle';
    const problems = (Array.isArray(s.problems) ? s.problems : [])
        .filter((p) => p && typeof p.key === 'string')
        .filter((p) => p.kind !== 'held_back' && p.kind !== 'not_this_version')
        .map(problemText);
    return { state, text, tone, actions: STATE_ACTIONS[state] || [], problems };
}

/** The count on the Sync sidebar item: open conflicts and reviews. */
export function sidebarBadgeCount(status) {
    if (!syncIsOn(status)) return 0;
    return (Number(status.conflicts_open) || 0) + (Number(status.reviews) || 0);
}

/**
 * Show the Sync sidebar item unless sync is unavailable, with a badge for
 * what needs attention, and keep both current (§13.8).
 *
 * @param {{ invoke: Function, listen: Function, onStatus?: Function }} deps
 */
export function wireSyncSidebar({ invoke, listen, onStatus } = {}) {
    if (typeof document === 'undefined') return { reapply: () => {} };
    let last = null;
    const apply = (status) => {
        last = status;
        onStatus?.(status);
        const available = !!status && status.state !== 'unavailable';
        // Pointers to Settings → Sync elsewhere (About → Backup & restore).
        for (const el of document.querySelectorAll('[data-sync-available-only]')) {
            el.hidden = !available;
        }
        const item = document.querySelector('.sidebar-item[data-section="sync"]');
        if (!item) return;
        item.classList.toggle('hidden', !available);
        const n = sidebarBadgeCount(status);
        let badge = item.querySelector('.sync-sidebar-badge');
        if (n > 0 && !badge) {
            badge = document.createElement('span');
            badge.className = 'sync-sidebar-badge';
            item.appendChild(badge);
        } else if (n === 0 && badge) {
            badge.remove();
            badge = null;
        }
        if (badge) {
            badge.textContent = String(n);
            badge.setAttribute('aria-label', t('settings.sync.attention_badge', { count: n }));
        }
    };
    Promise.resolve()
        .then(() => invoke('sync_get_status'))
        .then(apply, () => apply(null));
    if (typeof listen === 'function') {
        Promise.resolve(listen(EVT.SYNC_STATUS_CHANGED, (e) => apply(e?.payload ?? null))).catch(
            () => {}
        );
        Promise.resolve(
            listen(EVT.SYNC_CONFLICTS_CHANGED, (e) => {
                const open = Number(e?.payload?.open);
                if (last && Number.isFinite(open)) apply({ ...last, conflicts_open: open });
            })
        ).catch(() => {});
    }
    // Run again after the sections are redrawn.
    return { reapply: () => apply(last) };
}

/** What syncs for an installed extension (Extensions → Choose…, §13.3). */
export function extensionSyncInfo(item, config, status) {
    const manifest = item?.manifest || {};
    const id = manifest.id || '';
    const grant = config?.extension_grants?.[id] || null;
    const granted = Array.isArray(grant?.granted) ? grant.granted : [];
    const wantsData = !!manifest.sync && (manifest.permissions || []).includes('sync');
    const dataGranted = wantsData && granted.includes('sync');
    const off = Array.isArray(status?.ext_data_off) ? status.ext_data_off : [];
    let sensitive = [];
    try {
        sensitive = sensitiveConfigFields(manifest);
    } catch {
        sensitive = [];
    }
    return {
        id,
        name: manifest.name || id,
        icon: manifest.icon || '📦',
        data: dataGranted,
        dataOn: dataGranted && !off.includes(id),
        signIn: sensitive.length > 0,
        sideload: grant?.origin === 'sideload',
        description:
            typeof manifest.sync?.description === 'string' ? manifest.sync.description : '',
    };
}

function button(act, key, { primary = false, danger = false, data = {}, vars } = {}) {
    const attrs = Object.entries(data)
        .map(([k, v]) => ` data-${k}="${escapeAttr(v)}"`)
        .join('');
    const cls = `setting-button${primary ? ' primary' : ''}${danger ? ' danger' : ''}`;
    return `<button type="button" class="${cls}" data-sync-act="${act}"${attrs}>${escapeHtml(t(key, vars))}</button>`;
}

function pauseUntil(choice, now = Date.now()) {
    if (choice === 'forever') return 'forever';
    if (choice === '1h') return new Date(now + 3_600_000).toISOString();
    const d = new Date(now);
    d.setDate(d.getDate() + 1);
    d.setHours(6, 0, 0, 0);
    return d.toISOString();
}

export class SyncSettingsModule extends SettingsModule {
    /**
     * @param {{ invoke?: Function, listen?: Function,
     *   pickFolder?: () => Promise<string | null> }} [deps] - tests pass
     *   their own; the window's Tauri globals otherwise
     */
    constructor(deps = {}) {
        super('sync', t('settings.sync.title'), '🔄');
        // Every control calls its own sync_* command; save() is a no-op.
        this.persistsImmediately = true;
        this._deps = deps;
        this.status = null;
        this.reviews = null;
        this.config = null;
        this.extensions = [];
        this.view = 'main';
        this.form = null;
        this.flash = '';
        this.error = '';
        this.showTip = false;
        this.pauseOpen = false;
        this.removing = null;
        this.held = null;
        this.setup = null;
        this.conflicts = null;
        this.reviewView = null;
        this._timer = null;
        this._unlisten = [];
    }

    get invoke() {
        return this._deps.invoke || window.__TAURI__?.core?.invoke;
    }

    get listen() {
        return this._deps.listen || window.__TAURI__?.event?.listen;
    }

    render() {
        return `
            <div class="settings-section sync-page" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${escapeHtml(this.title)}</h2>
                <div id="syncPage" class="sync-body" aria-live="polite"></div>
            </div>
        `;
    }

    get root() {
        return document.getElementById('syncPage');
    }

    async initialize() {
        const root = this.root;
        if (root && !root.dataset.syncBound) {
            root.dataset.syncBound = '1';
            root.addEventListener('click', (e) => {
                const el = e.target.closest?.('[data-sync-act]');
                if (!el || !root.contains(el) || el.matches('input')) return;
                e.preventDefault();
                this.act(el.dataset.syncAct, el);
            });
            root.addEventListener('change', (e) => {
                const el = e.target;
                if (el.matches?.('[data-sync-category]') && this.view === 'main') {
                    this.act('category', el);
                } else if (el.matches?.('[data-sync-ext-data]')) {
                    this.act('ext-data', el);
                } else if (el.id === 'syncRemoveLost') {
                    const box = root.querySelector('.sync-lost-fields');
                    if (box) box.hidden = !el.checked;
                }
            });
        }
        const listen = this.listen;
        if (typeof listen === 'function' && this._unlisten.length === 0) {
            const un = await Promise.resolve(
                listen(EVT.SYNC_STATUS_CHANGED, (e) => this._onStatus(e?.payload))
            ).catch(() => null);
            if (typeof un === 'function') this._unlisten.push(un);
        }
        if (!this._timer) {
            // While the page is visible (§14.1).
            this._timer = setInterval(() => {
                if (this._visible() && this.view === 'main' && !this.form) this.refresh();
            }, 5000);
        }
        await this.refresh();
    }

    load(config) {
        this.config = config;
        if (this.view === 'main' && !this.form) this.paint();
    }

    save(_config) {}

    validate() {
        return { valid: true };
    }

    destroy() {
        if (this._timer) clearInterval(this._timer);
        this._timer = null;
        for (const un of this._unlisten) {
            try {
                un();
            } catch {}
        }
        this._unlisten = [];
    }

    _visible() {
        return !!document.querySelector('[data-section-content="sync"]:not(.hidden)');
    }

    _onStatus(status) {
        if (!status || typeof status !== 'object') return;
        this.status = status;
        if (this.view === 'main' && !this.form) this.paint();
        else this._paintStatusLine();
    }

    /** Read the status (and, when on, the reviews and installed extensions). */
    async refresh() {
        const invoke = this.invoke;
        if (typeof invoke !== 'function') return;
        try {
            this.status = await invoke('sync_get_status');
        } catch (e) {
            this.status = { state: 'unavailable' };
            this.error = errMessage(e);
        }
        if (syncIsOn(this.status)) {
            try {
                this.reviews = await invoke('sync_list_reviews');
            } catch {
                this.reviews = null;
            }
        } else {
            this.reviews = null;
        }
        this.paint();
    }

    // --- Painting --------------------------------------------------------------

    paint() {
        const root = this.root;
        if (!root) return;
        const before = this._paintedView;
        const html = {
            main: () => this._mainHtml(),
            setup: () => this.setup?.render() ?? '',
            conflicts: () => this.conflicts?.render() ?? '',
            reviews: () => this.reviewView?.render() ?? '',
            extensions: () => this._extensionsHtml(),
            held: () => this._heldHtml(),
        }[this.view]();
        root.innerHTML = html;
        root.dataset.view = this.view;
        const step = this.view === 'setup' ? this.setup?.step : this.view;
        if (before !== undefined && before !== step) {
            root.querySelector('.sync-step-title, h3')?.focus?.();
        }
        this._paintedView = step;
    }

    _paintStatusLine() {
        const el = this.root?.querySelector('.sync-status-text');
        if (el) el.textContent = statusView(this.status).text;
    }

    _offHtml() {
        return `<div class="sync-off">
            <p>${escapeHtml(t('settings.sync.intro'))}</p>
            <p class="setting-description">${escapeHtml(t('settings.sync.no_server'))}</p>
            <div class="sync-actions">${button('setup-start', 'settings.sync.setup', { primary: true })}</div>
            <p class="setting-description">${escapeHtml(t('settings.sync.off.syncs'))}</p>
            <p class="setting-description">${escapeHtml(t('settings.sync.stays_local'))} ${escapeHtml(t('settings.sync.off.chats_optional'))}</p>
        </div>`;
    }

    _mainHtml() {
        const status = this.status;
        const state = status?.state || 'unavailable';
        const flash = this.flash
            ? `<div class="sync-flash" role="status">${escapeHtml(this.flash)}</div>`
            : '';
        const error = this.error
            ? `<div class="sync-error" role="alert">${escapeHtml(this.error)}</div>`
            : '';
        if (state === 'unavailable') {
            return `<p class="setting-description">${escapeHtml(t('settings.sync.unavailable'))}</p>`;
        }
        if (state === 'off') return this._offHtml() + flash + error;
        if (state === 'setting_up') {
            return `<div class="sync-status-line" role="status"><span class="sync-dot sync-dot--busy" aria-hidden="true"></span>
                <span class="sync-status-text">${escapeHtml(t('sync.status.setting_up'))}</span></div>
                <div class="sync-actions">${button('setup-start', 'settings.sync.setup.again')}</div>${flash}${error}`;
        }
        return [
            this._headHtml(),
            flash,
            error,
            this._folderHtml(),
            this._categoriesHtml(),
            this._attentionHtml(),
            this._devicesHtml(),
            this._notSyncedHtml(),
            this._staysHtml(),
            this._advancedHtml(),
        ].join('');
    }

    _headHtml() {
        const v = statusView(this.status);
        const paused = v.state === 'paused';
        const pauseBtn = paused
            ? button('resume', 'settings.sync.action.resume')
            : `<div class="sync-pause">
                <button type="button" class="setting-button" data-sync-act="pause-menu" aria-haspopup="true" aria-expanded="${this.pauseOpen}">${escapeHtml(t('settings.sync.pause'))} ▾</button>
                ${
                    this.pauseOpen
                        ? `<div class="sync-pause-menu" role="menu">
                    ${['1h', 'tomorrow', 'forever']
                        .map(
                            (c) =>
                                `<button type="button" role="menuitem" class="sync-pause-item" data-sync-act="pause" data-until="${c}">${escapeHtml(t(`settings.sync.pause.${c}`))}</button>`
                        )
                        .join('')}
                </div>`
                        : ''
                }
            </div>`;
        const actions = v.actions
            .filter((a) => !(paused && a === 'resume'))
            .map((a) =>
                button(a, ACTION_KEYS[a], { primary: a === 'unlock-start' || a === 'join-again' })
            )
            .join('');
        const tip =
            this.showTip && v.state === 'placeholders'
                ? `<p class="setting-description">${escapeHtml(providerTip(this.status?.folder?.provider) || t('settings.sync.tip.generic'))}</p>`
                : '';
        const problems = v.problems
            .map((p) => `<div class="sync-problem" role="note">${escapeHtml(p)}</div>`)
            .join('');
        const unlock =
            this.form === 'unlock'
                ? `<div class="sync-inline-form">
                <label class="sync-field"><span>${escapeHtml(t('settings.sync.setup.passphrase.label'))}</span>
                    <input type="password" class="setting-input" id="syncUnlockPass" dir="ltr" autocomplete="off"></label>
                <div class="sync-actions">${button('form-cancel', 'settings.sync.setup.cancel')}${button('unlock', 'settings.sync.action.unlock', { primary: true })}</div>
            </div>`
                : '';
        return `<div class="sync-head">
                <div class="sync-status-line" role="status">
                    <span class="sync-dot sync-dot--${v.tone}" aria-hidden="true"></span>
                    <span class="sync-status-text">${escapeHtml(v.text)}</span>
                </div>
                <div class="sync-head-actions">${button('sync-now', 'settings.sync.sync_now')}${pauseBtn}</div>
            </div>
            ${problems}
            ${actions ? `<div class="sync-actions">${actions}</div>` : ''}
            ${tip}${unlock}`;
    }

    _folderHtml() {
        const s = this.status || {};
        const me = (s.devices || []).find((d) => d.this);
        const rename =
            this.form === 'rename'
                ? `<div class="sync-inline-form">
                <label class="sync-field"><span>${escapeHtml(t('settings.sync.setup.name.label'))}</span>
                    <input type="text" class="setting-input" id="syncRenameInput" dir="auto" maxlength="64" value="${escapeAttr(me?.name || '')}"></label>
                <div class="sync-actions">${button('form-cancel', 'settings.sync.setup.cancel')}${button('rename-save', 'settings.sync.save', { primary: true })}</div>
            </div>`
                : '';
        return `<div class="sync-facts">
            <div class="sync-fact"><span class="sync-fact-label">${escapeHtml(t('settings.sync.folder_label'))}</span>
                <span class="sync-fact-value">${ltrText(s.folder?.display || '')}</span>
                ${button('change-folder', 'settings.sync.change')}</div>
            <div class="sync-fact"><span class="sync-fact-label">${escapeHtml(t('settings.sync.this_computer'))}</span>
                <span class="sync-fact-value">${userText(me?.name || '')}</span>
                ${button('rename-start', 'settings.sync.rename')}</div>
            ${rename}
        </div>`;
    }

    _categoryDetail(c) {
        const cfg = this.config || {};
        const on = this.status?.categories?.[c] !== false && !!this.status?.categories;
        switch (c) {
            case 'automations':
                return t('settings.sync.category.automations_counts', {
                    shortcuts: (cfg.shortcuts || []).length,
                    macros: (cfg.macros || []).length,
                    actions: (cfg.quick_actions?.custom_actions || []).length,
                });
            case 'extensions':
                return t('settings.sync.category.extensions_count', {
                    count: this.extensions.length,
                });
            case 'chats':
                return on ? t('settings.sync.category.chats_on') : t('settings.sync.category.off');
            default:
                return t('settings.sync.category.settings_desc');
        }
    }

    _categoriesHtml() {
        const cats = this.status?.categories || {};
        const rows = CATEGORIES.map((c) => {
            const choose =
                c === 'extensions' ? button('open-extensions', 'settings.sync.choose') : '';
            return `<div class="sync-category">
                <label><input type="checkbox" data-sync-category="${c}"${cats[c] ? ' checked' : ''}>
                <span class="sync-category-name">${escapeHtml(t(`settings.sync.category.${c}`))}</span></label>
                <span class="setting-description">${escapeHtml(this._categoryDetail(c))}</span>
                ${choose}
            </div>`;
        }).join('');
        return `<h3 class="sync-subtitle">${escapeHtml(t('settings.sync.categories.title'))}</h3>
            <div class="sync-categories">${rows}</div>
            <p class="setting-description">${escapeHtml(t('settings.sync.categories.per_computer'))}</p>`;
    }

    _attentionHtml() {
        const s = this.status || {};
        const r = this.reviews || {};
        const lines = [];
        const row = (icon, html, act) =>
            `<li class="sync-attention-row"><span class="sync-attention-icon" aria-hidden="true">${icon}</span>
                <span class="sync-attention-text">${html}</span>${button(act, 'settings.sync.review')}</li>`;
        const open = Number(s.conflicts_open) || 0;
        if (open > 0) {
            lines.push(
                row(
                    '⚠',
                    escapeHtml(t('settings.sync.attention.conflicts', { count: open })),
                    'open-conflicts'
                )
            );
        }
        const sug = Array.isArray(r.suggestions) ? r.suggestions : [];
        if (sug.length > 0) {
            lines.push(
                row(
                    '⬇',
                    escapeHtml(t('settings.sync.attention.suggestions', { count: sug.length })),
                    'open-reviews'
                )
            );
        }
        const run = Array.isArray(r.runnables) ? r.runnables : [];
        if (run.length > 0) {
            lines.push(
                row(
                    '▶',
                    escapeHtml(t('sync.review.runnable_title', { count: run.length })),
                    'open-reviews'
                )
            );
        }
        for (const m of Array.isArray(r.mass_changes) ? r.mass_changes : []) {
            lines.push(row('🗑', labelHtml(m.label_key, m.label_params), 'open-reviews'));
        }
        for (const a of Array.isArray(r.approvals) ? r.approvals : []) {
            lines.push(row('🔒', labelHtml(a.label_key, a.label_params), 'open-reviews'));
        }
        const removals = Array.isArray(r.removals) ? r.removals : [];
        for (const m of removals) {
            lines.push(row('↩', labelHtml(m.label_key, m.label_params), 'open-reviews'));
        }
        if (lines.length === 0) return '';
        return `<h3 class="sync-subtitle">${escapeHtml(t('settings.sync.attention.title'))}</h3>
            <ul class="sync-attention">${lines.join('')}</ul>`;
    }

    _devicesHtml() {
        return `<h3 class="sync-subtitle">${escapeHtml(t('settings.sync.devices.title'))}</h3>
            ${devicesHtml(this.status?.devices, Date.now(), { removing: this.removing })}`;
    }

    _notSyncedHtml() {
        const problems = Array.isArray(this.status?.problems) ? this.status.problems : [];
        const held = problems.filter(
            (p) => p.kind === 'held_back' || p.kind === 'not_this_version'
        );
        if (held.length === 0) return '';
        const lines = held
            .map(
                (p) =>
                    `<li><span>${escapeHtml(problemText(p))}</span>${p.kind === 'held_back' ? button('open-held', 'settings.sync.details') : ''}</li>`
            )
            .join('');
        return `<h3 class="sync-subtitle">${escapeHtml(t('settings.sync.not_synced.title'))}</h3>
            <ul class="sync-not-synced">${lines}</ul>`;
    }

    _staysHtml() {
        return `<h3 class="sync-subtitle">${escapeHtml(t('settings.sync.stays.title'))}</h3>
            <p class="setting-description">${escapeHtml(t('settings.sync.stays.list'))}</p>`;
    }

    _advancedHtml() {
        const s = this.status || {};
        const keyStore = s.key_store
            ? escapeHtml(
                  t('settings.sync.key_store_line', {
                      store: t(`settings.sync.key_store.${s.key_store}`),
                  })
              )
            : '';
        const undo = s.undo_join_until
            ? button('undo-join', 'settings.sync.undo_join_until', {
                  vars: { date: dateText(Date.parse(s.undo_join_until)) },
              })
            : '';
        let form = '';
        if (this.form === 'passphrase') {
            form = `<div class="sync-inline-form">
                <label class="sync-field"><span>${escapeHtml(t('settings.sync.passphrase.new'))}</span>
                    <input type="password" class="setting-input" id="syncNewPass" dir="ltr" autocomplete="new-password"></label>
                <label class="sync-field"><span>${escapeHtml(t('settings.sync.setup.passphrase.confirm'))}</span>
                    <input type="password" class="setting-input" id="syncNewPass2" dir="ltr" autocomplete="new-password"></label>
                <p class="setting-description">${escapeHtml(t('settings.sync.passphrase.change_help'))}</p>
                <div class="sync-actions">${button('form-cancel', 'settings.sync.setup.cancel')}${button('passphrase-save', 'settings.sync.passphrase.change_button', { primary: true })}</div>
            </div>`;
        } else if (this.form === 'start_over') {
            form = `<div class="sync-inline-form">
                <h4 class="sync-subtitle">${escapeHtml(t('settings.sync.start_over.title'))}</h4>
                <p>${escapeHtml(t('settings.sync.start_over.body'))}</p>
                <label class="sync-field"><span>${escapeHtml(t('settings.sync.passphrase.new'))}</span>
                    <input type="password" class="setting-input" id="syncNewPass" dir="ltr" autocomplete="new-password"></label>
                <label class="sync-field"><span>${escapeHtml(t('settings.sync.setup.passphrase.confirm'))}</span>
                    <input type="password" class="setting-input" id="syncNewPass2" dir="ltr" autocomplete="new-password"></label>
                <label class="sync-check"><input type="checkbox" id="syncStartOverAck">
                    <span>${escapeHtml(t('settings.sync.start_over.ack'))}</span></label>
                <div class="sync-actions">${button('form-cancel', 'settings.sync.setup.cancel')}${button('start-over', 'settings.sync.start_over.button', { danger: true })}</div>
            </div>`;
        }
        const open = this.form === 'passphrase' || this.form === 'start_over' ? ' open' : '';
        return `<details class="sync-advanced"${open}>
            <summary>${escapeHtml(t('settings.sync.advanced'))}</summary>
            <div class="sync-advanced-body">
                ${button('passphrase-start', 'settings.sync.passphrase.change_start')}
                ${keyStore ? `<span class="setting-description">${keyStore}</span>` : ''}
                ${undo}
                ${button('leave', 'settings.sync.leave.start')}
                ${button('start-over-start', 'settings.sync.start_over.start', { danger: true })}
            </div>
            ${form}
        </details>`;
    }

    _extensionsHtml() {
        const rows = this.extensions
            .map((item) => extensionSyncInfo(item, this.config, this.status))
            .map((x) => {
                const what = x.data
                    ? t('settings.sync.ext.settings_and_data')
                    : t('settings.sync.ext.settings_only');
                const control = x.data
                    ? `<label class="sync-check"><input type="checkbox" data-sync-ext-data="${escapeAttr(x.id)}"${x.dataOn ? ' checked' : ''}>
                        <span>${escapeHtml(t('settings.sync.ext.sync_data'))}</span></label>
                       ${button('ext-purge', 'settings.sync.ext.purge', { data: { id: x.id } })}`
                    : `<span class="setting-description">${escapeHtml(t(x.signIn ? 'settings.sync.ext.sign_in_local' : 'settings.sync.ext.no_data'))}</span>`;
                const notes = [
                    x.description
                        ? `<div class="setting-description">${escapeHtml(x.description)}</div>`
                        : '',
                    x.data && x.sideload
                        ? `<div class="setting-description">${escapeHtml(t('settings.sync.ext.sideload_hint'))}</div>`
                        : '',
                ].join('');
                return `<li class="sync-ext">
                    <span class="sync-ext-icon" aria-hidden="true">${escapeHtml(x.icon)}</span>
                    <span class="sync-ext-name">${userText(x.name)}</span>
                    <span class="setting-description">${escapeHtml(what)}</span>
                    ${control}${notes}
                </li>`;
            })
            .join('');
        return `<div class="sync-extensions">
            <div class="sync-view-head">${button('back', 'settings.sync.back')}
                <h3 class="sync-step-title" tabindex="-1">${escapeHtml(t('settings.sync.ext.title'))}</h3></div>
            <p class="setting-description">${escapeHtml(t('settings.sync.ext.intro'))}</p>
            ${this.error ? `<div class="sync-error" role="alert">${escapeHtml(this.error)}</div>` : ''}
            <ul class="sync-ext-list">${rows || `<li class="setting-description">${escapeHtml(t('settings.sync.ext.none'))}</li>`}</ul>
        </div>`;
    }

    /** Held values, by label: the Settings row's own label when there is one. */
    _heldHtml() {
        const rows = (this.held || [])
            .map(([path, view]) => {
                const label = document
                    .querySelector(`[data-sync-path="${CSS.escape(path)}"] .setting-label`)
                    ?.childNodes?.[0]?.textContent?.trim();
                const reason = view.reason_key ? t(view.reason_key) : '';
                return `<li><span>${label ? escapeHtml(label) : ltrText(path)}</span>
                    <span class="setting-description">${escapeHtml(reason)}</span></li>`;
            })
            .join('');
        return `<div class="sync-held">
            <div class="sync-view-head">${button('back', 'settings.sync.back')}
                <h3 class="sync-step-title" tabindex="-1">${escapeHtml(t('settings.sync.not_synced.title'))}</h3></div>
            <p class="setting-description">${escapeHtml(t('settings.sync.not_synced.help'))}</p>
            <ul class="sync-not-synced">${rows}</ul>
        </div>`;
    }

    // --- Actions ---------------------------------------------------------------

    async _destructive(action, run) {
        const confirm = await this.invoke('sync_prepare_destructive', { action });
        return run(confirm);
    }

    async _pickFolder() {
        if (this._deps.pickFolder) return this._deps.pickFolder();
        const dialog = window.__TAURI__?.dialog;
        if (!dialog?.open) return null;
        const chosen = await dialog.open({
            directory: true,
            multiple: false,
            title: t('settings.sync.setup.folder.title'),
        });
        return typeof chosen === 'string' ? chosen : null;
    }

    _startSetup() {
        this.setup = new SyncSetup({
            invoke: this.invoke,
            pickFolder: () => this._pickFolder(),
            onCancel: () => {
                this.setup = null;
                this.view = 'main';
            },
            onDone: (status) => {
                if (status) this.status = status;
                this.setup = null;
                this.view = 'main';
                this.refresh();
            },
        });
        this.view = 'setup';
        return this.setup.start();
    }

    async _leaveThen(next) {
        await this._destructive('leave', (confirm) => this.invoke('sync_leave', { confirm }));
        if (next === 'setup') await this._startSetup();
    }

    _go(section) {
        document.querySelector(`.sidebar-item[data-section="${section}"]`)?.click();
    }

    /** Run one action of the page. */
    async act(act, el) {
        const root = this.root;
        const invoke = this.invoke;
        this.flash = '';
        try {
            if (
                this.view === 'setup' &&
                this.setup &&
                (act.startsWith('setup-') || act === 'open-updates')
            ) {
                if (act === 'open-updates') this._go('updates');
                else await this.setup.handle(act, el, root);
                this.paint();
                return;
            }
            if (
                this.view === 'conflicts' &&
                this.conflicts &&
                (await this.conflicts.handle(act, el))
            ) {
                this.paint();
                return;
            }
            if (
                this.view === 'reviews' &&
                this.reviewView &&
                (await this.reviewView.handle(act, el))
            ) {
                this.paint();
                return;
            }
            this.error = '';
            switch (act) {
                case 'setup-start':
                    await this._startSetup();
                    break;
                case 'back':
                    this.view = 'main';
                    this.form = null;
                    await this.refresh();
                    return;
                case 'sync-now':
                    await invoke('sync_now');
                    this.flash = t('settings.sync.syncing_now');
                    break;
                case 'pause-menu':
                    this.pauseOpen = !this.pauseOpen;
                    break;
                case 'pause':
                    this.pauseOpen = false;
                    await invoke('sync_set_paused', { until: pauseUntil(el?.dataset.until) });
                    await this.refresh();
                    return;
                case 'resume':
                    await invoke('sync_set_paused', { until: null });
                    await this.refresh();
                    return;
                case 'keep-downloaded':
                    this.showTip = !this.showTip;
                    break;
                case 'unlock-start':
                    this.form = 'unlock';
                    break;
                case 'unlock': {
                    const passphrase = root.querySelector('#syncUnlockPass')?.value || '';
                    this.status = await invoke('sync_unlock', { passphrase });
                    this.form = null;
                    await this.refresh();
                    return;
                }
                case 'form-cancel':
                    this.form = null;
                    break;
                case 'find-folder': {
                    // Finding only looks; using the folder is its own step
                    // (§19.5 `sync_apply_folder`).
                    const found = await invoke('sync_find_folder');
                    if (found) await invoke('sync_apply_folder', { path: found });
                    this.flash = found
                        ? t('settings.sync.find.found')
                        : t('settings.sync.find.not_found');
                    await this.refresh();
                    return;
                }
                case 'change-folder': {
                    const ok = await confirmDialog({
                        title: t('settings.sync.change_folder.title'),
                        message: t('settings.sync.change_folder.body'),
                        confirmLabel: t('settings.sync.change_folder.ok'),
                        cancelLabel: t('settings.sync.setup.cancel'),
                    });
                    if (!ok) return;
                    await this._leaveThen('setup');
                    break;
                }
                case 'join-again':
                    await this._leaveThen('setup');
                    break;
                case 'ok-removed':
                    await this._leaveThen(null);
                    await this.refresh();
                    return;
                case 'open-updates':
                    this._go('updates');
                    return;
                case 'rename-start':
                    this.form = 'rename';
                    break;
                case 'rename-save': {
                    const name = (root.querySelector('#syncRenameInput')?.value || '').trim();
                    if (!name) {
                        this.error = t('errors.sync.no_name');
                        break;
                    }
                    await invoke('sync_rename_device', { name });
                    this.form = null;
                    await this.refresh();
                    return;
                }
                case 'passphrase-start':
                    this.form = 'passphrase';
                    break;
                case 'passphrase-save': {
                    const p = root.querySelector('#syncNewPass')?.value || '';
                    const p2 = root.querySelector('#syncNewPass2')?.value || '';
                    const err = passphraseError(p, p2);
                    if (err) {
                        this.error = err;
                        break;
                    }
                    await invoke('sync_change_passphrase', { newPassphrase: p });
                    this.form = null;
                    this.flash = t('settings.sync.passphrase.changed');
                    break;
                }
                case 'start-over-start':
                    this.form = 'start_over';
                    break;
                case 'start-over': {
                    const p = root.querySelector('#syncNewPass')?.value || '';
                    const p2 = root.querySelector('#syncNewPass2')?.value || '';
                    const err = passphraseError(p, p2);
                    if (err) {
                        this.error = err;
                        break;
                    }
                    if (!root.querySelector('#syncStartOverAck')?.checked) {
                        this.error = t('settings.sync.start_over.need_ack');
                        break;
                    }
                    this.status = await this._destructive('start_over', (confirm) =>
                        invoke('sync_start_over', { passphrase: p, confirm })
                    );
                    this.form = null;
                    await this.refresh();
                    return;
                }
                case 'leave': {
                    const ok = await confirmDialog({
                        title: t('settings.sync.leave.title'),
                        message: t('settings.sync.leave.body'),
                        confirmLabel: t('settings.sync.leave.ok'),
                        cancelLabel: t('settings.sync.setup.cancel'),
                        destructive: true,
                    });
                    if (!ok) return;
                    await this._leaveThen(null);
                    await this.refresh();
                    return;
                }
                case 'undo-join': {
                    const ok = await confirmDialog({
                        title: t('settings.sync.undo_join.title'),
                        message: t('settings.sync.undo_join.body'),
                        confirmLabel: t('settings.sync.undo_join.ok'),
                        cancelLabel: t('settings.sync.setup.cancel'),
                        destructive: true,
                    });
                    if (!ok) return;
                    await this._destructive('undo_join', (confirm) =>
                        invoke('sync_undo_join', { confirm })
                    );
                    this.view = 'main';
                    this.setup = null;
                    await this.refresh();
                    return;
                }
                case 'category': {
                    const category = el.dataset.syncCategory;
                    await invoke('sync_set_category', { category, on: el.checked });
                    await this.refresh();
                    return;
                }
                case 'open-conflicts':
                    this.conflicts = new SyncConflicts({
                        invoke,
                        onChanged: () => this._refreshStatusOnly(),
                    });
                    this.view = 'conflicts';
                    await this.conflicts.load();
                    break;
                case 'open-reviews':
                    this.reviewView = new SyncReviews({
                        invoke,
                        onChanged: () => this._refreshStatusOnly(),
                        openStore: () => {
                            invoke('open_store_window', { tab: 'extensions' }).catch(() => {});
                        },
                        openExtension: (id) => this._go(`ext-${id}`),
                    });
                    this.view = 'reviews';
                    await this.reviewView.load();
                    break;
                case 'open-extensions':
                    try {
                        this.extensions = await invoke('list_extensions');
                    } catch {
                        this.extensions = [];
                    }
                    this.view = 'extensions';
                    break;
                case 'ext-data':
                    await invoke('sync_set_extension_data', {
                        extensionId: el.dataset.syncExtData,
                        on: el.checked,
                    });
                    await this._refreshStatusOnly();
                    break;
                case 'ext-purge': {
                    const id = el.dataset.id;
                    const ok = await confirmDialog({
                        title: t('settings.sync.ext.purge_title'),
                        message: t('settings.sync.ext.purge_body'),
                        confirmLabel: t('settings.sync.ext.purge_ok'),
                        cancelLabel: t('settings.sync.setup.cancel'),
                        destructive: true,
                    });
                    if (!ok) return;
                    await this._destructive('purge_extension', (confirm) =>
                        invoke('sync_purge_extension', { extensionId: id, confirm })
                    );
                    await this._refreshStatusOnly();
                    this.flash = t('settings.sync.ext.purged');
                    break;
                }
                case 'open-held': {
                    const views = await invoke('sync_get_scopes', { paths: null });
                    this.held = Object.entries(views || {}).filter(([, v]) => v?.held);
                    this.view = 'held';
                    break;
                }
                case 'device-remove-start':
                    this.removing = el.dataset.id;
                    break;
                case 'device-remove-cancel':
                    this.removing = null;
                    break;
                case 'device-remove-confirm':
                    await removeDevice(invoke, root, el.dataset.id);
                    this.removing = null;
                    await this.refresh();
                    return;
                default:
                    return;
            }
        } catch (e) {
            this.error = errMessage(e);
        }
        this.paint();
    }

    async _refreshStatusOnly() {
        try {
            this.status = await this.invoke('sync_get_status');
        } catch {}
    }
}
