/**
 * Todos toolbar button — injects a task summary into the chat.
 * Icon matches the core toolbar style (16x16 SVG, stroke-based).
 */
export default class TodosToolbarProvider {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
        this._todos = [];
        this._ready = this._loadTodos();
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    async _loadTodos() {
        try {
            const invoke = this.invoke || window.__TAURI__?.core?.invoke;
            if (!invoke) return;
            const raw = await invoke('load_extension_data', { key: 'kiro-todos' });
            this._todos = raw ? JSON.parse(raw) : [];
        } catch {
            this._todos = [];
        }
    }

    _getTodos() {
        return this._todos;
    }

    _getStats(todos) {
        const total = todos.length;
        const complete = todos.filter(t => t.status === 'complete').length;
        const pending = total - complete;
        const now = new Date();
        now.setHours(0, 0, 0, 0);
        const overdue = todos.filter(t => {
            if (!t.dueDate || t.status === 'complete') return false;
            return new Date(t.dueDate) < now;
        }).length;
        return { total, complete, pending, overdue };
    }

    _formatSummary(todos, stats) {
        if (stats.total === 0) return '📋 **No tasks yet.** Type `todo+ buy milk` in the floating window to add one.';

        const lines = [];
        lines.push(`📋 **Tasks** — ${stats.complete}/${stats.total} done${stats.overdue > 0 ? `, ⚠️ ${stats.overdue} overdue` : ''}`);
        lines.push('');

        const pending = todos.filter(t => t.status !== 'complete');
        const completed = todos.filter(t => t.status === 'complete');

        // Sort pending: overdue first, then by priority (high→med→low), then by due date
        const priorityOrder = { high: 0, medium: 1, low: 2, '': 3 };
        const now = new Date();
        now.setHours(0, 0, 0, 0);

        pending.sort((a, b) => {
            const aOverdue = a.dueDate && new Date(a.dueDate) < now ? 0 : 1;
            const bOverdue = b.dueDate && new Date(b.dueDate) < now ? 0 : 1;
            if (aOverdue !== bOverdue) return aOverdue - bOverdue;
            const aPri = priorityOrder[a.priority || ''] ?? 3;
            const bPri = priorityOrder[b.priority || ''] ?? 3;
            if (aPri !== bPri) return aPri - bPri;
            if (a.dueDate && b.dueDate) return new Date(a.dueDate) - new Date(b.dueDate);
            return 0;
        });

        if (pending.length > 0) {
            for (const t of pending) {
                const due = t.dueDate ? ` (due ${t.dueDate})` : '';
                const overdue = t.dueDate && new Date(t.dueDate) < now ? ' ⚠️' : '';
                const pri = t.priority === 'high' ? ' 🔴' : t.priority === 'medium' ? ' 🟡' : '';
                const cat = t.category ? ` [${t.category}]` : '';
                lines.push(`- [ ] ${t.text}${pri}${cat}${due}${overdue}`);
            }
        }

        if (completed.length > 0 && this.config?.show_completed !== false) {
            if (pending.length > 0) lines.push('');
            const shown = completed.slice(0, 5);
            for (const t of shown) {
                lines.push(`- [x] ~~${t.text}~~`);
            }
            if (completed.length > 5) {
                lines.push(`- *...and ${completed.length - 5} more completed*`);
            }
        }

        return lines.join('\n');
    }

    getButtons() {
        const todos = this._getTodos();
        const stats = this._getStats(todos);
        const pendingLabel = stats.pending > 0 ? ` (${stats.pending})` : '';

        // SVG checkbox icon matching core toolbar style
        const icon = `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 11l3 3L22 4"/><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/></svg>`;

        return [{
            id: 'todos-summary',
            icon,
            tooltip: `Tasks${pendingLabel} — click to show summary`,
            onClick: (ctx) => {
                const summary = this._formatSummary(todos, stats);
                // Inject as a message in the chat
                const messagesArea = document.querySelector('.messages-area');
                if (messagesArea) {
                    this._injectSummaryBubble(messagesArea, summary);
                }
            },
        }];
    }

    _injectSummaryBubble(container, markdown) {
        // Remove any previous summary bubble
        container.querySelectorAll('.todos-summary-bubble').forEach(el => el.remove());

        const bubble = document.createElement('div');
        bubble.className = 'todos-summary-bubble';

        // Render markdown if marked is available, otherwise plain text
        const rendered = typeof marked !== 'undefined' && marked.parse
            ? marked.parse(markdown)
            : markdown.replace(/\n/g, '<br>');

        bubble.innerHTML = `
            <div class="todos-summary-header">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 11l3 3L22 4"/><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/></svg>
                <span>Task Summary</span>
                <button class="todos-summary-close" title="Dismiss">✕</button>
            </div>
            <div class="todos-summary-body">${rendered}</div>
        `;

        bubble.querySelector('.todos-summary-close').addEventListener('click', () => {
            bubble.remove();
        });

        container.appendChild(bubble);
        container.scrollTop = container.scrollHeight;
    }

    destroy() {}
}
