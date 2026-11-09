/**
 * Entry point of the extension view page (`ui/extension-view.html`), the
 * page every `ext-view:<extension>:<view>` window loads
 * (docs/design/EXTENSION_VIEWS.md §8).
 *
 * Wiring only: it hands the real modules to `page.js`, which does the work
 * and is what the tests drive.
 */

import { getConfig, onConfigChange } from '../shared/config-cache.js';
import { installExtensionDataEvents } from '../shared/extension-data-events.js';
import {
    installFileTextBroker,
    isWindowShown,
    strictUserActivation,
} from '../shared/extension-file-text.js';
import { ExtensionSandbox } from '../shared/extension-sandbox-host.js';
import {
    isSafeTokenValue,
    normalizeViews,
    sanitizeViewTitle,
} from '../shared/extension-view-policy.js';
import { applyStaticTranslations, initI18n, t } from '../shared/i18n.js';
import { initLinkHandler } from '../shared/link-handler.js';
import { waitForTauri } from '../shared/tauri-init.js';
import { initThemeListener, loadAndApplyTheme } from '../shared/theme.js';
import { createViewPage } from './page.js';

/**
 * Host features this window offers a view (§9.4). The view sandbox gets the
 * ones its grant allows (`hostFeaturesFor`). Each design that adds a
 * feature to the other windows adds it here too (SYNC.md §23.2), except
 * `agent.background`: background agent work isn't a view's (§8.6).
 */
const OFFERED_HOST_FEATURES = ['views', 'storage.change_events', 'files.pick_text'];

waitForTauri(({ invoke, appWindow, listen }) => {
    // Clicked links open in the system browser, never in this window.
    initLinkHandler(invoke);
    // Files the view asks the user for (EXTENSION_FILE_TEXT.md §5.2, §5.3):
    // the dialog is attached to this window, and opens at once only right
    // after a click or key press in the view; else the ask bar shows.
    installFileTextBroker({
        listen,
        isVisible: () => isWindowShown(appWindow),
        activationLease: strictUserActivation,
    });

    const page = createViewPage({
        invoke,
        listen,
        appWindow,
        t,
        initI18n: () => initI18n(invoke),
        applyStaticTranslations,
        applyTheme: () => loadAndApplyTheme(invoke),
        initThemeListener,
        getConfig: () => getConfig(invoke),
        onConfigChange,
        createSandbox: (spec, container) => new ExtensionSandbox(spec, invoke, container),
        installDataEvents: installExtensionDataEvents,
        policy: { normalizeViews, sanitizeViewTitle, isSafeTokenValue },
        hostFeatures: OFFERED_HOST_FEATURES,
        userActivation: () => navigator.userActivation,
        hasFocus: () => document.hasFocus(),
    });
    page.start().catch((e) => console.error('[extension-view] page failed to start:', e));
});
