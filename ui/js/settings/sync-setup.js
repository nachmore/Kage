/**
 * Settings → Sync setup, as steps in the page (docs/design/SYNC.md §13.2):
 * choose a folder, the passphrase, this computer's name, what to sync,
 * which side wins when joining, then the first sync.
 *
 * A step object holds the state; `render()` returns HTML and `handle()`
 * runs a button's action. The page (`sync.js`) repaints after each call.
 * Every visible string goes through the catalog; user text is escaped.
 */

import { errKind, errMessage } from '../shared/error-message.js';
import { t, tHtml } from '../shared/i18n.js';
import { currentOs } from '../shared/sync-scope-chip.js';
import {
    escapeAttr,
    escapeHtml,
    isolate,
    listText,
    ltrText,
    osName,
    userText,
} from './sync-format.js';

/** The shortest passphrase sync accepts (§11.3). */
export const MIN_PASSPHRASE = 12;

export const CATEGORIES = ['settings', 'automations', 'extensions', 'chats'];

const DEFAULT_CATEGORIES = { settings: true, automations: true, extensions: true, chats: false };

/** Steps, in order. `join` exists only when joining. */
export const STEPS = ['folder', 'passphrase', 'name', 'categories', 'join', 'done'];

/**
 * Check a new passphrase: long enough, and both fields the same. Returns
 * an error message, or null.
 */
export function passphraseError(passphrase, confirm, { creating = true } = {}) {
    if (typeof passphrase !== 'string' || passphrase.length === 0) {
        return t('settings.sync.passphrase.required');
    }
    if (!creating) return null;
    // The engine's rule: at least 12 characters after NFC (membership.rs).
    if ([...passphrase.normalize('NFC')].length < MIN_PASSPHRASE) {
        return t('errors.sync.passphrase_short');
    }
    if (passphrase !== confirm) return t('settings.sync.passphrase.mismatch');
    return null;
}

/** The provider tip under the folder choice (§13.2). */
export function providerTip(provider, os = currentOs()) {
    switch (provider) {
        case 'onedrive':
        case 'dropbox':
            return os === 'windows' ? t('settings.sync.tip.keep_downloaded_windows') : '';
        case 'icloud':
            return t('settings.sync.tip.icloud');
        case 'google_drive':
            return t('settings.sync.tip.google_drive');
        case 'syncthing':
            return t('settings.sync.tip.syncthing');
        default:
            return '';
    }
}

/** A default name for this computer. */
export function defaultDeviceName(os = currentOs()) {
    return t(`settings.sync.name.default.${os === 'macos' || os === 'windows' ? os : 'linux'}`);
}

function sumCounts(c) {
    if (!c || typeof c !== 'object') return 0;
    return CATEGORIES.reduce((n, k) => n + (Number(c[k]) || 0), 0);
}

export class SyncSetup {
    /**
     * @param {{ invoke: Function, onDone: (status: object) => void,
     *   onCancel: () => void, onOpen?: (view: string) => void,
     *   pickFolder?: () => Promise<string | null> }} deps
     * @param {{ role?: 'join' }} [opts] - start by joining (Join again)
     */
    constructor(deps) {
        this.deps = deps;
        this.step = 'folder';
        this.suggestions = null;
        this.path = '';
        this.info = null;
        this.passphrase = '';
        this.confirm = '';
        this.generated = '';
        this.name = defaultDeviceName();
        this.categories = { ...DEFAULT_CATEGORIES };
        this.preview = null;
        this.choice = 'theirs';
        this.status = null;
        this.reviews = null;
        this.error = '';
        this.busy = false;
    }

    get role() {
        return this.info?.kind === 'existing' ? 'join' : 'create';
    }

    /** The steps this run goes through. */
    steps() {
        return STEPS.filter((s) => s !== 'join' || this.role === 'join');
    }

    async start() {
        try {
            this.suggestions = await this.deps.invoke('sync_suggest_folders');
        } catch {
            this.suggestions = [];
        }
    }

