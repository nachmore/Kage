/**
 * Shared agent-connection helpers used by both the welcome wizard
 * (`welcome.html`) and the connection settings page
 * (`js/settings/connection.js`).
 *
 * Concerns:
 *   - Auto-detect installed ACP agents and render a "found" card.
 *   - Validate a saved connection (binary present? host/port set?).
 *   - Render a list of saved connections with click-to-edit + issue
 *     badges.
 *   - Render the edit form (Local / Remote) with preset metadata.
 *
 * ES module — import via `import { renderDetected, ... } from './agent-connections.js'`.
 *
 * Required globals at runtime:
 *   - `window.__TAURI__.core.invoke`
 */

import { escapeAttr, escapeHtml } from './tool-utils.js';

/**
 * "What kind of agent are you adding?" picker.
 *
 * Renders a modal overlay with four options, returns a Promise that
 * resolves to one of:
 *
 *   - `'detect'`     — auto-scan for installed ACP binaries
 *   - `'ollama'`     — open the Ollama wizard sub-flow
 *   - `'acp_preset'` — pick from the ACP preset list (Kiro / Claude / Codex)
 *   - `'custom'`     — raw spawn command / remote
 *   - `null`         — user cancelled
 *
 * Used by both the welcome wizard's "Or configure something else"
 * link and the connections list's `+ Add agent` button. Lives in
 * shared/ so neither owns the picker chrome and a future surface
 * (settings sub-page, command palette) can call it too.
 */
export function pickAgentType() {
    return new Promise((resolve) => {
        const overlay = document.createElement('div');
        overlay.className = 'agent-type-picker-overlay';
        overlay.innerHTML = `
            <div class="agent-type-picker-box" role="dialog" aria-label="Add an agent">
                <div class="agent-type-picker-title">Add an agent</div>
                <div class="agent-type-picker-desc">What kind of agent are you adding?</div>
                <button type="button" class="agent-type-card" data-kind="detect">
                    <span class="agent-type-icon">⚡</span>
                    <div class="agent-type-text">
                        <div class="agent-type-name">Auto-detect</div>
                        <div class="agent-type-sub">Scan this machine for installed agents (Kiro, Claude, Codex).</div>
                    </div>
                </button>
                <button type="button" class="agent-type-card" data-kind="ollama">
                    <span class="agent-type-icon">🦙</span>
                    <div class="agent-type-text">
                        <div class="agent-type-name">Ollama (local model)</div>
                        <div class="agent-type-sub">Pick a model running on a local Ollama daemon. Free, private, no API key.</div>
                    </div>
                </button>
                <button type="button" class="agent-type-card" data-kind="acp_preset">
                    <span class="agent-type-icon">🔌</span>
                    <div class="agent-type-text">
                        <div class="agent-type-name">ACP-compatible agent</div>
                        <div class="agent-type-sub">Wire up Kiro, Claude Code, or OpenAI Codex by hand.</div>
                    </div>
                </button>
                <button type="button" class="agent-type-card" data-kind="custom">
                    <span class="agent-type-icon">⚙️</span>
                    <div class="agent-type-text">
                        <div class="agent-type-name">Custom</div>
                        <div class="agent-type-sub">Raw spawn command or remote TCP connection. Advanced.</div>
                    </div>
                </button>
                <div class="agent-type-picker-actions">
                    <button type="button" class="setting-button agent-type-cancel">Cancel</button>
                </div>
            </div>
        `;
        document.body.appendChild(overlay);

        const close = (value) => {
            overlay.remove();
            document.removeEventListener('keydown', onKey);
            resolve(value);
        };
        const onKey = (e) => {
            if (e.key === 'Escape') {
                e.preventDefault();
                close(null);
            }
        };
        document.addEventListener('keydown', onKey);

        // Click on any card resolves with that kind. Backdrop click
        // (overlay itself, not the inner box) cancels.
        overlay.addEventListener('click', (e) => {
            const card = e.target.closest('.agent-type-card');
            if (card) {
                close(card.getAttribute('data-kind'));
                return;
            }
            if (e.target === overlay) {
                close(null);
                return;
            }
        });
        overlay.querySelector('.agent-type-cancel').addEventListener('click', () => close(null));
    });
}

