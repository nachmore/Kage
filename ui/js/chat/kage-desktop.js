import { renderMarkdown } from '../shared/markdown.js';

/**
 * Kage Desktop session viewer — read-only display of Kage IDE chat sessions.
 * Reuses the ChatApp's message rendering for consistent UX.
 */

export class KageDesktopViewer {
    constructor(invoke, elements, chatApp) {
        this.invoke = invoke;
        this.elements = elements;
        this.chatApp = chatApp; // Reference to ChatApp for message rendering
        this.sessions = [];
        this.workspaces = [];
        this.activeSessionId = null;
        this.activeWorkspace = null;
        this._pollInterval = null;
        this._pollUpdatedAt = 0;
    }

    /** Stop polling for CLI session updates. */
    _stopPolling() {
        if (this._pollInterval) {
            clearInterval(this._pollInterval);
            this._pollInterval = null;
        }
    }

    /** Start polling a CLI session for updates every 3 seconds. */
    _startPolling(conversationId, updatedAt) {
        this._stopPolling();
        this._pollUpdatedAt = updatedAt;
        this._pollInterval = setInterval(async () => {
            try {
                const newTs = await this.invoke('kage_cli_check_updated', {
                    conversationId,
                    lastUpdatedAt: this._pollUpdatedAt,
                });
                if (newTs) {
                    console.log(`[KageDesktop] CLI session updated, reloading...`);
                    this._pollUpdatedAt = newTs;
                    // Reload and re-render, preserving scroll + marking new messages
                    const messages = await this.invoke('kage_cli_load_session', { conversationId });
                    const area = this.chatApp.elements.messagesArea;
                    const session = this.sessions.find(s => s.id === conversationId);
                    if (area) {
                        const prevCount = area.querySelectorAll('.message').length;
                        const wasAtBottom = area.scrollTop + area.clientHeight >= area.scrollHeight - 50;
                        this.renderMessages(area, messages, session);
                        const newCount = area.querySelectorAll('.message').length;
                        // Insert a "new" divider before the new messages
                        if (newCount > prevCount && prevCount > 0) {
                            const allMsgs = area.querySelectorAll('.message');
                            const dividerTarget = allMsgs[prevCount];
                            if (dividerTarget) {
                                const divider = document.createElement('div');
                                divider.className = 'kd-new-divider';
                                divider.textContent = '● New';
                                dividerTarget.before(divider);
                                // Fade out when scrolled into view
                                const observer = new IntersectionObserver((entries) => {
                                    if (entries[0].isIntersecting) {
                                        setTimeout(() => divider.classList.add('kd-new-divider-seen'), 2000);
                                        observer.disconnect();
                                    }
                                }, { root: area, threshold: 0.5 });
                                observer.observe(divider);
                            }
                        }
                        if (wasAtBottom) area.scrollTop = area.scrollHeight;
                    }
                }
            } catch (e) {
                // Silently ignore poll errors (db might be locked momentarily)
            }
        }, 3000);
    }

    async init() {
        const [desktopAvailable, cliAvailable] = await Promise.all([
            this.invoke('kage_desktop_available'),
            this.invoke('kage_cli_available'),
        ]);
        if (!desktopAvailable && !cliAvailable) return false;

        this._hasDesktop = desktopAvailable;
        this._hasCli = cliAvailable;

        const toggle = document.getElementById('sessionSourceToggle');
        if (toggle) {
            toggle.style.display = 'flex';
            // Update label to reflect available sources
            const btn = toggle.querySelector('[data-source="desktop"]');
            if (btn) btn.textContent = 'Kage IDE & CLI';
        }

        if (desktopAvailable) {
            this.workspaces = await this.invoke('kage_desktop_workspaces');
        }
        return true;
    }

