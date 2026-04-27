/**
 * Todos toolbar provider (sandboxed).
 * Shows a task summary as an ephemeral chat bubble on click.
 */
const STORAGE_KEY = 'kage-todos';

export default class TodosToolbarProvider {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
    }

    onConfigUpdate(config) { this.config = config || {}; }

    getButtons() {
        // Icon is a single character (emoji) — the sandbox contract
        // doesn't allow SVG passthrough because the host renders it as
        // text content rather than HTML.
        return [{
            id: 'todos-summary',
            icon: '✅',
            tooltip: 'Show task summary',
        }];
    }

    async onClick(buttonId, _ctx) {
        if (buttonId !== 'todos-summary') return {};
        const todos = await this._loadTodos();
        const stats = this._getStats(todos);
        const html = this._renderSummary(todos, stats);
        return {
            host: {
                type: 'show_ephemeral_message',
                tag: 'summary',
                title: '📋 Task Summary',
                html,
            },
        };
    }

    destroy() {}

    // --- Internals ---

    async _loadTodos() {
        try {
            const raw = await this.invoke('load_extension_data', { key: STORAGE_KEY });
            return raw ? JSON.parse(raw) : [];
        } catch {
            return [];
        }
    }

    _getStats(todos) {
        const total = todos.length;
        const complete = todos.filter(t => t.status === 'complete').length;
        const pending = total - complete;
        const now = new Date(); now.setHours(0, 0, 0, 0);
        const overdue = todos.filter(t => {
            if (!t.dueDate || t.status === 'complete') return false;
            const d = this._parseDue(t.dueDate);
            return d && d < now;
        }).length;
        return { total, complete, pending, overdue };
    }

    _parseDue(due) {
        if (!due) return null;
        const parts = due.split('-');
        if (parts.length === 3) {
            return new Date(parseInt(parts[0]), parseInt(parts[1]) - 1, parseInt(parts[2]));
        }
        const d = new Date(due);
        return isNaN(d) ? null : d;
    }

    _renderSummary(todos, stats) {
        if (stats.total === 0) {
            return '<p>No tasks yet. Type <code>todo+ buy milk</code> in the floating window to add one.</p>';
        }

        const now = new Date(); now.setHours(0, 0, 0, 0);
        const priorityOrder = { high: 0, medium: 1, low: 2, '': 3 };
        const pending = todos.filter(t => t.status !== 'complete').slice();
        const completed = todos.filter(t => t.status === 'complete');

        pending.sort((a, b) => {
            const aDue = this._parseDue(a.dueDate);
            const bDue = this._parseDue(b.dueDate);
            const aOverdue = aDue && aDue < now ? 0 : 1;
            const bOverdue = bDue && bDue < now ? 0 : 1;
            if (aOverdue !== bOverdue) return aOverdue - bOverdue;
            const aPri = priorityOrder[a.priority || ''] ?? 3;
            const bPri = priorityOrder[b.priority || ''] ?? 3;
            if (aPri !== bPri) return aPri - bPri;
            if (aDue && bDue) return aDue - bDue;
            return 0;
        });

        const lines = [];
        lines.push(
            `<p><strong>${stats.complete}/${stats.total}</strong> done` +
            (stats.overdue > 0 ? `, <span style="color:#e25">⚠ ${stats.overdue} overdue</span>` : '') +
            '</p>',
        );

        if (pending.length > 0) {
            lines.push('<ul>');
            for (const t of pending) {
                const d = this._parseDue(t.dueDate);
                const overdue = d && d < now;
                const due = t.dueDate ? ` <small>(due ${escape(t.dueDate)}${overdue ? ' ⚠' : ''})</small>` : '';
                const pri = t.priority === 'high' ? ' 🔴'
                          : t.priority === 'medium' ? ' 🟡' : '';
                const cat = t.category ? ` <small>[${escape(t.category)}]</small>` : '';
                lines.push(`<li>${escape(t.text)}${pri}${cat}${due}</li>`);
            }
            lines.push('</ul>');
        }

        if (completed.length > 0 && this.config?.show_completed !== false) {
            const shown = completed.slice(0, 5);
            lines.push('<p style="margin-top:8px;"><small>Recently completed:</small></p><ul>');
            for (const t of shown) lines.push(`<li><s>${escape(t.text)}</s></li>`);
            if (completed.length > 5) {
                lines.push(`<li><small>…and ${completed.length - 5} more</small></li>`);
            }
            lines.push('</ul>');
        }

        return lines.join('');
    }
}

function escape(s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
