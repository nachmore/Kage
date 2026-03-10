/**
 * Folder Tools — Tool Provider
 *
 * Exposes pick_folder, scan_folder, and execute_folder_plan to the LLM agent.
 * The agent can use these to organize folders, find duplicates, sort files, etc.
 */
export default class FolderToolProvider {
    initialize(context) {
        this.invoke = context.invoke;
        this.config = context.config;
    }

    onConfigUpdate(config) {
        this.config = config;
    }

    /**
     * Custom timeouts: folder picker waits for user, scan can be slow on large dirs.
     */
    getToolTimeout(toolName) {
        switch (toolName) {
            case 'pick_folder': return 120000;      // 2 minutes — user interaction
            case 'scan_folder': return 60000;        // 1 minute — large directory I/O
            case 'execute_folder_plan': return 300000; // 5 minutes — includes user review time
            default: return 5000;
        }
    }

    getTools() {
        return [
            {
                name: 'pick_folder',
                description:
                    'Open a native folder picker dialog so the user can select a folder. ' +
                    'Use this when the user wants to organize, clean, or sort a folder but ' +
                    'has not provided a specific path and it is not a well-known folder. ' +
                    'Prefer get_common_folders first if the user mentions a folder by name ' +
                    '(e.g. "downloads", "pictures", "desktop").',
                parameters: {},
            },
            {
                name: 'get_common_folders',
                description:
                    'Get a map of well-known folder names to their absolute paths on this system. ' +
                    'Returns folders like downloads, documents, pictures, videos, music, desktop, ' +
                    'home, screenshots, fonts, temp, etc. Use this to resolve folder names the user ' +
                    'mentions (e.g. "organize my downloads") without needing the folder picker.',
                parameters: {},
            },
            {
                name: 'scan_folder',
                description:
                    'Scan a folder recursively and return a manifest of all files and directories ' +
                    'including names, sizes, dates, extensions, and content hashes for duplicate detection. ' +
                    'Also returns pre-computed duplicate groups (files with identical content). ' +
                    'Use this after obtaining a folder path to understand its contents before proposing an organization plan.',
                parameters: {
                    path: {
                        type: 'string',
                        description: 'Absolute path to the folder to scan',
                    },
                    max_depth: {
                        type: 'number',
                        description: 'Maximum recursion depth (default: 10)',
                        default: 10,
                    },
                    compute_hashes: {
                        type: 'boolean',
                        description: 'Whether to compute content hashes for duplicate detection (default: true)',
                        default: true,
                    },
                },
            },
            {
                name: 'execute_folder_plan',
                description:
                    'Execute a folder organization plan. Takes a list of operations (move, rename, delete) ' +
                    'and applies them to the folder. The user will be shown the plan and must approve before ' +
                    'execution proceeds. Moves are atomic per-file. Deletes are safe — files are ' +
                    'moved to a _kiro_trash subfolder, not permanently removed. Returns a result with success/failure ' +
                    'counts and a rollback manifest.',
                parameters: {
                    root: {
                        type: 'string',
                        description: 'Absolute path to the root folder (same as the scanned folder)',
                    },
                    operations: {
                        type: 'array',
                        description:
                            'Array of operations. Each is an object with: ' +
                            'action ("move", "rename", or "delete"), ' +
                            'from (relative path from root), ' +
                            'to (relative destination path, required for move/rename), ' +
                            'reason (short explanation, e.g. "temporary file", "empty directory", "organize by type"). ' +
                            'Example: [{"action":"move","from":"photo.jpg","to":"2026/03/photo.jpg","reason":"organize by date"}]',
                    },
                },
            },
        ];
    }

    async execute(toolName, params) {
        switch (toolName) {
            case 'pick_folder':
                return this._pickFolder();
            case 'get_common_folders':
                return this._getCommonFolders();
            case 'scan_folder':
                return this._scanFolder(params);
            case 'execute_folder_plan':
                return this._executePlan(params);
            default:
                return { error: `Unknown tool: ${toolName}` };
        }
    }

    async _pickFolder() {
        try {
            const path = await this.invoke('pick_folder');
            if (path) {
                return { result: { path } };
            }
            return { result: { path: null, message: 'User cancelled the folder picker' } };
        } catch (e) {
            return { error: `Failed to open folder picker: ${e.message || e}` };
        }
    }

    async _getCommonFolders() {
        try {
            const folders = await this.invoke('get_common_folders');
            return { result: { folders } };
        } catch (e) {
            return { error: `Failed to get common folders: ${e.message || e}` };
        }
    }

