/**
 * The sync chip in an extension's Settings header (docs/design/SYNC.md
 * §12.3): "Syncs settings and data", "Syncs settings", and "Sign-in stays
 * on this computer" when the extension has a sign-in or key. It sits right
 * after the capability badges (header order, §23.3).
 *
 * The header is built as one HTML string, so `renderSyncChip` returns an
 * empty placeholder and fills it once it knows whether sync is on. With
 * sync off it stays empty: nothing changes on the page.
 */

import { sensitiveConfigFields } from '../shared/extension-sensitive-data.js';
import { extensionDataSyncs, syncIsOn } from '../shared/sync-scope-chip.js';
import { escapeAttr, escapeHtml } from '../shared/tool-utils.js';

/** One status read serves every extension page of a render. */
let statusPromise = null;
let statusAt = 0;
const STATUS_TTL_MS = 2000;

function readStatus(invoke) {
    const now = Date.now();
    if (!statusPromise || now - statusAt > STATUS_TTL_MS) {
        statusAt = now;
        statusPromise = Promise.resolve()
            .then(() => invoke('sync_get_status'))
            .catch(() => null);
    }
    return statusPromise;
}

/** Forget the cached status (tests). */
export function resetSyncChipCache() {
    statusPromise = null;
    statusAt = 0;
}

/**
 * Which chips an extension's header shows, from the sync status, its
 * manifest and the capabilities this computer granted. Pure.
 *
 * @returns {string[]} `settings.manager.ext.sync.*` suffixes, in order
 */
export function syncChipKinds({ status, manifest, capabilities, extensionId }) {
    if (!syncIsOn(status)) return [];
    if (status.categories && status.categories.extensions === false) return [];
    const granted = Array.isArray(capabilities) ? capabilities : [];
    const id = extensionId || manifest?.id || '';
    const out = [
        extensionDataSyncs(status, { granted }, id) ? 'settings_and_data' : 'settings_only',
    ];
    let sensitive = [];
    try {
        sensitive = sensitiveConfigFields(manifest || {});
    } catch {
        sensitive = [];
    }
    if (sensitive.length > 0) out.push('sign_in_local');
    return out;
}

/** The chips as HTML. */
export function syncChipHtml(kinds, t) {
    return kinds
        .map(
            (k) =>
                `<span class="sync-ext-chip sync-ext-chip--${escapeAttr(k)}">${escapeHtml(
                    t(`settings.manager.ext.sync.${k}`)
                )}</span>`
        )
        .join('');
}

/**
 * HTML for the extension page header: a placeholder the chips fill in.
 *
 * @param {string} extensionId
 * @param {{ invoke: Function, t: Function, manifest?: object,
 *   capabilities?: string[] }} deps
 * @returns {string}
 */
export function renderSyncChip(extensionId, { invoke, t, manifest, capabilities } = {}) {
    if (typeof invoke !== 'function' || !extensionId) return '';
    readStatus(invoke).then((status) => {
        const kinds = syncChipKinds({ status, manifest, capabilities, extensionId });
        if (kinds.length === 0) return;
        for (const el of document.querySelectorAll('[data-sync-ext-chip]')) {
            if (el.dataset.syncExtChip !== extensionId) continue;
            el.innerHTML = syncChipHtml(kinds, t);
            el.hidden = false;
        }
    });
    return `<div class="sync-ext-chips" data-sync-ext-chip="${escapeAttr(extensionId)}" data-sync-ui hidden></div>`;
}
