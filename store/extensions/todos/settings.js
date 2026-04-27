/**
 * Todos & Reminders settings provider (sandboxed).
 */
const STORAGE_KEY = 'kage-todos';

export default class TodosSettingsProvider {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
    }

    onConfigUpdate(config) { this.config = config || {}; }

    getSettings() {
        return {
            description: 'Task manager with reminders. Type "todo+ <task>" to add, "todo+ <task> due:<date>" for a reminder, "todos" to view list.',
            sections: [
                {
                    label: 'Display',
                    controls: [
                        {
                            type: 'text',
                            id: 'default_category',
                            label: 'Default Category',
                            description: 'New todos get this category unless overridden with #tag.',
                            default: '',
                            placeholder: 'e.g. work, personal',
                            maxWidth: 200,
                        },
                        {
                            type: 'select',
                            id: 'sort_by',
                            label: 'Sort By',
                            description: 'How to order todos in the list.',
                            default: 'created',
                            maxWidth: 200,
                            options: [
                                { value: 'created', label: 'Newest first' },
                                { value: 'due', label: 'Due date' },
                                { value: 'priority', label: 'Priority' },
                                { value: 'status', label: 'Status' },
                            ],
                        },
                        {
                            type: 'checkbox',
                            id: 'show_completed',
                            label: 'Show Completed',
                            description: 'Display completed todos in the main list.',
                            default: true,
                        },
                    ],
                },
                {
                    label: 'Behavior',
                    controls: [
                        {
                            type: 'checkbox',
                            id: 'confirm_delete',
                            label: 'Confirm Delete',
                            description: 'Ask for confirmation before deleting a todo.',
                            default: true,
                        },
                        {
                            type: 'checkbox',
                            id: 'show_due_banner',
                            label: 'Show Due Reminder Banner',
                            description: 'Show a banner in the floating window when reminders are due today or overdue.',
                            default: true,
                        },
                    ],
                },
                {
                    label: 'Data',
                    controls: [
                        {
                            type: 'info',
                            html: 'Export or clear all your todos and reminders.',
                        },
                        { type: 'action', id: 'export',   label: 'Export JSON', action: 'export' },
                        { type: 'action', id: 'import',   label: 'Import JSON', action: 'import' },
                        { type: 'action', id: 'clearAll', label: 'Clear All',   action: 'clear_all', variant: 'danger',
                          confirm: 'Delete ALL todos and reminders? This cannot be undone.' },
                    ],
                },
            ],
        };
    }

    async runAction(action, _values) {
        if (action === 'export') {
            try {
                const raw = await this.invoke('load_extension_data', { key: STORAGE_KEY });
                const data = raw || '[]';
                return {
                    host: {
                        type: 'download',
                        filename: 'kage-todos.json',
                        content: data,
                        mime: 'application/json',
                    },
                    status: '✅ Exported',
                };
            } catch (e) {
                return { status: `❌ Export failed: ${e?.message || e}` };
            }
        }
        if (action === 'import') {
            return { host: { type: 'pick_file', accept: '.json', action: 'import' } };
        }
        if (action === 'clear_all') {
            try {
                await this.invoke('delete_extension_data', { key: STORAGE_KEY });
                return { status: '✅ All todos and reminders cleared.' };
            } catch (e) {
                return { status: `❌ ${e?.message || e}` };
            }
        }
        return {};
    }

    async onFileSelected(params) {
        if (params.action !== 'import') return {};
        try {
            const data = JSON.parse(params.content);
            if (!Array.isArray(data)) throw new Error('Invalid format (expected JSON array)');
            await this.invoke('save_extension_data', {
                key: STORAGE_KEY,
                data: JSON.stringify(data),
            });
            return { status: `✅ Imported ${data.length} todos.` };
        } catch (e) {
            return { status: `❌ Import failed: ${e?.message || e}` };
        }
    }
}
