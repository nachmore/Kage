/**
 * Todos Settings Module
 * Configures default category, sort order, display preferences.
 */
class TodosExtSettingsModule extends SettingsModule {
    constructor() {
        super('todos', 'Todos', '✅');
        this.description = 'Task manager. Type "todo! <task>" to add, "todos" to view list.';
    }

    renderContent() {
        return `
            ${this.createControlRow(
                'Default Category',
                'New todos get this category unless overridden with #tag.',
                '<input type="text" class="setting-input" id="todosDefaultCategory" placeholder="e.g. work, personal" style="max-width:200px;">'
            )}
            ${this.createControlRow(
                'Sort By',
                'How to order todos in the list.',
                `<select class="setting-input" id="todosSortBy" style="max-width:200px;">
                    <option value="created">Newest first</option>
                    <option value="due">Due date</option>
                    <option value="priority">Priority</option>
                    <option value="status">Status</option>
                </select>`
            )}
            ${this.createCheckboxRow(
                'Show Completed',
                'Display completed todos in the main list.',
                'todosShowCompleted',
                true
            )}
            ${this.createCheckboxRow(
                'Confirm Delete',
                'Ask for confirmation before deleting a todo.',
                'todosConfirmDelete',
                true
            )}
            <div class="setting-row">
                <div class="setting-label">Data</div>
                <div class="setting-description">Export or clear all your todos.</div>
                <div class="setting-control" style="display:flex;gap:8px;margin-top:6px;">
                    <button class="setting-button" id="todosExport">Export JSON</button>
                    <button class="setting-button" id="todosImport">Import JSON</button>
                    <button class="setting-button danger" id="todosClearAll">Clear All</button>
                </div>
            </div>
        `;
    }

    render() { return this.renderContent(); }

    initialize() {
        document.getElementById('todosExport')?.addEventListener('click', () => this._export());
        document.getElementById('todosImport')?.addEventListener('click', () => this._import());
        document.getElementById('todosClearAll')?.addEventListener('click', () => this._clearAll());
    }

    load(config) {
        const ext = (config.extensions && config.extensions['todos']) || {};
        const cat = document.getElementById('todosDefaultCategory');
        const sort = document.getElementById('todosSortBy');
        const show = document.getElementById('todosShowCompleted');
        const confirm = document.getElementById('todosConfirmDelete');
        if (cat) cat.value = ext.default_category || '';
        if (sort) sort.value = ext.sort_by || 'created';
        if (show) show.checked = ext.show_completed !== false;
        if (confirm) confirm.checked = ext.confirm_delete !== false;
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions['todos'] = {
            default_category: document.getElementById('todosDefaultCategory')?.value || '',
            sort_by: document.getElementById('todosSortBy')?.value || 'created',
            show_completed: document.getElementById('todosShowCompleted')?.checked ?? true,
            confirm_delete: document.getElementById('todosConfirmDelete')?.checked ?? true,
        };
    }

    validate() { return { valid: true }; }

    _export() {
        try {
            const raw = localStorage.getItem('kiro-todos') || '[]';
            const blob = new Blob([raw], { type: 'application/json' });
            const url = URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = url;
            a.download = 'kiro-todos.json';
            a.click();
            URL.revokeObjectURL(url);
        } catch (e) {
            console.error('Todos export failed:', e);
        }
    }

    _import() {
        const input = document.createElement('input');
        input.type = 'file';
        input.accept = '.json';
        input.onchange = async (e) => {
            const file = e.target.files?.[0];
            if (!file) return;
            try {
                const text = await file.text();
                const data = JSON.parse(text);
                if (!Array.isArray(data)) throw new Error('Invalid format');
                localStorage.setItem('kiro-todos', JSON.stringify(data));
                alert(`Imported ${data.length} todos.`);
            } catch (err) {
                alert('Failed to import: ' + err.message);
            }
        };
        input.click();
    }

    _clearAll() {
        if (!confirm('Delete ALL todos? This cannot be undone.')) return;
        localStorage.removeItem('kiro-todos');
        alert('All todos cleared.');
    }

    destroy() {}
}

window.TodosExtSettingsModule = TodosExtSettingsModule;
