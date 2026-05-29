/**
 * Install-time permission prompt.
 *
 * Given a manifest, shows a modal that lists the capabilities the extension
 * is asking for, each with icon + label + description. The user approves
 * the whole set or cancels. On approval, the caller is responsible for
 * recording the grant (`save_extension_grant`) and then actually installing.
 *
 * This modal is deliberately all-or-nothing for now: users can see exactly
 * what the extension wants, and if they don't trust one capability they
 * don't install the extension. Fine-grained per-capability opt-in is
 * future work.
 */

import { CAPABILITIES, normalizePermissions } from './extension-permissions.js';
import { t, tHtml } from './i18n.js';

const MODAL_ID = 'kage-extension-permission-modal';

/**
 * Result returned by showPermissionPrompt.
 * @typedef {object} PermissionPromptResult
 * @property {boolean} approved
 * @property {string[]} granted - the list the caller should record
 */

/**
 * @param {object} manifest - the extension manifest (needs id, name, icon, permissions)
 * @param {object} [opts]
 * @param {boolean} [opts.isUpgrade] - true if this is a version upgrade requesting more caps
 * @param {string[]} [opts.previouslyGranted] - caps the user already approved (shown differently)
 * @returns {Promise<PermissionPromptResult>}
 */
export function showPermissionPrompt(manifest, opts = {}) {
    return new Promise((resolve) => {
        const requested = normalizePermissions(manifest?.permissions, manifest?.id || '<unknown>');
        const previouslyGranted = new Set(opts.previouslyGranted || []);

        // Build modal
        const backdrop = document.createElement('div');
        backdrop.id = MODAL_ID;
        backdrop.className = 'kage-ext-perm-backdrop';
        backdrop.innerHTML = renderModal(manifest, requested, previouslyGranted, !!opts.isUpgrade);
        document.body.appendChild(backdrop);

        const cleanup = (result) => {
            try {
                backdrop.remove();
            } catch {}
            document.removeEventListener('keydown', onKey);
            resolve(result);
        };

        const onKey = (e) => {
            if (e.key === 'Escape') cleanup({ approved: false, granted: [] });
        };
        document.addEventListener('keydown', onKey);

        backdrop.addEventListener('click', (e) => {
            if (e.target === backdrop) cleanup({ approved: false, granted: [] });
        });

        backdrop
            .querySelector('[data-action="cancel"]')
            .addEventListener('click', () => cleanup({ approved: false, granted: [] }));

        backdrop
            .querySelector('[data-action="approve"]')
            .addEventListener('click', () => cleanup({ approved: true, granted: requested }));

        // Focus the approve button for keyboard users — but only after a tick
        // so the modal animation has settled.
        setTimeout(() => {
            backdrop.querySelector('[data-action="approve"]')?.focus();
        }, 30);
    });
}

