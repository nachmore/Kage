/**
 * Appearance Settings Module
 */
class AppearanceSettingsModule extends SettingsModule {
    constructor() {
        super('appearance', 'Appearance', '🎨');
    }

    render() {
            return `
                <div class="settings-section" id="${this.id}-section">
                    <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                    <!-- Theme -->
                    <div class="setting-row" id="themeSection">
                        <div class="setting-label">Theme</div>
                        <div class="setting-description">Choose your preferred theme or follow system settings.</div>
                        <input type="hidden" id="theme" value="system">
                        <div id="themeList" class="theme-list-scroll" style="margin-top:8px;"></div>
                        <div style="margin-top:8px;">
                            <button class="setting-button" id="browseThemesBtn" style="font-size:12px;">🛍️ Browse Themes in Store...</button>
                        </div>
                    </div>

                    ${this.createControlRow(
                        'Font Size',
                        'Base font size for the Launcher and Session Manager (in pixels).',
                        '<div class="range-container">' +
                            '<input type="range" class="range-slider" id="fontSize" min="11" max="20" step="1" value="14">' +
                            '<span class="range-value" id="fontSizeValue">14px</span>' +
                        '</div>'
                    )}

                    <!-- Launcher -->
                    <div class="setting-section-label">Launcher</div>

                    ${this.createControlRow(
                        'Opacity',
                        'Adjust transparency (0.3 = very transparent, 1.0 = solid).',
                        '<div class="range-container">' +
                            '<input type="range" class="range-slider" id="opacity" min="0.3" max="1" step="0.05" value="1">' +
                            '<span class="range-value" id="opacityValue">1.0</span>' +
                        '</div>'
                    )}

                    ${this.createControlRow(
                        'Start Position',
                        'Where the Launcher appears when activated.',
                        '<select class="setting-select" id="windowStartPosition">' +
                            '<option value="center">Center of active monitor</option>' +
                            '<option value="mouse">Next to mouse cursor</option>' +
                            '<option value="remember">Remember last position</option>' +
                        '</select>'
                    )}

                    ${this.createCheckboxRow(
                        'Remember Size',
                        'Restore the Launcher to its last manually resized dimensions when reopened.',
                        'rememberLauncherSize',
                        false
                    )}

                    ${this.createCheckboxRow(
                        'Preserve Last Response',
                        'Keep the last AI response visible when the Launcher is reshown.',
                        'preserveLastResponse',
                        true
                    )}

                    ${this.createCheckboxRow(
                        'Show Toolbar',
                        'Show the attach file/image toolbar below the input.',
                        'showFloatingToolbar',
                        false
                    )}

                    ${this.createCheckboxRow(
                        'Show Time',
                        'Display the current time in the input area.',
                        'showTime',
                        false
                    )}

                    <div id="timeFormatRow" style="display:none; padding-left: 28px;">
                        ${this.createControlRow(
                            'Time Format',
                            '',
                            '<select class="setting-select" id="timeFormat">' +
                                '<option value="HH:mm">14:30 (24h)</option>' +
                                '<option value="HH:mm:ss">14:30:45 (24h + sec)</option>' +
                                '<option value="h:mm A">2:30 PM (12h)</option>' +
                                '<option value="h:mm:ss A">2:30:45 PM (12h + sec)</option>' +
                            '</select>'
                        )}
                    </div>

                    ${this.createCheckboxRow(
                        'Show Date',
                        'Display the current date in the input area.',
                        'showDate',
                        false
                    )}

                    <div id="dateFormatRow" style="display:none; padding-left: 28px;">
                        ${this.createControlRow(
                            'Date Format',
                            '',
                            '<select class="setting-select" id="dateFormat">' +
                                '<option value="ddd, MMM D">Mon, Jan 5</option>' +
                                '<option value="dddd, MMMM D">Monday, January 5</option>' +
                                '<option value="MMM D, YYYY">Jan 5, 2026</option>' +
                                '<option value="D MMM YYYY">5 Jan 2026</option>' +
                                '<option value="YYYY-MM-DD">2026-01-05</option>' +
                                '<option value="MM/DD/YYYY">01/05/2026</option>' +
                                '<option value="DD/MM/YYYY">05/01/2026</option>' +
                            '</select>'
                        )}
                    </div>

                    <!-- Session Manager -->
                    <div class="setting-section-label">Session Manager</div>

                    ${this.createCheckboxRow(
                        'Remember Window Size & Position',
                        'Restore the Session Manager to its last size and position when reopened.',
                        'rememberChatGeometry',
                        true
                    )}
                </div>
            `;
        }

