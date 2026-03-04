/**
 * Store Settings Module — auto-update and store URL configuration.
 */
class StoreSettingsModule extends SettingsModule {
    constructor() {
        super('store', 'Extension Store', '🛍️');
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
                    'Store URL',
                    'Custom store URL for browsing and installing extensions. Leave empty for the default store.',
                    '<input type="text" class="setting-input" id="storeUrl" placeholder="https://your-store.example.com">'
                )}

                <div class="setting-row">
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
    }

    save(config) {
        config.auto_update_extensions = document.getElementById('autoUpdateExtensions')?.checked ?? false;
        const url = document.getElementById('storeUrl')?.value?.trim() || '';
        config.store_url = url || null;
    }

    initialize() {
        document.getElementById('openStoreBtn')?.addEventListener('click', () => {
            if (window.__TAURI__?.core) {
                window.__TAURI__.core.invoke('open_store_window', { tab: null });
            }
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

    validate() { return { valid: true }; }
}
