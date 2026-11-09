/**
 * The sync chip in an extension's Settings header (docs/design/SYNC.md
 * §12.3): "Syncs settings and data", "Syncs settings" or "Sign-in stays on
 * this computer". It sits right after the capability badges.
 *
 * Skeleton from the contracts commit: it draws nothing yet. WP-S6 writes
 * it and calls it from manager.js.
 *
 * @param {string} _extensionId
 * @param {{ invoke: Function, t: Function }} _deps
 * @returns {string} HTML for the header, or '' for no chip
 */
export function renderSyncChip(_extensionId, _deps) {
    return '';
}
