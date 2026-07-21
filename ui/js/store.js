/**
 * Extension Store window logic.
 *
 * Loaded as an ES module from `store.html`. Sets up theme, owns the
 * tab/search state, and orchestrates install/update/uninstall flows
 * (including the install-time permission prompt).
 *
 * Exposes a small `window.__kageStore` facade so the static inline
 * `onclick` attributes in store.html (tabs, refresh, filters) can route
 * through it. Store-card buttons — whose contents derive from untrusted
 * catalog JSON — use a `data-action` dispatcher instead; never interpolate
 * catalog fields into inline JS.
 */

import { alertDialog, confirmDialog } from './shared/confirm-dialog.js';
import { errMessage } from './shared/error-message.js';
import { escapeAttr } from './shared/tool-utils.js';
import { initI18n, applyStaticTranslations, t } from './shared/i18n.js';
import { runStagedExtensionInstall } from './shared/staged-extension-install.js';
import { cmdOrCtrlPressed } from './shared/shortcuts.js';
import { applyTheme, initThemeListener, loadAndApplyTheme } from './shared/theme.js';

let currentTab = 'extensions';
let showInstalledOnly = false;
let installedMap = new Map(); // id → { version, kind }
let _isDevMode = false;
let _hasMultipleSources = false;
// Set to true for the next renderBrowse() call only — bypasses GitHub
// Pages' edge cache via a query-param bust on the catalog fetch.
// Cleared as soon as the call goes out so subsequent renders (e.g.
// the search-input debounce) stay fast.
let _forceRefreshNext = false;

function waitForTauri(cb) {
    if (window.__TAURI__?.core) cb();
    else setTimeout(() => waitForTauri(cb), 50);
}

async function init() {
    const invoke = window.__TAURI__.core.invoke;

    try {
        await initI18n(invoke);
    } catch (e) {
        console.warn('[store] i18n init failed', e);
    }
    applyStaticTranslations(document);

    // Check dev mode
    try {
        _isDevMode = await invoke('is_dev_mode');
    } catch {}

    // Apply theme
    initThemeListener();
    try {
        await loadAndApplyTheme(invoke);
    } catch {
        applyTheme('system');
    }

    // Check if multiple sources configured
    try {
        const config = await invoke('get_config');
        _hasMultipleSources = (config.store_sources || []).filter((s) => s.enabled).length > 0;
    } catch {}

    // Initial tab from URL param
    const urlParams = new URLSearchParams(window.location.search);
    const initialTab = urlParams.get('tab');
    const installIntent = urlParams.get('install');

    await refreshInstalled();

    // Auto-update check on store open
    try {
        const config = await invoke('get_config');
        if (config.auto_update_extensions) {
            autoUpdateCheck();
        }
    } catch {}

    if (initialTab && ['extensions', 'themes', 'commands'].includes(initialTab)) {
        switchTab(initialTab);
    } else {
        renderTab();
    }

    // Deep-link install intent. The Rust deep-link handler routes
    // `kage://install/<id>` here via the URL query param. We wait for
    // the initial render to settle before triggering, otherwise
    // installFromStore tries to setCardBusy on a card that hasn't
    // mounted yet. The render path is async (renderBrowse fetches the
    // catalog), so we let one more microtask flush before kicking off.
    if (installIntent) {
        // requestAnimationFrame puts us after the next paint; the
        // catalog fetch in renderBrowse is what we're waiting on but
        // it's already in flight, and browsers schedule its completion
        // ahead of the next animation frame in practice.
        requestAnimationFrame(() => handleDeepLinkInstall(installIntent));
    }
}

/**
 * Deep-link install entry point. Called both:
 *   - from init() when the URL carries `?install=<id>` (cold launch
 *     from a `kage://install/<id>` click), and
 *   - from Rust via eval_script when the store window was already
 *     open (warm reuse — the URL change isn't reflected, so we
 *     re-enter from the host side).
 *
 * Behaviour:
 *   - If the extension is already installed, scroll to the row and
 *     surface a brief flash so the user sees their click did
 *     something. Don't re-prompt — they have it.
 *   - Otherwise, scroll to the card and call installFromStore which
 *     stages → prompts for capability approval → commits.
 *   - If the id isn't in the catalog (typo in the URL, the catalog
 *     hasn't reloaded yet, etc.) we just log and let the user
 *     discover it.
 */