// escapeHtml / escapeAttr are imported from ./tool-utils.js — single
// canonical source. The previous local copy was a quote-escaping
// regex that matched escapeAttr's behaviour, so call sites were doing
// attribute-safe escaping under the escapeHtml name. Migrated callers
// to escapeAttr where the value lands in an HTML attribute and to
// escapeHtml for body content.

export function uuidLite() {
    // Cheap RFC4122-shaped id; uniqueness within a single config is
    // all that matters here, so crypto.randomUUID-grade entropy is
    // overkill and not always available in older WebView2.
    return 'c-' + Math.random().toString(36).slice(2, 10) + '-' + Date.now().toString(36);
}

function getInvoke() {
    return window.__TAURI__?.core?.invoke;
}

/**
 * Render the auto-detect status into a container element. Returns
 * the list of detected agents so callers can decide what to do
 * (pre-fill a form, show a picker, fall through to manual config).
 *
 * @param {HTMLElement} container
 * @param {object} opts
 * @param {(agent: object) => void} [opts.onSelect] — called when the
 *   user clicks an "Use this" button on a detected agent card.
 * @param {() => void} [opts.onManual] — called when the user picks
 *   the manual-config link.
 * @param {string} [opts.searchingHtml] — override the searching state.
 * @returns {Promise<Array<object>>}
 */
export async function renderDetected(container, opts) {
    const invoke = getInvoke();
    opts = opts || {};
    if (!container || !invoke) return [];

    container.innerHTML =
        opts.searchingHtml || '<div class="agent-searching">🔍 Searching for agents…</div>';

    let agents = [];
    try {
        agents = (await invoke('detect_agents')) || [];
    } catch (e) {
        console.warn('detect_agents failed:', e);
        container.innerHTML =
            '<div class="agent-not-found">Could not detect agents. Configure the connection manually below.</div>';
        return [];
    }

    if (!agents.length) {
        container.innerHTML = `
            <div class="agent-not-found">
                No agents found automatically. You can install one — Kiro CLI, Claude Code, or OpenAI Codex —
                or configure the connection manually below.
            </div>`;
        return [];
    }

    // Render one card per detected agent. The "Use this" button is
    // wired below by walking the list (cheaper + safer than parsing
    // a data-attribute map back out of the DOM).
    const cards = agents
        .map((a, i) => {
            const versionHtml = a.version
                ? `<div class="agent-detected-version">${escapeHtml(a.version)}</div>`
                : '';
            return `
                <div class="agent-detected">
                    <div class="agent-detected-icon">✅</div>
                    <div class="agent-detected-status">${escapeHtml(a.name)} found</div>
                    <div class="agent-detected-name">${escapeHtml(a.name)}</div>
                    <div class="agent-detected-path">${escapeHtml(a.path)}</div>
                    ${versionHtml}
                    <button class="agent-use-btn" data-idx="${i}">Use this agent</button>
                </div>`;
        })
        .join('');

    const manualHtml = opts.onManual
        ? `<button class="agent-manual-link" id="agentDetectShowManual">Configure manually</button>`
        : '';

    container.innerHTML = cards + manualHtml;

    if (opts.onSelect) {
        container.querySelectorAll('.agent-use-btn').forEach((btn) => {
            btn.addEventListener('click', () => {
                const idx = parseInt(btn.getAttribute('data-idx'), 10);
                if (!Number.isNaN(idx) && agents[idx]) {
                    opts.onSelect(agents[idx]);
                }
            });
        });
    }
    if (opts.onManual) {
        const manual = document.getElementById('agentDetectShowManual');
        if (manual) manual.addEventListener('click', opts.onManual);
    }

    return agents;
}

