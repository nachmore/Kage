/**
 * Extension sandbox host.
 *
 * Runs in the main window (floating, chat, etc.). Responsible for:
 *   - Creating a sandboxed iframe for each loaded extension
 *   - Opening a MessageChannel to the iframe and transferring one end
 *   - Authoritatively enforcing capability permissions on every
 *     invoke() the extension makes (reject if command is forbidden or
 *     out-of-capability before the underlying Tauri call)
 *   - Routing RPC calls from the main window to the sandbox
 *     (match/matchAsync/execute/getTools/executeTool/getTriggers/…)
 *
 * Security:
 *   - iframes are loaded with `sandbox="allow-scripts"` only. No
 *     `allow-same-origin`, so the iframe gets a null origin: no cookies,
 *     no parent DOM access, no `window.__TAURI__`.
 *   - Extension source is fetched by the host (via `read_extension_file`)
 *     and handed to the sandbox as plain text. The sandbox never makes a
 *     network request.
 *   - The host never trusts identity claims from the sandbox. The
 *     extension id is known to the host because the host created the
 *     iframe; messages arriving on the port inherit that identity.
 */

import { decideInvoke } from './extension-permissions.js';

const SANDBOX_PATH = 'extension-sandbox.html';
const BOOT_TIMEOUT_MS = 10_000;
const RPC_TIMEOUT_MS = 10_000;

/**
 * Commands that read or write per-extension storage and therefore must be
 * scoped to the calling extension's identity. The host force-injects
 * `extension_id` into the args before forwarding; any value the sandbox
 * supplied is overwritten. Backend rejects calls without a valid id, so a
 * missing entry here would surface as a hard error rather than silent
 * cross-extension access.
 */
const STORAGE_COMMANDS = new Set([
    'save_extension_data',
    'load_extension_data',
    'delete_extension_data',
]);

/**
 * @typedef {object} SandboxSpec
 * @property {string} extensionId
 * @property {string[]} capabilities - normalized list of granted capabilities
 * @property {object} config - the extension's config object
 * @property {object} sources - map of provider kind → JS source text
 * @property {string} [sources.searchProvider]
 * @property {string} [sources.toolProvider]
 * @property {string} [sources.triggerProvider]
 * @property {Record<string,string>} [sharedSources] - relative-path → source
 *   for modules the extension imports via `import ... from './rel.js'`.
 * @property {Record<string,string>} [vendorSources] - name → UMD/IIFE source
 *   for allow-listed vendor libs that set globals (e.g. mathjs → window.math).
 *   Injected via `<script>` tag before provider modules are evaluated.
 */

/**
 * One running sandbox. Wraps the iframe, the host-side message port, and
 * tracks in-flight RPCs.
 */
export class ExtensionSandbox {
    /**
     * @param {SandboxSpec} spec
     * @param {Function} rawInvoke - the unwrapped Tauri invoke; the host uses
     *   this itself after permission checks pass.
     * @param {HTMLElement} container - DOM node to mount the hidden iframe into
     */
    constructor(spec, rawInvoke, container) {
        this.extensionId = spec.extensionId;
        this.capabilities = new Set(spec.capabilities || []);
        this.config = spec.config || {};
        this.sources = spec.sources || {};
        /** Relative-path → source map for shared modules (see runtime.js). */
        this.sharedSources = spec.sharedSources || {};
        /** Name → UMD/IIFE source map for allow-listed vendor globals. */
        this.vendorSources = spec.vendorSources || {};
        this._rawInvoke = rawInvoke;
        this._container = container;

        this._iframe = null;
        this._port = null;
        this._ready = false;
        /** @type {Map<number, {resolve:Function, reject:Function, timer:any}>} */
        this._pendingRpcs = new Map();
        this._nextRpcId = 1;
        this._destroyed = false;

        // Capabilities surfaced by the extension after init-ack.
        this.hasSearch = false;
        this.hasTools = false;
        this.hasTriggers = false;
        this.hasSettings = false;
        this.hasToolbar = false;
        this.hasFormatter = false;
        /** If true, formatMessage RPCs fire on streaming chunks too. */
        this.formatterOptsInStreaming = false;
        /** @type {string[]} ids of widgets the extension declared, post-init */
        this.widgetIds = [];
    }

