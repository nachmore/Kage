/**
 * Settings → Sync → Review (docs/design/SYNC.md §8.10, §13.3): synced
 * items that start programs and wait for approval here, extensions on
 * other computers, extensions removed elsewhere, bulk deletes held for an
 * answer, and extensions that need consent on this computer.
 *
 * What a held item runs is shown left to right, as text, before the user
 * allows it.
 */

import { errMessage } from '../shared/error-message.js';
import { t, tHtml } from '../shared/i18n.js';
import { escapeAttr, escapeHtml, isolate, labelHtml } from './sync-format.js';

export const REVIEW_LISTS = ['runnables', 'mass_changes', 'approvals', 'suggestions', 'removals'];

/** How many items wait in a `Reviews` answer. */
export function reviewCount(reviews) {
    if (!reviews || typeof reviews !== 'object') return 0;
    return REVIEW_LISTS.reduce(
        (n, k) => n + (Array.isArray(reviews[k]) ? reviews[k].length : 0),
        0
    );
}

/** The buttons each kind of item offers: [action, label key, primary]. */
function buttonsFor(list, item) {
    switch (list) {
        case 'runnables':
            return [
                ['review-approve', 'sync.review.allow', true],
                ['review-keep-off', 'sync.review.keep_off', false],
            ];
        case 'mass_changes':
            return item.kind === 'mass_delete_out'
                ? [
                      ['review-mass-accept', 'sync.review.delete_everywhere', true],
                      ['review-mass-refuse', 'sync.review.cancel_delete', false],
                  ]
                : [
                      ['review-mass-accept', 'sync.review.delete_here', true],
                      ['review-mass-refuse', 'sync.review.keep_them', false],
                  ];
        case 'approvals':
            return [['review-open-extension', 'settings.sync.review', true]];
        case 'suggestions':
            return item.kind === 'sideload'
                ? [['review-dismiss', 'sync.review.not_here', false]]
                : [
                      ['review-open-store', 'settings.sync.review', true],
                      ['review-dismiss', 'sync.review.not_here', false],
                  ];
        case 'removals':
            return [
                ['review-remove-here', 'sync.review.remove_here', false],
                ['review-dismiss', 'sync.review.keep', false],
            ];
        default:
            return [];
    }
}

/** One item as HTML. */
export function reviewItemHtml(list, item) {
    const label = labelHtml(item.label_key, item.label_params);
    const from = item.computer
        ? `<div class="setting-description">${tHtml('sync.review.from', { computer: isolate(item.computer) })}</div>`
        : '';
    const detail = item.detail
        ? `<pre class="sync-runnable" dir="ltr">${escapeHtml(item.detail)}</pre>`
        : '';
    const hint =
        list === 'suggestions' && item.kind === 'sideload'
            ? `<div class="setting-description">${escapeHtml(t('sync.review.sideload_hint'))}</div>`
            : '';
    const buttons = buttonsFor(list, item)
        .map(
            ([act, key, primary]) =>
                `<button type="button" class="setting-button${primary ? ' primary' : ''}" data-sync-act="${act}" data-list="${list}" data-id="${escapeAttr(item.id)}" data-kind="${escapeAttr(item.kind || '')}">${escapeHtml(t(key))}</button>`
        )
        .join('');
    return `<li class="sync-review-item">
        <div class="sync-review-label">${label}</div>
        ${from}${detail}${hint}
        <div class="sync-actions">${buttons}</div>
    </li>`;
}

export class SyncReviews {
    /**
     * @param {{ invoke: Function, onChanged?: () => void,
     *   openStore?: (id: string) => void,
     *   openExtension?: (id: string) => void }} deps
     */
    constructor(deps) {
        this.deps = deps;
        this.reviews = null;
        this.error = '';
    }

    async load() {
        try {
            this.reviews = await this.deps.invoke('sync_list_reviews');
            this.error = '';
        } catch (e) {
            this.reviews = null;
            this.error = errMessage(e);
        }
    }

    render() {
        const r = this.reviews || {};
        const sections = REVIEW_LISTS.map((list) => {
            const items = Array.isArray(r[list]) ? r[list] : [];
            if (items.length === 0) return '';
            const title =
                list === 'runnables'
                    ? t('sync.review.runnable_title', { count: items.length })
                    : t(`sync.review.title.${list}`, { count: items.length });
            const intro =
                list === 'runnables'
                    ? `<p class="setting-description">${escapeHtml(t('sync.review.runnable_intro'))}</p>`
                    : '';
            return `<h4 class="sync-subtitle">${escapeHtml(title)}</h4>${intro}
                <ul class="sync-review-list">${items.map((it) => reviewItemHtml(list, it)).join('')}</ul>`;
        }).join('');
        const empty =
            this.reviews && reviewCount(this.reviews) === 0
                ? `<p class="setting-description">${escapeHtml(t('sync.review.empty'))}</p>`
                : '';
        return `<div class="sync-reviews">
            <div class="sync-view-head">
                <button type="button" class="setting-button" data-sync-act="back">${escapeHtml(t('settings.sync.back'))}</button>
                <h3 class="sync-step-title" tabindex="-1">${escapeHtml(t('sync.review.heading'))}</h3>
            </div>
            ${this.error ? `<div class="sync-error" role="alert">${escapeHtml(this.error)}</div>` : ''}
            ${empty}${sections}
        </div>`;
    }

    async _do(fn) {
        try {
            await fn();
            this.error = '';
        } catch (e) {
            this.error = errMessage(e);
        }
        await this.load();
        if (this.error === '') this.deps.onChanged?.();
    }

    /** Run one review action; false when it isn't one. */
    async handle(act, el) {
        const invoke = this.deps.invoke;
        const id = el?.dataset.id || '';
        const kind = el?.dataset.kind || '';
        switch (act) {
            case 'review-approve':
                await this._do(() => invoke('sync_approve_runnable', { key: id }));
                return true;
            case 'review-keep-off':
                await this._do(() => invoke('sync_keep_off', { key: id }));
                return true;
            case 'review-mass-accept':
                await this._do(() => invoke('sync_answer_mass_change', { ns: id, accept: true }));
                return true;
            case 'review-mass-refuse':
                await this._do(() => invoke('sync_answer_mass_change', { ns: id, accept: false }));
                return true;
            case 'review-dismiss':
                await this._do(() => invoke('sync_dismiss_suggestion', { kind, id }));
                return true;
            case 'review-remove-here': {
                // `kind:id` → uninstall the item, then the removal is done.
                const [itemKind, ...rest] = id.split(':');
                const itemId = rest.join(':');
                await this._do(async () => {
                    await invoke('uninstall_extension', { id: itemId, kind: itemKind });
                    await invoke('sync_dismiss_suggestion', { kind, id });
                });
                return true;
            }
            case 'review-open-store':
                this.deps.openStore?.(id);
                return true;
            case 'review-open-extension':
                this.deps.openExtension?.(id);
                return true;
            default:
                return false;
        }
    }
}