/**
 * Build a connection object suitable for `acp.connections` from a
 * detected agent (as returned by `detect_agents`).
 */
export function connectionFromDetected(agent) {
    return {
        id: uuidLite(),
        name: agent.name || 'Detected agent',
        preset_id: agent.preset_id || null,
        mode: { type: 'local', spawn_command: agent.spawn_command },
        sessions_directory: null,
    };
}

/**
 * Validate a single connection's mode by asking the backend. Wraps
 * the Tauri command so callers don't have to handle the missing-
 * invoke fallback.
 */
export async function validateMode(mode) {
    const invoke = getInvoke();
    if (!invoke) {
        return { ok: true, issues: [], resolved_path: null };
    }
    try {
        return await invoke('validate_agent_connection', { mode });
    } catch (e) {
        console.warn('validate_agent_connection failed:', e);
        return { ok: false, issues: ['validation-failed'], resolved_path: null };
    }
}

/** Friendly copy for the issue codes returned by the backend. */
export function describeIssue(code) {
    switch (code) {
        case 'empty':
            return 'Spawn command is empty.';
        case 'binary-not-found':
            return 'Binary not found on disk or PATH.';
        case 'host-empty':
            return 'Host is empty.';
        case 'port-invalid':
            return 'Port is not valid.';
        case 'validation-failed':
            return 'Could not validate connection.';
        default:
            return code;
    }
}

export async function listPresets() {
    const invoke = getInvoke();
    if (!invoke) return [];
    try {
        return (await invoke('list_agent_presets')) || [];
    } catch (e) {
        console.warn('list_agent_presets failed:', e);
        return [];
    }
}

/**
 * Single source of truth for the local/remote edit form. Returns
 * an HTML string the caller drops into a container; subsequent
 * reads use `readEditForm(prefix)`.
 *
 * Uses an `idPrefix` so multiple instances can coexist on the same
 * page (today there's only one, but it costs nothing to be safe).
 */
