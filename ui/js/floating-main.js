// Main entry point
import { FloatingApp } from './floating-app.js';
import { initMarkdown } from './markdown.js';
import { applyTheme, initThemeListener, loadAndApplyTheme } from './theme.js';

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
    loadAndApplyTheme(invoke);
    
    // Re-apply theme and opacity when config changes
    listen('config_updated', async () => {
        await loadAndApplyTheme(invoke);
    });
    
    const app = new FloatingApp(invoke, appWindow, listen);
    window._floatingApp = app; // Expose for permission modal resize
    app.init();
}

console.log('Script loaded, document.readyState:', document.readyState);
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initApp);
} else {
    initApp();
}
