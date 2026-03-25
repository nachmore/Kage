/**
 * Todos search provider.
 * - "todo" → show summary + active items
 * - "todo+ buy milk" → quick-add a new todo
 * - "todo-" → select a todo to delete
 * - "todo #work" → filter by category
 * - "todo /done" → show completed
 * - "todo /overdue" → show overdue
 */

const STORAGE_KEY = 'kiro-todos';

export default class TodosSearchProvider {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
        this.todos = [];
        this._loadFailed = false;
        this._ready = this._load();
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    // --- Persistence via file (through Tauri IPC) ---

    async _load() {
        try {
            const invoke = this.invoke || window.__TAURI__?.core?.invoke;
            if (!invoke) { this.todos = []; return; }
            const raw = await invoke('load_extension_data', { key: STORAGE_KEY });
            this.todos = raw ? JSON.parse(raw) : [];
            this._loadFailed = false;
        } catch (e) {
            console.error('Todos: failed to load', e);
            this.todos = [];
            this._loadFailed = true;
        }
    }

    async _save() {
        if (this._loadFailed) {
            console.warn('Todos: skipping save — last load failed');
            return;
        }
        try {
            const invoke = this.invoke || window.__TAURI__?.core?.invoke;
            if (!invoke) return;
            await invoke('save_extension_data', { key: STORAGE_KEY, data: JSON.stringify(this.todos) });
        } catch (e) {
            console.error('Todos: failed to save', e);
        }
    }

    // --- Data helpers ---

    _generateId() {
        return Date.now().toString(36) + Math.random().toString(36).slice(2, 7);
    }

    _isOverdue(todo) {
        if (!todo.dueDate || todo.status === 'complete') return false;
        const now = new Date();
        now.setHours(0, 0, 0, 0);
        return new Date(todo.dueDate) < now;
    }

    _isDueToday(todo) {
        if (!todo.dueDate || todo.status === 'complete') return false;
        const now = new Date();
        const due = new Date(todo.dueDate);
        return now.toDateString() === due.toDateString();
    }

