// Main entry point
import { FloatingApp } from './app.js';
import { initMarkdown, setExtensionManager as setMarkdownExtManager } from '../shared/markdown.js';
import { applyTheme, initThemeListener, loadAndApplyTheme } from '../shared/theme.js';
import { initLinkHandler } from '../shared/link-handler.js';

function initApp() {
    console.log('initApp called, checking Tauri...');
    
    if (!window.__TAURI__ || !window.__TAURI__.core || !window.__TAURI__.webviewWindow) {
        console.log('Tauri not ready, retrying in 50ms...');
        setTimeout(initApp, 50);
        return;
    }
    
    console.log('Tauri ready! Initializing...');
    
    const { invoke } = window.__TAURI__.core;
    const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
    const { listen } = window.__TAURI__.event;
    
    initMarkdown();
    initThemeListener();
    initLinkHandler(invoke);
    loadAndApplyTheme(invoke);
    
    // Re-apply theme and opacity when config changes
    listen('config_updated', async () => {
        await loadAndApplyTheme(invoke);
    });
    
    const app = new FloatingApp(invoke, appWindow, listen);
    window._floatingApp = app; // Expose for permission modal resize
    app.init().then(() => {
        setMarkdownExtManager(app.extensionManager);
        // Signal the backend that the frontend is fully initialized.
        // Until this fires, hotkey presses are silently ignored to prevent
        // showing a half-initialized window (wrong theme, dead input).
        invoke('notify_frontend_ready').catch(() => {});
    });
}

console.log('Script loaded, document.readyState:', document.readyState);
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initApp);
} else {
    initApp();
}
