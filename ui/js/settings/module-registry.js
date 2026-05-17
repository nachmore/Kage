/**
 * Shared singleton for the settings window's `SettingsManager`.
 *
 * Settings modules call `getSettingsManager()` to look up sibling
 * modules — typically used from action handlers wired via
 * `registerSettingsActions` (see actions.js).
 *
 * Re-exports `registerSettingsActions` from actions.js so modules can
 * import everything they need from a single place. This is the only
 * cross-module wiring helper they should reach for; never read the
 * manager from a window global.
 */

let _manager = null;

export function setSettingsManager(manager) {
    _manager = manager;
}

export function getSettingsManager() {
    return _manager;
}

export { registerSettingsActions, dispatchSettingsAction } from './actions.js';
