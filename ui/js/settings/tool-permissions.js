import { SettingsModule } from './base.js';
import { getToolEmoji, escapeHtml } from '../shared/tool-utils.js';
import { getSettingsManager, registerSettingsActions } from './module-registry.js';
/**
 * Agent Tools Settings Module
 * Shows tools seen during permission requests with per-tool policy toggles.
 */

export class ToolPermissionsSettingsModule extends SettingsModule {
    constructor() {
        super('tool-permissions', 'Agent Permissions', '🔧');
        this.trustAll = false;
        this.tools = [];
    }

    render() {
        return `
            <div class="settings-section-header">${this.icon} Agent Tools</div>

            <div class="setting-row">
                <div class="setting-label">Seen Tools</div>
                <div class="setting-description">
                    Tools the AI has requested to use. Set each tool's permission policy.
                </div>
                <div class="agent-tools-list" id="agentToolsList" style="margin-top: 8px;">
                    <div class="tools-empty">No tools seen yet. Tools will appear here as the AI requests them.</div>
                </div>
                <button class="reset-permissions-btn" id="resetPermissionsBtn" style="margin-top: 12px;">Reset All Permissions</button>
            </div>

            <div class="setting-section-label" style="margin-top: 32px;">📜 Audit Log</div>
            <div class="setting-row">
                <div class="setting-label">Permission History</div>
                <div class="setting-description">
                    Every permission grant, denial, revoke, expiry, and terminator-mode toggle is recorded here. The log is stored locally in plain JSON and can be tampered with — use it to spot-check recent activity, not for forensic audit.
                </div>
                <div class="audit-log-controls" style="display:flex; gap:8px; margin-top:8px; align-items:center;">
                    <select id="auditLogFilter" class="agent-tool-select" style="min-width:140px;">
                        <option value="all">All events</option>
                        <option value="granted">Granted</option>
                        <option value="denied">Denied</option>
                        <option value="revoked">Revoked</option>
                        <option value="expired">Expired</option>
                        <option value="terminator_mode_changed">Terminator mode</option>
                    </select>
                    <button id="auditLogRefreshBtn" class="reset-permissions-btn" style="background:#3a3640;">Refresh</button>
                    <button id="auditLogClearBtn" class="reset-permissions-btn">Clear log</button>
                    <span id="auditLogPath" style="flex:1; text-align:right; font-size:11px; color:#938f9b; font-family:monospace; overflow:hidden; text-overflow:ellipsis; white-space:nowrap;"></span>
                </div>
                <div class="audit-log-list" id="auditLogList" style="margin-top: 8px; max-height: 320px; overflow-y: auto; border:1px solid #3a3640; border-radius:6px; background:#1a1720;">
                    <div class="tools-empty">Loading…</div>
                </div>
            </div>

            <div class="setting-section-label" style="color: #ef4444; margin-top: 32px;">⚠️ Danger Zone</div>
            <div class="setting-row terminator-row">
                <div class="terminator-header">
                    <div class="terminator-mascot" id="terminatorMascot"></div>
                    <div class="terminator-text">
                        ${this.createCheckboxRow(
                            'Terminator Mode',
                            'Auto-approve ALL tool requests without prompting. The AI can read, write, execute, and delete without your permission.',
                            'terminatorMode',
                            false
                        )}
                    </div>
                </div>
            </div>
        `;
    }

