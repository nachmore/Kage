/**
 * Settings → Sync → Your computers (docs/design/SYNC.md §13.3): every
 * computer in the sync folder, when it last wrote, and Remove from sync
 * for another one (with a new key and passphrase when it was lost or
 * stolen). This computer can be renamed.
 */

import { t, tHtml } from '../shared/i18n.js';
import { passphraseError } from './sync-setup.js';
import { escapeAttr, escapeHtml, isolate, osName, relativeTime, userText } from './sync-format.js';

/** The state text of one computer. */
export function deviceStateText(d, now = Date.now()) {
    if (d.this) return t('settings.sync.devices.this');
    const time = relativeTime(d.last_seen_at, now);
    switch (d.state) {
        case 'inactive':
            return t('settings.sync.devices.inactive', { time });
        case 'needs_update':
            return t('settings.sync.devices.needs_update');
        case 'retired':
            return t('settings.sync.devices.retired');
        default:
            return t('settings.sync.devices.synced', { time });
    }
}

/** The list of computers, this one first. */
export function devicesHtml(devices, now = Date.now(), { removing = null } = {}) {
    const list = [...(Array.isArray(devices) ? devices : [])].sort(
        (a, b) => Number(!!b.this) - Number(!!a.this)
    );
    return `<ul class="sync-devices">${list
        .map((d) => {
            const action = d.this
                ? `<button type="button" class="setting-button" data-sync-act="rename-start">${escapeHtml(t('settings.sync.rename'))}</button>`
                : d.state === 'retired'
                  ? ''
                  : `<button type="button" class="setting-button" data-sync-act="device-remove-start" data-id="${escapeAttr(d.id)}" aria-label="${escapeAttr(t('settings.sync.devices.remove_label', { name: isolate(d.name) }))}">${escapeHtml(t('settings.sync.devices.remove'))}</button>`;
            const form = removing === d.id ? removeFormHtml(d) : '';
            return `<li class="sync-device sync-device--${escapeAttr(d.state || 'ok')}">
                <span class="sync-device-name">${userText(d.name)}</span>
                <span class="sync-device-os">${escapeHtml(osName(d.os))}</span>
                <span class="sync-device-state">${escapeHtml(deviceStateText(d, now))}</span>
                ${action}
                ${form}
            </li>`;
        })
        .join('')}</ul>`;
}

function removeFormHtml(d) {
    return `<div class="sync-inline-form" role="group" aria-label="${escapeAttr(t('settings.sync.devices.remove_label', { name: isolate(d.name) }))}">
        <p>${tHtml('settings.sync.devices.remove_confirm', { name: isolate(d.name) })}</p>
        <label class="sync-check"><input type="checkbox" id="syncRemoveLost" data-sync-act="device-lost-toggle">
            <span>${escapeHtml(t('settings.sync.devices.lost'))}</span></label>
        <div class="sync-lost-fields" hidden>
            <label class="sync-field"><span>${escapeHtml(t('settings.sync.passphrase.new'))}</span>
                <input type="password" class="setting-input" id="syncRemovePass" dir="ltr" autocomplete="new-password"></label>
            <label class="sync-field"><span>${escapeHtml(t('settings.sync.setup.passphrase.confirm'))}</span>
                <input type="password" class="setting-input" id="syncRemovePass2" dir="ltr" autocomplete="new-password"></label>
        </div>
        <div class="sync-actions">
            <button type="button" class="setting-button" data-sync-act="device-remove-cancel">${escapeHtml(t('settings.sync.setup.cancel'))}</button>
            <button type="button" class="setting-button danger" data-sync-act="device-remove-confirm" data-id="${escapeAttr(d.id)}">${escapeHtml(t('settings.sync.devices.remove'))}</button>
        </div>
    </div>`;
}

/**
 * Remove another computer: a one-use token from
 * `sync_prepare_destructive`, then `sync_remove_device`.
 */
export async function removeDevice(invoke, root, deviceId) {
    const lost = !!root.querySelector('#syncRemoveLost')?.checked;
    let newPassphrase = null;
    if (lost) {
        const p = root.querySelector('#syncRemovePass')?.value || '';
        const p2 = root.querySelector('#syncRemovePass2')?.value || '';
        const err = passphraseError(p, p2);
        if (err) throw new Error(err);
        newPassphrase = p;
    }
    const confirm = await invoke('sync_prepare_destructive', { action: 'remove_device' });
    await invoke('sync_remove_device', { deviceId, lost, newPassphrase, confirm });
}