    _stepNumber() {
        const steps = this.steps().filter((s) => s !== 'done');
        return { n: steps.indexOf(this.step) + 1, of: steps.length };
    }

    render() {
        const body = {
            folder: () => this._renderFolder(),
            passphrase: () => this._renderPassphrase(),
            name: () => this._renderName(),
            categories: () => this._renderCategories(),
            join: () => this._renderJoin(),
            done: () => this._renderDone(),
        }[this.step]();
        const { n, of } = this._stepNumber();
        const progress =
            this.step === 'done'
                ? ''
                : `<div class="sync-step-count">${escapeHtml(
                      t('settings.sync.setup.step', { n, of })
                  )}</div>`;
        const error = this.error
            ? `<div class="sync-error" role="alert">${escapeHtml(this.error)}</div>`
            : '';
        return `<div class="sync-setup" data-step="${escapeAttr(this.step)}" aria-busy="${this.busy}">
            ${progress}${body}${error}</div>`;
    }

    _nav({
        back = true,
        next = 'next',
        nextLabel = t('settings.sync.setup.continue'),
        disabled = false,
    } = {}) {
        return `<div class="sync-actions">
            ${back ? `<button type="button" class="setting-button" data-sync-act="setup-back">${escapeHtml(t('settings.sync.setup.back'))}</button>` : `<button type="button" class="setting-button" data-sync-act="setup-cancel">${escapeHtml(t('settings.sync.setup.cancel'))}</button>`}
            <button type="button" class="setting-button primary" data-sync-act="setup-${next}"${disabled || this.busy ? ' disabled' : ''}>${escapeHtml(nextLabel)}</button>
        </div>`;
    }

    _renderFolder() {
        const picks = (this.suggestions || [])
            .map(
                (
                    s,
                    i
                ) => `<li><button type="button" class="sync-pick${s.path === this.path ? ' selected' : ''}" data-sync-act="setup-pick" data-index="${i}" aria-pressed="${s.path === this.path}">
                    <span class="sync-pick-label">${userText(s.label)}</span>
                    <span class="sync-pick-path">${ltrText(s.path)}</span>
                </button></li>`
            )
            .join('');
        let result = '';
        const info = this.info;
        if (info) {
            const tip = providerTip(info.provider);
            const kindText = {
                new: tHtml('settings.sync.folder.new'),
                existing: tHtml('settings.sync.folder.existing', {
                    count: Number(info.computer_count) || 0,
                }),
                unknown_sync: tHtml('settings.sync.folder.unknown_sync'),
                not_usable: info.reason ? escapeHtml(t(info.reason)) : '',
                newer_format: tHtml('settings.sync.folder.newer_format'),
            }[info.kind];
            result = `<div class="sync-folder-result sync-folder-result--${escapeAttr(info.kind)}" role="status">
                <div class="sync-folder-chosen">${ltrText(this.path)}</div>
                <p>${kindText || ''}</p>
                ${tip && info.kind !== 'not_usable' ? `<p class="setting-description">${escapeHtml(tip)}</p>` : ''}
                ${info.kind === 'newer_format' ? `<button type="button" class="setting-button" data-sync-act="open-updates">${escapeHtml(t('settings.sync.check_updates'))}</button>` : ''}
            </div>`;
        }
        const usable = info && info.kind !== 'not_usable' && info.kind !== 'newer_format';
        return `<h3 class="sync-step-title" tabindex="-1">${escapeHtml(t('settings.sync.setup.folder.title'))}</h3>
            <p class="setting-description">${escapeHtml(t('settings.sync.setup.folder.help'))}</p>
            ${picks ? `<ul class="sync-picks">${picks}</ul>` : ''}
            <button type="button" class="setting-button" data-sync-act="setup-browse">${escapeHtml(t('settings.sync.setup.folder.other'))}</button>
            ${result}
            ${this._nav({ back: false, disabled: !usable })}`;
    }

