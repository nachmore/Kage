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
// This file is loaded as a non-module script (matching every other
// settings/* module). The two functions it defines are intentionally
// global so the existing module scripts can call them without ES
// import wiring.
//
// Tested via `ui/tests/shared/settings-actions.test.js`.

(function () {
    if (typeof window === 'undefined') return;
    if (window.__settingsActionsInstalled) return;
    window.__settingsActionsInstalled = true;

    const handlers = Object.create(null);

    function registerSettingsActions(map) {
        Object.assign(handlers, map);
    }

    function dispatchSettingsAction(name, arg, element, event) {
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

    function findActionElement(start, attr) {
        // Walk up to a parent carrying the requested data-attr. We use
        // closest() so clicks on inner spans/icons still trigger.
        if (!start?.closest) return null;
        return start.closest(`[${attr}]`);
    }

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

    window.registerSettingsActions = registerSettingsActions;
    window.dispatchSettingsAction = dispatchSettingsAction;
})();
