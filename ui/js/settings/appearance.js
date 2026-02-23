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

                ${this.createControlRow(
                    'Chat Window Size',
                    'Default size of the expanded chat window (width × height in pixels).',
                    `<div class="input-group">
                        <input type="number" class="setting-input" id="windowWidth" placeholder="Width" min="400" max="3000" value="800">
                        <span style="color: #938F9B; padding: 0 4px;">×</span>
                        <input type="number" class="setting-input" id="windowHeight" placeholder="Height" min="300" max="2000" value="600">
                    </div>`
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
            </div>
        `;
    }

    load(config) {
        if (!config.ui) return;

        const theme = document.getElementById('theme');
        const opacity = document.getElementById('opacity');
        const opacityValue = document.getElementById('opacityValue');
        const width = document.getElementById('windowWidth');
        const height = document.getElementById('windowHeight');
        const preserve = document.getElementById('preserveLastResponse');
        const startPos = document.getElementById('windowStartPosition');
        const fontSize = document.getElementById('fontSize');
        const fontSizeValue = document.getElementById('fontSizeValue');

        if (theme) theme.value = config.ui.theme || 'system';
        if (opacity) {
            opacity.value = config.ui.floating_window_opacity ?? 1.0;
            if (opacityValue) opacityValue.textContent = (config.ui.floating_window_opacity ?? 1.0).toFixed(2);
        }
        if (width) width.value = config.ui.chat_window_width || 800;
        if (height) height.value = config.ui.chat_window_height || 600;
        if (preserve) preserve.checked = config.ui.preserve_last_response !== false;
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
        config.ui.chat_window_width = parseInt(document.getElementById('windowWidth')?.value ?? '800');
        config.ui.chat_window_height = parseInt(document.getElementById('windowHeight')?.value ?? '600');
        config.ui.preserve_last_response = document.getElementById('preserveLastResponse')?.checked ?? true;
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
        const width = parseInt(document.getElementById('windowWidth')?.value ?? '800');
        const height = parseInt(document.getElementById('windowHeight')?.value ?? '600');
        if (width < 400 || width > 3000) {
            return { valid: false, error: 'Chat window width must be between 400 and 3000 pixels' };
        }
        if (height < 300 || height > 2000) {
            return { valid: false, error: 'Chat window height must be between 300 and 2000 pixels' };
        }
        return { valid: true };
    }

    destroy() {
        const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
        if (this.themeChangeHandler) {
            mediaQuery.removeEventListener('change', this.themeChangeHandler);
        }
    }
}