    _renderPassphrase() {
        if (this.role === 'join') {
            return `<h3 class="sync-step-title" tabindex="-1">${escapeHtml(t('settings.sync.setup.passphrase.join_title'))}</h3>
                <label class="sync-field">
                    <span>${escapeHtml(t('settings.sync.setup.passphrase.label'))}</span>
                    <input type="password" class="setting-input" id="syncPassphrase" dir="ltr" autocomplete="off" value="${escapeAttr(this.passphrase)}">
                </label>
                <p class="setting-description">${escapeHtml(t('settings.sync.setup.passphrase.join_help'))}</p>
                ${this._nav()}`;
        }
        const generated = this.generated
            ? `<div class="sync-generated" role="status">
                <span class="setting-description">${escapeHtml(t('settings.sync.setup.passphrase.generated'))}</span>
                <code dir="ltr" class="ltr sync-generated-value">${escapeHtml(this.generated)}</code>
                <button type="button" class="setting-button" data-sync-act="setup-copy">${escapeHtml(t('settings.sync.setup.passphrase.copy'))}</button>
            </div>`
            : '';
        return `<h3 class="sync-step-title" tabindex="-1">${escapeHtml(t('settings.sync.setup.passphrase.create_title'))}</h3>
            <label class="sync-field">
                <span>${escapeHtml(t('settings.sync.setup.passphrase.label'))}</span>
                <input type="password" class="setting-input" id="syncPassphrase" dir="ltr" autocomplete="new-password" value="${escapeAttr(this.passphrase)}">
            </label>
            <label class="sync-field">
                <span>${escapeHtml(t('settings.sync.setup.passphrase.confirm'))}</span>
                <input type="password" class="setting-input" id="syncPassphraseConfirm" dir="ltr" autocomplete="new-password" value="${escapeAttr(this.confirm)}">
            </label>
            <p class="setting-description">${escapeHtml(t('settings.sync.setup.passphrase.rule', { count: MIN_PASSPHRASE }))}</p>
            <button type="button" class="setting-button" data-sync-act="setup-generate">${escapeHtml(t('settings.sync.setup.passphrase.make'))}</button>
            ${generated}
            <p class="setting-description">${escapeHtml(t('settings.sync.setup.passphrase.once'))} ${escapeHtml(t('settings.sync.passphrase.lost'))}</p>
            ${this._nav()}`;
    }

    _renderName() {
        return `<h3 class="sync-step-title" tabindex="-1">${escapeHtml(t('settings.sync.setup.name.title'))}</h3>
            <label class="sync-field">
                <span>${escapeHtml(t('settings.sync.setup.name.label'))}</span>
                <input type="text" class="setting-input" id="syncDeviceName" dir="auto" maxlength="64" value="${escapeAttr(this.name)}">
            </label>
            <p class="setting-description">${escapeHtml(t('settings.sync.setup.name.help'))}</p>
            ${this._nav()}`;
    }

    _renderCategories() {
        const rows = CATEGORIES.map(
            (c) => `<label class="sync-category">
                <input type="checkbox" data-sync-category="${c}"${this.categories[c] ? ' checked' : ''}>
                <span class="sync-category-name">${escapeHtml(t(`settings.sync.category.${c}`))}</span>
                <span class="setting-description">${escapeHtml(t(`settings.sync.category.${c}_desc`))}</span>
            </label>`
        ).join('');
        const last = this.role === 'join' ? 'join' : 'create';
        return `<h3 class="sync-step-title" tabindex="-1">${escapeHtml(t('settings.sync.setup.categories.title'))}</h3>
            <fieldset class="sync-categories"><legend class="visually-hidden">${escapeHtml(t('settings.sync.setup.categories.title'))}</legend>${rows}</fieldset>
            <p class="setting-description">${escapeHtml(t('settings.sync.categories.per_computer'))}</p>
            <p class="setting-description">${escapeHtml(t('settings.sync.setup.categories.pasted_keys'))}</p>
            ${this._nav({
                next: last,
                nextLabel:
                    last === 'join'
                        ? t('settings.sync.setup.unlock')
                        : t('settings.sync.setup.turn_on'),
            })}`;
    }

