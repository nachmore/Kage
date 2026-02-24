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
            </div>
        `;
    }

    initialize() {
        const sidebarIcon = document.getElementById('integrationSidebarIcon');
        if (sidebarIcon) sidebarIcon.textContent = this.icon;

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