    /**
     * Create the iframe and wait until the sandbox runtime sends `ready`
     * followed by `init-ack`. Rejects on timeout or runtime error.
     */
    async start() {
        if (this._iframe) throw new Error('sandbox already started');

        const iframe = document.createElement('iframe');
        iframe.dataset.extensionId = this.extensionId;
        iframe.setAttribute('sandbox', 'allow-scripts');
        iframe.setAttribute('aria-hidden', 'true');
        // We position offscreen rather than using visibility:hidden or
        // width/height:0, because Chromium throttles timers and task
        // scheduling in frames that never intersect the viewport. A
        // zero-sized hidden iframe would run the extension sandbox at a
        // crawl (observed: 1-second setTimeouts firing after 3+ seconds
        // during rapid keystrokes), causing RPCs to time out.
        iframe.style.cssText =
            'position:fixed;top:0;left:0;width:1px;height:1px;border:0;opacity:0;pointer-events:none;';
        this._iframe = iframe;
        this._container.appendChild(iframe);
        // Set src *after* appendChild so there's exactly one 'load' event
        // for the sandbox document. If src is set before insertion, some
        // browsers fire an extra load for the initial about:blank, which
        // we'd then race against.
        iframe.src = SANDBOX_PATH;

        const channel = new MessageChannel();
        this._port = channel.port1;
        this._port.onmessage = (ev) => this._onPortMessage(ev);
        this._port.onmessageerror = (ev) => {
            console.warn(`[sandbox ${this.extensionId}] port messageerror:`, ev);
        };
        this._port.start();

        // Wait for the iframe to load before handshaking — otherwise the
        // postMessage could race past the runtime's message listener.
        await this._waitForIframeLoad();

        const readyPromise = this._waitForReady();
        iframe.contentWindow.postMessage({ type: 'handshake' }, '*', [channel.port2]);

        await readyPromise;

        const initResult = await this._rpcRaw(
            {
                type: 'init',
                config: this.config,
                sources: this.sources,
                sharedSources: this.sharedSources,
                vendorSources: this.vendorSources,
            },
            { ackType: 'init-ack' }
        );

        if (!initResult.ok) {
            throw new Error(`sandbox init failed: ${initResult.error || 'unknown'}`);
        }
        this.hasSearch = !!initResult.result?.hasSearch;
        this.hasTools = !!initResult.result?.hasTools;
        this.hasTriggers = !!initResult.result?.hasTriggers;
        this.hasSettings = !!initResult.result?.hasSettings;
        this.hasToolbar = !!initResult.result?.hasToolbar;
        this.hasFormatter = !!initResult.result?.hasFormatter;
        this.formatterOptsInStreaming = !!initResult.result?.formatterOptsInStreaming;
        this.widgetIds = Array.isArray(initResult.result?.widgetIds)
            ? initResult.result.widgetIds.slice()
            : [];
    }

    /**
     * Send an RPC to the sandbox and wait for its response.
     * Times out after RPC_TIMEOUT_MS so a hung extension cannot wedge us.
     */
    async call(method, params) {
        if (this._destroyed) throw new Error(`sandbox '${this.extensionId}' is destroyed`);
        if (!this._ready) throw new Error(`sandbox '${this.extensionId}' not ready`);
        const rpcId = this._nextRpcId++;
        return new Promise((resolve, reject) => {
            const timer = setTimeout(() => {
                this._pendingRpcs.delete(rpcId);
                reject(new Error(`RPC '${method}' timed out after ${RPC_TIMEOUT_MS}ms`));
            }, RPC_TIMEOUT_MS);
            this._pendingRpcs.set(rpcId, { resolve, reject, timer });
            try {
                this._port.postMessage({ type: 'rpc', rpcId, method, params });
            } catch (e) {
                clearTimeout(timer);
                this._pendingRpcs.delete(rpcId);
                reject(e);
            }
        });
    }

