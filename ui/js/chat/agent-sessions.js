import { errMessage } from '../shared/error-message.js';
import { t } from '../shared/i18n.js';
import { renderMarkdown } from '../shared/markdown.js';
import { mascotHTML } from '../shared/mascot.js';

/**
 * Agent session viewer — read-only display of sessions from registered
 * `AgentSessionProvider`s on the Rust side (kiro-cli, kiro-desktop,
 * future Claude Code / Codex / Ollama).
 *
 * The frontend is provider-agnostic: it bootstraps the list of
 * available providers via `agent_session_providers`, fetches sessions
 * for each available one in parallel, and merges them into a single
 * date-sorted list with provider chips on each item. Adding a new
 * agent on the backend requires no frontend changes beyond optionally
 * tweaking the chip icon for that provider.
 */

export class AgentSessionViewer {
    constructor(invoke, elements, chatApp) {
        this.invoke = invoke;
        this.elements = elements;
        this.chatApp = chatApp;
        this.providers = [];
        this.sessions = [];
        this.activeSessionId = null;
        this._pollInterval = null;
        this._pollUpdatedMs = 0;
    }

    _stopPolling() {
        if (this._pollInterval) {
            clearInterval(this._pollInterval);
            this._pollInterval = null;
        }
    }

    /** Poll for updates on the currently-loaded session; reload + render on change. */
    _startPolling(providerId, locator, updatedMs) {
        this._stopPolling();
        this._pollUpdatedMs = updatedMs;
        this._pollInterval = setInterval(async () => {
            try {
                const newTs = await this.invoke('agent_check_session_updated', {
                    providerId,
                    locator,
                    sinceMs: this._pollUpdatedMs,
                });
                if (!newTs) return;
                console.log('[AgentSessions] Session updated, reloading...');
                this._pollUpdatedMs = newTs;
                const messages = await this.invoke('agent_load_session', {
                    providerId,
                    locator,
                });
                const area = this.chatApp.elements.messagesArea;
                const session = this.sessions.find((s) => s.session_id === this.activeSessionId);
                if (!area) return;
                const prevCount = area.querySelectorAll('.message').length;
                const wasAtBottom = area.scrollTop + area.clientHeight >= area.scrollHeight - 50;
                this.renderMessages(area, messages, session);
                const newCount = area.querySelectorAll('.message').length;
                if (newCount > prevCount && prevCount > 0) {
                    const allMsgs = area.querySelectorAll('.message');
                    const dividerTarget = allMsgs[prevCount];
                    if (dividerTarget) {
                        const divider = document.createElement('div');
                        divider.className = 'kd-new-divider';
                        divider.textContent = t('chat.session_list.new_divider');
                        dividerTarget.before(divider);
                        const observer = new IntersectionObserver(
                            (entries) => {
                                if (entries[0].isIntersecting) {
                                    setTimeout(
                                        () => divider.classList.add('kd-new-divider-seen'),
                                        2000
                                    );
                                    observer.disconnect();
                                }
                            },
                            { root: area, threshold: 0.5 }
                        );
                        observer.observe(divider);
                    }
                }
                if (wasAtBottom) area.scrollTop = area.scrollHeight;
            } catch (_e) {
                // Silently ignore poll errors (db might be locked momentarily)
            }
        }, 3000);
    }

    /**
     * Load the registered providers and decide whether the source
     * toggle is worth showing. Returns true when at least one external
     * provider is available.
     */
    async init() {
        try {
            this.providers = await this.invoke('agent_session_providers');
        } catch (e) {
            console.warn('[AgentSessions] providers fetch failed:', e);
            this.providers = [];
        }
        const anyAvailable = this.providers.some((p) => p.available);
        if (!anyAvailable) return false;

        const toggle = document.getElementById('sessionSourceToggle');
        if (toggle) {
            toggle.style.display = 'flex';
            const btn = toggle.querySelector('[data-source="desktop"]');
            if (btn) {
                const labels = this.providers.filter((p) => p.available).map((p) => p.label);
                btn.textContent = labels.length === 1 ? labels[0] : 'Other Agents';
            }
        }
        return true;
    }