    initialize() {
        // Render terminator mascot with red outline
        const mascotEl = document.getElementById('terminatorMascot');
        if (mascotEl) {
            (async () => {
                const { createMascot } = await import('../shared/mascot.js');
                const svg = await createMascot({
                    src: 'assets/kage-terminator.svg',
                    size: 96,
                    outline: { color: '#ef4444', radius: 1.5 },
                });
                mascotEl.appendChild(svg);
            })();
        }

        const terminatorCheckbox = document.getElementById('terminatorMode');
        if (terminatorCheckbox) {
            terminatorCheckbox.addEventListener('change', async (e) => {
                if (e.target.checked) {
                    const { ask } = window.__TAURI__?.dialog || {};
                    let confirmed = false;
                    if (ask) {
                        confirmed = await ask(
                            'Terminator Mode will auto-approve ALL tool requests.\n\nThe AI can read, write, execute, and delete without asking.\n\nAre you sure?',
                            { title: '⚠️ Enable Terminator Mode', kind: 'warning' }
                        );
                    } else {
                        confirmed = confirm(
                            '⚠️ Terminator Mode will auto-approve ALL tool requests. Are you sure?'
                        );
                    }
                    if (!confirmed) {
                        e.target.checked = false;
                        return;
                    }
                }
                this.terminatorMode = e.target.checked;
            });
        }

        const resetBtn = document.getElementById('resetPermissionsBtn');
        if (resetBtn) {
            resetBtn.addEventListener('click', async () => {
                if (
                    !confirm(
                        'This will remove all tool permissions and reset to "Always Ask" for everything. Continue?'
                    )
                )
                    return;
                this.tools = [];
                this.terminatorMode = false;
                const terminatorCheckbox = document.getElementById('terminatorMode');
                if (terminatorCheckbox) terminatorCheckbox.checked = false;
                this.renderToolsList();
                // Save immediately
                try {
                    const config = await window.__TAURI__.core.invoke('get_config');
                    config.tool_permissions = {
                        trust_all: false,
                        terminator_mode: false,
                        tools: [],
                    };
                    await window.__TAURI__.core.invoke('save_config', { config });
                } catch (e) {
                    console.error('Failed to reset permissions:', e);
                }
            });
        }

        // Audit log: filter, refresh, and clear controls.
        const filterEl = document.getElementById('auditLogFilter');
        if (filterEl) {
            filterEl.addEventListener('change', () => this.renderAuditLog());
        }
        const refreshBtn = document.getElementById('auditLogRefreshBtn');
        if (refreshBtn) {
            refreshBtn.addEventListener('click', () => this.loadAuditLog());
        }
        const clearBtn = document.getElementById('auditLogClearBtn');
        if (clearBtn) {
            clearBtn.addEventListener('click', async () => {
                if (
                    !confirm(
                        'Clear the permission audit log? This cannot be undone. Permissions themselves are NOT affected.'
                    )
                )
                    return;
                try {
                    await window.__TAURI__.core.invoke('clear_permission_audit_log');
                    this.auditEntries = [];
                    this.renderAuditLog();
                } catch (e) {
                    console.error('Failed to clear audit log:', e);
                }
            });
        }
        // Show the on-disk path as a hint.
        (async () => {
            try {
                const p = await window.__TAURI__.core.invoke('get_permission_audit_log_path');
                const pathEl = document.getElementById('auditLogPath');
                if (pathEl && p) {
                    pathEl.textContent = p;
                    pathEl.title = p;
                }
            } catch {}
        })();

        // Initial load.
        this.loadAuditLog();
    }

    load(config) {
        this.terminatorMode = config.tool_permissions?.terminator_mode || false;
        this.tools = config.tool_permissions?.tools || [];

        const terminatorCheckbox = document.getElementById('terminatorMode');
        if (terminatorCheckbox) {
            terminatorCheckbox.checked = this.terminatorMode;
        }

        this.renderToolsList();
    }

    save(config) {
        config.tool_permissions = {
            trust_all: false, // deprecated, kept for compat
            terminator_mode: this.terminatorMode,
            tools: this.tools,
        };
    }

    validate() {
        return { valid: true };
    }

