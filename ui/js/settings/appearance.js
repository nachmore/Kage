import { SettingsModule } from './base.js';
import { applyTheme as kageApplyTheme, loadAndApplyTheme } from '../shared/theme.js';
import { t, isMachineTranslated, activeLanguage, systemLanguage } from '../shared/i18n.js';
/**
 * Appearance Settings Module
 */
export class AppearanceSettingsModule extends SettingsModule {
    constructor() {
        super('appearance', t('settings.appearance.title'), '🎨');
    }

    render() {
        return `
                <div class="settings-section" id="${this.id}-section">
                    <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                    <!-- Language -->
                    <div class="setting-row" id="languageSection">
                        <div class="setting-label">${t('settings.appearance.language.label')}</div>
                        <div class="setting-description">${t('settings.appearance.language.description')}</div>
                        <select class="setting-select" id="language">
                            <option value="">${t('settings.appearance.language.system', { detected: systemLanguage() || activeLanguage() })}</option>
                        </select>
                        <div id="languageMachineWarning" class="setting-description" style="display:none;color:var(--kage-text-muted);margin-top:6px;">
                            ${t('settings.appearance.language.machine_translated_warning')}
                        </div>
                    </div>

                    <!-- Theme -->
                    <div class="setting-row" id="themeSection">
                        <div class="setting-label">${t('settings.appearance.theme.label')}</div>
                        <div class="setting-description">${t('settings.appearance.theme.description')}</div>
                        <input type="hidden" id="theme" value="system">
                        <div id="themeList" class="theme-list-scroll" style="margin-top:8px;"></div>
                        <div style="margin-top:8px;">
                            <button class="setting-button" id="browseThemesBtn" style="font-size:12px;">${t('settings.appearance.browse_themes')}</button>
                        </div>
                    </div>

                    ${this.createControlRow(
                        t('settings.appearance.font_size.label'),
                        t('settings.appearance.font_size.description'),
                        '<div class="range-container">' +
                            '<input type="range" class="range-slider" id="fontSize" min="11" max="20" step="1" value="14">' +
                            '<span class="range-value" id="fontSizeValue">14px</span>' +
                            '</div>'
                    )}

                    <!-- Launcher -->
                    <div class="setting-section-label">${t('settings.appearance.launcher.label')}</div>

                    ${this.createControlRow(
                        t('settings.appearance.opacity.label'),
                        t('settings.appearance.opacity.description'),
                        '<div class="range-container">' +
                            '<input type="range" class="range-slider" id="opacity" min="0.3" max="1" step="0.05" value="1">' +
                            '<span class="range-value" id="opacityValue">1.0</span>' +
                            '</div>'
                    )}

                    ${this.createControlRow(
                        t('settings.appearance.start_position.label'),
                        t('settings.appearance.start_position.description'),
                        '<select class="setting-select" id="windowStartPosition">' +
                            `<option value="center">${t('settings.appearance.start_position.center')}</option>` +
                            `<option value="mouse">${t('settings.appearance.start_position.mouse')}</option>` +
                            `<option value="remember">${t('settings.appearance.start_position.remember')}</option>` +
                            '</select>'
                    )}

                    ${this.createCheckboxRow(
                        t('settings.appearance.remember_size.label'),
                        t('settings.appearance.remember_size.description'),
                        'rememberLauncherSize',
                        false
                    )}

                    ${this.createCheckboxRow(
                        t('settings.appearance.preserve_last_response.label'),
                        t('settings.appearance.preserve_last_response.description'),
                        'preserveLastResponse',
                        true
                    )}

                    ${this.createCheckboxRow(
                        t('settings.appearance.show_toolbar.label'),
                        t('settings.appearance.show_toolbar.description'),
                        'showFloatingToolbar',
                        false
                    )}

                    ${this.createCheckboxRow(
                        t('settings.appearance.show_time.label'),
                        t('settings.appearance.show_time.description'),
                        'showTime',
                        false
                    )}

                    <div id="timeFormatRow" style="display:none; padding-left: 28px;">
                        ${this.createControlRow(
                            t('settings.appearance.time_format.label'),
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
                        t('settings.appearance.show_date.label'),
                        t('settings.appearance.show_date.description'),
                        'showDate',
                        false
                    )}

                    <div id="dateFormatRow" style="display:none; padding-left: 28px;">
                        ${this.createControlRow(
                            t('settings.appearance.date_format.label'),
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
                    <div class="setting-section-label">${t('settings.appearance.session_manager.label')}</div>

                    ${this.createCheckboxRow(
                        t('settings.appearance.remember_chat_geometry.label'),
                        t('settings.appearance.remember_chat_geometry.description'),
                        'rememberChatGeometry',
                        true
                    )}
                </div>
            `;
    }

