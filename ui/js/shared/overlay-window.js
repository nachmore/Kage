/**
 * Show + focus an overlay window (floating launcher, inline assist, context
 * menu) the way the backend does.
 *
 * On macOS the overlays are non-activating NSPanels so they can appear over
 * other apps' full-screen Spaces. Tauri's `setFocus()` activates the whole
 * app (`activateIgnoringOtherApps`), which can drag the user back to the
 * desktop Space — so on macOS we ask the backend to present the panel
 * without activating (`present_overlay_window`). Elsewhere, or if that
 * fails, it's the usual show + setFocus.
 */
import { isMac } from './shortcuts.js';

export async function presentOverlayWindow(appWindow) {
    const invoke = window.__TAURI__?.core?.invoke;
    if (isMac() && typeof invoke === 'function') {
        try {
            await invoke('present_overlay_window');
            return;
        } catch (e) {
            console.warn('[overlay] present_overlay_window failed; falling back', e);
        }
    }
    await appWindow.show();
    await appWindow.setFocus();
}
