/**
 * In-app confirm / alert / message dialogs that match Kage's UX.
 *
 * Why this exists rather than `window.confirm()` / `window.alert()`:
 *   1. The browser primitives play a system bell + look alien against
 *      Kage's chrome. They also block the entire renderer in
 *      WebView2's modal mode, which has caused subtle bugs in the
 *      past — most recently a report that pressing Cancel on the
 *      uninstall confirm let the uninstall proceed anyway, which we
 *      attribute to the dialog primitive's behaviour drifting
 *      between platforms / runtimes (Tauri 2.x WebView2 vs WKWebView
 *      etc.) and the surrounding async handler racing with the
 *      modal close.
 *   2. We get themed visuals (light/dark/branded), keyboard handling
 *      (Esc cancels, Enter confirms), backdrop click, and Promise
 *      semantics that callers can `await` reliably.
 *
 * Patterned on `permission-prompt.js`'s modal — same backdrop class
 * structure but a dedicated stylesheet block so the two can evolve
 * independently if the install prompt ever gains extra chrome.
 *
 * Public API:
 *
 *   confirmDialog({
 *     title: 'Uninstall calendar?',
 *     message: 'This deletes the extension and its grants. You can always reinstall it later.',
 *     confirmLabel: 'Uninstall',     // optional, default: 'Confirm'
 *     cancelLabel:  'Keep it',       // optional, default: 'Cancel'
 *     destructive:  true,            // optional, styles the confirm button red
 *     icon:         '🗑️',           // optional, single emoji shown left of the title
 *   }) → Promise<boolean>
 *
 *   alertDialog({ title?, message, okLabel?: 'OK', icon?: '⚠️' }) → Promise<void>
 *
 * Both are no-ops returning a resolved Promise if `document.body`
 * isn't present (e.g. during very early bootstrap), to keep callers
 * from blocking on a never-resolving handler.
 */

const STYLE_ID = 'kage-confirm-dialog-style';
const MODAL_ID_BASE = 'kage-confirm-dialog';

let _modalCounter = 0;

