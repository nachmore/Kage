/**
 * The `open_view` host effect (docs/design/EXTENSION_VIEWS.md §6): an
 * extension asks the host to open one of its own view windows, from a
 * click or key press the host handled (a result row, a formatter card or
 * widget button, a toolbar button, a settings action).
 *
 * Every window that runs extension instances opens views through
 * {@link runOpenViewEffect}. The caller names the extension from what the
 * host knows (the row's stamp, the sandbox it asked), never from the
 * effect. The host-side checks of §6.2 run first; Rust repeats them
 * against the installed manifest and grant in `ext_view_open`.
 */

import { errKind, errMessage } from './error-message.js';
import { normalizeViewSource, validateOpenViewEffect } from './extension-view-policy.js';
import { t } from './i18n.js';

/** The effect's `type`. */
export const OPEN_VIEW = 'open_view';

/** Whether `host` is an `open_view` effect (its shape is checked later). */
export function isOpenViewEffect(host) {
    return !!host && typeof host === 'object' && host.type === OPEN_VIEW;
}

function refusal(kind, key, vars, reason) {
    return { ok: false, kind, key, vars, reason };
}

/**
 * The §6.2 host-side checks for `effect`, in order: the window offers the
 * `views` host feature, the extension holds `views`, the manifest declares
 * the view, and the params are valid.
 *
 * @param {unknown} effect
 * @param {object} ctx - see `validateOpenViewEffect` in extension-view-policy.js
 * @returns {null | {ok: false, kind: string, key: string, vars: object, reason: string}}
 *   null when the view may open
 */
export function openViewDenial(effect, ctx) {
    const check = validateOpenViewEffect(effect, ctx);
    return check.ok ? null : check;
}

/**
 * Open the view an `open_view` effect asks for. Never throws: a refusal
 * or a failure is logged (`[ext <id>] open_view refused: <reason>`) and
 * comes back as `{ ok: false, … }` with a localised `message`, which the
 * launcher and chat show as a reply and Settings shows on the action row.
 *
 * @param {unknown} effect - `{ type: 'open_view', view, params? }` from the extension
 * @param {object} opts
 * @param {string} opts.extensionId - the host's id for the extension
 * @param {string} opts.source - what opened it (`VIEW_SOURCES`); anything else is `launcher`
 * @param {Function} opts.invoke - Tauri invoke
 * @param {(effect: unknown) => (null | object)} opts.check - the host-side
 *   checks, e.g. {@link openViewDenial} with this window's grant; a
 *   refusal object has `kind`, `key`, `vars` and `reason`
 * @param {() => void} [opts.beforeOpen] - runs just before `ext_view_open`
 *   (the launcher arms its blur suppression here, §15.2)
 * @returns {Promise<{ok: true, label: string, reused: boolean}
 *   | {ok: false, kind: string, key: string|null, vars: object, reason: string, message: string}>}
 */
export async function runOpenViewEffect(effect, opts = {}) {
    const { extensionId, invoke, check, beforeOpen } = opts;
    const source = normalizeViewSource(opts.source);
    const tag = `[ext ${typeof extensionId === 'string' && extensionId ? extensionId : '?'}]`;

    let denied;
    if (typeof extensionId !== 'string' || !extensionId) {
        denied = refusal(
            'not_permitted',
            'errors.ext_view.opener_refused',
            {},
            'the host knows no extension id for this effect'
        );
    } else if (!isOpenViewEffect(effect)) {
        denied = refusal(
            'invalid_input',
            'errors.ext_view.unknown_view',
            { extension: extensionId, view: '' },
            'not an open_view effect'
        );
    } else if (typeof check !== 'function' || typeof invoke !== 'function') {
        denied = refusal(
            'not_permitted',
            'errors.ext_view.opener_refused',
            {},
            'this window cannot check or open extension views'
        );
    } else {
        try {
            denied = check(effect);
        } catch (e) {
            denied = refusal(
                'not_permitted',
                'errors.ext_view.opener_refused',
                {},
                `the checks failed: ${errMessage(e)}`
            );
        }
    }
    if (denied !== null && denied !== undefined) {
        // A check returns null or a refusal; anything else is a refusal too.
        if (typeof denied !== 'object' || typeof denied.key !== 'string') {
            denied = refusal(
                'not_permitted',
                'errors.ext_view.opener_refused',
                {},
                'the checks gave no reason'
            );
        }
        console.warn(`${tag} open_view refused: ${denied.reason || denied.key}`);
        return { ...denied, message: t(denied.key, denied.vars || {}) };
    }

    try {
        beforeOpen?.();
    } catch (e) {
        console.warn(`${tag} open_view: the window's beforeOpen hook failed:`, e);
    }
    try {
        const opened = await invoke('ext_view_open', {
            extensionId,
            viewId: effect.view,
            params: effect.params === undefined ? null : effect.params,
            source,
        });
        return {
            ok: true,
            label: typeof opened?.label === 'string' ? opened.label : '',
            reused: opened?.reused === true,
        };
    } catch (e) {
        const message = errMessage(e);
        console.warn(`${tag} open_view failed: ${message}`);
        return {
            ok: false,
            kind: errKind(e) || 'internal',
            key: typeof e?.key === 'string' ? e.key : null,
            vars: {},
            reason: message,
            message,
        };
    }
}