    async loadSessions(workspaceEncoded = null) {
        try {
            console.log('[KageDesktop] Loading sessions...');
            const list = this.elements.sessionList;
            list.innerHTML = '<div class="kd-loading" style="display:flex;justify-content:center;padding:24px 0;"><div class="loading-dot"></div><div class="loading-dot"></div><div class="loading-dot"></div></div>';

            // Load from both sources in parallel
            const promises = [];
            if (this._hasDesktop) {
                promises.push(this.invoke('kage_desktop_chat_sessions', { limit: 100 }));
            } else {
                promises.push(Promise.resolve([]));
            }
            if (this._hasCli) {
                promises.push(this.invoke('kage_cli_sessions', { limit: 100 }));
            } else {
                promises.push(Promise.resolve([]));
            }

            const [desktopSessions, cliSessions] = await Promise.all(promises);

            // Merge and sort by date
            this.sessions = [...desktopSessions, ...cliSessions];
            this.sessions.sort((a, b) => (b.updated_at || '').localeCompare(a.updated_at || ''));
            console.log(`[KageDesktop] Loaded ${this.sessions.length} sessions (${desktopSessions.length} desktop + ${cliSessions.length} cli)`);
            this.renderSessionList();
        } catch (e) {
            console.warn('[KageDesktop] Failed to load sessions:', e);
            this.sessions = [];
            this.renderSessionList();
        }
    }

    renderSessionList() {
        const list = this.elements.sessionList;
        const searchQuery = (this.elements.sessionSearch?.value || '').toLowerCase().trim();

        if (this.sessions.length === 0) {
            list.innerHTML = '<div class="session-list-empty">No Kage Desktop sessions found</div>';
            return;
        }

        const filtered = searchQuery
            ? this.sessions.filter(s => s.title.toLowerCase().includes(searchQuery))
            : this.sessions;

        if (filtered.length === 0) {
            list.innerHTML = '<div class="session-list-empty">No matching sessions</div>';
            return;
        }

        // Render interleaved by date (no workspace grouping)
        let html = '';
        for (const s of filtered) {
            const isActive = s.id === this.activeSessionId;
            const date = new Date(s.updated_at);
            const dateStr = formatRelativeDate(date);
            const sourceIcon = s.session_type === 'cli'
                ? '<svg class="kd-source-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="3" width="20" height="18" rx="2"/><polyline points="7 10 10 13 7 16"/><line x1="13" y1="16" x2="17" y2="16"/></svg>'
                : '<svg class="kd-source-icon" width="14" height="14" viewBox="0 0 65 47" fill="none"><path d="M5.7 33.3C21.4 50.4 43.7 49.7 56.8 37.7C64.9 30.3 68.9 13.9 55.4 3.7C41.9-6.4 32.4 11.2 17.3 8.7C14.1 8.2 9.9 9 12.7 12.7C13.2 13.4 13.9 14 14.5 14.5C10.2 14.6 8.7 14.4 6.1 14.3C3.7 14.2 2 14.4 1.1 15.6C-.2 17.5 3.2 20.5 6.2 23.1C8 24.8 9.8 27.1 11 29C9.6 28.8 9.3 28.7 7.2 28.5C3.9 28.1 1.4 28.6 5.7 33.3Z" fill="currentColor"/><path d="M48.5 21.9C46.4 22.1 45.8 19.6 45.7 18.1C45.6 16.7 45.8 15.6 46.2 14.9C46.5 14.2 47.1 13.9 47.8 13.8C48.6 13.7 49.3 14 49.8 14.6C50.3 15.2 50.7 16.3 50.8 17.7C51 20.2 50.1 21.8 48.5 21.9Z" fill="var(--kage-bg-primary, #1E1A24)"/><path d="M57.3 21.2C55.1 21.4 54.6 18.9 54.5 17.4C54.4 16 54.5 14.9 55 14.2C55.3 13.5 55.9 13.2 56.6 13.1C57.4 13 58 13.3 58.5 13.9C59.1 14.5 59.5 15.6 59.6 17C59.8 19.5 58.9 21.1 57.3 21.2Z" fill="var(--kage-bg-primary, #1E1A24)"/></svg>';
            const sourceLabel = s.session_type === 'cli' ? 'CLI' : 'Desktop';
            const wsShort = (s.workspace || '').split(/[/\\]/).pop() || '';

            html += `<div class="session-item kd-session-item ${isActive ? 'active' : ''}"
                data-session-id="${esc(s.id)}" data-workspace="${esc(s.workspace_encoded)}" data-filepath="${esc(s.file_path || '')}">
                <div class="kd-session-content">
                    <div class="session-item-title">${sourceIcon} ${esc(s.title)}</div>
                    <div class="session-item-date">${dateStr} · ${esc(sourceLabel)} · ${s.message_count} turns</div>
                </div>
                <div class="kd-session-actions">
                    <button class="kd-action-btn kd-folder-btn" title="Open folder">
                        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
                    </button>
                    <button class="kd-action-btn kd-delete-btn" title="Delete">
                        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                    </button>
                </div>
            </div>`;
        }

        list.innerHTML = html;

        list.querySelectorAll('.kd-session-item').forEach(item => {
            // Session click — load conversation
            item.addEventListener('click', async (e) => {
                // Don't trigger on action button clicks
                if (e.target.closest('.kd-action-btn')) return;
                console.log(`[KageDesktop] Clicked session: ${item.dataset.sessionId}`);
                this.activeSessionId = item.dataset.sessionId;
                this.activeWorkspace = item.dataset.workspace;
                try {
                    await this.loadAndDisplaySession(item.dataset.workspace, item.dataset.sessionId);
                } catch (e) {
                    console.error('[KageDesktop] Error loading session:', e);
                }
                list.querySelectorAll('.kd-session-item').forEach(el => el.classList.remove('active'));
                item.classList.add('active');
            });

            // Folder button
            const folderBtn = item.querySelector('.kd-folder-btn');
            if (folderBtn) {
                folderBtn.addEventListener('click', (e) => {
                    e.stopPropagation();
                    const fp = item.dataset.filepath;
                    if (fp) this.invoke('kage_desktop_open_folder', { filePath: fp }).catch(console.warn);
                });
            }

            // Delete button
            const deleteBtn = item.querySelector('.kd-delete-btn');
            if (deleteBtn) {
                deleteBtn.addEventListener('click', async (e) => {
                    e.stopPropagation();
                    const fp = item.dataset.filepath;
                    if (!fp || !confirm('Delete this session?')) return;
                    try {
                        await this.invoke('kage_desktop_delete_session', { filePath: fp });
                        item.remove();
                        this.sessions = this.sessions.filter(s => s.file_path !== fp);
                    } catch (err) {
                        console.warn('[KageDesktop] Delete failed:', err);
                    }
                });
            }
        });
    }

