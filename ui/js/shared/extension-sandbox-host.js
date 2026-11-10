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
 *   - Validating and forwarding generic mascot activity hints
 *     (`context.mascot` → `type: 'mascot-activity'` → `window.__kageMascotHint`)
 *
 * Two surfaces:
 *   - `hidden` (default): a 1px iframe in the launcher, chat and settings
 *     windows that runs the extension's providers.
 *   - `view`: the visible iframe of an extension view window
 *     (docs/design/EXTENSION_VIEWS.md §8). It runs only the view module,
 *     has a CSP with no network, explicit `allow` denials, a navigation
 *     detector, the view RPCs and the `view-request` messages, and refuses
 *     the window-bound commands of `VIEW_SURFACE_DENIED_COMMANDS`.
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
 *   - Background agent calls (docs/design/EXTENSION_AGENT_CALLS.md §3.8):
 *     the host injects the id, passes `when: 'now'` only right after a
 *     click, refuses an oversized send, and cancels a sandbox's calls
 *     when it goes away.
 */

import { getConfig, onConfigChange } from './config-cache.js';
import { EVT } from './events.js';
import {
    abandonFileText,
    ClickLease,
    cleanDisplayIcon,
    cleanDisplayName,
    PICK_FILE_TEXT,
    routeFileTextInvoke,
    rpcTimer,
} from './extension-file-text.js';
import { decideInvoke } from './extension-permissions.js';
import { validateExtensionInvokeArgs } from './extension-url-policy.js';
import {
    buildViewSrcdoc,
    createRateWindow,
    escapeInlineScript,
    isValidViewId,
    isViewRequestKind,
    MAX_COMPOSE_CHARS,
    MAX_COPY_CHARS,
    MAX_CSS_FILE_BYTES,
    MAX_CSS_TOTAL_BYTES,
    MAX_SUBTITLE_CHARS,
    normalizeViewSource,
    normalizeViews,
    sanitizeStorageChanges,
    sanitizeViewTheme,
    sanitizeViewTitle,
    VIEW_IFRAME_ALLOW,
    VIEW_IFRAME_SANDBOX,
    VIEW_REQUEST_RATES,
    validateOpenViewEffect,
    viewTextError,
    viewThemesEqual,
} from './extension-view-policy.js';
import { t } from './i18n.js';

const BOOT_TIMEOUT_MS = 10_000;
const RPC_TIMEOUT_MS = 10_000;

/** The two sandbox surfaces (see the file comment). */
export const SURFACE_HIDDEN = 'hidden';
export const SURFACE_VIEW = 'view';

/** How long `mount` may take before the view page shows its error (§8.5). */
export const VIEW_MOUNT_TIMEOUT_MS = 10_000;
/** How long one watchdog ping may go unanswered (§8.7: one ping per 5 s). */
export const VIEW_PING_TIMEOUT_MS = 5_000;

// --- Mascot activity hints --------------------------------------------------

/**
 * Closed vocabulary of generic mascot activities. The mascot never learns
 * about individual extensions: an extension declares "music", not "spotify".
 * Must stay in sync with `MASCOT_ACTIVITIES` in
 * `js/extension-sandbox/runtime.js` (the sandbox-side convenience check).
 */
export const MASCOT_ACTIVITIES = Object.freeze(['music', 'meeting', 'timer']);

/**
 * Lease bounds for a mascot hint. The TTL is mandatory in spirit: an omitted
 * or unusable value collapses to the default rather than to "forever", so a
 * crashed, disabled, or hidden extension can never pin a pose. Extensions are
 * expected to re-assert from their refresh/render cycle.
 */
export const MASCOT_TTL_DEFAULT_MS = 30_000;
export const MASCOT_TTL_MIN_MS = 15_000;
export const MASCOT_TTL_MAX_MS = 600_000;

/** Clamp an extension-supplied TTL into the lease window. */
export function clampMascotTtl(ttlMs) {
    const n = Number(ttlMs);
    if (!Number.isFinite(n) || n <= 0) return MASCOT_TTL_DEFAULT_MS;
    return Math.min(MASCOT_TTL_MAX_MS, Math.max(MASCOT_TTL_MIN_MS, Math.round(n)));
}

/**
 * Normalize a manifest's `contributes.mascotActivities` into a deduped set of
 * known activity names. Unknown entries are dropped with a warning — same
 * contract as `normalizePermissions` for capabilities.
 */
export function normalizeMascotActivities(raw, extensionId) {
    if (!Array.isArray(raw)) return [];
    const out = [];
    for (const entry of raw) {
        if (typeof entry !== 'string') continue;
        const name = entry.trim().toLowerCase();
        if (!name) continue;
        if (!MASCOT_ACTIVITIES.includes(name)) {
            console.warn(`Extension '${extensionId}': unknown mascot activity '${name}' — ignored`);
            continue;
        }
        if (out.includes(name)) continue;
        out.push(name);
    }
    return out;
}

/**
 * Sandboxes that currently hold a forwarded mascot hint. Used to withdraw
 * hints the moment the user turns `ui.mascot_extension_hints` off — the
 * lease would expire on its own, but up to MASCOT_TTL_MAX_MS later.
 * @type {Set<ExtensionSandbox>}
 */
const _mascotHintHolders = new Set();
let _mascotHintWatcherInstalled = false;

/**
 * Is the user's mascot-extension-hint setting on? Reads through the shared
 * config cache (`ui.mascot_extension_hints`, default true), so the first hint
 * costs one `get_config` and the rest are free until `config_updated` fires.
 * A failed read defaults to "allowed" — a hint is cosmetic, and failing it
 * closed would silently break the feature on a transient IPC error.
 */
async function _mascotHintsAllowed(rawInvoke) {
    try {
        const cfg = await getConfig(rawInvoke);
        return cfg?.ui?.mascot_extension_hints !== false;
    } catch {
        return true;
    }
}

/** Install the one-per-module listener that withdraws live hints on opt-out. */
function _ensureMascotHintWatcher(rawInvoke) {
    if (_mascotHintWatcherInstalled) return;
    _mascotHintWatcherInstalled = true;
    try {
        onConfigChange(async () => {
            if (_mascotHintHolders.size === 0) return;
            if (await _mascotHintsAllowed(rawInvoke)) return;
            for (const sb of [..._mascotHintHolders]) sb.clearMascotActivity();
        });
    } catch {
        _mascotHintWatcherInstalled = false;
    }
}

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
 * The runtime is authored as ES modules (so vitest can import the
 * pieces), but a null-origin srcdoc iframe can only run a classic
 * script — so the host fetches every part, strips the module syntax,
 * and concatenates them in dependency order. Parts must be
 * self-contained (no imports other than each other) and collision-free
 * at top-level scope.
 *
 * `shared/icu-message.js` is the SAME file `shared/i18n.js` uses in the
 * host windows — one ICU parser for both sides of the sandbox boundary.
 */
export const RUNTIME_SOURCE_PATHS = [
    'vendor/lib/extension-module-lexer.js',
    'js/shared/extension-modules.js',
    'js/shared/icu-message.js',
    'js/extension-sandbox/runtime/sandboxed-worker.js',
    'js/extension-sandbox/runtime/host-api.js',
    'js/extension-sandbox/runtime/view-api.js',
    'js/extension-sandbox/runtime/files-api.js',
    'js/extension-sandbox/runtime/sync-api.js',
    'js/extension-sandbox/runtime/agent-api.js',
    'js/extension-sandbox/runtime.js',
];

/** Strip `import`/`export` module syntax so a module source runs as a classic script. */
export function stripModuleSyntax(source) {
    return source
        .replace(/^import\s[^\n]*\n/gm, '')
        .replace(/^export\s+(async\s+)?function\s/gm, '$1function ')
        .replace(/^export\s+const\s/gm, 'const ');
}

