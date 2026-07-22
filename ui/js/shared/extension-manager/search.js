import { sanitizeExtensionHtml } from '../extension-html-sanitizer.js';
import { resolveExtensionMessage } from './i18n.js';
import { applyMixin } from '../mixin.js';

export function installExtensionSearchMethods(ExtensionManager) {
    applyMixin(ExtensionManager.prototype, {
        async _buildKeywordGate(lowerQuery) {
            let defs = [];
            try {
                defs = await this.getKeywordDefinitions();
            } catch (e) {
                console.warn('keyword gate: getKeywordDefinitions failed:', e);
            }
            const byExt = new Map();
            for (const d of defs) {
                if (!byExt.has(d.extensionId)) byExt.set(d.extensionId, []);
                byExt.get(d.extensionId).push(d.keyword);
            }
            return (id) => {
                const keywords = byExt.get(id);
                if (!keywords || keywords.length === 0) return true; // content matcher
                return keywords.some((kw) => lowerQuery === kw || lowerQuery.startsWith(kw + ' '));
            };
        },

        /**
         * Synchronous match, fan out to all extensions. The sandbox RPC is
         * async, so the "sync" match() becomes async too — the caller can
         * await it or treat the promise as a stream-of-results batch.
         *
         * Returns a Promise that resolves to an array of SearchResult, each
         * stamped with `_extensionId`.
         */ async matchAll(query) {
            if (query.trim().startsWith('>')) return []; // > prefix reserved for built-ins
            const lowerQuery = query.trim().toLowerCase();
            const shouldCall = await this._buildKeywordGate(lowerQuery);
            const promises = [];
            for (const [id, ext] of this.extensions) {
                if (!ext.sandbox?.hasSearch) continue;
                if (!this._isEnabled(id)) continue;
                if (!shouldCall(id)) continue;
                promises.push(
                    ext.sandbox
                        .call('match', { query })
                        .then((matches) => (matches || []).map((m) => ({ ...m, _extensionId: id })))
                        .catch((e) => {
                            console.warn(`match() in '${id}' failed:`, e);
                            return [];
                        })
                );
            }
            const results = await Promise.all(promises);
            return results.flat();
        },

        async matchAllAsync(query) {
            if (query.trim().startsWith('>')) return [];
            const lowerQuery = query.trim().toLowerCase();
            const shouldCall = await this._buildKeywordGate(lowerQuery);
            const promises = [];
            for (const [id, ext] of this.extensions) {
                if (!ext.sandbox?.hasSearch) continue;
                if (!this._isEnabled(id)) continue;
                if (!shouldCall(id)) continue;
                promises.push(
                    ext.sandbox
                        .call('matchAsync', { query })
                        .then((matches) => (matches || []).map((m) => ({ ...m, _extensionId: id })))
                        .catch((e) => {
                            console.warn(`matchAsync() in '${id}' failed:`, e);
                            return [];
                        })
                );
            }
            const results = await Promise.all(promises);
            return results.flat();
        },

        /**
         * Collect search keywords registered by every enabled search provider,
         * with their hint labels already localised host-side.
         *
         * Each entry: { extensionId, keyword, label, description, icon, acceptsArgs }
         * where `keyword` is lowercased for matching and `label`/`description` are
         * resolved from the extension's i18n catalog (the provider returns KEYS).
         *
         * Result is cached because getKeywords() is config-derived but stable
         * between config changes — the cache is cleared by reload() and
         * onConfigUpdate(). Returns [] before any extension has loaded.
         */ async getKeywordDefinitions() {
            if (this._keywordDefsCache) return this._keywordDefsCache;
            const defs = [];
            for (const [id, ext] of this.extensions) {
                if (!ext.sandbox?.hasSearch) continue;
                if (!this._isEnabled(id)) continue;
                let keywords;
                try {
                    keywords = await ext.sandbox.call('getKeywords', {});
                } catch (e) {
                    console.warn(`getKeywords() in '${id}' failed:`, e);
                    continue;
                }
                if (!Array.isArray(keywords)) continue;
                const catalog = ext.i18n?.catalog;
                const fallback = ext.i18n?.fallback;
                const icon = ext.localizedManifest?.icon || ext.manifest?.icon || '🔌';
                for (const k of keywords) {
                    const keyword = typeof k?.keyword === 'string' ? k.keyword.trim() : '';
                    if (!keyword) continue;
                    defs.push({
                        extensionId: id,
                        keyword: keyword.toLowerCase(),
                        // Fall back to the bare keyword if the label key is
                        // missing — a hint with no text is worse than the raw word.
                        label: resolveExtensionMessage(k.labelKey, catalog, fallback, keyword),
                        description: resolveExtensionMessage(
                            k.descriptionKey,
                            catalog,
                            fallback,
                            ''
                        ),
                        icon: typeof k.icon === 'string' && k.icon ? k.icon : icon,
                        acceptsArgs: k.acceptsArgs !== false, // default true
                    });
                }
            }
            this._keywordDefsCache = defs;
            return defs;
        },

        async executeResult(result) {
            const id = result?._extensionId;
            if (!id) return null;
            const ext = this.extensions.get(id);
            if (!ext?.sandbox?.hasSearch) return null;
            try {
                // Strip the host-only stamp before sending; the extension never needs it.
                const { _extensionId: _ignore, ...clean } = result;
                return await ext.sandbox.call('execute', { result: clean });
            } catch (e) {
                console.warn(`execute() in '${id}' failed:`, e);
                return null;
            }
        },

        /**
         * Custom render hook. Asks the sandbox for a custom HTML string;
         * host sanitizes and injects. Returns true if the extension handled
         * rendering, false to fall back to the default renderer.
         *
         * This must stay synchronous to match the existing call site, so we
         * keep a per-result cache warmed by `prefetchCustomRender()`. Use
         * that async method in the code path that produces results before
         * rendering — see search-unified.js.
         */ renderResult(result, element) {
            const id = result?._extensionId;
            if (!id) return false;
            const ext = this.extensions.get(id);
            if (!ext?.sandbox?.hasSearch) return false;
            const cached = this._customRenderCache?.get(result.id);
            if (!cached) return false;
            // Result rows are structural — use rich sanitization so the
            // extension can lay out its own row (icon + label + buttons).
            const frag = sanitizeExtensionHtml(cached.html, 'rich');
            if (cached.className)
                element.classList.add(...cached.className.split(/\s+/).filter(Boolean));
            element.appendChild(frag);
            this._wireExtActionsFor(id, element);
            return true;
        },

        /**
         * Pre-warm the custom-render cache for a batch of results. Called
         * once per render pass before we start building suggestion DOM.
         *
         * Cache has a soft cap to prevent unbounded growth as users type
         * many different queries. When the cap is hit, we drop the oldest
         * half. Pure map-order LRU — simple and good enough for this scale.
         */ async prefetchCustomRender(results) {
            if (!this._customRenderCache) this._customRenderCache = new Map();
            const CAP = 200;
            if (this._customRenderCache.size >= CAP) {
                const toDrop = Math.floor(CAP / 2);
                let i = 0;
                for (const key of this._customRenderCache.keys()) {
                    if (i++ >= toDrop) break;
                    this._customRenderCache.delete(key);
                }
            }
            const tasks = [];
            for (const r of results) {
                const id = r?._extensionId;
                if (!id) continue;
                const ext = this.extensions.get(id);
                if (!ext?.sandbox?.hasSearch) continue;
                if (!r.id) continue;
                if (this._customRenderCache.has(r.id)) {
                    // Re-insert at the tail so recently-used entries survive
                    // the next eviction pass.
                    const v = this._customRenderCache.get(r.id);
                    this._customRenderCache.delete(r.id);
                    this._customRenderCache.set(r.id, v);
                    continue;
                }
                // Strip host-only stamp.
                const { _extensionId: _ig, ...clean } = r;
                tasks.push(
                    ext.sandbox
                        .call('renderCustom', { result: clean })
                        .then((out) => {
                            if (out && typeof out.html === 'string') {
                                this._customRenderCache.set(r.id, out);
                            }
                        })
                        .catch(() => {
                            /* null/throw → fall back to default */
                        })
                );
            }
            if (tasks.length) await Promise.all(tasks);
        },
    });
}
