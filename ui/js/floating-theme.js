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
    } catch (e) {
        applyTheme('system');
    }
}
