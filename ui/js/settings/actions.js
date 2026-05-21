// Delegated action dispatcher for the settings window.
//
// Replaces the previous pattern of inline `onclick="globalFn(...)"`
// attributes calling window-level globals. Each settings module
// registers its handlers via `registerSettingsActions({ id: handler })`
// and rendered HTML uses `data-action="id"` (and optionally
// `data-arg="..."`) on the element.
//
// Click and change events bubble up to a single listener installed on
// document.body that walks the event target up to the nearest element
// carrying `data-action`, looks up the registered handler, and calls
// it with `(arg, element, event)`. Handlers can read further context
// from element data attributes if they need to.
//
// Tested via `ui-tests/shared/settings-actions.test.js`.

const handlers = Object.create(null);
let _installed = false;

function findActionElement(start, attr) {
    // Walk up to a parent carrying the requested data-attr. We use
    // closest() so clicks on inner spans/icons still trigger.
    if (!start?.closest) return null;
    return start.closest(`[${attr}]`);
}

/**
 * Install the global click/change listener exactly once. Idempotent —
 * safe to call from each module's import path.
 */
export function installActionDispatcher() {
    if (typeof document === 'undefined') return;
    if (_installed) return;
    _installed = true;

    document.addEventListener(
        'click',
        (event) => {
            const el = findActionElement(event.target, 'data-action');
            if (!el) return;
            const name = el.getAttribute('data-action');
            if (!name) return;
            // For button/anchor inside a form, prevent the default submit.
            // It's a settings click, never a navigation.
            const tag = el.tagName;
            if (tag === 'BUTTON' || tag === 'A') event.preventDefault();
            dispatchSettingsAction(name, el.dataset.arg, el, event);
        },
        true
    );

    document.addEventListener(
        'change',
        (event) => {
            // Selects/inputs use data-action-change for change events.
            // Routed separately so a button with data-action doesn't ALSO
            // fire on bubbling change events from inside it.
            const el = findActionElement(event.target, 'data-action-change');
            if (!el) return;
            const name = el.getAttribute('data-action-change');
            if (!name) return;
            dispatchSettingsAction(name, el.dataset.arg, el, event);
        },
        true
    );
}

export function registerSettingsActions(map) {
    Object.assign(handlers, map);
}

export function dispatchSettingsAction(name, arg, element, event) {
    const handler = handlers[name];
    if (!handler) {
        console.warn('[settings] no handler for action:', name);
        return;
    }
    try {
        handler(arg, element, event);
    } catch (e) {
        console.error('[settings] handler for', name, 'threw:', e);
    }
}

// Install dispatcher on import.
installActionDispatcher();

// Keep window globals so the existing settings-actions test (and any
// not-yet-migrated callers) still work.
if (typeof window !== 'undefined') {
    window.registerSettingsActions = registerSettingsActions;
    window.dispatchSettingsAction = dispatchSettingsAction;
}