async function handleDeepLinkInstall(rawId) {
    if (typeof rawId !== 'string') return;
    // Defence in depth — Rust already validated, but the eval_script
    // path is a separate trust boundary.
    const id = rawId.replace(/[^a-z0-9_-]/gi, '');
    if (!id) return;
    console.log('[store] deep-link install intent:', id);

    // Make sure we're on the extensions tab — themes and commands
    // can't be the target of an install URL today.
    if (currentTab !== 'extensions') {
        switchTab('extensions');
        // switchTab kicks off renderTab; wait one frame so the new
        // grid is in the DOM before we look for the card.
        await new Promise((r) => requestAnimationFrame(r));
    }

    // Wait briefly for the card to appear. The catalog fetch may
    // still be in flight; poll for ~3 seconds, then give up.
    const card = await waitForCard(id, 3000);
    if (card) {
        card.scrollIntoView({ behavior: 'smooth', block: 'center' });
        // Visible cue so the user sees they're in the right place.
        card.classList.add('store-card-deep-link');
        setTimeout(() => card.classList.remove('store-card-deep-link'), 1200);
    }

    if (installedMap.has(id)) {
        console.log(`[store] '${id}' already installed; not re-prompting`);
        return;
    }
    try {
        await installFromStore(id);
    } catch (e) {
        console.warn('[store] deep-link install failed', e);
    }
}

function waitForCard(id, timeoutMs) {
    const sel = `.store-card[data-item-id="${CSS.escape(id)}"]`;
    const found = document.querySelector(sel);
    if (found) return Promise.resolve(found);
    return new Promise((resolve) => {
        const start = Date.now();
        const tick = () => {
            const el = document.querySelector(sel);
            if (el) return resolve(el);
            if (Date.now() - start > timeoutMs) return resolve(null);
            setTimeout(tick, 100);
        };
        tick();
    });
}

async function refreshInstalled() {
    const invoke = window.__TAURI__.core.invoke;
    installedMap = new Map();
    // Capture enough manifest fields up front to render meaningful cards
    // when the store is offline. The store window's "browse online" view
    // normally pulls these from the catalog; offline we synthesise rows
    // from this same map so the user can still see what they have.
    //
    // Resolve `__MSG_*__` tokens in name/description by reading the
    // extension's _locales/<lang>/messages.json. Without this the
    // offline view shows raw tokens for any extension that uses the
    // Chrome convention, which looks like "this install is broken."
    const captureManifest = async (e, kind) => {
        const localized = await localizeManifestForPrompt(invoke, e.manifest);
        return {
            version: e.manifest.version,
            kind,
            hasSettings: !!e.manifest.contributes?.settingsProvider,
            name: localized.name,
            description: localized.description || '',
            icon: e.manifest.icon || '',
            author: e.manifest.author || null,
            permissions: Array.isArray(e.manifest.permissions) ? e.manifest.permissions : [],
        };
    };
    const collect = async (cmd, kind) => {
        try {
            const items = await invoke(cmd);
            const captured = await Promise.all(items.map((e) => captureManifest(e, kind)));
            items.forEach((e, i) => installedMap.set(e.manifest.id, captured[i]));
        } catch {}
    };
    await Promise.all([
        collect('list_extensions', 'extension'),
        collect('list_themes', 'theme'),
        collect('list_command_packs', 'commands'),
    ]);
}

function hasUpdate(itemId, remoteVersion) {
    const local = installedMap.get(itemId);
    if (!local) return false;
    try {
        const lv = local.version.split('.').map(Number);
        const rv = remoteVersion.split('.').map(Number);
        for (let i = 0; i < 3; i++) {
            if ((rv[i] || 0) > (lv[i] || 0)) return true;
            if ((rv[i] || 0) < (lv[i] || 0)) return false;
        }
    } catch {}
    return false;
}

async function autoUpdateCheck() {
    const invoke = window.__TAURI__.core.invoke;
    try {
        const result = await invoke('check_extension_updates');
        if (result.updated > 0) {
            await refreshInstalled();
            renderTab();
        }
    } catch {}
}

function switchTab(tab) {
    currentTab = tab;
    document.querySelectorAll('.store-tab').forEach((t) => {
        t.classList.toggle('active', t.dataset.tab === tab);
    });
    renderTab();
}

function _toggleInstalledFilter() {
    showInstalledOnly = !showInstalledOnly;
    document.getElementById('installedFilter').classList.toggle('active', showInstalledOnly);
    renderTab();
}

