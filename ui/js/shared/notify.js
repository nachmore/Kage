import { getConfig } from './config-cache.js';

/**
 * Send a system notification.
 *
 * Tries the Rust `notify_response_ready` command first: on Windows (installed
 * build) it shows a toast whose click foregrounds the `source` window. The
 * notification plugin has no desktop click callback, so that clickable path
 * lives in Rust (WinRT). When the command reports it couldn't show a clickable
 * toast (dev build, or any non-Windows platform), we fall back to the plain
 * plugin `sendNotification` so the user still sees the message.
 *
 * Checks permission and the show_notifications config setting.
 * @param {Function} invoke - Tauri invoke function
 * @param {string} title - Notification title
 * @param {string} body - Notification body
 * @param {string} source - Which window should come forward on click
 *                          (e.g. 'floating', 'main', or a 'chat-<uuid>' label)
 */
export async function sendAppNotification(invoke, title, body, source) {
    try {
        const config = await getConfig(invoke);
        if (config.system?.show_notifications === false) return;

        // Clickable path (Windows/installed). Returns true if it showed the
        // toast; false means "fall back to the plugin toast below".
        try {
            const shown = await invoke('notify_response_ready', {
                title,
                body,
                targetLabel: source || 'main',
            });
            if (shown) return;
        } catch {
            /* command missing or failed — fall through to plugin toast */
        }

        const notif = window.__TAURI__?.notification;
        if (!notif) return;

        let granted = await notif.isPermissionGranted();
        if (!granted) {
            const perm = await notif.requestPermission();
            granted = perm === 'granted';
        }
        if (!granted) return;

        notif.sendNotification({ title, body });
    } catch {
        /* ignore */
    }
}
