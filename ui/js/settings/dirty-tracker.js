/**
 * Per-module "has unsaved edits" bookkeeping for the settings window.
 *
 * Edits in the settings window are only persisted by the global Save, so
 * the manager must know which sections hold unsaved changes:
 *   - a dirty section is never reloaded from config (that would silently
 *     revert the user's edits), and
 *   - a clean section is never written back on Save (its DOM may be a stale
 *     snapshot of config that another window has since changed — e.g. a
 *     shortcut added from the floating window or a grant changed).
 *
 * Kept free of manager/Tauri state so it can be unit-tested directly
 * (ui-tests/settings/dirty-tracker.test.js).
 */

/**
 * Event a module dispatches (bubbling, from inside its section) to flag an
 * edit the DOM-event heuristics below can't see. `SettingsModule.markDirty()`
 * wraps it.
 */
export const SETTINGS_DIRTY_EVENT = 'kage:settings-dirty';

/**
 * Clicks on these count as edits. Many list editors (shortcuts, automations,
 * connections, store sources, extension toggles) mutate module state from a
 * button with no input/change event, so a click on any button-like control
 * inside a section marks it dirty. Deliberately over-inclusive: a false
 * "dirty" only means the section isn't auto-refreshed and gets written on
 * Save (the old behaviour), while a missed edit would be silently dropped.
 */
const EDIT_CLICK_SELECTOR =
    'button, input[type="button"], input[type="submit"], [data-action], [role="button"], [role="switch"], [role="checkbox"], [role="radio"]';

/** DOM events the manager listens for (document capture phase). */
export const DIRTY_EVENT_TYPES = ['input', 'change', 'click', SETTINGS_DIRTY_EVENT];

/**
 * Return the id of the settings section an edit event belongs to, or null
 * when the event isn't an edit or happened outside every section.
 *
 * Walks `composedPath()` rather than `target.closest()`: the delegated
 * action dispatcher runs earlier in the capture phase and its handlers often
 * re-render or remove the clicked node (e.g. "delete row"), detaching the
 * target before this runs. The event path is fixed at dispatch time, so it
 * still leads to the section.
 */
export function dirtySectionForEvent(event) {
    if (!event) return null;
    const path = typeof event.composedPath === 'function' ? event.composedPath() : [];
    let isEdit = event.type !== 'click';
    for (const node of path) {
        if (!node || node.nodeType !== 1) continue;
        if (!isEdit && typeof node.matches === 'function' && node.matches(EDIT_CLICK_SELECTOR)) {
            isEdit = true;
        }
        const id = node.dataset?.sectionContent;
        if (id) return isEdit ? id : null;
    }
    return null;
}

/**
 * Tracks which module ids hold unsaved edits. Each mark bumps a per-id
 * version so a Save can clear exactly the edits it persisted: anything
 * edited again while the save was in flight stays dirty.
 */
export class DirtyTracker {
    constructor() {
        this._versions = new Map();
    }

    mark(id) {
        if (!id) return;
        this._versions.set(id, (this._versions.get(id) || 0) + 1);
    }

    isDirty(id) {
        return this._versions.has(id);
    }

    /** Copy of the current id → version map, taken when a Save starts. */
    snapshot() {
        return new Map(this._versions);
    }

    /** Clear the ids captured in `snap` unless they were edited since. */
    clearSaved(snap) {
        for (const [id, version] of snap) {
            if (this._versions.get(id) === version) this._versions.delete(id);
        }
    }

    clearAll() {
        this._versions.clear();
    }
}
