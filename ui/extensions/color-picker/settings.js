/**
 * Color Picker Settings Module — extension version.
 * Reads/writes config from config.extensions['color-picker'].
 */
class ColorPickerExtSettingsModule extends SettingsModule {
    constructor() {
        super('colorpicker', 'Color Picker', '🎨');
        this.description = 'Detect color values (hex, rgb, hsl, named colors) and show a preview with format conversions.';
    }

    renderContent() {
        return `
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
        `;
    }

    render() { return this.renderContent(); }

    load(config) {
        const cp = (config.extensions && config.extensions['color-picker']) || {};
        const format = document.getElementById('colorCopyFormat');
        if (format) format.value = cp.copy_format || 'all';
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions['color-picker'] = {
            copy_format: document.getElementById('colorCopyFormat')?.value || 'all',
        };
    }
}
window.ColorPickerExtSettingsModule = ColorPickerExtSettingsModule;