function escape(s) {
    return String(s).replace(
        /[&<>"']/g,
        (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]
    );
}

function renderModal(manifest, requested, previouslyGranted, isUpgrade) {
    // Raw values go to the t()/tHtml() callsites; pre-escaped values get
    // dropped directly into HTML. Mixing would either render `&amp;`
    // literally or skip an escape that should have happened.
    const idRaw = manifest?.id || 'unknown';
    const nameRaw = manifest?.name || idRaw;
    const id = escape(idRaw);
    const name = escape(nameRaw);
    const version = escape(manifest?.version || '');
    const icon = escape(manifest?.icon || '🧩');
    const author = manifest?.author
        ? tHtml('ext_perm.author_html', { author: manifest.author })
        : '';
    const description = manifest?.description
        ? `<div class="kage-ext-perm-description">${escape(manifest.description)}</div>`
        : '';

    let permsSection;
    if (requested.length === 0) {
        permsSection = `
            <div class="kage-ext-perm-empty">${t('ext_perm.empty')}</div>`;
    } else {
        const rows = requested.map((cap) => renderCapRow(cap, previouslyGranted.has(cap))).join('');
        permsSection = `
            <div class="kage-ext-perm-list">${rows}</div>
            <div class="kage-ext-perm-footnote">${t('ext_perm.footnote')}</div>`;
    }

    // tHtml escapes nameRaw, so the title lands HTML-safe even if a
    // manifest.name contains `<` or `&`.
    const titleHtml = isUpgrade
        ? tHtml('ext_perm.title.upgrade', { name: nameRaw })
        : tHtml('ext_perm.title.install', { name: nameRaw });
    const subtitle = isUpgrade ? t('ext_perm.subtitle.upgrade') : t('ext_perm.subtitle.install');

    return `
        <div class="kage-ext-perm-modal" role="dialog" aria-modal="true" aria-labelledby="kage-ext-perm-title">
            <div class="kage-ext-perm-header">
                <div class="kage-ext-perm-icon">${icon}</div>
                <div class="kage-ext-perm-heading">
                    <div id="kage-ext-perm-title" class="kage-ext-perm-title">${titleHtml}</div>
                    <div class="kage-ext-perm-subtitle">${subtitle}</div>
                </div>
            </div>
            <div class="kage-ext-perm-body">
                <div class="kage-ext-perm-meta">
                    <strong>${name}</strong>
                    ${version ? `<span class="kage-ext-perm-version">v${version}</span>` : ''}
                    ${author}
                </div>
                ${description}
                ${permsSection}
            </div>
            <div class="kage-ext-perm-footer">
                <button class="kage-ext-perm-btn" data-action="cancel">${t('ext_perm.cancel_btn')}</button>
                <button class="kage-ext-perm-btn kage-ext-perm-btn-primary" data-action="approve">
                    ${requested.length === 0 ? t('ext_perm.install_btn') : t('ext_perm.approve_btn')}
                </button>
            </div>
        </div>
    `;
}

function renderCapRow(cap, previouslyGranted) {
    const meta = CAPABILITIES[cap];
    if (!meta) return ''; // unknown cap — already filtered by normalizePermissions
    const badge = previouslyGranted
        ? `<span class="kage-ext-perm-badge kage-ext-perm-badge-existing">${t('ext_perm.badge.existing')}</span>`
        : `<span class="kage-ext-perm-badge kage-ext-perm-badge-new">${t('ext_perm.badge.new')}</span>`;
    return `
        <div class="kage-ext-perm-row">
            <div class="kage-ext-perm-cap-icon">${escape(meta.icon)}</div>
            <div class="kage-ext-perm-cap-body">
                <div class="kage-ext-perm-cap-label">
                    ${escape(meta.label)} ${badge}
                </div>
                <div class="kage-ext-perm-cap-desc">${escape(meta.description)}</div>
            </div>
        </div>
    `;
}

// --- CSS (injected once per document) --------------------------------------

const STYLE_ID = 'kage-ext-perm-style';
(function ensureStyles() {
    if (document.getElementById(STYLE_ID)) return;
    const style = document.createElement('style');
    style.id = STYLE_ID;
    style.textContent = `
.kage-ext-perm-backdrop {
    position: fixed; inset: 0; z-index: 99999;
    background: rgba(0,0,0,0.55);
    display: flex; align-items: center; justify-content: center;
    font-family: inherit;
}
.kage-ext-perm-modal {
    width: min(92vw, 520px);
    max-height: 85vh;
    overflow: hidden;
    display: flex; flex-direction: column;
    background: var(--kage-bg, #1f1b24);
    color: var(--kage-text-primary, #E5E7EB);
    border: 1px solid var(--kage-border, #3a3640);
    border-radius: 10px;
    box-shadow: 0 16px 40px rgba(0,0,0,0.5);
}
.kage-ext-perm-header {
    display: flex; gap: 14px; padding: 18px 18px 8px;
    align-items: flex-start;
}
.kage-ext-perm-icon {
    font-size: 28px; line-height: 1;
    background: var(--kage-surface, #2A2530);
    border: 1px solid var(--kage-border, #3a3640);
    border-radius: 8px;
    width: 48px; height: 48px;
    display: flex; align-items: center; justify-content: center;
    flex-shrink: 0;
}
.kage-ext-perm-heading { flex: 1; min-width: 0; }
.kage-ext-perm-title { font-size: 16px; font-weight: 600; margin-bottom: 4px; }
.kage-ext-perm-subtitle { font-size: 12px; color: var(--kage-text-muted, #938F9B); line-height: 1.4; }
.kage-ext-perm-body { padding: 6px 18px 18px; overflow-y: auto; }
.kage-ext-perm-meta {
    font-size: 13px; display: flex; align-items: baseline; gap: 8px;
    margin: 6px 0 8px; color: var(--kage-text-primary);
    flex-wrap: wrap;
}
.kage-ext-perm-version { color: var(--kage-text-muted); font-size: 12px; }
.kage-ext-perm-author { color: var(--kage-text-muted); font-size: 12px; }
.kage-ext-perm-description {
    font-size: 13px; color: var(--kage-text-muted);
    margin-bottom: 14px; line-height: 1.5;
}
.kage-ext-perm-list {
    background: var(--kage-surface, #2A2530);
    border: 1px solid var(--kage-border, #3a3640);
    border-radius: 8px;
    padding: 2px 0;
}
.kage-ext-perm-empty {
    font-size: 13px; color: var(--kage-text-muted);
    padding: 14px; background: var(--kage-surface, #2A2530);
    border: 1px solid var(--kage-border, #3a3640);
    border-radius: 8px;
}
.kage-ext-perm-row {
    display: flex; gap: 12px; padding: 10px 14px;
    border-bottom: 1px solid var(--kage-border, #3a3640);
}
.kage-ext-perm-row:last-child { border-bottom: none; }
.kage-ext-perm-cap-icon {
    font-size: 18px; line-height: 1.4;
    flex-shrink: 0; width: 22px; text-align: center;
}
.kage-ext-perm-cap-body { flex: 1; min-width: 0; }
.kage-ext-perm-cap-label {
    font-size: 13px; font-weight: 600; margin-bottom: 2px;
    display: flex; gap: 8px; align-items: center;
}
.kage-ext-perm-cap-desc {
    font-size: 12px; color: var(--kage-text-muted);
    line-height: 1.4;
}
.kage-ext-perm-badge {
    font-size: 10px; padding: 2px 6px; border-radius: 10px;
    font-weight: 500; letter-spacing: 0.02em;
}
.kage-ext-perm-badge-new {
    background: rgba(217, 160, 91, 0.15);
    color: #d9a05b;
    border: 1px solid rgba(217, 160, 91, 0.35);
}
.kage-ext-perm-badge-existing {
    background: rgba(127, 127, 127, 0.12);
    color: var(--kage-text-muted);
    border: 1px solid var(--kage-border);
}
.kage-ext-perm-footnote {
    font-size: 11px; color: var(--kage-text-muted);
    margin-top: 10px; padding: 0 4px; line-height: 1.4;
}
.kage-ext-perm-footer {
    display: flex; justify-content: flex-end; gap: 10px;
    padding: 12px 18px;
    border-top: 1px solid var(--kage-border, #3a3640);
    background: rgba(0,0,0,0.12);
}
.kage-ext-perm-btn {
    font-size: 13px; padding: 8px 16px;
    background: var(--kage-surface-hover, #352F3D);
    color: var(--kage-text-primary);
    border: 1px solid var(--kage-border, #3a3640);
    border-radius: 6px; cursor: pointer;
    font-family: inherit;
}
.kage-ext-perm-btn:hover { background: var(--kage-surface, #2A2530); }
.kage-ext-perm-btn-primary {
    background: var(--kage-accent, #8B5CF6);
    border-color: var(--kage-accent, #8B5CF6);
    color: #fff;
}
.kage-ext-perm-btn-primary:hover { filter: brightness(1.1); }
body.light-theme .kage-ext-perm-modal {
    background: #ffffff;
    color: #1a1a1a;
    border-color: #ddd;
}
body.light-theme .kage-ext-perm-list,
body.light-theme .kage-ext-perm-empty,
body.light-theme .kage-ext-perm-icon {
    background: #f5f5f5;
    border-color: #ddd;
}
body.light-theme .kage-ext-perm-subtitle,
body.light-theme .kage-ext-perm-description,
body.light-theme .kage-ext-perm-cap-desc,
body.light-theme .kage-ext-perm-footnote,
body.light-theme .kage-ext-perm-version,
body.light-theme .kage-ext-perm-author {
    color: #666;
}
body.light-theme .kage-ext-perm-btn {
    background: #ffffff; border-color: #ccc; color: #1a1a1a;
}
body.light-theme .kage-ext-perm-btn:hover { background: #f0f0f0; }
body.light-theme .kage-ext-perm-footer { background: #fafafa; border-top-color: #ddd; }
    `;
    document.head.appendChild(style);
})();
