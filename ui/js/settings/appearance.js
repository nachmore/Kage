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
                        `
                        <select class="setting-select" id="theme">
                            <option value="system">System (Auto)</option>
                            <option value="dark">Dark</option>
                            <option value="light">Light</option>
                        </select>
                        `
                    )}

                    ${this.createControlRow(
                        'Floating Window Opacity',
                        'Adjust transparency of the floating window.',
                        `
                        <div class="range-container">
                            <input type="range" class="range-slider" id="opacity" min="0.5" max="1" step="0.1" value="1">
                            <span class="range-value" id="opacityValue">1.0</span>
                        </div>
                        `
                    )}

                    ${this.createControlRow(
                        'Chat Window Size',
                        '',
                        `
                        <div class="input-group">
                            <input type="number" class="setting-input" id="windowWidth" placeholder="Width" value="800">
                            <input type="number" class="setting-input" id="windowHeight" placeholder="Height" value="600">
                        </div>
                        `
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
        if (config.ui) {
            const theme = document.getElementById('theme');
            const opacity = document.getElementById('opacity');
            const opacityValue = document.getElementById('opacityValue');
            const width = document.getElementById('windowWidth');
            const height = document.getElementById('windowHeight');
            const preserve = document.getElementById('preserveLastResponse');
            
            if (theme) theme.value = config.ui.theme;
            if (opacity) {
                opacity.value = config.ui.floating_window_opacity;
                if (opacityValue) opacityValue.textContent = config.ui.floating_window_opacity.toFixed(1);
            }
            if (width) width.value = config.ui.chat_window_width;
            if (height) height.value = config.ui.chat_window_height;
            if (preserve) preserve.checked = config.ui.preserve_last_response !== false;
            
            this.applyTheme(config.ui.theme);
        }
    }

    save(config) {
        config.ui = {
            theme: document.getElementById('theme').value,
            floating_window_opacity: parseFloat(document.getElementById('opacity').value),
            chat_window_width: parseInt(document.getElementById('windowWidth').value),
            chat_window_height: parseInt(document.getElementById('windowHeight').value),
            preserve_last_response: document.getElementById('preserveLastResponse').checked
        };
        
        // Apply theme immediately
        this.applyTheme(config.ui.theme);
    }

    initialize() {
        // Update opacity value display
        const opacity = document.getElementById('opacity');
        const opacityValue = document.getElementById('opacityValue');
        if (opacity && opacityValue) {
            opacity.addEventListener('input', (e) => {
                opacityValue.textContent = parseFloat(e.target.value).toFixed(1);
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
        // Theme is already dark by default in the new design
        // This can be extended if light theme is needed
    }

    destroy() {
        const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
        if (this.themeChangeHandler) {
            mediaQuery.removeEventListener('change', this.themeChangeHandler);
        }
    }
}