    load(config) {
        if (!config.ui) return;

        const theme = document.getElementById('theme');
        const opacity = document.getElementById('opacity');
        const opacityValue = document.getElementById('opacityValue');
        const preserve = document.getElementById('preserveLastResponse');
        const rememberChat = document.getElementById('rememberChatGeometry');
        const startPos = document.getElementById('windowStartPosition');
        const fontSize = document.getElementById('fontSize');
        const fontSizeValue = document.getElementById('fontSizeValue');

        if (theme) theme.value = config.ui.theme || 'system';
        if (opacity) {
            opacity.value = config.ui.floating_window_opacity ?? 1.0;
            if (opacityValue) opacityValue.textContent = (config.ui.floating_window_opacity ?? 1.0).toFixed(2);
        }
        if (preserve) preserve.checked = config.ui.preserve_last_response !== false;
        const showToolbar = document.getElementById('showFloatingToolbar');
        if (showToolbar) showToolbar.checked = config.ui.show_floating_toolbar === true;
        const showTime = document.getElementById('showTime');
        const showDate = document.getElementById('showDate');
        const timeFormat = document.getElementById('timeFormat');
        const dateFormat = document.getElementById('dateFormat');
        if (showTime) showTime.checked = config.ui.show_time === true;
        if (showDate) showDate.checked = config.ui.show_date === true;
        if (timeFormat) timeFormat.value = config.ui.time_format || 'HH:mm';
        if (dateFormat) dateFormat.value = config.ui.date_format || 'ddd, MMM D';
        this.toggleDateTimeFormats();
        if (rememberChat) rememberChat.checked = (config.ui.chat_window_width || 0) > 0;
        const rememberLauncher = document.getElementById('rememberLauncherSize');
        if (rememberLauncher) rememberLauncher.checked = config.ui.remember_launcher_size === true;
        if (startPos) startPos.value = config.ui.window_start_position || 'center';
        if (fontSize) {
            fontSize.value = config.ui.font_size || 14;
            if (fontSizeValue) fontSizeValue.textContent = (config.ui.font_size || 14) + 'px';
        }

        this.applyTheme(config.ui.theme || 'system');
    }

    save(config) {
        config.ui = config.ui || {};
        config.ui.theme = document.getElementById('theme')?.value || 'system';
        config.ui.floating_window_opacity = parseFloat(document.getElementById('opacity')?.value ?? '1');
        const rememberChat = document.getElementById('rememberChatGeometry')?.checked ?? true;
        if (!rememberChat) {
            config.ui.chat_window_width = 0;
            config.ui.chat_window_height = 0;
            config.ui.chat_window_x = null;
            config.ui.chat_window_y = null;
        }
        // Don't overwrite saved geometry when checkbox is on — it's saved by the chat window itself
        config.ui.preserve_last_response = document.getElementById('preserveLastResponse')?.checked ?? true;
        config.ui.show_floating_toolbar = document.getElementById('showFloatingToolbar')?.checked ?? false;
        config.ui.remember_launcher_size = document.getElementById('rememberLauncherSize')?.checked ?? false;
        if (!config.ui.remember_launcher_size) {
            config.ui.launcher_width = null;
            config.ui.launcher_height = null;
        }
        config.ui.show_time = document.getElementById('showTime')?.checked ?? false;
        config.ui.show_date = document.getElementById('showDate')?.checked ?? false;
        config.ui.time_format = document.getElementById('timeFormat')?.value || 'HH:mm';
        config.ui.date_format = document.getElementById('dateFormat')?.value || 'ddd, MMM D';
        config.ui.window_start_position = document.getElementById('windowStartPosition')?.value || 'center';
        config.ui.font_size = parseInt(document.getElementById('fontSize')?.value ?? '14');

        // Apply immediately
        this.applyTheme(config.ui.theme);
    }

    initialize() {
        const opacity = document.getElementById('opacity');
        const opacityValue = document.getElementById('opacityValue');
        if (opacity && opacityValue) {
            opacity.addEventListener('input', (e) => {
                opacityValue.textContent = parseFloat(e.target.value).toFixed(2);
            });
        }

        const fontSize = document.getElementById('fontSize');
        const fontSizeValue = document.getElementById('fontSizeValue');
        if (fontSize && fontSizeValue) {
            fontSize.addEventListener('input', (e) => {
                fontSizeValue.textContent = e.target.value + 'px';
            });
        }

        // Show/hide date/time format selectors
        document.getElementById('showTime')?.addEventListener('change', () => this.toggleDateTimeFormats());
        document.getElementById('showDate')?.addEventListener('change', () => this.toggleDateTimeFormats());

        // Browse themes button
        document.getElementById('browseThemesBtn')?.addEventListener('click', () => {
            if (window.__TAURI__?.core) {
                window.__TAURI__.core.invoke('open_store_window', { tab: 'themes' });
            }
        });

        // Load installed themes into the select and the list
        this.loadInstalledThemes();

        // Listen for system theme changes
        const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
        this.themeChangeHandler = (e) => {
            const currentTheme = document.getElementById('theme')?.value;
            if (currentTheme === 'system') {
                document.body.classList.toggle('dark-theme', e.matches);
            }
        };
        mediaQuery.addEventListener('change', this.themeChangeHandler);
    }

    async loadInstalledThemes() {
        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) return;

