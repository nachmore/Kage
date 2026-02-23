// Main entry point for expanded chat window
import { ChatApp } from './chat-app.js';
import { applyTheme, initThemeListener, loadAndApplyTheme } from './floating-theme.js';

let app = null;

function initApp() {
    if (!window.__TAURI__ || !window.__TAURI__.core || !window.__TAURI__.webviewWindow) {
        setTimeout(initApp, 50);
        return;
    }

    const { invoke } = window.__TAURI__.core;
    const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
    const { listen } = window.__TAURI__.event;

    initThemeListener();
    loadAndApplyTheme(invoke);

    // Re-apply theme when config changes
    listen('config_updated', () => loadAndApplyTheme(invoke));

    app = new ChatApp(invoke, appWindow, listen);
    app.init();

    // Re-refresh sessions + current session every time the window becomes visible
    appWindow.listen('tauri://focus', async () => {
        if (app) {
            await app.loadFloatingSessionId();
            await app.loadCurrentSessionId();
            await app.loadSessions();
            await app.checkConnection();

            // Auto-select current session if nothing is selected
            if (!app.activeSessionId && app.currentAcpSessionId) {
                const exists = app.sessions.find(s => s.session_id === app.currentAcpSessionId);
                if (exists) {
                    await app.selectSession(app.currentAcpSessionId);
                } else {
                    // Add synthetic entry
                    app.sessions.unshift({
                        session_id: app.currentAcpSessionId,
                        title: 'Current Session',
                        created_at: new Date().toISOString(),
                        updated_at: new Date().toISOString()
                    });
                    app.activeSessionId = app.currentAcpSessionId;
                    app.renderSessionList();
                    try {
                        const data = await app.invoke('load_session', { sessionId: app.currentAcpSessionId });
                        app.displaySession(data);
                    } catch (e) {
                        console.log('[CHAT] Could not load current session from disk:', e);
                    }
                }
            }

            app.renderSessionList();
        }
    });
}

if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initApp);
} else {
    initApp();
}
