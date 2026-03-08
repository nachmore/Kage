/**
 * System Integration Settings Module
 */
class IntegrationSettingsModule extends SettingsModule {
    constructor() {
        super('integration', 'System Integration', getSystemIcon());
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                ${this.createCheckboxRow(
                    'Capture selected text',
                    'Grab selected text from the active window when the hotkey is pressed (uses Ctrl+C). Disable if this interferes with terminal apps or other programs.',
                    'captureSelection',
                    true
                )}

                ${this.createCheckboxRow(
                    'Show notifications',
                    'Show a system notification when a response completes while the window is hidden.',
                    'showNotifications',
                    true
                )}

                <div class="setting-row" style="padding-left: 28px;">
                    <button class="setting-button" id="testNotificationBtn">Test Notification</button>
                    <span class="setting-description" id="notificationStatus" style="margin-left: 8px;"></span>
                </div>

                <div class="setting-row" style="margin-top: 16px;">
                    <div class="setting-label">Quick Directory Access</div>
                    <div class="setting-description">Type any of these keywords in the floating window to open the folder. Prefix matching works too — e.g. "down" opens Downloads.</div>
                </div>
                <table class="dir-reference-table">
                    <thead><tr><th>Keyword</th><th>Aliases</th><th>Path</th></tr></thead>
                    <tbody id="dirReferenceBody"></tbody>
                </table>

                <style>
                    .dir-reference-table { width: 100%; border-collapse: collapse; font-size: 12px; margin: 8px 0 4px; }
                    .dir-reference-table th { text-align: left; padding: 6px 10px; color: var(--kiro-text-muted); font-weight: 500; border-bottom: 1px solid var(--kiro-border-subtle); }
                    .dir-reference-table td { padding: 5px 10px; border-bottom: 1px solid var(--kiro-border-subtle); color: var(--kiro-text); }
                    .dir-reference-table tr:last-child td { border-bottom: none; }
                    .dir-reference-table tr:hover td { background: var(--kiro-bg-input); }
                    .dir-reference-table code { font-size: 11px; padding: 1px 5px; background: var(--kiro-bg-input); border-radius: 3px; }
                </style>
            </div>
        `;
    }

    async initialize() {
        const sidebarIcon = document.getElementById('integrationSidebarIcon');
        if (sidebarIcon) sidebarIcon.textContent = this.icon;

        // Populate directory reference table with resolved paths from the backend
        const invoke = window.__TAURI__?.core?.invoke;
        if (invoke) {
            try {
                const dirs = await invoke('resolve_directories');
                const tbody = document.getElementById('dirReferenceBody');
                if (tbody) {
                    tbody.innerHTML = dirs.map(d =>
                        `<tr><td><code>${d.keyword}</code></td><td>${d.aliases}</td><td>${d.path || '<span style="color:var(--kiro-text-muted)">not available</span>'}</td></tr>`
                    ).join('');
                }
            } catch (e) {
                console.warn('Failed to resolve directories:', e);
            }
        }

        document.getElementById('testNotificationBtn')?.addEventListener('click', async () => {
            const statusEl = document.getElementById('notificationStatus');
            try {
                const notif = window.__TAURI__?.notification;
                if (!notif) {
                    statusEl.textContent = '❌ Notification plugin not available.';
                    return;
                }
                let granted = await notif.isPermissionGranted();
                if (!granted) {
                    const perm = await notif.requestPermission();
                    granted = perm === 'granted';
                }
                if (granted) {
                    notif.sendNotification({ title: 'Kiro Assistant', body: 'Notifications are working!' });
                    statusEl.textContent = '✅ Notification sent!';
                } else {
                    statusEl.textContent = '❌ Permission denied. Check your OS notification settings.';
                }
            } catch (e) {
                statusEl.textContent = '❌ Error: ' + e;
            }
        });
    }

    load(config) {
        const captureSel = document.getElementById('captureSelection');
        if (captureSel) captureSel.checked = config.system?.capture_selection !== false;
        const showNotif = document.getElementById('showNotifications');
        if (showNotif) showNotif.checked = config.system?.show_notifications !== false;
    }

    save(config) {
        config.system = config.system || {};
        config.system.capture_selection = document.getElementById('captureSelection')?.checked ?? true;
        config.system.show_notifications = document.getElementById('showNotifications')?.checked ?? true;
    }
}