    /** Push a config update into the sandbox. Best-effort; errors are logged. */
    async updateConfig(config) {
        this.config = config || {};
        try {
            await this.call('onConfigUpdate', { config: this.config });
        } catch (e) {
            console.warn(`[sandbox ${this.extensionId}] onConfigUpdate failed:`, e);
        }
    }

    /** Tear down the iframe and reject all in-flight RPCs. */
    destroy() {
        if (this._destroyed) return;
        this._destroyed = true;

        // Best-effort: ask the extension to clean up inside the sandbox
        // before we tear the iframe out from under it.
        try {
            if (this._ready && this._port) {
                this._port.postMessage({ type: 'rpc', rpcId: -1, method: 'destroy', params: {} });
            }
        } catch {}

        for (const [, entry] of this._pendingRpcs) {
            clearTimeout(entry.timer);
            entry.reject(new Error('sandbox destroyed'));
        }
        this._pendingRpcs.clear();

        try {
            this._port?.close();
        } catch {}
        this._port = null;

        try {
            this._iframe?.remove();
        } catch {}
        this._iframe = null;
    }

    // --- internals ---------------------------------------------------------

    _waitForIframeLoad() {
        return new Promise((resolve, reject) => {
            const onLoad = () => {
                this._iframe.removeEventListener('load', onLoad);
                this._iframe.removeEventListener('error', onErr);
                resolve();
            };
            const onErr = (e) => {
                this._iframe.removeEventListener('load', onLoad);
                this._iframe.removeEventListener('error', onErr);
                reject(new Error(`iframe load failed: ${e?.message || e}`));
            };
            this._iframe.addEventListener('load', onLoad, { once: true });
            this._iframe.addEventListener('error', onErr, { once: true });
        });
    }

    _waitForReady() {
        return new Promise((resolve, reject) => {
            const timeout = setTimeout(() => {
                this._readyResolver = null;
                reject(
                    new Error(
                        `sandbox '${this.extensionId}' did not signal ready within ${BOOT_TIMEOUT_MS}ms`
                    )
                );
            }, BOOT_TIMEOUT_MS);
            this._readyResolver = () => {
                clearTimeout(timeout);
                this._readyResolver = null;
                this._ready = true;
                resolve();
            };
        });
    }

    /**
     * Like call() but used during init when we have a different ack type
     * (init-ack) than the regular rpc-response envelope.
     */
    _rpcRaw(msg, { ackType }) {
        return new Promise((resolve, reject) => {
            const timer = setTimeout(() => {
                this._pendingInit = null;
                reject(new Error(`init timed out after ${BOOT_TIMEOUT_MS}ms`));
            }, BOOT_TIMEOUT_MS);
            this._pendingInit = { ackType, resolve, reject, timer };
            try {
                this._port.postMessage(msg);
            } catch (e) {
                clearTimeout(timer);
                this._pendingInit = null;
                reject(e);
            }
        });
    }

    _onPortMessage(ev) {
        const msg = ev.data;
        if (!msg || typeof msg !== 'object') return;

        switch (msg.type) {
            case 'ready': {
                if (this._readyResolver) this._readyResolver();
                break;
            }
            case 'init-ack': {
                const pending = this._pendingInit;
                if (!pending || pending.ackType !== 'init-ack') return;
                clearTimeout(pending.timer);
                this._pendingInit = null;
                pending.resolve({ ok: !!msg.ok, result: msg.result, error: msg.error });
                break;
            }
            case 'rpc-response': {
                const entry = this._pendingRpcs.get(msg.rpcId);
                if (!entry) return;
                clearTimeout(entry.timer);
                this._pendingRpcs.delete(msg.rpcId);
                if (msg.error) entry.reject(new Error(msg.error));
                else entry.resolve(msg.result);
                break;
            }
            case 'invoke': {
                this._handleInvoke(msg);
                break;
            }
            case 'log': {
                this._handleLog(msg);
                break;
            }
            default:
                // unknown — ignore
                break;
        }
    }

