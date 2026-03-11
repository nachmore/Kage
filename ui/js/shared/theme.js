// Theme management — config-aware with system/dark/light support + theme extensions

let currentThemeSetting = 'system';
const BUILTIN_THEMES = ['system', 'dark', 'light'];

function isCustomTheme(setting) {
    return setting && !BUILTIN_THEMES.includes(setting);
}

function resolveTheme(setting) {
    if (setting === 'dark') return true;
    if (setting === 'light') return false;
    // "system" or custom themes — follow OS preference for dark/light class
    return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

export function applyTheme(setting) {
    if (setting !== undefined) {
        currentThemeSetting = setting;
    }
    const isDark = resolveTheme(currentThemeSetting);
    document.body.classList.toggle('dark-theme', isDark);
}

/**
 * Clear any previously applied custom theme CSS variables.
 */
function clearCustomThemeColors() {
    const root = document.documentElement;
    // Remove all --kiro-* custom properties that were set inline
    for (const prop of Array.from(root.style)) {
        if (prop.startsWith('--kiro-')) {
            root.style.removeProperty(prop);
        }
    }
}

/**
 * Apply CSS variables from a theme extension's color map.
 * @param {Object} colors - key/value map, e.g. { "kiro-accent": "#E8853D" }
 */
function applyCustomThemeColors(colors) {
    if (!colors || typeof colors !== 'object') return;
    const root = document.documentElement;
    for (const [key, value] of Object.entries(colors)) {
        root.style.setProperty(`--${key}`, value);
    }
}

export function initThemeListener() {
    // React to OS theme changes (only matters when setting is "system" or custom)
    window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
        if (currentThemeSetting === 'system' || isCustomTheme(currentThemeSetting)) {
            applyTheme();
            // Re-apply custom theme colors for the new variant if needed
            if (isCustomTheme(currentThemeSetting) && _lastInvoke) {
                applyThemeExtensionColors(_lastInvoke, currentThemeSetting);
            }
        }
    });
}

let _lastInvoke = null;

/**
 * Load and apply CSS variables from a theme extension.
 */
async function applyThemeExtensionColors(invoke, themeId) {
    try {
        const isDark = resolveTheme(themeId);
        const variant = isDark ? 'dark' : 'light';
        const colors = await invoke('load_theme_colors', { themeId, variant });
        if (colors && typeof colors === 'object') {
            applyCustomThemeColors(colors);
        }
    } catch (e) {
        console.warn(`Failed to load theme colors for '${themeId}':`, e);
    }
}

/**
 * Load theme from config and apply. Call on init and config_updated.
 */
export async function loadAndApplyTheme(invoke) {
    _lastInvoke = invoke;
    try {
        const config = await invoke('get_config');
        const theme = config.ui?.theme || 'system';

        // Always clear custom theme colors first
        clearCustomThemeColors();

        applyTheme(theme);

        // If it's a custom theme extension, load and apply its CSS variables
        if (isCustomTheme(theme)) {
            await applyThemeExtensionColors(invoke, theme);
        }

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

let _dateTimeTimer = null;
let _dateTimeUi = null; // cached UI config for resume

function applyDateTime(ui) {
    const container = document.getElementById('datetimeDisplay');
    const timeEl = document.getElementById('datetimeTime');
    const dateEl = document.getElementById('datetimeDate');
    if (!container) return;

    _dateTimeUi = ui;

    const showTime = ui?.show_time === true;
    const showDate = ui?.show_date === true;

    if (!showTime && !showDate) {
        container.style.display = 'none';
        _stopDateTimeTimer();
        return;
    }

    const timeFormat = ui?.time_format || 'HH:mm';
    const dateFormat = ui?.date_format || 'ddd, MMM D';
    const needsSeconds = timeFormat.includes('ss');

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

    function scheduleNext() {
        const now = new Date();
        let delayMs;
        if (needsSeconds) {
            delayMs = 1000 - now.getMilliseconds();
        } else {
            delayMs = (60 - now.getSeconds()) * 1000 - now.getMilliseconds();
        }
        if (delayMs < 100) delayMs += needsSeconds ? 1000 : 60000;
        _dateTimeTimer = setTimeout(() => {
            update();
            scheduleNext();
        }, delayMs);
    }

    _stopDateTimeTimer();
    update();
    // Only schedule updates if the page is visible
    if (!document.hidden) {
        scheduleNext();
    }
}

function _stopDateTimeTimer() {
    if (_dateTimeTimer) { clearTimeout(_dateTimeTimer); _dateTimeTimer = null; }
}

// Pause timer when window is hidden, resume when shown
document.addEventListener('visibilitychange', () => {
    if (!_dateTimeUi) return;
    if (document.hidden) {
        _stopDateTimeTimer();
    } else {
        // Re-apply to update the display immediately and restart the timer
        applyDateTime(_dateTimeUi);
    }
});

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
