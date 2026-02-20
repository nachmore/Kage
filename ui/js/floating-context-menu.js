/**
 * Custom right-click context menu using a separate Tauri popup window.
 */

const { invoke } = window.__TAURI__.tauri;
const { appWindow } = window.__TAURI__.window;

async function initContextMenu() {
    // Suppress default context menu, spawn popup window
    document.addEventListener('contextmenu', async (e) => {
        e.preventDefault();
        
        try {
            // Get the floating window's screen position
            const windowPos = await appWindow.outerPosition();
            const scaleFactor = await appWindow.scaleFactor();
            
            // Convert click coordinates to screen coordinates
            const screenX = Math.round(windowPos.x / scaleFactor + e.clientX);
            const screenY = Math.round(windowPos.y / scaleFactor + e.clientY);
            
            // Set flag so blur handler doesn't hide the floating window
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
                // Focus the input first, then execute
                document.querySelector('.input-box')?.focus();
                document.execCommand('cut');
                break;
            case 'copy':
                // If there's a text selection, copy it; otherwise copy response
                if (window.getSelection().toString()) {
                    document.execCommand('copy');
                } else {
                    // Copy the full response text
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
                try { await invoke('open_settings_window'); } catch (e) { console.error(e); }
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
