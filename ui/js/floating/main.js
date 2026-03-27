// Main entry point
import { FloatingApp } from './app.js';
import { initMarkdown, setExtensionManager as setMarkdownExtManager } from '../shared/markdown.js';
import { applyTheme, initThemeListener, loadAndApplyTheme } from '../shared/theme.js';
import { initLinkHandler } from '../shared/link-handler.js';
import { createMascotController } from '../shared/mascot.js';
import { ANIMATIONS } from '../shared/mascot-animations.js';

const _t0 = performance.now();
const _ts = (label) => console.log(`⏱ [${(performance.now() - _t0).toFixed(0)}ms] ${label}`);

function initApp() {
    _ts('initApp called, checking Tauri...');
    
    if (!window.__TAURI__ || !window.__TAURI__.core || !window.__TAURI__.webviewWindow) {
        console.log('Tauri not ready, retrying in 50ms...');
        setTimeout(initApp, 50);
        return;
    }
    
    _ts('Tauri ready');
    
    const { invoke } = window.__TAURI__.core;
    const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
    const { listen } = window.__TAURI__.event;
    
    initMarkdown();
    initThemeListener();
    initLinkHandler(invoke);
    loadAndApplyTheme(invoke);
    _ts('Theme + markdown initialized');
    
    // Re-apply theme and opacity when config changes
    listen('config_updated', async () => {
        await loadAndApplyTheme(invoke);
    });
    
    const app = new FloatingApp(invoke, appWindow, listen);
    window._floatingApp = app; // Expose for permission modal resize
    // Extension manager will be set asynchronously after extensions load in background
    app._onExtensionsReady = () => setMarkdownExtManager(app.extensionManager);
    app.init();

    // Set up mascot with idle → periodic waving → jumping when active
    const mascotContainer = document.getElementById('floatingMascot');
    if (mascotContainer) {
        const mascotCtrl = createMascotController(mascotContainer, {
            size: 40,
            idle: ANIMATIONS.waving,
            periodic: ANIMATIONS.waving,
            periodicInterval: 10000,
            periodicJitter: 2000,
            preload: [ANIMATIONS.jumping],
        });
        // Expose so FloatingApp can drive it from startThinking/stopThinking
        window._kageMascot = mascotCtrl;
    }
}

console.log('Script loaded, document.readyState:', document.readyState);
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initApp);
} else {
    initApp();
}