    _parseDueDate(text) {
        // Natural-ish date parsing: "tomorrow", "next friday", "2026-03-15", "mar 15"
        const lower = text.toLowerCase().trim();
        const now = new Date();

        if (lower === 'today') {
            return this._formatDate(now);
        }
        if (lower === 'tomorrow') {
            const d = new Date(now);
            d.setDate(d.getDate() + 1);
            return this._formatDate(d);
        }
        const nextDayMatch = lower.match(/^next\s+(monday|tuesday|wednesday|thursday|friday|saturday|sunday)$/);
        if (nextDayMatch) {
            const days = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday'];
            const target = days.indexOf(nextDayMatch[1]);
            const d = new Date(now);
            let diff = target - d.getDay();
            if (diff <= 0) diff += 7;
            d.setDate(d.getDate() + diff);
            return this._formatDate(d);
        }
        // ISO date
        if (/^\d{4}-\d{2}-\d{2}$/.test(lower)) return lower;
        // "mar 15", "march 15"
        const monthMatch = lower.match(/^(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)\w*\s+(\d{1,2})$/);
        if (monthMatch) {
            const months = ['jan', 'feb', 'mar', 'apr', 'may', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec'];
            const m = months.indexOf(monthMatch[1]);
            const d = new Date(now.getFullYear(), m, parseInt(monthMatch[2]));
            if (d < now) d.setFullYear(d.getFullYear() + 1);
            return this._formatDate(d);
        }
        return null;
    }

    _formatDate(d) {
        return d.toISOString().split('T')[0];
    }

    _formatDateDisplay(dateStr) {
        if (!dateStr) return '';
        const d = new Date(dateStr);
        const now = new Date();
        now.setHours(0, 0, 0, 0);
        const diff = Math.round((d - now) / 86400000);
        if (diff === 0) return '📅 Today';
        if (diff === 1) return '📅 Tomorrow';
        if (diff === -1) return '📅 Yesterday';
        if (diff < -1) return `📅 ${Math.abs(diff)}d overdue`;
        if (diff <= 7) return `📅 In ${diff}d`;
        return `📅 ${dateStr}`;
    }

    getStats() {
        const total = this.todos.length;
        const complete = this.todos.filter(t => t.status === 'complete').length;
        const inProgress = this.todos.filter(t => t.status === 'in-progress').length;
        const overdue = this.todos.filter(t => this._isOverdue(t)).length;
        const pending = total - complete;
        return { total, complete, inProgress, overdue, pending };
    }

    // --- Sorting ---

    _sortTodos(todos) {
        const sortBy = this.config.sort_by || 'created';
        const statusOrder = { 'in-progress': 0, 'pending': 1, 'complete': 2 };
        const priorityOrder = { 'high': 0, 'medium': 1, 'low': 2 };

        return [...todos].sort((a, b) => {
            // Always push completed to bottom
            if (a.status === 'complete' && b.status !== 'complete') return 1;
            if (b.status === 'complete' && a.status !== 'complete') return -1;

            // Overdue items first (among non-complete)
            const aOverdue = this._isOverdue(a);
            const bOverdue = this._isOverdue(b);
            if (aOverdue && !bOverdue) return -1;
            if (bOverdue && !aOverdue) return 1;

            switch (sortBy) {
                case 'due':
                    if (a.dueDate && !b.dueDate) return -1;
                    if (!a.dueDate && b.dueDate) return 1;
                    if (a.dueDate && b.dueDate) return a.dueDate.localeCompare(b.dueDate);
                    return 0;
                case 'priority':
                    return (priorityOrder[a.priority] ?? 1) - (priorityOrder[b.priority] ?? 1);
                case 'status':
                    return (statusOrder[a.status] ?? 1) - (statusOrder[b.status] ?? 1);
                case 'created':
                default:
                    return (b.createdAt || 0) - (a.createdAt || 0);
            }
        });
    }

    // --- Search matching ---

    match(query) {
        const lower = query.trim().toLowerCase();

        // "todo" alone → summary as first item + a few recent todos below
        if (lower === 'todo') {
            const stats = this.getStats();
            const pct = stats.total > 0 ? Math.round((stats.complete / stats.total) * 100) : 0;
            const results = [{
                id: 'todo:summary',
                type: 'todo_action',
                label: `📋 Todos ${stats.complete}/${stats.total} ${this._renderProgressText(pct)}`,
                description: 'Press Enter for full summary · todo+ to add · todo- to remove',
                icon: '✅',
                score: 100,
                data: { action: 'summary' },
            }];

            // Show a few active/overdue items below the summary
            const active = this.todos.filter(t => t.status !== 'complete');
            const sorted = this._sortTodos(active);
            for (const todo of sorted.slice(0, 5)) {
                const statusIcon = this._isOverdue(todo) ? '🔴'
                    : todo.status === 'in-progress' ? '🔵' : '⬜';
                const parts = [
                    todo.category ? `#${todo.category}` : '',
                    this._formatDateDisplay(todo.dueDate),
                ].filter(Boolean).join(' · ');
                results.push({
                    id: `todo:${todo.id}`,
                    type: 'todo_item',
                    label: `${statusIcon} ${todo.text}`,
                    description: parts || 'Enter: cycle status',
                    icon: statusIcon,
                    score: 90,
                    data: { action: 'cycle', todoId: todo.id },
                });
            }
            return results;
        }

        // "todo+" alone → hint
        if (lower === 'todo+') {
            return [{
                id: 'todo:add-hint',
                type: 'todo_header',
                label: '➕ Type a task to add',
                description: 'e.g. todo+ buy milk #shopping @high due:friday',
                icon: '✅',
                score: 100,
                data: { action: 'none' },
            }];
        }

        // "todo+ buy milk" → quick add
        const addMatch = query.trim().match(/^todo\+\s+(.+)$/i);
        if (addMatch) {
            return this._buildQuickAdd(addMatch[1]);
        }

        // "todo-" alone → hint or full delete list
        if (lower === 'todo-') {
            if (this.todos.length === 0) {
                return [{
                    id: 'todo:del-empty',
                    type: 'todo_header',
                    label: '🗑️ No todos to remove',
                    description: 'Use todo+ to add tasks first',
                    icon: '✅',
                    score: 100,
                    data: { action: 'none' },
                }];
            }
            return this._buildDeleteList(null);
        }

        // "todo- <search>" → filtered delete list, no header
        if (lower.startsWith('todo- ')) {
            const term = lower.replace(/^todo-\s+/, '');
            const filterFn = t => t.text.toLowerCase().includes(term) || (t.category || '').toLowerCase().includes(term);
            const results = this._buildDeleteList(filterFn, { showHeader: false });
            if (results.length === 0) {
                return [{
                    id: 'todo:del-none',
                    type: 'todo_header',
                    label: `🗑️ No todos matching "${term}"`,
                    description: 'Try a different search',
                    icon: '✅',
                    score: 100,
                    data: { action: 'none' },
                }];
            }
            return results;
        }

        // "todo #category" → filter by category
        const catMatch = lower.match(/^todo\s+#(\S+)$/);
        if (catMatch) {
            return this._buildTodoList(t =>
                (t.category || '').toLowerCase() === catMatch[1]
            );
        }

        // "todo /done" → show completed
        if (lower === 'todo /done') {
            return this._buildTodoList(t => t.status === 'complete');
        }

        // "todo /overdue" → show overdue
        if (lower === 'todo /overdue') {
            return this._buildTodoList(t => this._isOverdue(t));
        }

        // "todo /active" → show non-complete
        if (lower === 'todo /active') {
            return this._buildTodoList(t => t.status !== 'complete');
        }

        // "todo /high" etc → filter by priority
        const prioMatch = lower.match(/^todo\s+\/(high|medium|low)$/);
        if (prioMatch) {
            return this._buildTodoList(t => t.priority === prioMatch[1]);
        }

        // "todo <search>" → search within todos, no summary
        const searchMatch = lower.match(/^todo\s+(.+)$/);
        if (searchMatch) {
            const term = searchMatch[1];
            if (!term.startsWith('#') && !term.startsWith('/')) {
                const filtered = this.todos.filter(t =>
                    t.text.toLowerCase().includes(term) ||
                    (t.category || '').toLowerCase().includes(term)
                );
                if (filtered.length === 0) {
                    return [{
                        id: 'todo:no-match',
                        type: 'todo_header',
                        label: `No todos matching "${term}"`,
                        description: `Use todo+ ${term} to create one`,
                        icon: '📋',
                        score: 100,
                        data: { action: 'none' },
                    }];
                }
                return this._buildTodoList(t =>
                    t.text.toLowerCase().includes(term) ||
                    (t.category || '').toLowerCase().includes(term)
                );
            }
        }

        return [];
    }

    _buildQuickAdd(rawText) {
        // Parse: "buy milk #shopping @high due:tomorrow"
        let text = rawText;
        let category = this.config.default_category || '';
        let priority = 'medium';
        let dueDate = null;

        // Extract #category
        const catExtract = text.match(/#(\S+)/);
        if (catExtract) {
            category = catExtract[1];
            text = text.replace(catExtract[0], '').trim();
        }

        // Extract @priority
        const prioExtract = text.match(/@(high|medium|low)/i);
        if (prioExtract) {
            priority = prioExtract[1].toLowerCase();
            text = text.replace(prioExtract[0], '').trim();
        }

        // Extract due:date
        const dueExtract = text.match(/due:(\S+)/i);
        if (dueExtract) {
            dueDate = this._parseDueDate(dueExtract[1]);
            text = text.replace(dueExtract[0], '').trim();
        }

        if (!text) return [];

        const desc = [
            category ? `#${category}` : '',
            `@${priority}`,
            dueDate ? `due: ${dueDate}` : '',
        ].filter(Boolean).join(' · ');

        return [{
            id: 'todo:add',
            type: 'todo_action',
            label: `➕ Add: ${text}`,
            description: desc || 'Press Enter to add',
            icon: '✅',
            score: 95,
            data: {
                action: 'add',
                text,
                category,
                priority,
                dueDate,
            },
        }];
    }

    _buildTodoList(filterFn) {
        let filtered = filterFn ? this.todos.filter(filterFn) : this.todos;
        const showCompleted = this.config.show_completed !== false;
        if (!showCompleted && !filterFn) {
            filtered = filtered.filter(t => t.status !== 'complete');
        }

        const sorted = this._sortTodos(filtered);
        const stats = this.getStats();
        const results = [];

        // Header with stats and progress
        const pct = stats.total > 0 ? Math.round((stats.complete / stats.total) * 100) : 0;
        const progressBar = this._renderProgressText(pct);
        results.push({
            id: 'todo:header',
            type: 'todo_header',
            label: `📋 Todos ${stats.complete}/${stats.total} ${progressBar}`,
            description: [
                stats.overdue > 0 ? `🔴 ${stats.overdue} overdue` : '',
                stats.inProgress > 0 ? `🔵 ${stats.inProgress} in progress` : '',
                'Type todo+ <task> to add · todo- to remove',
            ].filter(Boolean).join(' · '),
            icon: '✅',
            score: 100,
            data: { action: 'none' },
        });

        // Todo items
        for (const todo of sorted.slice(0, 20)) {
            const statusIcon = todo.status === 'complete' ? '✅'
                : todo.status === 'in-progress' ? '🔵'
                : this._isOverdue(todo) ? '🔴'
                : '⬜';

            const prioIcon = todo.priority === 'high' ? '🔺'
                : todo.priority === 'low' ? '🔽' : '';

            const parts = [
                todo.category ? `#${todo.category}` : '',
                prioIcon,
                this._formatDateDisplay(todo.dueDate),
            ].filter(Boolean).join(' · ');

            results.push({
                id: `todo:${todo.id}`,
                type: 'todo_item',
                label: `${statusIcon} ${todo.text}`,
                description: parts || 'Enter: cycle status',
                icon: statusIcon,
                score: 90 - sorted.indexOf(todo),
                data: { action: 'cycle', todoId: todo.id },
            });
        }

        if (sorted.length > 20) {
            results.push({
                id: 'todo:more',
                type: 'todo_header',
                label: `... and ${sorted.length - 20} more`,
                description: 'Use filters: /done /overdue /active /high #category',
                icon: '📋',
                score: 0,
                data: { action: 'none' },
            });
        }

        // Clear completed action
        if (stats.complete > 0) {
            results.push({
                id: 'todo:clear',
                type: 'todo_action',
                label: `🧹 Clear ${stats.complete} completed`,
                description: 'Press Enter to remove completed todos',
                icon: '🧹',
                score: -1,
                data: { action: 'clear_completed' },
            });
        }

        return results;
    }

    _renderProgressText(pct) {
        const filled = Math.round(pct / 10);
        return '▓'.repeat(filled) + '░'.repeat(10 - filled) + ` ${pct}%`;
    }

    // --- Execution ---

    _buildDeleteList(filterFn, { showHeader = true } = {}) {
        let filtered = filterFn ? this.todos.filter(filterFn) : this.todos;
        const sorted = this._sortTodos(filtered);
        const results = [];

        if (showHeader) {
            results.push({
                id: 'todo:delete-header',
                type: 'todo_header',
                label: '🗑️ Select a todo to remove',
                description: 'Type todo- <search> to filter',
                icon: '✅',
                score: 100,
                data: { action: 'none' },
            });
        }

        for (const todo of sorted.slice(0, 20)) {
            const statusIcon = todo.status === 'complete' ? '✅'
                : todo.status === 'in-progress' ? '🔵' : '⬜';
            results.push({
                id: `todo:del:${todo.id}`,
                type: 'todo_item',
                label: `🗑️ ${statusIcon} ${todo.text}`,
                description: 'Press Enter to delete',
                icon: '✅',
                score: 90 - sorted.indexOf(todo),
                data: { action: 'delete', todoId: todo.id },
            });
        }

        return results;
    }

    _buildSummaryText() {
        const stats = this.getStats();
        const lines = [];

        const pct = stats.total > 0 ? Math.round((stats.complete / stats.total) * 100) : 0;
        lines.push(`📋 **Todos** — ${stats.complete}/${stats.total} done (${pct}%)`);

        if (stats.total === 0) {
            lines.push('\nNo tasks yet. Use `todo+ <task>` to add one.');
            return { type: 'display', value: lines.join('\n') };
        }

        // Overdue
        const overdue = this.todos.filter(t => t.status !== 'complete' && this._isOverdue(t));
        if (overdue.length > 0) {
            lines.push('\n🔴 **Overdue:**');
            for (const t of overdue) lines.push(`- ${t.text}${t.dueDate ? ` *(due ${this._formatDateDisplay(t.dueDate)})*` : ''}`);
        }

        // In progress
        const active = this.todos.filter(t => t.status === 'in-progress');
        if (active.length > 0) {
            lines.push('\n🔵 **In Progress:**');
            for (const t of active) lines.push(`- ${t.text}`);
        }

        // Pending (not overdue)
        const pending = this.todos.filter(t => t.status === 'pending' && !this._isOverdue(t));
        if (pending.length > 0) {
            lines.push(`\n⬜ **Pending** (${pending.length}):`);
            for (const t of pending.slice(0, 10)) {
                const due = t.dueDate ? ` *(${this._formatDateDisplay(t.dueDate)})*` : '';
                lines.push(`- ${t.text}${due}`);
            }
            if (pending.length > 10) lines.push(`- *...and ${pending.length - 10} more*`);
        }

        if (stats.complete > 0) {
            lines.push(`\n✅ ${stats.complete} completed`);
        }

        return { type: 'display', value: lines.join('\n') };
    }

    execute(result) {
        const data = result.data;
        if (!data) return null;

        switch (data.action) {
            case 'add':
                return this._addTodo(data);
            case 'cycle':
                return this._cycleTodoStatus(data.todoId);
            case 'delete':
                return this._deleteTodo(data.todoId);
            case 'clear_completed':
                return this._clearCompleted();
            case 'summary':
                return this._buildSummaryText();
            case 'none':
            default:
                return null;
        }
    }

    _addTodo(data) {
        const todo = {
            id: this._generateId(),
            text: data.text,
            status: 'pending',
            priority: data.priority || 'medium',
            category: data.category || '',
            dueDate: data.dueDate || null,
            createdAt: Date.now(),
        };
        this.todos.unshift(todo);
        this._save();
        return { type: 'display', value: `✅ Added: ${todo.text}` };
    }

    _cycleTodoStatus(todoId) {
        const todo = this.todos.find(t => t.id === todoId);
        if (!todo) return null;

        // pending → in-progress → complete → pending
        const cycle = { 'pending': 'in-progress', 'in-progress': 'complete', 'complete': 'pending' };
        todo.status = cycle[todo.status] || 'pending';
        this._save();

        const icons = { 'pending': '⬜', 'in-progress': '🔵', 'complete': '✅' };
        return { type: 'display', value: `${icons[todo.status]} ${todo.text} → ${todo.status}` };
    }

    _deleteTodo(todoId) {
        const idx = this.todos.findIndex(t => t.id === todoId);
        if (idx === -1) return null;
        const removed = this.todos.splice(idx, 1)[0];
        this._save();
        return { type: 'display', value: `🗑️ Removed: ${removed.text}` };
    }

    _clearCompleted() {
        const count = this.todos.filter(t => t.status === 'complete').length;
        this.todos = this.todos.filter(t => t.status !== 'complete');
        this._save();
        return { type: 'display', value: `🧹 Cleared ${count} completed todos` };
    }

    // --- Public API for toolbar badge ---

    getTodos() {
        return this.todos;
    }

    addTodo(text, opts = {}) {
        const todo = {
            id: this._generateId(),
            text,
            status: 'pending',
            priority: opts.priority || 'medium',
            category: opts.category || '',
            dueDate: opts.dueDate || null,
            createdAt: Date.now(),
        };
        this.todos.unshift(todo);
        this._save();
        return todo;
    }

    updateTodo(todoId, updates) {
        const todo = this.todos.find(t => t.id === todoId);
        if (!todo) return null;
        Object.assign(todo, updates);
        this._save();
        return todo;
    }

    deleteTodo(todoId) {
        const idx = this.todos.findIndex(t => t.id === todoId);
        if (idx === -1) return null;
        const removed = this.todos.splice(idx, 1)[0];
        this._save();
        return removed;
    }

    destroy() {}
}