    async loadSessions() {
        try {
            console.log('[AgentSessions] Loading sessions...');
            const list = this.elements.sessionList;
            list.innerHTML =
                '<div class="kd-loading" style="display:flex;justify-content:center;padding:24px 0;"><div class="loading-dot"></div><div class="loading-dot"></div><div class="loading-dot"></div></div>';

            const available = this.providers.filter((p) => p.available);
            const results = await Promise.all(
                available.map((p) =>
                    this.invoke('agent_list_sessions', {
                        providerId: p.id,
                        limit: 100,
                    }).catch((e) => {
                        console.warn(`[AgentSessions] ${p.id} list failed:`, e);
                        return [];
                    })
                )
            );

            this.sessions = results.flat();
            this.sessions.sort((a, b) => (b.updated_at || '').localeCompare(a.updated_at || ''));
            console.log(
                `[AgentSessions] Loaded ${this.sessions.length} sessions across ${available.length} providers`
            );
            this.renderSessionList();
        } catch (e) {
            console.warn('[AgentSessions] Failed to load sessions:', e);
            this.sessions = [];
            this.renderSessionList();
        }
    }

    renderSessionList() {
        const list = this.elements.sessionList;
        const searchQuery = (this.elements.sessionSearch?.value || '').toLowerCase().trim();

        if (this.sessions.length === 0) {
            list.innerHTML = `<div class="session-list-empty">${t('chat.session_list.no_external')}</div>`;
            return;
        }

        const filtered = searchQuery
            ? this.sessions.filter((s) => s.title.toLowerCase().includes(searchQuery))
            : this.sessions;

        if (filtered.length === 0) {
            list.innerHTML = `<div class="session-list-empty">${t('chat.session_list.no_matches')}</div>`;
            return;
        }

        let html = '';
        for (const s of filtered) {
            const isActive = s.session_id === this.activeSessionId;
            const date = new Date(s.updated_at);
            const dateStr = formatRelativeDate(date);
            const provider = this.providers.find((p) => p.id === s.provider_id);
            const providerLabel = provider?.label || s.provider_id;
            const sourceIcon = providerIcon(s.provider_id);
            const filePath = s.extras?.file_path || '';
            const locatorJson = JSON.stringify(s.locator);

            html += `<div class="session-item kd-session-item ${isActive ? 'active' : ''}"
                data-session-id="${esc(s.session_id)}"
                data-provider-id="${esc(s.provider_id)}"
                data-locator="${esc(locatorJson)}"
                data-filepath="${esc(filePath)}">
                <div class="kd-session-content">
                    <div class="session-item-title">${sourceIcon} ${esc(s.title)}</div>
                    <div class="session-item-date">${dateStr} · ${esc(providerLabel)} · ${s.message_count} turns</div>
                </div>
                <div class="kd-session-actions">
                    ${
                        filePath
                            ? `<button class="kd-action-btn kd-folder-btn" title="${t('chat.session.kd_action.open_folder_title')}">
                        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
                    </button>`
                            : ''
                    }
                    ${
                        s.provider_id === 'kiro-desktop' && filePath.endsWith('.json')
                            ? `<button class="kd-action-btn kd-delete-btn" title="${t('chat.session.kd_action.delete_title')}">
                        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                    </button>`
                            : ''
                    }
                </div>
            </div>`;
        }

        list.innerHTML = html;

        list.querySelectorAll('.kd-session-item').forEach((item) => {
            item.addEventListener('click', async (e) => {
                if (e.target.closest('.kd-action-btn')) return;
                const sessionId = item.dataset.sessionId;
                const providerId = item.dataset.providerId;
                const locator = JSON.parse(item.dataset.locator);
                console.log(`[AgentSessions] Clicked session: ${providerId}/${sessionId}`);
                this.activeSessionId = sessionId;
                try {
                    await this.loadAndDisplaySession(providerId, locator, sessionId);
                } catch (e) {
                    console.error('[AgentSessions] Error loading session:', e);
                }
                list.querySelectorAll('.kd-session-item').forEach((el) =>
                    el.classList.remove('active')
                );
                item.classList.add('active');
            });

            const folderBtn = item.querySelector('.kd-folder-btn');
            if (folderBtn) {
                folderBtn.addEventListener('click', (e) => {
                    e.stopPropagation();
                    const fp = item.dataset.filepath;
                    if (fp)
                        this.invoke('kiro_desktop_open_folder', { filePath: fp }).catch(
                            console.warn
                        );
                });
            }

            const deleteBtn = item.querySelector('.kd-delete-btn');
            if (deleteBtn) {
                deleteBtn.addEventListener('click', async (e) => {
                    e.stopPropagation();
                    const fp = item.dataset.filepath;
                    if (!fp || !confirm(t('chat.session.kd_action.delete_confirm'))) return;
                    try {
                        await this.invoke('kiro_desktop_delete_session', { filePath: fp });
                        item.remove();
                        const sid = item.dataset.sessionId;
                        this.sessions = this.sessions.filter((s) => s.session_id !== sid);
                    } catch (err) {
                        console.warn('[AgentSessions] Delete failed:', err);
                    }
                });
            }
        });
    }