    _renderJoin() {
        const p = this.preview || {};
        const computers = listText(
            (p.computers || []).map((c) =>
                t('settings.sync.join.computer', { name: isolate(c.name), os: osName(c.os) })
            )
        );
        const different = sumCounts(p.different);
        const onlyHere = sumCounts(p.only_here);
        const option = (value, key) => `<label class="sync-choice">
            <input type="radio" name="syncJoinChoice" value="${value}"${this.choice === value ? ' checked' : ''} data-sync-choice>
            <span>${escapeHtml(t(key))}</span>
        </label>`;
        return `<h3 class="sync-step-title" tabindex="-1">${escapeHtml(t('settings.sync.join.title'))}</h3>
            ${computers ? `<p>${tHtml('settings.sync.join.from', { computers })}</p>` : ''}
            <p>${escapeHtml(t('settings.sync.join.question'))}</p>
            <p class="setting-description">${escapeHtml(t('settings.sync.join.counts', { different, only_here: onlyHere }))}</p>
            <fieldset class="sync-choices" role="radiogroup"><legend class="visually-hidden">${escapeHtml(t('settings.sync.join.question'))}</legend>
                ${option('theirs', 'settings.sync.join.theirs')}
                ${option('mine', 'settings.sync.join.mine')}
            </fieldset>
            <p class="setting-description">${escapeHtml(t('settings.sync.join.safe'))}</p>
            ${this._nav({ next: 'confirm', nextLabel: t('settings.sync.setup.join') })}`;
    }

    _renderDone() {
        const s = this.status || {};
        const r = this.reviews || {};
        const lines = [];
        if (s.state === 'waiting') {
            lines.push(
                `<p class="setting-description">${escapeHtml(t('sync.status.waiting_any', { count: Number(s.waiting_files) || 0 }))}</p>`
            );
        }
        const suggestions = Array.isArray(r.suggestions) ? r.suggestions.length : 0;
        const runnables = Array.isArray(r.runnables) ? r.runnables.length : 0;
        if (suggestions > 0) {
            lines.push(
                `<div class="sync-attention-row"><span>${escapeHtml(t('settings.sync.done.suggestions', { count: suggestions }))}</span>
                <button type="button" class="setting-button" data-sync-act="open-reviews">${escapeHtml(t('settings.sync.review'))}</button></div>`
            );
        }
        if (runnables > 0) {
            lines.push(
                `<div class="sync-attention-row"><span>${escapeHtml(t('sync.review.runnable_title', { count: runnables }))}</span>
                <button type="button" class="setting-button" data-sync-act="open-reviews">${escapeHtml(t('settings.sync.review'))}</button></div>`
            );
        }
        const replaced = Number(s.conflicts_open) || 0;
        if (this.role === 'join' && replaced > 0) {
            lines.push(
                `<div class="sync-attention-row"><span>${escapeHtml(t('settings.sync.done.replaced', { count: replaced }))}</span>
                <button type="button" class="setting-button" data-sync-act="open-conflicts">${escapeHtml(t('settings.sync.review'))}</button>
                <button type="button" class="setting-button" data-sync-act="undo-join">${escapeHtml(t('settings.sync.undo'))}</button></div>`
            );
        }
        return `<h3 class="sync-step-title" tabindex="-1"><span aria-hidden="true">✓</span> ${escapeHtml(t('settings.sync.done.title'))}</h3>
            ${lines.join('')}
            <div class="sync-actions"><button type="button" class="setting-button primary" data-sync-act="setup-finish">${escapeHtml(t('settings.sync.done.button'))}</button></div>`;
    }