        try {
            const themes = await invoke('list_themes');
            const themeList = document.getElementById('themeList');
            const themeInput = document.getElementById('theme');
            const config = await invoke('get_config');
            const activeThemeId = config.ui?.theme || 'system';

            if (themeInput) themeInput.value = activeThemeId;

            if (!themeList) return;

            // Built-in themes (cannot be deleted)
            const builtins = [
                { id: 'system', icon: '🖥️', name: 'System (Auto)', description: 'Follow OS preference' },
                { id: 'light',  icon: '☀️', name: 'Kiro Light', description: '' },
                { id: 'dark',   icon: '🌙', name: 'Kiro Dark', description: '' },
            ];

            let html = '';
            for (const b of builtins) {
                const isActive = b.id === activeThemeId;
                const tickBtn = isActive
                    ? `<span class="theme-action-btn theme-active-tick" title="Active theme">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                    </span>`
                    : `<button class="theme-action-btn theme-use-btn" data-theme-id="${b.id}" title="Use this theme">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                    </button>`;
                html += `<div class="theme-list-item${isActive ? ' theme-list-item-active' : ''}">
                    <span class="theme-list-icon">${b.icon}</span>
                    <span class="theme-list-name">${b.name}</span>
                    <span class="theme-list-actions">${tickBtn}</span>
                </div>`;
            }

            // Custom/installed themes
            for (const t of themes) {
                if (!t.enabled) continue;
                const isActive = t.manifest.id === activeThemeId;
                const tickBtn = isActive
                    ? `<span class="theme-action-btn theme-active-tick" title="Active theme">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                    </span>`
                    : `<button class="theme-action-btn theme-use-btn" data-theme-id="${esc(t.manifest.id)}" title="Use this theme">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                    </button>`;
                const removeBtn = t.bundled ? '' : `<button class="theme-action-btn theme-remove-btn" data-theme-id="${esc(t.manifest.id)}" title="Uninstall theme">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                    </button>`;
                html += `<div class="theme-list-item${isActive ? ' theme-list-item-active' : ''}">
                    <span class="theme-list-icon">${t.manifest.icon || '🎨'}</span>
                    <span class="theme-list-name">${esc(t.manifest.name)} <span style="color:var(--kiro-text-muted);font-size:11px;">v${esc(t.manifest.version)}</span></span>
                    <span class="theme-list-actions">${tickBtn}${removeBtn}</span>
                </div>`;
            }

            themeList.innerHTML = html;

            // Wire up Use buttons
            themeList.querySelectorAll('.theme-use-btn').forEach(btn => {
                btn.addEventListener('click', () => this._useTheme(btn.dataset.themeId));
            });
            // Wire up Remove buttons
            themeList.querySelectorAll('.theme-remove-btn').forEach(btn => {
                btn.addEventListener('click', () => this._removeTheme(btn.dataset.themeId));
            });
        } catch (e) {
            console.warn('Failed to load themes:', e);
        }

        function esc(s) { const d = document.createElement('div'); d.textContent = s; return d.innerHTML; }
    }

    async _useTheme(themeId) {
        const themeInput = document.getElementById('theme');
        if (themeInput) themeInput.value = themeId;
        // Use the global saveSettings() from manager.js
        if (typeof saveSettings === 'function') {
            saveSettings();
        }
        // Refresh the list to update active state
        this.loadInstalledThemes();
    }

    async _removeTheme(themeId) {
        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) return;
        try {
            await invoke('uninstall_extension', { id: themeId, kind: 'theme' });
            // If the removed theme was active, switch back to system
            const themeInput = document.getElementById('theme');
            if (themeInput && themeInput.value === themeId) {
                themeInput.value = 'system';
                if (typeof saveSettings === 'function') {
                    saveSettings();
                }
            }
            // Refresh the list
            this.loadInstalledThemes();
        } catch (e) {
            console.warn('Failed to uninstall theme:', e);
        }
    }

    toggleDateTimeFormats() {
        const showTime = document.getElementById('showTime')?.checked;
        const showDate = document.getElementById('showDate')?.checked;
        const timeRow = document.getElementById('timeFormatRow');
        const dateRow = document.getElementById('dateFormatRow');
        if (timeRow) timeRow.style.display = showTime ? '' : 'none';
        if (dateRow) dateRow.style.display = showDate ? '' : 'none';
    }

    applyTheme(theme) {
        const builtins = ['system', 'dark', 'light'];
        if (theme === 'dark') {
            document.body.classList.add('dark-theme');
        } else if (theme === 'light') {
            document.body.classList.remove('dark-theme');
        } else if (!builtins.includes(theme)) {
            // Custom theme — follow OS preference for dark/light class
            const isDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
            document.body.classList.toggle('dark-theme', isDark);
        } else {
            // system
            const isDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
            document.body.classList.toggle('dark-theme', isDark);
        }
    }

    validate() {
        return { valid: true };
    }

    destroy() {
        const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
        if (this.themeChangeHandler) {
            mediaQuery.removeEventListener('change', this.themeChangeHandler);
        }
    }
}