function escapeHtml(s) {
    return String(s ?? '').replace(
        /[&<>"']/g,
        (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]
    );
}

ensureStyles();

/**
 * Show a confirm dialog. Resolves to `true` on the affirmative
 * button, `false` on cancel / Escape / backdrop click.
 *
 * Multiple dialogs CAN stack (each gets a unique id) — useful for
 * "are you sure?"-on-top-of-an-error patterns — but consider whether
 * a single composite dialog reads better.
 *
 * @param {object} opts
 * @returns {Promise<boolean>}
 */
export function confirmDialog(opts = {}) {
    if (!document.body) return Promise.resolve(false);
    const {
        title = '',
        message = '',
        confirmLabel = 'Confirm',
        cancelLabel = 'Cancel',
        destructive = false,
        icon = '',
    } = opts;

    return new Promise((resolve) => {
        const modalId = `${MODAL_ID_BASE}-${++_modalCounter}`;
        const backdrop = document.createElement('div');
        backdrop.id = modalId;
        backdrop.className = 'kage-confirm-backdrop';
        backdrop.innerHTML = `
            <div class="kage-confirm-modal" role="dialog" aria-modal="true">
                <div class="kage-confirm-header">
                    ${icon ? `<div class="kage-confirm-icon">${escapeHtml(icon)}</div>` : ''}
                    <div class="kage-confirm-heading">
                        ${title ? `<div class="kage-confirm-title">${escapeHtml(title)}</div>` : ''}
                        ${message ? `<div class="kage-confirm-message">${escapeHtml(message)}</div>` : ''}
                    </div>
                </div>
                <div class="kage-confirm-footer">
                    <button class="kage-confirm-btn" data-action="cancel">${escapeHtml(cancelLabel)}</button>
                    <button class="kage-confirm-btn ${destructive ? 'kage-confirm-btn-danger' : 'kage-confirm-btn-primary'}" data-action="confirm">${escapeHtml(confirmLabel)}</button>
                </div>
            </div>
        `;
        document.body.appendChild(backdrop);

        let settled = false;
        const cleanup = (result) => {
            if (settled) return;
            settled = true;
            try {
                backdrop.remove();
            } catch {}
            document.removeEventListener('keydown', onKey, true);
            resolve(result);
        };

        const onKey = (e) => {
            if (e.key === 'Escape') {
                e.stopPropagation();
                cleanup(false);
            } else if (e.key === 'Enter') {
                // Enter confirms unless the user is in a focused
                // form control whose own Enter handler should win.
                const tag = (document.activeElement?.tagName || '').toLowerCase();
                if (tag === 'textarea' || tag === 'input' || tag === 'select') return;
                e.stopPropagation();
                cleanup(true);
            }
        };
        // Capture phase — we want to intercept Esc/Enter before any
        // background handlers (e.g. the chat input listening for
        // Enter to submit). Without capture, opening this from the
        // chat window leaked Enter clicks through to the chat send
        // button.
        document.addEventListener('keydown', onKey, true);

        backdrop.addEventListener('click', (e) => {
            if (e.target === backdrop) cleanup(false);
        });
        backdrop
            .querySelector('[data-action="cancel"]')
            .addEventListener('click', () => cleanup(false));
        backdrop
            .querySelector('[data-action="confirm"]')
            .addEventListener('click', () => cleanup(true));

        // Focus the cancel button by default for destructive actions
        // (so accidental Space/Enter doesn't blow away data) and the
        // confirm button otherwise. The 30ms tick mirrors the
        // permission-prompt pattern — gives the modal a beat to land
        // on screen before we steal focus.
        setTimeout(() => {
            const safe = destructive ? 'cancel' : 'confirm';
            backdrop.querySelector(`[data-action="${safe}"]`)?.focus();
        }, 30);
    });
}

/**
 * Show a one-button alert / message dialog. Resolves when the user
 * dismisses (OK / Esc / backdrop click). Use this in place of
 * `window.alert()` so every surface stays themed.
 *
 * @param {object} opts
 * @returns {Promise<void>}
 */
export function alertDialog(opts = {}) {
    if (!document.body) return Promise.resolve();
    const { title = '', message = '', okLabel = 'OK', icon = '' } = opts;

    return new Promise((resolve) => {
        const modalId = `${MODAL_ID_BASE}-${++_modalCounter}`;
        const backdrop = document.createElement('div');
        backdrop.id = modalId;
        backdrop.className = 'kage-confirm-backdrop';
        backdrop.innerHTML = `
            <div class="kage-confirm-modal" role="alertdialog" aria-modal="true">
                <div class="kage-confirm-header">
                    ${icon ? `<div class="kage-confirm-icon">${escapeHtml(icon)}</div>` : ''}
                    <div class="kage-confirm-heading">
                        ${title ? `<div class="kage-confirm-title">${escapeHtml(title)}</div>` : ''}
                        ${message ? `<div class="kage-confirm-message">${escapeHtml(message)}</div>` : ''}
                    </div>
                </div>
                <div class="kage-confirm-footer">
                    <button class="kage-confirm-btn kage-confirm-btn-primary" data-action="ok">${escapeHtml(okLabel)}</button>
                </div>
            </div>
        `;
        document.body.appendChild(backdrop);

        let settled = false;
        const cleanup = () => {
            if (settled) return;
            settled = true;
            try {
                backdrop.remove();
            } catch {}
            document.removeEventListener('keydown', onKey, true);
            resolve();
        };

        const onKey = (e) => {
            if (e.key === 'Escape' || e.key === 'Enter') {
                e.stopPropagation();
                cleanup();
            }
        };
        document.addEventListener('keydown', onKey, true);

        backdrop.addEventListener('click', (e) => {
            if (e.target === backdrop) cleanup();
        });
        backdrop.querySelector('[data-action="ok"]').addEventListener('click', cleanup);

        setTimeout(() => {
            backdrop.querySelector('[data-action="ok"]')?.focus();
        }, 30);
    });
}

function ensureStyles() {
    if (typeof document === 'undefined') return; // SSR / tests
    if (document.getElementById(STYLE_ID)) return;
    const style = document.createElement('style');
    style.id = STYLE_ID;
    style.textContent = `
.kage-confirm-backdrop {
    position: fixed; inset: 0; z-index: 99998;
    background: rgba(0,0,0,0.55);
    display: flex; align-items: center; justify-content: center;
    font-family: inherit;
}
.kage-confirm-modal {
    width: min(92vw, 440px);
    background: var(--kage-bg, #1f1b24);
    color: var(--kage-text-primary, #E5E7EB);
    border: 1px solid var(--kage-border, #3a3640);
    border-radius: 10px;
    box-shadow: 0 16px 40px rgba(0,0,0,0.5);
    overflow: hidden;
}
.kage-confirm-header {
    display: flex; gap: 14px; padding: 18px 18px 14px;
    align-items: flex-start;
}
.kage-confirm-icon {
    font-size: 28px; line-height: 1;
    flex-shrink: 0;
}
.kage-confirm-heading { flex: 1; min-width: 0; }
.kage-confirm-title { font-size: 16px; font-weight: 600; margin-bottom: 6px; }
.kage-confirm-message {
    font-size: 13px; color: var(--kage-text-muted, #938F9B);
    line-height: 1.5;
    /* Multi-line confirm messages — preserve user-supplied newlines
       and let very long words break rather than overflow the modal. */
    white-space: pre-wrap;
    word-break: break-word;
}
.kage-confirm-footer {
    display: flex; justify-content: flex-end; gap: 10px;
    padding: 12px 18px;
    border-top: 1px solid var(--kage-border, #3a3640);
    background: rgba(0,0,0,0.12);
}
.kage-confirm-btn {
    font-size: 13px; padding: 8px 16px;
    background: var(--kage-surface-hover, #352F3D);
    color: var(--kage-text-primary, #E5E7EB);
    border: 1px solid var(--kage-border, #3a3640);
    border-radius: 6px; cursor: pointer;
    font-family: inherit;
    min-width: 84px;
}
.kage-confirm-btn:hover { background: var(--kage-surface, #2A2530); }
.kage-confirm-btn:focus-visible {
    outline: 2px solid var(--kage-accent, #8B5CF6);
    outline-offset: 1px;
}
.kage-confirm-btn-primary {
    background: var(--kage-accent, #8B5CF6);
    border-color: var(--kage-accent, #8B5CF6);
    color: #fff;
}
.kage-confirm-btn-primary:hover { filter: brightness(1.1); }
.kage-confirm-btn-danger {
    background: #c0392b;
    border-color: #c0392b;
    color: #fff;
}
.kage-confirm-btn-danger:hover { filter: brightness(1.08); }

body.light-theme .kage-confirm-modal {
    background: #ffffff;
    color: #1a1a1a;
    border-color: #ddd;
}
body.light-theme .kage-confirm-message { color: #555; }
body.light-theme .kage-confirm-btn {
    background: #ffffff; border-color: #ccc; color: #1a1a1a;
}
body.light-theme .kage-confirm-btn:hover { background: #f0f0f0; }
body.light-theme .kage-confirm-footer { background: #fafafa; border-top-color: #ddd; }
    `;
    document.head.appendChild(style);
}