    async _scanFolder(params) {
        if (!params.path) {
            return { error: 'Missing required parameter: path' };
        }
        try {
            const result = await this.invoke('scan_folder', {
                path: params.path,
                maxDepth: params.max_depth ?? 10,
                computeHashes: params.compute_hashes ?? true,
            });
            return { result };
        } catch (e) {
            return { error: `Failed to scan folder: ${e.message || e}` };
        }
    }

    async _executePlan(params) {
        if (!params.root) {
            return { error: 'Missing required parameter: root' };
        }
        if (!params.operations || !Array.isArray(params.operations)) {
            return { error: 'Missing or invalid parameter: operations (must be an array)' };
        }

        // Show the plan for user review before executing.
        // We render a confirmation UI in the response area and wait for the user's decision.
        const approved = await this._showPlanConfirmation(params.root, params.operations);
        if (!approved) {
            return { result: { cancelled: true, message: 'User cancelled the folder plan.' } };
        }

        try {
            const result = await this.invoke('execute_folder_plan', {
                root: params.root,
                operations: params.operations,
            });
            return { result };
        } catch (e) {
            return { error: `Failed to execute folder plan: ${e.message || e}` };
        }
    }

    /**
     * Render the folder plan as a reviewable list and return a Promise
     * that resolves to true (approved) or false (rejected).
     */
    _showPlanConfirmation(root, operations) {
        return new Promise((resolve) => {
            // Find the response area — works in both floating and chat windows
            const responseText = document.querySelector('.response-text')
                || document.querySelector('#responseText')
                || document.querySelector('.chat-response');
            if (!responseText) {
                // No UI available — auto-approve (shouldn't happen in practice)
                resolve(true);
                return;
            }

            // Build the plan review UI
            const container = document.createElement('div');
            container.className = 'folder-plan-review';

            const header = document.createElement('div');
            header.className = 'folder-plan-header';
            header.textContent = `📁 Folder Plan — ${operations.length} operation${operations.length !== 1 ? 's' : ''}`;
            container.appendChild(header);

            const rootInfo = document.createElement('div');
            rootInfo.className = 'folder-plan-root';
            rootInfo.textContent = root;
            container.appendChild(rootInfo);

            const list = document.createElement('div');
            list.className = 'folder-plan-list';
            list.setAttribute('role', 'list');

            for (const op of operations) {
                const item = document.createElement('div');
                item.className = 'folder-plan-item';
                item.setAttribute('role', 'listitem');

                const icon = op.action === 'delete' ? '🗑️' : op.action === 'rename' ? '✏️' : '📦';
                const reason = op.reason ? ` (${op.reason})` : '';
                const label = op.action === 'delete'
                    ? `${icon} Delete: ${op.from}${reason}`
                    : `${icon} ${op.from} → ${op.to || '?'}${reason}`;

                item.textContent = label;
                list.appendChild(item);
            }
            container.appendChild(list);

            const actions = document.createElement('div');
            actions.className = 'taskplan-review-actions';
            actions.innerHTML = `
                <button class="taskplan-review-btn taskplan-run-btn" id="folderPlanRunBtn">▶ Run</button>
                <button class="taskplan-review-btn folder-plan-cancel-btn" id="folderPlanCancelBtn">✕ Cancel</button>
            `;
            container.appendChild(actions);

            // Clear existing content and add our plan UI
            responseText.innerHTML = '';
            responseText.appendChild(container);

            // Wire up buttons
            const runBtn = container.querySelector('#folderPlanRunBtn');
            const cancelBtn = container.querySelector('#folderPlanCancelBtn');

            const cleanup = () => {
                runBtn?.removeEventListener('click', onRun);
                cancelBtn?.removeEventListener('click', onCancel);
            };

            const onRun = (e) => {
                e.stopPropagation();
                cleanup();
                actions.remove();
                // Update header to show "running" with spinner
                header.innerHTML = `📁 Executing plan... <span class="folder-plan-spinner"></span>`;
                // Dim the list items to indicate they're being processed
                list.style.opacity = '0.5';
                resolve(true);
            };

            const onCancel = (e) => {
                e.stopPropagation();
                cleanup();
                container.remove();
                resolve(false);
            };

            runBtn?.addEventListener('mousedown', (e) => e.preventDefault());
            cancelBtn?.addEventListener('mousedown', (e) => e.preventDefault());
            runBtn?.addEventListener('click', onRun);
            cancelBtn?.addEventListener('click', onCancel);
        });
    }

    destroy() {}
}
