// NOTE for the srcdoc path: the sandbox host strips these imports and
// concatenates the imported files ahead of this one (see
// RUNTIME_SOURCE_PATHS in extension-sandbox-host.js) — keep them
// single-line and pointing at self-contained modules.
import { formatIcu } from '../shared/icu-message.js';
import { createSandboxedRunner } from './runtime/sandboxed-worker.js';

/**
 * Kage extension sandbox runtime.
 *
 * Executes inside a sandboxed iframe. Responsible for:
 *   - Receiving an INIT message from the host with the extension id,
 *     manifest, config, and list of provider source URLs to import.
 *   - Dynamically importing each provider module (the source is fetched
 *     as a string by the host and passed as a blob URL so we never go
 *     through the host's network stack from here).
 *   - Exposing the standard extension `context` object: a controlled
 *     `invoke` that round-trips through postMessage, an `log` function,
 *     and a (stable-across-config-updates) `config` value.
 *   - Dispatching RPC calls from the host to the loaded providers
 *     (match, matchAsync, execute, getTools, execute tool, getTriggers,
 *     onConfigUpdate, destroy).
 *
 * Design properties:
 *   - The iframe is sandboxed with a *null origin*, so it has no
 *     access to window.__TAURI__, the parent document, localStorage,
 *     cookies, or any other first-party global. The host's postMessage
 *     channel is the only IPC path.
 *   - The host validates the origin on every message it sends us and
 *     ignores messages from unknown ports. We do the symmetric thing.
 *   - `invoke` requests are tagged with an incrementing id so the host
 *     can reject them based on capabilities and pair the response.
 *   - Unhandled errors inside the sandbox are caught and reported
 *     back to the host rather than silently killing the iframe.
 */

// --- Host handshake ---------------------------------------------------------

/** @type {MessagePort | null} */
let hostPort = null;
/** @type {any} */
let extensionConfig = {};

/**
 * Per-extension i18n state. Populated from the host's `init` message:
 *   - `catalog`: the matched `_locales/<lang>/messages.json` payload, OR
 *     the EN fallback when the active language has no translation file.
 *   - `fallback`: the EN catalog. Used when a key is missing from `catalog`.
 *   - `language`: the resolved language code (after region-strip / fallback).
 *   - `rtl`: whether the active language is right-to-left.
 *
 * The runtime exposes `context.i18n.t(key, vars)` to extensions; the lookup
 * goes catalog → fallback → literal-key, mirroring host i18n semantics.
 */
let i18nState = { catalog: {}, fallback: {}, language: 'en', rtl: false };

/** Cache of vendor sources provided at init time. Used by runSandboxed()
 *  to inject allow-listed libraries into per-call Workers. */
let vendorSourcesCache = {};

/** RPC id → { resolve, reject } for outbound invoke() calls. */
const pendingInvokes = new Map();
let nextInvokeId = 1;

/** Provider instances loaded from the extension bundle. */
const providers = {
    searchProvider: null,
    toolProvider: null,
    triggerProvider: null,
    settingsProvider: null,
    toolbarProvider: null,
    messageFormatter: null,
    /** Widget instances, keyed by widget id (from manifest). */
    widgets: {},
};

function log(level, ...args) {
    const msg = args
        .map((a) => {
            if (typeof a === 'string') return a;
            try {
                return JSON.stringify(a);
            } catch {
                return String(a);
            }
        })
        .join(' ');
    safePost({ type: 'log', level, msg });
}

function safePost(payload) {
    if (!hostPort) return;
    try {
        hostPort.postMessage(payload);
    } catch (e) {
        // If the host side has gone away, we have nothing useful to do here.
        // Avoid recursing back into logging.
        console.error('[sandbox] failed to post to host:', e);
    }
}

// --- Context ---------------------------------------------------------------

/**
 * Build the `context` object handed to each provider's initialize().
 * The `invoke` here proxies to the host, which runs the authoritative
 * permission check before dispatching to Tauri.
 */
