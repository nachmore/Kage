import { SettingsModule } from './base.js';
import { getToolEmoji, escapeHtml, LTR_SLOT, setTextWithLtrValue } from '../shared/tool-utils.js';
import { errMessage } from '../shared/error-message.js';
import { t } from '../shared/i18n.js';
import { getSettingsManager, registerSettingsActions } from './module-registry.js';
import { getConfigSnapshot, saveConfig } from '../shared/config-cache.js';
import {
    agentOutcomeLabel,
    cleanText,
    extensionDisplayName,
    formatCount,
} from './extension-agent-access.js';
/**
 * Agent Tools Settings Module
 * Shows tools seen during permission requests with per-tool policy toggles.
 */

export class ToolPermissionsSettingsModule extends SettingsModule {
    constructor() {
        super('tool-permissions', t('settings.sidebar.tool_permissions'), '🔧');
        this.trustAll = false;
        this.trustCommands = false;
        /** MCP servers whose every tool is trusted (backend-owned). */
        this.trustedServers = [];
        /** Rules for commands the agent runs outside its sandbox (backend-owned). */
        this.hostRules = [];
        this.hostAllowAll = false;
        this.tools = [];
    }

    render() {
        return `
            <div class="settings-section-header">${this.icon} ${t('settings.tool_permissions.title')}</div>

            <div class="setting-row" data-sync-path="tool_permissions.tools">
                <div class="setting-label">${t('settings.tool_permissions.seen.label')}</div>
                <div class="setting-description">
                    ${t('settings.tool_permissions.seen.description')}
                </div>
                <div class="agent-tools-list" id="agentToolsList" style="margin-top: 8px;">
                    <div class="tools-empty">${t('settings.tool_permissions.empty')}</div>
                </div>
                <button class="reset-permissions-btn" id="resetPermissionsBtn" style="margin-top: 12px;">${t('settings.tool_permissions.reset_btn')}</button>
            </div>

            <div class="setting-section-label" style="margin-top: 32px;">${t('settings.tool_permissions.host.section')}</div>
            <div class="setting-row">
                <div class="setting-label">${t('settings.tool_permissions.host.rules_label')}</div>
                <div class="setting-description">
                    ${t('settings.tool_permissions.host.description')}
                </div>
                <div class="agent-tools-list host-rules-list" id="hostCommandRulesList" style="margin-top: 8px;">
                    <div class="tools-empty">${t('settings.tool_permissions.host.rules_empty')}</div>
                </div>
                <div class="trusted-servers-status" id="hostCommandRulesStatus" role="status"></div>
            </div>
            ${this.createCheckboxRow(
                t('settings.tool_permissions.host.allow_all.label'),
                t('settings.tool_permissions.host.allow_all.description'),
                'hostCommandsAllowAll',
                false,
                { syncPath: 'tool_permissions.host_commands_allow_all' }
            )}
            <div class="setting-row">
                <button type="button" class="host-audit-link" id="hostCommandAuditLink">${t('settings.tool_permissions.host.audit_link')}</button>
            </div>

            <div class="setting-section-label" style="margin-top: 32px;">${t('settings.tool_permissions.audit.section')}</div>
            <div class="setting-row">
                <div class="setting-label">${t('settings.tool_permissions.audit.label')}</div>
                <div class="setting-description">
                    ${t('settings.tool_permissions.audit.description')}
                </div>
                <div class="audit-log-controls" style="display:flex; gap:8px; margin-top:8px; align-items:center;">
                    <select id="auditLogFilter" class="agent-tool-select" style="min-width:140px;">
                        <option value="all">${t('settings.tool_permissions.audit.filter.all')}</option>
                        <option value="granted">${t('settings.tool_permissions.audit.filter.granted')}</option>
                        <option value="denied">${t('settings.tool_permissions.audit.filter.denied')}</option>
                        <option value="revoked">${t('settings.tool_permissions.audit.filter.revoked')}</option>
                        <option value="expired">${t('settings.tool_permissions.audit.filter.expired')}</option>
                        <option value="terminator_mode_changed">${t('settings.tool_permissions.audit.filter.terminator')}</option>
                        <option value="user_ran_command">${t('settings.tool_permissions.audit.filter.user_ran')}</option>
                        <option value="host_command">${t('settings.tool_permissions.audit.filter.host_command')}</option>
                        <option value="background_ai">${t('settings.tool_permissions.audit.filter.background_ai')}</option>
                    </select>
                    <button id="auditLogRefreshBtn" class="reset-permissions-btn" style="background:#3a3640;">${t('settings.tool_permissions.audit.refresh')}</button>
                    <button id="auditLogClearBtn" class="reset-permissions-btn">${t('settings.tool_permissions.audit.clear')}</button>
                    <span id="auditLogPath" style="flex:1; text-align:right; font-size:11px; color:#938f9b; font-family:monospace; overflow:hidden; text-overflow:ellipsis; white-space:nowrap;"></span>
                </div>
                <div class="audit-log-list" id="auditLogList" style="margin-top: 8px; max-height: 320px; overflow-y: auto; border:1px solid #3a3640; border-radius:6px; background:#1a1720;">
                    <div class="tools-empty">${t('settings.tool_permissions.audit.loading')}</div>
                </div>
            </div>

            <div class="setting-section-label" style="color: var(--kage-error); margin-top: 32px;">${t('settings.tool_permissions.danger.section')}</div>
            ${this.createCheckboxRow(
                t('settings.tool_permissions.trust_commands.label'),
                t('settings.tool_permissions.trust_commands.description'),
                'trustAllCommands',
                false,
                { syncPath: 'tool_permissions.trust_all' }
            )}
            <div class="setting-row">
                <div class="setting-label">${t('settings.tool_permissions.trusted_servers.label')}</div>
                <div class="setting-description">
                    ${t('settings.tool_permissions.trusted_servers.description')}
                </div>
                <div class="agent-tools-list trusted-servers-list" id="trustedMcpServersList" style="margin-top: 8px;">
                    <div class="tools-empty">${t('settings.tool_permissions.trusted_servers.empty')}</div>
                </div>
                <div class="trusted-servers-status" id="trustedMcpServersStatus" role="status"></div>
            </div>
            <div class="setting-row terminator-row">
                <div class="terminator-header">
                    <div class="terminator-mascot" id="terminatorMascot"></div>
                    <div class="terminator-text">
                        ${this.createCheckboxRow(
                            t('settings.tool_permissions.terminator.label'),
                            t('settings.tool_permissions.terminator.description'),
                            'terminatorMode',
                            false,
                            { syncPath: 'tool_permissions.terminator_mode' }
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
                            t('settings.tool_permissions.terminator.confirm_full'),
                            {
                                title: t('settings.tool_permissions.terminator.dialog_title'),
                                kind: 'warning',
                            }
                        );
                    } else {
                        confirmed = confirm(
                            t('settings.tool_permissions.terminator.confirm_short')
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

        // Trust all CLI commands (ACP kind "execute"). Applied immediately
        // through the backend, which also audits it — not via save().
        const trustCommandsCheckbox = document.getElementById('trustAllCommands');
        if (trustCommandsCheckbox) {
            trustCommandsCheckbox.addEventListener('change', async (e) => {
                const enable = e.target.checked;
                if (enable) {
                    const { ask } = window.__TAURI__?.dialog || {};
                    const confirmed = ask
                        ? await ask(t('settings.tool_permissions.trust_commands.confirm'), {
                              title: t('settings.tool_permissions.trust_commands.dialog_title'),
                              kind: 'warning',
                          })
                        : confirm(t('settings.tool_permissions.trust_commands.confirm'));
                    if (!confirmed) {
                        e.target.checked = false;
                        return;
                    }
                }
                try {
                    await window.__TAURI__.core.invoke('set_tool_kind_trusted', {
                        kind: 'execute',
                        trusted: enable,
                    });
                    this.trustCommands = enable;
                } catch (err) {
                    console.error('Failed to update command trust:', err);
                    e.target.checked = this.trustCommands;
                }
            });
        }

        // Commands outside the agent's sandbox: allow-all is applied at once
        // through its audited command, after a plain warning.
        const allowAllBox = document.getElementById('hostCommandsAllowAll');
        if (allowAllBox) {
            allowAllBox.addEventListener('change', async (e) => {
                const enable = e.target.checked;
                if (enable) {
                    const { ask } = window.__TAURI__?.dialog || {};
                    const message = t('settings.tool_permissions.host.allow_all.confirm');
                    const confirmed = ask
                        ? await ask(message, {
                              title: t('settings.tool_permissions.host.allow_all.dialog_title'),
                              kind: 'warning',
                          })
                        : confirm(message);
                    if (!confirmed) {
                        e.target.checked = false;
                        return;
                    }
                }
                try {
                    await window.__TAURI__.core.invoke('set_host_commands_allow_all', {
                        enabled: enable,
                    });
                    this.hostAllowAll = enable;
                } catch (err) {
                    console.error('Failed to update allow-all for host commands:', err);
                    e.target.checked = this.hostAllowAll;
                }
            });
        }
        const auditLink = document.getElementById('hostCommandAuditLink');
        if (auditLink) {
            auditLink.addEventListener('click', () => this.showHostCommandAudit());
        }

        const resetBtn = document.getElementById('resetPermissionsBtn');
        if (resetBtn) {
            resetBtn.addEventListener('click', async () => {
                if (!confirm(t('settings.tool_permissions.reset_confirm'))) return;
                this.tools = [];
                this.terminatorMode = false;
                this.trustCommands = false;
                const terminatorCheckbox = document.getElementById('terminatorMode');
                if (terminatorCheckbox) terminatorCheckbox.checked = false;
                const trustBox = document.getElementById('trustAllCommands');
                if (trustBox) trustBox.checked = false;
                this.renderToolsList();
                // Save immediately. Command and MCP server trust are
                // backend-owned (save_config ignores them), so revoke them
                // through their own audited commands, independently of the
                // rest of the reset.
                const { invoke } = window.__TAURI__.core;
                await invoke('set_tool_kind_trusted', { kind: 'execute', trusted: false }).catch(
                    (e) => console.error('Failed to revoke command trust:', e)
                );
                await this._revokeAllServers(invoke);
                await this._clearHostCommandRules(invoke);
                try {
                    const { revision, config } = await getConfigSnapshot(invoke);
                    config.tool_permissions = {
                        trust_all: false,
                        terminator_mode: false,
                        trusted_kinds: config.tool_permissions?.trusted_kinds ?? [],
                        trusted_mcp_servers: config.tool_permissions?.trusted_mcp_servers ?? [],
                        host_command_rules: config.tool_permissions?.host_command_rules ?? [],
                        host_commands_allow_all:
                            config.tool_permissions?.host_commands_allow_all ?? false,
                        tools: [],
                    };
                    await saveConfig(config, { baseRevision: revision, invoke });
                    this._syncTrustCommands(config);
                    this._syncTrustedServers(config);
                    this._syncHostCommands(config);
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
                if (!confirm(t('settings.tool_permissions.audit.clear_confirm'))) return;
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
        this._syncTrustCommands(config);
        this._syncTrustedServers(config);
        this._syncHostCommands(config);

        const terminatorCheckbox = document.getElementById('terminatorMode');
        if (terminatorCheckbox) {
            terminatorCheckbox.checked = this.terminatorMode;
        }

        this.renderToolsList();
    }

    /** Called by the manager each time the tab is re-shown. */
    async onShow() {
        // Tool edits persist immediately, so the list holds no unsaved state;
        // refresh it so tools seen / grants consumed since open are shown.
        try {
            const config = await window.__TAURI__.core.invoke('get_config');
            this.tools = config.tool_permissions?.tools || [];
            this.renderToolsList();
            // Command and MCP server trust are backend-owned and can change
            // from a prompt in another window ("Trust all commands…",
            // "Trust all tools from …") — always reflect them.
            this._syncTrustCommands(config);
            this._syncTrustedServers(config);
            // Rules can be added from a prompt in another window.
            this._syncHostCommands(config);
        } catch (e) {
            console.warn('Failed to refresh tool permissions:', e);
        }
    }

    _syncTrustCommands(config) {
        this.trustCommands = (config?.tool_permissions?.trusted_kinds || []).includes('execute');
        const box = document.getElementById('trustAllCommands');
        if (box) box.checked = this.trustCommands;
    }

    _syncTrustedServers(config) {
        const servers = config?.tool_permissions?.trusted_mcp_servers;
        this.trustedServers = Array.isArray(servers)
            ? servers.filter((s) => typeof s === 'string')
            : [];
        this.renderTrustedServers();
    }

    _syncHostCommands(config) {
        const rules = config?.tool_permissions?.host_command_rules;
        this.hostRules = Array.isArray(rules)
            ? rules.filter((r) => r && typeof r.prefix === 'string')
            : [];
        this.hostAllowAll = !!config?.tool_permissions?.host_commands_allow_all;
        const box = document.getElementById('hostCommandsAllowAll');
        if (box) box.checked = this.hostAllowAll;
        this.renderHostRules();
    }

    /** Rules for commands outside the agent's sandbox, each with Remove. */
    renderHostRules() {
        const list = document.getElementById('hostCommandRulesList');
        if (!list) return;
        list.textContent = '';
        if (this.hostRules.length === 0) {
            const empty = document.createElement('div');
            empty.className = 'tools-empty';
            empty.textContent = t('settings.tool_permissions.host.rules_empty');
            list.append(empty);
            return;
        }
        // Built with the DOM API: a prefix is words of an agent's command.
        for (const rule of this.hostRules) {
            const row = document.createElement('div');
            row.className = 'agent-tool-item host-rule-item';
            const info = document.createElement('div');
            info.className = 'agent-tool-info';
            const name = document.createElement('div');
            name.className = 'agent-tool-name host-rule-name';
            setTextWithLtrValue(
                name,
                t('settings.tool_permissions.host.rule', { prefix: LTR_SLOT }),
                rule.prefix
            );
            info.append(name);
            if (rule.created_at) {
                const meta = document.createElement('div');
                meta.className = 'agent-tool-meta';
                meta.textContent = t('settings.tool_permissions.host.rule_added_at', {
                    date: new Date(rule.created_at).toLocaleDateString(),
                });
                info.append(meta);
            }
            const remove = document.createElement('button');
            remove.type = 'button';
            remove.className = 'trusted-server-revoke host-rule-remove';
            remove.textContent = t('settings.tool_permissions.host.remove');
            remove.title = t('settings.tool_permissions.host.remove_title', {
                prefix: rule.prefix,
            });
            remove.addEventListener('click', () => this.removeHostRule(rule.prefix, remove));
            row.append(info, remove);
            list.append(row);
        }
    }

    /** Remove one rule, through the audited backend command. */
    async removeHostRule(prefix, button) {
        if (button) button.disabled = true;
        const status = document.getElementById('hostCommandRulesStatus');
        if (status) status.textContent = '';
        try {
            await window.__TAURI__.core.invoke('remove_host_command_rule', { prefix });
            this.hostRules = this.hostRules.filter((r) => r.prefix !== prefix);
        } catch (e) {
            console.error('Failed to remove host command rule:', e);
            if (status) {
                status.textContent = t('settings.tool_permissions.host.remove_failed', {
                    error: errMessage(e),
                });
            }
        }
        this.renderHostRules();
    }

    /** Reset: every rule goes, and allow-all is turned off. */
    async _clearHostCommandRules(invoke) {
        let rules = this.hostRules;
        try {
            const config = await invoke('get_config');
            rules = config?.tool_permissions?.host_command_rules ?? rules;
        } catch (e) {
            console.warn('Failed to read host command rules:', e);
        }
        for (const rule of rules) {
            await invoke('remove_host_command_rule', { prefix: rule.prefix }).catch((e) =>
                console.error('Failed to remove host command rule:', e)
            );
        }
        await invoke('set_host_commands_allow_all', { enabled: false }).catch((e) =>
            console.error('Failed to turn off allow-all for host commands:', e)
        );
    }

    /** The audit log, showing only commands for the agent. */
    showHostCommandAudit() {
        const filter = document.getElementById('auditLogFilter');
        if (filter) filter.value = 'host_command';
        this.renderAuditLog();
        document.getElementById('auditLogList')?.scrollIntoView({ block: 'nearest' });
    }

    _setServersStatus(text) {
        const status = document.getElementById('trustedMcpServersStatus');
        if (status) status.textContent = text || '';
    }

    /** Trusted MCP servers, each with a "Stop trusting" button. */
    renderTrustedServers() {
        const list = document.getElementById('trustedMcpServersList');
        if (!list) return;
        list.textContent = '';
        if (this.trustedServers.length === 0) {
            const empty = document.createElement('div');
            empty.className = 'tools-empty';
            empty.textContent = t('settings.tool_permissions.trusted_servers.empty');
            list.append(empty);
            return;
        }
        // Built with the DOM API: server names come from agent tool titles.
        for (const server of this.trustedServers) {
            const row = document.createElement('div');
            row.className = 'agent-tool-item trusted-server-item';
            const info = document.createElement('div');
            info.className = 'agent-tool-info';
            const name = document.createElement('bdi');
            name.dir = 'ltr';
            name.className = 'agent-tool-name trusted-server-name';
            name.textContent = server;
            info.append(name);
            const revoke = document.createElement('button');
            revoke.type = 'button';
            revoke.className = 'trusted-server-revoke';
            revoke.dataset.server = server;
            revoke.textContent = t('settings.tool_permissions.trusted_servers.revoke');
            revoke.title = t('settings.tool_permissions.trusted_servers.revoke_title', { server });
            revoke.addEventListener('click', () => this.revokeServer(server, revoke));
            row.append(info, revoke);
            list.append(row);
        }
    }

    /** Stop trusting one MCP server, through the audited backend command. */
    async revokeServer(server, button) {
        if (button) button.disabled = true;
        this._setServersStatus('');
        try {
            await window.__TAURI__.core.invoke('set_mcp_server_trusted', {
                server,
                trusted: false,
            });
            this.trustedServers = this.trustedServers.filter((s) => s !== server);
        } catch (e) {
            console.error('Failed to stop trusting MCP server:', e);
            const status = document.getElementById('trustedMcpServersStatus');
            setTextWithLtrValue(
                status,
                t('settings.tool_permissions.trusted_servers.revoke_failed', { server: LTR_SLOT }),
                server
            );
        }
        this.renderTrustedServers();
    }

    /**
     * Reset: revoke every MCP server the backend trusts — not just this
     * window's list, which may predate a trust granted from a prompt.
     */
    async _revokeAllServers(invoke) {
        let servers = this.trustedServers;
        try {
            const config = await invoke('get_config');
            servers = config?.tool_permissions?.trusted_mcp_servers ?? servers;
        } catch (e) {
            console.warn('Failed to read trusted MCP servers:', e);
        }
        for (const server of servers) {
            await invoke('set_mcp_server_trusted', { server, trusted: false }).catch((e) =>
                console.error('Failed to stop trusting MCP server:', e)
            );
        }
    }

    save(config) {
        config.tool_permissions = {
            trust_all: false, // deprecated, kept for compat
            terminator_mode: this.terminatorMode,
            // Owned by set_tool_kind_trusted / set_mcp_server_trusted (applied
            // immediately); keep the backend's current values — config is
            // fresh from get_config.
            trusted_kinds: config.tool_permissions?.trusted_kinds ?? [],
            trusted_mcp_servers: config.tool_permissions?.trusted_mcp_servers ?? [],
            // Owned by answer_host_command / remove_host_command_rule /
            // set_host_commands_allow_all; kept as the backend has them.
            host_command_rules: config.tool_permissions?.host_command_rules ?? [],
            host_commands_allow_all: config.tool_permissions?.host_commands_allow_all ?? false,
            // Every per-tool edit (policy, remove, reset) is persisted
            // immediately, so keep the backend's current list (config is
            // fresh from get_config) rather than this window's snapshot —
            // which would drop tools seen, and restore one-shot grants
            // consumed, since the settings window opened.
            tools: config.tool_permissions?.tools ?? this.tools,
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
                <div class="tools-empty">${t('settings.tool_permissions.empty')}</div>
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
                    ? `<span class="agent-tool-badge ext-badge">${t('settings.tool_permissions.tool.badge.extension')}</span>`
                    : `<span class="agent-tool-badge mcp-badge">${t('settings.tool_permissions.tool.badge.mcp')}</span>`;
                const grantedSuffix =
                    tool.grant_type === '24h' && tool.granted_at
                        ? t('settings.tool_permissions.tool.granted_at', {
                              date: new Date(tool.granted_at).toLocaleString(),
                          })
                        : '';
                return `
                <div class="agent-tool-item">
                    <div class="agent-tool-icon">${icon}</div>
                    <div class="agent-tool-info">
                        <div class="agent-tool-name">${escapeHtml(tool.title)} ${badge}</div>
                        <div class="agent-tool-meta">${t('settings.tool_permissions.tool.last_seen', { date: lastSeen })}${grantedSuffix}</div>
                    </div>
                    <select class="agent-tool-select" data-index="${index}" data-action-change="toolPermissions.updatePolicy" data-arg="${index}">
                        <option value="ask" ${tool.policy === 'ask' ? 'selected' : ''}>${t('settings.tool_permissions.tool.policy.ask')}</option>
                        <option value="allow" ${tool.policy === 'allow' && tool.grant_type === '24h' ? 'selected' : ''} data-grant="24h">${t('settings.tool_permissions.tool.policy.allow_24h')}</option>
                        <option value="allow" ${tool.policy === 'allow' && tool.grant_type === 'always' ? 'selected' : ''} data-grant="always">${t('settings.tool_permissions.tool.policy.always_allow')}</option>
                        <option value="deny" ${tool.policy === 'deny' ? 'selected' : ''}>${t('settings.tool_permissions.tool.policy.deny')}</option>
                    </select>
                    <button class="agent-tool-remove" data-action="toolPermissions.removeTool" data-arg="${index}" title="${t('settings.tool_permissions.tool.remove_title')}">✕</button>
                </div>
            `;
            })
            .join('');

        container.innerHTML = toolsHtml;
    }

    async updatePolicy(index, policy, grantType) {
        if (index >= 0 && index < this.tools.length) {
            const tool = this.tools[index];
            // The two "allow" options differ only by grant type; without it
            // the backend defaults to a one-shot grant. Mirror the backend's
            // write locally so a later manager save() stays consistent.
            const grant = grantType || 'once';
            tool.policy = policy;
            tool.grant_type = grant;
            tool.granted_at = new Date().toISOString();

            try {
                await window.__TAURI__.core.invoke('update_tool_policy', {
                    toolTitle: tool.title,
                    policy: policy,
                    grantType: grant,
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

        const events = AUDIT_FILTER_EVENTS[filter] || [filter];
        const filtered =
            filter === 'all' ? entries : entries.filter((e) => events.includes(e.event));

        if (filtered.length === 0) {
            list.innerHTML = `<div class="tools-empty" style="padding:14px; text-align:center; color:#938f9b;">
                ${entries.length === 0 ? t('settings.tool_permissions.audit.empty') : t('settings.tool_permissions.audit.no_match')}
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
            : t('settings.tool_permissions.audit.unknown_time');
        const kind = entry.event || 'unknown';

        let icon = '📝';
        let summary = '';
        let meta = '';

        switch (kind) {
            case 'granted':
                icon = '✅';
                summary = t('settings.tool_permissions.audit.summary.granted', {
                    tool: escapeHtml(entry.tool || '?'),
                    grant_type: escapeHtml(entry.grant_type || '?'),
                });
                if (entry.session_id)
                    meta = t('settings.tool_permissions.audit.meta.session', {
                        id: escapeHtml(entry.session_id.slice(0, 8)),
                    });
                if (entry.args_preview) {
                    meta +=
                        (meta ? ' · ' : '') +
                        `<code>${escapeHtml(entry.args_preview.slice(0, 140))}</code>`;
                }
                break;
            case 'denied':
                icon = '⛔';
                summary = t('settings.tool_permissions.audit.summary.denied', {
                    tool: escapeHtml(entry.tool || '?'),
                });
                if (entry.session_id)
                    meta = t('settings.tool_permissions.audit.meta.session', {
                        id: escapeHtml(entry.session_id.slice(0, 8)),
                    });
                break;
            case 'revoked':
                icon = '🗑️';
                summary = t('settings.tool_permissions.audit.summary.revoked', {
                    tool: escapeHtml(entry.tool || '?'),
                });
                if (entry.prior_policy)
                    meta = t('settings.tool_permissions.audit.meta.was_policy', {
                        policy: escapeHtml(entry.prior_policy),
                    });
                if (entry.prior_grant_type)
                    meta += (meta ? ' ' : '') + `(${escapeHtml(entry.prior_grant_type)})`;
                break;
            case 'expired':
                icon = '⏳';
                summary = t('settings.tool_permissions.audit.summary.expired', {
                    tool: escapeHtml(entry.tool || '?'),
                });
                if (entry.prior_grant_type)
                    meta = t('settings.tool_permissions.audit.meta.was_policy', {
                        policy: escapeHtml(entry.prior_grant_type),
                    });
                break;
            case 'terminator_mode_changed':
                icon = entry.enabled ? '⚠️' : '🛡️';
                summary = entry.enabled
                    ? t('settings.tool_permissions.audit.summary.terminator_on')
                    : t('settings.tool_permissions.audit.summary.terminator_off');
                break;
            case 'user_ran_command': {
                // A command the user ran themselves from a sandbox notice.
                icon = '▶️';
                summary =
                    entry.via === 'terminal'
                        ? t('settings.tool_permissions.audit.summary.user_ran_terminal')
                        : t('settings.tool_permissions.audit.summary.user_ran');
                const details = [];
                if (entry.outcome === 'exited' && Number.isInteger(entry.exit_code)) {
                    details.push(
                        t('settings.tool_permissions.audit.meta.exit_code', {
                            code: entry.exit_code,
                        })
                    );
                } else if (entry.outcome === 'cancelled') {
                    details.push(t('settings.tool_permissions.audit.meta.stopped'));
                } else if (entry.outcome === 'timed_out') {
                    details.push(t('settings.tool_permissions.audit.meta.timed_out'));
                } else if (entry.outcome === 'failed_to_start') {
                    details.push(t('settings.tool_permissions.audit.meta.not_started'));
                }
                if (entry.session_id) {
                    details.push(
                        t('settings.tool_permissions.audit.meta.session', {
                            id: escapeHtml(entry.session_id.slice(0, 8)),
                        })
                    );
                }
                if (entry.command) {
                    details.push(
                        `<code dir="ltr">${escapeHtml(String(entry.command).slice(0, 140))}</code>`
                    );
                }
                meta = details.join(' · ');
                break;
            }
            case 'host_command': {
                // A command an agent asked Kage to run outside its sandbox.
                const action = HOST_AUDIT_ACTIONS.includes(entry.action) ? entry.action : 'unknown';
                icon = HOST_AUDIT_ICONS[action] || '🛡️';
                summary = t(`settings.tool_permissions.audit.summary.host.${action}`, {
                    rule: escapeHtml(entry.rule || ''),
                });
                const details = [];
                if (entry.by) {
                    details.push(
                        t(
                            `settings.tool_permissions.audit.meta.host_by.${HOST_AUDIT_BY.includes(entry.by) ? entry.by : 'other'}`
                        )
                    );
                }
                if (Array.isArray(entry.guard) && entry.guard.length) {
                    details.push(escapeHtml(entry.guard.join(', ')));
                }
                if (entry.outcome === 'exited' && Number.isInteger(entry.exit_code)) {
                    details.push(
                        t('settings.tool_permissions.audit.meta.exit_code', {
                            code: entry.exit_code,
                        })
                    );
                } else if (entry.outcome === 'cancelled') {
                    details.push(t('settings.tool_permissions.audit.meta.stopped'));
                } else if (entry.outcome === 'timed_out') {
                    details.push(t('settings.tool_permissions.audit.meta.timed_out'));
                } else if (entry.outcome === 'failed_to_start') {
                    details.push(t('settings.tool_permissions.audit.meta.not_started'));
                } else if (entry.outcome) {
                    details.push(escapeHtml(String(entry.outcome).slice(0, 140)));
                }
                if (entry.agent) details.push(escapeHtml(String(entry.agent).slice(0, 64)));
                if (entry.session_id) {
                    details.push(
                        t('settings.tool_permissions.audit.meta.session', {
                            id: escapeHtml(entry.session_id.slice(0, 8)),
                        })
                    );
                }
                if (entry.command) {
                    details.push(
                        `<code dir="ltr">${escapeHtml(String(entry.command).slice(0, 140))}</code>`
                    );
                }
                meta = details.join(' · ');
                break;
            }
            case 'extension_agent_call': {
                // A background request an extension sent to the agent
                // (EXTENSION_AGENT_CALLS.md §4.5). Counts and ids only.
                icon = '🌙';
                summary = t('settings.tool_permissions.audit.summary.extension_agent_call', {
                    extension: escapeHtml(extensionDisplayName(entry.extension_id)),
                    chars: escapeHtml(formatCount(entry.prompt_chars)),
                    agent: ltrHtml(entry.agent),
                    purpose: escapeHtml(cleanText(entry.purpose, 24) || '?'),
                    outcome: escapeHtml(agentOutcomeLabel(entry.outcome)),
                });
                const label = cleanText(entry.label, 80);
                if (label) meta = escapeHtml(label);
                break;
            }
            case 'extension_agent_access_changed':
                icon = '🌙';
                summary = entry.enabled
                    ? t('settings.tool_permissions.audit.summary.extension_agent_access_on', {
                          extension: escapeHtml(extensionDisplayName(entry.extension_id)),
                          agent: ltrHtml(entry.agent),
                      })
                    : t('settings.tool_permissions.audit.summary.extension_agent_access_off', {
                          extension: escapeHtml(extensionDisplayName(entry.extension_id)),
                      });
                break;
            default:
                icon = '❔';
                summary = t('settings.tool_permissions.audit.summary.unknown', {
                    kind: escapeHtml(kind),
                });
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

/** Filter options that cover more than one event kind. */
const AUDIT_FILTER_EVENTS = Object.freeze({
    background_ai: ['extension_agent_call', 'extension_agent_access_changed'],
});

/** An agent name in an audit summary: escaped, left to right. */
function ltrHtml(value) {
    const text = cleanText(typeof value === 'string' ? value : '', 64) || '?';
    return `<bdi dir="ltr">${escapeHtml(text)}</bdi>`;
}

/** `host_command` audit actions (src/permission_audit/types.rs `HostCommand`). */
const HOST_AUDIT_ACTIONS = Object.freeze([
    'requested',
    'guard',
    'auto_approved',
    'approved',
    'denied',
    'expired',
    'withdrawn',
    'refused',
    'finished',
    'rule_added',
    'rule_removed',
    'allow_all_on',
    'allow_all_off',
]);
const HOST_AUDIT_BY = Object.freeze(['rule', 'allow_all', 'once', 'always']);
const HOST_AUDIT_ICONS = Object.freeze({
    requested: '🛡️',
    guard: '🔐',
    auto_approved: '⚡',
    approved: '✅',
    denied: '⛔',
    expired: '⏳',
    withdrawn: '↩️',
    refused: '⛔',
    finished: '▶️',
    rule_added: '➕',
    rule_removed: '🗑️',
    allow_all_on: '⚠️',
    allow_all_off: '🛡️',
});

// Global functions for onclick handlers
function updateToolPolicy(index, policy, grantType) {
    const settingsManager = getSettingsManager();
    const module = settingsManager?.modules.find((m) => m.id === 'tool-permissions');
    if (module) module.updatePolicy(index, policy, grantType);
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
        updateToolPolicy(parseInt(arg, 10), el.value, el.selectedOptions?.[0]?.dataset.grant);
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

    .trusted-servers-list .tools-empty {
        padding: 14px 16px;
    }
    .trusted-server-name {
        overflow-wrap: anywhere;
    }
    .trusted-server-revoke {
        padding: 4px 10px;
        background: rgba(244, 67, 54, 0.12);
        border: 1px solid rgba(244, 67, 54, 0.3);
        border-radius: 4px;
        color: #f44336;
        font-size: 12px;
        cursor: pointer;
        flex-shrink: 0;
    }
    .trusted-server-revoke:hover:not(:disabled) {
        background: rgba(244, 67, 54, 0.2);
    }
    .trusted-server-revoke:disabled {
        opacity: 0.6;
        cursor: default;
    }
    .trusted-servers-status {
        margin-top: 6px;
        font-size: 12px;
        /* Theme token: the fixed light red was ~2:1 on the light theme. */
        color: var(--kage-error, #e53e3e);
    }
    .trusted-servers-status:empty {
        display: none;
    }
    .host-rules-list .tools-empty {
        padding: 14px 16px;
    }
    .host-rule-name {
        overflow-wrap: anywhere;
    }
    .host-audit-link {
        padding: 0;
        border: none;
        background: none;
        color: var(--kage-accent);
        font-size: 13px;
        text-decoration: underline;
        cursor: pointer;
    }

    .terminator-row {
        border: 1px solid rgba(239, 68, 68, 0.3);
        border-radius: 8px;
        padding: 4px;
        background: rgba(239, 68, 68, 0.05);
    }
    .terminator-row .setting-label {
        color: var(--kage-error);
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
