import { errLabel } from '../shared/error-message.js';
import { isMac } from '../shared/shortcuts.js';
import { t } from '../shared/i18n.js';
import { SettingsModule } from './base.js';
import { getSystemIcon } from './system.js';
/**
 * System Integration Settings Module
 */
export class IntegrationSettingsModule extends SettingsModule {
    constructor() {
        super('integration', t('settings.integration.title'), getSystemIcon());
        this.bindFields([
            {
                id: 'captureSelection',
                path: 'system.capture_selection',
                kind: 'checkbox',
                default: true,
            },
            {
                id: 'showNotifications',
                path: 'system.show_notifications',
                kind: 'checkbox',
                default: true,
            },
            {
                id: 'screenContext',
                path: 'system.screen_context',
                kind: 'checkbox',
                default: true,
            },
        ]);
    }

    render() {
        const copyShortcut = isMac() ? 'Cmd+C' : 'Ctrl+C';
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                <div class="setting-section-label">${t('settings.integration.behavior.section')}</div>

                ${this.createCheckboxRow(
                    t('settings.integration.capture_selection.label'),
                    t('settings.integration.capture_selection.description', {
                        shortcut: copyShortcut,
                    }),
                    'captureSelection',
                    true
                )}

                ${this.createControlRow(
                    t('settings.integration.blocklist.label'),
                    t('settings.integration.blocklist.description', { shortcut: copyShortcut }),
                    `<textarea id="captureSelectionBlocklist" rows="6" class="setting-input" spellcheck="false" style="font-family: var(--kage-font-mono, monospace); width: 100%; resize: vertical;"></textarea>`
                )}

                ${this.createCheckboxRow(
                    t('settings.integration.show_notifications.label'),
                    t('settings.integration.show_notifications.description'),
                    'showNotifications',
                    true
                )}

                ${this.createCheckboxRow(
                    t('settings.integration.screen_context.label'),
                    t('settings.integration.screen_context.description'),
                    'screenContext',
                    true
                )}

                <div class="setting-row" style="padding-left: 28px;">
                    <button class="setting-button" id="testNotificationBtn">${t('settings.integration.test_notification.btn')}</button>
                    <span class="setting-description" id="notificationStatus" style="margin-left: 8px;"></span>
                </div>

                <div class="setting-section-label">${t('settings.integration.directories.section')}</div>

                <div class="setting-row">
                    <div class="setting-description">${t('settings.integration.directories.description')}</div>
                </div>
                <table class="dir-reference-table">
                    <thead><tr><th>${t('settings.integration.directories.col.keyword')}</th><th>${t('settings.integration.directories.col.aliases')}</th><th>${t('settings.integration.directories.col.path')}</th></tr></thead>
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
                    const unavailable = t('settings.integration.directories.path_unavailable');
                    tbody.innerHTML = dirs
                        .map(
                            (d) =>
                                `<tr><td><code>${d.keyword}</code></td><td>${d.aliases}</td><td>${d.path || `<span style="color:var(--kage-text-muted)">${unavailable}</span>`}</td></tr>`
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
                    statusEl.textContent = t('settings.integration.test_notification.no_plugin');
                    return;
                }
                let granted = await notif.isPermissionGranted();
                if (!granted) {
                    const perm = await notif.requestPermission();
                    granted = perm === 'granted';
                }
                if (granted) {
                    notif.sendNotification({
                        title: 'Kage',
                        body: t('settings.integration.test_notification.body'),
                    });
                    statusEl.textContent = t('settings.integration.test_notification.sent');
                } else {
                    statusEl.textContent = t('settings.integration.test_notification.denied');
                }
            } catch (e) {
                statusEl.textContent =
                    '❌ ' + errLabel(t('settings.integration.test_notification.error_label'), e);
            }
        });
    }

    load(config) {
        this.loadFields(config);
        // Blocklist is a list serialised through a textarea (one per line);
        // can't go through the bind DSL (kind: 'value' would render the
        // array as "[object Array]"). Hand-coded.
        const blocklist = document.getElementById('captureSelectionBlocklist');
        if (blocklist) {
            const list = Array.isArray(config.system?.capture_selection_blocklist)
                ? config.system.capture_selection_blocklist
                : [];
            blocklist.value = list.join('\n');
        }
    }

    save(config) {
        this.saveFields(config);
        config.system = config.system || {};
        const blocklistText = document.getElementById('captureSelectionBlocklist')?.value ?? '';
        config.system.capture_selection_blocklist = blocklistText
            .split('\n')
            .map((s) => s.trim())
            .filter((s) => s.length > 0);
    }
}