/**
 * Fetch + assemble the sandbox runtime source once. Returns the JS text
 * that will be inlined into each sandbox iframe's srcdoc.
 */
async function getSandboxRuntimeSource() {
    if (_runtimeSourceCache !== null) return _runtimeSourceCache;
    const parts = [];
    for (const path of RUNTIME_SOURCE_PATHS) {
        const resp = await fetch(path);
        if (!resp.ok)
            throw new Error(`Failed to fetch sandbox runtime part ${path}: HTTP ${resp.status}`);
        parts.push(stripModuleSyntax(await resp.text()));
    }
    _runtimeSourceCache = parts.join('\n;\n');
    return _runtimeSourceCache;
}

/**
 * Commands that read or write per-extension storage and therefore must be
 * scoped to the calling extension's identity. The host force-injects the
 * calling sandbox's real extension id into the arg named by the value; any
 * value the sandbox supplied is overwritten. The host owns the sandbox ->
 * extension mapping (it created the iframe), so this is authoritative — a
 * sandbox can't read or write another extension's data even if it knows the
 * key. A command reachable under the `storage` capability that touches
 * per-extension state but is missing here would be a cross-extension access
 * hole, so keep this in sync with the `storage` entries in
 * extension-permissions.js.
 *
 * The value is the arg name the backend command expects the id under:
 *   - `extension_data` commands take `extensionId` (Tauri auto-renames the
 *     Rust `extension_id: String` to camelCase across the IPC boundary).
 *   - `*_extension_config` commands take a bare `id: String`.
 *   - `ext_sync_*` commands (`context.sync`, docs/design/SYNC.md §12.6)
 *     take `extensionId`.
 *   - `agent_call_*` commands (`context.agent`,
 *     docs/design/EXTENSION_AGENT_CALLS.md §3.5) take `extensionId`. Rust
 *     binds each call id to the extension that reserved it.
 */
export const IDENTITY_SCOPED_COMMANDS = Object.freeze({
    save_extension_data: 'extensionId',
    load_extension_data: 'extensionId',
    delete_extension_data: 'extensionId',
    get_extension_config: 'id',
    save_extension_config: 'id',
    // A file the user picks (docs/design/EXTENSION_FILE_TEXT.md §3.5):
    // the dialog and the cancel belong to the calling extension.
    pick_file_text: 'extensionId',
    cancel_file_text: 'extensionId',
    ext_sync_status: 'extensionId',
    ext_sync_push: 'extensionId',
    ext_sync_pull: 'extensionId',
    ext_sync_ack: 'extensionId',
    agent_call_reserve: 'extensionId',
    agent_call_ready: 'extensionId',
    agent_call_send: 'extensionId',
    agent_call_cancel: 'extensionId',
    agent_call_status: 'extensionId',
});

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
 * @property {string[]} [mascotActivities] - the manifest's
 *   `contributes.mascotActivities`. An extension may only hint an activity it
 *   declared here; anything else is rejected host-side.
 * @property {string[]} [hostFeatures] - host UI features this window offers
 *   the extension, already filtered by {@link hostFeaturesFor}. Surfaced to
 *   extension code as the frozen `context.hostFeatures` array so it can feature-
 *   detect (older hosts send nothing → `undefined`).
 * @property {'hidden'|'view'} [surface] - `'view'` for an extension view
 *   window's sandbox. Anything else is the hidden surface.
 * @property {string} [displayName] - the manifest name, `__MSG_*__`
 *   resolved: the file dialog's title and the ask bar name the extension
 *   with it (EXTENSION_FILE_TEXT §3.5). Default: `extensionName`, else the id.
 * @property {string} [displayIcon] - the manifest icon, for the ask bar
 *
 * View surface only (docs/design/EXTENSION_VIEWS.md §8):
 * @property {string} [viewId] - the view this window shows; the manifest
 *   (`views`) must declare it
 * @property {unknown} [views] - the manifest's `contributes.views` (raw or
 *   normalised), for `context.view.open` checks
 * @property {string} [viewTitle] - the view's title, `__MSG_*__` resolved
 * @property {string} [extensionName] - the extension's display name
 * @property {unknown} [viewTheme] - the theme snapshot (`readViewTheme`)
 * @property {string[]} [viewCss] - the view's CSS file contents, in order
 * @property {ViewHooks} [viewHooks]
 * `sources` holds `{ view: <module source>, sourcePaths: { view: <path> } }`.
 */

/**
 * What the view page does for its sandbox. `setTitle` and `close` are
 * the page's; the rest default to the Tauri commands of §16.1.
 *
 * @typedef {object} ViewHooks
 * @property {(subtitle: string|null) => (void|Promise<void>)} [setTitle] -
 *   a cleaned subtitle (≤ 80 characters) or null to clear it
 * @property {() => (void|Promise<void>)} [close] - hide the window
 * @property {(viewId: string, params: object|null) => Promise<unknown>} [openView] -
 *   default: `ext_view_open` with `source: 'view'`
 * @property {(text: string) => Promise<unknown>} [compose] - default `ext_view_compose`
 * @property {(text: string) => Promise<unknown>} [copyText] - default `ext_view_copy_text`
 * @property {() => Promise<unknown>} [openSettings] - default
 *   `open_settings_window({ section: 'ext-<id>' })`
 * @property {() => void} [onNavigated] - the iframe navigated away from its
 *   document; the sandbox is already destroyed (§8.8)
 * @property {() => boolean} [hasUserActivation] - the window is focused and
 *   has a recent click or key press (§13.4). Default: `document.hasFocus()`
 *   and `navigator.userActivation.isActive` where the engine has it.
 */

/**
 * Whether this page has a recent user activation (§13.4): it has focus,
 * and `navigator.userActivation.isActive` where the engine supports it.
 * Activation in the view iframe reaches the page, its ancestor.
 */
export function defaultUserActivation() {
    try {
        if (typeof document !== 'undefined' && !document.hasFocus()) return false;
    } catch {}
    const ua = typeof navigator !== 'undefined' ? navigator.userActivation : undefined;
    return ua && typeof ua.isActive === 'boolean' ? ua.isActive : true;
}

// --- Host features ----------------------------------------------------------

/**
 * Host-rendered date picker. An extension holding the `calendar` capability
 * may mark date labels in its widget / result HTML as
 * `<button data-kage-date="YYYY-MM-DD">`; the host turns each into an opener
 * for its own month-calendar popup (the extension never touches that DOM).
 * Its search provider answers `getDateMarkers(start, end)` for the busy-day
 * dots and `onDateSelected(date)` with the launcher query that lists that
 * day. See docs/EXTENSIONS.md § Date picker.
 */
export const HOST_FEATURE_DATE_PICKER = 'date-picker';

/**
 * Folder sync for extension data (docs/design/SYNC.md §12.4–§12.6): whole
 * keys, lists inside a key, and record collections (`context.sync`). Each
 * requires the `sync` capability. The floating, chat, settings and view
 * windows offer all three (EXTENSION_VIEWS.md §9.4).
 */
export const HOST_FEATURE_SYNC_KEYS = 'sync.keys';
export const HOST_FEATURE_SYNC_LISTS = 'sync.lists';
export const HOST_FEATURE_SYNC_RECORDS = 'sync.records';

/** The three sync features, for a window's offered list. */
export const SYNC_HOST_FEATURES = Object.freeze([
    HOST_FEATURE_SYNC_KEYS,
    HOST_FEATURE_SYNC_LISTS,
    HOST_FEATURE_SYNC_RECORDS,
]);

