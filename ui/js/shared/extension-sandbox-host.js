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

const BOOT_TIMEOUT_MS = 10_000;
const RPC_TIMEOUT_MS = 10_000;

// Per-extension invoke rate limit. A misbehaving extension calling invoke()
// in a tight loop (open_url, search_files, get_calendar_events — several of
// which spawn OS work host-side) could otherwise pile up unbounded host work
// and tank the machine. We allow a generous burst — real extensions fire a
// handful of invokes per user action, never sustained hundreds/sec — and
// reject the excess so the extension gets immediate backpressure instead of
// the host melting down. Sliding 1s window, counted per ExtensionSandbox
// instance (i.e. per extension).
const INVOKE_RATE_WINDOW_MS = 1_000;
const INVOKE_RATE_MAX_PER_WINDOW = 100;

/**
 * Cached sandbox runtime source. Fetched once on first pool load and
 * reused for all subsequent sandbox iframes via srcdoc. This avoids
 * the CORS issue where a sandboxed iframe (null origin) can't load
 * ES modules from the Tauri custom-protocol origin in production builds.
 * @type {string|null}
 */
let _runtimeSourceCache = null;

/**
 * Fetch the sandbox runtime source once. Returns the JS text that will
 * be inlined into each sandbox iframe's srcdoc as a classic script.
 */
