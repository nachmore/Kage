/**
 * macOS TCC permission UX helpers.
 *
 * Kage relies on three macOS privacy permissions that don't live in Info.plist
 * (the system prompts at runtime and the user must grant manually in System
 * Settings → Privacy & Security):
 *
 *   - Accessibility   — needed by CGEvent posting (simulate_paste, hotkey
 *                       capture) and AX APIs (UI automation)
 *   - Input Monitoring — needed by the global hotkey CGEventTap
 *   - Screen Recording — needed to read window titles from CGWindowList
 *
 * The deep-link URLs use the macOS 13 (Ventura) `com.apple.settings.*` scheme,
 * which opens the correct pane in the new System Settings app. On older
 * macOS these URLs still resolve to a sensible fallback (the top-level
 * Privacy pane). See https://gist.github.com/dvessel/2b6ad97b2da16d445671b39618221aab
 * for the exhaustive URL list.
 *
 * ES module — import via `import { renderAllInto, isMacOS } from './mac-permissions.js'`.
 */

import { escapeAttr, escapeHtml } from './tool-utils.js';
import { t } from './i18n.js';

export function isMacOS() {
    return (navigator.platform || '').startsWith('Mac');
}

export const MAC_PERMISSIONS = Object.freeze([
    Object.freeze({
        id: 'accessibility',
        icon: '♿',
        name: 'Accessibility',
        why: 'Pastes captured text back into the active window and lets Kage automate UI elements on your behalf.',
        url: 'x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility',
    }),
    Object.freeze({
        id: 'input-monitoring',
        icon: '⌨️',
        name: 'Input Monitoring',
        why: 'Lets Kage listen for your global hotkey anywhere in the system.',
        url: 'x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ListenEvent',
    }),
    Object.freeze({
        id: 'screen-recording',
        icon: '🖥️',
        name: 'Screen Recording',
        why: 'Reads the title of the frontmost window so Kage knows what you are looking at. Kage never captures screen contents.',
        url: 'x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ScreenCapture',
    }),
]);

/**
 * Render a single permission card. `buttonId` is used so callers can
 * attach a click handler after insertion without a global dispatch.
 */
export function renderPermissionCard(perm, buttonId) {
    return `
        <div class="mac-perm-card" data-perm-id="${escapeAttr(perm.id)}">
            <div class="mac-perm-icon">${escapeHtml(perm.icon)}</div>
            <div class="mac-perm-info">
                <div class="mac-perm-name">${escapeHtml(perm.name)}</div>
                <div class="mac-perm-why">${escapeHtml(perm.why)}</div>
            </div>
            <button type="button" class="mac-perm-btn" id="${escapeAttr(buttonId)}">
                ${escapeHtml(t('shared.mac_permissions.open_settings'))}
            </button>
        </div>`;
}

/**
 * Render all three cards inside a container element. Wires "Open System
 * Settings" buttons to invoke the given open_url tauri command.
 */
export function renderAllInto(container, invoke, idPrefix) {
    const prefix = idPrefix || 'macPerm';
    const html = MAC_PERMISSIONS.map((p) => renderPermissionCard(p, `${prefix}-${p.id}-btn`)).join(
        ''
    );
    container.innerHTML = html;
    MAC_PERMISSIONS.forEach((p) => {
        const btn = container.querySelector(`#${prefix}-${p.id}-btn`);
        if (!btn) return;
        btn.addEventListener('click', () => {
            invoke('open_url', { url: p.url }).catch((err) => {
                console.error('Failed to open System Settings:', err);
            });
        });
    });
}