/**
 * Extension views (docs/design/EXTENSION_VIEWS.md §9.4): `open_view`
 * actions and effects work in this window, and in a view window
 * `context.view` exists. Needs the `views` capability. Offered by the
 * floating, chat, settings and view windows.
 */
export const HOST_FEATURE_VIEWS = 'views';

/**
 * Files the user picks (docs/design/EXTENSION_FILE_TEXT.md §3.3):
 * `context.files.pickText()` works here. Needs `file_text`. Offered by the
 * windows that can show the ask bar: floating, chat, settings and view.
 */
export const HOST_FEATURE_FILE_TEXT = 'files.pick_text';

/**
 * Background agent calls (docs/design/EXTENSION_AGENT_CALLS.md §3.3):
 * `context.agent` works here. Needs `agent_background`. Offered by the
 * windows with long-lived extension instances: floating, chat and
 * settings. A view window never offers it, and a view sandbox drops it.
 */
export const HOST_FEATURE_AGENT_BACKGROUND = 'agent.background';

/** Capability each host feature requires. A feature with no entry needs none. */
const HOST_FEATURE_CAPABILITY = Object.freeze({
    [HOST_FEATURE_DATE_PICKER]: 'calendar',
    [HOST_FEATURE_VIEWS]: 'views',
    [HOST_FEATURE_FILE_TEXT]: 'file_text',
    [HOST_FEATURE_AGENT_BACKGROUND]: 'agent_background',
    [HOST_FEATURE_SYNC_KEYS]: 'sync',
    [HOST_FEATURE_SYNC_LISTS]: 'sync',
    [HOST_FEATURE_SYNC_RECORDS]: 'sync',
});

// --- `extension_sync_changes` (docs/design/SYNC.md §12.6) -------------------

/** Why a collection changed, as `context.sync.onChanges` reports it. */
export const SYNC_CHANGE_REASONS = Object.freeze(['remote', 'enabled', 'reset', 'purged']);

/** Collection names (`^[a-z][a-z0-9_]{0,31}$`). */
const SYNC_COLLECTION_RE = /^[a-z][a-z0-9_]{0,31}$/;

/** Live sandboxes offered `sync.records` in this window. */
const syncSandboxes = new Set();
let syncEventsInstalled = false;

/**
 * Hand one `extension_sync_changes` payload
 * (`{ extension_id, collections, reason }`) to this window's sandboxes of
 * that extension. Malformed payloads are dropped.
 *
 * @param {unknown} payload
 * @param {Iterable<ExtensionSandbox>} [sandboxes] - default: the live ones
 * @returns {number} how many sandboxes were told
 */
export function deliverSyncChanges(payload, sandboxes = syncSandboxes) {
    if (!payload || typeof payload !== 'object') return 0;
    const id = payload.extension_id;
    if (typeof id !== 'string' || !id) return 0;
    const reason = SYNC_CHANGE_REASONS.includes(payload.reason) ? payload.reason : null;
    if (!reason) return 0;
    const collections = Array.isArray(payload.collections)
        ? payload.collections.filter((c) => typeof c === 'string' && SYNC_COLLECTION_RE.test(c))
        : [];
    let told = 0;
    for (const s of sandboxes) {
        if (s.extensionId === id && s.notifySyncChanges(collections, reason)) told += 1;
    }
    return told;
}

/**
 * Listen for `extension_sync_changes` in this window, once, and forward
 * each to the matching sandboxes.
 *
 * @param {Function} [listen] - Tauri's `listen`; default: the window's
 * @returns {Promise<boolean>} whether a listener is installed
 */
export async function installExtensionSyncEvents(listen) {
    if (syncEventsInstalled) return true;
    const fn = listen || globalThis.window?.__TAURI__?.event?.listen;
    if (typeof fn !== 'function') return false;
    syncEventsInstalled = true;
    try {
        await fn(EVT.EXTENSION_SYNC_CHANGES, (event) => deliverSyncChanges(event?.payload));
        return true;
    } catch (e) {
        syncEventsInstalled = false;
        console.warn('[sandbox] sync change events unavailable:', e);
        return false;
    }
}

// --- Background agent calls (docs/design/EXTENSION_AGENT_CALLS.md §3.8) -----

export const AGENT_CALL_RESERVE = 'agent_call_reserve';
export const AGENT_CALL_READY = 'agent_call_ready';
export const AGENT_CALL_SEND = 'agent_call_send';
export const AGENT_CALL_CANCEL = 'agent_call_cancel';

/**
 * The largest `agent_call_send` payload the host forwards: its JSON text,
 * in UTF-16 code units (about one per character, so text in any script
 * counts the same). A fast fail before the IPC. Rust's limits are the
 * real ones, and a request within them is about 36,000 characters.
 */
export const AGENT_SEND_MAX_JSON_CHARS = 64 * 1024;

/**
 * Call ids a sandbox tracks at most. Only 5 of an extension's calls are
 * live at a time (4 waiting, 1 running), so an older id is long finished.
 */
const MAX_TRACKED_AGENT_CALLS = 64;

/** Rust's answer to a send before its turn: the reservation still waits. */
const AGENT_NOT_READY_KEY = 'errors.agent_call.not_ready';

/** The call id an `agent_call_ready` / `_send` / `_cancel` names, else null. */
export function agentCallId(args) {
    const a = args?.args;
    const id = a && typeof a === 'object' ? (a.call_id ?? a.callId) : undefined;
    return typeof id === 'string' && id ? id : null;
}

/**
 * `agent_call_reserve`'s args as the host forwards them: only `purpose`,
 * `when` and `dedupe_key`, in a new object, so no other shape reaches
 * serde (an array, or `when: { now: null }`, would read as `now`). `when`
 * is `'now'` only when `now` is true (a click lease), else `'idle'`.
 *
 * @param {unknown} raw - the sandbox's `args`
 * @param {boolean} now
 */
export function agentReserveArgs(raw, now) {
    const a = raw && typeof raw === 'object' && !Array.isArray(raw) ? raw : {};
    const out = { purpose: a.purpose, when: now === true ? 'now' : 'idle' };
    const key = a.dedupe_key ?? a.dedupeKey;
    if (key !== undefined && key !== null) out.dedupe_key = key;
    return out;
}

/**
 * Why the host refuses an `agent_call_send` before it reaches Rust, or
 * null: its JSON is over {@link AGENT_SEND_MAX_JSON_CHARS}, or it can't
 * be sent as JSON at all.
 *
 * @param {object} args - the forwarded args
 * @returns {{kind: string, key: string, vars: object, reason: string} | null}
 */
export function agentSendRefusal(args) {
    let size;
    try {
        size = JSON.stringify(args ?? {}).length;
    } catch {
        return {
            kind: 'invalid_input',
            key: 'errors.agent_call.field_invalid',
            vars: { field: 'request' },
            reason: "the request can't be sent as JSON",
        };
    }
    if (size <= AGENT_SEND_MAX_JSON_CHARS) return null;
    return {
        kind: 'invalid_input',
        key: 'errors.agent_call.prompt_too_large',
        vars: { size, limit: AGENT_SEND_MAX_JSON_CHARS },
        reason: `the request is ${size} characters of JSON (limit ${AGENT_SEND_MAX_JSON_CHARS})`,
    };
}

/** Sandboxes that tracked agent calls, for the `pagehide` cancel. */
const _agentCallHolders = new Set();
let _agentPagehideInstalled = false;

/**
 * When this window's page goes away, its sandboxes can't send or read
 * their calls any more, so cancel them. Best effort: the cancel may not
 * arrive before the window is gone; Rust's turn and call timeouts end
 * the calls then.
 */
