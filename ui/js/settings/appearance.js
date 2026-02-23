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

                ${this.createControlRow(
                    'Theme',
                    'Choose your preferred theme or follow system settings.',
                    `<select class="setting-select" id="theme">
                        <option value="system">System (Auto)</option>
                        <option value="dark">Dark</option>
                        <option value="light">Light</option>
                    </select>`
                )}

                ${this.createControlRow(
                    'Floating Window Opacity',
                    'Adjust transparency of the floating window (0.3 = very transparent, 1.0 = solid).',
                    `<div class="range-container">
                        <input type="range" class="range-slider" id="opacity" min="0.3" max="1" step="0.05" value="1">
                        <span class="range-value" id="opacityValue">1.0</span>
                    </div>`
                )}

                ${this.createControlRow(
                    'Window Start Position',
                    'Where the floating window appears when activated.',
                    `<select class="setting-select" id="windowStartPosition">
                        <option value="center">Center of active monitor</option>
                        <option value="mouse">Next to mouse cursor</option>
                        <option value="remember">Remember last position</option>
                    </select>`
                )}

                ${this.createCheckboxRow(
                    'Remember Session Manager Window Size & Position',
                    'Restore the full size Session Manager chat window to its last size and position when reopened.',
                    'rememberChatGeometry',
                    true
                )}

                ${this.createControlRow(
                    'Font Size',
                    'Base font size for the floating and chat windows (in pixels).',
                    `<div class="range-container">
                        <input type="range" class="range-slider" id="fontSize" min="11" max="20" step="1" value="14">
                        <span class="range-value" id="fontSizeValue">14px</span>
                    </div>`
                )}

                ${this.createCheckboxRow(
                    'Preserve Last Response',
                    'Keep the last AI response visible when the floating window is reshown.',
                    'preserveLastResponse',
                    true
                )}

                ${this.createCheckboxRow(
                    'Show Time',
                    'Display the current time in the floating window input area.',
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
                    'Display the current date in the floating window input area.',
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

    toggleDateTimeFormats() {
        const showTime = document.getElementById('showTime')?.checked;
        const showDate = document.getElementById('showDate')?.checked;
        const timeRow = document.getElementById('timeFormatRow');
        const dateRow = document.getElementById('dateFormatRow');
        if (timeRow) timeRow.style.display = showTime ? '' : 'none';
        if (dateRow) dateRow.style.display = showDate ? '' : 'none';
    }

    applyTheme(theme) {
        if (theme === 'dark') {
            document.body.classList.add('dark-theme');
        } else if (theme === 'light') {
            document.body.classList.remove('dark-theme');
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
