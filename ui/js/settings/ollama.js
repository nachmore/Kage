import { uuidLite } from '../shared/agent-connections.js';
import { registerSettingsActions } from './module-registry.js';
import { SettingsModule } from './base.js';

/**
 * Settings → Ollama.
 *
 * First-class Ollama support (P1.1 from product_suggestions.md). Ollama
 * isn't an ACP agent itself; this page surfaces the integration so users
 * can answer the recurring "does Kage work with my local model?" without
 * digging through agent-connection spawn-command syntax.
 *
 * What the page does:
 *   - Toggle the integration on/off (purely a config flag — nothing
 *     happens automatically until the user runs the wizard).
 *   - Set the Ollama base URL. Defaults to http://localhost:11434.
 *   - "Test connection" probes /api/version + /api/tags and reports
 *     reachable / unreachable with a readable reason.
 *   - "Refresh models" populates the dropdown from /api/tags. Empty list
 *     gets a "no models — pull one with `ollama pull llama3`" hint.
 *   - "Use Ollama with Codex" wizard: builds the right shell-wrapped
 *     spawn command and adds (or replaces) a Codex agent connection
 *     wired at the local Ollama. Tells the user a restart is needed.
 *
 * Why route through codex-acp rather than ship our own ACP shim:
 * codex-acp already speaks OpenAI-compatible HTTP, and Ollama exposes
 * /v1/chat/completions. The env-var dance is the entire integration —
 * adding a separate Ollama-ACP adapter is more code without more value.
 */
