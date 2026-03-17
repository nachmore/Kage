import { renderMarkdown } from '../shared/markdown.js';

/**
 * Kiro Desktop session viewer — read-only display of Kiro IDE chat sessions.
 * Reuses the ChatApp's message rendering for consistent UX.
 */

export class KiroDesktopViewer {
    constructor(invoke, elements, chatApp) {
        this.invoke = invoke;
        this.elements = elements;
        this.chatApp = chatApp; // Reference to ChatApp for message rendering
        this.sessions = [];
        this.workspaces = [];
        this.activeSessionId = null;
        this.activeWorkspace = null;
    }

    async init() {
        const available = await this.invoke('kiro_desktop_available');
        if (!available) return false;

        const toggle = document.getElementById('sessionSourceToggle');
        if (toggle) toggle.style.display = 'flex';

        this.workspaces = await this.invoke('kiro_desktop_workspaces');
        return true;
    }

    async loadSessions(workspaceEncoded = null) {
        try {
            console.log('[KiroDesktop] Loading sessions...');
            // Show spinner while loading
            const list = this.elements.sessionList;
            list.innerHTML = '<div class="kd-loading" style="display:flex;justify-content:center;padding:24px 0;"><div class="loading-dot"></div><div class="loading-dot"></div><div class="loading-dot"></div></div>';

            // Load .chat files — these have the full conversations with agent responses
            this.sessions = await this.invoke('kiro_desktop_chat_sessions', { limit: 200 });
            // Sort by date
            this.sessions.sort((a, b) => (b.updated_at || '').localeCompare(a.updated_at || ''));
            console.log(`[KiroDesktop] Loaded ${this.sessions.length} chat sessions`);
            this.renderSessionList();
        } catch (e) {
            console.warn('[KiroDesktop] Failed to load sessions:', e);
            this.sessions = [];
            this.renderSessionList();
        }
    }

    renderSessionList() {
        const list = this.elements.sessionList;
        const searchQuery = (this.elements.sessionSearch?.value || '').toLowerCase().trim();

        if (this.sessions.length === 0) {
            list.innerHTML = '<div class="session-list-empty">No Kiro Desktop sessions found</div>';
            return;
        }

        const filtered = searchQuery
            ? this.sessions.filter(s => s.title.toLowerCase().includes(searchQuery))
            : this.sessions;

        if (filtered.length === 0) {
            list.innerHTML = '<div class="session-list-empty">No matching sessions</div>';
            return;
        }

        // Group by workspace
        const byWorkspace = new Map();
        for (const s of filtered) {
            const ws = s.workspace || 'Unknown';
            if (!byWorkspace.has(ws)) byWorkspace.set(ws, []);
            byWorkspace.get(ws).push(s);
        }

        let html = '';
        for (const [ws, sessions] of byWorkspace) {
            const wsShort = ws.split(/[/\\]/).pop() || ws;
            html += `<div class="kd-workspace-header" title="${esc(ws)}">📁 ${esc(wsShort)}</div>`;

            for (const s of sessions) {
                const isActive = s.id === this.activeSessionId;
                const date = new Date(s.updated_at);
                const dateStr = formatRelativeDate(date);
                const typeIcon = s.session_type === 'vibe' ? '💬' : '🤖';

                html += `<div class="session-item kd-session-item ${isActive ? 'active' : ''}"
                    data-session-id="${esc(s.id)}" data-workspace="${esc(s.workspace_encoded)}" data-filepath="${esc(s.file_path || '')}">
                    <div class="kd-session-content">
                        <div class="session-item-title">${typeIcon} ${esc(s.title)}</div>
                        <div class="session-item-date">${dateStr} · ${s.message_count} turns</div>
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
        }

        list.innerHTML = html;

        list.querySelectorAll('.kd-session-item').forEach(item => {
            // Session click — load conversation
            item.addEventListener('click', async (e) => {
                // Don't trigger on action button clicks
                if (e.target.closest('.kd-action-btn')) return;
                console.log(`[KiroDesktop] Clicked session: ${item.dataset.sessionId}`);
                this.activeSessionId = item.dataset.sessionId;
                this.activeWorkspace = item.dataset.workspace;
                try {
                    await this.loadAndDisplaySession(item.dataset.workspace, item.dataset.sessionId);
                } catch (e) {
                    console.error('[KiroDesktop] Error loading session:', e);
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
                    if (fp) this.invoke('kiro_desktop_open_folder', { filePath: fp }).catch(console.warn);
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
                        await this.invoke('kiro_desktop_delete_session', { filePath: fp });
                        item.remove();
                        this.sessions = this.sessions.filter(s => s.file_path !== fp);
                    } catch (err) {
                        console.warn('[KiroDesktop] Delete failed:', err);
                    }
                });
            }
        });
    }

    async loadAndDisplaySession(workspaceEncoded, sessionId) {
        const area = this.chatApp.elements.messagesArea;
        if (!area) {
            console.error('[KiroDesktop] messagesArea not found');
            return;
        }

        area.innerHTML = '<div class="kd-loading">Loading session...</div>';

        // Hide input area for read-only mode
        const inputContainer = document.querySelector('.chat-input-container');
        if (inputContainer) inputContainer.style.display = 'none';

        try {
            // Find the session to check if it has a file_path (meaning it's a .chat file)
            const session = this.sessions.find(s => s.id === sessionId && s.workspace_encoded === workspaceEncoded);
            let messages;

            if (session?.file_path?.endsWith('.chat')) {
                // Load from .chat file (has full conversations)
                console.log(`[KiroDesktop] Loading .chat file: ${session.file_path}`);
                messages = await this.invoke('kiro_desktop_load_chat_file', { filePath: session.file_path });
            } else {
                // Load from workspace-session
                console.log(`[KiroDesktop] Loading workspace session: ${sessionId}`);
                messages = await this.invoke('kiro_desktop_load_session', { workspaceEncoded, sessionId });
            }

            console.log(`[KiroDesktop] Loaded ${messages.length} messages`);
            this.renderMessages(area, messages);
        } catch (e) {
            console.error('[KiroDesktop] Failed to load session:', e);
            area.innerHTML = `<div class="kd-loading">Failed to load: ${esc(String(e))}</div>`;
        }
    }

    renderMessages(container, messages) {
        container.innerHTML = '';

        // Read-only banner
        const banner = document.createElement('div');
        banner.className = 'kd-readonly-banner';
        banner.textContent = '🔒 Read-only — Kiro Desktop session';
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
                el.innerHTML = `<details class="kd-tool-details">
                    <summary class="kd-tool-summary">🔧 Tool output</summary>
                    <pre class="kd-tool-output">${esc(msg.content)}</pre>
                </details>`;
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
                !b.text.startsWith('[KIRO_STEERING') &&
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
