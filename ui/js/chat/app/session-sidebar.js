function animateTitleSwap(el, newText, animate) {
    if (!el) return;
    if (!animate) {
        el.textContent = newText;
        return;
    }
    el.classList.add('kd-title-flash');
    // Wait one frame so the fade-out is visible before the text swap.
    requestAnimationFrame(() => {
        el.textContent = newText;
        // The CSS animation handles the fade back in; remove the class
        // once it completes so subsequent changes don't double-trigger.
        setTimeout(() => el.classList.remove('kd-title-flash'), 700);
    });
}

export function createSessionSidebarMixin(dependencies) {
    const { escapeHtml, stripKageTags, STREAM, t, formatRelativeDate, orderSessionsForSidebar } =
        dependencies;
    return class {
        renderSessionList() {
            // Don't overwrite the list if we're viewing Kiro Desktop sessions
            if (window._kageSessionSource === 'desktop') return;

            const list = this.elements.sessionList;
            const searchQuery = (this.elements.sessionSearch?.value || '').toLowerCase().trim();

            if (this.sessions.length === 0) {
                if (this._loadingMore) return; // Still loading — don't show empty state
                list.innerHTML = `<div class="session-list-empty">${t('chat.sidebar.empty')}</div>`;
                return;
            }

            // "Default session" is the floating window's pinned session — the
            // one launcher chats land in. We pin it to the top of the sidebar
            // and badge it so the user can always find their main thread.
            // Pre-fix this also folded in `currentAcpSessionId` (this window's
            // active selection), which had two visible bugs: (1) clicking
            // around the list reshuffled which item lived in the top "default"
            // slot; (2) when the chat window's pinned session diverged from
            // floating's, both rows got the badge at once. The active
            // selection already gets the `.active` highlight, so it doesn't
            // need to also masquerade as the default.
            const defaultId = this.floatingSessionId;
            // Sort (default-pinned, newest-first) + filter (search, or hide
            // steering-only "New Chat" peers) — pure logic in session-render.js.
            const filtered = orderSessionsForSidebar(this.sessions, {
                defaultId,
                searchQuery,
                keepIds: [this.currentAcpSessionId, this.activeSessionId],
            });

            if (filtered.length === 0) {
                if (this._loadingMore || !this._sessionsFullyLoaded) {
                    // Still loading — show dots instead of empty state
                    if (!list.querySelector('.session-list-loader')) {
                        list.innerHTML =
                            '<div class="session-list-loader"><div class="loading-dot"></div><div class="loading-dot"></div><div class="loading-dot"></div></div>';
                    }
                    return;
                }
                list.innerHTML = `<div class="session-list-empty">${t('chat.session_list.no_matches')}</div>`;
                return;
            }

            // Build map of existing DOM items by session_id for diffing
            const existingById = new Map();
            list.querySelectorAll('.session-item[data-session-id]').forEach((el) => {
                existingById.set(el.dataset.sessionId, el);
            });

            // Build the desired ordered list of session_ids + separator
            // positions, plus a session_id → session map so the per-id
            // lookup below is O(1) instead of O(n) (the previous code
            // re-scanned `filtered` for every id, making the whole loop
            // O(n²) — noticeable past ~100 sessions).
            const desiredIds = [];
            const sessionById = new Map();
            for (const session of filtered) {
                sessionById.set(session.session_id, session);
                desiredIds.push(session.session_id);
                const isDefault = session.session_id === this.floatingSessionId;
                if (isDefault && !searchQuery) {
                    desiredIds.push('__separator__');
                }
            }

            // Remove items no longer in the filtered list
            for (const [id, el] of existingById) {
                if (!sessionById.has(id)) el.remove();
            }
            // Remove stale empty-state messages and separators (will re-add separator if needed)
            list.querySelectorAll('.session-list-empty, .session-list-separator').forEach((el) =>
                el.remove()
            );

            // Create or update each item, then ensure correct DOM order
            let insertionIndex = 0;
            for (const key of desiredIds) {
                if (key === '__separator__') {
                    // Insert separator if not already at this position
                    const current = list.children[insertionIndex];
                    if (!current?.classList.contains('session-list-separator')) {
                        const sep = document.createElement('div');
                        sep.className = 'session-list-separator';
                        if (current) list.insertBefore(sep, current);
                        else list.appendChild(sep);
                    }
                    insertionIndex++;
                    continue;
                }

                const session = sessionById.get(key);
                const isFloating = session.session_id === this.floatingSessionId;
                const isActive = session.session_id === this.activeSessionId;
                const isNew = !this._seenSessionIds.has(session.session_id);
                const title = stripKageTags(session.title) || t('chat.session.default_title');
                const date = new Date(session.updated_at || session.created_at);
                const dateStr = this.formatDate(date);

                let item = existingById.get(key);
                if (item) {
                    // Reuse existing DOM node — update only what changed
                    item.classList.toggle('active', isActive);
                    item.classList.toggle('session-new', isNew);

                    const titleEl = item.querySelector('.session-item-title');
                    const newDot = isNew
                        ? `<span class="session-new-dot" title="${t('chat.session.new_dot_title')}">●</span>`
                        : '';
                    // Badge + "default session" suffix represent floating's
                    // pinned thread only — the row this window happens to
                    // have selected gets `.active` styling instead.
                    const newTitleHtml = this._sessionTitleHtml(session.session_id, {
                        isNew,
                        isFloating,
                        title,
                    });
                    if (titleEl && titleEl.innerHTML !== newTitleHtml)
                        titleEl.innerHTML = newTitleHtml;

                    const dateEl = item.querySelector('.session-item-date');
                    const dateSuffix = isFloating
                        ? ' · <span class="session-default-label">default session</span>'
                        : '';
                    const newDateHtml = `${dateStr}${dateSuffix}`;
                    if (dateEl && dateEl.innerHTML !== newDateHtml) dateEl.innerHTML = newDateHtml;
                } else {
                    // Create new item
                    item = this._createSessionItem(session, {
                        isFloating,
                        isActive,
                        isNew,
                        title,
                        dateStr,
                    });
                    existingById.set(key, item);
                }

                // Ensure correct position in DOM
                if (list.children[insertionIndex] !== item) {
                    if (insertionIndex < list.children.length) {
                        list.insertBefore(item, list.children[insertionIndex]);
                    } else {
                        list.appendChild(item);
                    }
                }
                insertionIndex++;
            }

            // Remove any trailing stale children
            while (list.children.length > insertionIndex) {
                list.lastChild.remove();
            }

            // If the filtered list is too short to scroll, auto-load more
            if (
                !searchQuery &&
                filtered.length < 15 &&
                !this._sessionsFullyLoaded &&
                !this._loadingMore
            ) {
                this.loadMoreSessions();
            }
        }

        /** Create a new session-item DOM element with event listeners. */
        _createSessionItem(session, { isFloating, isActive, isNew, title, dateStr }) {
            const item = document.createElement('div');
            item.className =
                'session-item' + (isActive ? ' active' : '') + (isNew ? ' session-new' : '');
            item.dataset.sessionId = session.session_id;

            // See note in renderSessionList — only floating's pinned row
            // gets the default-session badge + suffix.
            const dateSuffix = isFloating
                ? ' · <span class="session-default-label">default session</span>'
                : '';

            item.innerHTML = `
                <div class="session-item-content">
                    <div class="session-item-title">${this._sessionTitleHtml(session.session_id, { isNew, isFloating, title })}</div>
                    <div class="session-item-date">${dateStr}${dateSuffix}</div>
                </div>
                <div class="session-item-actions">
                    <button class="session-action-btn session-action-edit" title="${t('chat.session.action.rename_title')}">
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.85 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/><path d="m15 5 4 4"/></svg>
                    </button>
                    <button class="session-action-btn session-action-reveal" title="${t('chat.session.action.reveal_title')}">
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/></svg>
                    </button>
                    <button class="session-action-btn session-action-delete" title="${t('chat.session.action.delete_title')}">
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
                    </button>
                </div>
            `;

            item.querySelector('.session-action-edit').addEventListener('click', (e) => {
                e.stopPropagation();
                this.startInlineRename(session.session_id, item);
            });
            item.querySelector('.session-action-reveal').addEventListener('click', (e) => {
                e.stopPropagation();
                this.revealSessionFile(session.session_id);
            });
            item.querySelector('.session-action-delete').addEventListener('click', (e) => {
                e.stopPropagation();
                this.deleteSession(session.session_id, title);
            });

            item.addEventListener('click', () => this.selectSession(session.session_id));
            return item;
        }

        /**
         * The full innerHTML of a sidebar row's `.session-item-title`. The
         * SINGLE builder for both the create path (_createSessionItem) and
         * the diff-update path (renderSessionList's reuse branch) — the two
         * previously built it independently, and the diff path forgot the
         * activity slot: any list re-render (loadSessions after a complete,
         * sessions_changed, a rename) rewrote the title without the span,
         * after which _refreshSessionBadges had nothing to update and the
         * spinner/unread badges silently died. One builder, one shape.
         */
        _sessionTitleHtml(sessionId, { isNew, isFloating, title }) {
            const newDot = isNew
                ? `<span class="session-new-dot" title="${t('chat.session.new_dot_title')}">●</span>`
                : '';
            const badges = isFloating ? '<span class="session-current-badge">●</span>' : '';
            const activity = this._sessionActivityBadgeHtml(sessionId);
            return `${newDot}${escapeHtml(title)}${badges}<span class="session-activity-slot">${activity}</span>`;
        }

        /**
         * Badge HTML for a session's live-activity state. Empty string when
         * the session is idle. The active session never shows a badge — its
         * activity is visible in the transcript itself.
         */
        _sessionActivityBadgeHtml(sessionId) {
            if (sessionId === this.activeSessionId) return '';
            const state = this.streamRegistry.states().get(sessionId);
            if (state === STREAM.STREAMING) {
                return `<span class="session-activity-spinner" title="${t('chat.session.activity.streaming')}"></span>`;
            }
            if (state === STREAM.UNREAD) {
                return `<span class="session-unread-dot" title="${t('chat.session.activity.unread')}">●</span>`;
            }
            return '';
        }

        /**
         * Update just the activity slots in the rendered sidebar — called
         * from the registry's onChange so badges track stream state without
         * a full list re-render (which would fight inline rename, scroll
         * position, etc.).
         */
        _refreshSessionBadges() {
            const list = this.elements.sessionList;
            if (!list) return;
            for (const item of list.querySelectorAll('.session-item')) {
                const sid = item.dataset.sessionId;
                const slot = item.querySelector('.session-activity-slot');
                if (sid && slot) slot.innerHTML = this._sessionActivityBadgeHtml(sid);
            }
        }

        formatDate(date) {
            return formatRelativeDate(date);
        }
    };
}
