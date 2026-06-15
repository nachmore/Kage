import { SettingsModule } from './base.js';
import { getSettingsManager, registerSettingsActions } from './module-registry.js';
import { t } from '../shared/i18n.js';
/**
 * Store Settings Module — auto-update, primary store URL, and additional store sources.
 */
export class StoreSettingsModule extends SettingsModule {
    constructor() {
        super('store', t('settings.store.title'), '🛍️');
        this._sources = [];
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                <div class="setting-row">
                    <button class="setting-button" id="openStoreBtn" style="font-size:12px;">${t('settings.store.open_btn')}</button>
                </div>

                <div class="setting-section-label">${t('settings.store.updates.section')}</div>

                ${this.createCheckboxRow(
                    t('settings.store.auto_update.label'),
                    t('settings.store.auto_update.description'),
                    'autoUpdateExtensions',
                    false
                )}

                <div class="setting-row" style="margin-top: 4px;">
                    <button class="setting-button" id="checkUpdatesBtn" style="font-size:12px;">${t('settings.store.update_all_btn')}</button>
                    <span id="updateCheckStatus" style="font-size:12px;color:var(--kage-text-muted);margin-left:8px;"></span>
                </div>

                <div class="setting-section-label">${t('settings.store.url.section')}</div>

                ${this.createControlRow(
                    t('settings.store.url.label'),
                    t('settings.store.url.description'),
                    '<input type="text" class="setting-input" id="storeUrl" placeholder="https://your-store.example.com">'
                )}

                <div class="setting-section-label">${t('settings.store.sources.section')}</div>
                <p style="font-size: 12px; color: var(--kage-text-muted); margin: 4px 0 12px; line-height: 1.4;">
                    ${t('settings.store.sources.description')}
                </p>
                <div id="storeSources"></div>
                <button class="setting-button" id="addSourceBtn" style="font-size:12px;margin-top:8px;">${t('settings.store.sources.add_btn')}</button>
            </div>
        `;
    }

    load(config) {
        const autoUpdate = document.getElementById('autoUpdateExtensions');
        const storeUrl = document.getElementById('storeUrl');
        if (autoUpdate) autoUpdate.checked = config.auto_update_extensions === true;
        if (storeUrl) storeUrl.value = config.store_url || '';

        this._sources = (config.store_sources || []).map((s) => ({ ...s }));
        this._renderSources();
    }

    save(config) {
        config.auto_update_extensions =
            document.getElementById('autoUpdateExtensions')?.checked ?? false;
        const url = document.getElementById('storeUrl')?.value?.trim() || '';
        config.store_url = url || null;

        // Collect sources from DOM
        config.store_sources = [];
        document.querySelectorAll('.store-source-row').forEach((row) => {
            const name = row.querySelector('.source-name')?.value?.trim();
            const url = row.querySelector('.source-url')?.value?.trim();
            const enabled = row.querySelector('.source-enabled')?.checked ?? true;
            if (name && url) {
                config.store_sources.push({ name, url, enabled });
            }
        });
    }

    initialize() {
        document.getElementById('openStoreBtn')?.addEventListener('click', () => {
            if (window.__TAURI__?.core) {
                window.__TAURI__.core.invoke('open_store_window', { tab: null });
            }
        });

        document.getElementById('addSourceBtn')?.addEventListener('click', () => {
            this._sources.push({ name: '', url: '', enabled: true });
            this._renderSources();
        });

        document.getElementById('checkUpdatesBtn')?.addEventListener('click', async () => {
            // The in-section span carries the transient "Checking…" hint.
            // The *result* goes through the manager's persistent status
            // banner instead: updating extensions emits extensions_changed,
            // which rebuilds the settings sections (settingsModules) —
            // detaching this span before we could write to it. The banner
            // lives outside that container, so it survives the rebuild and
            // is visible regardless of which section we land on.
            const status = document.getElementById('updateCheckStatus');
            if (status) status.textContent = t('settings.store.update_check.checking');
            const showResult = (msg, kind) => {
                if (status) status.textContent = '';
                getSettingsManager()?.showStatus(msg, kind);
            };
            try {
                const invoke = window.__TAURI__?.core?.invoke;
                if (!invoke) return;
                const result = await invoke('check_extension_updates');
                if (result.updated > 0) {
                    showResult(
                        t('settings.store.update_check.updated', { count: result.updated }),
                        'success'
                    );
                } else {
                    showResult(t('settings.store.update_check.up_to_date'), 'success');
                }
            } catch (e) {
                showResult(t('settings.store.update_check.failed', { reason: String(e) }), 'error');
            }
        });
    }

    _renderSources() {
        const container = document.getElementById('storeSources');
        if (!container) return;
        const enabledTitle = t('settings.store.sources.row.enabled_title');
        const namePlaceholder = t('settings.store.sources.row.name_placeholder');
        container.innerHTML = this._sources
            .map(
                (s, _i) => `
            <div class="store-source-row" style="display:flex;gap:8px;align-items:center;margin-bottom:6px;">
                <input type="checkbox" class="source-enabled" ${s.enabled ? 'checked' : ''} title="${enabledTitle}">
                <input type="text" class="setting-input source-name" value="${this._esc(s.name)}" placeholder="${namePlaceholder}" style="width:120px;">
                <input type="text" class="setting-input source-url" value="${this._esc(s.url)}" placeholder="https://store.example.com" style="flex:1;">
                <button class="setting-button" style="font-size:11px;padding:4px 8px;" data-action="store.removeSourceRow">✕</button>
            </div>
        `
            )
            .join('');
    }

    _esc(s) {
        return String(s || '')
            .replace(/&/g, '&amp;')
            .replace(/"/g, '&quot;')
            .replace(/</g, '&lt;');
    }

    validate() {
        return { valid: true };
    }
}

// Register the store section's row-removal handler with the delegated
// dispatcher. The button used to carry an inline `onclick="this.closest(
// '.store-source-row').remove()"` — same behavior, expressed once.
registerSettingsActions({
    'store.removeSourceRow': (_arg, el) => {
        el.closest('.store-source-row')?.remove();
    },
});
