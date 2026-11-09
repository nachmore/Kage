/**
 * The Windows block on an extension's Settings page
 * (docs/design/EXTENSION_VIEWS.md §18.4): each view the extension declares,
 * with Open, and one "Reset window size and position" button.
 *
 * Host-rendered. Shown only for an extension that holds `views` and
 * declares at least one valid view; the grant is the switch. The buttons go
 * through the settings window's delegated dispatcher (actions.js), and the
 * commands they call re-check everything in Rust.
 *
 * The manager calls {@link renderExtensionViewsBlock} once per extension
 * page, after the capability badges, the sync chip and the Background AI
 * panel, before the extension's own settings (SYNC.md §23.3).
 */

import { errMessage } from '../shared/error-message.js';
import { resolveExtensionMessage } from '../shared/extension-manager/i18n.js';
import {
    MAX_TITLE_CHARS,
    normalizeViews,
    sanitizeViewTitle,
} from '../shared/extension-view-policy.js';
import { t } from '../shared/i18n.js';
import { escapeAttr, escapeHtml } from '../shared/tool-utils.js';
import { registerSettingsActions } from './module-registry.js';

/** Action names on the dispatcher. */
export const OPEN_VIEW_ACTION = 'extensionViews.open';
export const RESET_GEOMETRY_ACTION = 'extensionViews.resetGeometry';

const MSG_TOKEN = /^__MSG_(.+)__$/;

/**
 * The views an extension page lists: id, display title and icon. Empty
 * unless the extension holds `views` in its grant.
 *
 * @param {object} module - the page's settings module: `_extensionId`,
 *   `_manifest` (localised), `_capabilities`, `title`, `icon`, and
 *   `_sandbox` (for the extension's `_locales` catalog)
 * @returns {Array<{id: string, title: string, icon: string}>}
 */
export function settingsViewList(module) {
    const extensionId = module?._extensionId;
    if (typeof extensionId !== 'string' || !extensionId) return [];
    const caps = module._capabilities;
    const holdsViews = Array.isArray(caps)
        ? caps.includes('views')
        : typeof caps?.has === 'function' && caps.has('views');
    if (!holdsViews) return [];
    const name = typeof module.title === 'string' ? module.title : extensionId;
    const icon = typeof module.icon === 'string' ? module.icon : '';
    const views = normalizeViews(module._manifest?.contributes?.views, name, icon, extensionId, {
        warn: null,
    });
    const catalog = module._sandbox?.i18nCatalog;
    const fallback = module._sandbox?.i18nFallback;
    return views.map((v) => {
        const token = MSG_TOKEN.exec(v.title);
        const text = token ? resolveExtensionMessage(token[1], catalog, fallback, '') : v.title;
        return {
            id: v.id,
            title: sanitizeViewTitle(text, MAX_TITLE_CHARS) || sanitizeViewTitle(name) || v.id,
            icon: v.icon,
        };
    });
}

/**
 * HTML for the block, or '' when the extension has no views here. Every
 * extension-supplied string is escaped: this window has full Tauri access.
 *
 * @param {object} module - see {@link settingsViewList}
 * @returns {string}
 */
export function renderExtensionViewsBlock(module) {
    const views = settingsViewList(module);
    if (views.length === 0) return '';
    const extId = module._extensionId;
    const extAttr = escapeAttr(extId);
    const rows = views
        .map((v) => {
            const titleId = `ext-views-title-${extId}-${v.id}`;
            return `<div class="ext-views-row" style="display:flex;align-items:center;gap:8px;margin:4px 0;">
                <span class="ext-views-icon" aria-hidden="true">${escapeHtml(v.icon)}</span>
                <span class="ext-views-title" id="${escapeAttr(titleId)}" style="flex:1;min-width:0;">${escapeHtml(v.title)}</span>
                <button type="button" class="setting-button" data-action="${OPEN_VIEW_ACTION}" data-arg="${extAttr}" data-view="${escapeAttr(v.id)}" aria-describedby="${escapeAttr(titleId)}">${escapeHtml(t('settings.manager.ext.views.open'))}</button>
            </div>`;
        })
        .join('');
    return `<div class="setting-row ext-views-block" data-ext-views="${extAttr}">
        <div class="setting-label">${escapeHtml(t('settings.manager.ext.views.heading'))}</div>
        ${rows}
        <div style="display:flex;align-items:center;gap:8px;margin-top:8px;flex-wrap:wrap;">
            <button type="button" class="setting-button" data-action="${RESET_GEOMETRY_ACTION}" data-arg="${extAttr}">${escapeHtml(t('settings.manager.ext.views.reset'))}</button>
            <span class="ext-views-status setting-description" role="status" aria-live="polite"></span>
        </div>
    </div>`;
}

/** The status line of the block that holds `element`. */
function statusSetterFor(element) {
    const status = element?.closest?.('.ext-views-block')?.querySelector('.ext-views-status');
    return (text) => {
        if (status) status.textContent = text;
    };
}

/**
 * Open a view from its Open button. The host's own button, so no
 * extension effect is involved: Rust checks the opener (`settings`), the
 * install, the switch, the grant and the view.
 *
 * @param {string} extensionId
 * @param {string} viewId
 * @param {{ invoke: Function, setStatus?: (text: string) => void }} deps
 * @returns {Promise<boolean>} whether it opened
 */
export async function openViewFromSettings(extensionId, viewId, { invoke, setStatus = () => {} }) {
    setStatus('');
    try {
        await invoke('ext_view_open', { extensionId, viewId, params: null, source: 'settings' });
        return true;
    } catch (e) {
        console.warn(`[ext ${extensionId}] opening view '${viewId}' from Settings failed:`, e);
        setStatus(errMessage(e));
        return false;
    }
}

/**
 * Forget the saved size and position of every view of the extension.
 *
 * @param {string} extensionId
 * @param {{ invoke: Function, setStatus?: (text: string) => void }} deps
 * @returns {Promise<boolean>} whether the reset succeeded
 */
export async function resetViewGeometry(extensionId, { invoke, setStatus = () => {} }) {
    setStatus('');
    try {
        await invoke('ext_view_reset_geometry', { extensionId, viewId: null });
        setStatus(t('settings.manager.ext.views.reset_done'));
        return true;
    } catch (e) {
        console.warn(`[ext ${extensionId}] resetting view geometry failed:`, e);
        setStatus(errMessage(e));
        return false;
    }
}

/**
 * Register the block's two actions with the settings dispatcher.
 *
 * @param {{ invoke?: Function }} [deps] - default: this window's Tauri invoke
 */
export function registerExtensionViewsActions({ invoke } = {}) {
    const call = (...args) => {
        const fn = invoke || globalThis.window?.__TAURI__?.core?.invoke;
        if (typeof fn !== 'function') return Promise.reject(new Error('Tauri is not ready'));
        return fn(...args);
    };
    registerSettingsActions({
        [OPEN_VIEW_ACTION]: (extensionId, element) => {
            const viewId = element?.dataset?.view;
            if (!extensionId || !viewId) return;
            openViewFromSettings(extensionId, viewId, {
                invoke: call,
                setStatus: statusSetterFor(element),
            });
        },
        [RESET_GEOMETRY_ACTION]: (extensionId, element) => {
            if (!extensionId) return;
            resetViewGeometry(extensionId, { invoke: call, setStatus: statusSetterFor(element) });
        },
    });
}

registerExtensionViewsActions();
