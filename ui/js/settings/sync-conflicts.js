/**
 * Settings → Sync → Conflicts (docs/design/SYNC.md §10.4, §10.5): what two
 * computers changed at once, what was deleted, and what was resolved.
 *
 * Entries made by one action (a join, a restored backup, a computer back
 * from a long absence) are grouped into one line. Kage's own generated
 * documents sit in a collapsed group. Values are escaped text; Compare
 * shows both versions side by side.
 */

import { errMessage } from '../shared/error-message.js';
import { t, tHtml } from '../shared/i18n.js';
import {
    dateText,
    escapeAttr,
    escapeHtml,
    isolate,
    labelHtml,
    labelText,
    shortTime,
} from './sync-format.js';

export const TABS = ['replaced', 'deleted', 'resolved'];

/** Groups made by one action, and how they read. */
const GROUP_KINDS = new Set(['replaced_on_join', 'restore_backup', 'deleted_while_away']);

function startOfDay(ms) {
    const d = new Date(ms);
    d.setHours(0, 0, 0, 0);
    return d.getTime();
}

/**
 * Sort a tab's rows into the sections of the screen: single entries from
 * today and earlier, grouped actions, and Kage's generated documents.
 *
 * @param {object[]} rows - `ConflictSummary` rows
 * @param {number} now
 * @returns {{ today: object[], earlier: object[],
 *   groups: { group: string, kind: string, rows: object[] }[],
 *   generated: object[] }}
 */
export function groupConflicts(rows, now = Date.now()) {
    const out = { today: [], earlier: [], groups: [], generated: [] };
    const byGroup = new Map();
    const today = startOfDay(now);
    for (const r of Array.isArray(rows) ? rows : []) {
        if (r.group === 'generated') {
            out.generated.push(r);
            continue;
        }
        if (r.group && GROUP_KINDS.has(r.kind)) {
            if (!byGroup.has(r.group)) {
                const g = { group: r.group, kind: r.kind, rows: [] };
                byGroup.set(r.group, g);
                out.groups.push(g);
            }
            byGroup.get(r.group).rows.push(r);
            continue;
        }
        ((r.loser?.at ?? 0) >= today ? out.today : out.earlier).push(r);
    }
    return out;
}

function itemLabel(row) {
    return labelHtml(row.label_key, row.label_params);
}

function who(name) {
    return isolate(name || t('settings.sync.conflicts.another_computer'));
}

/**
 * The text lines of one entry, as HTML, and the actions it offers. Kinds
 * that read as a sentence about the item (a delete against an edit, a
 * restore) carry the item in the sentence and have no separate title.
 *
 * @returns {{ title: string, lines: string[], actions: string[] }}
 */
export function conflictView(row, { tab = 'replaced', now = Date.now() } = {}) {
    let title = itemLabel(row);
    const item = labelText(row.label_key, row.label_params);
    const loser = row.loser || {};
    const winner = row.winner || null;
    const lines = [];
    const actions = [];
    if (tab === 'deleted') {
        lines.push(
            tHtml('sync.conflicts.deleted_on', {
                computer: who(loser.computer),
                time: shortTime(loser.at, now),
            })
        );
        return { title, lines, actions: ['restore'] };
    }
    switch (row.kind) {
        case 'edit_vs_delete':
            title = '';
            lines.push(
                tHtml('sync.conflicts.kind.edit_vs_delete', {
                    item,
                    deleter: who(winner?.computer),
                    editor: who(loser.computer),
                })
            );
            break;
        case 'delete_vs_edit':
            title = '';
            lines.push(
                tHtml('sync.conflicts.kind.delete_vs_edit', {
                    item,
                    deleter: who(loser.computer),
                    editor: who(winner?.computer),
                })
            );
            actions.push('delete_anyway');
            break;
        case 'replaced_by_restore':
            title = '';
            lines.push(
                tHtml('sync.conflicts.kind.replaced_by_restore', {
                    item,
                    value: loser.preview || '',
                })
            );
            break;
        default:
            if (winner) {
                lines.push(
                    tHtml('sync.conflicts.now_changed', {
                        computer: who(winner.computer),
                        time: shortTime(winner.at, now),
                    })
                );
            }
            lines.push(
                loser.preview
                    ? tHtml('sync.conflicts.replaced', {
                          value: loser.preview,
                          computer: who(loser.computer),
                          time: shortTime(loser.at, now),
                      })
                    : tHtml('sync.conflicts.replaced_changed', {
                          computer: who(loser.computer),
                          time: shortTime(loser.at, now),
                      })
            );
    }
    if (tab === 'resolved') {
        lines.push(
            escapeHtml(
                t(
                    row.state === 'restored'
                        ? 'sync.conflicts.state.restored'
                        : 'sync.conflicts.state.dismissed'
                )
            )
        );
        return { title, lines, actions: [] };
    }
    if (row.kind !== 'delete_vs_edit' && row.kind !== 'replaced_by_restore') {
        actions.push('compare');
    }
    actions.push('restore', 'dismiss');
    return { title, lines, actions };
}

