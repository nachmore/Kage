/**
 * Link Preview Settings Module
 */
class LinkPreviewExtSettingsModule extends SettingsModule {
    constructor() {
        super('link-preview', 'Link Preview', '🔗');
        this.description = 'Shows inline preview cards for URLs in AI responses.';
    }

    renderContent() {
        return `
            ${this.createCheckboxRow(
                'Enable Link Previews',
                'Show preview cards for URLs found in assistant messages.',
                'linkPreviewEnabled',
                true
            )}
            ${this.createControlRow(
                'Max Previews Per Message',
                'Limit the number of preview cards shown per message to avoid clutter.',
                '<input type="number" class="setting-input" id="linkPreviewMax" min="1" max="20" value="5" style="max-width:80px;">'
            )}
        `;
    }

    render() { return this.renderContent(); }

    load(config) {
        const ext = (config.extensions && config.extensions['link-preview']) || {};
        const enabled = document.getElementById('linkPreviewEnabled');
        const max = document.getElementById('linkPreviewMax');
        if (enabled) enabled.checked = ext.enabled !== false;
        if (max) max.value = ext.max_previews || 5;
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions['link-preview'] = {
            enabled: document.getElementById('linkPreviewEnabled')?.checked ?? true,
            max_previews: parseInt(document.getElementById('linkPreviewMax')?.value || '5'),
        };
    }
}

window.LinkPreviewExtSettingsModule = LinkPreviewExtSettingsModule;