    renderToolsList() {
        const container = document.getElementById('agentToolsList');
        if (!container) return;

        if (this.tools.length === 0) {
            container.innerHTML = `
                <div class="tools-empty">No tools seen yet. Tools will appear here as the AI requests them.</div>
            `;
            return;
        }

        const toolsHtml = this.tools
            .map((tool, index) => {
                const icon = getToolEmoji(tool.title);
                const lastSeen = tool.last_seen
                    ? new Date(tool.last_seen).toLocaleDateString()
                    : '';
                const isExtension = tool.title.startsWith('ext:');
                const badge = isExtension
                    ? '<span class="agent-tool-badge ext-badge">Extension</span>'
                    : '<span class="agent-tool-badge mcp-badge">MCP</span>';
                return `
                <div class="agent-tool-item">
                    <div class="agent-tool-icon">${icon}</div>
                    <div class="agent-tool-info">
                        <div class="agent-tool-name">${escapeHtml(tool.title)} ${badge}</div>
                        <div class="agent-tool-meta">Last seen: ${lastSeen}${tool.grant_type === '24h' && tool.granted_at ? ' · Granted: ' + new Date(tool.granted_at).toLocaleString() : ''}</div>
                    </div>
                    <select class="agent-tool-select" data-index="${index}" data-action-change="toolPermissions.updatePolicy" data-arg="${index}">
                        <option value="ask" ${tool.policy === 'ask' ? 'selected' : ''}>Always Ask</option>
                        <option value="allow" ${tool.policy === 'allow' && tool.grant_type === '24h' ? 'selected' : ''} data-grant="24h">Allow 24h</option>
                        <option value="allow" ${tool.policy === 'allow' && tool.grant_type === 'always' ? 'selected' : ''} data-grant="always">Always Allow</option>
                        <option value="deny" ${tool.policy === 'deny' ? 'selected' : ''}>Deny</option>
                    </select>
                    <button class="agent-tool-remove" data-action="toolPermissions.removeTool" data-arg="${index}" title="Remove">✕</button>
                </div>
            `;
            })
            .join('');

        container.innerHTML = toolsHtml;
    }

    async updatePolicy(index, policy) {
        if (index >= 0 && index < this.tools.length) {
            const tool = this.tools[index];
            tool.policy = policy;

            try {
                await window.__TAURI__.core.invoke('update_tool_policy', {
                    toolTitle: tool.title,
                    policy: policy,
                });
            } catch (error) {
                console.error('Failed to update tool policy:', error);
            }
        }
    }

    async removeTool(index) {
        if (index >= 0 && index < this.tools.length) {
            const tool = this.tools[index];
            this.tools.splice(index, 1);
            this.renderToolsList();

            try {
                await window.__TAURI__.core.invoke('remove_tool_permission', {
                    toolTitle: tool.title,
                });
            } catch (error) {
                console.error('Failed to remove tool:', error);
            }
        }
    }

    async loadAuditLog() {
        try {
            const entries = await window.__TAURI__.core.invoke('get_permission_audit_log', {
                limit: 500,
            });
            this.auditEntries = Array.isArray(entries) ? entries : [];
        } catch (e) {
            console.error('Failed to load audit log:', e);
            this.auditEntries = [];
        }
        this.renderAuditLog();
    }

    renderAuditLog() {
        const list = document.getElementById('auditLogList');
        if (!list) return;
        const entries = this.auditEntries || [];
        const filter = document.getElementById('auditLogFilter')?.value || 'all';

        const filtered = filter === 'all' ? entries : entries.filter((e) => e.event === filter);

        if (filtered.length === 0) {
            list.innerHTML = `<div class="tools-empty" style="padding:14px; text-align:center; color:#938f9b;">
                ${entries.length === 0 ? 'No audit entries yet. Events will appear here as the AI requests tools.' : 'No entries match the current filter.'}
            </div>`;
            return;
        }

        list.innerHTML = filtered.map((entry) => this._renderAuditRow(entry)).join('');
    }

