/**
 * Custom right-click context menu using a cached Tauri popup window.
 * The window is pre-created at startup and repositioned/shown on demand.
 *
 * Tauri readiness:
 *   `window.__TAURI__` is injected by the runtime (`withGlobalTauri: true`),
 *   but in release builds the module bundle can hit the parser before the
 *   injection lands — destructuring `__TAURI__` at module top-level then
 *   throws and aborts the whole `<script type="module">` graph, taking
 *   `main.js` (and `notify_frontend_ready`) down with it. Defer all
 *   `__TAURI__` access until `waitForTauri` confirms it's there.
 */

import { waitForTauri } from '../shared/tauri-init.js';
import { EVT } from '../shared/events.js';

waitForTauri(({ invoke, appWindow, listen }) => {
    initContextMenu(invoke, appWindow, listen);
});

function initContextMenu(invoke, appWindow, listen) {
    // Suppress default context menu, show cached popup window
    document.addEventListener('contextmenu', async (e) => {
        e.preventDefault();

        try {
            const windowPos = await appWindow.outerPosition();
            const scaleFactor = await appWindow.scaleFactor();

            const screenX = Math.round(windowPos.x / scaleFactor + e.clientX);
            const screenY = Math.round(windowPos.y / scaleFactor + e.clientY);

            // Prevent blur handler from hiding the floating window
            window._contextMenuOpen = true;

            await invoke('show_context_menu', { x: screenX, y: screenY });
        } catch (err) {
            console.error('Failed to show context menu:', err);
            window._contextMenuOpen = false;
        }
    });

    // Listen for actions from the context menu popup (global event)
    listen(EVT.CONTEXT_MENU_ACTION, async (event) => {
        window._contextMenuOpen = false;
        const action = event.payload;

        switch (action) {
            case 'cut':
                document.querySelector('.input-box')?.focus();
                document.execCommand('cut');
                break;
            case 'copy':
                if (window.getSelection().toString()) {
                    document.execCommand('copy');
                } else {
                    const responseEl = document.getElementById('responseText');
                    if (responseEl?.textContent) {
                        navigator.clipboard.writeText(responseEl.textContent).catch(() => {});
                    }
                }
                break;
            case 'paste':
                try {
                    const text = await invoke('read_clipboard');
                    const input = document.querySelector('.input-box');
                    if (input && text) {
                        input.focus();
                        const start = input.selectionStart;
                        const end = input.selectionEnd;
                        input.value =
                            input.value.substring(0, start) + text + input.value.substring(end);
                        input.selectionStart = input.selectionEnd = start + text.length;
                        input.dispatchEvent(new Event('input', { bubbles: true }));
                    }
                } catch (e) {
                    console.error('Paste failed:', e);
                }
                break;
            case 'settings':
                try {
                    await invoke('open_settings_window');
                    await appWindow.hide();
                } catch (e) {
                    console.error(e);
                }
                break;
            case 'close':
                await appWindow.hide();
                break;
            case 'inspect':
                try {
                    await invoke('open_devtools');
                } catch (e) {
                    console.error(e);
                }
                break;
            case 'dismissed':
                // Menu closed without action
                break;
        }
    });
}
