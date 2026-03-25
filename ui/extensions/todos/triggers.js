/**
 * Todos & Reminders trigger provider — emits signals for due items.
 */
export default class TodosTriggerProvider {
    initialize(context) {
        this.invoke = context.invoke;
        this._interval = null;
        this._lastNotified = new Set();
        this._startPolling();
    }

    onConfigUpdate(config) {
        this._config = config || {};
    }

    getTriggers() {
        return [
            { name: 'todos:item_due', description: 'A todo item is due today', icon: '🔔' },
            { name: 'todos:item_overdue', description: 'A todo item is overdue', icon: '🔴' },
            { name: 'todos:all_complete', description: 'All todos are complete', icon: '✅' },
        ];
    }

    _startPolling() {
        // Check every 5 minutes for due items
        this._checkDueItems();
        this._interval = setInterval(() => this._checkDueItems(), 300_000);
    }

    _checkDueItems() {
        try {
            const raw = localStorage.getItem('kiro-todos');
            const todos = raw ? JSON.parse(raw) : [];
            const now = new Date();
            const todayStr = now.toISOString().split('T')[0];

            for (const t of todos) {
                if (t.status === 'complete' || !t.dueDate) continue;
                const key = t.id + '_' + t.dueDate;

                // Parse as local date
                const parts = t.dueDate.split('-');
                const due = parts.length === 3
                    ? new Date(parseInt(parts[0]), parseInt(parts[1]) - 1, parseInt(parts[2]))
                    : new Date(t.dueDate);
                const dueStr = due.toISOString().split('T')[0];

                if (dueStr === todayStr && !this._lastNotified.has('due_' + key)) {
                    this._lastNotified.add('due_' + key);
                    this._emitSignal('todos:item_due', { id: t.id, text: t.text, dueDate: t.dueDate });
                }
                if (due < new Date(now.getFullYear(), now.getMonth(), now.getDate()) && !this._lastNotified.has('overdue_' + key)) {
                    this._lastNotified.add('overdue_' + key);
                    this._emitSignal('todos:item_overdue', { id: t.id, text: t.text, dueDate: t.dueDate });
                }
            }

            // Check if all complete
            const pending = todos.filter(t => t.status !== 'complete');
            if (todos.length > 0 && pending.length === 0 && !this._lastNotified.has('all_complete')) {
                this._lastNotified.add('all_complete');
                this._emitSignal('todos:all_complete', { total: todos.length });
            }
        } catch {}
    }

    _emitSignal(name, data) {
        if (!this.invoke) return;
        this.invoke('emit_automation_signal', { name, data }).catch(() => {});
    }

    destroy() {
        if (this._interval) { clearInterval(this._interval); this._interval = null; }
    }
}
