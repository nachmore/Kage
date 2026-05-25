/**
 * Link Preview settings provider (sandboxed).
 *
 * Schema-only; the actual rendering happens in the host's
 * settings-renderer. Cache-management buttons round-trip through
 * `runSettingsAction` and return a `link_metadata` host effect — the
 * renderer recognises it and runs the corresponding Tauri command.
 * The extension itself doesn't need any new capability.
 */
export default class LinkPreviewSettingsProvider {
    initialize(context) {
        this.config = context.config || {};
    }
    onConfigUpdate(config) {
        this.config = config || {};
    }

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
                            description:
                                'Show preview cards for URLs found in assistant messages.',
                            default: true,
                        },
                        {
                            type: 'checkbox',
                            id: 'show_images',
                            label: 'Show hero images',
                            description:
                                "Render the page's Open Graph or Twitter Card image when available. Turn off to keep cards compact (no images at all).",
                            default: true,
                        },
                        {
                            type: 'number',
                            id: 'max_previews',
                            label: 'Max Previews Per Message',
                            description:
                                'Limit the number of preview cards shown per message to avoid clutter.',
                            default: 5,
                            min: 1,
                            max: 20,
                            maxWidth: 80,
                        },
                    ],
                },
                {
                    title: 'Cache',
                    controls: [
                        {
                            type: 'info',
                            html: 'Fetched metadata is cached on disk for up to 7 days so reopening a chat does not re-fetch every URL. Failed fetches are cached briefly so transient errors recover on their own.',
                        },
                        {
                            type: 'action',
                            id: 'cacheStats',
                            label: 'Show cache size',
                            action: 'cache_stats',
                            description:
                                'Check how much disk space the link-preview cache is using.',
                        },
                        {
                            type: 'action',
                            id: 'cacheClear',
                            label: 'Clear cache',
                            action: 'cache_clear',
                            variant: 'danger',
                            confirm:
                                'Clear all cached link previews? URLs will be re-fetched the next time they appear.',
                            description:
                                'Wipe the cache. Use this if a link preview looks stale or wrong.',
                        },
                    ],
                },
            ],
        };
    }

    /**
     * Action dispatcher. Renderer calls this when the user clicks one
     * of the action buttons; the return value's `host` field tells the
     * renderer to run a host-side effect we can't run from inside the
     * sandbox.
     */
    runSettingsAction({ action }) {
        if (action === 'cache_clear') {
            return { host: { type: 'link_metadata', op: 'clear' } };
        }
        if (action === 'cache_stats') {
            return { host: { type: 'link_metadata', op: 'stats' } };
        }
        return { error: `Unknown action: ${action}` };
    }
}
