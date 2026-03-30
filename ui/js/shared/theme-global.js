/**
 * Theme utilities for non-module windows (settings, store, welcome).
 *
 * ES-module windows (floating, chat) use theme.js instead.
 * This file provides the same logic as a plain <script> — sets
 * window.kageTheme with the public API.
 *
 * Usage:
 *   <script src="js/shared/theme-global.js"></script>
 *   <script>
 *     kageTheme.init();  // on DOMContentLoaded or after Tauri is ready
 *   </script>
 */
(function () {
    const BUILTIN = ['system', 'dark', 'light'];
    let _osDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    let _invoke = null;
    let _currentTheme = 'system';

    function isDark(theme) {
        if (theme === 'dark') return true;
        if (theme === 'light') return false;
        return _osDark;
    }

    function applyClasses(theme) {
        _currentTheme = theme;
        const dark = isDark(theme);
        document.body.classList.toggle('dark-theme', dark);
        document.body.classList.toggle('light-theme', !dark);
    }

    function clearCustomColors() {
        const root = document.documentElement;
        for (const prop of Array.from(root.style)) {
            if (prop.startsWith('--kage-')) root.style.removeProperty(prop);
        }
    }

    function applyCustomColors(colors) {
        if (!colors || typeof colors !== 'object') return;
        const root = document.documentElement;
        for (const [key, value] of Object.entries(colors)) {
            root.style.setProperty('--' + key, value);
        }
    }

    async function refresh(themeOverride) {
        if (!_invoke) return;
        try { _osDark = await _invoke('get_os_dark_mode'); } catch {}
        let theme = themeOverride;
        if (!theme) {
            try {
                const config = await _invoke('get_config');
                theme = config.ui?.theme || 'system';
            } catch { theme = 'system'; }
        }
        clearCustomColors();
        applyClasses(theme);
        if (!BUILTIN.includes(theme)) {
            try {
                const variant = _osDark ? 'dark' : 'light';
                const colors = await _invoke('load_theme_colors', { themeId: theme, variant });
                applyCustomColors(colors);
            } catch {}
        }
    }

    function init() {
        if (!window.__TAURI__?.core?.invoke) {
            console.warn('[theme-global] Tauri not ready');
            return;
        }
        _invoke = window.__TAURI__.core.invoke;
        refresh();
        // Re-apply on config changes
        if (window.__TAURI__?.event?.listen) {
            window.__TAURI__.event.listen('config_updated', () => refresh());
        }
        // Re-apply on OS theme change
        window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => refresh());
    }

    window.kageTheme = { init, refresh, applyClasses, isDark: () => _osDark };
})();
