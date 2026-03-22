/**
 * Store Settings Module — auto-update, primary store URL, and additional store sources.
 */
class StoreSettingsModule extends SettingsModule {
    constructor() {
        super('store', 'Extension Store', '🛍️');
        this._sources = [];
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                ${this.createCheckboxRow(
                    'Auto-Update Extensions',
                    'Automatically check for and install extension updates.',
                    'autoUpdateExtensions',
                    false
                )}

                ${this.createControlRow(
                    'Primary Store URL',
                    'Main store URL for browsing and installing extensions. Leave empty for the default store.',
                    '<input type="text" class="setting-input" id="storeUrl" placeholder="https://your-store.example.com">'
                )}

                <div class="setting-section-label" style="margin-top: 16px; font-weight: 600; font-size: 13px;">Additional Store Sources</div>
                <p style="font-size: 12px; color: var(--kiro-text-muted); margin: 4px 0 12px; line-height: 1.4;">
                    Add extra stores (e.g. an internal company store). Items from all enabled sources are merged in the store browser.
                </p>
                <div id="storeSources"></div>
                <button class="setting-button" id="addSourceBtn" style="font-size:12px;margin-top:8px;">+ Add Source</button>

                <div class="setting-row" style="margin-top: 16px;">
                    <button class="setting-button" id="openStoreBtn" style="font-size:12px;">🛍️ Open Extension Store...</button>
                    <button class="setting-button" id="checkUpdatesBtn" style="font-size:12px;margin-left:8px;">🔄 Check for Updates Now</button>
                    <span id="updateCheckStatus" style="font-size:12px;color:var(--kiro-text-muted);margin-left:8px;"></span>
                </div>
            </div>
        `;
    }

    load(config) {
        const autoUpdate = document.getElementById('autoUpdateExtensions');
        const storeUrl = document.getElementById('storeUrl');
        if (autoUpdate) autoUpdate.checked = config.auto_update_extensions === true;
        if (storeUrl) storeUrl.value = config.store_url || '';

        this._sources = (config.store_sources || []).map(s => ({ ...s }));
        this._renderSources();
    }

    save(config) {
        config.auto_update_extensions = document.getElementById('autoUpdateExtensions')?.checked ?? false;
        const url = document.getElementById('storeUrl')?.value?.trim() || '';
        config.store_url = url || null;

        // Collect sources from DOM
        config.store_sources = [];
        document.querySelectorAll('.store-source-row').forEach(row => {
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
            const status = document.getElementById('updateCheckStatus');
            if (status) status.textContent = 'Checking...';
            try {
                const invoke = window.__TAURI__?.core?.invoke;
                if (!invoke) return;
                const result = await invoke('check_extension_updates');
                if (status) {
                    if (result.updated > 0) {
                        status.textContent = `✓ Updated ${result.updated} extension${result.updated > 1 ? 's' : ''}`;
                        status.style.color = 'var(--kiro-accent)';
                    } else {
                        status.textContent = '✓ All extensions up to date';
                        status.style.color = 'var(--kiro-accent)';
                    }
                    setTimeout(() => { if (status) status.textContent = ''; }, 5000);
                }
            } catch (e) {
                if (status) {
                    status.textContent = '✗ ' + e;
                    status.style.color = '#e55';
                    setTimeout(() => { if (status) status.textContent = ''; }, 5000);
                }
            }
        });
    }

    _renderSources() {
        const container = document.getElementById('storeSources');
        if (!container) return;
        container.innerHTML = this._sources.map((s, i) => `
            <div class="store-source-row" style="display:flex;gap:8px;align-items:center;margin-bottom:6px;">
                <input type="checkbox" class="source-enabled" ${s.enabled ? 'checked' : ''} title="Enable/disable this source">
                <input type="text" class="setting-input source-name" value="${this._esc(s.name)}" placeholder="Name" style="width:120px;">
                <input type="text" class="setting-input source-url" value="${this._esc(s.url)}" placeholder="https://store.example.com" style="flex:1;">
                <button class="setting-button" style="font-size:11px;padding:4px 8px;" onclick="this.closest('.store-source-row').remove()">✕</button>
            </div>
        `).join('');
    }

    _esc(s) {
        return String(s || '').replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;');
    }

    validate() { return { valid: true }; }
}