export class OllamaSettingsModule extends SettingsModule {
    constructor() {
        super('ollama', 'Ollama', '🦙');
        // Cached probe + model results so the user can re-render without
        // re-hitting the daemon. Cleared on every "Test" / "Refresh".
        this._probe = null;
        this._models = [];
        this._loading = false;
        // Track whether actions are registered so we don't double-register
        // when the settings window is closed and reopened.
        this._actionsRegistered = false;
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                <p class="setting-description" style="margin-bottom:12px;">
                    Use a local model running on <a href="https://ollama.com/" target="_blank" rel="noreferrer noopener">Ollama</a> with Kage.
                    Free, private, no API key required. Works through the OpenAI-compatible Codex agent
                    — Kage handles the wiring for you.
                </p>

                <div class="setting-row">
                    <div class="setting-label">Enable Ollama integration</div>
                    <div class="setting-checkbox-row">
                        <label class="kage-checkbox">
                            <input type="checkbox" id="ollamaEnabled">
                        </label>
                        <div class="setting-description">
                            When on, the wizard below can install a Codex-via-Ollama connection.
                            Turning this off leaves the connection in place — switch in Agent Connection
                            if you want a different agent active.
                        </div>
                    </div>
                </div>

                <div class="setting-row">
                    <div class="setting-label">Ollama base URL</div>
                    <div class="setting-description">
                        Where the Ollama daemon is listening. Default is the local install.
                    </div>
                    <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                        <input type="text" class="setting-input" id="ollamaBaseUrl" placeholder="http://localhost:11434" style="flex:1;">
                        <button class="setting-button" data-action="ollamaTest">Test connection</button>
                    </div>
                    <div id="ollamaProbeStatus" class="setting-description" style="margin-top:8px;min-height:1em;"></div>
                </div>

                <div class="setting-row">
                    <div class="setting-label">Model</div>
                    <div class="setting-description">
                        Pulled models from this Ollama daemon. Click Refresh to re-scan.
                    </div>
                    <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                        <select class="setting-select" id="ollamaModel" style="flex:1;">
                            <option value="">—</option>
                        </select>
                        <button class="setting-button" data-action="ollamaRefreshModels">Refresh</button>
                    </div>
                    <div id="ollamaModelHint" class="setting-description" style="margin-top:8px;min-height:1em;"></div>
                </div>

                <div class="setting-section-label">One-click setup</div>

                <div class="setting-row">
                    <div class="setting-label">Use Ollama with Kage</div>
                    <div class="setting-description">
                        Adds a Codex agent connection wired to your Ollama daemon, and makes it active.
                        Kage will need a restart afterwards to switch backends.
                    </div>
                    <div class="setting-control">
                        <button class="setting-button" data-action="ollamaInstallWizard" id="ollamaInstallBtn">
                            Use Ollama with Codex
                        </button>
                    </div>
                    <div id="ollamaWizardStatus" class="setting-description" style="margin-top:8px;min-height:1em;"></div>
                </div>

                <details style="margin-top:16px;">
                    <summary style="cursor:pointer;font-size:12px;color:var(--kage-text-secondary);">Don't have Ollama yet?</summary>
                    <div class="setting-description" style="margin-top:8px;line-height:1.6;">
                        1. Install Ollama from <a href="https://ollama.com/download" target="_blank" rel="noreferrer noopener">ollama.com/download</a>.<br>
                        2. Start the daemon (it runs in the background; ignore the chat UI if any).<br>
                        3. Pull a model — for example <code>ollama pull llama3</code> or <code>ollama pull qwen2.5-coder</code>.<br>
                        4. Click Test connection above, pick the model, then run the wizard.
                    </div>
                </details>
            </div>
        `;
    }

    load(config) {
        const cfg = config.ollama || {};
        const enabled = document.getElementById('ollamaEnabled');
        const url = document.getElementById('ollamaBaseUrl');
        if (enabled) enabled.checked = !!cfg.enabled;
        if (url) url.value = cfg.base_url || 'http://localhost:11434';
        // Stash the previously-saved model so we can re-select it once
        // the dropdown finishes its async populate.
        this._savedModel = cfg.model || '';
        this._populateModelDropdown(this._models, this._savedModel);

        // Kick off a fresh probe + model list in the background so the
        // page lands populated. Failure is silent — the user sees the
        // empty state if Ollama isn't running.
        this._probeBackground();
    }

    save(config) {
        const enabled = document.getElementById('ollamaEnabled')?.checked ?? false;
        const baseUrl = document.getElementById('ollamaBaseUrl')?.value?.trim() || '';
        const modelSel = document.getElementById('ollamaModel');
        const model = modelSel?.value?.trim() || null;
        config.ollama = {
            enabled,
            base_url: baseUrl || 'http://localhost:11434',
            model,
        };
    }

    initialize() {
        if (this._actionsRegistered) return;
        this._actionsRegistered = true;
        registerSettingsActions({
            ollamaTest: () => this._runProbe(true),
            ollamaRefreshModels: () => this._refreshModels(true),
            ollamaInstallWizard: () => this._installWizard(),
        });
    }

    // --- probe + model fetch --------------------------------------------

    _currentBaseUrl() {
        return document.getElementById('ollamaBaseUrl')?.value?.trim() || 'http://localhost:11434';
    }

    async _probeBackground() {
        // Silent probe — only updates the status line if it succeeds or
        // explicitly fails. Used on initial load so the UI reflects state
        // without the user clicking anything.
        await this._runProbe(false);
        await this._refreshModels(false);
    }

    async _runProbe(verbose) {
        const baseUrl = this._currentBaseUrl();
        const status = document.getElementById('ollamaProbeStatus');
        if (verbose && status) status.textContent = 'Probing…';

        try {
            this._probe = await window.__TAURI__.core.invoke('ollama_probe', { baseUrl });
        } catch (e) {
            this._probe = { status: 'Unreachable', reason: this._formatError(e) };
        }

        if (status) {
            if (this._probe?.status === 'Reachable') {
                const v = this._probe.version ? ` (Ollama ${this._probe.version})` : '';
                status.textContent = `✓ Reachable${v}`;
                status.style.color = 'var(--kage-accent)';
            } else if (this._probe) {
                status.textContent = `✕ ${this._probe.reason || 'Unreachable'}`;
                status.style.color = '#c44';
            }
        }
    }

    async _refreshModels(verbose) {
        const baseUrl = this._currentBaseUrl();
        const hint = document.getElementById('ollamaModelHint');
        if (verbose && hint) {
            hint.textContent = 'Loading…';
            hint.style.color = '';
        }

        let models = [];
        try {
            models = await window.__TAURI__.core.invoke('ollama_list_models', { baseUrl });
        } catch (e) {
            this._models = [];
            this._populateModelDropdown([], this._savedModel || '');
            if (hint) {
                hint.textContent = `Couldn't list models: ${this._formatError(e)}`;
                hint.style.color = '#c44';
            }
            return;
        }

        this._models = Array.isArray(models) ? models : [];
        const previousSelection =
            document.getElementById('ollamaModel')?.value || this._savedModel || '';
        this._populateModelDropdown(this._models, previousSelection);

        if (hint) {
            if (this._models.length === 0) {
                hint.textContent =
                    'No models pulled yet. Try `ollama pull llama3` or `ollama pull qwen2.5-coder`.';
                hint.style.color = '';
            } else {
                hint.textContent = `${this._models.length} model${this._models.length === 1 ? '' : 's'} available.`;
                hint.style.color = 'var(--kage-text-secondary)';
            }
        }
    }

    _populateModelDropdown(models, selectedValue) {
        const sel = document.getElementById('ollamaModel');
        if (!sel) return;
        const opts = ['<option value="">—</option>'];
        for (const m of models || []) {
            const sizeStr = this._formatSize(m.size);
            const label = sizeStr ? `${m.name} — ${sizeStr}` : m.name;
            const escaped = String(m.name).replace(/[&<>"']/g, (c) => {
                return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
            });
            const labelEsc = String(label).replace(/[&<>"']/g, (c) => {
                return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
            });
            opts.push(`<option value="${escaped}">${labelEsc}</option>`);
        }
        sel.innerHTML = opts.join('');
        if (selectedValue) sel.value = selectedValue;
    }

    _formatSize(bytes) {
        if (typeof bytes !== 'number' || bytes <= 0) return '';
        const units = ['B', 'KB', 'MB', 'GB', 'TB'];
        let unitIdx = 0;
        let value = bytes;
        while (value >= 1024 && unitIdx < units.length - 1) {
            value /= 1024;
            unitIdx += 1;
        }
        const rounded = value >= 100 ? value.toFixed(0) : value.toFixed(1);
        return `${rounded} ${units[unitIdx]}`;
    }

    // --- wizard ---------------------------------------------------------

    async _installWizard() {
        const status = document.getElementById('ollamaWizardStatus');
        const setStatus = (text, kind) => {
            if (!status) return;
            status.textContent = text || '';
            status.style.color =
                kind === 'error' ? '#c44' : kind === 'success' ? 'var(--kage-accent)' : '';
        };

        const baseUrl = this._currentBaseUrl();
        const model = document.getElementById('ollamaModel')?.value?.trim();
        if (!model) {
            setStatus('Pick a model first.', 'error');
            return;
        }

        setStatus('Checking Ollama…');
        await this._runProbe(false);
        if (this._probe?.status !== 'Reachable') {
            setStatus(
                `Ollama isn't reachable at ${baseUrl}. Start the daemon and try again.`,
                'error'
            );
            return;
        }

        setStatus('Building Codex connection…');
        let spawnCommand;
        try {
            spawnCommand = await window.__TAURI__.core.invoke('ollama_codex_spawn_command', {
                baseUrl,
                model,
            });
        } catch (e) {
            setStatus('Could not build spawn command: ' + this._formatError(e), 'error');
            return;
        }

        // Read existing config, add or update a "Codex (Ollama)" connection,
        // and save back. Tagging the preset_id as `codex` lets the connections
        // page render the existing Codex preset metadata (install URL, etc.)
        // automatically; the spawn command differs from a vanilla Codex
        // setup, so we name it distinctively so the user can spot which is
        // which.
        const invoke = window.__TAURI__.core.invoke;
        let config;
        try {
            config = await invoke('get_config');
        } catch (e) {
            setStatus('Could not read config: ' + this._formatError(e), 'error');
            return;
        }

        if (!config.acp) config.acp = {};
        if (!Array.isArray(config.acp.connections)) config.acp.connections = [];

        const targetName = `Ollama (${model})`;
        const existing = config.acp.connections.find((c) => c.name === targetName);
        if (existing) {
            existing.mode = { type: 'local', spawn_command: spawnCommand };
            existing.preset_id = 'codex';
            existing.sessions_directory = existing.sessions_directory ?? null;
            config.acp.active_connection_id = existing.id;
        } else {
            const id = uuidLite();
            config.acp.connections.push({
                id,
                name: targetName,
                preset_id: 'codex',
                mode: { type: 'local', spawn_command: spawnCommand },
                sessions_directory: null,
            });
            config.acp.active_connection_id = id;
        }

        // Mirror the page's own state into the saved config.
        if (!config.ollama) config.ollama = {};
        config.ollama.enabled = true;
        config.ollama.base_url = baseUrl;
        config.ollama.model = model;

        try {
            await invoke('save_config', { config });
        } catch (e) {
            setStatus('Could not save config: ' + this._formatError(e), 'error');
            return;
        }

        // Reflect back in the page so the toggle + URL agree with what
        // we just wrote (they should already, but it's a cheap re-sync).
        const enabledBox = document.getElementById('ollamaEnabled');
        if (enabledBox) enabledBox.checked = true;

        setStatus(
            `✓ Connection "${targetName}" is now active. Restart Kage to switch backends.`,
            'success'
        );

        // Offer a restart prompt — same pattern Connection settings uses
        // when the active connection mode changes. Wrapped in try so a
        // missing dialog plugin (theoretically) wouldn't trap the page.
        try {
            const { ask } = window.__TAURI__.dialog || {};
            if (typeof ask === 'function') {
                const restart = await ask(
                    'Kage needs to restart to switch to the Ollama-backed Codex agent.\n\nRestart now?',
                    { title: 'Restart required', kind: 'info' }
                );
                if (restart) await invoke('restart_app');
            }
        } catch {
            // Non-fatal — user can restart manually.
        }
    }

    _formatError(e) {
        if (!e) return 'Unknown error';
        if (typeof e === 'string') return e;
        if (e instanceof Error) return e.message || String(e);
        if (typeof e === 'object') {
            if (typeof e.message === 'string' && e.message) return e.message;
            try {
                return JSON.stringify(e);
            } catch {
                return String(e);
            }
        }
        return String(e);
    }
}