    async loadAndDisplaySession(providerId, locator, sessionId) {
        const area = this.chatApp.elements.messagesArea;
        if (!area) {
            console.error('[AgentSessions] messagesArea not found');
            return;
        }

        this._stopPolling();

        area.innerHTML = `<div class="kd-loading">${t('chat.session.kd_loading')}</div>`;

        const inputContainer = document.querySelector('.chat-input-container');
        if (inputContainer) inputContainer.style.display = 'none';

        try {
            const messages = await this.invoke('agent_load_session', {
                providerId,
                locator,
            });
            const session = this.sessions.find((s) => s.session_id === sessionId);
            console.log(`[AgentSessions] Loaded ${messages.length} messages`);
            this.renderMessages(area, messages, session);

            // Start live-polling for providers that support it. The
            // backend returns None for providers that don't, which the
            // poll loop handles gracefully.
            const updatedMs = new Date(session?.updated_at || Date.now()).getTime();
            this._startPolling(providerId, locator, updatedMs);
        } catch (e) {
            console.error('[AgentSessions] Failed to load session:', e);
            area.innerHTML = `<div class="kd-loading">Failed to load: ${esc(errMessage(e))}</div>`;
        }
    }

    renderMessages(container, messages, session) {
        container.innerHTML = '';

        const provider = this.providers.find((p) => p.id === session?.provider_id);
        const sourceLabel = provider?.label || session?.provider_id || 'Agent';
        const bannerIcon = providerIcon(session?.provider_id);
        const banner = document.createElement('div');
        banner.className = 'kd-readonly-banner';
        const workspace = session?.extras?.workspace;
        const model = session?.extras?.model;
        const filePath = session?.extras?.file_path;
        banner.innerHTML = `<details class="kd-banner-details">
            <summary>${bannerIcon} Read-only — ${esc(sourceLabel)} session</summary>
            <div class="kd-banner-info">
                ${workspace ? `<div>📁 Workspace: <code>${esc(workspace)}</code></div>` : ''}
                ${session?.session_id ? `<div>🆔 Session: <code>${esc(session.session_id)}</code></div>` : ''}
                ${model ? `<div>🤖 Model: ${esc(model)}</div>` : ''}
                ${filePath ? `<div>📄 File: <code>${esc(filePath)}</code></div>` : ''}
            </div>
        </details>`;
        container.appendChild(banner);

        for (const msg of messages) {
            if (msg.role === 'user') {
                const text = this.extractUserText(msg);
                if (!text.trim()) continue;
                const el = this.chatApp.createMessageElement('user', '');
                const contentDiv = el.querySelector('.message-content');
                if (contentDiv) {
                    const { cleanText, images } = extractInlineImages(text);
                    if (cleanText.trim()) renderMarkdown(cleanText, contentDiv);
                    for (const img of images) {
                        const imgEl = document.createElement('img');
                        imgEl.src = `data:${img.mime};base64,${img.data}`;
                        imgEl.style.cssText = 'max-width:100%;border-radius:8px;margin-top:8px;';
                        contentDiv.appendChild(imgEl);
                    }
                }
                container.appendChild(el);
            } else if (msg.role === 'assistant') {
                if (!msg.content.trim() || msg.content === 'On it.') continue;
                const el = this.chatApp.createMessageElement('assistant', '');
                const contentDiv = el.querySelector('.message-content');
                if (contentDiv) renderMarkdown(msg.content, contentDiv);
                container.appendChild(el);
            } else if (msg.role === 'tool') {
                if (!msg.content.trim()) continue;
                const el = document.createElement('div');
                el.className = 'message tool-message';
                const toolContent = msg.content;
                el.innerHTML = `<details class="kd-tool-details">
                    <summary class="kd-tool-summary">
                        🔧 Tool output
                        <button class="kd-tool-copy-btn" title="${t('chat.session.kd_tool_copy_title')}">
                            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                        </button>
                    </summary>
                    <pre class="kd-tool-output">${esc(toolContent)}</pre>
                </details>`;
                el.querySelector('.kd-tool-copy-btn')?.addEventListener('click', (e) => {
                    e.stopPropagation();
                    navigator.clipboard.writeText(toolContent).then(() => {
                        const btn = e.currentTarget;
                        btn.innerHTML =
                            '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>';
                        setTimeout(() => {
                            btn.innerHTML =
                                '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>';
                        }, 1500);
                    });
                });
                container.appendChild(el);
            }
        }

        container.scrollTop = 0;
    }

