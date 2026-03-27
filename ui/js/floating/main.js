// Main entry point
import { FloatingApp } from './app.js';
import { initMarkdown, setExtensionManager as setMarkdownExtManager } from '../shared/markdown.js';
import { initThemeListener, loadAndApplyTheme } from '../shared/theme.js';
import { initLinkHandler } from '../shared/link-handler.js';
import { createMascotController } from '../shared/mascot.js';
import { ANIMATIONS } from '../shared/mascot-animations.js';
import { waitForTauri } from '../shared/tauri-init.js';

const _t0 = performance.now();
const _ts = (label) => console.log(`⏱ [${(performance.now() - _t0).toFixed(0)}ms] ${label}`);

waitForTauri(({ invoke, appWindow, listen }) => {
    _ts('Tauri ready');

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
});
