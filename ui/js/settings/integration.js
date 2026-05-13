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

                <div class="setting-section-label">Behavior</div>

                ${this.createCheckboxRow(
                    'Capture selected text',
                    `Grab selected text from the active window when the hotkey is pressed (uses ${window.kagePlatform.isMac() ? 'Cmd+C' : 'Ctrl+C'}). Disable if this interferes with terminal apps or other programs.`,
                    'captureSelection',
                    true
                )}

                ${this.createControlRow(
                    "Don't auto-copy in these apps",
                    `One process name per line. When these apps are focused, Kage won't inject ${window.kagePlatform.isMac() ? 'Cmd+C' : 'Ctrl+C'} — useful for terminals where that sequence means "interrupt" and can cancel in-progress commands. Matching is case-insensitive; a trailing ".exe" is optional. Clear to disable the blocklist entirely.`,
                    `<textarea id="captureSelectionBlocklist" rows="6" class="setting-input" spellcheck="false" style="font-family: var(--kage-font-mono, monospace); width: 100%; resize: vertical;"></textarea>`
                )}

                ${this.createCheckboxRow(
                    'Show notifications',
                    'Show a system notification when a response completes while the window is hidden.',
                    'showNotifications',
                    true
                )}

                ${this.createCheckboxRow(
                    'Screen context awareness',
                    'Include the source application name and window title when sending messages, so Kage knows what you were looking at.',
                    'screenContext',
                    true
                )}

                <div class="setting-row" style="padding-left: 28px;">
                    <button class="setting-button" id="testNotificationBtn">Test Notification</button>
                    <span class="setting-description" id="notificationStatus" style="margin-left: 8px;"></span>
                </div>

                <div class="setting-section-label">Quick Directory Access</div>

                <div class="setting-row">
                    <div class="setting-description">Type any of these keywords in the Launcher to open the folder. Prefix matching works too — e.g. "down" opens Downloads.</div>
                </div>
                <table class="dir-reference-table">
                    <thead><tr><th>Keyword</th><th>Aliases</th><th>Path</th></tr></thead>
                    <tbody id="dirReferenceBody"></tbody>
                </table>

                <style>
                    .dir-reference-table { width: 100%; border-collapse: collapse; font-size: 12px; margin: 8px 0 4px; }
                    .dir-reference-table th { text-align: left; padding: 6px 10px; color: var(--kage-text-muted); font-weight: 500; border-bottom: 1px solid var(--kage-border-subtle); }
                    .dir-reference-table td { padding: 5px 10px; border-bottom: 1px solid var(--kage-border-subtle); color: var(--kage-text); }
                    .dir-reference-table tr:last-child td { border-bottom: none; }
                    .dir-reference-table tr:hover td { background: var(--kage-bg-input); }
                    .dir-reference-table code { font-size: 11px; padding: 1px 5px; background: var(--kage-bg-input); border-radius: 3px; }
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
                    tbody.innerHTML = dirs
                        .map(
                            (d) =>
                                `<tr><td><code>${d.keyword}</code></td><td>${d.aliases}</td><td>${d.path || '<span style="color:var(--kage-text-muted)">not available</span>'}</td></tr>`
                        )
                        .join('');
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
                    notif.sendNotification({ title: 'Kage', body: 'Notifications are working!' });
                    statusEl.textContent = '✅ Notification sent!';
                } else {
                    statusEl.textContent =
                        '❌ Permission denied. Check your OS notification settings.';
                }
            } catch (e) {
                statusEl.textContent = '❌ Error: ' + e;
            }
        });
    }

    load(config) {
        const captureSel = document.getElementById('captureSelection');
        if (captureSel) captureSel.checked = config.system?.capture_selection !== false;
        const blocklist = document.getElementById('captureSelectionBlocklist');
        if (blocklist) {
            const list = Array.isArray(config.system?.capture_selection_blocklist)
                ? config.system.capture_selection_blocklist
                : [];
            blocklist.value = list.join('\n');
        }
        const showNotif = document.getElementById('showNotifications');
        if (showNotif) showNotif.checked = config.system?.show_notifications !== false;
        const screenCtx = document.getElementById('screenContext');
        if (screenCtx) screenCtx.checked = config.system?.screen_context !== false;
    }

    save(config) {
        config.system = config.system || {};
        config.system.capture_selection =
            document.getElementById('captureSelection')?.checked ?? true;
        const blocklistText = document.getElementById('captureSelectionBlocklist')?.value ?? '';
        config.system.capture_selection_blocklist = blocklistText
            .split('\n')
            .map((s) => s.trim())
            .filter((s) => s.length > 0);
        config.system.show_notifications =
            document.getElementById('showNotifications')?.checked ?? true;
        config.system.screen_context = document.getElementById('screenContext')?.checked ?? true;
    }
}
