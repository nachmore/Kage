/**
 * Tool Permissions Settings Module
 * Manages tool permission preferences and history
 */

class ToolPermissionsSettingsModule extends SettingsModule {
    constructor() {
        super('tool-permissions', 'Tool Permissions');
        this.trustAll = false;
        this.allowedTools = [];
    }

    render() {
        return `
            <div class="settings-section-header">Tool Permissions</div>
            
            <div class="setting-row">
                <div class="setting-label-container">
                    <div class="setting-label">Trust All Tools</div>
                    <div class="setting-description">
                        Automatically approve all tool requests without prompting. 
                        <strong style="color: #f44336;">Warning:</strong> This allows the AI to use any tool without your explicit permission.
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
                <div class="setting-label-container" style="padding-right: 0; margin-bottom: 12px;">
                    <div class="setting-label">Allowed Tools</div>
                    <div class="setting-description">
                        Tools that you've granted "Trust Always" permission. You can revoke access at any time.
                    </div>
                </div>
                <div class="allowed-tools-list" id="allowedToolsList">
                    <!-- Tools will be rendered here -->
                </div>
            </div>
        `;
    }

    initialize() {
        // Set up trust all toggle
        const trustAllCheckbox = document.getElementById('trustAllTools');
        if (trustAllCheckbox) {
            trustAllCheckbox.addEventListener('change', (e) => {
                this.trustAll = e.target.checked;
                
                // Show warning if enabling
                if (e.target.checked) {
                    const confirmed = confirm(
                        'Warning: Enabling "Trust All Tools" will automatically approve all tool requests without prompting you.\n\n' +
                        'This means the AI can use any tool (like web search, file operations, etc.) without your explicit permission.\n\n' +
                        'Are you sure you want to enable this?'
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
        
        // Update UI
        const trustAllCheckbox = document.getElementById('trustAllTools');
        if (trustAllCheckbox) {
            trustAllCheckbox.checked = this.trustAll;
        }
        
        this.renderAllowedTools();
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

    renderAllowedTools() {
        const container = document.getElementById('allowedToolsList');
        if (!container) return;
        
        if (this.allowedTools.length === 0) {
            container.innerHTML = `
                <div class="empty-state">
                    <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                        <path d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z"/>
                    </svg>
                    <p>No tools have been granted "Trust Always" permission yet.</p>
                    <p class="empty-state-hint">When you approve a tool with "Trust Always", it will appear here.</p>
                </div>
            `;
            return;
        }
        
        const toolsHtml = this.allowedTools.map((tool, index) => {
            const date = new Date(tool.allowed_at);
            const formattedDate = date.toLocaleDateString() + ' ' + date.toLocaleTimeString();
            
            return `
                <div class="tool-item" data-index="${index}">
                    <div class="tool-info">
                        <div class="tool-name">${this.escapeHtml(tool.title)}</div>
                        <div class="tool-date">Allowed on ${formattedDate}</div>
                    </div>
                    <button class="tool-remove-btn" onclick="removeToolPermission(${index})">
                        <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
                            <path d="M5.5 5.5A.5.5 0 016 6v6a.5.5 0 01-1 0V6a.5.5 0 01.5-.5zm2.5 0a.5.5 0 01.5.5v6a.5.5 0 01-1 0V6a.5.5 0 01.5-.5zm3 .5a.5.5 0 00-1 0v6a.5.5 0 001 0V6z"/>
                            <path fill-rule="evenodd" d="M14.5 3a1 1 0 01-1 1H13v9a2 2 0 01-2 2H5a2 2 0 01-2-2V4h-.5a1 1 0 01-1-1V2a1 1 0 011-1H6a1 1 0 011-1h2a1 1 0 011 1h3.5a1 1 0 011 1v1zM4.118 4L4 4.059V13a1 1 0 001 1h6a1 1 0 001-1V4.059L11.882 4H4.118zM2.5 3V2h11v1h-11z"/>
                        </svg>
                        Revoke
                    </button>
                </div>
            `;
        }).join('');
        
        container.innerHTML = toolsHtml;
    }

    escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }

    removeToolAtIndex(index) {
        if (index >= 0 && index < this.allowedTools.length) {
            const tool = this.allowedTools[index];
            
            if (confirm(`Are you sure you want to revoke permission for "${tool.title}"?\n\nYou will be prompted again next time the AI wants to use this tool.`)) {
                this.allowedTools.splice(index, 1);
                this.renderAllowedTools();
                
                // Also remove from backend
                if (window.__TAURI__) {
                    window.__TAURI__.tauri.invoke('remove_tool_permission', {
                        toolTitle: tool.title
                    }).catch(error => {
                        console.error('Failed to remove tool permission:', error);
                    });
                }
            }
        }
    }
}

// Global function for removing tools (called from onclick)
function removeToolPermission(index) {
    const module = settingsManager.modules.find(m => m.id === 'tool-permissions');
    if (module) {
        module.removeToolAtIndex(index);
    }
}

// Add styles for tool permissions
const style = document.createElement('style');
style.textContent = `
    .allowed-tools-list {
        border: 1px solid #2b2b2b;
        border-radius: 4px;
        background: #1e1e1e;
        max-height: 400px;
        overflow-y: auto;
    }

    .tool-item {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 12px 16px;
        border-bottom: 1px solid #2b2b2b;
    }

    .tool-item:last-child {
        border-bottom: none;
    }

    .tool-item:hover {
        background: #252526;
    }

    .tool-info {
        flex: 1;
    }

    .tool-name {
        font-size: 13px;
        color: #cccccc;
        font-weight: 500;
        margin-bottom: 4px;
    }

    .tool-date {
        font-size: 12px;
        color: #888888;
    }

    .tool-remove-btn {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 6px 12px;
        background: transparent;
        border: 1px solid #f44336;
        border-radius: 3px;
        color: #f44336;
        font-size: 12px;
        cursor: pointer;
        transition: all 0.2s;
    }

    .tool-remove-btn:hover {
        background: rgba(244, 67, 54, 0.1);
    }

    .tool-remove-btn svg {
        flex-shrink: 0;
    }

    .empty-state {
        padding: 40px 20px;
        text-align: center;
        color: #888888;
    }

    .empty-state svg {
        margin: 0 auto 16px;
        opacity: 0.5;
    }

    .empty-state p {
        margin: 8px 0;
        font-size: 13px;
    }

    .empty-state-hint {
        font-size: 12px;
        color: #666666;
    }
`;
document.head.appendChild(style);
