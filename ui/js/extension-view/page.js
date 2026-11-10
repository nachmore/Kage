/**
 * The extension view page (docs/design/EXTENSION_VIEWS.md §8).
 *
 * A view window loads `ui/extension-view.html`. This page is first-party
 * and trusted: it draws the host bar, loads the view's module into a
 * visible sandboxed iframe (`ExtensionSandbox`, surface `'view'`), and
 * decides everything the view asks for. The view itself only draws and
 * asks, over the sandbox's port.
 *
 * Identity comes from the window label (`ext-view:<extension>:<view>`),
 * which Rust chose when it built the window. `ext_view_boot` re-checks the
 * install, the enabled state, the `views` grant and the view on every
 * load, so a (re)load never runs a view Rust wouldn't open.
 *
 * Everything the page touches outside this file is passed in (`deps`), so
 * the tests can drive it with fakes; `main.js` wires the real modules.
 */

import { errMessage } from '../shared/error-message.js';
import { EVT } from '../shared/events.js';
import { isExtensionEnabled, resolveExtensionConfig } from '../shared/extension-config.js';
import {
    applyManifestI18n,
    extensionDisplay,
    fetchExtensionLocaleViaInvoke,
    fetchSharedSourcesViaInvoke,
    resolveExtensionMessage,
} from '../shared/extension-manager/i18n.js';
import { normalizePermissions } from '../shared/extension-permissions.js';
import { hostFeaturesFor } from '../shared/extension-sandbox-host.js';
import { cmdOrCtrlPressed } from '../shared/shortcuts.js';
import { parseExtViewLabel } from '../shared/window-labels.js';
import { createChrome, windowTitle } from './chrome.js';
import { readThemeSnapshot, sameThemeSnapshot } from './theme-snapshot.js';
import { createWatchdog } from './watchdog.js';

/** `mount` must resolve within this, or the page shows "couldn't open". */
export const MOUNT_TIMEOUT_MS = 10_000;
/** A view title after `__MSG_*__` resolution (§4). */
export const TITLE_MAX_CHARS = 60;
/** A subtitle from `context.view.setTitle`, and an extension name (§8.6). */
export const SUBTITLE_MAX_CHARS = 80;
/** One view CSS file, and all of a view's CSS together (§4). */
export const CSS_FILE_MAX_BYTES = 256 * 1024;
export const CSS_TOTAL_MAX_BYTES = 1024 * 1024;

/** What opened a view (§9.1); the same closed list as `ext_views::VIEW_SOURCES`. */
export const VIEW_SOURCES = Object.freeze([
    'launcher',
    'chat',
    'formatter',
    'widget',
    'toolbar',
    'settings',
    'view',
]);

/** A `source` from Rust, checked against {@link VIEW_SOURCES}. */
export function normalizeViewSource(source) {
    return VIEW_SOURCES.includes(source) ? source : 'launcher';
}

/**
 * The capabilities an extension runs with: what its manifest requests,
 * cut down to what the user granted. No grant record means none. The same
 * rule the extension manager and Settings apply.
 */
export function grantedCapabilities(manifest, config) {
    const id = manifest?.id || '<unknown>';
    const requested = normalizePermissions(manifest?.permissions, id);
    const record = config?.extension_grants?.[manifest?.id];
    if (!record) return [];
    const granted = new Set(normalizePermissions(record.granted, id));
    return requested.filter((cap) => granted.has(cap));
}

/**
 * The "recent user activation" rule (§13.4): the view window is focused
 * and, where the engine reports it, the user just clicked or pressed a key
 * in it. A click in the view's iframe activates this page too, and script
 * can't fake it. Where `navigator.userActivation` is missing, focus (plus
 * the caller's rate limit) is the rule. EXTENSION_FILE_TEXT's file broker
 * uses the same rule for a view (its §5.2).
 *
 * @param {boolean} focused - `document.hasFocus()`
 * @param {{ isActive?: boolean } | undefined} activation - `navigator.userActivation`
 */
export function recentUserActivation(focused, activation) {
    if (!focused) return false;
    if (activation && typeof activation.isActive === 'boolean') return activation.isActive;
    return true;
}