function buildContext() {
    const invoke = (command, args) => {
        if (typeof command !== 'string') {
            return Promise.reject(new Error('invoke() requires a string command name'));
        }
        const id = nextInvokeId++;
        return new Promise((resolve, reject) => {
            pendingInvokes.set(id, { resolve, reject });
            safePost({ type: 'invoke', id, command, args: args ?? {} });
        });
    };
    const extLog = {
        debug: (...a) => log('debug', ...a),
        info: (...a) => log('info', ...a),
        warn: (...a) => log('warn', ...a),
        error: (...a) => log('error', ...a),
    };
    // i18n proxy. Lives on `context.i18n.t(key, vars)`. Implementation is a
    // self-contained ICU MessageFormat subset (simple sub + plural + select)
    // so extensions don't need to load the host's i18n.js. It's identical in
    // behaviour to the host implementation and shares the test suite.
    const i18n = {
        t(key, vars) {
            const tpl =
                i18nState.catalog?.[key]?.message ?? i18nState.fallback?.[key]?.message ?? key;
            return formatIcu(tpl, vars || {}, i18nState.language || 'en');
        },
        language() {
            return i18nState.language || 'en';
        },
        isRtl() {
            return !!i18nState.rtl;
        },
    };
    return { invoke, config: extensionConfig, log: extLog, runSandboxed, i18n };
}

const runSandboxed = createSandboxedRunner({
    getVendorSources: () => vendorSourcesCache,
    log,
});

// --- Provider loading -------------------------------------------------------

/**
 * Dynamically import a provider module from source text. The host sends
 * us the pre-read JS text to avoid giving the sandboxed iframe any
 * network permission.
 *
 * Relative imports inside extension code (e.g. `import ... from './cache.js'`)
 * can't resolve from a blob URL base. To support them we pre-compute blob
 * URLs for every shared module and textually rewrite matching `import`
 * statements in the consumer source before building its blob URL.
 */

/** Map from relative-path string ("./cache.js") to blob URL, built at init time. */
const sharedModuleBlobs = new Map();

/**
 * Rewrite a source string so that `import ... from './xxx.js'` (and the
 * side-effect `import './xxx.js'`) refer to the blob URL assigned to
 * that relative path. Unknown relative imports are left alone — they'll
 * simply fail to resolve (and the failure surfaces through the provider
 * load error path).
 *
 * Exported as a pure function (taking the map explicitly) for unit
 * testing; the runtime path uses the module-level `sharedModuleBlobs`
 * instance via `rewriteRelativeImports`.
 */