export function renderEditForm(connection, opts) {
    opts = opts || {};
    const prefix = opts.idPrefix || 'connEdit';
    const c = connection || {
        id: '',
        name: '',
        preset_id: null,
        mode: { type: 'local', spawn_command: '' },
        sessions_directory: null,
    };
    const isLocal = c.mode?.type !== 'remote';
    const hidden = (cond) => (cond ? 'style="display:none;"' : '');
    const includeSessions = opts.includeSessionsDirectory !== false;

    // Layout style — 'wizard' (welcome.html, .form-group + .section-label)
    // or 'settings' (settings.html, .setting-row + .setting-label) so
    // each context can match its own surrounding chrome without
    // duplicating the form markup.
    const style = opts.style === 'settings' ? 'settings' : 'wizard';
    const wrap = style === 'settings' ? 'setting-row' : 'form-group';
    const labelClass = style === 'settings' ? 'setting-label' : 'section-label';
    const descClass = style === 'settings' ? 'setting-description' : 'section-desc';
    const desc = (text) => (text ? `<div class="${descClass}">${escapeHtml(text)}</div>` : '');
    const ctrlOpen = style === 'settings' ? '<div class="setting-control">' : '';
    const ctrlClose = style === 'settings' ? '</div>' : '';

    return `
        <div class="${wrap}">
            <div class="${labelClass}">Name</div>
            ${ctrlOpen}<input type="text" class="setting-input" id="${prefix}Name" value="${escapeAttr(c.name || '')}" placeholder="My agent">${ctrlClose}
        </div>

        <div class="${wrap}">
            <div class="${labelClass}">Connection Mode</div>
            ${ctrlOpen}<select class="setting-select" id="${prefix}Mode">
                <option value="local"${isLocal ? ' selected' : ''}>Local — spawn a process on this machine</option>
                <option value="remote"${!isLocal ? ' selected' : ''}>Remote — connect to a running server</option>
            </select>${ctrlClose}
        </div>

        <div id="${prefix}LocalSettings" ${hidden(!isLocal)}>
            <div class="${wrap}">
                <div class="${labelClass}">Spawn Command</div>
                ${desc('Full command to start the ACP server, including the path to the binary and any arguments.')}
                ${ctrlOpen}<input type="text" class="setting-input" id="${prefix}SpawnCommand"
                    value="${escapeAttr(isLocal ? c.mode?.spawn_command || '' : '')}"
                    placeholder="e.g., C:\\path\\to\\agent.exe acp">${ctrlClose}
            </div>
        </div>

        <div id="${prefix}RemoteSettings" ${hidden(isLocal)}>
            <div class="${wrap}">
                <div class="${labelClass}">Host</div>
                ${ctrlOpen}<input type="text" class="setting-input" id="${prefix}Host"
                    value="${escapeAttr(!isLocal ? c.mode?.host || '127.0.0.1' : '127.0.0.1')}">${ctrlClose}
            </div>
            <div style="display:flex;gap:12px;">
                <div class="${wrap}" style="flex:1;">
                    <div class="${labelClass}">Port</div>
                    ${ctrlOpen}<input type="number" class="setting-input" id="${prefix}Port"
                        value="${!isLocal ? c.mode?.port || 8765 : 8765}">${ctrlClose}
                </div>
                <div class="${wrap}" style="flex:1;">
                    <div class="${labelClass}">Timeout (ms)</div>
                    ${ctrlOpen}<input type="number" class="setting-input" id="${prefix}Timeout"
                        value="${!isLocal ? c.mode?.timeout_ms || 30000 : 30000}">${ctrlClose}
                </div>
            </div>
        </div>

        ${
            includeSessions
                ? `
        <div class="${wrap}">
            <div class="${labelClass}">Sessions directory</div>
            ${desc(`Where this agent stores its session files. Leave empty to use the agent's default location.`)}
            ${ctrlOpen}<input type="text" class="setting-input" id="${prefix}SessionsDir"
                value="${escapeAttr(c.sessions_directory || '')}"
                placeholder="Auto-detect">${ctrlClose}
        </div>`
                : ''
        }
    `;
}

/** Wire mode-toggle behaviour for an edit form rendered with `renderEditForm`. */
export function bindEditForm(prefix) {
    const sel = document.getElementById(`${prefix}Mode`);
    const local = document.getElementById(`${prefix}LocalSettings`);
    const remote = document.getElementById(`${prefix}RemoteSettings`);
    if (!sel || !local || !remote) return;
    sel.addEventListener('change', () => {
        const isLocal = sel.value === 'local';
        local.style.display = isLocal ? '' : 'none';
        remote.style.display = isLocal ? 'none' : '';
    });
}

/**
 * Read the values from a rendered edit form into a connection
 * object. Returns null if the required fields aren't present yet.
 */
export function readEditForm(prefix, existing) {
    const sel = document.getElementById(`${prefix}Mode`);
    if (!sel) return null;
    const name = document.getElementById(`${prefix}Name`)?.value?.trim() || 'Untitled';
    const isLocal = sel.value === 'local';
    const mode = isLocal
        ? {
              type: 'local',
              spawn_command: document.getElementById(`${prefix}SpawnCommand`)?.value?.trim() || '',
          }
        : {
              type: 'remote',
              host: document.getElementById(`${prefix}Host`)?.value?.trim() || '127.0.0.1',
              port: parseInt(document.getElementById(`${prefix}Port`)?.value || '8765', 10),
              timeout_ms: parseInt(
                  document.getElementById(`${prefix}Timeout`)?.value || '30000',
                  10
              ),
          };
    const sessionsInput = document.getElementById(`${prefix}SessionsDir`);
    const sessions_directory = sessionsInput
        ? sessionsInput.value.trim() || null
        : (existing?.sessions_directory ?? null);
    return {
        id: existing?.id || uuidLite(),
        name,
        preset_id: existing?.preset_id || null,
        mode,
        sessions_directory,
    };
}