let _refreshing = false;
async function refresh() {
    if (_refreshing) return;
    _refreshing = true;
    const btn = document.getElementById('storeRefreshBtn');
    if (btn) {
        btn.classList.add('is-refreshing');
        btn.setAttribute('disabled', '');
    }
    // Refresh both sides:
    //   - the local installed list, in case the user side-loaded
    //     something via the .zip flow since opening the store, and
    //   - the remote catalog, with a cache-bust so GitHub Pages'
    //     edge cache doesn't serve us a stale catalog.json (the
    //     CDN can lag by minutes after a publish).
    _forceRefreshNext = true;
    try {
        await refreshInstalled();
        renderTab(); // kicks off renderBrowse(), which consumes _forceRefreshNext
    } finally {
        // The spinner stays on for a beat after the renderTab() call
        // returns so the user gets visual confirmation that something
        // actually happened. renderBrowse() itself is async — by the
        // time we land here it has fired off the request but may not
        // have painted the result yet.
        setTimeout(() => {
            if (btn) {
                btn.classList.remove('is-refreshing');
                btn.removeAttribute('disabled');
            }
            _refreshing = false;
        }, 600);
    }
}

function renderTab() {
    const content = document.getElementById('storeContent');
    content.innerHTML =
        '<div class="store-empty"><div class="store-loading-spinner"></div>Loading...</div>';
    renderBrowse(content, currentTab);
}

async function renderBrowse(container, type) {
    const invoke = window.__TAURI__.core.invoke;
    const search = document.getElementById('storeSearch')?.value || '';
    const kind = type === 'commands' ? 'commands' : type === 'themes' ? 'theme' : 'extension';
    const sourceFilter = document.getElementById('sourceFilter')?.value || '';

    const forceRefresh = _forceRefreshNext;
    _forceRefreshNext = false;
    try {
        const catalog = await invoke('store_get_catalog', {
            kind,
            search: search || null,
            page: 1,
            source: sourceFilter || null,
            forceRefresh,
        });
        let items = catalog.items || [];

        // Update source filter dropdown
        const sources = catalog.sources || [];
        updateSourceFilter(sources);

        // Offline degradation. When every configured store source failed
        // (typically: the user has no network), the backend returns
        // `offline: true` with an empty items list. Fall back to showing
        // the user's installed items by synthesising "catalog" rows from
        // their on-disk extensions, and render a banner pointing them at
        // the store when they reconnect.
        let offlineBanner = '';
        if (catalog.offline) {
            offlineBanner = `<div class="store-offline-banner">
                <span class="store-offline-banner-icon">📡</span>
                <span class="store-offline-banner-text">
                    Couldn't reach the extension store. Showing only what's
                    already installed — browse the full catalog when you're
                    back online.
                </span>
            </div>`;
            // Synthesise catalog entries from the local installed items so
            // users can still see, configure, and uninstall what they have.
            const localItems = [];
            for (const [id, info] of installedMap.entries()) {
                if (info && info.kind === kind) {
                    localItems.push({
                        id,
                        type: kind,
                        name: info.name || id,
                        version: info.version || '',
                        author: info.author || null,
                        description: info.description || '',
                        icon: info.icon || '📦',
                        permissions: info.permissions || [],
                        tags: [],
                        _local: true,
                    });
                }
            }
            items = localItems;
        }

        if (showInstalledOnly) {
            items = items.filter((i) => installedMap.has(i.id));
        }

        if (items.length === 0) {
            const msg = catalog.offline
                ? `You don't have any ${type} installed yet, and the store can't be reached right now.`
                : showInstalledOnly
                  ? `No installed ${type} found.`
                  : `No ${type} found in the store.`;
            container.innerHTML = `${offlineBanner}<div class="store-empty"><div class="store-empty-icon">${catalog.offline ? '📡' : '🔍'}</div>${msg}</div>`;
            return;
        }

        let html = offlineBanner + '<div class="store-grid">';
        for (const item of items) {
            html += renderCard(item, kind);
        }
        html += '</div>';
        container.innerHTML = html;
    } catch (e) {
        container.innerHTML = `<div class="store-empty"><div class="store-empty-icon">⚠️</div>Could not connect to store.<br>${esc(errMessage(e))}</div>`;
    }
}

