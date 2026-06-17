// Main entry point for expanded chat window
import { ChatApp } from './app.js';
import { AgentSessionViewer } from './agent-sessions.js';
import { initThemeListener, loadAndApplyTheme } from '../shared/theme.js';
import { initLinkHandler } from '../shared/link-handler.js';
import { setExtensionManager as setMarkdownExtManager } from '../shared/markdown.js';
import {
    createMascotController,
    createMascot,
    getMascotThemeSettings,
    setTerminatorMode,
} from '../shared/mascot.js';
import { ANIMATIONS } from '../shared/mascot-animations.js';
import { waitForTauri } from '../shared/tauri-init.js';
import { interceptConsole, setVerboseConsoleCapture } from '../shared/kage-log.js';
import { getConfig, onConfigChange } from '../shared/config-cache.js';
import { trackEventOnce } from '../shared/telemetry.js';
import { EVT } from '../shared/events.js';
import { initI18n, applyStaticTranslations } from '../shared/i18n.js';

let app = null;

waitForTauri(async ({ invoke, appWindow, listen }) => {
    // Direct log beacon (bypasses interceptConsole) so we can tell from
    // the structured log whether the chat window's JS is actually
    // executing in the installer build. If you see this line in the
    // backend log, the webview loaded HTML+JS and `waitForTauri`'s
    // callback fired. If you don't, the webview is stuck before we
    // ever get a chance to call `interceptConsole`.
    invoke('app_log_write', {
        level: 'info',
        source: 'chat',
        msg: '[CHAT] main.js: waitForTauri callback entered',
    }).catch(() => {});

    // One-shot telemetry for the chat window opening. Fired here (not on
    // focus) because the chat window is typically opened deliberately,
    // unlike the floating window which can flash open-close as users
    // dismiss it.
    trackEventOnce('chat_opened');
    // Read the "Log all messages" preference before intercepting console so
    // we honour the saved toggle from the About > Logging settings panel.
    // Safe to default to quiet on any read failure.
    let verboseLogs = false;
    try {
        const cfg = await getConfig(invoke);
        verboseLogs = !!cfg?.system?.verbose_frontend_logging;
    } catch {}
    interceptConsole('chat', { verbose: verboseLogs });

    // Load the active locale's catalog before any rendering so `t()` calls
    // in subsequent code see real translations rather than literal keys.
    try {
        await initI18n(invoke);
    } catch (e) {
        console.warn('[chat] i18n init failed', e);
    }
    applyStaticTranslations(document);

    initThemeListener();
    initLinkHandler(invoke);
    loadAndApplyTheme(invoke);

    // Check terminator mode and set it globally for mascot rendering
    let isTerminator = false;
    try {
        isTerminator = await invoke('is_terminator_mode');
    } catch {}
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

    // Re-apply theme when config changes. onConfigChange (not a raw
    // config_updated listener) runs after the config cache is invalidated,
    // so getConfig()/loadShortcuts() below see fresh data. See config-cache.js.
    onConfigChange(async () => {
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
            const cfg = await getConfig(invoke);
            setVerboseConsoleCapture(!!cfg?.system?.verbose_frontend_logging);
        } catch {}

        // Refresh terminator mode (may have been toggled in settings)
        let newTerminator = false;
        try {
            newTerminator = await invoke('is_terminator_mode');
        } catch {}
        if (newTerminator !== isTerminator) {
            isTerminator = newTerminator;
            setTerminatorMode(isTerminator);
        }
        // Always refresh mascot — theme change may affect outline color
        await refreshSidebarMascot();
    });

    // Listen for extension install/uninstall
    listen(EVT.EXTENSIONS_CHANGED, async () => {
        if (app?.extensionManager) {
            await app.extensionManager.reload();
            app.renderExtensionToolbarButtons();
        }
    });

    app = new ChatApp(invoke, appWindow, listen);
    window._chatApp = app; // Expose for permission modal flush
    // Surface init failures to the backend log — without this catch a
    // throw inside any of the awaits below would silently leave the
    // chat window in a half-initialized state with no signal anywhere.
    // Mirrors the equivalent guard in ui/js/floating/main.js.
    app.init()
        .then(() => {
            setMarkdownExtManager(app.extensionManager);
            app.renderExtensionToolbarButtons();
        })
        .catch((err) => {
            const msg = err instanceof Error ? `${err.message}\n${err.stack || ''}` : String(err);
            console.error('ChatApp.init failed:', msg);
            invoke('app_log_write', {
                level: 'error',
                source: 'chat',
                msg: `ChatApp.init failed: ${msg}`,
            }).catch(() => {});
        });

    // Initialize Kiro Desktop viewer
    let desktopViewer = null;
    let currentSource = 'kage';
    window._kageSessionSource = 'kage';

    const kdElements = {
        sessionList: document.getElementById('sessionList'),
        sessionSearch: document.getElementById('sessionSearch'),
    };

    (async () => {
        const viewer = new AgentSessionViewer(invoke, kdElements, app);
        const available = await viewer.init();
        if (available) {
            desktopViewer = viewer;
            const toggle = document.getElementById('sessionSourceToggle');
            if (toggle) {
                toggle.querySelectorAll('.source-toggle-btn').forEach((btn) => {
                    btn.addEventListener('click', async () => {
                        const source = btn.dataset.source;
                        if (source === currentSource) return;
                        currentSource = source;
                        window._kageSessionSource = source;
                        toggle
                            .querySelectorAll('.source-toggle-btn')
                            .forEach((b) => b.classList.remove('active'));
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
        } catch (e) {
            console.warn('[Chat] Failed to save window geometry:', e);
        }
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
                const exists = app.sessions.find((s) => s.session_id === app.currentAcpSessionId);
                if (exists) {
                    await app.selectSession(app.currentAcpSessionId);
                } else {
                    app.sessions.unshift({
                        session_id: app.currentAcpSessionId,
                        title: 'Current Session',
                        created_at: new Date().toISOString(),
                        updated_at: new Date().toISOString(),
                    });
                    app.activeSessionId = app.currentAcpSessionId;
                    app.renderSessionList();
                    try {
                        const data = await app.invoke('load_session', {
                            sessionId: app.currentAcpSessionId,
                        });
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