    /** Read the step's fields into the state before acting. */
    readForm(root) {
        const val = (id) => root.querySelector(`#${id}`)?.value;
        if (this.step === 'passphrase') {
            this.passphrase = val('syncPassphrase') ?? this.passphrase;
            this.confirm = val('syncPassphraseConfirm') ?? this.confirm;
        } else if (this.step === 'name') {
            this.name = (val('syncDeviceName') ?? this.name).trim();
        } else if (this.step === 'categories') {
            for (const el of root.querySelectorAll('[data-sync-category]')) {
                this.categories[el.dataset.syncCategory] = el.checked;
            }
        } else if (this.step === 'join') {
            const picked = root.querySelector('[data-sync-choice]:checked');
            if (picked) this.choice = picked.value;
        }
    }

    _go(step) {
        this.step = step;
        this.error = '';
    }

    _prev() {
        const steps = this.steps();
        const i = steps.indexOf(this.step);
        if (this.step === 'join') return 'passphrase';
        return i > 0 ? steps[i - 1] : 'folder';
    }

    _next() {
        const steps = this.steps();
        return steps[steps.indexOf(this.step) + 1] || 'done';
    }

    async _choose(path) {
        this.path = path;
        this.info = null;
        this.error = '';
        try {
            this.info = await this.deps.invoke('sync_inspect_folder', { path });
        } catch (e) {
            this.error = errMessage(e);
        }
    }

    /**
     * Run one action of the setup. Returns false when the action isn't a
     * setup action.
     */
    async handle(act, el, root) {
        this.readForm(root);
        const invoke = this.deps.invoke;
        switch (act) {
            case 'setup-cancel':
                this.deps.onCancel();
                return true;
            case 'setup-back':
                this._go(this._prev());
                return true;
            case 'setup-pick': {
                const s = this.suggestions?.[Number(el?.dataset.index)];
                if (s) await this._choose(s.path);
                return true;
            }
            case 'setup-browse': {
                const path = await this.deps.pickFolder?.();
                if (path) await this._choose(path);
                return true;
            }
            case 'setup-generate':
                try {
                    this.generated = await invoke('sync_generate_passphrase');
                    this.passphrase = this.generated;
                    this.confirm = this.generated;
                    this.error = '';
                } catch (e) {
                    this.error = errMessage(e);
                }
                return true;
            case 'setup-copy':
                try {
                    await navigator.clipboard?.writeText(this.generated);
                } catch {}
                return true;
            case 'setup-next':
                if (this.step === 'passphrase') {
                    const err = passphraseError(this.passphrase, this.confirm, {
                        creating: this.role === 'create',
                    });
                    if (err) {
                        this.error = err;
                        return true;
                    }
                }
                if (this.step === 'name' && !this.name) {
                    this.error = t('errors.sync.no_name');
                    return true;
                }
                this._go(this._next());
                return true;
            case 'setup-create':
                return this._run(async () => {
                    this.status = await invoke('sync_create', this._args());
                    await this._afterOn();
                });
            case 'setup-join':
                return this._run(async () => {
                    try {
                        this.preview = await invoke('sync_join', this._args());
                        this._go('join');
                    } catch (e) {
                        // A wrong passphrase goes back to where it's typed.
                        if (
                            errKind(e) === 'invalid_input' &&
                            errMessage(e) === t('settings.sync.passphrase.wrong')
                        ) {
                            this._go('passphrase');
                        }
                        throw e;
                    }
                });
            case 'setup-confirm':
                return this._run(async () => {
                    this.status = await invoke('sync_confirm_join', { choice: this.choice });
                    await this._afterOn();
                });
            case 'setup-finish':
                this.deps.onDone(this.status);
                return true;
            default:
                return false;
        }
    }

    _args() {
        return {
            path: this.path,
            passphrase: this.passphrase,
            deviceName: this.name,
            categories: { ...this.categories },
        };
    }

    async _afterOn() {
        this.passphrase = '';
        this.confirm = '';
        this.generated = '';
        try {
            this.reviews = await this.deps.invoke('sync_list_reviews');
        } catch {
            this.reviews = null;
        }
        this._go('done');
    }

    async _run(fn) {
        this.busy = true;
        this.error = '';
        try {
            await fn();
        } catch (e) {
            this.error = errMessage(e);
        } finally {
            this.busy = false;
        }
        return true;
    }
}