function renderCard(item, kind) {
    const isInstalled = installedMap.has(item.id);
    const updateAvailable = isInstalled && hasUpdate(item.id, item.version);
    const localInfo = installedMap.get(item.id);
    const itemSource = item._source || '';

    // Build action buttons. Catalog fields are untrusted (remote JSON from
    // user-addable store sources), so the item id is never interpolated into
    // inline JS — buttons carry a data-action and the delegated click
    // handler on #storeContent reads the id from the card's data-item-id.
    let actionHtml = '';
    if (isInstalled) {
        const buttons = [];

        // Settings deep link (if extension has a settings module)
        if (localInfo?.hasSettings) {
            buttons.push(
                `<button class="store-card-btn settings-link" data-action="settings" title="Open settings">⚙️</button>`
            );
        }

        // Update button
        if (updateAvailable) {
            const localVer = localInfo?.version || '';
            buttons.push(
                `<button class="store-card-btn update" data-action="update" title="${esc(localVer)} → ${esc(item.version)}">Update</button>`
            );
        }

        // Reinstall button (dev mode only)
        if (_isDevMode) {
            buttons.push(
                `<button class="store-card-btn reinstall" data-action="reinstall" title="Re-download and reinstall">🔄</button>`
            );
        }

        // Uninstall button
        buttons.push(
            `<button class="store-card-btn uninstall" data-action="uninstall" data-kind="${esc(kind)}">Uninstall</button>`
        );

        actionHtml = `<div class="store-card-actions">${buttons.join('')}</div>`;
    } else {
        actionHtml = `<button class="store-card-btn install" data-action="install">Install</button>`;
    }

    // Source badge (only show when multiple sources exist)
    const sourceBadge =
        _hasMultipleSources && itemSource
            ? `<span class="store-source-badge">${esc(itemSource)}</span>`
            : '';

    return `
        <div class="store-card" data-item-id="${esc(item.id)}">
            <div class="store-card-header">
                <div class="store-card-icon">${esc(item.icon || '📦')}</div>
                <div>
                    <div class="store-card-title">${esc(item.name)}</div>
                    <div class="store-card-author">${esc(item.author || '')} · v${esc(item.version)}</div>
                </div>
            </div>
            <div class="store-card-tags">
                ${sourceBadge}
                ${(item.tags || []).map((t) => `<span class="store-tag">${esc(t)}</span>`).join('')}
            </div>
            <div class="store-card-desc">${esc(item.description || '')}</div>
            <div class="store-card-footer">
                ${actionHtml}
            </div>
        </div>
    `;
}

function updateSourceFilter(sources) {
    const select = document.getElementById('sourceFilter');
    if (!select) return;
    if (sources.length <= 1) {
        select.style.display = 'none';
        _hasMultipleSources = false;
        return;
    }
    _hasMultipleSources = true;
    select.style.display = '';
    const current = select.value;
    select.innerHTML =
        '<option value="">All Sources</option>' +
        sources
            .map(
                (s) =>
                    `<option value="${esc(s)}"${s === current ? ' selected' : ''}>${esc(s)}</option>`
            )
            .join('');
}

// --- Actions ---

function setCardBusy(id, label) {
    const card = document.querySelector(`.store-card[data-item-id="${CSS.escape(id)}"]`);
    if (!card) return;
    const footer = card.querySelector('.store-card-footer');
    if (footer)
        footer.innerHTML = `<span class="store-card-btn installed" style="opacity:0.7;">${label}</span>`;
}

/**
 * Shared install orchestration: stage (download + extract), prompt the user
 * for capability approval, either commit (writes grant + emits changed)
 * or roll back (uninstall). `stager` is a function that takes no args and
 * returns a Promise<InstalledItem> from the chosen Tauri command
 * (store_install or install_extension_from_path).
 *
 * For updates: we fetch the prior grant first so the prompt can mark
 * already-approved caps differently from new ones, and flag the
 * modal as an upgrade. When the updated manifest requests the same
 * capability set as before and the approval is implicit-same, we
 * skip the prompt and commit directly — no user friction for
 * routine updates that don't expand the capability surface.
 */
async function installFromStore(id) {
    const invoke = window.__TAURI__.core.invoke;
    setCardBusy(id, 'Installing…');
    try {
        const result = await runStagedExtensionInstall(
            invoke,
            () => invoke('store_install', { id }),
            {
                onSuccess: async () => {
                    await refreshInstalled();
                },
            }
        );
        if (result.cancelled) {
            renderTab();
        } else {
            renderTab();
        }
    } catch (e) {
        await alertDialog({
            icon: '⚠️',
            title: t('store.install.failed.title'),
            message: errMessage(e),
        });
        renderTab();
    }
}

async function updateItem(id) {
    const invoke = window.__TAURI__.core.invoke;
    setCardBusy(id, 'Updating…');
    try {
        await runStagedExtensionInstall(invoke, () => invoke('store_install', { id }), {
            onSuccess: async () => {
                await refreshInstalled();
            },
        });
        renderTab();
    } catch (e) {
        await alertDialog({
            icon: '⚠️',
            title: t('store.update.failed.title'),
            message: errMessage(e),
        });
        renderTab();
    }
}

