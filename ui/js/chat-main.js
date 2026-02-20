// Main entry point for expanded chat window
import { ChatApp } from './chat-app.js';
import { applyTheme, initThemeListener } from './floating-theme.js';

function initApp() {
    if (!window.__TAURI__ || !window.__TAURI__.core || !window.__TAURI__.webviewWindow) {
        setTimeout(initApp, 50);
        return;
    }

    const { invoke } = window.__TAURI__.core;
    const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
    const { listen } = window.__TAURI__.event;

    applyTheme();
    initThemeListener();

    const app = new ChatApp(invoke, appWindow, listen);
    app.init();
}

if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initApp);
} else {
    initApp();
}
