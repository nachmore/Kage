/**
 * Color Picker settings provider (sandboxed).
 */
export default class ColorPickerSettingsProvider {
    initialize(context) { this.config = context.config || {}; }
    onConfigUpdate(config) { this.config = config || {}; }

    getSettings() {
        return {
            description: 'Detect color values (hex, rgb, hsl, named colors) and show a preview with format conversions.',
            sections: [
                {
                    controls: [
                        {
                            type: 'select',
                            id: 'copy_format',
                            label: 'Copy format',
                            description: 'Which format to copy when pressing Enter on a color result.',
                            default: 'all',
                            maxWidth: 200,
                            options: [
                                { value: 'all', label: 'All formats' },
                                { value: 'hex', label: 'HEX only' },
                                { value: 'rgb', label: 'RGB only' },
                                { value: 'hsl', label: 'HSL only' },
                            ],
                        },
                    ],
                },
            ],
        };
    }
}
