/**
 * Right-click menus for launcher results (apps, files, folders): which
 * results get one, and the menu items for what the backend says a result
 * supports on this OS (`launcher_result_actions`). Shared so the chat
 * window's launcher can reuse it.
 */

import { SEPARATOR } from './popup-menu.js';

/** Actions in menu order; a separator goes between non-empty groups. */
export const RESULT_ACTION_GROUPS = Object.freeze([
    ['open', 'run_as_admin'],
    ['reveal'],
    ['cut_file', 'copy_file', 'copy_path'],
    ['trash', 'uninstall'],
    ['properties'],
]);

/** Actions that destroy something (styled as dangerous). */
export const DESTRUCTIVE_ACTIONS = Object.freeze(['trash', 'uninstall']);

/**
 * What to ask the backend about for `result`: `{ kind: 'app', target:
 * <display name> }` or `{ kind: 'path', target: <path> }`; `null` for every
 * other result kind (they keep the plain text menu). Extension-provided
 * rows are excluded even when they look like apps or files — only Kage's
 * own results get file actions.
 */
export function resultMenuTarget(result) {
    if (!result || typeof result !== 'object' || result._extensionId) return null;
    const str = (v) => (typeof v === 'string' && v.trim() ? v : null);
    switch (result.type) {
        case 'app': {
            const name = str(result.data?.name) || str(result.label);
            return name ? { kind: 'app', target: name } : null;
        }
        case 'file': {
            const path = str(result.data?.path);
            return path ? { kind: 'path', target: path } : null;
        }
        case 'path': {
            const path = str(result.data?.value);
            return path ? { kind: 'path', target: path } : null;
        }
        default:
            return null;
    }
}

/** The i18n key labelling `action` on `platform` (OS file-manager wording). */
export function resultActionLabelKey(action, platform) {
    switch (action) {
        case 'reveal':
            return platform === 'macos'
                ? 'result_menu.reveal_finder'
                : platform === 'windows'
                  ? 'result_menu.reveal_explorer'
                  : 'result_menu.reveal_folder';
        case 'copy_path':
            return platform === 'windows' ? 'result_menu.copy_as_path' : 'result_menu.copy_path';
        case 'trash':
            return platform === 'windows'
                ? 'result_menu.move_to_recycle_bin'
                : 'result_menu.move_to_trash';
        case 'copy_file':
            return 'result_menu.copy';
        case 'cut_file':
            return 'result_menu.cut';
        default:
            return `result_menu.${action}`;
    }
}

/** Why-it's-disabled reasons the menu can show (`unavailable[].reason`). */
const UNAVAILABLE_REASONS = Object.freeze(['path_too_long']);

/**
 * The i18n key labelling `action` shown disabled for `reason` — the
 * action's own wording plus why (`result_menu.unavailable.trash_path_too_long`).
 */
export function unavailableLabelKey(action, reason) {
    return `result_menu.unavailable.${action}_${reason}`;
}

/**
 * Menu items for `info` (`{ platform, kind, name, path, actions,
 * unavailable }` from `launcher_result_actions`), grouped and ordered per
 * RESULT_ACTION_GROUPS. `unavailable` lists actions shown disabled, with
 * the reason in their label. Unknown action ids and reasons are ignored.
 */
export function buildResultMenuItems(info) {
    const actions = new Set(Array.isArray(info?.actions) ? info.actions : []);
    const unavailable = new Map();
    for (const u of Array.isArray(info?.unavailable) ? info.unavailable : []) {
        if (u && !actions.has(u.action) && UNAVAILABLE_REASONS.includes(u.reason)) {
            unavailable.set(u.action, u.reason);
        }
    }
    const platform = info?.platform;
    const items = [];
    for (const group of RESULT_ACTION_GROUPS) {
        const present = group.filter((a) => actions.has(a) || unavailable.has(a));
        if (!present.length) continue;
        if (items.length) items.push(SEPARATOR);
        for (const action of present) {
            const reason = unavailable.get(action);
            if (reason) {
                items.push({
                    id: action,
                    labelKey: unavailableLabelKey(action, reason),
                    labelVars: { name: info.name || '' },
                    enabled: false,
                    danger: false,
                });
                continue;
            }
            const item = {
                id: action,
                labelKey: resultActionLabelKey(action, platform),
                labelVars: { name: info.name || '' },
                enabled: true,
                danger: DESTRUCTIVE_ACTIONS.includes(action),
            };
            // Enter opens the selected result, as it always has.
            if (action === 'open') item.shortcut = platform === 'macos' ? '↩' : 'Enter';
            items.push(item);
        }
    }
    return items.map((i) => (i.separator ? { separator: true } : i));
}
