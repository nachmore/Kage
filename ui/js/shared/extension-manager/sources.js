import { normalizePermissions } from '../extension-permissions.js';
import { resolveExtensionCatalog } from './i18n.js';
import { applyMixin } from '../mixin.js';

const SANDBOX_VENDOR_ALLOWLIST = {
    math: 'vendor/lib/math.js',
};

// Extension CSS is injected into the trusted host document, so it must not
// be able to restyle the permission UI (hide Deny, swap buttons, overlay
// it) or phone home via @import/url(). Parsed with the browser's own CSS
// parser in an inert document (no resource loads), then filtered rule by
// rule. Not a full scoping solution — it targets the redress and beacon
// vectors without breaking extensions that style their own rows/widgets.
const PERMISSION_UI_SELECTOR = /permission|kage-ext-perm/i;
// data: URIs cause no network request; anything else (incl. image-set
// strings) can beacon.
const NON_DATA_URL = /url\(\s*(?!['"]?data:)|image-set\(/i;

// Pseudo-elements never match in matches()/querySelector(), so
// `button::after { content: 'Deny' }` would sail through. Strip them for
// the probe; a bare one (`::before`) means `*::before`.
const PSEUDO_ELEMENT = /::[\w-]+(?:\([^)]*\))?|:(?:before|after|first-line|first-letter)\b/gi;

// Overlay cap. The permission modal sits at z-index 10000 (floating) / 100
// (chat-scoped), so anything an extension stacks at or below this paints
// under it no matter how it's positioned. Leaves room for a widget's own
// dropdown/tooltip layering.
const MAX_EXTENSION_Z_INDEX = 10;
// Offsets that let a fixed/sticky box cover arbitrary host UI.
const OFFSET_PROPS = [
    'inset',
    'inset-block',
    'inset-inline',
    'inset-block-start',
    'inset-block-end',
    'inset-inline-start',
    'inset-inline-end',
    'top',
    'right',
    'bottom',
    'left',
];
// Functional pseudo-classes whose argument list stands in for the compound
// they sit in (`:is(body)` targets body). :not/:has deliberately excluded.
const MATCHES_ANY_PSEUDO = /:(?:is|where|matches|-webkit-any)\($/i;

// Selector parts whose match flips after load: interaction/form state,
// language/direction, and the classes the host toggles on html/body
// (theme, rtl, clipboard mode, animations). The live-DOM probe runs once,
// so `button:hover` or `.light-theme::after` would miss while the modal is
// idle / the theme is dark. Treat them as always-true.
const STATE_SELECTOR =
    /:(?:dir|lang|state)\([^)]*\)|:(?:hover|active|focus(?:-visible|-within)?|target(?:-within)?|visited|any-link|link|checked|indeterminate|default|enabled|disabled|read-only|read-write|placeholder-shown|valid|invalid|user-valid|user-invalid|in-range|out-of-range|required|optional|-webkit-autofill|autofill|empty|popover-open|modal|open|fullscreen|playing|paused|defined)(?![\w-])|\[\s*(?:lang|dir)(?![\w-])[^\]]*\]|\.(?:light-theme|dark-theme|rtl|clipboard-mode|animations-paused)(?![\w-])/gi;

// `*` where the removed token stood alone in its compound, '' otherwise.
const stand = (offset, s) => (offset === 0 || /[\s>+~,(]/.test(s[offset - 1]) ? '*' : '');

function stripPseudoElements(selectorText) {
    return selectorText.replace(PSEUDO_ELEMENT, (_m, offset, s) => stand(offset, s));
}

// A superset of `sel` for one-shot probing: pseudo-elements, :not() (only
// ever narrows) and state-dependent parts removed.
function broadenSelector(sel) {
    let out = sel;
    for (let i = out.search(/:not\(/i); i >= 0; i = out.search(/:not\(/i)) {
        let end = out.length;
        scanSelector(out.slice(i), (j, c, parens) => {
            if (c === ')' && parens === 0 && end === out.length) end = i + j;
        });
        out = out.slice(0, i) + stand(i, out) + out.slice(end + 1);
    }
    return stripPseudoElements(out).replace(STATE_SELECTOR, (_m, offset, s) => stand(offset, s));
}

// Selector strings are serialized by the browser, so a small quote- and
// bracket-aware walk is enough to split them structurally. `cb` sees every
// unquoted, unescaped character with the current ()/[] depths.
function scanSelector(sel, cb) {
    let parens = 0;
    let brackets = 0;
    let quote = null;
    for (let i = 0; i < sel.length; i++) {
        const c = sel[i];
        if (c === '\\') {
            i++;
            continue;
        }
        if (quote) {
            if (c === quote) quote = null;
            continue;
        }
        if (c === '"' || c === "'") {
            quote = c;
            continue;
        }
        if (c === '(') parens++;
        else if (c === ')') parens--;
        else if (c === '[') brackets++;
        else if (c === ']') brackets--;
        cb(i, c, parens, brackets);
    }
}

// Top-level comma split: `a, :is(b, c)` → ['a', ':is(b, c)'].
function splitSelectorList(sel) {
    const parts = [];
    let start = 0;
    scanSelector(sel, (i, c, parens, brackets) => {
        if (c === ',' && parens === 0 && brackets === 0) {
            parts.push(sel.slice(start, i).trim());
            start = i + 1;
        }
    });
    parts.push(sel.slice(start).trim());
    return parts.filter(Boolean);
}

// Complex selector → [{ comb, text }] compounds; `comb` is the combinator
// in front of the compound (null for the first, ' ' for descendant).
function splitCompounds(item) {
    const out = [];
    let buf = '';
    let pending = null;
    let last = 0;
    const flush = () => {
        if (!buf) return;
        out.push({ comb: out.length ? pending || ' ' : null, text: buf });
        buf = '';
        pending = null;
    };
    scanSelector(item, (i, c, parens, brackets) => {
        // Quoted/escaped characters the scanner skipped belong to the
        // current compound verbatim.
        buf += item.slice(last, i);
        last = i + 1;
        if (parens === 0 && brackets === 0 && /[\s>+~]/.test(c)) {
            flush();
            if (!/\s/.test(c)) pending = c;
        } else {
            buf += c;
        }
    });
    buf += item.slice(last);
    flush();
    return out;
}

// Compound text with every (...) argument removed, and optionally every
// [...] attribute, so tests only see what applies to the compound itself.
function compoundTopLevel(text, { keepBrackets = false } = {}) {
    let out = '';
    let last = 0;
    let outside = true;
    scanSelector(text, (i, c, parens, brackets) => {
        if (outside) out += text.slice(last, i);
        last = i + 1;
        const nowOutside = parens === 0 && (keepBrackets || brackets === 0);
        // Keep the delimiters themselves so `:is()` / `[]` stay visible.
        if (nowOutside || outside) out += c;
        outside = nowOutside;
    });
    if (outside) out += text.slice(last);
    return out;
}

// Arguments of top-level :is()/:where() in a compound.
function matchesAnyArgs(text) {
    const args = [];
    let open = -1;
    scanSelector(text, (i, c, parens) => {
        if (c === '(' && parens === 1) {
            open = MATCHES_ANY_PSEUDO.test(text.slice(0, i + 1)) ? i + 1 : -1;
        } else if (c === ')' && parens === 0 && open >= 0) {
            args.push(text.slice(open, i));
            open = -1;
        }
    });
    return args;
}

// Turn a nested rule's selector into the absolute selector it matches.
// Chromium serializes nested selectors with an explicit `&`; a relative
// one without it means `& <sel>`. Top-level `&` is `:scope` = the root.
function resolveNestedSelector(selectorText, parentSelector) {
    const parent = parentSelector === undefined ? ':root' : `:is(${parentSelector})`;
    return splitSelectorList(selectorText)
        .map((item) => {
            let out = '';
            let last = 0;
            let hasAmp = false;
            scanSelector(item, (i, c, _parens, brackets) => {
                if (c === '&' && brackets === 0) {
                    out += item.slice(last, i) + parent;
                    last = i + 1;
                    hasAmp = true;
                }
            });
            out += item.slice(last);
            return hasAmp || parentSelector === undefined ? out : `${parent} ${item}`;
        })
        .join(', ');
}

// Does any branch of the selector have html/body/:root as its subject
// (incl. their pseudo-elements: `body::after` is a full-viewport overlay
// waiting to happen)? Non-subject uses (`body.light-theme .card`) stay.
function selectorTargetsDocumentRoot(sel) {
    for (const item of splitSelectorList(stripPseudoElements(sel))) {
        const compounds = splitCompounds(item);
        const subject = compounds[compounds.length - 1]?.text || '';
        const top = compoundTopLevel(subject);
        if (/^(?:html|body)(?![\w-])/i.test(top) || /:(?:root|scope)(?![\w-])/i.test(top)) {
            return true;
        }
        if (matchesAnyArgs(subject).some(selectorTargetsDocumentRoot)) return true;
    }
    // Structural selectors (`:not(.x)`, `:first-child`, `[lang]`) can land
    // on the root without naming it — ask the live DOM. Unparseable → drop.
    const probe = broadenSelector(sel);
    try {
        return document.documentElement.matches(probe) || !!document.body?.matches(probe);
    } catch {
        return true;
    }
}

// Widget hosts carry `data-ext-widget-key="<extId>:<widgetId>"` (set by
// the host in ui.js, not the extension), so this attribute is the one
// trustworthy "own container" marker.
function compoundIsExtensionContainer(text, id) {
    const top = compoundTopLevel(text, { keepBrackets: true });
    const attr = /\[\s*data-ext-widget-key\s*(\^?=)\s*(["'])(.*?)\2\s*\]/gi;
    for (const m of top.matchAll(attr)) {
        const value = m[3].replace(/\\(.)/g, '$1');
        // `^="acme:"` / `^="acme:bar"` / `="acme:bar"` all stay inside acme.
        if (value.startsWith(`${id}:`)) return true;
    }
    const args = matchesAnyArgs(text);
    return args.length > 0 && args.every((a) => selectorScopedToExtension(a, id));
}

// Every branch must pass through the extension's container and only
// descend from there (`~`/`+` after it would reach host siblings).
function selectorScopedToExtension(sel, id) {
    return splitSelectorList(sel).every((item) => {
        const compounds = splitCompounds(item);
        const k = compounds.findIndex((c) => compoundIsExtensionContainer(c.text, id));
        if (k < 0) return false;
        return compounds.slice(k + 1).every((c) => c.comb === ' ' || c.comb === '>');
    });
}

function zIndexTooHigh(value) {
    const v = value.trim().toLowerCase();
    if (!v || v === 'auto' || /^(?:initial|unset|revert|revert-layer)$/.test(v)) return false;
    // var()/calc()/inherit can't be bounded statically — treat as over.
    if (!/^[+-]?\d+$/.test(v)) return true;
    return Number(v) > MAX_EXTENSION_Z_INDEX;
}

// 'fixed' escapes every container; var()/env() could resolve to it.
function positionEscapes(value) {
    const v = value.trim().toLowerCase();
    return v === 'fixed' || /\(/.test(v);
}

// Reason string when a style block could overlay host UI, else null.
// `scoped` relaxes only positioning: a fixed box inside the extension's
// container is still capped by z-index, which no selector can contain.
function overlayReason(style, scoped) {
    if (zIndexTooHigh(style.getPropertyValue('z-index'))) return 'z-index';
    if (scoped) return null;
    const position = style.getPropertyValue('position');
    if (positionEscapes(position)) return 'position';
    if (position.trim().toLowerCase() === 'sticky') {
        const z = style.getPropertyValue('z-index').trim().toLowerCase();
        if ((z && z !== 'auto') || OFFSET_PROPS.some((p) => style.getPropertyValue(p))) {
            return 'position';
        }
    }
    return null;
}

function ruleTouchesPermissionUi(selectorText) {
    if (PERMISSION_UI_SELECTOR.test(selectorText)) return true;
    // Generic selectors (`button`, `div > *`) reach the modal without
    // naming it — test against the live modal DOM. Ancestors count too:
    // `opacity`, `direction`, custom properties etc. inherit into the
    // modal (`direction: rtl` on its container swaps Allow/Deny).
    // Unparseable → drop.
    const modal = document.getElementById('permissionModal');
    if (!modal) return false;
    const probe = broadenSelector(selectorText);
    try {
        return (
            modal.matches(probe) ||
            !!modal.querySelector(probe) ||
            !!modal.parentElement?.closest(probe)
        );
    } catch {
        return true;
    }
}

// `parentSelector` is the resolved selector of the enclosing style rule
// (undefined at top level / inside @keyframes). Grouping rules pass it
// through unchanged; nested style rules resolve against it so `& button`
// inside `.my-widget` is judged as `.my-widget button`, not as a bare
// `& button` (= `:scope button`, which matched the modal's own buttons).
function sanitizeCssRules(list, id, parentSelector) {
    for (let i = list.cssRules.length - 1; i >= 0; i--) {
        const rule = list.cssRules[i];
        const style = rule.style;
        // Nested declaration blocks (CSSNestedDeclarations) carry no
        // selector of their own — they apply to the parent's.
        const selector =
            rule.selectorText !== undefined
                ? resolveNestedSelector(rule.selectorText, parentSelector)
                : style
                  ? parentSelector
                  : undefined;
        let drop = false;
        if (rule.type === CSSRule.IMPORT_RULE) {
            drop = true;
        } else if (typeof CSSScopeRule !== 'undefined' && rule instanceof CSSScopeRule) {
            // `@scope (body) { :scope::after {…} }` re-roots selectors in a
            // way the checks below can't see through. Rare in practice.
            drop = true;
        } else if (selector !== undefined) {
            drop =
                ruleTouchesPermissionUi(selector) ||
                selectorTargetsDocumentRoot(selector) ||
                (!!style && overlayReason(style, selectorScopedToExtension(selector, id)) !== null);
        }
        if (drop) {
            console.warn(`Extension '${id}': dropped CSS rule: ${rule.cssText.slice(0, 120)}`);
            list.deleteRule(i);
            continue;
        }
        if (style) {
            for (let j = style.length - 1; j >= 0; j--) {
                const prop = style[j];
                if (NON_DATA_URL.test(style.getPropertyValue(prop))) style.removeProperty(prop);
            }
            // Selector-less blocks (@keyframes frames) can't be dropped by
            // index, and an animation can apply them to any element — so
            // strip the overlay-capable declarations instead.
            if (selector === undefined) {
                if (zIndexTooHigh(style.getPropertyValue('z-index')))
                    style.removeProperty('z-index');
                const position = style.getPropertyValue('position').trim().toLowerCase();
                if (positionEscapes(position) || position === 'sticky')
                    style.removeProperty('position');
            }
        }
        // Grouping (@media/@supports/@layer) and nested style rules.
        if (rule.cssRules) sanitizeCssRules(rule, id, selector ?? parentSelector);
    }
}

function sanitizeExtensionCss(cssCode, id) {
    const doc = document.implementation.createHTMLDocument('');
    const el = doc.createElement('style');
    el.textContent = cssCode;
    doc.head.appendChild(el);
    const sheet = el.sheet;
    if (!sheet) return '';
    sanitizeCssRules(sheet, id);
    return Array.from(sheet.cssRules, (r) => r.cssText).join('\n');
}

export function installExtensionSourceMethods(ExtensionManager) {
    applyMixin(ExtensionManager.prototype, {
        _hasSandboxedProvider(sources) {
            return !!(
                sources.searchProvider ||
                sources.toolProvider ||
                sources.triggerProvider ||
                sources.toolbarProvider ||
                sources.messageFormatter ||
                (sources.widgets && Object.keys(sources.widgets).length > 0)
            );
        },

        async _fetchProviderSources(id, manifest) {
            const out = {};
            const c = manifest.contributes || {};
            if (c.searchProvider) out.searchProvider = await this._fetchText(id, c.searchProvider);
            if (c.toolProvider) out.toolProvider = await this._fetchText(id, c.toolProvider);
            if (c.triggerProvider)
                out.triggerProvider = await this._fetchText(id, c.triggerProvider);
            if (c.toolbarButtons) out.toolbarProvider = await this._fetchText(id, c.toolbarButtons);
            if (c.messageFormatters)
                out.messageFormatter = await this._fetchText(id, c.messageFormatters);
            if (Array.isArray(c.widgets) && c.widgets.length) {
                out.widgets = {};
                for (const w of c.widgets) {
                    if (!w?.id || !w?.module) continue;
                    out.widgets[w.id] = await this._fetchText(id, w.module);
                }
            }
            out.sharedSources = await this._fetchSharedSources(out, id);
            return out;
        },

        /**
         * Walk every fetched provider source looking for `import ... from './...'`
         * statements. Recursively fetch those files too so the sandbox can
         * wire them up as shared blob URLs (see runtime.js :
         * registerSharedModules).
         *
         * Note: this is a deliberately dumb regex-based discovery. It does
         * NOT handle dynamic `import()` expressions or conditional imports.
         * Extensions that need those should keep all their JS in a single
         * file or declare them explicitly later.
         *
         * @param {object} sources - the sources bag accumulated so far
         * @param {string} extensionId - extension id used for the read_extension_file IPC
         * @returns {Promise<object>} flat map of { "./rel/path.js": sourceText }
         */ async _fetchSharedSources(sources, extensionId) {
            const collected = new Map();
            const queue = [];

            const scan = (src) => {
                if (typeof src !== 'string') return;
                // Matches both `import X from './x.js'` and `import './x.js'`.
                const re = /\bimport\s+(?:[^'"]+?\s+from\s+)?['"](\.{1,2}\/[^'"]+?)['"]/g;
                let m;
                while ((m = re.exec(src)) !== null) {
                    const rel = m[1];
                    if (!collected.has(rel) && !queue.includes(rel)) queue.push(rel);
                }
            };

            // Seed queue from the entry-point provider sources.
            for (const [kind, val] of Object.entries(sources)) {
                if (kind === 'widgets' && val && typeof val === 'object') {
                    for (const s of Object.values(val)) scan(s);
                } else {
                    scan(val);
                }
            }

            // Breadth-first resolve — shared modules can import each other.
            while (queue.length) {
                const rel = queue.shift();
                if (collected.has(rel)) continue;
                const text = await this._fetchText(extensionId, rel);
                if (text == null) continue;
                collected.set(rel, text);
                scan(text);
            }

            if (collected.size === 0) return undefined;
            const out = {};
            for (const [k, v] of collected) out[k] = v;
            return out;
        },

        /**
         * Fetch vendor libraries declared by the extension in
         * `manifest.sandboxVendor` (an array of allow-listed basenames).
         *
         * Vendor libs are non-ES-module UMD/IIFE bundles (like mathjs) that
         * set globals when run. The sandbox runtime injects them via a
         * `<script>` tag before loading provider code so providers can rely
         * on those globals.
         *
         * Only a small allow-list is accepted — we never load arbitrary
         * paths that the extension names, because that would be a path
         * traversal vector. Unknown names are dropped with a warning.
         *
         * @returns {Promise<Record<string,string> | undefined>}
         *   Map of allow-list name → source text, or undefined if none.
         */
        /**
         * Fetch an extension's `_locales/<lang>/messages.json` via the Tauri
         * command `read_extension_locale`, which path-validates the extension
         * id and stays inside the user install dir.
         */ async _fetchExtensionLocale(id, manifest) {
            const kind = manifest?.type || 'extension';
            return resolveExtensionCatalog(async (code) => {
                try {
                    const v = await this.invoke('read_extension_locale', {
                        extensionId: id,
                        kind,
                        language: code,
                    });
                    return v && typeof v === 'object' ? v : null;
                } catch {
                    return null;
                }
            });
        },

        async _fetchVendorSources(manifest) {
            const list = Array.isArray(manifest?.sandboxVendor) ? manifest.sandboxVendor : null;
            if (!list || list.length === 0) return undefined;
            const out = {};
            for (const name of list) {
                if (typeof name !== 'string') continue;
                const url = SANDBOX_VENDOR_ALLOWLIST[name];
                if (!url) {
                    console.warn(
                        `Extension '${manifest.id}': unknown sandboxVendor '${name}', ignored`
                    );
                    continue;
                }
                try {
                    const resp = await fetch(url);
                    if (!resp.ok) {
                        console.warn(
                            `Failed to fetch vendor '${name}' from ${url}: HTTP ${resp.status}`
                        );
                        continue;
                    }
                    out[name] = await resp.text();
                } catch (e) {
                    console.warn(`Failed to fetch vendor '${name}':`, e);
                }
            }
            return Object.keys(out).length ? out : undefined;
        },

        async _fetchText(id, relPath) {
            try {
                return await this.invoke('read_extension_file', {
                    extensionId: id,
                    kind: 'extension',
                    filePath: relPath.replace('./', ''),
                });
            } catch (e) {
                console.warn(`Failed to read extension file '${id}/${relPath}':`, e);
                return null;
            }
        },

        // --- Capabilities ------------------------------------------------------

        /**
         * Compute the capabilities actually granted to this extension.
         *
         * Grant flow:
         *   1. Manifest declares requested capabilities in `permissions[]`.
         *   2. At install time the user approves that set (or uninstalls).
         *   3. We store the approved set in config under
         *      `extension_grants[<id>]` alongside the manifest version that
         *      was approved. If the manifest later requests more caps, we
         *      drop the extras until the user re-approves.
         *
         * @returns {string[]}
         */ _resolveGrantedCapabilities(manifest) {
            const id = manifest.id;
            const requested = normalizePermissions(manifest.permissions, id);

            const grants = this._configCache?.extension_grants || {};
            const record = grants[id];
            if (!record) {
                // Extension without a grant record. This shouldn't happen if
                // install went through the permission prompt, but handle it
                // defensively: grant nothing and log loudly.
                console.warn(
                    `Extension '${id}': no user grant recorded, running with no capabilities. ` +
                        `The extension may be broken until it is reinstalled.`
                );
                return [];
            }

            // Intersect: the user's grant is authoritative. If the extension
            // is updated to request new capabilities, we drop the extras
            // until the user approves them.
            const grantedSet = new Set(normalizePermissions(record.granted, id));
            return requested.filter((cap) => grantedSet.has(cap));
        },

        async _loadExtensionCss(id, manifest) {
            const cssFiles = manifest.contributes?.css;
            if (!Array.isArray(cssFiles) || cssFiles.length === 0) return;
            // Checked once, not per file: every file's <style> carries the
            // same data-ext-css, so a per-file check skipped all but the first.
            if (document.querySelector(`style[data-ext-css="${CSS.escape(id)}"]`)) return;
            for (const cssPath of cssFiles) {
                try {
                    const cssCode = await this.invoke('read_extension_file', {
                        extensionId: id,
                        kind: 'extension',
                        filePath: cssPath.replace('./', ''),
                    });
                    const style = document.createElement('style');
                    style.dataset.extCss = id;
                    style.textContent = sanitizeExtensionCss(String(cssCode ?? ''), id);
                    document.head.appendChild(style);
                    console.log(`ExtensionManager: loaded CSS for '${id}'`);
                } catch (e) {
                    console.warn(`Failed to load CSS for '${id}':`, e);
                }
            }
        },

        _getExtensionConfig(id, manifest) {
            const saved = this._configCache?.extensions?.[id];
            if (saved) return saved;
            const defaults = {};
            if (manifest.config) {
                for (const [key, schema] of Object.entries(manifest.config)) {
                    defaults[key] = schema.default;
                }
            }
            return defaults;
        },

        _isEnabled(id) {
            const states = this._configCache?.extension_states || {};
            return states[id] !== false;
        },
    });
}
