/**
 * Extension Store window logic.
 *
 * Loaded as an ES module from `store.html`. Sets up theme, owns the
 * tab/search state, and orchestrates install/update/uninstall flows
 * (including the install-time permission prompt).
 *
 * Exposes a small `window.__kageStore` facade so the inline `onclick`
 * attributes on store cards can route through it. The cleaner path
 * would be a `data-action` dispatcher like the settings window, but
 * the store is a single page with simple actions and direct calls
 * are easier to follow.
 */

import { errLabel, errMessage } from './shared/error-message.js';
import { initI18n, applyStaticTranslations, t } from './shared/i18n.js';
import { showPermissionPrompt } from './shared/permission-prompt.js';
import { cmdOrCtrlPressed } from './shared/shortcuts.js';
import { applyTheme, initThemeListener, loadAndApplyTheme } from './shared/theme.js';

let currentTab = 'extensions';
let showInstalledOnly = false;
let installedMap = new Map(); // id → { version, kind }
const _bundledIds = new Set();
let _isDevMode = false;
let _hasMultipleSources = false;

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

    // Load bundled extension IDs
    try {
        const resp = await fetch('extensions/bundled.json');
        if (resp.ok) {
            const list = await resp.json();
            for (const e of list) _bundledIds.add(e.id);
        }
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
}

