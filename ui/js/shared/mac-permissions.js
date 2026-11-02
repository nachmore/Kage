/**
 * macOS TCC permission UX helpers: one card per privacy permission, with a
 * live status pill, a deep link into System Settings, and a "Repair"
 * action for a grant macOS holds for an older build of Kage.
 *
 * Kage uses two macOS privacy permissions (granted in System Settings →
 * Privacy & Security; macOS also prompts the first time they're needed):
 *
 *   - Accessibility    — synthetic Cmd+C / Cmd+V (selection capture and
 *                        paste-back), raising a specific window from the
 *                        window list, and Computer Control's AX tree +
 *                        input synthesis
 *   - Screen Recording — window titles from CGWindowList (active-window
 *                        context, activity tracking, open-windows list)
 *
 * Input Monitoring is deliberately absent: global hotkeys use
 * tauri-plugin-global-shortcut (Carbon RegisterEventHotKey), which needs
 * no permission, and hotkey recording doesn't rely on it either, so
 * nothing Kage ships needs it.
 *
 * Status comes from `get_privacy_permissions` (non-prompting preflight
 * checks — see src/os/permissions.rs). "Repair" calls
 * `repair_privacy_permission`, which runs `tccutil reset` for Kage's own
 * bundle id and re-requests access so the *running* binary is listed;
 * this module then opens the matching pane so the user can switch it on.
 * That fixes the stale-grant case: Kage's macOS builds are ad-hoc signed,
 * so TCC ties each grant to one exact build and every update leaves
 * System Settings showing Kage enabled while the new binary isn't.
 *
 * The deep-link URLs use the macOS 13 (Ventura) `com.apple.settings.*`
 * scheme, which opens the correct pane in the new System Settings app. On
 * older macOS these URLs still resolve to a sensible fallback (the
 * top-level Privacy pane). See
 * https://gist.github.com/dvessel/2b6ad97b2da16d445671b39618221aab for the
 * exhaustive URL list.
 *
 * ES module — import via `import { renderAllInto, isMacOS } from './mac-permissions.js'`.
 */

import { escapeAttr, escapeHtml } from './tool-utils.js';
import { errMessage } from './error-message.js';
import { t } from './i18n.js';

export function isMacOS() {
    return (navigator.platform || '').startsWith('Mac');
}

// `name` / `why` are getters so they resolve through t() at render time
// (the catalog loads after this module is imported).
export const MAC_PERMISSIONS = Object.freeze([
    Object.freeze({
        id: 'accessibility',
        icon: '♿',
        get name() {
            return t('shared.mac_permissions.accessibility.name');
        },
        get why() {
            return t('shared.mac_permissions.accessibility.why');
        },
        url: 'x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility',
    }),
    Object.freeze({
        id: 'screen-recording',
        icon: '🖥️',
        get name() {
            return t('shared.mac_permissions.screen_recording.name');
        },
        get why() {
            return t('shared.mac_permissions.screen_recording.why');
        },
        url: 'x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ScreenCapture',
    }),
]);

/**
 * Card states. `granted` / `not_granted` mirror the Rust
 * `PermissionStatus`; `checking` is the pre-response placeholder and
 * `unknown` covers a failed check or any other value (e.g. the
 * `not_applicable` the stub returns off macOS).
 */
export const PERMISSION_STATUS = Object.freeze({
    CHECKING: 'checking',
    GRANTED: 'granted',
    NOT_GRANTED: 'not_granted',
    UNKNOWN: 'unknown',
});

/** Collapse anything that isn't a known card state to `unknown`. */
export function normalizeStatus(status) {
    switch (status) {
        case PERMISSION_STATUS.CHECKING:
        case PERMISSION_STATUS.GRANTED:
        case PERMISSION_STATUS.NOT_GRANTED:
            return status;
        default:
            return PERMISSION_STATUS.UNKNOWN;
    }
}

/** Localised pill text for a card state. */
export function statusLabel(status) {
    switch (normalizeStatus(status)) {
        case PERMISSION_STATUS.CHECKING:
            return t('shared.mac_permissions.status.checking');
        case PERMISSION_STATUS.GRANTED:
            return t('shared.mac_permissions.status.granted');
        case PERMISSION_STATUS.NOT_GRANTED:
            return t('shared.mac_permissions.status.not_granted');
        default:
            return t('shared.mac_permissions.status.unknown');
    }
}

/**
 * Render a single permission card. `idBase` prefixes the element ids
 * (the Open System Settings button is `${idBase}-btn`, Repair is
 * `${idBase}-repair`) so callers can find them after insertion.
 */
