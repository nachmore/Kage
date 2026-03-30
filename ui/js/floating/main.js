// Main entry point
import { FloatingApp } from './app.js';
import { initMarkdown, setExtensionManager as setMarkdownExtManager } from '../shared/markdown.js';
import { initThemeListener, loadAndApplyTheme } from '../shared/theme.js';
import { initLinkHandler } from '../shared/link-handler.js';
import { createMascotController, getMascotThemeSettings, setTerminatorMode } from '../shared/mascot.js';
import { ANIMATIONS } from '../shared/mascot-animations.js';
import { waitForTauri } from '../shared/tauri-init.js';

const _t0 = performance.now();
const _ts = (label) => console.log(`⏱ [${(performance.now() - _t0).toFixed(0)}ms] ${label}`);

waitForTauri(async ({ invoke, appWindow, listen }) => {
    _ts('Tauri ready');

    initMarkdown();
    initThemeListener();
    initLinkHandler(invoke);
    loadAndApplyTheme(invoke);
    _ts('Theme + markdown initialized');

    // Re-apply theme and opacity when config changes
    listen('config_updated', async () => {
        await loadAndApplyTheme(invoke);

        // Refresh terminator mode (may have been toggled in settings)
        let newTerminator = false;
        try { newTerminator = await invoke('is_terminator_mode'); } catch {}
        if (newTerminator !== isTerminator) {
            isTerminator = newTerminator;
            setTerminatorMode(isTerminator);
        }
        // Always refresh mascot — theme change may affect outline color
        await refreshFloatingMascot();
    });

    const app = new FloatingApp(invoke, appWindow, listen);
    window._floatingApp = app; // Expose for permission modal resize
    // Extension manager will be set asynchronously after extensions load in background
    app._onExtensionsReady = () => setMarkdownExtManager(app.extensionManager);
    app.init();

    // Set up mascot — use terminator variant if terminator mode is active
    let isTerminator = false;
    try { isTerminator = await invoke('is_terminator_mode'); } catch {}
    setTerminatorMode(isTerminator);

    async function refreshFloatingMascot() {
        const mascotContainer = document.getElementById('floatingMascot');
        if (!mascotContainer) return;
        // Destroy existing mascot controller if any
        if (window._kageMascot) { window._kageMascot.destroy(); window._kageMascot = null; }
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
            const { outlineColor, invert } = getMascotThemeSettings();
            const mascotCtrl = createMascotController(mascotContainer, {
                size: 40,
                idle: ANIMATIONS.waving,
                periodic: ANIMATIONS.waving,
                periodicInterval: 10000,
                periodicJitter: 2000,
                invert,
                outline: { color: outlineColor, radius: 2 },
                preload: [ANIMATIONS.jumping],
            });
            window._kageMascot = mascotCtrl;
        }
    }
    await refreshFloatingMascot();
});