async function refreshInstalled() {
    const invoke = window.__TAURI__.core.invoke;
    installedMap = new Map();
    // Capture enough manifest fields up front to render meaningful cards
    // when the store is offline. The store window's "browse online" view
    // normally pulls these from the catalog; offline we synthesise rows
    // from this same map so the user can still see what they have.
    const captureManifest = (e, kind) => ({
        version: e.manifest.version,
        kind,
        hasSettings: !!e.manifest.contributes?.settingsProvider,
        name: e.manifest.name,
        description: e.manifest.description || '',
        icon: e.manifest.icon || '',
        author: e.manifest.author || null,
        permissions: Array.isArray(e.manifest.permissions) ? e.manifest.permissions : [],
    });
    try {
        const exts = await invoke('list_extensions');
        exts.forEach((e) => installedMap.set(e.manifest.id, captureManifest(e, 'extension')));
    } catch {}
    try {
        const themes = await invoke('list_themes');
        themes.forEach((e) => installedMap.set(e.manifest.id, captureManifest(e, 'theme')));
    } catch {}
    try {
        const packs = await invoke('list_command_packs');
        packs.forEach((e) => installedMap.set(e.manifest.id, captureManifest(e, 'commands')));
    } catch {}
    _bundledIds.forEach((id) => {
        if (!installedMap.has(id)) {
            installedMap.set(id, {
                version: '0.0.0',
                kind: 'extension',
                hasSettings: false,
                name: id,
                description: '',
                icon: '',
                author: null,
                permissions: [],
            });
        }
    });
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

    try {
        const catalog = await invoke('store_get_catalog', {
            kind,
            search: search || null,
            page: 1,
            source: sourceFilter || null,
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
    const isBundled = _bundledIds.has(item.id);
    const updateAvailable = isInstalled && hasUpdate(item.id, item.version);
    const localInfo = installedMap.get(item.id);
    const itemSource = item._source || '';

    // Build action buttons
    let actionHtml = '';
    if (isBundled) {
        actionHtml = '<span class="store-card-btn installed">Built-in</span>';
    } else if (isInstalled) {
        const buttons = [];

        // Settings deep link (if extension has a settings module)
        if (localInfo?.hasSettings) {
            buttons.push(
                `<button class="store-card-btn settings-link" onclick="event.stopPropagation();window.__kageStore.openSettings('${esc(item.id)}')" title="Open settings">⚙️</button>`
            );
        }

        // Update button
        if (updateAvailable) {
            const localVer = localInfo?.version || '';
            buttons.push(
                `<button class="store-card-btn update" onclick="event.stopPropagation();window.__kageStore.updateItem('${esc(item.id)}')" title="${esc(localVer)} → ${esc(item.version)}">Update</button>`
            );
        }

        // Reinstall button (dev mode only)
        if (_isDevMode) {
            buttons.push(
                `<button class="store-card-btn reinstall" onclick="event.stopPropagation();window.__kageStore.reinstallItem('${esc(item.id)}')" title="Re-download and reinstall">🔄</button>`
            );
        }

        // Uninstall button
        buttons.push(
            `<button class="store-card-btn uninstall" onclick="event.stopPropagation();window.__kageStore.uninstallItem('${esc(item.id)}','${kind}')">Uninstall</button>`
        );

        actionHtml = `<div class="store-card-actions">${buttons.join('')}</div>`;
    } else {
        actionHtml = `<button class="store-card-btn install" onclick="event.stopPropagation();window.__kageStore.installFromStore('${esc(item.id)}')">Install</button>`;
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
    const card = document.querySelector(`.store-card[data-item-id="${id}"]`);
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
async function runStagedInstall(stager, { onSuccess } = {}) {
    const invoke = window.__TAURI__.core.invoke;

    // Pre-fetch current config so we can detect upgrades.
    let priorGrant = null;
    try {
        const cfg = await invoke('get_config');
        priorGrant = cfg?.extension_grants || {};
    } catch {
        priorGrant = {};
    }

    const item = await stager();
    const manifest = item?.manifest;
    if (!manifest?.id) {
        throw new Error('install returned no manifest');
    }

    const existing = priorGrant[manifest.id] || null;
    const previouslyGranted = Array.isArray(existing?.granted) ? existing.granted : [];
    const isUpgrade = !!existing;

    // Requested capabilities per manifest (normalized). If the update
    // asks for exactly the same set (or a subset of) what was
    // previously approved, skip the re-prompt — this is a no-change
    // update.
    const requested = Array.isArray(manifest.permissions) ? manifest.permissions : [];
    const grantedSet = new Set(previouslyGranted);
    const expandsCaps = requested.some((cap) => !grantedSet.has(cap));

    let decision;
    if (isUpgrade && !expandsCaps) {
        // Auto-approve: no capability expansion, user already said yes.
        decision = { approved: true, granted: requested };
    } else {
        decision = await showPermissionPrompt(manifest, {
            isUpgrade,
            previouslyGranted,
        });
    }

    if (!decision.approved) {
        // Roll back: the staged files are on disk but nothing has
        // loaded the extension yet (commit was never called, so no
        // extensions_changed fired). Uninstall to remove them.
        try {
            await invoke('uninstall_extension', {
                id: manifest.id,
                kind: manifest.type || 'extension',
            });
        } catch (e) {
            console.warn('Rollback uninstall failed:', e);
        }
        return { cancelled: true };
    }

    await invoke('commit_extension_install', {
        extensionId: manifest.id,
        granted: decision.granted,
        approvedVersion: manifest.version || '',
    });
    if (onSuccess) await onSuccess();
    return { cancelled: false, item };
}

async function installFromStore(id) {
    const invoke = window.__TAURI__.core.invoke;
    setCardBusy(id, 'Installing…');
    try {
        const result = await runStagedInstall(() => invoke('store_install', { id }), {
            onSuccess: async () => {
                await refreshInstalled();
            },
        });
        if (result.cancelled) {
            renderTab();
        } else {
            renderTab();
        }
    } catch (e) {
        alert(errLabel('Install failed', e));
        renderTab();
    }
}

async function updateItem(id) {
    const invoke = window.__TAURI__.core.invoke;
    setCardBusy(id, 'Updating…');
    try {
        await runStagedInstall(() => invoke('store_install', { id }), {
            onSuccess: async () => {
                await refreshInstalled();
            },
        });
        renderTab();
    } catch (e) {
        alert(errLabel('Update failed', e));
        renderTab();
    }
}

async function reinstallItem(id) {
    const invoke = window.__TAURI__.core.invoke;
    setCardBusy(id, 'Reinstalling…');
    try {
        await runStagedInstall(() => invoke('store_install', { id }), {
            onSuccess: async () => {
                await refreshInstalled();
            },
        });
        renderTab();
    } catch (e) {
        alert(errLabel('Reinstall failed', e));
        renderTab();
    }
}

async function uninstallItem(id, kind) {
    if (!confirm(t('store.uninstall.confirm', { id }))) return;
    const invoke = window.__TAURI__.core.invoke;
    setCardBusy(id, 'Removing…');
    try {
        await invoke('uninstall_extension', { id, kind });
        await refreshInstalled();
        renderTab();
    } catch (e) {
        alert(errLabel('Uninstall failed', e));
        renderTab();
    }
}

function openSettings(extensionId) {
    const invoke = window.__TAURI__.core.invoke;
    invoke('open_settings_window', { section: extensionId });
}

let searchTimeout;
function onSearch() {
    clearTimeout(searchTimeout);
    searchTimeout = setTimeout(() => renderTab(), 300);
}

function esc(s) {
    const d = document.createElement('div');
    d.textContent = s;
    return d.innerHTML;
}

// --- Wire up DOM event handlers --------------------------------------------

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
};

waitForTauri(() => {
    init();
});
