/**
 * MCP Settings Module
 */
class McpSettingsModule extends SettingsModule {
    constructor() {
        super('mcp', 'MCP Servers', '🔌');
        this._mcpConfig = null;
        this._mcpPath = null;
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                <div class="setting-row">
                    <div class="setting-label">Configuration File</div>
                    <div style="display: flex; align-items: center; gap: 8px;">
                        <div class="setting-description" id="mcpPathDisplay" style="font-family: monospace; font-size: 12px; word-break: break-all; flex: 1;"></div>
                        <button class="setting-button" id="mcpOpenFileBtn" style="font-size: 12px; flex-shrink: 0;">Browse...</button>
                    </div>
                </div>

                <div class="setting-row">
                    <div class="setting-label">Servers</div>
                    <div class="setting-description">MCP servers provide tools the agent can use. Built-in servers are managed by Kage.</div>
                    <div id="mcpServerList" style="margin-top: 8px;"></div>
                    <div style="margin-top: 10px;">
                        <button class="setting-button" id="mcpAddServerBtn" style="font-size: 12px;">+ Add Server</button>
                    </div>
                </div>
            </div>
        `;
    }

    async load(config) {
        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) return;

        try {
            // Use custom path from config if set, otherwise default
            const customPath = config.mcp_config_path || null;
            this._mcpPath = customPath || await invoke('get_mcp_json_path');
            const pathEl = document.getElementById('mcpPathDisplay');
            if (pathEl) pathEl.textContent = this._mcpPath;

            this._mcpConfig = await invoke('get_mcp_config', { path: customPath });
            this._renderServerList();
        } catch (e) {
            console.warn('[MCP] Failed to load config:', e);
        }
    }

    save(config) {
        // MCP config is saved immediately on each change, not via the global save
    }

    validate() { return { valid: true }; }

    initialize() {
        document.getElementById('mcpAddServerBtn')?.addEventListener('click', () => this._showAddDialog());
        document.getElementById('mcpOpenFileBtn')?.addEventListener('click', async () => {
            const invoke = window.__TAURI__?.core?.invoke;
            if (!invoke) return;
            try {
                const { open } = window.__TAURI__.dialog;
                const selected = await open({
                    title: 'Select MCP configuration file',
                    filters: [{ name: 'JSON', extensions: ['json'] }],
                    defaultPath: this._mcpPath || undefined,
                });
                if (selected) {
                    this._mcpPath = selected;
                    document.getElementById('mcpPathDisplay').textContent = selected;
                    // Save the custom path to config
                    const config = await invoke('get_config');
                    config.mcp_config_path = selected;
                    await invoke('save_config', { config });
                    // Reload with the new path
                    this._mcpConfig = await invoke('get_mcp_config', { path: selected });
                    this._renderServerList();
                }
            } catch (e) {
                console.warn('[MCP] Browse failed:', e);
            }
        });
    }

    _getBuiltins() {
        return [
            {
                key: 'kage-computer-control',
                name: 'Computer Control',
                icon: '🖥️',
                description: 'UI automation, app launching, and desktop interaction',
                builtin: true,
                getCommand: () => {
                    // The command is the path to the MCP binary next to the main exe
                    const invoke = window.__TAURI__?.core?.invoke;
                    return invoke ? invoke('get_computer_control_enabled').then(() => true) : Promise.resolve(false);
                },
            },
        ];
    }

    _renderServerList() {
        const list = document.getElementById('mcpServerList');
        if (!list || !this._mcpConfig) return;

        const servers = this._mcpConfig.mcpServers || {};
        const builtins = this._getBuiltins();
        let html = '';

        // Render built-in servers first
        for (const b of builtins) {
            const entry = servers[b.key];
            const enabled = !!entry && !entry.disabled;
            const toggleId = `mcp-toggle-${b.key}`;
            html += `<div class="mcp-server-item">
                <div class="mcp-server-info">
                    <span class="mcp-server-icon">${b.icon}</span>
                    <div class="mcp-server-details">
                        <div class="mcp-server-name">${b.name} <span class="mcp-server-badge">Built-in</span></div>
                        <div class="mcp-server-desc">${b.description}</div>
                    </div>
                </div>
                <div class="mcp-server-actions">
                    <label class="kage-toggle">
                        <input type="checkbox" id="${toggleId}" ${enabled ? 'checked' : ''} data-key="${b.key}" data-builtin="true">
                        <span class="kage-toggle-slider"></span>
                    </label>
                </div>
            </div>`;
        }

        // Render user-defined servers
        const builtinKeys = new Set(builtins.map(b => b.key));
        for (const [key, entry] of Object.entries(servers)) {
            if (builtinKeys.has(key)) continue;
            const enabled = !entry.disabled;
            const cmd = entry.command || '';
            const args = (entry.args || []).join(' ');
            const toggleId = `mcp-toggle-${key}`;
            html += `<div class="mcp-server-item">
                <div class="mcp-server-info">
                    <span class="mcp-server-icon">📦</span>
                    <div class="mcp-server-details">
                        <div class="mcp-server-name">${esc(key)}</div>
                        <div class="mcp-server-desc mcp-server-cmd">${esc(cmd)} ${esc(args)}</div>
                    </div>
                </div>
                <div class="mcp-server-actions">
                    <button class="mcp-server-edit-btn" data-key="${esc(key)}" title="Edit">
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
                    </button>
                    <button class="mcp-server-delete-btn" data-key="${esc(key)}" title="Remove">
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                    </button>
                    <label class="kage-toggle">
                        <input type="checkbox" id="${toggleId}" ${enabled ? 'checked' : ''} data-key="${esc(key)}">
                        <span class="kage-toggle-slider"></span>
                    </label>
                </div>
            </div>`;
        }

        if (!html) {
            html = '<div style="color: var(--kage-text-muted); font-size: 12px; padding: 8px 0;">No MCP servers configured.</div>';
        }

        list.innerHTML = html;
        this._bindServerEvents(list);

        function esc(s) { const d = document.createElement('div'); d.textContent = s; return d.innerHTML; }
    }

    _bindServerEvents(list) {
        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) return;

        // Toggle switches
        list.querySelectorAll('input[type="checkbox"]').forEach(toggle => {
            toggle.addEventListener('change', async () => {
                const key = toggle.dataset.key;
                const isBuiltin = toggle.dataset.builtin === 'true';
                const enabled = toggle.checked;

                if (isBuiltin) {
                    await invoke('set_computer_control_enabled', { enabled });
                    this._mcpConfig = await invoke('get_mcp_config', { path: null });
                    this._renderServerList();
                } else {
                    const servers = this._mcpConfig.mcpServers || {};
                    if (servers[key]) {
                        if (enabled) { delete servers[key].disabled; }
                        else { servers[key].disabled = true; }
                        await invoke('save_mcp_config', { path: null, config: this._mcpConfig });
                        this._renderServerList();
                    }
                }
            });
        });

        // Edit buttons
        list.querySelectorAll('.mcp-server-edit-btn').forEach(btn => {
            btn.addEventListener('click', () => this._showEditDialog(btn.dataset.key));
        });

        // Delete buttons
        list.querySelectorAll('.mcp-server-delete-btn').forEach(btn => {
            btn.addEventListener('click', async () => {
                const key = btn.dataset.key;
                if (!confirm(`Remove MCP server "${key}"?`)) return;
                const servers = this._mcpConfig.mcpServers || {};
                delete servers[key];
                await invoke('save_mcp_config', { path: null, config: this._mcpConfig });
                this._renderServerList();
            });
        });
    }

    _showAddDialog() { this._showServerDialog(null); }
    _showEditDialog(key) { this._showServerDialog(key); }

    _showServerDialog(editKey) {
        const existing = editKey ? (this._mcpConfig?.mcpServers?.[editKey] || {}) : {};
        const isEdit = !!editKey;

        const overlay = document.createElement('div');
        overlay.className = 'mcp-dialog-overlay';
        overlay.innerHTML = `
            <div class="mcp-dialog">
                <div class="mcp-dialog-title">${isEdit ? 'Edit' : 'Add'} MCP Server</div>
                <label class="mcp-dialog-label">Name (unique key)</label>
                <input class="setting-input mcp-dialog-input" id="mcpDialogKey" value="${isEdit ? editKey : ''}" ${isEdit ? 'disabled' : ''} placeholder="e.g. my-server">
                <label class="mcp-dialog-label">Command</label>
                <input class="setting-input mcp-dialog-input" id="mcpDialogCmd" value="${existing.command || ''}" placeholder="e.g. python, uvx, node">
                <label class="mcp-dialog-label">Arguments (one per line)</label>
                <textarea class="setting-input mcp-dialog-input" id="mcpDialogArgs" rows="3" placeholder="e.g. -m my_server">${(existing.args || []).join('\n')}</textarea>
                <label class="mcp-dialog-label">Working Directory (optional)</label>
                <input class="setting-input mcp-dialog-input" id="mcpDialogCwd" value="${existing.cwd || ''}" placeholder="Leave empty for default">
                <div class="mcp-dialog-actions">
                    <button class="setting-button" id="mcpDialogCancel">Cancel</button>
                    <button class="setting-button mcp-dialog-save" id="mcpDialogSave">${isEdit ? 'Save' : 'Add'}</button>
                </div>
            </div>
        `;
        document.body.appendChild(overlay);

        overlay.querySelector('#mcpDialogCancel').addEventListener('click', () => overlay.remove());
        overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });

        overlay.querySelector('#mcpDialogSave').addEventListener('click', async () => {
            const key = document.getElementById('mcpDialogKey').value.trim();
            const cmd = document.getElementById('mcpDialogCmd').value.trim();
            const args = document.getElementById('mcpDialogArgs').value.trim().split('\n').filter(a => a.trim());
            const cwd = document.getElementById('mcpDialogCwd').value.trim();

            if (!key || !cmd) { alert('Name and command are required.'); return; }

            if (!this._mcpConfig.mcpServers) this._mcpConfig.mcpServers = {};
            this._mcpConfig.mcpServers[key] = { command: cmd, args, disabled: false };
            if (cwd) this._mcpConfig.mcpServers[key].cwd = cwd;

            const invoke = window.__TAURI__?.core?.invoke;
            await invoke('save_mcp_config', { path: null, config: this._mcpConfig });
            overlay.remove();
            this._renderServerList();
        });
    }
}