    _renderAuditRow(entry) {
        // Pluck the event-kind field (serde adds it as `event`) and
        // format timestamp/summary defensively — a malformed entry
        // should render as something debuggable, not crash the UI.
        const when = entry.at
            ? new Date(entry.at).toLocaleString(undefined, {
                  year: 'numeric',
                  month: 'short',
                  day: 'numeric',
                  hour: '2-digit',
                  minute: '2-digit',
                  second: '2-digit',
              })
            : '(unknown time)';
        const kind = entry.event || 'unknown';

        let icon = '📝';
        let summary = '';
        let meta = '';

        switch (kind) {
            case 'granted':
                icon = '✅';
                summary = `Granted <strong>${escapeHtml(entry.tool || '?')}</strong> (${escapeHtml(entry.grant_type || '?')})`;
                if (entry.session_id) meta = `session ${escapeHtml(entry.session_id.slice(0, 8))}`;
                if (entry.args_preview) {
                    meta +=
                        (meta ? ' · ' : '') +
                        `<code>${escapeHtml(entry.args_preview.slice(0, 140))}</code>`;
                }
                break;
            case 'denied':
                icon = '⛔';
                summary = `Denied <strong>${escapeHtml(entry.tool || '?')}</strong>`;
                if (entry.session_id) meta = `session ${escapeHtml(entry.session_id.slice(0, 8))}`;
                break;
            case 'revoked':
                icon = '🗑️';
                summary = `Revoked <strong>${escapeHtml(entry.tool || '?')}</strong>`;
                if (entry.prior_policy) meta = `was ${escapeHtml(entry.prior_policy)}`;
                if (entry.prior_grant_type)
                    meta += (meta ? ' ' : '') + `(${escapeHtml(entry.prior_grant_type)})`;
                break;
            case 'expired':
                icon = '⏳';
                summary = `Expired <strong>${escapeHtml(entry.tool || '?')}</strong>`;
                if (entry.prior_grant_type) meta = `was ${escapeHtml(entry.prior_grant_type)}`;
                break;
            case 'terminator_mode_changed':
                icon = entry.enabled ? '⚠️' : '🛡️';
                summary = entry.enabled
                    ? '<strong>Terminator mode enabled</strong>'
                    : 'Terminator mode disabled';
                break;
            default:
                icon = '❔';
                summary = `Unknown event: ${escapeHtml(kind)}`;
                try {
                    meta = escapeHtml(JSON.stringify(entry));
                } catch {}
        }

        return `
            <div class="audit-log-row">
                <div class="audit-log-icon">${icon}</div>
                <div class="audit-log-body">
                    <div class="audit-log-summary">${summary}</div>
                    ${meta ? `<div class="audit-log-meta">${meta}</div>` : ''}
                </div>
                <div class="audit-log-time" title="${escapeHtml(entry.at || '')}">${escapeHtml(when)}</div>
            </div>
        `;
    }
}

// Global functions for onclick handlers
function updateToolPolicy(index, policy) {
    const settingsManager = getSettingsManager();
    const module = settingsManager?.modules.find((m) => m.id === 'tool-permissions');
    if (module) module.updatePolicy(index, policy);
}

function removeSeenTool(index) {
    const settingsManager = getSettingsManager();
    const module = settingsManager?.modules.find((m) => m.id === 'tool-permissions');
    if (module) module.removeTool(index);
}

// Register the tool-permissions handlers with the delegated dispatcher.
// The select uses data-action-change with the row index in data-arg, so
// the handler receives the index as a string and reads `this.value` from
// the element argument.
registerSettingsActions({
    'toolPermissions.updatePolicy': (arg, el) => {
        updateToolPolicy(parseInt(arg, 10), el.value);
    },
    'toolPermissions.removeTool': (arg) => {
        removeSeenTool(parseInt(arg, 10));
    },
});

