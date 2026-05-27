import { SettingsModule } from './base.js';
/**
 * Color Picker Settings Module
 */
export class ColorPickerSettingsModule extends SettingsModule {
    constructor() {
        super('colorpicker', 'Color Picker', '🎨');
        this.bindFields([
            {
                id: 'colorPickerEnabled',
                path: 'color_picker.enabled',
                kind: 'checkbox',
                default: true,
            },
            {
                id: 'colorCopyFormat',
                path: 'color_picker.copy_format',
                kind: 'value',
                default: 'all',
            },
        ]);
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
        this.loadFields(config);
    }

    save(config) {
        this.saveFields(config);
    }
}
