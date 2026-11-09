// NOTE for the srcdoc path: the sandbox host strips these imports and
// concatenates the imported files ahead of this one (see
// RUNTIME_SOURCE_PATHS in extension-sandbox-host.js) — keep them
// single-line and pointing at self-contained modules.
import { formatIcu } from '../shared/icu-message.js';
import { createSandboxedRunner } from './runtime/sandboxed-worker.js';
import { HOST_FEATURES, createStorageEvents, invokeError } from './runtime/host-api.js';
import { createViewSurface, removeWebRtc } from './runtime/view-api.js';
import { extensionModuleOrder, rewriteExtensionModules } from '../shared/extension-modules.js';

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
 *   - On the view surface (an extension view window, see
 *     docs/design/EXTENSION_VIEWS.md §8–§9): loading only the view module,
 *     `context.view`, and the viewMount / viewReopen / viewVisibility /
 *     viewTheme / viewPing RPCs (runtime/view-api.js).
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

/**
 * Host UI features this window offers the extension (e.g. `'date-picker'`),
 * as a frozen array on `context.hostFeatures`. The host already filtered it
 * by capability; extensions use it purely to feature-detect, so an older
 * host (which sends nothing) leaves `context.hostFeatures` undefined.
 */
let hostFeatures;

/** RPC id → { resolve, reject } for outbound invoke() calls. */
const pendingInvokes = new Map();
let nextInvokeId = 1;

/**
 * Which surface this sandbox runs on: `'hidden'` (the 1px iframe in the
 * launcher, chat and settings windows) or `'view'` (the visible iframe of
 * an extension view window). Set by the host's `init`.
 */
let surface = 'hidden';

/** The view surface (runtime/view-api.js), on the view surface only. */
let viewSurface = null;

/** `context.storage`: change events for every surface (§10). */
const storageEvents = createStorageEvents({ log });