export function renderPermissionCard(perm, idBase, status = PERMISSION_STATUS.CHECKING) {
    const s = normalizeStatus(status);
    const id = escapeAttr(idBase);
    const name = perm.name;
    const needsRepair = s === PERMISSION_STATUS.NOT_GRANTED;
    const hidden = needsRepair ? '' : ' hidden';
    return `
        <div class="mac-perm-card" data-perm-id="${escapeAttr(perm.id)}" data-status="${s}" role="group" aria-labelledby="${id}-name">
            <div class="mac-perm-icon" aria-hidden="true">${escapeHtml(perm.icon)}</div>
            <div class="mac-perm-info">
                <div class="mac-perm-title">
                    <span class="mac-perm-name" id="${id}-name">${escapeHtml(name)}</span>
                    <span class="mac-perm-status" data-role="status" data-status="${s}" role="status" aria-live="polite"><span class="mac-perm-visually-hidden">${escapeHtml(name)}: </span><span data-role="status-label">${escapeHtml(statusLabel(s))}</span></span>
                </div>
                <div class="mac-perm-why">${escapeHtml(perm.why)}</div>
                <div class="mac-perm-repair-hint" id="${id}-hint" data-role="repair-hint"${hidden}>${escapeHtml(t('shared.mac_permissions.repair_hint'))}</div>
                <div class="mac-perm-message" data-role="message" role="status" aria-live="polite"></div>
            </div>
            <div class="mac-perm-actions">
                <button type="button" class="mac-perm-btn" id="${id}-btn" data-role="open" aria-label="${escapeAttr(t('shared.mac_permissions.open_settings_aria_label', { name }))}">
                    ${escapeHtml(t('shared.mac_permissions.open_settings'))}
                </button>
                <button type="button" class="mac-perm-btn mac-perm-btn-secondary" id="${id}-repair" data-role="repair" aria-describedby="${id}-hint" aria-label="${escapeAttr(t('shared.mac_permissions.repair_aria_label', { name }))}"${hidden}>${escapeHtml(t('shared.mac_permissions.repair'))}</button>
            </div>
        </div>`;
}

/**
 * Update a rendered card in place. The pill text is only rewritten when it
 * changes, so a focus refresh that finds nothing new doesn't make screen
 * readers re-announce every card.
 */
export function applyStatus(card, status) {
    const s = normalizeStatus(status);
    card.dataset.status = s;
    const pill = card.querySelector('[data-role="status"]');
    if (pill) pill.dataset.status = s;
    const label = card.querySelector('[data-role="status-label"]');
    const text = statusLabel(s);
    if (label && label.textContent !== text) label.textContent = text;
    const needsRepair = s === PERMISSION_STATUS.NOT_GRANTED;
    card.querySelector('[data-role="repair"]')?.toggleAttribute('hidden', !needsRepair);
    card.querySelector('[data-role="repair-hint"]')?.toggleAttribute('hidden', !needsRepair);
}

function setMessage(card, text, isError = false) {
    const el = card.querySelector('[data-role="message"]');
    if (!el) return;
    el.textContent = text;
    el.classList.toggle('error', !!(text && isError));
}

/**
 * Render every card into `container`, fetch live status, and wire the
 * buttons: "Open System Settings" deep-links via `open_url`; "Repair"
 * (shown only while a permission is not granted) resets Kage's grant,
 * then opens the pane. Status refreshes after each action and whenever
 * the window regains focus — typically on the way back from System
 * Settings.
 *
 * Returns `{ refresh, dispose }`: `refresh()` re-reads status (e.g. when
 * a settings section is re-shown); `dispose()` drops the focus listeners.
 * The listeners also drop themselves once `container` leaves the DOM.
 */
export function renderAllInto(container, invoke, idPrefix) {
    const prefix = idPrefix || 'macPerm';
    container.innerHTML = MAC_PERMISSIONS.map((p) =>
        renderPermissionCard(p, `${prefix}-${p.id}`)
    ).join('');

    const cards = new Map();
    let generation = 0;

    async function refresh() {
        const gen = ++generation;
        let statuses = null;
        try {
            const list = await invoke('get_privacy_permissions');
            statuses = new Map((Array.isArray(list) ? list : []).map((s) => [s?.id, s?.status]));
        } catch (err) {
            console.warn('[mac-permissions] status check failed:', errMessage(err));
        }
        // A newer refresh (focus + section show can overlap) owns the cards.
        if (gen !== generation) return;
        for (const [id, card] of cards) {
            applyStatus(card, statuses ? statuses.get(id) : PERMISSION_STATUS.UNKNOWN);
        }
    }

    const openSettings = (perm) =>
        invoke('open_url', { url: perm.url }).catch((err) => {
            console.error('Failed to open System Settings:', err);
        });

    MAC_PERMISSIONS.forEach((p) => {
        const card = container.querySelector(`.mac-perm-card[data-perm-id="${p.id}"]`);
        if (!card) return;
        cards.set(p.id, card);

        card.querySelector('[data-role="open"]')?.addEventListener('click', () => {
            openSettings(p);
        });

        const repairBtn = card.querySelector('[data-role="repair"]');
        repairBtn?.addEventListener('click', async () => {
            if (repairBtn.disabled) return;
            const label = repairBtn.textContent;
            repairBtn.disabled = true;
            repairBtn.setAttribute('aria-busy', 'true');
            repairBtn.textContent = t('shared.mac_permissions.repairing');
            setMessage(card, '');
            try {
                const result = await invoke('repair_privacy_permission', { permission: p.id });
                applyStatus(card, result?.status);
                setMessage(card, t('shared.mac_permissions.repair_done'));
                // Kage is listed again (switched off) — take the user there.
                await openSettings(p);
            } catch (err) {
                console.error(`[mac-permissions] repair of ${p.id} failed:`, errMessage(err));
                setMessage(
                    card,
                    t('shared.mac_permissions.repair_failed', { reason: errMessage(err) }),
                    true
                );
            } finally {
                repairBtn.disabled = false;
                repairBtn.removeAttribute('aria-busy');
                repairBtn.textContent = label;
                refresh();
            }
        });
    });

    const onFocus = () => {
        if (!container.isConnected) {
            dispose();
            return;
        }
        refresh();
    };
    const onVisibility = () => {
        if (document.visibilityState === 'visible') onFocus();
    };
    function dispose() {
        window.removeEventListener('focus', onFocus);
        document.removeEventListener('visibilitychange', onVisibility);
    }
    window.addEventListener('focus', onFocus);
    document.addEventListener('visibilitychange', onVisibility);

    refresh();
    return { refresh, dispose };
}