    async loadAndDisplaySession(workspaceEncoded, sessionId) {
        const area = this.chatApp.elements.messagesArea;
        if (!area) {
            console.error('[KageDesktop] messagesArea not found');
            return;
        }

        // Stop any existing polling
        this._stopPolling();

        area.innerHTML = '<div class="kd-loading">Loading session...</div>';

        // Hide input area for read-only mode
        const inputContainer = document.querySelector('.chat-input-container');
        if (inputContainer) inputContainer.style.display = 'none';

        try {
            let messages;
            const session = this.sessions.find(s => s.id === sessionId && s.workspace_encoded === workspaceEncoded);

            if (session?.session_type === 'cli') {
                console.log(`[KageDesktop] Loading CLI session: ${sessionId}`);
                messages = await this.invoke('kage_cli_load_session', { conversationId: sessionId });
                // Start polling for live updates
                const updatedMs = new Date(session.updated_at).getTime();
                this._startPolling(sessionId, updatedMs);
            } else if (session?.file_path?.endsWith('.chat')) {
                console.log(`[KageDesktop] Loading .chat file: ${session.file_path}`);
                messages = await this.invoke('kage_desktop_load_chat_file', { filePath: session.file_path });
            } else {
                console.log(`[KageDesktop] Loading workspace session: ${sessionId}`);
                messages = await this.invoke('kage_desktop_load_session', { workspaceEncoded, sessionId });
            }

            console.log(`[KageDesktop] Loaded ${messages.length} messages`);
            this.renderMessages(area, messages, session);
        } catch (e) {
            console.error('[KageDesktop] Failed to load session:', e);
            area.innerHTML = `<div class="kd-loading">Failed to load: ${esc(String(e))}</div>`;
        }
    }

