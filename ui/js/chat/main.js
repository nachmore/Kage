// Main entry point for expanded chat window
import { ChatApp } from './app.js';
import { KiroDesktopViewer } from './kiro-desktop.js';
import { applyTheme, initThemeListener, loadAndApplyTheme } from '../shared/theme.js';
import { initLinkHandler } from '../shared/link-handler.js';
import { setExtensionManager as setMarkdownExtManager } from '../shared/markdown.js';

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
    initLinkHandler(invoke);
    loadAndApplyTheme(invoke);

    // Re-apply theme when config changes
    listen('config_updated', async () => {
        loadAndApplyTheme(invoke);
        if (app?.speech) app.speech.updateVisibility();
        if (app?.extensionManager) {
            await app.extensionManager.onConfigUpdate();
            await app.extensionManager.reload();
            app.renderExtensionToolbarButtons();
        }
        if (app?.loadShortcuts) app.loadShortcuts();
    });

    // Listen for extension install/uninstall
    listen('extensions_changed', async () => {
        if (app?.extensionManager) {
            await app.extensionManager.reload();
            app.renderExtensionToolbarButtons();
        }
    });

    app = new ChatApp(invoke, appWindow, listen);
    app.init().then(() => {
        setMarkdownExtManager(app.extensionManager);
        // Render extension toolbar buttons
        app.renderExtensionToolbarButtons();
    });

    // Initialize Kiro Desktop viewer
    let desktopViewer = null;
    let currentSource = 'assistant';
    // Expose source state so ChatApp's session refresh doesn't overwrite desktop sessions
    window._kiroSessionSource = 'assistant';

    const kdElements = {
        sessionList: document.getElementById('sessionList'),
        sessionSearch: document.getElementById('sessionSearch'),
    };

    (async () => {
        const viewer = new KiroDesktopViewer(invoke, kdElements, app);
        const available = await viewer.init();
        if (available) {
            desktopViewer = viewer;
            // Wire up source toggle buttons
            const toggle = document.getElementById('sessionSourceToggle');
            if (toggle) {
                toggle.querySelectorAll('.source-toggle-btn').forEach(btn => {
                    btn.addEventListener('click', async () => {
                        const source = btn.dataset.source;
                        if (source === currentSource) return;
                        currentSource = source;
                        window._kiroSessionSource = source;
                        toggle.querySelectorAll('.source-toggle-btn').forEach(b => b.classList.remove('active'));
                        btn.classList.add('active');

                        if (source === 'desktop') {
                            await desktopViewer.loadSessions();
                        } else {
                            desktopViewer.restoreInputArea();
                            app.renderSessionList();
                            // Reload current assistant session
                            if (app.activeSessionId) {
                                app.selectSession(app.activeSessionId);
                            }
                        }
                    });
                });
            }
            // Also filter desktop sessions on search
            kdElements.sessionSearch?.addEventListener('input', () => {
                if (currentSource === 'desktop') desktopViewer.renderSessionList();
            });
        }
    })();

    // Sidebar resize
    const sidebar = document.getElementById('chatSidebar');
    const resizeHandle = document.getElementById('sidebarResizeHandle');
    const toggleBtn = document.getElementById('sidebarToggleBtn');

    if (resizeHandle && sidebar) {
        let isResizing = false;
        resizeHandle.addEventListener('mousedown', (e) => {
            isResizing = true;
            document.body.style.cursor = 'col-resize';
            document.body.style.userSelect = 'none';
            e.preventDefault();
        });
        document.addEventListener('mousemove', (e) => {
            if (!isResizing) return;
            const newWidth = Math.min(500, Math.max(180, e.clientX));
            sidebar.style.width = newWidth + 'px';
        });
        document.addEventListener('mouseup', () => {
            if (isResizing) {
                isResizing = false;
                document.body.style.cursor = '';
                document.body.style.userSelect = '';
            }
        });
    }

    // Sidebar collapse toggle
    const toggleBtnCollapsed = document.getElementById('sidebarToggleBtnCollapsed');
    if (toggleBtn && sidebar) {
        const doToggle = () => {
            sidebar.classList.toggle('collapsed');
            const isCollapsed = sidebar.classList.contains('collapsed');
            if (toggleBtnCollapsed) toggleBtnCollapsed.style.display = isCollapsed ? '' : 'none';
        };
        toggleBtn.addEventListener('click', doToggle);
        if (toggleBtnCollapsed) toggleBtnCollapsed.addEventListener('click', doToggle);
    }

    // Save chat window geometry when it loses focus or is about to close
    async function saveChatGeometry() {
        try {
            const size = await appWindow.innerSize();
            const pos = await appWindow.outerPosition();
            const scale = await appWindow.scaleFactor();
            await invoke('save_chat_window_geometry', {
                width: Math.round(size.width / scale),
                height: Math.round(size.height / scale),
                x: pos.x,
                y: pos.y,
            });
        } catch (e) { console.warn('[Chat] Failed to save window geometry:', e); }
    }
    appWindow.listen('tauri://blur', saveChatGeometry);
    appWindow.listen('tauri://close-requested', saveChatGeometry);

    // Re-refresh sessions + current session every time the window becomes visible
    appWindow.listen('tauri://focus', async () => {
        if (app) {
            await app.loadFloatingSessionId();
            await app.loadCurrentSessionId();
            await app.loadSessions();
            await app.checkConnection();

            // Refresh toolbar data
            app.loadModels();
            app.refreshContextUsage();

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
