/**
 * Todos toolbar button — shows badge with pending count and progress bar.
 * Clicking it types "todos" into the input to show the list.
 */
export default class TodosToolbarProvider {
    initialize(context) {
        this.config = context.config || {};
        this._refreshInterval = null;
        this._startAutoRefresh();
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    _getStats() {
        try {
            const raw = localStorage.getItem('kiro-todos');
            const todos = raw ? JSON.parse(raw) : [];
            const total = todos.length;
            const complete = todos.filter(t => t.status === 'complete').length;
            const overdue = todos.filter(t => {
                if (!t.dueDate || t.status === 'complete') return false;
                const now = new Date();
                now.setHours(0, 0, 0, 0);
                return new Date(t.dueDate) < now;
            }).length;
            const pending = total - complete;
            const pct = total > 0 ? Math.round((complete / total) * 100) : 0;
            return { total, complete, pending, overdue, pct };
        } catch {
            return { total: 0, complete: 0, pending: 0, overdue: 0, pct: 0 };
        }
    }

    _startAutoRefresh() {
        // Refresh badge every 30s to catch overdue changes
        this._refreshInterval = setInterval(() => {
            this._updateBadge();
        }, 30000);
    }

    _updateBadge() {
        const badge = document.querySelector('.todos-toolbar-badge');
        if (!badge) return;
        const stats = this._getStats();
        badge.textContent = stats.pending > 0 ? stats.pending : '';
        badge.style.display = stats.pending > 0 ? '' : 'none';
        badge.classList.toggle('has-overdue', stats.overdue > 0);
    }

    getButtons() {
        const stats = this._getStats();
        return [{
            id: 'todos-badge',
            icon: this._renderBadgeHtml(stats),
            tooltip: this._buildTooltip(stats),
            onClick: () => {
                // Set input to "todos" to trigger the search provider
                const input = document.querySelector('#floatingInput, #chatInput, input[type="text"]');
                if (input) {
                    input.value = 'todos';
                    input.dispatchEvent(new Event('input', { bubbles: true }));
                    input.focus();
                }
            },
        }];
    }

    _renderBadgeHtml(stats) {
        const pendingBadge = stats.pending > 0
            ? `<span class="todos-toolbar-badge${stats.overdue > 0 ? ' has-overdue' : ''}">${stats.pending}</span>`
            : '';
        return `<span class="todos-toolbar-icon">✅${pendingBadge}</span>`;
    }

    _buildTooltip(stats) {
        if (stats.total === 0) return 'Todos — no tasks';
        const parts = [`${stats.complete}/${stats.total} done (${stats.pct}%)`];
        if (stats.overdue > 0) parts.push(`${stats.overdue} overdue`);
        return `Todos — ${parts.join(', ')}`;
    }

    destroy() {
        if (this._refreshInterval) {
            clearInterval(this._refreshInterval);
            this._refreshInterval = null;
        }
    }
}