export function rewriteRelativeImportsWith(source, blobMap) {
    if (!blobMap || blobMap.size === 0) return source;
    // Match both forms:
    //   import X from './x.js'     (named or default)
    //   import './x.js'            (side effect only)
    // The specifier is quoted with either ' or ".
    const pattern = /(\bimport\s+(?:[^'"]+?\s+from\s+)?)(['"])(\.{1,2}\/[^'"]+?)\2/g;
    return source.replace(pattern, (whole, leading, quote, specifier) => {
        const blob = blobMap.get(specifier);
        if (!blob) return whole;
        return `${leading}${quote}${blob}${quote}`;
    });
}

function rewriteRelativeImports(source) {
    return rewriteRelativeImportsWith(source, sharedModuleBlobs);
}

async function importFromSource(sourceText, kind) {
    // Rewrite relative imports first so the blob URL we build below
    // references sibling blobs via their registered URLs.
    const rewritten = rewriteRelativeImports(sourceText);
    // Build a blob URL we own so module specifier resolution stays local.
    const blob = new Blob([rewritten], { type: 'application/javascript' });
    const url = URL.createObjectURL(blob);
    // We deliberately DON'T revoke this URL after the import resolves.
    // Revoking while the engine still holds a reference to the module
    // (for dynamic source-map fetches, debugger stepping, or late child
    // import resolution in throttled iframes) races with Blink's same-
    // origin check and produces the "Unsafe attempt to load URL ..."
    // warning in the console. Blob URLs are cheap and die with the
    // sandbox iframe when it's torn down, so leaking them for the
    // lifetime of the extension is fine.
    const mod = await import(/* @vite-ignore */ url);
    if (!mod?.default) {
        throw new Error(`${kind}: module has no default export`);
    }
    return mod.default;
}

/**
 * Build blob URLs for every shared module BEFORE loading any provider.
 * Shared modules can import each other, so we can't just process them
 * in source order — `a.js` might import `./b.js` before we've seen
 * `b.js`. We do two passes:
 *   1. Build an initial blob URL for each path, populating the blob
 *      map so all specifiers are resolvable.
 *   2. For modules that use relative imports, rewrite them (now that
 *      every sibling has a known blob URL) and replace the pass-1 URL.
 * Modules without relative imports keep their pass-1 URL and are
 * never rebuilt.
 */
function registerSharedModules(sharedSources) {
    if (!sharedSources || typeof sharedSources !== 'object') return;
    const rawEntries = Object.entries(sharedSources);
    if (rawEntries.length === 0) return;

    // Pass 1: initial blob URLs with unmodified source.
    for (const [relPath, src] of rawEntries) {
        const blob = new Blob([src], { type: 'application/javascript' });
        sharedModuleBlobs.set(relPath, URL.createObjectURL(blob));
    }

    // Pass 2: for each module that imports siblings relatively, rebuild
    // the blob with rewritten specifiers. Revoke the pass-1 URL first
    // so we don't leak its backing Blob.
    for (const [relPath, src] of rawEntries) {
        if (!/import\s.*['"]\.{1,2}\//.test(src)) continue;
        const rewritten = rewriteRelativeImports(src);
        if (rewritten === src) continue;
        const oldUrl = sharedModuleBlobs.get(relPath);
        try {
            URL.revokeObjectURL(oldUrl);
        } catch {}
        const newBlob = new Blob([rewritten], { type: 'application/javascript' });
        sharedModuleBlobs.set(relPath, URL.createObjectURL(newBlob));
    }
}

async function initExtension(init) {
    extensionConfig = init.config || {};
    vendorSourcesCache = init.vendorSources || {};
    // i18n payload: catalog + fallback + active language + RTL flag. Each
    // is plain JSON; the host fetched the right `_locales/<lang>/messages.json`
    // before sending init. A missing payload (older host or extension with
    // no `_locales/`) collapses to literal-key rendering.
    i18nState = {
        catalog: init.i18nCatalog || {},
        fallback: init.i18nFallback || {},
        language: init.i18nLanguage || 'en',
        rtl: !!init.i18nRtl,
    };

    const context = buildContext();
    const sources = init.sources || {};

    // Shared modules (e.g. an extension's cache.js sibling) come in a
    // separate bag keyed by their relative path. We build blob URLs
    // for them first so provider-level imports can be rewritten to
    // point at those blobs.
    registerSharedModules(init.sharedSources || {});

    if (sources.searchProvider) {
        try {
            const Cls = await importFromSource(sources.searchProvider, 'searchProvider');
            providers.searchProvider = new Cls();
            providers.searchProvider.initialize?.(context);
        } catch (e) {
            log('error', `failed to load searchProvider: ${e?.message || e}`);
        }
    }

    if (sources.toolProvider) {
        try {
            const Cls = await importFromSource(sources.toolProvider, 'toolProvider');
            providers.toolProvider = new Cls();
            providers.toolProvider.initialize?.(context);
        } catch (e) {
            log('error', `failed to load toolProvider: ${e?.message || e}`);
        }
    }

    if (sources.triggerProvider) {
        try {
            const Cls = await importFromSource(sources.triggerProvider, 'triggerProvider');
            providers.triggerProvider = new Cls();
            providers.triggerProvider.initialize?.(context);
        } catch (e) {
            log('error', `failed to load triggerProvider: ${e?.message || e}`);
        }
    }

    if (sources.settingsProvider) {
        try {
            const Cls = await importFromSource(sources.settingsProvider, 'settingsProvider');
            providers.settingsProvider = new Cls();
            providers.settingsProvider.initialize?.(context);
        } catch (e) {
            log('error', `failed to load settingsProvider: ${e?.message || e}`);
        }
    }

    if (sources.toolbarProvider) {
        try {
            const Cls = await importFromSource(sources.toolbarProvider, 'toolbarProvider');
            providers.toolbarProvider = new Cls();
            providers.toolbarProvider.initialize?.(context);
        } catch (e) {
            log('error', `failed to load toolbarProvider: ${e?.message || e}`);
        }
    }

    if (sources.messageFormatter) {
        try {
            const Cls = await importFromSource(sources.messageFormatter, 'messageFormatter');
            providers.messageFormatter = new Cls();
            providers.messageFormatter.initialize?.(context);
        } catch (e) {
            log('error', `failed to load messageFormatter: ${e?.message || e}`);
        }
    }

    // Extensions can opt into being called during streaming by declaring
    // `formatDuringStreaming: true` as a class static. The default is
    // false because every streaming chunk would round-trip through the
    // bridge, which is expensive for long assistant messages.
    const formatterOptsIn = !!(
        providers.messageFormatter &&
        (providers.messageFormatter.formatDuringStreaming ||
            providers.messageFormatter.constructor?.formatDuringStreaming)
    );

    if (sources.widgets && typeof sources.widgets === 'object') {
        for (const [widgetId, src] of Object.entries(sources.widgets)) {
            try {
                const Cls = await importFromSource(src, `widget '${widgetId}'`);
                const instance = new Cls();
                instance.initialize?.(context);
                providers.widgets[widgetId] = instance;
            } catch (e) {
                log('error', `failed to load widget '${widgetId}': ${e?.message || e}`);
            }
        }
    }

    return {
        hasSearch: !!providers.searchProvider,
        hasTools: !!providers.toolProvider,
        hasTriggers: !!providers.triggerProvider,
        hasSettings: !!providers.settingsProvider,
        hasToolbar: !!providers.toolbarProvider,
        hasFormatter: !!providers.messageFormatter,
        formatterOptsInStreaming: formatterOptsIn,
        widgetIds: Object.keys(providers.widgets),
    };
}

// --- RPC dispatch -----------------------------------------------------------

async function handleRpc(msg) {
    const { method, params, rpcId } = msg;
    try {
        const result = await dispatchMethod(method, params);
        safePost({ type: 'rpc-response', rpcId, result });
    } catch (e) {
        safePost({ type: 'rpc-response', rpcId, error: String(e?.message || e) });
    }
}

async function dispatchMethod(method, params) {
    switch (method) {
        case 'match': {
            const p = providers.searchProvider;
            if (!p || typeof p.match !== 'function') return [];
            return p.match(params?.query ?? '') || [];
        }
        case 'matchAsync': {
            const p = providers.searchProvider;
            if (!p || typeof p.matchAsync !== 'function') return [];
            return (await p.matchAsync(params?.query ?? '')) || [];
        }
        case 'execute': {
            const p = providers.searchProvider;
            if (!p || typeof p.execute !== 'function') return null;
            return p.execute(params?.result) ?? null;
        }
        case 'getKeywords': {
            // Optional. Lets a search provider register its trigger words
            // with the host so partial input (e.g. "cal-ref") can surface a
            // completion hint before the full keyword is typed. Runs in the
            // sandbox so the list can reflect this.config (a user-customised
            // trigger). Labels are i18n KEYS, resolved host-side against the
            // extension's catalog — never raw text.
            const p = providers.searchProvider;
            if (!p || typeof p.getKeywords !== 'function') return [];
            return p.getKeywords() || [];
        }
        case 'getTools': {
            const p = providers.toolProvider;
            if (!p || typeof p.getTools !== 'function') return [];
            return p.getTools() || [];
        }
        case 'executeTool': {
            const p = providers.toolProvider;
            if (!p || typeof p.execute !== 'function') return { error: 'no tool provider' };
            return p.execute(params?.toolName, params?.params || {});
        }
        case 'getToolTimeout': {
            const p = providers.toolProvider;
            if (!p || typeof p.getToolTimeout !== 'function') return null;
            return p.getToolTimeout(params?.toolName) ?? null;
        }
        case 'getTriggers': {
            const p = providers.triggerProvider;
            if (!p || typeof p.getTriggers !== 'function') return [];
            return p.getTriggers() || [];
        }
        case 'getSettings': {
            const p = providers.settingsProvider;
            if (!p || typeof p.getSettings !== 'function') {
                return { sections: [] };
            }
            return p.getSettings() || { sections: [] };
        }
        case 'validateSettings': {
            const p = providers.settingsProvider;
            if (!p || typeof p.validate !== 'function') {
                return { valid: true };
            }
            const out = p.validate(params?.values || {});
            return out && typeof out === 'object' ? out : { valid: true };
        }
        case 'normalizeSettings': {
            const p = providers.settingsProvider;
            if (!p || typeof p.normalize !== 'function') {
                return {};
            }
            const out = p.normalize(params?.values || {});
            // Expected shape: { values: {...canonicalized...} }
            if (out && typeof out === 'object' && out.values && typeof out.values === 'object') {
                return { values: out.values };
            }
            return {};
        }
        case 'runSettingsAction': {
            const p = providers.settingsProvider;
            if (!p || typeof p.runAction !== 'function') {
                return { error: 'no settings provider (or runAction not implemented)' };
            }
            const result = await p.runAction(params?.action || '', params?.values || {});
            return result && typeof result === 'object' ? result : {};
        }
        case 'onFileSelected': {
            const p = providers.settingsProvider;
            if (!p || typeof p.onFileSelected !== 'function') {
                return {};
            }
            const result = await p.onFileSelected(params || {});
            return result && typeof result === 'object' ? result : {};
        }
        case 'renderCustom': {
            const p = providers.searchProvider;
            if (!p || typeof p.renderCustom !== 'function') return null;
            const out = p.renderCustom(params?.result);
            // Expected shape: { html, className? } or null to skip.
            if (!out || typeof out !== 'object') return null;
            if (typeof out.html !== 'string') return null;
            return {
                html: out.html,
                className: typeof out.className === 'string' ? out.className : '',
            };
        }
        case 'onResultAction': {
            const p = providers.searchProvider;
            if (!p || typeof p.onResultAction !== 'function') return {};
            const out = await p.onResultAction(params?.actionId || '', {
                resultId: params?.resultId ?? null,
            });
            return out && typeof out === 'object' ? out : {};
        }
        case 'getToolbarButtons': {
            const p = providers.toolbarProvider;
            if (!p || typeof p.getButtons !== 'function') return [];
            const defs = p.getButtons() || [];
            // Pass through only the declarative fields. Any function
            // references would be unserializable anyway, but we guard
            // to make the contract explicit.
            return defs
                .map((d) => ({
                    id: String(d?.id || ''),
                    icon: String(d?.icon || ''),
                    tooltip: String(d?.tooltip || ''),
                }))
                .filter((d) => d.id);
        }
        case 'onToolbarClick': {
            const p = providers.toolbarProvider;
            if (!p || typeof p.onClick !== 'function') return {};
            const result = await p.onClick(params?.buttonId || '', params?.context || {});
            return result && typeof result === 'object' ? result : {};
        }
        case 'formatMessage': {
            const p = providers.messageFormatter;
            if (!p || typeof p.format !== 'function') return null;
            const out = await p.format(String(params?.html || ''), params?.context || {});
            if (typeof out !== 'string') return null;
            return { html: out };
        }
        case 'renderWidget': {
            const w = providers.widgets[params?.widgetId];
            if (!w || typeof w.render !== 'function') return null;
            const out = await w.render();
            if (!out || typeof out !== 'object') return null;
            // Shape: { html, className?, actions? }. Actions are declared
            // buttons the host will wire to onWidgetAction.
            const actions = Array.isArray(out.actions)
                ? out.actions
                      .map((a) => ({
                          id: String(a?.id || ''),
                          rpc: String(a?.rpc || a?.id || ''),
                      }))
                      .filter((a) => a.id)
                : [];
            return {
                html: typeof out.html === 'string' ? out.html : '',
                className: typeof out.className === 'string' ? out.className : '',
                actions,
            };
        }
        case 'getWidgetRefreshInterval': {
            const w = providers.widgets[params?.widgetId];
            if (!w || typeof w.getRefreshInterval !== 'function') return 0;
            const n = Number(w.getRefreshInterval());
            return Number.isFinite(n) && n >= 0 ? n : 0;
        }
        case 'onWidgetAction': {
            const w = providers.widgets[params?.widgetId];
            if (!w || typeof w.onAction !== 'function') return {};
            const out = await w.onAction(params?.actionId || '', params?.context || {});
            return out && typeof out === 'object' ? out : {};
        }
        case 'onConfigUpdate': {
            extensionConfig = params?.config || {};
            for (const p of [
                providers.searchProvider,
                providers.toolProvider,
                providers.triggerProvider,
                providers.settingsProvider,
                providers.toolbarProvider,
                providers.messageFormatter,
            ]) {
                if (p?.onConfigUpdate) {
                    try {
                        p.onConfigUpdate(extensionConfig);
                    } catch (e) {
                        log('warn', `onConfigUpdate failed: ${e?.message || e}`);
                    }
                }
            }
            for (const w of Object.values(providers.widgets)) {
                if (w?.onConfigUpdate) {
                    try {
                        w.onConfigUpdate(extensionConfig);
                    } catch (e) {
                        log('warn', `widget onConfigUpdate failed: ${e?.message || e}`);
                    }
                }
            }
            return true;
        }
        case 'destroy': {
            const all = [
                providers.searchProvider,
                providers.toolProvider,
                providers.triggerProvider,
                providers.settingsProvider,
                providers.toolbarProvider,
                providers.messageFormatter,
                ...Object.values(providers.widgets),
            ];
            for (const p of all) {
                try {
                    p?.destroy?.();
                } catch {}
            }
            return true;
        }
        default:
            throw new Error(`unknown method '${method}'`);
    }
}

// --- Message handling -------------------------------------------------------

function handleHostMessage(ev) {
    const msg = ev.data;
    if (!msg || typeof msg !== 'object') return;

    switch (msg.type) {
        case 'init': {
            initExtension(msg)
                .then((result) => safePost({ type: 'init-ack', ok: true, result }))
                .catch((e) =>
                    safePost({ type: 'init-ack', ok: false, error: String(e?.message || e) })
                );
            break;
        }
        case 'rpc':
            handleRpc(msg);
            break;
        case 'invoke-response': {
            const entry = pendingInvokes.get(msg.id);
            if (!entry) return;
            pendingInvokes.delete(msg.id);
            if (msg.error) entry.reject(new Error(msg.error));
            else entry.resolve(msg.result);
            break;
        }
        default:
            // Unknown messages are ignored — the host may be newer than us.
            break;
    }
}

// The host transfers a MessagePort in the very first message it sends to
// our window. From that point on, all traffic is over the port.
window.addEventListener('message', function onHandshake(ev) {
    // We don't check ev.origin here because sandboxed iframes have a
    // null origin and the host sends via postMessage(..., '*'); the
    // meaningful check is that we only accept the port that arrives with
    // the handshake message. Once bound, we only listen on hostPort.
    if (!ev.data || ev.data.type !== 'handshake') return;
    if (!ev.ports || ev.ports.length === 0) return;

    hostPort = ev.ports[0];
    hostPort.onmessage = handleHostMessage;
    hostPort.onmessageerror = (e) => log('error', `messageerror: ${String(e)}`);
    hostPort.start();

    // Once the port is set, the window channel is no longer used. Remove
    // the listener so we can't be confused by stray messages from any
    // other frame.
    window.removeEventListener('message', onHandshake);

    safePost({ type: 'ready' });
});

// Surface unhandled errors to the host instead of letting them vanish.
window.addEventListener('error', (e) => {
    log('error', `uncaught: ${e?.message || String(e)} at ${e?.filename}:${e?.lineno}`);
});
window.addEventListener('unhandledrejection', (e) => {
    log('error', `unhandledrejection: ${String(e?.reason?.message || e?.reason || e)}`);
});
