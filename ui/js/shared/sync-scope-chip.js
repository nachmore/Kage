/**
 * The where-it-applies control on Settings rows (docs/design/SYNC.md §9.4):
 * "All computers", "Macs only", "This computer", and its menu of scope
 * actions (§9.3). Rows opt in with `data-sync-path`.
 *
 * Skeleton from the contracts commit: nothing is attached yet, and the
 * pure helpers answer "no control". WP-S6 writes them. With sync off,
 * nothing changes on any page.
 */

/**
 * Add a control to every `[data-sync-path]` row under `root`, with one
 * `sync_get_scopes` call. Returns a function that removes them.
 *
 * @param {Element} _root
 * @param {{ invoke: Function, listen: Function, t: Function, os: string }} _deps
 * @returns {() => void}
 */
export function attachScopeChips(_root, _deps) {
    return () => {};
}

/**
 * What the control shows for one `ScopeView` on this OS, or `null` for no
 * control.
 *
 * @param {object} _scopeView
 * @param {string} _os - `macos`, `windows` or `linux`
 * @returns {{ label: string, tone: string } | null}
 */
export function chipState(_scopeView, _os) {
    return null;
}

/**
 * The menu items for one `ScopeView` on this OS: each a scope action with
 * its text.
 *
 * @param {object} _scopeView
 * @param {string} _os
 * @returns {{ action: string, label: string, checked?: boolean }[]}
 */
export function menuItems(_scopeView, _os) {
    return [];
}
