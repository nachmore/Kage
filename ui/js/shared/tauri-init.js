/**
 * Shared Tauri readiness helper.
 *
 * Waits for window.__TAURI__ APIs to be available, then calls the callback
 * with { invoke, appWindow, listen } already extracted.
 *
 * Handles both DOMContentLoaded gating and the Tauri API polling.
 */

function pollTauri(callback) {
    if (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.webviewWindow) {
        callback({
            invoke: window.__TAURI__.core.invoke,
            appWindow: window.__TAURI__.webviewWindow.getCurrentWebviewWindow(),
            listen: window.__TAURI__.event.listen,
        });
    } else {
        setTimeout(() => pollTauri(callback), 50);
    }
}

/**
 * Wait for DOM + Tauri to be ready, then call `callback({ invoke, appWindow, listen })`.
 */
export function waitForTauri(callback) {
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', () => pollTauri(callback));
    } else {
        pollTauri(callback);
    }
}