async function getSandboxRuntimeSource() {
    if (_runtimeSourceCache !== null) return _runtimeSourceCache;
    const resp = await fetch('js/extension-sandbox/runtime.js');
    if (!resp.ok) throw new Error(`Failed to fetch sandbox runtime: HTTP ${resp.status}`);
    let source = await resp.text();
    // The runtime has one `export` (for unit tests). Strip it so the
    // source works as a classic (non-module) script inside srcdoc.
    source = source.replace(/^export\s+function\s/m, 'function ');
    _runtimeSourceCache = source;
    return source;
}

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
        // i18n payload — host-resolved before sandbox start.
        // The host calls `read_extension_locale(id, kind, lang)` for the
        // active locale and EN, then hands the resulting JSON over here.
        this.i18nCatalog = spec.i18nCatalog || {};
        this.i18nFallback = spec.i18nFallback || {};
        this.i18nLanguage = spec.i18nLanguage || 'en';
        this.i18nRtl = !!spec.i18nRtl;
        this._rawInvoke = rawInvoke;
        this._container = container;

        this._iframe = null;
        this._port = null;
        this._ready = false;
        /** @type {Map<number, {resolve:Function, reject:Function, timer:any}>} */
        this._pendingRpcs = new Map();
        this._nextRpcId = 1;
        this._destroyed = false;

        // Sliding-window invoke rate limiter (see INVOKE_RATE_* constants).
        // Timestamps of invokes within the current window; pruned on each
        // call. _rateLimitedLogged throttles the warning so a runaway loop
        // doesn't itself flood the log.
        this._invokeTimes = [];
        this._rateLimitedLogged = false;

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

        // Fetch the runtime source (cached after first call).
        const runtimeSource = await getSandboxRuntimeSource();

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
        // Use srcdoc with the runtime inlined as a classic script. This
        // avoids the CORS issue where a null-origin sandboxed iframe
        // can't load ES modules from the Tauri custom-protocol origin
        // in production builds. The runtime uses dynamic import() for
        // extension code (via blob URLs, which are same-origin to the
        // iframe), so module loading still works for extensions.
        iframe.srcdoc = `<!DOCTYPE html><html><head><meta charset="UTF-8"></head><body><script>${runtimeSource.replace(/<\/script/gi, '<\x2fscript')}</script></body></html>`;

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
                // i18n payload — host already fetched the matched
                // `_locales/<lang>/messages.json` for the active language
                // and an EN fallback. The runtime exposes them via
                // `context.i18n.t(key, vars)` to the extension.
                i18nCatalog: this.i18nCatalog || {},
                i18nFallback: this.i18nFallback || {},
                i18nLanguage: this.i18nLanguage || 'en',
                i18nRtl: !!this.i18nRtl,
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
     *
     * Times out after RPC_TIMEOUT_MS by default so a hung extension cannot
     * wedge us. Callers can pass `opts.timeoutMs` to override — necessary for
     * RPCs that legitimately block on the *user* rather than on work, e.g. a
     * settings action that runs an OAuth flow (the user has to consent in a
     * browser tab, which routinely takes longer than 10s). Pass `0` to
     * disable the timeout entirely.
     */
    async call(method, params, opts = {}) {
        if (this._destroyed) throw new Error(`sandbox '${this.extensionId}' is destroyed`);
        if (!this._ready) throw new Error(`sandbox '${this.extensionId}' not ready`);
        const timeoutMs = Number.isFinite(opts.timeoutMs) ? opts.timeoutMs : RPC_TIMEOUT_MS;
        const rpcId = this._nextRpcId++;
        return new Promise((resolve, reject) => {
            // timeoutMs <= 0 means "no timeout" — used for user-gated RPCs.
            const timer =
                timeoutMs > 0
                    ? setTimeout(() => {
                          this._pendingRpcs.delete(rpcId);
                          reject(new Error(`RPC '${method}' timed out after ${timeoutMs}ms`));
                      }, timeoutMs)
                    : null;
            this._pendingRpcs.set(rpcId, { resolve, reject, timer });
            try {
                this._port.postMessage({ type: 'rpc', rpcId, method, params });
            } catch (e) {
                if (timer) clearTimeout(timer);
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

    /**
     * Sliding-window rate check. Returns true if this invoke would exceed
     * the per-extension budget (and should be rejected). On an allowed call
     * it records the timestamp; rejected calls are NOT recorded, so an
     * extension that backs off recovers immediately once the window drains.
     */
    _overInvokeRateLimit() {
        const now = Date.now();
        const cutoff = now - INVOKE_RATE_WINDOW_MS;
        // Drop timestamps older than the window. The array stays small
        // (bounded by the cap) because we reject once full.
        while (this._invokeTimes.length && this._invokeTimes[0] <= cutoff) {
            this._invokeTimes.shift();
        }
        if (this._invokeTimes.length >= INVOKE_RATE_MAX_PER_WINDOW) {
            return true;
        }
        this._invokeTimes.push(now);
        // Reset the log-throttle once the extension is behaving again.
        if (this._rateLimitedLogged && this._invokeTimes.length < INVOKE_RATE_MAX_PER_WINDOW / 2) {
            this._rateLimitedLogged = false;
        }
        return false;
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

        // Per-command argument validation that's tighter than the
        // capability gate. Lives here rather than in the Rust command
        // because the same command can be called from host UI with a
        // wider set of legal arguments — only extension-originated calls
        // are constrained. Returning an error here is exactly equivalent
        // to a capability denial from the extension's perspective.
        const argError = validateArgsForExtension(command, args);
        if (argError) {
            this._port.postMessage({
                type: 'invoke-response',
                id,
                error: `Extension '${this.extensionId}': ${argError}`,
            });
            console.warn(`[sandbox ${this.extensionId}] BLOCKED invoke('${command}'): ${argError}`);
            return;
        }

        // Rate limit AFTER the capability/arg gates (a denied call shouldn't
        // count against the budget) but BEFORE dispatch (so a runaway loop
        // never reaches the host command). Reject excess with an error the
        // extension sees on its invoke() promise.
        if (this._overInvokeRateLimit()) {
            this._port.postMessage({
                type: 'invoke-response',
                id,
                error: `Extension '${this.extensionId}': invoke rate limit exceeded (max ${INVOKE_RATE_MAX_PER_WINDOW}/s)`,
            });
            if (!this._rateLimitedLogged) {
                this._rateLimitedLogged = true;
                console.warn(
                    `[sandbox ${this.extensionId}] invoke rate limit exceeded (>${INVOKE_RATE_MAX_PER_WINDOW}/s) — throttling further warnings`
                );
            }
            return;
        }

        // For storage commands, force-inject the extension's identity. The
        // host owns the sandbox -> extension mapping (it created the
        // iframe), so this is authoritative. Any extensionId the sandbox
        // tried to supply is overwritten — sandboxes can't read or write
        // another extension's data even if they know the key.
        //
        // Tauri 2 expects camelCase arg names on the IPC boundary
        // (`save_extension_data(extension_id: String)` in Rust ⇄
        // `{ extensionId }` in JS) — our previous snake_case
        // injection was silently failing the auto-rename: the
        // command rejected the call with "missing required key
        // extensionId" and the spotify widget's now-playing fetch
        // came up empty, with the install never persisting because
        // commit_extension_install couldn't write its grant payload.
        const forwardedArgs = STORAGE_COMMANDS.has(command)
            ? { ...(args || {}), extensionId: this.extensionId }
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
 * Schemes that the `urls` capability is allowed to hand to the OS via
 * `open_url`. Anything outside this list — including custom app URI
 * schemes like `spotify://`, `vscode://`, `slack://` — must use the
 * `launch` capability instead.
 *
 * Why each entry is on the list:
 *   - http/https: web pages.
 *   - mailto/tel/sms/facetime/imessage: comms — every OS pre-registers
 *     handlers and they don't accept arbitrary code paths.
 *   - x-apple.systempreferences / ms-settings / prefs: deep links into
 *     the user's own OS settings panes (e.g. macOS calendar privacy).
 *     They cannot navigate to arbitrary apps.
 *
 * Naked scheme strings, no trailing colon. Comparison is
 * case-insensitive (RFC 3986 §3.1).
 */
const URLS_CAP_ALLOWED_SCHEMES = Object.freeze([
    'http',
    'https',
    'mailto',
    'tel',
    'sms',
    'facetime',
    'facetime-audio',
    'imessage',
    'x-apple.systempreferences',
    'ms-settings',
    'prefs', // iOS-style; macOS occasionally accepts via reverse-dns variants
]);

function extractScheme(url) {
    if (typeof url !== 'string') return null;
    // Trim leading whitespace; URI parsers tolerate it but our own
    // matching shouldn't depend on call-site cleanliness.
    const trimmed = url.replace(/^\s+/, '');
    // Common typo we already auto-fix in open_url's Rust side: bare
    // "www.foo.com". Pretend that's https for validation purposes; the
    // Rust side will rewrite to https://.
    if (/^www\./i.test(trimmed)) return 'https';
    const m = trimmed.match(/^([a-zA-Z][a-zA-Z0-9+.-]*):/);
    return m ? m[1].toLowerCase() : null;
}

/**
 * Per-command argument validation for extension-originated calls.
 * Returns null if the args are acceptable, or a human-readable string
 * describing why they aren't (which the host surfaces back to the
 * extension as the invoke error).
 *
 * Today this validates the URL scheme passed to `open_url` so the
 * `urls` capability can't be used to launch arbitrary apps via
 * `file:///` or custom URI schemes (`spotify://`, `vscode://`, etc.).
 * Add more entries as new commands need extension-specific argument
 * narrowing.
 */
function validateArgsForExtension(command, args) {
    if (command === 'open_url') {
        const url = args?.url;
        if (typeof url !== 'string' || !url.trim()) {
            return "open_url called with no 'url' argument.";
        }
        const scheme = extractScheme(url);
        if (!scheme) {
            return `open_url rejected: '${url}' has no scheme. Use http(s), mailto, tel, or an OS-settings deep link.`;
        }
        if (!URLS_CAP_ALLOWED_SCHEMES.includes(scheme)) {
            return `open_url rejected: scheme '${scheme}:' is not allowed for the 'urls' capability. Custom app URI schemes need the 'launch' capability.`;
        }
    }
    return null;
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
