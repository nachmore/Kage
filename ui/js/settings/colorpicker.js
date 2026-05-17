import { SettingsModule } from './base.js';
/**
 * Color Picker Settings Module
 */
export class ColorPickerSettingsModule extends SettingsModule {
    constructor() {
        super('colorpicker', 'Color Picker', '🎨');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                ${this.createCheckboxRow(
                    'Enable color detection',
                    'Detect color values (hex, rgb, hsl, named colors) in the Launcher and show a preview with format conversions.',
                    'colorPickerEnabled',
                    true
                )}

                <div class="setting-row">
                    <div class="setting-label">Copy format</div>
                    <div class="setting-description">Which format to copy when pressing Enter on a color result.</div>
                    <div class="setting-control">
                        <select class="setting-input" id="colorCopyFormat" style="max-width: 200px;">
                            <option value="all">All formats</option>
                            <option value="hex">HEX only</option>
                            <option value="rgb">RGB only</option>
                            <option value="hsl">HSL only</option>
                        </select>
                    </div>
                </div>
            </div>
        `;
    }

    load(config) {
        const cp = config.color_picker || { enabled: true, copy_format: 'all' };
        const enabled = document.getElementById('colorPickerEnabled');
        const format = document.getElementById('colorCopyFormat');
        if (enabled) enabled.checked = cp.enabled !== false;
        if (format) format.value = cp.copy_format || 'all';
    }

    save(config) {
        config.color_picker = config.color_picker || {};
        config.color_picker.enabled =
            document.getElementById('colorPickerEnabled')?.checked ?? true;
        config.color_picker.copy_format =
            document.getElementById('colorCopyFormat')?.value || 'all';
    }
}