    load(config) {
        if (!config.ui) return;

        // Populate language dropdown from the embedded catalog list and set
        // the current selection. An empty value means "follow system locale".
        this._populateLanguageDropdown(config.ui.language || '');

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
            if (opacityValue)
                opacityValue.textContent = (config.ui.floating_window_opacity ?? 1.0).toFixed(2);
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

        this.toggleDateTimeFormats();

        // Apply theme classes immediately so the settings window reflects the change.
        kageApplyTheme(config.ui.theme || 'system');

        // Load theme list now that the hidden input has the correct value
        this.loadInstalledThemes();
    }

    save(config) {
        config.ui = config.ui || {};
        const langSel = document.getElementById('language');
        if (langSel) {
            const v = langSel.value;
            // Empty string means "follow system" — store as null so a future
            // system-locale change is still honoured. Anything else is a hard
            // override the user explicitly picked.
            config.ui.language = v || null;
            // Ask the backend to switch locales immediately. The backend
            // also persists this in config.ui.language and broadcasts
            // config_updated, but we save explicitly here so the rest of
            // the save() flow doesn't lose the value.
            const invoke = window.__TAURI__?.core?.invoke;
            if (invoke) {
                invoke('set_language', { language: v || null }).catch((e) =>
                    console.warn('set_language failed', e)
                );
            }
        }
        config.ui.theme = document.getElementById('theme')?.value || 'system';
        config.ui.floating_window_opacity = parseFloat(
            document.getElementById('opacity')?.value ?? '1'
        );
        const rememberChat = document.getElementById('rememberChatGeometry')?.checked ?? true;
        if (!rememberChat) {
            config.ui.chat_window_width = 0;
            config.ui.chat_window_height = 0;
            config.ui.chat_window_x = null;
            config.ui.chat_window_y = null;
        }
        // Don't overwrite saved geometry when checkbox is on — it's saved by the chat window itself
        config.ui.preserve_last_response =
            document.getElementById('preserveLastResponse')?.checked ?? true;
        config.ui.show_floating_toolbar =
            document.getElementById('showFloatingToolbar')?.checked ?? false;
        config.ui.remember_launcher_size =
            document.getElementById('rememberLauncherSize')?.checked ?? false;
        if (!config.ui.remember_launcher_size) {
            config.ui.launcher_width = null;
            config.ui.launcher_height = null;
        }
        config.ui.show_time = document.getElementById('showTime')?.checked ?? false;
        config.ui.show_date = document.getElementById('showDate')?.checked ?? false;
        config.ui.time_format = document.getElementById('timeFormat')?.value || 'HH:mm';
        config.ui.date_format = document.getElementById('dateFormat')?.value || 'ddd, MMM D';
        config.ui.window_start_position =
            document.getElementById('windowStartPosition')?.value || 'center';
        config.ui.font_size = parseInt(document.getElementById('fontSize')?.value ?? '14', 10);

        // Apply theme classes immediately so the settings window reflects the change.
        kageApplyTheme(config.ui.theme);
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
        document
            .getElementById('showTime')
            ?.addEventListener('change', () => this.toggleDateTimeFormats());
        document
            .getElementById('showDate')
            ?.addEventListener('change', () => this.toggleDateTimeFormats());

        // Browse themes button
        document.getElementById('browseThemesBtn')?.addEventListener('click', () => {
            if (window.__TAURI__?.core) {
                window.__TAURI__.core.invoke('open_store_window', { tab: 'themes' });
            }
        });
    }

