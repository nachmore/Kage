// Theme management — config-aware with system/dark/light support

let currentThemeSetting = 'system';

function resolveTheme(setting) {
    if (setting === 'dark') return true;
    if (setting === 'light') return false;
    // "system" — follow OS preference
    return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

export function applyTheme(setting) {
    if (setting !== undefined) {
        currentThemeSetting = setting;
    }
    const isDark = resolveTheme(currentThemeSetting);
    document.body.classList.toggle('dark-theme', isDark);
}

export function initThemeListener() {
    // React to OS theme changes (only matters when setting is "system")
    window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
        if (currentThemeSetting === 'system') {
            applyTheme();
        }
    });
}

/**
 * Load theme from config and apply. Call on init and config_updated.
 */
export async function loadAndApplyTheme(invoke) {
    try {
        const config = await invoke('get_config');
        const theme = config.ui?.theme || 'system';
        applyTheme(theme);

        // Apply font size
        const fontSize = config.ui?.font_size || 14;
        document.documentElement.style.setProperty('--app-font-size', fontSize + 'px');

        // Apply floating window opacity via CSS
        const opacity = config.ui?.floating_window_opacity ?? 1.0;
        document.documentElement.style.setProperty('--window-opacity', opacity);

        // Apply date/time display
        applyDateTime(config.ui);
    } catch (e) {
        applyTheme('system');
    }
}

let _dateTimeInterval = null;

function applyDateTime(ui) {
    const container = document.getElementById('datetimeDisplay');
    const timeEl = document.getElementById('datetimeTime');
    const dateEl = document.getElementById('datetimeDate');
    if (!container) return;

    const showTime = ui?.show_time === true;
    const showDate = ui?.show_date === true;

    if (!showTime && !showDate) {
        container.style.display = 'none';
        if (_dateTimeInterval) { clearInterval(_dateTimeInterval); _dateTimeInterval = null; }
        return;
    }

    const timeFormat = ui?.time_format || 'HH:mm';
    const dateFormat = ui?.date_format || 'ddd, MMM D';

    function update() {
        const now = new Date();
        if (showTime && timeEl) {
            timeEl.textContent = formatDateTime(now, timeFormat);
            timeEl.style.display = '';
        } else if (timeEl) {
            timeEl.style.display = 'none';
        }
        if (showDate && dateEl) {
            dateEl.textContent = formatDateTime(now, dateFormat);
            dateEl.style.display = '';
        } else if (dateEl) {
            dateEl.style.display = 'none';
        }
    }

    // Hide on any keypress (including arrows), show when window appears
    const input = document.getElementById('promptInput');
    if (input && !input._dtListenerAdded) {
        input._dtListenerAdded = true;
        input.addEventListener('keydown', () => {
            if (container) container.style.opacity = '0';
        });
    }

    container.style.display = '';
    container.style.opacity = '1';
    update();
    if (_dateTimeInterval) clearInterval(_dateTimeInterval);
    _dateTimeInterval = setInterval(update, 1000);
}

function formatDateTime(date, fmt) {
    const days = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
    const daysFull = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
    const months = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
    const monthsFull = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];

    const h24 = date.getHours();
    const h12 = h24 % 12 || 12;
    const m = date.getMinutes();
    const s = date.getSeconds();
    const ampm = h24 >= 12 ? 'PM' : 'AM';
    const D = date.getDate();
    const M = date.getMonth();
    const Y = date.getFullYear();
    const dow = date.getDay();

    // Replace longest tokens first to avoid partial matches
    return fmt
        .replace('dddd', daysFull[dow])
        .replace('ddd', days[dow])
        .replace('MMMM', monthsFull[M])
        .replace('MMM', months[M])
        .replace('YYYY', String(Y))
        .replace('MM', String(M + 1).padStart(2, '0'))
        .replace('DD', String(D).padStart(2, '0'))
        .replace('HH', String(h24).padStart(2, '0'))
        .replace('hh', String(h12).padStart(2, '0'))
        .replace(/\bh\b/, String(h12))
        .replace('mm', String(m).padStart(2, '0'))
        .replace('ss', String(s).padStart(2, '0'))
        .replace('A', ampm)
        .replace(/\bD\b/, String(D));
}