/** The one line of a grouped action. */
export function groupLine(g) {
    const first = g.rows[0] || {};
    const count = g.rows.length;
    const at = first.loser?.at;
    switch (g.kind) {
        case 'replaced_on_join':
            return tHtml('sync.conflicts.kind.replaced_on_join', {
                computer: who(first.winner?.computer || first.loser?.computer),
                count,
            });
        case 'restore_backup':
            return tHtml('sync.conflicts.kind.restore_backup', {
                computer: who(first.winner?.computer || first.loser?.computer),
                count,
            });
        default:
            return tHtml('sync.conflicts.kind.deleted_while_away', {
                computer: who(first.loser?.computer),
                date: dateText(at),
                count,
            });
    }
}

const BUTTON_KEYS = {
    compare: 'sync.conflicts.compare',
    restore: 'sync.conflicts.restore',
    dismiss: 'sync.conflicts.dismiss',
    delete_anyway: 'sync.conflicts.delete_anyway',
};

export class SyncConflicts {
    /**
     * @param {{ invoke: Function, onChanged?: () => void,
     *   onUndoJoin?: () => void }} deps
     */
    constructor(deps) {
        this.deps = deps;
        this.tab = 'replaced';
        this.rows = [];
        this.expanded = new Set();
        this.details = new Map();
        this.showGenerated = false;
        this.error = '';
        this.loaded = false;
    }

    async load() {
        try {
            this.rows = await this.deps.invoke('sync_list_conflicts', { tab: this.tab });
            this.error = '';
        } catch (e) {
            this.rows = [];
            this.error = errMessage(e);
        }
        this.loaded = true;
    }

    _entry(row, now) {
        const v = conflictView(row, { tab: this.tab, now });
        const buttons = v.actions
            .map(
                (a) =>
                    `<button type="button" class="setting-button${a === 'restore' ? ' primary' : ''}" data-sync-act="conflict-${a}" data-id="${escapeAttr(row.id)}">${escapeHtml(t(BUTTON_KEYS[a]))}</button>`
            )
            .join('');
        const detail = this.details.get(row.id);
        let compare = '';
        if (detail) {
            const fields = (detail.fields || [])
                .map(
                    (f) => `<tr><th scope="row" dir="ltr">${escapeHtml(f.field)}</th>
                        <td dir="auto">${escapeHtml(f.now ?? '')}</td>
                        <td dir="auto">${escapeHtml(f.replaced ?? '')}</td></tr>`
                )
                .join('');
            compare = `<table class="sync-compare">
                <thead><tr><th scope="col">${escapeHtml(t('sync.conflicts.compare.field'))}</th>
                <th scope="col">${escapeHtml(t('sync.conflicts.compare.now'))}</th>
                <th scope="col">${escapeHtml(t('sync.conflicts.compare.replaced'))}</th></tr></thead>
                <tbody>${fields}</tbody></table>`;
        }
        return `<li class="sync-conflict" data-conflict="${escapeAttr(row.id)}">
            ${v.title ? `<div class="sync-conflict-title">${v.title}</div>` : ''}
            ${v.lines.map((l) => `<div class="sync-conflict-line">${l}</div>`).join('')}
            ${buttons ? `<div class="sync-actions">${buttons}</div>` : ''}
            ${compare}
        </li>`;
    }

    _section(titleKey, rows, now) {
        if (rows.length === 0) return '';
        return `<h4 class="sync-subtitle">${escapeHtml(t(titleKey))}</h4>
            <ul class="sync-conflict-list">${rows.map((r) => this._entry(r, now)).join('')}</ul>`;
    }

