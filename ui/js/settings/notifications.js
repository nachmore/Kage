/**
 * Notifications Settings Module
 * Example of how easy it is to add a new settings category
 */
class NotificationsSettingsModule extends SettingsModule {
    constructor() {
        super('notifications', 'Notifications', '🔔');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2>${this.icon} ${this.title}</h2>
                <div class="setting-item">
                    <label class="setting-label">Enable Notifications</label>
                    <label class="toggle-switch">
                        <input type="checkbox" id="notificationsEnabled">
                        <span class="toggle-slider"></span>
                    </label>
                    <div class="setting-description">Show desktop notifications for important events</div>
                </div>
                <div class="setting-item">
                    <label class="setting-label">Notification Sound</label>
                    <select class="setting-input" id="notificationSound">
                        <option value="none">None</option>
                        <option value="default">Default</option>
                        <option value="chime">Chime</option>
                        <option value="bell">Bell</option>
                    </select>
                    <div class="setting-description">Sound to play when notifications appear</div>
                </div>
                <div class="setting-item">
                    <label class="setting-label">Notification Duration (seconds)</label>
                    <input type="number" class="setting-input" id="notificationDuration" min="1" max="30" value="5">
                    <div class="setting-description">How long notifications stay visible</div>
                </div>
            </div>
        `;
    }

    load(config) {
        if (config.notifications) {
            const enabled = document.getElementById('notificationsEnabled');
            const sound = document.getElementById('notificationSound');
            const duration = document.getElementById('notificationDuration');
            
            if (enabled) enabled.checked = config.notifications.enabled;
            if (sound) sound.value = config.notifications.sound;
            if (duration) duration.value = config.notifications.duration;
        }
    }

    save(config) {
        config.notifications = {
            enabled: document.getElementById('notificationsEnabled').checked,
            sound: document.getElementById('notificationSound').value,
            duration: parseInt(document.getElementById('notificationDuration').value)
        };
    }

    validate() {
        const duration = parseInt(document.getElementById('notificationDuration').value);
        if (duration < 1 || duration > 30) {
            return { valid: false, error: 'Notification duration must be between 1 and 30 seconds' };
        }
        return { valid: true };
    }
}