/** Provider instances loaded from the extension bundle. */
const providers = {
    searchProvider: null,
    toolProvider: null,
    triggerProvider: null,
    settingsProvider: null,
    toolbarProvider: null,
    messageFormatter: null,
    /** The view module, on the view surface only. */
    view: null,
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
function invoke(command, args) {
    if (typeof command !== 'string') {
        return Promise.reject(new Error('invoke() requires a string command name'));
    }
    const id = nextInvokeId++;
    return new Promise((resolve, reject) => {
        pendingInvokes.set(id, { resolve, reject });
        safePost({ type: 'invoke', id, command, args: args ?? {} });
    });
}

function buildContext() {
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
    const context = {
        invoke,
        config: extensionConfig,
        log: extLog,
        runSandboxed,
        i18n,
        mascot,
        formatter,
        // Change events for this extension's stored data (every surface).
        storage: storageEvents.api,
        // Feature detection for optional host behaviour: what this runtime
        // always supports (host-api.js) plus what the host granted this
        // extension at init (e.g. 'date-picker', capability-filtered).
        hostFeatures: Object.freeze([...new Set([...HOST_FEATURES, ...(hostFeatures || [])])]),
    };
    // Only a view window's sandbox has `context.view`; elsewhere it's
    // undefined, so extensions can feature-detect it.
    if (viewSurface) context.view = viewSurface.api;
    return context;
}

/**
 * `context.formatter` — lets a message formatter that returned placeholder
 * content (e.g. loading cards) ask the host to run it again once its data
 * has arrived. Like `context.mascot` it carries no data and never reaches
 * Tauri, so it needs no capability. The host coalesces bursts and re-runs
 * the formatter on the messages it already formatted, passing their
 * current HTML, so formatters that use this must be idempotent.
 */
const formatter = {
    requestReformat() {
        safePost({ type: 'reformat-request' });
    },
};

/**
 * Closed vocabulary of mascot activity names. The mascot deliberately knows
 * nothing about individual extensions — an extension says "music", never
 * "spotify" — so the engine can map a generic activity to whatever pose set
 * it has. Anything outside this list is dropped (with a warning) the same way
 * `normalizePermissions` drops unknown capabilities.
 *
 * Exported so the host-side validator and the tests can share one list.
 */
export const MASCOT_ACTIVITIES = ['music', 'meeting', 'timer'];

/**
 * `context.mascot` — a pure-UI hint channel. It carries no data, has a closed
 * vocabulary, and never reaches Tauri, so it needs no capability. The host
 * re-validates everything here (vocabulary + the manifest's declared
 * `contributes.mascotActivities` + the user's mascot-hint setting), which is
 * why these checks are only a fast local reject, not the security boundary.
 *
 * The hint is a LEASE: the host clamps `ttlMs` into a bounded range and the
 * mascot drops the activity when it expires. Re-assert from your refresh /
 * render cycle so a crashed, disabled or hidden extension can never pin a
 * pose.
 */
const mascot = {
    setActivity(name, opts) {
        if (typeof name !== 'string' || !MASCOT_ACTIVITIES.includes(name)) {
            log('warn', `mascot.setActivity: unknown activity '${String(name)}' — ignored`);
            return;
        }
        const payload = { type: 'mascot-activity', activity: name };
        const ttl = Number(opts?.ttlMs);
        if (Number.isFinite(ttl) && ttl > 0) payload.ttlMs = ttl;
        safePost(payload);
    },
    clearActivity() {
        safePost({ type: 'mascot-activity', activity: null });
    },
};

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
/** Failed bundle paths remain available for precise provider-level diagnostics. */
const sharedModuleFailures = new Map();

/**
 * Rewrite imports and re-exports to the final blob URL for the bundle path,
 * relative to the importing file. Runtime callers supply the failure map to
 * reject missing or failed dependencies before importing the provider.
 *
 * Exported as a pure function (taking the map explicitly) for unit
 * testing; the runtime path uses the module-level `sharedModuleBlobs`
 * instance.
 */
export function rewriteRelativeImportsWith(
    source,
    blobMap,
    importer = './',
    failures,
    deferredDynamic
) {
    if (!failures && (!blobMap || blobMap.size === 0)) return source;
    return rewriteExtensionModules(source, blobMap, importer, failures, deferredDynamic);
}

export async function importFromSource(
    sourceText,
    kind,
    importer = kind,
    blobMap = sharedModuleBlobs,
    failures = sharedModuleFailures
) {
    try {
        // Rewrite relative imports first so the blob URL we build below
        // references sibling blobs via their registered URLs.
        const rewritten = rewriteRelativeImportsWith(sourceText, blobMap, importer, failures);
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
    } catch (e) {
        throw new Error(`${kind} '${importer}': ${e?.message || e}`);
    }
}

/**
 * Build each healthy blob once, in dependency order. Failed modules affect only
 * their dependents. Keep final URLs alive for the sandbox lifetime, including late
 * quoted dynamic imports; the iframe's teardown releases them.
 */
let dynamicRegistrySequence = 0;

export function registerSharedModules(
    sharedSources,
    blobMap = sharedModuleBlobs,
    failures = blobMap === sharedModuleBlobs ? sharedModuleFailures : new Map()
) {
    const order = extensionModuleOrder(sharedSources, failures);
    let registryKey;
    const deferredDynamic = (path) => {
        if (!Object.hasOwn(sharedSources || {}, path)) return null;
        if (!registryKey) {
            // A dynamic back edge can refer to a blob built later. Resolve it
            // when import() executes, after all final URLs are registered.
            registryKey = `__kageDynamicModules${++dynamicRegistrySequence}`;
            globalThis[registryKey] = blobMap;
        }
        return `globalThis[${JSON.stringify(registryKey)}].get(${JSON.stringify(path)})`;
    };
    for (const path of order) {
        try {
            const rewritten = rewriteRelativeImportsWith(
                sharedSources[path],
                blobMap,
                path,
                failures,
                deferredDynamic
            );
            blobMap.set(
                path,
                URL.createObjectURL(new Blob([rewritten], { type: 'application/javascript' }))
            );
        } catch (e) {
            failures.set(path, new Error(`Extension module '${path}': ${e?.message || e}`));
        }
    }
    // Deferred references to a target that subsequently failed must also
    // reject with its recorded error, rather than import(undefined).
    if (registryKey) {
        const urls = new Map(blobMap);
        for (const [path, error] of failures) {
            urls.set(
                path,
                URL.createObjectURL(
                    new Blob([`throw new Error(${JSON.stringify(error.message)});`], {
                        type: 'application/javascript',
                    })
                )
            );
        }
        globalThis[registryKey] = urls;
    }
    return blobMap;
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
    hostFeatures = Array.isArray(init.hostFeatures)
        ? Object.freeze(init.hostFeatures.filter((f) => typeof f === 'string'))
        : undefined;

    surface = init.surface === 'view' ? 'view' : 'hidden';
    if (surface === 'view') {
        // Before any extension code runs: CSP doesn't cover WebRTC (§8.4).
        removeWebRtc(window);
        const view = init.view && typeof init.view === 'object' ? init.view : {};
        viewSurface = createViewSurface({
            viewId: typeof view.id === 'string' ? view.id : '',
            theme: view.theme,
            links: view.links === true,
            post: (msg) => {
                if (!hostPort) throw new Error('the host is gone');
                hostPort.postMessage(msg);
            },
            invoke,
            log,
            toError: invokeError,
            win: window,
            doc: document,
        });
        viewSurface.applyStyles(view.css);
        viewSurface.installGuards();
    }

    const context = buildContext();
    const sources = init.sources || {};
    const sourcePaths = sources.sourcePaths || {};

    // Shared modules (e.g. an extension's cache.js sibling) come in a
    // separate bag keyed by their relative path. We build blob URLs
    // for them first so provider-level imports can be rewritten to
    // point at those blobs.
    registerSharedModules(init.sharedSources || {});

    if (surface === 'view') {
        // A view window loads only its view module and what it imports.
        if (sources.view) {
            try {
                const Cls = await importFromSource(sources.view, 'view', sourcePaths.view);
                providers.view = new Cls();
                viewSurface.attach(providers.view);
                providers.view.initialize?.(context);
            } catch (e) {
                log('error', `failed to load view: ${e?.message || e}`);
            }
        }
        return {
            hasSearch: false,
            hasTools: false,
            hasTriggers: false,
            hasSettings: false,
            hasToolbar: false,
            hasFormatter: false,
            hasDateProvider: false,
            hasView: !!providers.view,
            formatterOptsInStreaming: false,
            widgetIds: [],
        };
    }

    if (sources.searchProvider) {
        try {
            const Cls = await importFromSource(
                sources.searchProvider,
                'searchProvider',
                sourcePaths.searchProvider
            );
            providers.searchProvider = new Cls();
            providers.searchProvider.initialize?.(context);
        } catch (e) {
            log('error', `failed to load searchProvider: ${e?.message || e}`);
        }
    }

    if (sources.toolProvider) {
        try {
            const Cls = await importFromSource(
                sources.toolProvider,
                'toolProvider',
                sourcePaths.toolProvider
            );
            providers.toolProvider = new Cls();
            providers.toolProvider.initialize?.(context);
        } catch (e) {
            log('error', `failed to load toolProvider: ${e?.message || e}`);
        }
    }

    if (sources.triggerProvider) {
        try {
            const Cls = await importFromSource(
                sources.triggerProvider,
                'triggerProvider',
                sourcePaths.triggerProvider
            );
            providers.triggerProvider = new Cls();
            providers.triggerProvider.initialize?.(context);
        } catch (e) {
            log('error', `failed to load triggerProvider: ${e?.message || e}`);
        }
    }

    if (sources.settingsProvider) {
        try {
            const Cls = await importFromSource(
                sources.settingsProvider,
                'settingsProvider',
                sourcePaths.settingsProvider
            );
            providers.settingsProvider = new Cls();
            providers.settingsProvider.initialize?.(context);
        } catch (e) {
            log('error', `failed to load settingsProvider: ${e?.message || e}`);
        }
    }

    if (sources.toolbarProvider) {
        try {
            const Cls = await importFromSource(
                sources.toolbarProvider,
                'toolbarProvider',
                sourcePaths.toolbarProvider
            );
            providers.toolbarProvider = new Cls();
            providers.toolbarProvider.initialize?.(context);
        } catch (e) {
            log('error', `failed to load toolbarProvider: ${e?.message || e}`);
        }
    }

    if (sources.messageFormatter) {
        try {
            const Cls = await importFromSource(
                sources.messageFormatter,
                'messageFormatter',
                sourcePaths.messageFormatter
            );
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
                const Cls = await importFromSource(
                    src,
                    `widget '${widgetId}'`,
                    sourcePaths.widgets?.[widgetId]
                );
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
        // Date-picker routing (see `onDateSelected` below). The host ANDs
        // this with whether it offered the feature at all.
        hasDateProvider: typeof providers.searchProvider?.onDateSelected === 'function',
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

/** The view surface, or the same error as an unknown method elsewhere. */
function requireViewSurface(method) {
    if (!viewSurface) throw new Error(`unknown method '${method}'`);
    return viewSurface;
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
        case 'getDateMarkers': {
            // Optional. Busy-day dots for the host's date picker: which days
            // in [start, end) have events. The host validates the shape and
            // renders only dates, counts and colours — never extension text.
            const p = providers.searchProvider;
            if (!p || typeof p.getDateMarkers !== 'function') return null;
            const out = await p.getDateMarkers(
                String(params?.start ?? ''),
                String(params?.end ?? '')
            );
            return out && typeof out === 'object' ? out : null;
        }
        case 'onDateSelected': {
            // The user picked a day in the host's date picker. Answer with
            // `{ query }` — a launcher query starting with one of this
            // provider's own keywords — and the host runs it.
            const p = providers.searchProvider;
            if (!p || typeof p.onDateSelected !== 'function') return null;
            const out = await p.onDateSelected(String(params?.date ?? ''));
            if (!out || typeof out !== 'object' || typeof out.query !== 'string') return null;
            return { query: out.query };
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
        case 'viewMount':
            return requireViewSurface(method).mount(params);
        case 'viewReopen':
            return requireViewSurface(method).reopen(params);
        case 'viewVisibility':
            return requireViewSurface(method).visibility(params);
        case 'viewTheme':
            return requireViewSurface(method).setTheme(params);
        case 'viewPing':
            // The runtime answers; the view module isn't involved.
            requireViewSurface(method);
            return true;
        case 'onConfigUpdate': {
            extensionConfig = params?.config || {};
            for (const p of [
                providers.searchProvider,
                providers.toolProvider,
                providers.triggerProvider,
                providers.settingsProvider,
                providers.toolbarProvider,
                providers.messageFormatter,
                providers.view,
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
                providers.view,
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
            if (msg.error) entry.reject(invokeError(msg));
            else entry.resolve(msg.result);
            break;
        }
        case 'storage-changed':
            // Fire-and-forget hint from the host: key names only (§10).
            storageEvents.deliver(msg.changes);
            break;
        case 'view-response':
            viewSurface?.handleResponse(msg);
            break;
        default:
            // Unknown messages are ignored — the host may be newer than us.
            break;
    }
}

// --- Stall watch ------------------------------------------------------------
//
// Chromium deprioritises this frame (cross-origin, 1x1, transparent). It has
// been seen freezing its timers and fetch callbacks for 10+ seconds while
// the window was VISIBLE, releasing them the instant the window hid — a
// Spotify poll's 4s timeout fired after 10s. That's invisible from the
// host, so the sandbox times its own heartbeat and logs when it runs badly
// late. Hidden pages are legitimately throttled (up to a minute between
// wake-ups), so only beats that start and end visible count.

/** Heartbeat period; a beat this many ms late is a stall worth logging. */
const STALL_BEAT_MS = 2000;
const STALL_THRESHOLD_MS = 3000;
/** At most one report per window of this length, so a long stall logs once. */
const STALL_REPORT_COOLDOWN_MS = 5 * 60 * 1000;

/**
 * Pure heartbeat bookkeeping. Call `beat()` from a fixed-period timer; it
 * reports via `report(lateMs)` when a visible-to-visible beat ran more than
 * `thresholdMs` late, rate-limited to one report per `cooldownMs`.
 */
export function createStallWatch({
    now,
    isVisible,
    report,
    periodMs = STALL_BEAT_MS,
    thresholdMs = STALL_THRESHOLD_MS,
    cooldownMs = STALL_REPORT_COOLDOWN_MS,
}) {
    let last = now();
    let lastVisible = isVisible();
    let lastReport = -Infinity;
    return {
        beat() {
            const t = now();
            const visible = isVisible();
            const late = t - last - periodMs;
            if (late > thresholdMs && visible && lastVisible && t - lastReport >= cooldownMs) {
                lastReport = t;
                report(Math.round(late));
            }
            last = t;
            lastVisible = visible;
        },
    };
}

function startStallWatch() {
    const watch = createStallWatch({
        now: () => performance.now(),
        isVisible: () => document.visibilityState === 'visible',
        report: (late) =>
            log(
                'warn',
                `sandbox stalled: a ${STALL_BEAT_MS}ms timer fired ${late}ms late ` +
                    'while the window was visible (timers and fetches in this sandbox were frozen)'
            ),
    });
    setInterval(() => watch.beat(), STALL_BEAT_MS);
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
    startStallWatch();
});

// Surface unhandled errors to the host instead of letting them vanish.
window.addEventListener('error', (e) => {
    log('error', `uncaught: ${e?.message || String(e)} at ${e?.filename}:${e?.lineno}`);
});
window.addEventListener('unhandledrejection', (e) => {
    log('error', `unhandledrejection: ${String(e?.reason?.message || e?.reason || e)}`);
});