async function reinstallItem(id) {
    const invoke = window.__TAURI__.core.invoke;
    setCardBusy(id, 'Reinstalling…');
    try {
        await runStagedExtensionInstall(invoke, () => invoke('store_install', { id }), {
            onSuccess: async () => {
                await refreshInstalled();
            },
        });
        renderTab();
    } catch (e) {
        await alertDialog({
            icon: '⚠️',
            title: t('store.reinstall.failed.title'),
            message: errMessage(e),
        });
        renderTab();
    }
}

async function uninstallItem(id, kind) {
    // Use the in-app modal rather than `window.confirm`. The browser
    // primitive misbehaves under WebView2 in Tauri 2 — the dialog
    // blocks the renderer's main thread but our async handler
    // still proceeds past `await` calls scheduled before the modal
    // opened, which has produced "Cancel doesn't actually cancel"
    // bugs in the wild. The themed dialog is async-clean: we await
    // the user's choice and only invoke uninstall on a `true`
    // resolve.
    const localInfo = installedMap.get(id);
    const displayName = localInfo?.name || id;
    const ok = await confirmDialog({
        icon: '🗑️',
        title: t('store.uninstall.confirm.title', { name: displayName }),
        message: t('store.uninstall.confirm.message'),
        confirmLabel: t('store.uninstall.confirm.btn'),
        cancelLabel: t('store.uninstall.cancel.btn'),
        destructive: true,
    });
    if (!ok) return;

    const invoke = window.__TAURI__.core.invoke;
    setCardBusy(id, 'Removing…');
    try {
        await invoke('uninstall_extension', { id, kind });
        await refreshInstalled();
        renderTab();
    } catch (e) {
        await alertDialog({
            icon: '⚠️',
            title: t('store.uninstall.failed.title'),
            message: errMessage(e),
        });
        renderTab();
    }
}

function openSettings(extensionId) {
    const invoke = window.__TAURI__.core.invoke;
    // Sandboxed extension settings modules register with id
    // `ext-<extensionId>` (see SandboxedExtensionSettingsModule in
    // settings/manager.js). switchSection() matches on that id; if
    // we passed the bare extension id the sidebar lookup would miss
    // and every `[data-section-content]` would end up hidden,
    // leaving the right pane empty.
    invoke('open_settings_window', { section: `ext-${extensionId}` });
}

let searchTimeout;
function onSearch() {
    clearTimeout(searchTimeout);
    searchTimeout = setTimeout(() => renderTab(), 300);
}

// Attribute-safe escaper: escapes `& < > " '` so untrusted catalog fields
// can't break out of quoted attributes. The old textContent/innerHTML trick
// left quotes intact, which was an XSS when interpolated into attributes.
function esc(s) {
    return escapeAttr(s);
}

// --- Wire up DOM event handlers --------------------------------------------

// Delegated click handler for store-card action buttons. Cards are rendered
// from untrusted catalog JSON, so the buttons carry data-action/data-kind and
// the item id lives only in the card's data-item-id attribute — never inside
// inline onclick JS (see renderCard).
document.getElementById('storeContent')?.addEventListener('click', (e) => {
    const btn = e.target.closest('.store-card-btn[data-action]');
    if (!btn) return;
    e.stopPropagation();
    const card = btn.closest('.store-card');
    const id = card?.dataset.itemId;
    if (!id) return;
    switch (btn.dataset.action) {
        case 'settings':
            openSettings(id);
            break;
        case 'update':
            updateItem(id);
            break;
        case 'reinstall':
            reinstallItem(id);
            break;
        case 'uninstall':
            uninstallItem(id, btn.dataset.kind);
            break;
        case 'install':
            installFromStore(id);
            break;
    }
});

document.addEventListener('keydown', (e) => {
    // Ctrl+W / ⌘+W — close window (triggers CloseRequested for activation policy update)
    if (cmdOrCtrlPressed(e) && e.key === 'w') {
        e.preventDefault();
        window.__TAURI__?.webviewWindow?.getCurrentWebviewWindow()?.close();
    }
});

// Expose action handlers on a single window facade so the inline
// onclick attributes generated in renderCard can reach them. Module
// scopes are isolated; we can't otherwise call these from inline HTML.
window.__kageStore = {
    switchTab,
    openSettings,
    installFromStore,
    updateItem,
    reinstallItem,
    uninstallItem,
    toggleInstalledFilter: _toggleInstalledFilter,
    onSearch,
    renderTab,
    refresh,
    handleDeepLinkInstall,
};
// Top-level alias for the eval_script path: the Rust deep-link
// handler injects `handleDeepLinkInstall('<id>')` into an
// already-open store window via webview.eval(), and a direct
// global is the simplest contract that doesn't depend on the
// __kageStore facade existing at the eval time.
window.handleDeepLinkInstall = handleDeepLinkInstall;

waitForTauri(() => {
    init();
});