    render(now = Date.now()) {
        const tabs = TABS.map(
            (tab) =>
                `<button type="button" role="tab" class="sync-tab${tab === this.tab ? ' active' : ''}" aria-selected="${tab === this.tab}" data-sync-act="conflict-tab" data-tab="${tab}">${escapeHtml(t(`sync.conflicts.tab.${tab}`))}</button>`
        ).join('');
        const g = groupConflicts(this.rows, now);
        const groups = g.groups
            .map((grp) => {
                const open = this.expanded.has(grp.group);
                const undo =
                    grp.kind === 'replaced_on_join' && this.tab === 'replaced'
                        ? `<button type="button" class="setting-button" data-sync-act="undo-join">${escapeHtml(t('settings.sync.undo_join_button'))}</button>`
                        : '';
                const keepAll =
                    this.tab === 'replaced'
                        ? `<button type="button" class="setting-button" data-sync-act="conflict-keep-all" data-group="${escapeAttr(grp.group)}">${escapeHtml(t('sync.conflicts.keep_all'))}</button>`
                        : '';
                return `<li class="sync-conflict sync-conflict-group">
                    <div class="sync-conflict-title">${groupLine(grp)}</div>
                    <div class="sync-actions">
                        <button type="button" class="setting-button" aria-expanded="${open}" data-sync-act="conflict-expand" data-group="${escapeAttr(grp.group)}">${escapeHtml(t(open ? 'sync.conflicts.hide' : 'settings.sync.review'))}</button>
                        ${keepAll}${undo}
                    </div>
                    ${open ? `<ul class="sync-conflict-list">${grp.rows.map((r) => this._entry(r, now)).join('')}</ul>` : ''}
                </li>`;
            })
            .join('');
        const generated =
            g.generated.length > 0
                ? `<h4 class="sync-subtitle">${escapeHtml(t('sync.conflicts.generated'))}</h4>
                <button type="button" class="setting-button" aria-expanded="${this.showGenerated}" data-sync-act="conflict-generated">${escapeHtml(
                    t(this.showGenerated ? 'sync.conflicts.hide' : 'sync.conflicts.show_versions', {
                        count: g.generated.length,
                    })
                )}</button>
                ${this.showGenerated ? `<ul class="sync-conflict-list">${g.generated.map((r) => this._entry(r, now)).join('')}</ul>` : ''}`
                : '';
        const empty =
            this.loaded && this.rows.length === 0 && !this.error
                ? `<p class="setting-description">${escapeHtml(t(`sync.conflicts.empty.${this.tab}`))}</p>`
                : '';
        const dismissAll =
            this.tab === 'replaced' && this.rows.length > 0
                ? `<button type="button" class="setting-button" data-sync-act="conflict-dismiss-all">${escapeHtml(t('sync.conflicts.dismiss_all'))}</button>`
                : '';
        return `<div class="sync-conflicts">
            <div class="sync-view-head">
                <button type="button" class="setting-button" data-sync-act="back">${escapeHtml(t('settings.sync.back'))}</button>
                <h3 class="sync-step-title" tabindex="-1">${escapeHtml(t('sync.conflicts.title'))}</h3>
                ${dismissAll}
            </div>
            <p class="setting-description">${escapeHtml(t('sync.conflicts.intro'))}</p>
            <div class="sync-tabs" role="tablist" aria-label="${escapeAttr(t('sync.conflicts.title'))}">${tabs}</div>
            ${this.error ? `<div class="sync-error" role="alert">${escapeHtml(this.error)}</div>` : ''}
            ${empty}
            ${this._section('sync.conflicts.today', g.today, now)}
            ${this._section('sync.conflicts.earlier', g.earlier, now)}
            ${groups ? `<h4 class="sync-subtitle">${escapeHtml(t('sync.conflicts.earlier_actions'))}</h4><ul class="sync-conflict-list">${groups}</ul>` : ''}
            ${generated}
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
        this.deps.onChanged?.();
    }

    /** Run one conflicts action; false when it isn't one. */
    async handle(act, el) {
        const invoke = this.deps.invoke;
        const id = el?.dataset.id;
        switch (act) {
            case 'conflict-tab':
                this.tab = TABS.includes(el?.dataset.tab) ? el.dataset.tab : 'replaced';
                this.details.clear();
                await this.load();
                return true;
            case 'conflict-expand': {
                const g = el?.dataset.group;
                if (this.expanded.has(g)) this.expanded.delete(g);
                else this.expanded.add(g);
                return true;
            }
            case 'conflict-generated':
                this.showGenerated = !this.showGenerated;
                return true;
            case 'conflict-compare':
                if (this.details.has(id)) {
                    this.details.delete(id);
                    return true;
                }
                try {
                    this.details.set(id, await invoke('sync_conflict_detail', { id }));
                } catch (e) {
                    this.error = errMessage(e);
                }
                return true;
            case 'conflict-restore':
                await this._do(() => invoke('sync_resolve_conflict', { id, action: 'restore' }));
                return true;
            case 'conflict-dismiss':
                await this._do(() => invoke('sync_resolve_conflict', { id, action: 'dismiss' }));
                return true;
            case 'conflict-delete_anyway':
                await this._do(() =>
                    invoke('sync_resolve_conflict', { id, action: 'delete_anyway' })
                );
                return true;
            case 'conflict-keep-all':
                await this._do(() => invoke('sync_dismiss_all', { group: el?.dataset.group }));
                return true;
            case 'conflict-dismiss-all':
                await this._do(() => invoke('sync_dismiss_all', { group: null }));
                return true;
            default:
                return false;
        }
    }
}
