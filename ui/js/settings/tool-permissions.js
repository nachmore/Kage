/**
 * Agent Tools Settings Module
 * Shows tools seen during permission requests with per-tool policy toggles.
 */

class ToolPermissionsSettingsModule extends SettingsModule {
    constructor() {
        super('tool-permissions', 'Agent Permissions', '🔧');
        this.trustAll = false;
        this.tools = [];
    }

    render() {
        return `
            <div class="settings-section-header">${this.icon} Agent Tools</div>
            
            ${this.createCheckboxRow(
                'Trust All Tools',
                'Automatically approve all tool requests without prompting. <span style="color: #f44336;">Warning:</span> This allows the AI to use any tool without your explicit permission.',
                'trustAllTools',
                false
            )}

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
        `;
    }

    initialize() {
        const trustAllCheckbox = document.getElementById('trustAllTools');
        if (trustAllCheckbox) {
            trustAllCheckbox.addEventListener('change', (e) => {
                this.trustAll = e.target.checked;
                if (e.target.checked) {
                    const confirmed = confirm(
                        'Warning: Enabling "Trust All Tools" will automatically approve all tool requests.\n\n' +
                        'The AI can use any tool without your permission.\n\nAre you sure?'
                    );
                    if (!confirmed) {
                        e.target.checked = false;
                        this.trustAll = false;
                    }
                }
            });
        }

        const resetBtn = document.getElementById('resetPermissionsBtn');
        if (resetBtn) {
            resetBtn.addEventListener('click', async () => {
                if (!confirm('This will remove all tool permissions and reset to "Always Ask" for everything. Continue?')) return;
                this.tools = [];
                this.trustAll = false;
                const trustAllCheckbox = document.getElementById('trustAllTools');
                if (trustAllCheckbox) trustAllCheckbox.checked = false;
                this.renderToolsList();
                // Save immediately
                try {
                    const config = await window.__TAURI__.core.invoke('get_config');
                    config.tool_permissions = { trust_all: false, tools: [] };
                    await window.__TAURI__.core.invoke('save_config', { config });
                } catch (e) {
                    console.error('Failed to reset permissions:', e);
                }
            });
        }
    }

    load(config) {
        this.trustAll = config.tool_permissions?.trust_all || false;
        this.tools = config.tool_permissions?.tools || [];
        
        const trustAllCheckbox = document.getElementById('trustAllTools');
        if (trustAllCheckbox) {
            trustAllCheckbox.checked = this.trustAll;
        }
        
        this.renderToolsList();
    }

    save(config) {
        config.tool_permissions = {
            trust_all: this.trustAll,
            tools: this.tools
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
        
        const toolsHtml = this.tools.map((tool, index) => {
            const icon = getToolEmoji(tool.title);
            const lastSeen = tool.last_seen ? new Date(tool.last_seen).toLocaleDateString() : '';
            const isExtension = tool.title.startsWith('ext:');
            const badge = isExtension ? '<span class="agent-tool-badge ext-badge">Extension</span>' : '<span class="agent-tool-badge mcp-badge">MCP</span>';
            return `
                <div class="agent-tool-item">
                    <div class="agent-tool-icon">${icon}</div>
                    <div class="agent-tool-info">
                        <div class="agent-tool-name">${escapeHtml(tool.title)} ${badge}</div>
                        <div class="agent-tool-meta">Last seen: ${lastSeen}</div>
                    </div>
                    <select class="agent-tool-select" data-index="${index}" onchange="updateToolPolicy(${index}, this.value)">
                        <option value="ask" ${tool.policy === 'ask' ? 'selected' : ''}>Always Ask</option>
                        <option value="allow" ${tool.policy === 'allow' ? 'selected' : ''}>Allow</option>
                        <option value="deny" ${tool.policy === 'deny' ? 'selected' : ''}>Deny</option>
                    </select>
                    <button class="agent-tool-remove" onclick="removeSeenTool(${index})" title="Remove">✕</button>
                </div>
            `;
        }).join('');
        
        container.innerHTML = toolsHtml;
    }

    async updatePolicy(index, policy) {
        if (index >= 0 && index < this.tools.length) {
            const tool = this.tools[index];
            tool.policy = policy;
            
            try {
                await window.__TAURI__.core.invoke('update_tool_policy', {
                    toolTitle: tool.title,
                    policy: policy
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
                    toolTitle: tool.title
                });
            } catch (error) {
                console.error('Failed to remove tool:', error);
            }
        }
    }
}

// Global functions for onclick handlers
function updateToolPolicy(index, policy) {
    const module = settingsManager.modules.find(m => m.id === 'tool-permissions');
    if (module) module.updatePolicy(index, policy);
}

function removeSeenTool(index) {
    const module = settingsManager.modules.find(m => m.id === 'tool-permissions');
    if (module) module.removeTool(index);
}

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
        border-color: var(--kiro-accent);
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
`;
document.head.appendChild(toolPermStyle);