function _ensureAgentPagehide() {
    if (_agentPagehideInstalled || typeof window === 'undefined') return;
    _agentPagehideInstalled = true;
    try {
        window.addEventListener('pagehide', () => {
            for (const sb of [..._agentCallHolders]) sb.cancelAgentCalls();
        });
    } catch {
        _agentPagehideInstalled = false;
    }
}

/**
 * Which of the features a window offers (`offered`) this extension may use,
 * given its granted capabilities. Unknown names are dropped, so a window
 * can't advertise something the boundary has no rules for.
 *
 * @param {string[]} offered
 * @param {Iterable<string>} capabilities
 * @returns {string[]}
 */
export function hostFeaturesFor(offered, capabilities) {
    if (!Array.isArray(offered)) return [];
    const held = new Set(capabilities || []);
    const out = [];
    for (const f of offered) {
        if (!Object.hasOwn(HOST_FEATURE_CAPABILITY, f)) continue;
        const needs = HOST_FEATURE_CAPABILITY[f];
        if (needs && !held.has(needs)) continue;
        if (!out.includes(f)) out.push(f);
    }
    return out;
}

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
        /**
         * Generic mascot activities this extension declared in
         * `contributes.mascotActivities`. Undeclared activities are rejected
         * even though they're in the global vocabulary.
         * @type {Set<string>}
         */
        this.mascotActivities = new Set(
            normalizeMascotActivities(spec.mascotActivities, spec.extensionId)
        );
        /**
         * Host features offered to this extension — re-filtered against the
         * capabilities here, so a caller that skipped `hostFeaturesFor`
         * still can't hand a feature to an extension that may not use it.
         * @type {string[]}
         */
        this.hostFeatures = hostFeaturesFor(spec.hostFeatures, this.capabilities);
        /** `'hidden'` or `'view'` (see the file comment). */
        this.surface = spec.surface === SURFACE_VIEW ? SURFACE_VIEW : SURFACE_HIDDEN;
        // Background agent work isn't a view's (EXTENSION_AGENT_CALLS §3.3);
        // its commands are refused there as well.
        if (this.surface === SURFACE_VIEW) {
            this.hostFeatures = this.hostFeatures.filter(
                (f) => f !== HOST_FEATURE_AGENT_BACKGROUND
            );
        }
        if (this.surface === SURFACE_VIEW) this._initViewSpec(spec);
        /** The host's name and icon for the extension (file dialog, ask bar). */
        this.displayName = cleanDisplayName(
            spec.displayName || spec.extensionName,
            spec.extensionId || ''
        );
        this.displayIcon = cleanDisplayIcon(spec.displayIcon);
        /**
         * Set by RPCs the host sends because of a click (`userGesture`):
         * one file dialog within 10 s (EXTENSION_FILE_TEXT §5.2).
         */
        this._clickLease = new ClickLease();
        /**
         * Whether the current click may still start one `when: 'now'` agent
         * call. Checking it doesn't use up the lease, so the same click can
         * still open a file dialog (EXTENSION_AGENT_CALLS §3.8).
         */
        this._agentNowUnused = false;
        /** >0 while a file request runs: click RPC timers are paused. */
        this._gesturePauses = 0;
        /** The activity we last forwarded and haven't cleared, else null. */
        this._mascotActivity = null;
        /**
         * Set by the extension manager: called when the extension's
         * formatter asks to be re-run (`context.formatter.requestReformat`).
         * @type {(() => void) | null}
         */
        this.onReformatRequest = null;
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
        /** View surface: the view module loaded (else `viewMount` fails). */
        this.hasView = false;
        /**
         * Search provider implements `onDateSelected` AND this window
         * offered it the date picker. Only then does the host route date
         * picks (and busy-day lookups) to it.
         */
        this.hasDateProvider = false;
        /** If true, formatMessage RPCs fire on streaming chunks too. */
        this.formatterOptsInStreaming = false;
        /** @type {string[]} ids of widgets the extension declared, post-init */
        this.widgetIds = [];

        /**
         * OAuth loopback listeners this extension started and hasn't
         * finished with. destroy() cancels them (see there).
         * @type {Set<string>}
         */
        this._oauthListeners = new Set();

        /**
         * Background agent calls this extension reserved and hasn't
         * finished with. destroy() cancels them (EXTENSION_AGENT_CALLS §3.8).
         * @type {Set<string>}
         */
        this._agentCalls = new Set();
    }

    /**
     * Create the iframe and wait until the sandbox runtime sends `ready`
     * followed by `init-ack`. Rejects on timeout or runtime error.
     */
    async start() {
        if (this._iframe) throw new Error('sandbox already started');

        // Fetch the runtime source (cached after first call).
        const runtimeSource = await getSandboxRuntimeSource();

        const iframe = this._createIframe();
        this._iframe = iframe;
        this._container.appendChild(iframe);
        // Use srcdoc with the runtime inlined as a classic script. This
        // avoids the CORS issue where a null-origin sandboxed iframe
        // can't load ES modules from the Tauri custom-protocol origin
        // in production builds. The runtime uses dynamic import() for
        // extension code (via blob URLs, which are same-origin to the
        // iframe), so module loading still works for extensions.
        iframe.srcdoc =
            this.surface === SURFACE_VIEW
                ? buildViewSrcdoc({
                      runtimeSource,
                      lang: this.i18nLanguage,
                      dir: this.i18nRtl ? 'rtl' : 'ltr',
                      theme: this.viewTheme,
                  })
                : `<!DOCTYPE html><html><head><meta charset="UTF-8"></head><body><script>${escapeInlineScript(runtimeSource)}</script></body></html>`;

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
        // That load was the srcdoc. Any later one means the frame navigated.
        if (this.surface === SURFACE_VIEW) this._watchViewNavigation();

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
                hostFeatures: this.hostFeatures.slice(),
                ...(this.surface === SURFACE_VIEW
                    ? {
                          surface: SURFACE_VIEW,
                          view: {
                              id: this.viewId,
                              theme: this.viewTheme,
                              css: this._viewCss.slice(),
                              // Links in a view open only with `urls` (§8.8).
                              links: this.capabilities.has('urls'),
                          },
                      }
                    : {}),
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
        this.hasView = this.surface === SURFACE_VIEW && !!initResult.result?.hasView;
        // The sandbox's claim is ANDed with what the host offered: an
        // extension can't become a date provider in a window that never
        // gave it the feature (or without the `calendar` grant behind it).
        this.hasDateProvider =
            !!initResult.result?.hasDateProvider &&
            this.hostFeatures.includes(HOST_FEATURE_DATE_PICKER);
        this.formatterOptsInStreaming = !!initResult.result?.formatterOptsInStreaming;
        this.widgetIds = Array.isArray(initResult.result?.widgetIds)
            ? initResult.result.widgetIds.slice()
            : [];
        // `context.sync.onChanges` hears of remote changes (SYNC.md §12.6).
        if (this.hostFeatures.includes(HOST_FEATURE_SYNC_RECORDS) && !this._destroyed) {
            syncSandboxes.add(this);
            installExtensionSyncEvents().catch(() => {});
        }
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
     *
     * `opts.userGesture: true` marks an RPC the host sends because the user
     * clicked or pressed a key (EXTENSION_FILE_TEXT §5.2): it gives the
     * sandbox a click lease, and its timer pauses while a file request runs.
     * The click may also start one `when: 'now'` agent call
     * (EXTENSION_AGENT_CALLS §3.8).
     */
    async call(method, params, opts = {}) {
        if (this._destroyed) throw new Error(`sandbox '${this.extensionId}' is destroyed`);
        if (!this._ready) throw new Error(`sandbox '${this.extensionId}' not ready`);
        const timeoutMs = Number.isFinite(opts.timeoutMs) ? opts.timeoutMs : RPC_TIMEOUT_MS;
        const gesture = opts.userGesture === true;
        if (gesture) {
            this._clickLease.grant();
            this._agentNowUnused = true;
        }
        const rpcId = this._nextRpcId++;
        return new Promise((resolve, reject) => {
            // timeoutMs <= 0 means "no timeout" — used for user-gated RPCs.
            const timer =
                timeoutMs > 0
                    ? rpcTimer(
                          timeoutMs,
                          () => {
                              this._pendingRpcs.delete(rpcId);
                              reject(new Error(`RPC '${method}' timed out after ${timeoutMs}ms`));
                          },
                          { pausable: gesture, paused: this._gesturePauses > 0 }
                      )
                    : null;
            this._pendingRpcs.set(rpcId, { resolve, reject, timer });
            try {
                this._port.postMessage({ type: 'rpc', rpcId, method, params });
            } catch (e) {
                timer?.clear();
                this._pendingRpcs.delete(rpcId);
                reject(e);
            }
        });
    }

    /** Use up the click lease: whether one was held (EXTENSION_FILE_TEXT §5.2). */
    consumeClickLease() {
        return this._clickLease.consume();
    }

    /** Whether the sandbox holds a click lease, without using it up. */
    hasClickLease() {
        return this._clickLease.has();
    }

    /**
     * Pause the timers of this sandbox's click RPCs while a file request
     * runs, so a launcher `execute()` waiting on the dialog doesn't time
     * out. Nests; {@link resumeGestureTimers} gives them the time they had left.
     */
    pauseGestureTimers() {
        if (this._gesturePauses++ > 0) return;
        for (const entry of this._pendingRpcs.values()) entry.timer?.pause();
    }

    resumeGestureTimers() {
        if (this._gesturePauses === 0 || --this._gesturePauses > 0) return;
        for (const entry of this._pendingRpcs.values()) entry.timer?.resume();
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

    /**
     * Tell the sandbox that its extension's stored data changed elsewhere
     * (docs/design/EXTENSION_VIEWS.md §10). Fire-and-forget, every surface.
     * The caller has matched the batch to this extension and set `self`;
     * this drops anything but `{ key, op, self, source }`, and sends
     * nothing unless the extension holds `storage`.
     *
     * @param {unknown} changes
     * @returns {boolean} whether a message was posted
     */
    notifyStorageChanged(changes) {
        if (this._destroyed || !this._ready || !this._port) return false;
        if (!this.capabilities.has('storage')) return false;
        const clean = sanitizeStorageChanges(changes);
        if (clean.length === 0) return false;
        try {
            this._port.postMessage({ type: 'storage-changed', changes: clean });
            return true;
        } catch (e) {
            console.warn(`[sandbox ${this.extensionId}] storage-changed failed:`, e);
            return false;
        }
    }

    /**
     * Tell the sandbox that its synced records changed (docs/design/SYNC.md
     * §12.6): `{ type: 'sync-changes', collections, reason }`, which
     * `context.sync.onChanges` delivers. Only to an extension this window
     * offered `sync.records`, which needs the `sync` grant.
     *
     * @param {string[]} collections
     * @param {string} reason
     * @returns {boolean} whether a message was posted
     */
    notifySyncChanges(collections, reason) {
        if (this._destroyed || !this._ready || !this._port) return false;
        if (!this.hostFeatures.includes(HOST_FEATURE_SYNC_RECORDS)) return false;
        if (!this.capabilities.has('sync') || !SYNC_CHANGE_REASONS.includes(reason)) return false;
        const clean = Array.isArray(collections)
            ? collections.filter((c) => typeof c === 'string' && SYNC_COLLECTION_RE.test(c))
            : [];
        try {
            this._port.postMessage({ type: 'sync-changes', collections: clean, reason });
            return true;
        } catch (e) {
            console.warn(`[sandbox ${this.extensionId}] sync-changes failed:`, e);
            return false;
        }
    }

    // --- view surface (docs/design/EXTENSION_VIEWS.md §8.6) -------------------

    /**
     * `viewMount`: the view module's `mount(root, info)`. Rejects after
     * {@link VIEW_MOUNT_TIMEOUT_MS} unless `opts.timeoutMs` says otherwise.
     *
     * @param {{params?: object|null, source?: string, reloaded?: boolean}} info
     */
    mountView({ params = null, source, reloaded = false } = {}, opts = {}) {
        this._requireViewSurface('mountView');
        const timeoutMs = Number.isFinite(opts.timeoutMs) ? opts.timeoutMs : VIEW_MOUNT_TIMEOUT_MS;
        return this.call(
            'viewMount',
            { params: params ?? null, source: normalizeViewSource(source), reloaded: !!reloaded },
            { timeoutMs }
        );
    }

    /** `viewReopen`: opened again while loaded; reaches the view's `onOpen`. */
    reopenView({ params = null, source } = {}) {
        this._requireViewSurface('reopenView');
        return this.call('viewReopen', {
            params: params ?? null,
            source: normalizeViewSource(source),
        });
    }

    /** `viewVisibility`: the window was shown or hidden (`onShow` / `onHide`). */
    setViewVisibility(visible) {
        this._requireViewSurface('setViewVisibility');
        return this.call('viewVisibility', { visible: !!visible });
    }

    /**
     * `viewTheme`, only when the snapshot changed (§11). The theme is
     * sanitised first: allowlisted tokens with safe values only.
     *
     * @returns {Promise<boolean>} whether the view was told
     */
    async setViewTheme(theme) {
        this._requireViewSurface('setViewTheme');
        const next = sanitizeViewTheme(theme);
        if (viewThemesEqual(next, this.viewTheme)) return false;
        this.viewTheme = next;
        await this.call('viewTheme', { theme: next });
        return true;
    }

    /** `viewPing`: the watchdog's heartbeat (§8.7). The runtime answers. */
    pingView(opts = {}) {
        this._requireViewSurface('pingView');
        const timeoutMs = Number.isFinite(opts.timeoutMs) ? opts.timeoutMs : VIEW_PING_TIMEOUT_MS;
        return this.call('viewPing', {}, { timeoutMs });
    }

    /**
     * Whether the view window has a recent user activation (§13.4): the
     * `hasUserActivation` hook, else {@link defaultUserActivation}.
     */
    hasUserActivation() {
        const hook = this._viewHooks?.hasUserActivation;
        try {
            return typeof hook === 'function' ? hook() === true : defaultUserActivation();
        } catch {
            return false;
        }
    }

    /** Tear down the iframe and reject all in-flight RPCs. */
    destroy() {
        if (this._destroyed) return;
        this._destroyed = true;
        syncSandboxes.delete(this);

        // A hint is a lease on a shared, visible thing — drop it immediately
        // rather than letting the mascot hold the pose until the TTL expires.
        this.clearMascotActivity();

        // Nobody can redeem an OAuth callback once the extension is gone,
        // so stop its listeners rather than let a later consent in the
        // browser be told "You're authorized". (The in-flight await command
        // isn't dropped with us, so the backend can't tell on its own.)
        for (const listenerId of this._oauthListeners) this._cancelOauthListener(listenerId);
        this._oauthListeners.clear();

        // Its ask bar goes, and a file request in flight is cancelled.
        abandonFileText(this);

        // Its background agent calls end too: Tauri doesn't drop a waiting
        // ready or send when the sandbox goes, and nobody could send or
        // read them now.
        this.cancelAgentCalls();
        _agentCallHolders.delete(this);

        // Best-effort: ask the extension to clean up inside the sandbox
        // before we tear the iframe out from under it.
        try {
            if (this._ready && this._port) {
                this._port.postMessage({ type: 'rpc', rpcId: -1, method: 'destroy', params: {} });
            }
        } catch {}

        for (const [, entry] of this._pendingRpcs) {
            entry.timer?.clear();
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

    /** Read and check the view-surface fields of the spec. */
    _initViewSpec(spec) {
        const declared = normalizeViews(spec.views, '', '', this.extensionId, { warn: null });
        if (!isValidViewId(spec.viewId) || !declared.some((v) => v.id === spec.viewId)) {
            throw new Error(
                `sandbox '${this.extensionId}': view surface needs a view id the manifest declares`
            );
        }
        this.viewId = spec.viewId;
        /** The manifest's views, for `context.view.open` checks. */
        this._viewDecls = declared;
        this.viewTitle = sanitizeViewTitle(spec.viewTitle) || this.viewId;
        this.extensionName = sanitizeViewTitle(spec.extensionName, 120) || this.extensionId;
        this.viewTheme = sanitizeViewTheme(spec.viewTheme);
        this._viewCss = this._limitViewCss(spec.viewCss);
        /** @type {ViewHooks} */
        this._viewHooks =
            spec.viewHooks && typeof spec.viewHooks === 'object' ? spec.viewHooks : {};
        this._viewRates = {};
        for (const [kind, [max, windowMs]] of Object.entries(VIEW_REQUEST_RATES)) {
            this._viewRates[kind] = createRateWindow(max, windowMs);
        }
        /** Kinds whose rate-limit refusal was logged and not allowed since. */
        this._viewRateLogged = new Set();
    }

    /** The view's CSS texts within the §4 limits; the rest are dropped. */
    _limitViewCss(list) {
        const out = [];
        let total = 0;
        const encoder = new TextEncoder();
        for (const text of Array.isArray(list) ? list : []) {
            if (typeof text !== 'string' || !text) continue;
            const size = encoder.encode(text).length;
            if (size > MAX_CSS_FILE_BYTES || total + size > MAX_CSS_TOTAL_BYTES) {
                console.warn(
                    `[sandbox ${this.extensionId}] view CSS over the size limits (256 KiB each, 1 MiB in total) — dropped`
                );
                continue;
            }
            total += size;
            out.push(text);
        }
        return out;
    }

    _requireViewSurface(method) {
        if (this.surface !== SURFACE_VIEW) {
            throw new Error(`${method} is only for view sandboxes`);
        }
    }

    /**
     * The sandbox iframe. Hidden surface: 1px and transparent. View
     * surface: fills its container, takes input, and carries the `allow`
     * denials, `referrerpolicy` and a title (§8.3). Both get exactly
     * `sandbox="allow-scripts"`.
     */
    _createIframe() {
        const iframe = document.createElement('iframe');
        iframe.dataset.extensionId = this.extensionId;
        if (this.surface === SURFACE_VIEW) {
            iframe.dataset.viewId = this.viewId;
            iframe.setAttribute('sandbox', VIEW_IFRAME_SANDBOX);
            iframe.setAttribute('allow', VIEW_IFRAME_ALLOW);
            iframe.setAttribute('referrerpolicy', 'no-referrer');
            iframe.title = t('ext_view.iframe_label', {
                view: this.viewTitle,
                extension: this.extensionName,
            });
            iframe.className = 'kv-frame';
            iframe.style.cssText = 'display:block;width:100%;height:100%;border:0;';
            return iframe;
        }
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
        return iframe;
    }

    /**
     * After the srcdoc's load, any further `load` of the view iframe means
     * it navigated itself (`location.href = …`). The Rust guard doesn't
     * see iframe navigations on Windows, so the page tears the sandbox
     * down on every platform (§8.8).
     */
    _watchViewNavigation() {
        const iframe = this._iframe;
        if (!iframe) return;
        iframe.addEventListener('load', () => this._onViewNavigated());
    }

    _onViewNavigated() {
        if (this._destroyed) return;
        console.warn(
            `[sandbox ${this.extensionId}] view '${this.viewId}' navigated away from its document — stopped`
        );
        this.destroy();
        try {
            this._viewHooks.onNavigated?.();
        } catch (e) {
            console.warn(`[sandbox ${this.extensionId}] onNavigated hook failed:`, e);
        }
    }

    /**
     * A `view-request` from the view sandbox (§8.6). Every field is
     * type-checked; the answer is a `view-response` with `result`, or with
     * `error` plus the `errorKind` / `errorKey` the extension can branch on.
     * Unknown kinds, and any request on the hidden surface, are ignored
     * (warned about once, so a loop can't flood the log).
     */
    async _handleViewRequest(msg) {
        if (this.surface !== SURFACE_VIEW || !isViewRequestKind(msg?.kind)) {
            if (!this._warnedStrayViewRequest) {
                this._warnedStrayViewRequest = true;
                console.warn(
                    this.surface !== SURFACE_VIEW
                        ? `[sandbox ${this.extensionId}] view-request outside a view window — ignored`
                        : `[sandbox ${this.extensionId}] unknown view request '${String(msg?.kind)}' — ignored`
                );
            }
            return;
        }
        const id = msg.id;
        if (!Number.isSafeInteger(id)) return;
        let response;
        try {
            const outcome = await this._runViewRequest(msg.kind, msg);
            if (outcome?.refused) {
                const { kind, key, vars, reason } = outcome.refused;
                // Rate-limit refusals log once until that kind is allowed again.
                const quiet = kind === 'rate_limited' && this._viewRateLogged.has(msg.kind);
                if (kind === 'rate_limited') this._viewRateLogged.add(msg.kind);
                if (!quiet) {
                    const what = msg.kind === 'open-view' ? 'open_view' : `view ${msg.kind}`;
                    console.warn(`[ext ${this.extensionId}] ${what} refused: ${reason}`);
                }
                response = {
                    type: 'view-response',
                    id,
                    error: key ? t(key, vars || {}) : reason,
                    errorKind: kind,
                    ...(key ? { errorKey: key } : {}),
                };
            } else {
                response = { type: 'view-response', id, result: outcome?.result ?? null };
            }
        } catch (e) {
            response = { type: 'view-response', id, error: String(e?.message || e) };
            if (typeof e?.kind === 'string') response.errorKind = e.kind;
            if (typeof e?.key === 'string') response.errorKey = e.key;
        }
        if (this._destroyed || !this._port) return;
        this._port.postMessage(response);
    }

    /** Rate check for a view request kind; true when allowed (and counted). */
    _viewRateOk(kind) {
        const ok = this._viewRates[kind] ? this._viewRates[kind].take() : true;
        if (ok) this._viewRateLogged.delete(kind);
        return ok;
    }

    async _runViewRequest(kind, msg) {
        const refuse = (errorKind, key, vars, reason) => ({
            refused: { kind: errorKind, key, vars, reason },
        });
        const tooFast = (what) =>
            refuse('rate_limited', 'errors.ext_view.too_fast', {}, `${what} is rate-limited`);
        const notActive = () =>
            refuse(
                'not_permitted',
                'errors.ext_view.needs_user_action',
                {},
                'needs the window in front and a recent click or key press'
            );
        const textInvalid = (reason) =>
            refuse('invalid_input', 'errors.ext_view.text_invalid', { reason }, reason);
        const hooks = this._viewHooks;
        switch (kind) {
            case 'set-title': {
                const text = msg.text;
                if (text !== null && typeof text !== 'string') {
                    return textInvalid('the title must be a string or null');
                }
                if (typeof hooks.setTitle !== 'function') {
                    return refuse('not_permitted', null, null, 'this window has no title bar hook');
                }
                if (!this._viewRateOk(kind)) return tooFast('setTitle');
                const clean =
                    text === null ? null : sanitizeViewTitle(text, MAX_SUBTITLE_CHARS) || null;
                await hooks.setTitle(clean);
                return { result: clean };
            }
            case 'close': {
                if (typeof hooks.close !== 'function') {
                    return refuse('not_permitted', null, null, 'this window has no close hook');
                }
                await hooks.close();
                return { result: null };
            }
            case 'open-view': {
                const check = validateOpenViewEffect(
                    { type: 'open_view', view: msg.view, params: msg.params ?? null },
                    {
                        hostFeatures: this.hostFeatures,
                        capabilities: this.capabilities,
                        views: this._viewDecls,
                        extensionId: this.extensionId,
                        extensionName: this.extensionName,
                    }
                );
                if (!check.ok) return { refused: check };
                if (!this.hasUserActivation()) return notActive();
                if (!this._viewRateOk(kind)) return tooFast('open');
                const result = await (typeof hooks.openView === 'function'
                    ? hooks.openView(check.view, check.params)
                    : this._rawInvoke('ext_view_open', {
                          extensionId: this.extensionId,
                          viewId: check.view,
                          params: check.params,
                          source: 'view',
                      }));
                return { result: { reused: !!result?.reused } };
            }
            case 'compose':
            case 'copy-text': {
                const max = kind === 'compose' ? MAX_COMPOSE_CHARS : MAX_COPY_CHARS;
                const problem = viewTextError(msg.text, max);
                if (problem) return textInvalid(problem);
                if (!this.hasUserActivation()) return notActive();
                if (!this._viewRateOk(kind)) return tooFast(kind);
                if (kind === 'compose') {
                    await (typeof hooks.compose === 'function'
                        ? hooks.compose(msg.text)
                        : this._rawInvoke('ext_view_compose', { text: msg.text }));
                } else {
                    await (typeof hooks.copyText === 'function'
                        ? hooks.copyText(msg.text)
                        : this._rawInvoke('ext_view_copy_text', { text: msg.text }));
                }
                return { result: null };
            }
            case 'open-settings': {
                // Settings comes to the front, so it needs a user action
                // like opening a view does.
                if (!this.hasUserActivation()) return notActive();
                if (!this._viewRateOk(kind)) return tooFast('openSettings');
                // The section is the host's idea of this extension, never
                // something the sandbox names.
                await (typeof hooks.openSettings === 'function'
                    ? hooks.openSettings()
                    : this._rawInvoke('open_settings_window', {
                          section: `ext-${this.extensionId}`,
                      }));
                return { result: null };
            }
            default:
                return refuse('invalid_input', null, null, `unknown view request '${kind}'`);
        }
    }

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
                entry.timer?.clear();
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
            case 'reformat-request': {
                // Pure-UI hint: no data, no capability. The extension
                // manager decides what (if anything) to re-run.
                if (!this._destroyed) this.onReformatRequest?.();
                break;
            }
            case 'mascot-activity': {
                // Pure-UI hint. No capability, no Tauri, no data — see
                // _handleMascotActivity for the validation it does go through.
                this._handleMascotActivity(msg);
                break;
            }
            case 'view-request': {
                // View surface only; checked field by field there.
                this._handleViewRequest(msg);
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
        // A view window's sandbox also can't call the window-bound
        // commands of VIEW_SURFACE_DENIED_COMMANDS (§8.6).
        const decision = decideInvoke(command, this.capabilities, { surface: this.surface });
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
        const argError = validateExtensionInvokeArgs(command, args);
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

        // For identity-scoped commands, force-inject the extension's identity
        // under whichever arg name the backend expects. The host owns the
        // sandbox -> extension mapping (it created the iframe), so this is
        // authoritative. Any id the sandbox tried to supply is overwritten —
        // sandboxes can't read or write another extension's data or config
        // even if they know the key.
        //
        // Tauri 2 expects camelCase arg names on the IPC boundary
        // (`save_extension_data(extension_id: String)` in Rust ⇄
        // `{ extensionId }` in JS) — an earlier snake_case injection silently
        // failed the auto-rename: the command rejected the call with "missing
        // required key extensionId". The *_extension_config commands take a
        // bare `id` instead, so the arg name is looked up per command.
        const idArgName = IDENTITY_SCOPED_COMMANDS[command];
        const forwardedArgs = idArgName
            ? { ...(args || {}), [idArgName]: this.extensionId }
            : args || {};
        // The file dialog's title names the extension with the host's name
        // for it, never one the sandbox sent.
        if (command === PICK_FILE_TEXT) forwardedArgs.displayName = this.displayName;

        // Background agent calls (EXTENSION_AGENT_CALLS §3.8): `now` needs a
        // click, an oversized send fails here, and a cancel ends tracking.
        let agentNow = false;
        if (command === AGENT_CALL_RESERVE) {
            agentNow = this._takeAgentNow(forwardedArgs.args);
            forwardedArgs.args = agentReserveArgs(forwardedArgs.args, agentNow);
        } else if (command === AGENT_CALL_SEND) {
            const refusal = agentSendRefusal(forwardedArgs);
            if (refusal) {
                this._refuseAgentSend(id, forwardedArgs, refusal);
                return;
            }
        } else if (command === AGENT_CALL_CANCEL) {
            this._agentCalls.delete(agentCallId(forwardedArgs));
        }

        try {
            // pick_file_text and cancel_file_text go through this window's
            // file broker (the ask bar, the dialog guard, the progress chip).
            const result = await (routeFileTextInvoke(this, command, forwardedArgs) ??
                this._rawInvoke(command, forwardedArgs));
            this._trackOauthListener(command, forwardedArgs, { result });
            this._trackAgentCall(command, forwardedArgs, { ok: true, result });
            // Destroyed while the command ran: the port is gone.
            if (this._destroyed) return;
            this._port.postMessage({ type: 'invoke-response', id, result });
        } catch (e) {
            this._trackOauthListener(command, forwardedArgs, { error: e });
            this._trackAgentCall(command, forwardedArgs, { ok: false, error: e });
            // A refused reservation started no call, so the click may still start one.
            if (agentNow) this._agentNowUnused = true;
            if (this._destroyed) return;
            // A Rust AppError arrives as `{ kind, key, message }`. Forward
            // kind + key so the extension can branch on them (e.g. an OAuth
            // `timeout` vs `cancelled`) instead of on the localised message.
            const response = { type: 'invoke-response', id, error: String(e?.message || e) };
            if (typeof e?.kind === 'string') response.errorKind = e.kind;
            if (typeof e?.key === 'string') response.errorKey = e.key;
            this._port.postMessage(response);
        }
    }

    /**
     * Keep `_oauthListeners` current from a finished oauth_loopback_* invoke:
     * a started listener is ours until an await settles it or a cancel
     * stops it. A rejected second await (`already_awaited`) leaves the
     * listener running, so it stays tracked.
     */
    _trackOauthListener(command, args, { result, error }) {
        if (command === 'oauth_loopback_start') {
            const listenerId = result?.listener_id;
            if (typeof listenerId !== 'string' || !listenerId) return;
            // Started after destroy(): the extension will never see the id.
            if (this._destroyed) this._cancelOauthListener(listenerId);
            else this._oauthListeners.add(listenerId);
            return;
        }
        if (command !== 'oauth_loopback_await' && command !== 'oauth_loopback_cancel') return;
        if (command === 'oauth_loopback_await' && error?.key === 'errors.oauth.already_awaited')
            return;
        const listenerId = args?.args?.listener_id;
        if (typeof listenerId === 'string') this._oauthListeners.delete(listenerId);
    }

    /** Fire-and-forget oauth_loopback_cancel (idempotent on the backend). */
    _cancelOauthListener(listenerId) {
        try {
            Promise.resolve(
                this._rawInvoke('oauth_loopback_cancel', { args: { listener_id: listenerId } })
            ).catch(() => {});
        } catch {}
    }

    // --- background agent calls (EXTENSION_AGENT_CALLS §3.8) -----------------

    /**
     * Whether this `agent_call_reserve` may keep `when: 'now'`: it asks for
     * it, the sandbox holds a click lease, and that click hasn't started a
     * `now` call yet. The lease itself stays for a file dialog. Anything
     * else goes as `'idle'`, so a timer or an agent tool can't skip the
     * idle gate.
     */
    _takeAgentNow(raw) {
        const wants = !!raw && typeof raw === 'object' && raw.when === 'now';
        if (!wants) return false;
        if (this._agentNowUnused && this.hasClickLease()) {
            this._agentNowUnused = false;
            return true;
        }
        if (!this._agentNowWarned) {
            this._agentNowWarned = true;
            console.warn(
                `[sandbox ${this.extensionId}] agent call with when: 'now' outside a click — sent as 'idle'`
            );
        }
        return false;
    }

    /**
     * Keep `_agentCalls` current from a finished agent_call_* invoke. A
     * reserved call is ours until a send is answered, its turn is refused,
     * or a cancel is sent. A failure without an AppError `kind` never
     * reached the service (the IPC refused the args), so the host cancels
     * that call itself: its turn would otherwise hold the agent for 10 s.
     *
     * @param {string} command
     * @param {object} args - the forwarded args
     * @param {{ok: boolean, result?: any, error?: any}} outcome
     */
    _trackAgentCall(command, args, outcome) {
        if (command === AGENT_CALL_RESERVE) {
            const callId = outcome.ok ? outcome.result?.call_id : null;
            if (typeof callId !== 'string' || !callId) return;
            // Reserved after destroy(): the extension will never see the id.
            if (this._destroyed) {
                this._cancelAgentCall(callId);
                return;
            }
            if (this._agentCalls.size >= MAX_TRACKED_AGENT_CALLS) {
                this._agentCalls.delete(this._agentCalls.values().next().value);
            }
            this._agentCalls.add(callId);
            _agentCallHolders.add(this);
            _ensureAgentPagehide();
            return;
        }
        if (command !== AGENT_CALL_READY && command !== AGENT_CALL_SEND) return;
        const callId = agentCallId(args);
        if (!callId || !this._agentCalls.has(callId)) return;
        if (outcome.ok) {
            if (command === AGENT_CALL_SEND) this._agentCalls.delete(callId);
            return;
        }
        if (typeof outcome.error?.kind !== 'string') {
            this._cancelAgentCall(callId);
        } else if (command === AGENT_CALL_SEND && outcome.error?.key === AGENT_NOT_READY_KEY) {
            // Sent before its turn: the reservation still waits.
            return;
        }
        this._agentCalls.delete(callId);
    }

    /**
     * Refuse an `agent_call_send` here, with the `kind` and `key` Rust
     * would use. Rust never sees it, so the host ends that call's turn.
     */
    _refuseAgentSend(id, args, refusal) {
        const callId = agentCallId(args);
        if (callId && this._agentCalls.delete(callId)) this._cancelAgentCall(callId);
        console.warn(
            `[sandbox ${this.extensionId}] BLOCKED invoke('agent_call_send'): ${refusal.reason}`
        );
        this._port.postMessage({
            type: 'invoke-response',
            id,
            error: t(refusal.key, refusal.vars),
            errorKind: refusal.kind,
            errorKey: refusal.key,
        });
    }

    /** Cancel every agent call this sandbox still tracks. Idempotent. */
    cancelAgentCalls() {
        for (const callId of this._agentCalls) this._cancelAgentCall(callId);
        this._agentCalls.clear();
    }

    /** Fire-and-forget agent_call_cancel (idempotent; unknown ids are fine). */
    _cancelAgentCall(callId) {
        try {
            Promise.resolve(
                this._rawInvoke(AGENT_CALL_CANCEL, {
                    extensionId: this.extensionId,
                    args: { call_id: callId },
                })
            ).catch(() => {});
        } catch {}
    }

    /**
     * Handle a `mascot-activity` port message.
     *
     * Shape: `{ type: 'mascot-activity', activity: 'music'|'meeting'|'timer'|null, ttlMs?: number }`.
     * `activity: null` (or a missing activity) means "clear mine".
     *
     * Validation ladder, in order:
     *   1. vocabulary — the activity must be a known generic name;
     *   2. declaration — the manifest must have declared it in
     *      `contributes.mascotActivities`;
     *   3. user setting — `ui.mascot_extension_hints` must not be off;
     *   4. TTL clamp — the lease is bounded regardless of what was asked for.
     *
     * Then it forwards to the mascot consumer's global, which owns the
     * arbiter (priority between extensions, expiry, picking a winner). Both
     * globals are optional: the chat window has no mascot, and the floating
     * window installs them only once the mascot exists.
     */
    async _handleMascotActivity(msg) {
        if (this._destroyed) return;

        const raw = msg?.activity;
        if (raw === null || raw === undefined) {
            this.clearMascotActivity();
            return;
        }
        const activity = typeof raw === 'string' ? raw.trim().toLowerCase() : '';
        if (!MASCOT_ACTIVITIES.includes(activity)) {
            console.warn(
                `[sandbox ${this.extensionId}] unknown mascot activity '${String(raw)}' — ignored`
            );
            return;
        }
        if (!this.mascotActivities.has(activity)) {
            console.warn(
                `[sandbox ${this.extensionId}] mascot activity '${activity}' is not declared in ` +
                    `contributes.mascotActivities — ignored`
            );
            return;
        }
        const ttlMs = clampMascotTtl(msg?.ttlMs);

        if (!(await _mascotHintsAllowed(this._rawInvoke))) return;
        // The await above yields; the sandbox may have been torn down since.
        if (this._destroyed) return;

        _ensureMascotHintWatcher(this._rawInvoke);
        this._mascotActivity = activity;
        _mascotHintHolders.add(this);
        try {
            window.__kageMascotHint?.(this.extensionId, activity, ttlMs);
        } catch (e) {
            console.warn(`[sandbox ${this.extensionId}] mascot hint failed:`, e);
        }
    }

    /**
     * Withdraw this extension's mascot hint, if it holds one. Called when the
     * extension asks (`context.mascot.clearActivity()`), when the sandbox is
     * destroyed (unload / disable / uninstall / window teardown), when a
     * widget circuit-breaker trips, and when the user turns mascot extension
     * hints off. Idempotent.
     */
    clearMascotActivity() {
        _mascotHintHolders.delete(this);
        if (!this._mascotActivity) return;
        this._mascotActivity = null;
        try {
            window.__kageMascotHintClear?.(this.extensionId);
        } catch (e) {
            console.warn(`[sandbox ${this.extensionId}] mascot hint clear failed:`, e);
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

    /**
     * Forward one extension's storage changes to its sandbox only (§10.1).
     * `changes` already carry `self`; see
     * {@link ExtensionSandbox#notifyStorageChanged}.
     *
     * @returns {boolean} whether a sandbox was told
     */
    notifyStorageChanged(extensionId, changes) {
        const sb = this._sandboxes.get(extensionId);
        return sb ? sb.notifyStorageChanged(changes) : false;
    }

    *entries() {
        yield* this._sandboxes.entries();
    }

    get size() {
        return this._sandboxes.size;
    }
}
