import { getConfig } from './config-cache.js';

/**
 * Send a system notification using Tauri's notification plugin.
 * Checks permission and the show_notifications config setting.
 * @param {Function} invoke - Tauri invoke function
 * @param {string} title - Notification title
 * @param {string} body - Notification body
 * @param {string} source - Which window sent this: 'floating' or 'main'
 */
export async function sendAppNotification(invoke, title, body, source) {
    try {
        const config = await getConfig(invoke);
        if (config.system?.show_notifications === false) return;

        const notif = window.__TAURI__?.notification;
        if (!notif) return;

        let granted = await notif.isPermissionGranted();
        if (!granted) {
            const perm = await notif.requestPermission();
            granted = perm === 'granted';
        }
        if (!granted) return;

        notif.sendNotification({ title, body });
        // The single-session "notification source" routing was removed
        // when chat windows became 1:1 with sessions. Notification
        // click-to-foreground now defers to the OS / Tauri default.
        void source;
    } catch {
        /* ignore */
    }
}
