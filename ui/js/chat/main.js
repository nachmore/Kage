// Main entry point for expanded chat window
import { ChatApp } from './app.js';
import { KageDesktopViewer } from './kage-desktop.js';
import { initThemeListener, loadAndApplyTheme } from '../shared/theme.js';
import { initLinkHandler } from '../shared/link-handler.js';
import { setExtensionManager as setMarkdownExtManager } from '../shared/markdown.js';
import { createMascotController, createMascot, getMascotThemeSettings, setTerminatorMode, mascotHTML } from '../shared/mascot.js';
import { ANIMATIONS } from '../shared/mascot-animations.js';
import { waitForTauri } from '../shared/tauri-init.js';
import { interceptConsole, setVerboseConsoleCapture } from '../shared/kage-log.js';

let app = null;

waitForTauri(async ({ invoke, appWindow, listen }) => {
    // Read the "Log all messages" preference before intercepting console so
    // we honour the saved toggle from the About > Logging settings panel.
    // Safe to default to quiet on any read failure.
    let verboseLogs = false;
    try {
        const cfg = await invoke('get_config');
        verboseLogs = !!cfg?.system?.verbose_frontend_logging;
    } catch {}
    interceptConsole('chat', { verbose: verboseLogs });
    initThemeListener();
    initLinkHandler(invoke);
    loadAndApplyTheme(invoke);

    // Check terminator mode and set it globally for mascot rendering
    let isTerminator = false;
    try { isTerminator = await invoke('is_terminator_mode'); } catch {}
    setTerminatorMode(isTerminator);

    // Render sidebar mascot and title — extracted so it can be refreshed on config change
    async function refreshSidebarMascot() {
        const title = document.querySelector('.sidebar-title');
        const mascot = document.getElementById('sidebarMascot');
        if (title) {
            if (isTerminator) {
                title.style.background = 'none';
                title.style.webkitTextFillColor = '#ef4444';
            } else {
                title.style.background = '';
                title.style.webkitTextFillColor = '';
            }
        }
        if (mascot) {
            mascot.innerHTML = '';
            if (isTerminator) {
                const svg = await createMascot({
                    src: 'assets/kage-terminator.svg',
                    size: 28,
                    outline: { color: '#ef4444', radius: 1 },
                });
                mascot.appendChild(svg);
            } else {
                const { outlineColor, invert } = getMascotThemeSettings();
                createMascotController(mascot, {
                    size: 28,
                    idle: ANIMATIONS.waving,
                    periodic: ANIMATIONS.waving,
                    periodicInterval: 30000,
                    periodicJitter: 5000,
                    invert,
                    outline: { color: outlineColor, radius: 1.5 },
                });
            }
        }
    }
    await refreshSidebarMascot();

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

        // Pick up changes to the verbose-logging toggle live so the user
        // doesn't have to restart anything.
        try {
            const cfg = await invoke('get_config');
            setVerboseConsoleCapture(!!cfg?.system?.verbose_frontend_logging);
        } catch {}

        // Refresh terminator mode (may have been toggled in settings)
        let newTerminator = false;
        try { newTerminator = await invoke('is_terminator_mode'); } catch {}
        if (newTerminator !== isTerminator) {
            isTerminator = newTerminator;
            setTerminatorMode(isTerminator);
        }
        // Always refresh mascot — theme change may affect outline color
        await refreshSidebarMascot();
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
        app.renderExtensionToolbarButtons();
    });

    // Initialize Kage Desktop viewer
    let desktopViewer = null;
    let currentSource = 'kage';
    window._kageSessionSource = 'kage';

    const kdElements = {
        sessionList: document.getElementById('sessionList'),
        sessionSearch: document.getElementById('sessionSearch'),
    };

    (async () => {
        const viewer = new KageDesktopViewer(invoke, kdElements, app);
        const available = await viewer.init();
        if (available) {
            desktopViewer = viewer;
            const toggle = document.getElementById('sessionSourceToggle');
            if (toggle) {
                toggle.querySelectorAll('.source-toggle-btn').forEach(btn => {
                    btn.addEventListener('click', async () => {
                        const source = btn.dataset.source;
                        if (source === currentSource) return;
                        currentSource = source;
                        window._kageSessionSource = source;
                        toggle.querySelectorAll('.source-toggle-btn').forEach(b => b.classList.remove('active'));
                        btn.classList.add('active');

                        if (source === 'desktop') {
                            await desktopViewer.loadSessions();
                        } else {
                            desktopViewer.restoreInputArea();
                            app.renderSessionList();
                            if (app.activeSessionId) {
                                app.selectSession(app.activeSessionId);
                            }
                        }
                    });
                });
            }
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

            app.loadModels();
            app.refreshContextUsage();

            if (!app.activeSessionId && app.currentAcpSessionId) {
                const exists = app.sessions.find(s => s.session_id === app.currentAcpSessionId);
                if (exists) {
                    await app.selectSession(app.currentAcpSessionId);
                } else {
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
});
