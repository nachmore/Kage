// Main entry point
import { FloatingApp } from './app.js';
import { initMarkdown, setExtensionManager as setMarkdownExtManager } from '../shared/markdown.js';
import { initThemeListener, loadAndApplyTheme } from '../shared/theme.js';
import { initLinkHandler } from '../shared/link-handler.js';
import { getMascotThemeSettings, setTerminatorMode } from '../shared/mascot.js';
import { createMascotController } from '../shared/mascot-engine.js';
import { flushPendingMascotUpdated, installMascotHintArbiter } from './mascot-signals.js';
import { waitForTauri } from '../shared/tauri-init.js';
import { interceptConsole, setVerboseConsoleCapture } from '../shared/kage-log.js';
import { getConfig, onConfigChange } from '../shared/config-cache.js';
import { trackEventOnce } from '../shared/telemetry.js';
import { WINDOW } from '../shared/window-labels.js';
import { initI18n, applyStaticTranslations } from '../shared/i18n.js';

const _t0 = performance.now();
const _ts = (label) => console.log(`⏱ [${(performance.now() - _t0).toFixed(0)}ms] ${label}`);

waitForTauri(async ({ invoke, appWindow, listen }) => {
    _ts('Tauri ready');

    // Read the "Log all messages" preference before intercepting console so
    // we honour the saved toggle from the About > Logging settings panel.
    // Safe to default to quiet on any read failure.
    let verboseLogs = false;
    try {
        const cfg = await getConfig(invoke);
        verboseLogs = !!cfg?.system?.verbose_frontend_logging;
    } catch {}
    interceptConsole(WINDOW.FLOATING, { verbose: verboseLogs });

    try {
        await initI18n(invoke);
    } catch (e) {
        console.warn('[floating] i18n init failed', e);
    }
    applyStaticTranslations(document);

    initMarkdown();
    initThemeListener();
    initLinkHandler(invoke);
    loadAndApplyTheme(invoke);
    _ts('Theme + markdown initialized');

    // Re-apply theme and opacity when config changes. onConfigChange (not a
    // raw config_updated listener) runs after the cache is invalidated, so
    // getConfig() below sees fresh data. See config-cache.js.
    onConfigChange(async () => {
        await loadAndApplyTheme(invoke);

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
        await refreshFloatingMascot();
    });

    // Publish `window.__kageMascotHint` before the app (and with it the
    // extension manager) exists, so an extension that declares an activity
    // hint during its own startup isn't dropped on the floor. The arbiter
    // doesn't need the mascot controller to exist — signalMascot is a no-op
    // until refreshFloatingMascot() runs below.
    installMascotHintArbiter();

    const app = new FloatingApp(invoke, appWindow, listen);
    window._floatingApp = app; // Expose for permission modal resize
    // Extension manager will be set asynchronously after extensions load in background
    app._onExtensionsReady = () => setMarkdownExtManager(app.extensionManager);
    // Don't await — init() does extension loading in the background and
    // signals frontend_ready partway through. Awaiting would delay the
    // mascot setup below. But we MUST surface failures: if init throws,
    // notify_frontend_ready never fires and the floating window appears
    // dead with no logs anywhere. Mirror the error to the backend so we
    // notice instead of silently hanging.
    app.init().catch((err) => {
        const msg = err instanceof Error ? `${err.message}\n${err.stack || ''}` : String(err);
        console.error('FloatingApp.init failed:', msg);
        // Try to surface to the user even though most of the UI may be broken
        invoke('app_log_write', {
            level: 'error',
            source: 'floating',
            msg: `FloatingApp.init failed: ${msg}`,
        }).catch(() => {});
    });

    // Telemetry: count once per process when the floating window becomes
    // visible for the first time. Subsequent shows/hides are implicit in
    // `app_daily_active` and `app_started` so we don't need a counter per
    // summons. Debounced via trackEventOnce.
    appWindow.listen('tauri://focus', () => {
        trackEventOnce('floating_shown');
    });

    // Set up mascot — use terminator variant if terminator mode is active
    let isTerminator = false;
    try {
        isTerminator = await invoke('is_terminator_mode');
    } catch {}
    setTerminatorMode(isTerminator);

    // Inputs the mascot is built from; config_updated fires for unrelated
    // saves too, and a rebuild drops the hidden-window pause + thinking anim.
    let lastMascotKey = null;

    // Config snapshot the mascot reads at rebuild time; nothing fatal if the
    // read fails (defaults are on + hints on).
    let _mascotCfg = {};
    try {
        _mascotCfg = (await getConfig(invoke)) || {};
    } catch {}

    async function refreshFloatingMascot() {
        const mascotContainer = document.getElementById('floatingMascot');
        if (!mascotContainer) return;
        const theme = getMascotThemeSettings();
        const key = `${isTerminator}|${theme.outlineColor}|${theme.invert}`;
        if (key === lastMascotKey) return;
        lastMascotKey = key;
        // Destroy existing mascot controller if any
        if (window._kageMascot) {
            window._kageMascot.destroy();
            window._kageMascot = null;
        }
        mascotContainer.innerHTML = '';

        if (isTerminator) {
            const { createMascot } = await import('../shared/mascot.js');
            const svg = await createMascot({
                src: 'assets/kage-terminator.svg',
                size: 40,
                outline: { color: '#ef4444', radius: 1 },
            });
            mascotContainer.appendChild(svg);
            window._kageMascot = null;
        } else {
            const { outlineColor, invert } = theme;
            // New engine: full behaviour set, inline SVG, prefers-reduced-motion
            // and the signal() API below. The legacy setActive/setIdle methods
            // are kept as aliases, so ui-state.js and friends work unchanged.
            const animationsEnabled = _mascotCfg?.ui?.mascot_animations !== false;
            const extensionHints = _mascotCfg?.ui?.mascot_extension_hints !== false;
            const mascotCtrl = createMascotController(mascotContainer, {
                size: 40,
                profile: 'full',
                invert,
                outline: { color: outlineColor, radius: 2 },
                animations: animationsEnabled,
                extensionHints,
            });
            window._kageMascot = mascotCtrl;
            // Carry over state the old controller had: the thinking state if
            // we rebuilt mid-response, and the hidden-window pause so a
            // hidden webview doesn't keep driving rAF.
            if (mascotContainer.classList.contains('thinking')) {
                mascotCtrl.signal('think');
            }
            if (window._kageFloatingHidden) mascotCtrl.pause();
            flushPendingMascotUpdated();
        }
    }
    await refreshFloatingMascot();
});
