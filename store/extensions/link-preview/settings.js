/**
 * Link Preview settings provider (sandboxed).
 */
export default class LinkPreviewSettingsProvider {
    initialize(context) { this.config = context.config || {}; }
    onConfigUpdate(config) { this.config = config || {}; }

    getSettings() {
        return {
            description: 'Shows inline preview cards for URLs in AI responses.',
            sections: [
                {
                    controls: [
                        {
                            type: 'checkbox',
                            id: 'enabled',
                            label: 'Enable Link Previews',
                            description: 'Show preview cards for URLs found in assistant messages.',
                            default: true,
                        },
                        {
                            type: 'number',
                            id: 'max_previews',
                            label: 'Max Previews Per Message',
                            description: 'Limit the number of preview cards shown per message to avoid clutter.',
                            default: 5,
                            min: 1,
                            max: 20,
                            maxWidth: 80,
                        },
                    ],
                },
            ],
        };
    }
}
