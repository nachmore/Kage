/**
 * Agent Tools Settings Module
 * Shows tools from the ACP agent and lets users toggle trust per tool.
 */

class ToolPermissionsSettingsModule extends SettingsModule {
    constructor() {
        super('tool-permissions', 'Agent Tools');
        this.trustAll = false;
        this.allowedTools = [];
        this.agentTools = []; // parsed from /tools output
        this.loading = false;
    }

    render() {
        return `
            <div class="settings-section-header">Agent Tools</div>
            
            <div class="setting-row">
                <div class="setting-label-container">
                    <div class="setting-label">Trust All Tools</div>
                    <div class="setting-description">
                        Automatically approve all tool requests without prompting.
                        <span style="color: #f44336;">Warning:</span> This allows the AI to use any tool without your explicit permission.
                    </div>
                </div>
                <div class="setting-control">
                    <label class="toggle-switch">
                        <input type="checkbox" id="trustAllTools">
                        <span class="toggle-slider"></span>
                    </label>
                </div>
            </div>

            <div class="setting-row" style="flex-direction: column; align-items: stretch;">
                <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px;">
                    <div class="setting-label-container" style="padding-right: 0;">
                        <div class="setting-label">Available Tools</div>
                        <div class="setting-description">
                            Tools available to the AI agent. Toggle trust to allow tools without per-request prompting.
                        </div>
                    </div>
                    <button class="setting-button" id="refreshToolsBtn" onclick="refreshAgentTools()">Refresh</button>
                </div>
                <div class="agent-tools-list" id="agentToolsList">
                    <div class="tools-loading">Connect to the agent and click Refresh to load tools.</div>
                </div>
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
    }

    load(config) {
        this.trustAll = config.tool_permissions?.trust_all || false;
        this.allowedTools = config.tool_permissions?.allowed_tools || [];
        
        const trustAllCheckbox = document.getElementById('trustAllTools');
        if (trustAllCheckbox) {
            trustAllCheckbox.checked = this.trustAll;
        }
    }

    save(config) {
        config.tool_permissions = {
            trust_all: this.trustAll,
            allowed_tools: this.allowedTools
        };
    }

    validate() {
        return { valid: true };
    }

    async fetchTools() {
        this.loading = true;
        this.renderToolsList();
        
        try {
            const response = await window.__TAURI__.tauri.invoke('fetch_agent_tools');
            this.agentTools = this.parseToolsResponse(response);
            this.loading = false;
            this.renderToolsList();
        } catch (error) {
            console.error('Failed to fetch tools:', error);
            this.loading = false;
            this.agentTools = [];
            this.renderToolsList(error.toString());
        }
    }

    parseToolsResponse(text) {
        // Parse the /tools output which looks like:
        // Tool Name     Status
        // read           trusted
        // write          per-request
        // shell          per-request
        // etc.
        const tools = [];
        const lines = text.split('\n');
        
        for (const line of lines) {
            const trimmed = line.trim();
            if (!trimmed || trimmed.startsWith('Tool') || trimmed.startsWith('---') || trimmed.startsWith('Available')) {
                continue;
            }
            
            // Try to match "toolname    status" or "toolname (source)    status"
            // Common patterns: "read           trusted", "write          per-request"
            // Also handle: "✓ read    trusted" or "• read    per-request"
            const cleaned = trimmed.replace(/^[✓•✗\-\s]+/, '');
            
            // Split on multiple spaces or tabs
            const parts = cleaned.split(/\s{2,}|\t+/);
            if (parts.length >= 2) {
                const name = parts[0].trim();
                const status = parts[parts.length - 1].trim().toLowerCase();
                
                if (name && (status.includes('trust') || status.includes('per-request') || status.includes('request'))) {
                    tools.push({
                        name: name,
                        trusted: status.includes('trust') && !status.includes('per-request'),
                        status: status
                    });
                }
            } else if (parts.length === 1) {
                // Might be just a tool name, try regex
                const match = cleaned.match(/^(\S+)\s+(.+)$/);
                if (match) {
                    const name = match[1].trim();
                    const status = match[2].trim().toLowerCase();
                    tools.push({
                        name: name,
                        trusted: status.includes('trust') && !status.includes('per-request'),
                        status: status
                    });
                }
            }
        }
        
        return tools;
    }

    renderToolsList(error) {
        const container = document.getElementById('agentToolsList');
        if (!container) return;
        
        if (this.loading) {
            container.innerHTML = `
                <div class="tools-loading">
                    <div class="tools-spinner"></div>
                    Loading tools from agent...
                </div>
            `;
            return;
        }
        
        if (error) {
            container.innerHTML = `
                <div class="tools-loading" style="color: #f44336;">
                    Failed to load tools: ${this.escapeHtml(error)}
                </div>
            `;
            return;
        }
        
        if (this.agentTools.length === 0) {
            container.innerHTML = `
                <div class="tools-loading">
                    No tools found. Make sure the agent is connected and click Refresh.
                </div>
            `;
            return;
        }
        
        const toolsHtml = this.agentTools.map((tool, index) => {
            const kindIcon = this.getToolIcon(tool.name);
            return `
                <div class="agent-tool-item">
                    <div class="agent-tool-icon">${kindIcon}</div>
                    <div class="agent-tool-info">
                        <div class="agent-tool-name">${this.escapeHtml(tool.name)}</div>
                        <div class="agent-tool-status ${tool.trusted ? 'trusted' : 'per-request'}">
                            ${tool.trusted ? '✓ Trusted' : '⚡ Per-request'}
                        </div>
                    </div>
                    <label class="toggle-switch">
                        <input type="checkbox" ${tool.trusted ? 'checked' : ''} 
                               onchange="toggleToolTrust('${this.escapeHtml(tool.name)}', this.checked)">
                        <span class="toggle-slider"></span>
                    </label>
                </div>
            `;
        }).join('');
        
        container.innerHTML = toolsHtml;
    }

    getToolIcon(name) {
        const icons = {
            'read': '📖', 'write': '✏️', 'shell': '💻', 'aws': '☁️',
            'web_search': '🔍', 'web_fetch': '🌐', 'report': '📋',
            'glob': '📁', 'grep': '🔎', 'subagent': '🤖'
        };
        return icons[name.toLowerCase()] || '🔧';
    }

    escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }
}

// Global functions
async function refreshAgentTools() {
    const module = settingsManager.modules.find(m => m.id === 'tool-permissions');
    if (module) {
        await module.fetchTools();
    }
}

async function toggleToolTrust(toolName, trusted) {
    try {
        await window.__TAURI__.tauri.invoke('set_tool_trust', { toolName, trusted });
        // Refresh the list to show updated state
        await refreshAgentTools();
    } catch (error) {
        console.error('Failed to toggle tool trust:', error);
        alert('Failed to update tool trust: ' + error);
        await refreshAgentTools();
    }
}

function removeToolPermission(index) {
    const module = settingsManager.modules.find(m => m.id === 'tool-permissions');
    if (module) {
        module.removeToolAtIndex(index);
    }
}

// Styles for agent tools
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
        padding: 12px 16px;
        border-bottom: 1px solid #2b2b2b;
    }

    .agent-tool-item:last-child {
        border-bottom: none;
    }

    .agent-tool-item:hover {
        background: #252526;
    }

    .agent-tool-icon {
        font-size: 20px;
        width: 32px;
        text-align: center;
        flex-shrink: 0;
    }

    .agent-tool-info {
        flex: 1;
    }

    .agent-tool-name {
        font-size: 14px;
        color: #cccccc;
        font-weight: 500;
        margin-bottom: 2px;
    }

    .agent-tool-status {
        font-size: 12px;
    }

    .agent-tool-status.trusted {
        color: #4caf50;
    }

    .agent-tool-status.per-request {
        color: #ff9800;
    }

    .tools-loading {
        padding: 30px 20px;
        text-align: center;
        color: #888888;
        font-size: 13px;
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 10px;
    }

    .tools-spinner {
        width: 16px;
        height: 16px;
        border: 2px solid #444;
        border-top-color: #007acc;
        border-radius: 50%;
        animation: spin 0.8s linear infinite;
    }

    @keyframes spin {
        to { transform: rotate(360deg); }
    }
`;
document.head.appendChild(toolPermStyle);