    renderMessages(container, messages, session) {
        container.innerHTML = '';

        // Read-only banner with expandable details
        const sourceLabel = session?.session_type === 'cli' ? 'Kage CLI' : 'Kage Desktop';
        const bannerIcon = session?.session_type === 'cli'
            ? '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="vertical-align:-2px"><rect x="2" y="3" width="20" height="18" rx="2"/><polyline points="7 10 10 13 7 16"/><line x1="13" y1="16" x2="17" y2="16"/></svg>'
            : '<svg width="14" height="14" viewBox="0 0 65 47" fill="none" style="vertical-align:-2px"><path d="M5.7 33.3C21.4 50.4 43.7 49.7 56.8 37.7C64.9 30.3 68.9 13.9 55.4 3.7C41.9-6.4 32.4 11.2 17.3 8.7C14.1 8.2 9.9 9 12.7 12.7C13.2 13.4 13.9 14 14.5 14.5C10.2 14.6 8.7 14.4 6.1 14.3C3.7 14.2 2 14.4 1.1 15.6C-.2 17.5 3.2 20.5 6.2 23.1C8 24.8 9.8 27.1 11 29C9.6 28.8 9.3 28.7 7.2 28.5C3.9 28.1 1.4 28.6 5.7 33.3Z" fill="currentColor"/><path d="M48.5 21.9C46.4 22.1 45.8 19.6 45.7 18.1C45.6 16.7 45.8 15.6 46.2 14.9C46.5 14.2 47.1 13.9 47.8 13.8C48.6 13.7 49.3 14 49.8 14.6C50.3 15.2 50.7 16.3 50.8 17.7C51 20.2 50.1 21.8 48.5 21.9Z" fill="var(--kage-bg-primary, #1E1A24)"/><path d="M57.3 21.2C55.1 21.4 54.6 18.9 54.5 17.4C54.4 16 54.5 14.9 55 14.2C55.3 13.5 55.9 13.2 56.6 13.1C57.4 13 58 13.3 58.5 13.9C59.1 14.5 59.5 15.6 59.6 17C59.8 19.5 58.9 21.1 57.3 21.2Z" fill="var(--kage-bg-primary, #1E1A24)"/></svg>';
        const banner = document.createElement('div');
        banner.className = 'kd-readonly-banner';
        banner.innerHTML = `<details class="kd-banner-details">
            <summary>${bannerIcon} Read-only — ${esc(sourceLabel)} session</summary>
            <div class="kd-banner-info">
                ${session?.workspace ? `<div>📁 Workspace: <code>${esc(session.workspace)}</code></div>` : ''}
                ${session?.id ? `<div>🆔 Session: <code>${esc(session.id)}</code></div>` : ''}
                ${session?.model ? `<div>🤖 Model: ${esc(session.model)}</div>` : ''}
                ${session?.file_path ? `<div>📄 File: <code>${esc(session.file_path)}</code></div>` : ''}
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
                    // Check for inline base64 images in the text
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
                        <button class="kd-tool-copy-btn" title="Copy">
                            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                        </button>
                    </summary>
                    <pre class="kd-tool-output">${esc(toolContent)}</pre>
                </details>`;
                // Wire up copy button
                el.querySelector('.kd-tool-copy-btn')?.addEventListener('click', (e) => {
                    e.stopPropagation();
                    navigator.clipboard.writeText(toolContent).then(() => {
                        const btn = e.currentTarget;
                        btn.innerHTML = '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>';
                        setTimeout(() => { btn.innerHTML = '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>'; }, 1500);
                    });
                });
                container.appendChild(el);
            }
        }

        container.scrollTop = 0;
    }

    extractUserText(msg) {
        if (msg.content_blocks && msg.content_blocks.length > 0) {
            const userBlocks = msg.content_blocks.filter(b =>
                b.block_type === 'text' &&
                !b.text.startsWith('<identity>') &&
                !b.text.startsWith('## Included Rules') &&
                !b.text.startsWith('[KAGE_STEERING') &&
                !b.text.startsWith('Follow these instructions') &&
                b.text.length < 5000
            );
            if (userBlocks.length > 0) {
                return userBlocks.map(b => b.text).join('\n');
            }
        }
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
    // Match "data":"<base64>","mimeType":"<mime>" patterns
    const regex = /"data":"([A-Za-z0-9+/=]{100,})","mimeType":"(image\/[a-z]+)"/g;
    let match;
    while ((match = regex.exec(text)) !== null) {
        images.push({ data: match[1], mime: match[2] });
    }
    // Also match markdown image syntax: ![...](data:image/...;base64,...)
    const mdRegex = /!\[.*?\]\(data:(image\/[a-z]+);base64,([A-Za-z0-9+/=]{100,})\)/g;
    while ((match = mdRegex.exec(text)) !== null) {
        images.push({ data: match[2], mime: match[1] });
    }
    // Clean the text — remove the base64 data blobs
    let cleanText = text
        .replace(/"data":"[A-Za-z0-9+/=]{100,}","mimeType":"image\/[a-z]+"/g, '[image]')
        .replace(/!\[.*?\]\(data:image\/[a-z]+;base64,[A-Za-z0-9+/=]{100,}\)/g, '[image]');
    return { cleanText, images };
}
