/**
 * Window Walker search provider.
 * Type the trigger (default "w ") to list open windows, then filter by typing.
 * Selecting a window brings it to the foreground.
 */

export default class WindowWalkerSearchProvider {
    initialize(context) {
        this.invoke = context.invoke;
        this.config = context.config || {};
        this._cache = null;
        this._cacheTime = 0;
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    match(_query) {
        // All work is async (needs IPC to list windows)
        return [];
    }

    async matchAsync(query) {
        const trigger = this.config.trigger || 'w ';
        const lower = query.toLowerCase();

        // Must start with the trigger
        if (!lower.startsWith(trigger.toLowerCase())) return [];

        const filter = query.substring(trigger.length).toLowerCase().trim();

        // Fetch window list (cache for 500ms to avoid hammering on each keystroke)
        const now = Date.now();
        if (!this._cache || now - this._cacheTime > 500) {
            try {
                this._cache = await this.invoke('list_open_windows');
                this._cacheTime = now;
            } catch (e) {
                console.warn('[WindowWalker] Failed to list windows:', e);
                return [];
            }
        }

        if (!this._cache || this._cache.length === 0) return [];

        // Filter by title or process name
        let windows = this._cache;
        if (filter) {
            windows = windows.filter(w =>
                w.title.toLowerCase().includes(filter) ||
                w.process_name.toLowerCase().includes(filter)
            );
        }

        return windows.map((w, i) => {
            let icon = '🪟';
            if (this.config.show_icons !== false && w.icon_base64) {
                icon = w.icon_base64.startsWith('data:') ? w.icon_base64 : 'data:image/png;base64,' + w.icon_base64;
            }
            return {
                id: 'window:' + w.handle,
                type: 'window',
                label: w.title,
                description: w.process_name,
                icon,
                score: 95 - i,
                data: { handle: w.handle, process_name: w.process_name, icon_base64: w.icon_base64 },
            };
        });
    }

    execute(result) {
        if (result.data?.handle != null) {
            this.invoke('focus_open_window', { handle: result.data.handle });
        }
        return null; // no copy/display action — the Rust side hides the window
    }

    destroy() {
        this._cache = null;
    }
}