function stripDotSlash(path) {
    return String(path).replace(/^\.\//, '');
}

function utf8Length(text) {
    return new TextEncoder().encode(text).length;
}

/**
 * @typedef {object} ViewPageDeps
 * @property {(cmd: string, args?: object) => Promise<any>} invoke - raw Tauri invoke
 * @property {(event: string, handler: Function) => Promise<Function>} listen - global
 *   (hears events sent to any window)
 * @property {{ label: string, close(): Promise<void>, setTitle(title: string): Promise<void>,
 *   isVisible(): Promise<boolean>, listen(event: string, handler: Function): Promise<Function>,
 *   onFocusChanged?(handler: Function): Promise<Function> }} appWindow - `listen` hears
 *   only events sent to this window
 * @property {(key: string, vars?: object) => string} t - Kage's catalog
 * @property {() => Promise<void>} initI18n
 * @property {(root: ParentNode) => void} applyStaticTranslations
 * @property {() => Promise<void>} applyTheme - `loadAndApplyTheme(invoke)`
 * @property {() => void} [initThemeListener]
 * @property {() => Promise<object>} getConfig - the config cache
 * @property {(handler: Function) => Function} onConfigChange
 * @property {(spec: object, container: HTMLElement) => object} createSandbox -
 *   an unstarted `ExtensionSandbox`
 * @property {(opts: { listen: Function, myLabel: string, deliver: Function }) => any} installDataEvents
 * @property {{ normalizeViews: Function, sanitizeViewTitle: Function, isSafeTokenValue: Function }} policy
 *   - from `shared/extension-view-policy.js`
 * @property {string[]} hostFeatures - offered to the view (filtered by its grant)
 * @property {() => ({ isActive: boolean } | undefined)} [userActivation]
 * @property {() => boolean} [hasFocus]
 * @property {Document} [doc]
 * @property {Window} [win]
 */

/** @param {ViewPageDeps} deps */
export function createViewPage(deps) {
    return new ViewPage(deps);
}

class ViewPage {
    /** @param {ViewPageDeps} deps */
    constructor(deps) {
        this._d = deps;
        this._doc = deps.doc || document;
        this._win = deps.win || window;
        // Named `t` so scripts/check_i18n.py sees the keys `this.t(...)` uses.
        this.t = deps.t;
        this._label = deps.appWindow?.label || '';
        /** `{ extensionId, viewId }` from the label, or null. */
        this._ids = parseExtViewLabel(this._label);

        this._chrome = null;
        /** Bumped by every load; a load whose number is stale stops. */
        this._gen = 0;
        /** 'idle' | 'loading' | 'mounted' | 'state' */
        this._phase = 'idle';
        /** Which panel shows in the 'state' phase. */
        this._stateKind = null;
        this._sandbox = null;
        this._sandboxStarted = false;
        /** What the mounted view runs with. */
        this._current = null;
        /** The newest `open_seq` this page has delivered. */
        this._seq = 0;
        /** The newest reopen that arrived while loading. */
        this._pendingReopen = null;
        this._visible = true;
        /** A config or extension change arrived while loading. */
        this._recheck = false;
        this._subtitle = null;
        /** The theme the running sandbox has. */
        this._sentTheme = null;
        this._themeQueued = false;
        this._watchdog = createWatchdog({
            ping: () =>
                this._sandbox ? this._sandbox.call('viewPing', {}, { timeoutMs: 0 }) : undefined,
            onUnresponsive: () =>
                this._chrome?.showBanner({
                    message: this.t('ext_view.state.not_responding', { view: this._viewName() }),
                    actions: ['reload', 'close'],
                }),
            onRecovered: () => this._chrome?.hideBanner(),
        });
    }

    /** §8.5, from step 1 (the link handler is main.js's). */
    async start() {
        try {
            await this._d.applyTheme();
        } catch (e) {
            console.warn('[extension-view] theme failed:', e);
        }
        this._d.initThemeListener?.();
        try {
            await this._d.initI18n();
        } catch (e) {
            console.warn('[extension-view] i18n failed:', e);
        }
        this._d.applyStaticTranslations(this._doc);
        this._chrome = createChrome({
            doc: this._doc,
            t: this.t,
            onAction: (action) => this._onAction(action),
        });
        if (this._ids) {
            this._chrome.setIdentity({
                extensionName: this._ids.extensionId,
                viewTitle: this._ids.viewId,
            });
            this._applyWindowTitle();
        }
        this._installGuards();
        this._watchTheme();
        // Before the first ext_view_boot, so a reopen that Rust sends after
        // it has marked this window booted isn't lost while we load.
        await this._subscribe();
        if (!this._ids) {
            console.error(`[extension-view] '${this._label}' isn't a view window label`);
            this._showState('load_failed', this.t('ext_view.state.load_failed', { view: '' }), [
                'close',
            ]);
            return;
        }
        await this._load();
    }

    // --- Loading -----------------------------------------------------------

    /**
     * Boot (or reload in place) the view: §8.5 steps 2–9. Any earlier load
     * or running sandbox is dropped first. Never rejects: anything
     * unexpected ends in the "couldn't open" panel.
     */
    async _load(opts = {}) {
        if (!this._ids) return;
        const gen = this._gen + 1;
        try {
            await this._loadOnce(opts);
        } catch (e) {
            console.error('[extension-view] view load failed:', e);
            if (this._gen === gen && this._phase === 'loading') this._fail();
        }
    }

    async _loadOnce({ updatedTo = null }) {
        const gen = ++this._gen;
        const stale = () => gen !== this._gen;
        this._dropSandbox();
        this._phase = 'loading';
        this._stateKind = null;
        this._pendingReopen = null;
        this._recheck = false;
        // The subtitle belonged to the view instance that is gone.
        this._subtitle = null;
        this._chrome.setSubtitle(null);
        this._chrome.hideState();
        this._chrome.hideBanner();
        this._chrome.showLoading(this._viewName());
        const { extensionId: id, viewId } = this._ids;

        let boot;
        try {
            boot = await this._d.invoke('ext_view_boot');
        } catch (e) {
            if (!stale()) this._showBootError(e);
            return;
        }
        if (stale()) return;
        if (boot?.extension_id !== id || boot?.view_id !== viewId) {
            console.error(`[extension-view] boot record doesn't match '${this._label}'`);
            this._fail();
            return;
        }
        const seq = Number(boot.open_seq);
        if (Number.isFinite(seq)) this._seq = Math.max(this._seq, seq);

        let items;
        let config;
        try {
            [items, config] = await Promise.all([
                this._d.invoke('list_extensions'),
                this._d.getConfig(),
            ]);
        } catch (e) {
            if (stale()) return;
            console.warn(`[ext ${id}] view load failed: couldn't read extensions or config`, e);
            this._fail();
            return;
        }
        if (stale()) return;
        const item = (Array.isArray(items) ? items : []).find((i) => i?.manifest?.id === id);
        if (!item) {
            this._showState(
                'unknown_extension',
                this.t('errors.ext_view.unknown_extension', { id }),
                ['close']
            );
            return;
        }
        const manifest = item.manifest;
        const i18n = await fetchExtensionLocaleViaInvoke(this._d.invoke, manifest);
        if (stale()) return;
        const localized = applyManifestI18n(manifest, i18n.catalog, i18n.fallback);
        const display = extensionDisplay({ manifest, localizedManifest: localized }, id);
        const extensionName = this._clean(display.name, SUBTITLE_MAX_CHARS) || id;

        if (item.enabled === false || !isExtensionEnabled(config, id)) {
            this._showState(
                'disabled',
                this.t('errors.ext_view.disabled', { extension: extensionName }),
                ['settings']
            );
            return;
        }
        const capabilities = grantedCapabilities(manifest, config);
        if (!capabilities.includes('views')) {
            this._showState(
                'not_granted',
                this.t('errors.ext_view.not_granted', { extension: extensionName }),
                ['approve', 'settings']
            );
            return;
        }
        const decl = this._findDecl(manifest, viewId);
        if (!decl) {
            this._showState(
                'removed',
                this.t('ext_view.state.removed', { extension: extensionName }),
                ['close']
            );
            return;
        }
        const viewTitle = this._resolveTitle(decl.title, i18n, extensionName) || viewId;
        const extConfig = resolveExtensionConfig(config, id, manifest);
        this._current = {
            extensionId: id,
            viewId,
            manifest,
            version: manifest.version ?? null,
            capabilities,
            capsSig: [...capabilities].sort().join(','),
            configSig: JSON.stringify(extConfig),
            declSig: JSON.stringify(decl),
            decl,
            viewTitle,
            extensionName,
        };
        this._chrome.setIdentity({ icon: decl.icon || display.icon, extensionName, viewTitle });
        this._applyWindowTitle();
        this._chrome.showLoading(viewTitle);

        const source = await this._readFile(id, decl.module);
        if (stale()) return;
        if (typeof source !== 'string') {
            this._fail();
            return;
        }
        const sharedSources = await fetchSharedSourcesViaInvoke(
            this._d.invoke,
            id,
            { view: source },
            { view: decl.module }
        );
        if (stale()) return;
        const viewCss = await this._readCss(id, decl.css);
        if (stale()) return;
        // The theme this sandbox starts with; later changes go as viewTheme.
        this._sentTheme = readThemeSnapshot({
            doc: this._doc,
            win: this._win,
            config,
            isSafeTokenValue: this._d.policy.isSafeTokenValue,
        });

        const iframeTitle = this.t('ext_view.iframe_label', {
            view: viewTitle,
            extension: extensionName,
        });
        // The hooks check that the sandbox asking is still this page's.
        const ref = { sb: null };
        let sb;
        try {
            sb = this._d.createSandbox(
                {
                    extensionId: id,
                    capabilities,
                    config: extConfig,
                    sources: { view: source, sourcePaths: { view: decl.module } },
                    sharedSources,
                    i18nCatalog: i18n.catalog,
                    i18nFallback: i18n.fallback,
                    i18nLanguage: i18n.language,
                    i18nRtl: i18n.rtl,
                    hostFeatures: hostFeaturesFor(this._d.hostFeatures, capabilities),
                    surface: 'view',
                    viewId,
                    views: manifest?.contributes?.views,
                    viewTitle,
                    extensionName,
                    viewTheme: this._sentTheme,
                    viewCss,
                    viewHooks: this._viewHooks(ref),
                },
                this._chrome.stage
            );
        } catch (e) {
            console.warn(`[ext ${id}] view sandbox couldn't be created:`, errMessage(e));
            this._fail();
            return;
        }
        ref.sb = sb;
        this._sandbox = sb;
        try {
            await sb.start();
        } catch (e) {
            if (stale()) return;
            console.warn(`[ext ${id}] view sandbox failed to start:`, errMessage(e));
            this._fail();
            return;
        }
        if (stale()) return;
        this._sandboxStarted = true;
        this._chrome.frame?.setAttribute('title', iframeTitle);

        try {
            await sb.call(
                'viewMount',
                {
                    params: boot.params ?? null,
                    source: normalizeViewSource(boot.source),
                    reloaded: boot.reloaded === true,
                },
                { timeoutMs: MOUNT_TIMEOUT_MS }
            );
        } catch (e) {
            if (stale()) return;
            console.warn(`[ext ${id}] view mount failed:`, errMessage(e));
            this._fail();
            return;
        }
        if (stale()) return;
        this._phase = 'mounted';
        this._chrome.hideLoading();
        this._applyWindowTitle();
        this._focusFrame(true);

        this._visible = await this._isVisible();
        if (stale()) return;
        if (this._visible) this._watchdog.start();
        else this._send('viewVisibility', { visible: false });
        this._flushReopen();
        if (updatedTo) {
            this._chrome.flashNotice(this.t('ext_view.state.updated', { version: updatedTo }));
        }
        // The theme, config or install may have moved on while we loaded.
        this._scheduleThemeRefresh();
        if (this._recheck) {
            this._recheck = false;
            this._onConfigChanged();
            this._onExtensionsChanged();
        }
    }

    async _readFile(id, path) {
        try {
            const text = await this._d.invoke('read_extension_file', {
                extensionId: id,
                kind: 'extension',
                filePath: stripDotSlash(path),
            });
            return typeof text === 'string' ? text : null;
        } catch (e) {
            console.warn(`[ext ${id}] couldn't read '${path}':`, errMessage(e));
            return null;
        }
    }

    /** The view's CSS texts, each ≤ 256 KiB and ≤ 1 MiB together (§4). */
    async _readCss(id, paths) {
        const out = [];
        let total = 0;
        for (const path of Array.isArray(paths) ? paths : []) {
            const text = await this._readFile(id, path);
            if (text === null) continue;
            const size = utf8Length(text);
            if (size > CSS_FILE_MAX_BYTES) {
                console.warn(`[ext ${id}] view css '${path}' is over 256 KiB — ignored`);
                continue;
            }
            if (total + size > CSS_TOTAL_MAX_BYTES) {
                console.warn(`[ext ${id}] view css '${path}' goes over 1 MiB in total — ignored`);
                continue;
            }
            total += size;
            out.push(text);
        }
        return out;
    }

    /** The manifest's declaration of `viewId`, normalised as Rust does. */
    _findDecl(manifest, viewId) {
        let decls;
        try {
            decls = this._d.policy.normalizeViews(
                manifest?.contributes?.views,
                manifest?.name ?? '',
                manifest?.icon ?? '',
                manifest?.id ?? ''
            );
        } catch (e) {
            console.warn(`[ext ${manifest?.id}] contributes.views unreadable:`, e);
            return null;
        }
        return (Array.isArray(decls) ? decls : []).find((d) => d?.id === viewId) || null;
    }

    /** Resolve a `__MSG_key__` title, then clean and cap it. */
    _resolveTitle(title, i18n, fallback) {
        let text = typeof title === 'string' ? title : '';
        const token = /^__MSG_(.+)__$/.exec(text);
        if (token) text = resolveExtensionMessage(token[1], i18n.catalog, i18n.fallback, '');
        return this._clean(text, TITLE_MAX_CHARS) || fallback;
    }

    /** Control and bidi-override characters removed, at most `max` characters. */
    _clean(text, max) {
        if (typeof text !== 'string' || !text) return '';
        try {
            const out = this._d.policy.sanitizeViewTitle(text, max);
            return typeof out === 'string' ? out : '';
        } catch {
            return '';
        }
    }

    async _isVisible() {
        try {
            return (await this._d.appWindow.isVisible()) !== false;
        } catch {
            return true;
        }
    }

    // --- States ------------------------------------------------------------

    _dropSandbox() {
        this._watchdog.reset();
        const sb = this._sandbox;
        this._sandbox = null;
        this._sandboxStarted = false;
        if (!sb) return;
        try {
            sb.destroy();
        } catch (e) {
            console.warn('[extension-view] sandbox destroy failed:', e);
        }
    }

    /** Replace the view with one of the §18.3 panels. */
    _showState(kind, message, actions) {
        this._gen += 1; // stop any load still in flight
        this._dropSandbox();
        this._phase = 'state';
        this._stateKind = kind;
        this._pendingReopen = null;
        this._chrome.hideBanner();
        this._chrome.showState({ message, actions });
    }

    _fail() {
        this._showState(
            'load_failed',
            this.t('ext_view.state.load_failed', { view: this._viewName() }),
            ['reload', 'settings']
        );
    }

    /** Map an `ext_view_boot` refusal to its panel. The detail goes to the log. */
    _showBootError(e) {
        const key = typeof e?.key === 'string' ? e.key : '';
        console.warn(
            `[extension-view] ext_view_boot refused for '${this._label}': ${e?.kind || 'error'} ${key}`
        );
        const extension = this._current?.extensionName;
        switch (key) {
            case 'errors.ext_view.not_granted':
                this._showState('not_granted', errMessage(e), ['approve', 'settings']);
                return;
            case 'errors.ext_view.disabled':
                this._showState('disabled', errMessage(e), ['settings']);
                return;
            case 'errors.ext_view.unknown_view':
                this._showState(
                    'removed',
                    extension ? this.t('ext_view.state.removed', { extension }) : errMessage(e),
                    ['close']
                );
                return;
            case 'errors.ext_view.unknown_extension':
                this._showState('unknown_extension', errMessage(e), ['close']);
                return;
            case 'errors.ext_view.not_a_view_window':
                this._showState(
                    'load_failed',
                    this.t('ext_view.state.load_failed', { view: this._viewName() }),
                    ['close']
                );
                return;
            default:
                this._fail();
        }
    }

    _onNavigated(sb) {
        if (sb !== this._sandbox) return;
        console.warn(
            `[ext ${this._ids.extensionId}] view frame navigated away from its document — stopped`
        );
        this._showState(
            'navigated',
            this.t('ext_view.state.navigated', { view: this._viewName() }),
            ['reload']
        );
    }

    _viewName() {
        return this._current?.viewTitle || this._ids?.viewId || '';
    }

    _applyWindowTitle() {
        const title = windowTitle(this.t, {
            view: this._viewName(),
            extension: this._current?.extensionName || this._ids?.extensionId || '',
            subtitle: this._subtitle,
        });
        this._doc.title = title;
        Promise.resolve(this._d.appWindow.setTitle?.(title)).catch((e) =>
            console.warn('[extension-view] setTitle failed:', e)
        );
    }

    // --- Host buttons ------------------------------------------------------

    _onAction(action) {
        switch (action) {
            case 'reload':
                this._load();
                break;
            case 'settings':
                this._openSettings().catch((e) =>
                    console.warn('[extension-view] open settings failed:', e)
                );
                break;
            case 'close':
                this._close();
                break;
            case 'approve':
                // The Store's card shows Approve for an extension whose grant
                // doesn't cover what it asks for.
                Promise.resolve(this._d.invoke('open_store_window', { tab: 'extensions' })).catch(
                    (e) => console.warn('[extension-view] open store failed:', e)
                );
                break;
        }
    }

    /** Close hides: Rust turns CloseRequested into a hide for view windows. */
    _close() {
        Promise.resolve(this._d.appWindow.close()).catch((e) =>
            console.warn('[extension-view] close failed:', e)
        );
        return null;
    }

    /** This extension's settings page. The id is the window's, never the view's. */
    async _openSettings() {
        await this._d.invoke('open_settings_window', {
            section: `ext-${this._ids.extensionId}`,
        });
        return null;
    }

    // --- Events --------------------------------------------------------------

    async _subscribe() {
        const { listen } = this._d;
        const subscribe = (event, handler, on = listen) =>
            Promise.resolve()
                .then(() => on(event, handler))
                .catch((e) => console.warn(`[extension-view] listen(${event}) failed:`, e));
        // Rust sends these two to this window only. A global listen hears
        // the copy sent to every other view window too (Tauri hands an
        // event to each listener whose target is `Any`), which would give
        // this view another window's params and hides. So listen on this
        // window.
        const appWindow = this._d.appWindow;
        const onThisWindow = (event, handler) => {
            if (typeof appWindow?.listen !== 'function') {
                throw new Error('this window has no listen()');
            }
            return appWindow.listen(event, handler);
        };
        await Promise.all([
            subscribe(EVT.EXT_VIEW_REOPEN, (e) => this._onReopen(e?.payload), onThisWindow),
            subscribe(EVT.EXT_VIEW_VISIBILITY, (e) => this._onVisibility(e?.payload), onThisWindow),
            subscribe(EVT.EXTENSIONS_CHANGED, () => this._onExtensionsChanged()),
        ]);
        this._d.onConfigChange(() => this._onConfigChanged());
        // i18n.js re-fetches the catalog on config_updated and fires this
        // when the language changed: reload so every string is new (§12).
        this._doc.addEventListener('kage:i18n-changed', () => {
            if (this._stateKind === 'navigated') {
                this._chrome.showState({
                    message: this.t('ext_view.state.navigated', { view: this._viewName() }),
                    actions: ['reload'],
                });
                return;
            }
            if (this._ids) this._load();
        });
        try {
            Promise.resolve(
                this._d.appWindow.onFocusChanged?.(({ payload }) => {
                    if (payload === true) this._focusFrame(false);
                })
            ).catch((e) => console.warn('[extension-view] focus listener failed:', e));
        } catch (e) {
            console.warn('[extension-view] focus listener failed:', e);
        }
        try {
            Promise.resolve(
                this._d.installDataEvents({
                    listen,
                    myLabel: this._label,
                    deliver: (a, b) => this._deliverData(a, b),
                })
            ).catch((e) => console.warn('[extension-view] data events failed:', e));
        } catch (e) {
            console.warn('[extension-view] data events failed:', e);
        }
    }

    /** `ext_view_reopen { params, open_seq, source }` (§6.3). */
    _onReopen(payload) {
        if (!this._ids) return;
        const seq = Number(payload?.open_seq);
        if (!Number.isFinite(seq) || seq <= this._seq) return;
        const reopen = {
            seq,
            params: payload?.params ?? null,
            source: normalizeViewSource(payload?.source),
        };
        if (this._phase === 'loading' || this._phase === 'idle') {
            if (!this._pendingReopen || seq > this._pendingReopen.seq) this._pendingReopen = reopen;
            return;
        }
        this._seq = seq;
        const wasHidden = !this._visible;
        this._visible = true;
        // Not running (a panel shows), or frozen when it was last seen: start
        // again. ext_view_boot hands the newest params over.
        if (this._phase !== 'mounted' || this._watchdog.lastPingFailed) {
            this._load();
            return;
        }
        if (wasHidden) {
            this._watchdog.start();
            this._send('viewVisibility', { visible: true });
        }
        this._send('viewReopen', { params: reopen.params, source: reopen.source });
    }

    _flushReopen() {
        const reopen = this._pendingReopen;
        this._pendingReopen = null;
        if (!reopen || reopen.seq <= this._seq) return;
        this._seq = reopen.seq;
        this._send('viewReopen', { params: reopen.params, source: reopen.source });
    }

    /** `ext_view_visibility { visible }`: the window was hidden or shown. */
    _onVisibility(payload) {
        if (typeof payload?.visible !== 'boolean') return;
        const visible = payload.visible;
        if (visible === this._visible) return;
        this._visible = visible;
        if (this._phase !== 'mounted') return;
        if (visible && this._watchdog.lastPingFailed) {
            this._load();
            return;
        }
        if (visible) this._watchdog.start();
        else this._watchdog.stop();
        this._send('viewVisibility', { visible });
    }

    /** An RPC whose answer doesn't matter; failures go to the log. */
    _send(method, params) {
        const sb = this._sandbox;
        if (!sb || !this._sandboxStarted) return;
        Promise.resolve()
            .then(() => sb.call(method, params))
            .catch((e) =>
                console.warn(`[ext ${this._ids?.extensionId}] ${method} failed:`, errMessage(e))
            );
    }

    async _onConfigChanged() {
        try {
            await this._d.applyTheme();
        } catch (e) {
            console.warn('[extension-view] theme failed:', e);
        }
        let config;
        try {
            config = await this._d.getConfig();
        } catch {
            return;
        }
        this._refreshTheme(config);
        if (this._phase === 'loading') {
            this._recheck = true;
            return;
        }
        if (
            this._phase === 'state' &&
            (this._stateKind === 'disabled' || this._stateKind === 'not_granted')
        ) {
            this._load();
            return;
        }
        const cur = this._current;
        if (this._phase !== 'mounted' || !cur) return;
        // Rust destroys the window when the extension is turned off or loses
        // `views`; this covers the moment in between.
        if (!isExtensionEnabled(config, cur.extensionId)) {
            this._showState(
                'disabled',
                this.t('errors.ext_view.disabled', { extension: cur.extensionName }),
                ['settings']
            );
            return;
        }
        const capabilities = grantedCapabilities(cur.manifest, config);
        if (!capabilities.includes('views')) {
            this._showState(
                'not_granted',
                this.t('errors.ext_view.not_granted', { extension: cur.extensionName }),
                ['approve', 'settings']
            );
            return;
        }
        if ([...capabilities].sort().join(',') !== cur.capsSig) {
            this._load();
            return;
        }
        const extConfig = resolveExtensionConfig(config, cur.extensionId, cur.manifest);
        const sig = JSON.stringify(extConfig);
        if (sig === cur.configSig) return;
        cur.configSig = sig;
        this._sandbox?.updateConfig?.(extConfig);
    }

    async _onExtensionsChanged() {
        if (!this._ids) return;
        let items;
        try {
            items = await this._d.invoke('list_extensions');
        } catch {
            return;
        }
        if (this._phase === 'loading') {
            this._recheck = true;
            return;
        }
        const { extensionId: id, viewId } = this._ids;
        const item = (Array.isArray(items) ? items : []).find((i) => i?.manifest?.id === id);
        if (!item) {
            // Uninstalled. Rust destroys the window; until then say so.
            if (this._stateKind !== 'unknown_extension') {
                this._showState(
                    'unknown_extension',
                    this.t('errors.ext_view.unknown_extension', { id }),
                    ['close']
                );
            }
            return;
        }
        if (this._phase === 'state') {
            // An update or reinstall may have fixed what the panel says. A
            // stopped navigation waits for the user's Reload.
            if (this._stateKind !== 'navigated') this._load();
            return;
        }
        const cur = this._current;
        if (this._phase !== 'mounted' || !cur) return;
        const version = item.manifest?.version ?? null;
        if (version !== cur.version) {
            this._load({ updatedTo: version });
            return;
        }
        // Same version, different declaration (a reinstall during
        // development): reload without the "updated" note.
        if (JSON.stringify(this._findDecl(item.manifest, viewId)) !== cur.declSig) this._load();
    }

    /** `extension_data_changed`, already filtered and given `self` (§10.1). */
    _deliverData(a, b) {
        const extensionId = typeof a === 'string' ? a : (a?.extension_id ?? a?.extensionId);
        const changes = typeof a === 'string' ? b : a?.changes;
        if (!this._ids || extensionId !== this._ids.extensionId) return;
        const sb = this._sandbox;
        if (!sb || !this._sandboxStarted) return;
        if (!this._current?.capabilities.includes('storage')) return;
        if (!Array.isArray(changes) || changes.length === 0) return;
        try {
            sb.notifyStorageChanged(changes);
        } catch (e) {
            console.warn(`[ext ${extensionId}] storage-changed failed:`, e);
        }
    }

    // --- Requests from the view (§8.6) ---------------------------------------

    _refuse(kind, key, vars) {
        const error = new Error(this.t(key, vars));
        error.kind = kind;
        error.key = key;
        return error;
    }

    /** {@link recentUserActivation} for this window. */
    _hasRecentActivation() {
        const focused = this._d.hasFocus ? this._d.hasFocus() : this._doc.hasFocus();
        return recentUserActivation(focused, this._d.userActivation?.());
    }

    /**
     * What the sandbox host calls for a view's requests (§8.6). The host
     * type-checks each request and applies the rate limits and the user
     * activation rule (`hasUserActivation`) first; these act on it. A
     * request from a sandbox this page has replaced is refused.
     */
    _viewHooks(ref) {
        const live = () => {
            if (!ref.sb || ref.sb !== this._sandbox || !this._sandboxStarted || !this._current) {
                throw this._refuse('not_permitted', 'errors.ext_view.not_a_view_window');
            }
            return this._current;
        };
        return {
            setTitle: (subtitle) => {
                live();
                this._subtitle = this._clean(subtitle ?? '', SUBTITLE_MAX_CHARS) || null;
                this._chrome.setSubtitle(this._subtitle);
                this._applyWindowTitle();
            },
            close: () => {
                live();
                this._close();
            },
            // Rust repeats every check against the installed manifest and
            // grant, and checks the params' size and depth.
            openView: (viewId, params) =>
                this._d.invoke('ext_view_open', {
                    extensionId: live().extensionId,
                    viewId,
                    params: params ?? null,
                    source: 'view',
                }),
            // Rust checks focus, the text and the rate again.
            compose: (text) => {
                live();
                return this._d.invoke('ext_view_compose', { text });
            },
            copyText: (text) => {
                live();
                return this._d.invoke('ext_view_copy_text', { text });
            },
            openSettings: () => {
                live();
                return this._openSettings();
            },
            onNavigated: () => this._onNavigated(ref.sb),
            hasUserActivation: () => this._hasRecentActivation(),
        };
    }

    // --- Focus, theme, guards ------------------------------------------------

    /** Keys go to the view without a click (§13.2). */
    _focusFrame(force) {
        if (this._phase !== 'mounted') return;
        const frame = this._chrome.frame;
        if (!frame) return;
        const active = this._doc.activeElement;
        // Leave a host bar button the user tabbed to alone.
        if (!force && active && active !== this._doc.body && active !== frame) return;
        try {
            frame.focus();
        } catch {}
    }

    /**
     * Theme changes come from config_updated (Settings, a theme extension)
     * and from the theme module's OS listener. Both end up changing the
     * body's class or the root's inline variables, so watch those.
     */
    _watchTheme() {
        const schedule = () => this._scheduleThemeRefresh();
        try {
            const observer = new this._win.MutationObserver(schedule);
            observer.observe(this._doc.documentElement, {
                attributes: true,
                attributeFilter: ['style', 'class'],
            });
            if (this._doc.body) {
                observer.observe(this._doc.body, { attributes: true, attributeFilter: ['class'] });
            }
        } catch (e) {
            console.warn('[extension-view] theme observer failed:', e);
        }
        try {
            this._win
                .matchMedia?.('(prefers-reduced-motion: reduce)')
                ?.addEventListener?.('change', schedule);
        } catch {}
    }

    _scheduleThemeRefresh() {
        if (this._themeQueued) return;
        this._themeQueued = true;
        setTimeout(() => {
            this._themeQueued = false;
            this._refreshTheme();
        }, 0);
    }

    /** Send `viewTheme` only when the snapshot changed (§11). */
    async _refreshTheme(config) {
        let cfg = config;
        if (!cfg) {
            try {
                cfg = await this._d.getConfig();
            } catch {
                cfg = null;
            }
        }
        const next = readThemeSnapshot({
            doc: this._doc,
            win: this._win,
            config: cfg,
            isSafeTokenValue: this._d.policy.isSafeTokenValue,
        });
        // Before start() the next sandbox picks the theme up when it's built.
        if (!this._sandbox || !this._sandboxStarted) return;
        if (sameThemeSnapshot(this._sentTheme, next)) return;
        this._sentTheme = next;
        this._send('viewTheme', { theme: next });
    }

    _installGuards() {
        const doc = this._doc;
        // No page zoom from a pinch or ctrl + wheel over the host bar (§13.1).
        this._win.addEventListener(
            'wheel',
            (e) => {
                if (e.ctrlKey) e.preventDefault();
            },
            { passive: false }
        );
        for (const type of ['gesturestart', 'gesturechange', 'gestureend']) {
            doc.addEventListener(type, (e) => e.preventDefault());
        }
        // Files dropped on the host page never navigate it. Tauri's native
        // handler intercepts OS drops; the page ignores its drag-drop events
        // (§13.5), so a path never reaches the view.
        doc.addEventListener('dragover', (e) => e.preventDefault());
        doc.addEventListener('drop', (e) => e.preventDefault());
        // ⌘W / Ctrl+W while focus is in the host bar. Inside the view the
        // runtime asks for the same with a `close` request (§13.2).
        doc.addEventListener('keydown', (e) => {
            if (!cmdOrCtrlPressed(e) || e.altKey || e.shiftKey) return;
            if (e.key !== 'w' && e.key !== 'W') return;
            e.preventDefault();
            this._close();
        });
    }
}