// Styles
const toolPermStyle = document.createElement('style');
toolPermStyle.textContent = `
    .agent-tools-list {
        border: 1px solid #2b2b2b;
        border-radius: 4px;
        background: #1e1e1e;
        max-height: 500px;
        overflow-y: auto;
    }

    .agent-tool-item {
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 10px 16px;
        border-bottom: 1px solid #2b2b2b;
    }

    .agent-tool-item:last-child {
        border-bottom: none;
    }

    .agent-tool-item:hover {
        background: #252526;
    }

    .agent-tool-icon {
        font-size: 18px;
        width: 28px;
        text-align: center;
        flex-shrink: 0;
    }

    .agent-tool-info {
        flex: 1;
        min-width: 0;
    }

    .agent-tool-name {
        font-size: 13px;
        color: #cccccc;
        font-weight: 500;
    }

    .agent-tool-badge {
        display: inline-block;
        font-size: 10px;
        font-weight: 600;
        padding: 1px 6px;
        border-radius: 3px;
        margin-left: 6px;
        vertical-align: middle;
    }
    .ext-badge {
        background: rgba(156, 39, 176, 0.2);
        color: #ce93d8;
        border: 1px solid rgba(156, 39, 176, 0.3);
    }
    .mcp-badge {
        background: rgba(33, 150, 243, 0.15);
        color: #64b5f6;
        border: 1px solid rgba(33, 150, 243, 0.25);
    }

    .agent-tool-meta {
        font-size: 11px;
        color: #666666;
        margin-top: 2px;
    }

    .agent-tool-select {
        padding: 4px 8px;
        background: #3c3c3c;
        border: 1px solid #3c3c3c;
        border-radius: 3px;
        color: #cccccc;
        font-size: 12px;
        cursor: pointer;
        flex-shrink: 0;
    }

    .agent-tool-select:focus {
        outline: none;
        border-color: var(--kage-accent);
    }

    .agent-tool-remove {
        background: transparent;
        border: none;
        color: #666;
        cursor: pointer;
        font-size: 14px;
        padding: 4px 6px;
        border-radius: 3px;
        flex-shrink: 0;
    }

    .agent-tool-remove:hover {
        background: rgba(244, 67, 54, 0.15);
        color: #f44336;
    }

    .tools-empty {
        padding: 30px 20px;
        text-align: center;
        color: #666666;
        font-size: 13px;
    }

    .reset-permissions-btn {
        padding: 8px 16px;
        background: rgba(244, 67, 54, 0.12);
        border: 1px solid rgba(244, 67, 54, 0.3);
        border-radius: 6px;
        color: #f44336;
        font-size: 13px;
        font-weight: 500;
        cursor: pointer;
        transition: all 0.15s;
    }

    .reset-permissions-btn:hover {
        background: rgba(244, 67, 54, 0.2);
        border-color: rgba(244, 67, 54, 0.5);
    }

    .terminator-row {
        border: 1px solid rgba(239, 68, 68, 0.3);
        border-radius: 8px;
        padding: 4px;
        background: rgba(239, 68, 68, 0.05);
    }
    .terminator-row .setting-label {
        color: #ef4444;
    }
    .terminator-header {
        display: flex;
        align-items: center;
        gap: 16px;
    }
    .terminator-mascot {
        flex-shrink: 0;
        width: 96px;
        height: 96px;
    }
    .terminator-text {
        flex: 1;
    }

    .audit-log-row {
        display: grid;
        grid-template-columns: 24px 1fr auto;
        gap: 10px;
        padding: 8px 12px;
        border-bottom: 1px solid #2b2b2b;
        align-items: center;
    }
    .audit-log-row:last-child {
        border-bottom: none;
    }
    .audit-log-icon {
        font-size: 16px;
        text-align: center;
    }
    .audit-log-summary {
        font-size: 13px;
        color: var(--kage-text-primary, #e5e7eb);
    }
    .audit-log-meta {
        font-size: 11px;
        color: var(--kage-text-secondary, #938f9b);
        margin-top: 2px;
    }
    .audit-log-meta code {
        background: #2a252f;
        padding: 1px 4px;
        border-radius: 3px;
        font-size: 11px;
    }
    .audit-log-time {
        font-size: 11px;
        color: var(--kage-text-secondary, #938f9b);
        white-space: nowrap;
        font-variant-numeric: tabular-nums;
    }
`;
document.head.appendChild(toolPermStyle);