    async _handleInvoke(msg) {
        const { id, command, args } = msg;
        const decision = decideInvoke(command, this.capabilities);
        if (!decision.allow) {
            this._port.postMessage({
                type: 'invoke-response',
                id,
                error: `Extension '${this.extensionId}': ${decision.reason}`,
            });
            console.warn(
                `[sandbox ${this.extensionId}] BLOCKED invoke('${command}'): ${decision.reason}`
            );
            return;
        }

        // For storage commands, force-inject the extension's identity. The
        // host owns the sandbox -> extension mapping (it created the
        // iframe), so this is authoritative. Any extension_id the sandbox
        // tried to supply is overwritten — sandboxes can't read or write
        // another extension's data even if they know the key.
        const forwardedArgs = STORAGE_COMMANDS.has(command)
            ? { ...(args || {}), extension_id: this.extensionId }
            : args || {};

        try {
            const result = await this._rawInvoke(command, forwardedArgs);
            this._port.postMessage({ type: 'invoke-response', id, result });
        } catch (e) {
            this._port.postMessage({
                type: 'invoke-response',
                id,
                error: String(e?.message || e),
            });
        }
    }

    _handleLog(msg) {
        const prefix = `[ext ${this.extensionId}]`;
        const level = msg.level || 'info';
        const text = typeof msg.msg === 'string' ? msg.msg : JSON.stringify(msg.msg);
        switch (level) {
            case 'error':
                console.error(prefix, text);
                break;
            case 'warn':
                console.warn(prefix, text);
                break;
            case 'debug':
                console.debug(prefix, text);
                break;
            default:
                console.log(prefix, text);
                break;
        }
    }
}

/**
 * Pool of sandboxes keyed by extension id. Owns a single hidden container
 * div that all extension iframes mount into.
 */
export class ExtensionSandboxPool {
    /** @param {Function} rawInvoke */
    constructor(rawInvoke) {
        this._rawInvoke = rawInvoke;
        this._container = null;
        /** @type {Map<string, ExtensionSandbox>} */
        this._sandboxes = new Map();
    }

    _ensureContainer() {
        if (this._container) return this._container;
        let el = document.getElementById('kage-extension-sandboxes');
        if (!el) {
            el = document.createElement('div');
            el.id = 'kage-extension-sandboxes';
            el.setAttribute('aria-hidden', 'true');
            // Sized to hold the 1px-tall sandbox iframes non-clipped so
            // Chromium doesn't apply intersection-throttling to them. We
            // keep the container positioned off the visible layout, with
            // opacity:0 so it never paints. (A 0x0 overflow:hidden
            // container clips the child iframes to zero rendered area,
            // which Chromium treats as backgrounded — timers get
            // throttled and RPCs time out during rapid input.)
            el.style.cssText =
                'position:fixed;top:0;left:0;width:1px;height:1px;opacity:0;pointer-events:none;z-index:-1;';
            document.body.appendChild(el);
        }
        this._container = el;
        return el;
    }

    /** @param {SandboxSpec} spec */
    async load(spec) {
        if (this._sandboxes.has(spec.extensionId)) {
            throw new Error(`sandbox for '${spec.extensionId}' already loaded`);
        }
        const container = this._ensureContainer();
        const sb = new ExtensionSandbox(spec, this._rawInvoke, container);
        this._sandboxes.set(spec.extensionId, sb);
        try {
            await sb.start();
        } catch (e) {
            this._sandboxes.delete(spec.extensionId);
            sb.destroy();
            throw e;
        }
        return sb;
    }

    get(extensionId) {
        return this._sandboxes.get(extensionId) || null;
    }

    has(extensionId) {
        return this._sandboxes.has(extensionId);
    }

    unload(extensionId) {
        const sb = this._sandboxes.get(extensionId);
        if (!sb) return;
        sb.destroy();
        this._sandboxes.delete(extensionId);
    }

    unloadAll() {
        for (const sb of this._sandboxes.values()) sb.destroy();
        this._sandboxes.clear();
    }

    *entries() {
        yield* this._sandboxes.entries();
    }

    get size() {
        return this._sandboxes.size;
    }
}