    extractUserText(msg) {
        return msg.content;
    }

    restoreInputArea() {
        this._stopPolling();
        const inputContainer = document.querySelector('.chat-input-container');
        if (inputContainer) inputContainer.style.display = '';
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const PROVIDER_ICONS = {
    'kiro-cli':
        '<svg class="kd-source-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="3" width="20" height="18" rx="2"/><polyline points="7 10 10 13 7 16"/><line x1="13" y1="16" x2="17" y2="16"/></svg>',
    'claude-code':
        '<svg class="kd-source-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>',
};

function providerIcon(providerId) {
    return PROVIDER_ICONS[providerId] || mascotHTML({ size: 14, className: 'kd-source-icon' });
}

function esc(s) {
    const d = document.createElement('div');
    d.textContent = s;
    return d.innerHTML;
}

function formatRelativeDate(date) {
    const now = new Date();
    const diff = now - date;
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return 'just now';
    if (mins < 60) return `${mins}m ago`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h ago`;
    const days = Math.floor(hours / 24);
    if (days < 7) return `${days}d ago`;
    return date.toLocaleDateString();
}

/** Extract base64 images from text that contains JSON-RPC prompt structures. */
function extractInlineImages(text) {
    const images = [];
    const regex = /"data":"([A-Za-z0-9+/=]{100,})","mimeType":"(image\/[a-z]+)"/g;
    let match;
    while ((match = regex.exec(text)) !== null) {
        images.push({ data: match[1], mime: match[2] });
    }
    const mdRegex = /!\[.*?\]\(data:(image\/[a-z]+);base64,([A-Za-z0-9+/=]{100,})\)/g;
    while ((match = mdRegex.exec(text)) !== null) {
        images.push({ data: match[2], mime: match[1] });
    }
    const cleanText = text
        .replace(/"data":"[A-Za-z0-9+/=]{100,}","mimeType":"image\/[a-z]+"/g, '[image]')
        .replace(/!\[.*?\]\(data:image\/[a-z]+;base64,[A-Za-z0-9+/=]{100,}\)/g, '[image]');
    return { cleanText, images };
}
