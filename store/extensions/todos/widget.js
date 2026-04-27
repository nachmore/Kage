/**
 * Todos due-reminder widget (sandboxed).
 * Mounts into the floating-bottom slot. Shows the next due-today /
 * overdue item with navigation + mark-done + dismiss actions.
 */
const STORAGE_KEY = 'kage-todos';

export default class TodosDueWidget {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
        this._dueItems = [];
        this._dueIndex = 0;
        this._dismissedSet = new Set();
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    getRefreshInterval() {
        // Reload + re-render every 5 minutes. That's frequent enough to
        // catch newly-added due-today items without hammering storage.
        return this.config?.show_due_banner === false ? 0 : 5 * 60_000;
    }

    async render() {
        if (this.config?.show_due_banner === false) return null;
        await this._reloadDue();
        if (this._dueItems.length === 0) return null;

        const item = this._dueItems[this._dueIndex];
        if (!item) return null;

        const dueLabel = this._formatDateDisplay(item.dueDate);
        const multi = this._dueItems.length > 1;
        const counter = multi ? `<span class="reminder-bar-counter">${this._dueIndex + 1}/${this._dueItems.length}</span>` : '';
        const navButtons = multi
            ? `<button data-ext-action="prev" class="extension-bar-btn" title="Previous">◀</button>
               <button data-ext-action="next" class="extension-bar-btn" title="Next">▶</button>`
            : '';

        return {
            className: 'extension-bar reminder-bar',
            html: `
                <span class="extension-bar-icon">🔔</span>
                <span class="extension-bar-text reminder-bar-text"
                      style="flex:1;font-size:13px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
                    ${escape(item.text)} ${escape(dueLabel)}
                </span>
                <div class="extension-bar-controls">
                    ${counter}
                    ${navButtons}
                    <button data-ext-action="done" class="extension-bar-btn" title="Mark done">✓</button>
                    <button data-ext-action="dismiss" class="extension-bar-btn" title="Dismiss all">✕</button>
                </div>
            `,
            actions: [
                { id: 'prev', rpc: 'prev' },
                { id: 'next', rpc: 'next' },
                { id: 'done', rpc: 'done' },
                { id: 'dismiss', rpc: 'dismiss' },
            ],
        };
    }

    async onAction(actionId) {
        switch (actionId) {
            case 'prev':
                this._dueIndex = (this._dueIndex - 1 + this._dueItems.length) % this._dueItems.length;
                return { rerender: true };
            case 'next':
                this._dueIndex = (this._dueIndex + 1) % this._dueItems.length;
                return { rerender: true };
            case 'done': {
                const item = this._dueItems[this._dueIndex];
                if (item) {
                    await this._markComplete(item.id);
                    this._dueItems.splice(this._dueIndex, 1);
                    if (this._dueItems.length > 0) {
                        this._dueIndex = this._dueIndex % this._dueItems.length;
                    }
                }
                return { rerender: true };
            }
            case 'dismiss':
                for (const it of this._dueItems) this._dismissedSet.add(it.id);
                this._dueItems = [];
                return { rerender: true };
            default:
                return {};
        }
    }

    destroy() {}

    // --- Internals ---

    async _reloadDue() {
        let todos = [];
        try {
            const raw = await this.invoke('load_extension_data', { key: STORAGE_KEY });
            todos = raw ? JSON.parse(raw) : [];
        } catch {
            todos = [];
        }

        const due = todos.filter(t => {
            if (this._dismissedSet.has(t.id)) return false;
            return this._isDueTodayOrOverdue(t);
        });
        // If the set changed structurally (new items, deleted items),
        // reset the index so we don't point past the end.
        const same = due.length === this._dueItems.length
            && due.every((t, i) => t.id === this._dueItems[i]?.id);
        if (!same) {
            this._dueItems = due;
            if (this._dueIndex >= due.length) this._dueIndex = 0;
        }
    }

    async _markComplete(id) {
        try {
            const raw = await this.invoke('load_extension_data', { key: STORAGE_KEY });
            const todos = raw ? JSON.parse(raw) : [];
            const t = todos.find(x => x.id === id);
            if (t) t.status = 'complete';
            await this.invoke('save_extension_data', {
                key: STORAGE_KEY,
                data: JSON.stringify(todos),
            });
        } catch (e) {
            console.warn('Todos: failed to mark complete', e);
        }
    }

    _isDueTodayOrOverdue(t) {
        if (!t.dueDate || t.status === 'complete') return false;
        const parts = t.dueDate.split('-');
        const d = parts.length === 3
            ? new Date(parseInt(parts[0]), parseInt(parts[1]) - 1, parseInt(parts[2]))
            : new Date(t.dueDate);
        if (isNaN(d)) return false;
        const today = new Date(); today.setHours(0, 0, 0, 0);
        return d <= today;
    }

    _formatDateDisplay(d) {
        if (!d) return '';
        const today = new Date(); today.setHours(0, 0, 0, 0);
        const parts = d.split('-');
        const parsed = parts.length === 3
            ? new Date(parseInt(parts[0]), parseInt(parts[1]) - 1, parseInt(parts[2]))
            : new Date(d);
        if (isNaN(parsed)) return '';
        parsed.setHours(0, 0, 0, 0);
        const diffDays = Math.round((parsed - today) / 86400000);
        if (diffDays === 0) return '(due today)';
        if (diffDays === -1) return '(1 day overdue)';
        if (diffDays < 0) return `(${Math.abs(diffDays)} days overdue)`;
        if (diffDays === 1) return '(due tomorrow)';
        return `(due in ${diffDays} days)`;
    }
}

function escape(s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
