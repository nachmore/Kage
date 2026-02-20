/**
 * Custom right-click context menu using a cached Tauri popup window.
 * The window is pre-created at startup and repositioned/shown on demand.
 */

const { invoke } = window.__TAURI__.core;
const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();

async function initContextMenu() {
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
    const { listen: globalListen } = window.__TAURI__.event;
    globalListen('context-menu-action', async (event) => {
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
                    if (responseEl && responseEl.textContent) {
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
                        input.value = input.value.substring(0, start) + text + input.value.substring(end);
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
                } catch (e) { console.error(e); }
                break;
            case 'close':
                await appWindow.hide();
                break;
            case 'inspect':
                try { await invoke('open_devtools'); } catch (e) { console.error(e); }
                break;
            case 'dismissed':
                // Menu closed without action
                break;
        }
    });
}

if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initContextMenu);
} else {
    initContextMenu();
}