    async _populateLanguageDropdown(currentValue) {
        const sel = document.getElementById('language');
        if (!sel) return;
        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) return;
        try {
            const langs = await invoke('get_available_languages');
            // Keep the leading "System default" option (already in the
            // markup) and append every shipped catalog. We don't sort — the
            // backend already returns them in code-sorted order, which is
            // close enough to alphabetical for an end-user dropdown.
            sel.innerHTML = '';
            const systemOpt = document.createElement('option');
            systemOpt.value = '';
            // Show the OS-reported locale, not the currently-active language.
            // The active language can differ if the user has an explicit
            // override; the "System default" hint should describe what
            // selecting "" will resolve to (i.e. sys-locale).
            systemOpt.textContent = t('settings.appearance.language.system', {
                detected: systemLanguage() || activeLanguage(),
            });
            sel.appendChild(systemOpt);
            for (const l of langs) {
                const opt = document.createElement('option');
                opt.value = l.code;
                // Append the language code in parens — disambiguates pairs like
                // "Português" pt-BR vs pt-PT and "中文" zh-CN vs zh-TW for
                // people who speak the language. The (β) marker has been
                // dropped: every non-EN catalog is currently machine-
                // translated, so the per-row badge was universal noise. The
                // global banner under the dropdown still warns about
                // machine-translated content for the *active* language.
                opt.textContent = `${l.name} (${l.code})`;
                sel.appendChild(opt);
            }
            sel.value = currentValue || '';
            // Show the machine-translated warning when the *current* active
            // language is machine-translated. Clearing the override so the
            // user falls back to system is allowed; we don't gate on that.
            const warn = document.getElementById('languageMachineWarning');
            if (warn) warn.style.display = isMachineTranslated() ? '' : 'none';
        } catch (e) {
            console.warn('Failed to load language list:', e);
        }
    }

    async loadInstalledThemes() {
        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) return;

        try {
            const themes = await invoke('list_themes');
            const themeList = document.getElementById('themeList');
            const themeInput = document.getElementById('theme');
            // The hidden input is the source of truth for the current selection
            const activeThemeId = themeInput?.value || 'system';

            if (!themeList) return;

            // Built-in themes (cannot be deleted). Names + descriptions go
            // through i18n so a German user sees "Kage (Hell / Dunkel)" etc.
            // The id stays canonical English ("system"/"light"/"dark") because
            // it's the wire format read from / written to config.ui.theme.
            const builtins = [
                {
                    id: 'system',
                    icon: '🖥️',
                    name: t('settings.appearance.builtin_theme.system.name'),
                    description: t('settings.appearance.builtin_theme.system.description'),
                },
                {
                    id: 'light',
                    icon: '☀️',
                    name: t('settings.appearance.builtin_theme.light.name'),
                    description: '',
                },
                {
                    id: 'dark',
                    icon: '🌙',
                    name: t('settings.appearance.builtin_theme.dark.name'),
                    description: '',
                },
            ];
            const useBtnTitle = t('settings.appearance.theme.use_btn_title');
            const activeTickTitle = t('settings.appearance.theme.active_tick_title');
            const uninstallBtnTitle = t('settings.appearance.theme.uninstall_btn_title');

            let html = '';
            for (const b of builtins) {
                const isActive = b.id === activeThemeId;
                const tickBtn = isActive
                    ? `<span class="theme-action-btn theme-active-tick" title="${activeTickTitle}">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                    </span>`
                    : `<button class="theme-action-btn theme-use-btn" data-theme-id="${b.id}" title="${useBtnTitle}">
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
                    ? `<span class="theme-action-btn theme-active-tick" title="${activeTickTitle}">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                    </span>`
                    : `<button class="theme-action-btn theme-use-btn" data-theme-id="${esc(t.manifest.id)}" title="${useBtnTitle}">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                    </button>`;
                const removeBtn = t.bundled
                    ? ''
                    : `<button class="theme-action-btn theme-remove-btn" data-theme-id="${esc(t.manifest.id)}" title="${uninstallBtnTitle}">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                    </button>`;
                html += `<div class="theme-list-item${isActive ? ' theme-list-item-active' : ''}">
                    <span class="theme-list-icon">${t.manifest.icon || '🎨'}</span>
                    <span class="theme-list-name">${esc(t.manifest.name)} <span style="color:var(--kage-text-muted);font-size:11px;">v${esc(t.manifest.version)}</span></span>
                    <span class="theme-list-actions">${tickBtn}${removeBtn}</span>
                </div>`;
            }

            themeList.innerHTML = html;

            // Wire up Use buttons
            themeList.querySelectorAll('.theme-use-btn').forEach((btn) => {
                btn.addEventListener('click', () => this._useTheme(btn.dataset.themeId));
            });
            // Wire up Remove buttons
            themeList.querySelectorAll('.theme-remove-btn').forEach((btn) => {
                btn.addEventListener('click', () => this._removeTheme(btn.dataset.themeId));
            });
        } catch (e) {
            console.warn('Failed to load themes:', e);
        }

        function esc(s) {
            const d = document.createElement('div');
            d.textContent = s;
            return d.innerHTML;
        }
    }

    async _useTheme(themeId) {
        const themeInput = document.getElementById('theme');
        if (themeInput) themeInput.value = themeId;
        if (typeof saveSettings === 'function') {
            saveSettings();
        }
        // Apply custom theme colors immediately in settings window
        const invoke = window.__TAURI__?.core?.invoke;
        if (invoke) loadAndApplyTheme(invoke);
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

    validate() {
        return { valid: true };
    }

    destroy() {}
}
