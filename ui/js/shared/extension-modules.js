import { parseExtensionModule } from '../../vendor/lib/extension-module-lexer.js';

/**
 * Static imports (including side effects), re-exports, and dynamic imports
 * with a quoted string argument. The lexer excludes comments, strings,
 * regexes, import.meta, and computed dynamic specifiers. Never evaluate an
 * expression to discover an import.
 *
 * Ranges include the quotes, so escaped specifiers can be replaced safely.
 */
export function extensionModuleImports(source) {
    if (typeof source !== 'string') return [];
    const [imports] = parseExtensionModule(source);
    return imports
        .filter((entry) => typeof entry.n === 'string' && entry.d !== -2)
        .map((entry) => ({
            specifier: entry.n,
            dynamic: entry.d >= 0,
            start: entry.d === -1 ? entry.s - 1 : entry.s,
            end: entry.d === -1 ? entry.e + 1 : entry.e,
        }));
}

/** Resolve relative to the importing file, staying inside this bundle.
 * No URL decoding, queries, fragments, schemes, or Windows separators.
 * Rust still authoritatively checks canonical containment (incl. symlinks).
 */
export function resolveExtensionModule(specifier, importer = './') {
    if (!specifier.startsWith('./') && !specifier.startsWith('../')) return null;
    const control = Array.from(specifier).some(
        (c) => c.charCodeAt(0) < 32 || c.charCodeAt(0) === 127
    );
    if (/[\\:?#%]/.test(specifier) || control) {
        throw new Error(`Invalid extension module path: ${specifier}`);
    }
    const parts = [];
    for (const part of [...importer.split('/').slice(0, -1), ...specifier.split('/')]) {
        if (!part || part === '.') continue;
        if (part === '..') {
            if (!parts.length) {
                throw new Error(`Extension module path escapes bundle root: ${specifier}`);
            }
            parts.pop();
        } else {
            parts.push(part);
        }
    }
    return `./${parts.join('/')}`;
}

/** Shared discovery for the normal manager and the Settings sandbox. */
export async function collectExtensionModules(entrySources, fetchText, sourcePaths = {}) {
    const queue = new Set();
    const visited = new Set();
    const collected = new Map();
    const scan = (source, importer) => {
        try {
            // Validate the entire file before queueing any of its imports.
            const paths = extensionModuleImports(source).map(({ specifier, dynamic }) => {
                try {
                    return resolveExtensionModule(specifier, importer);
                } catch (e) {
                    if (!dynamic) throw e;
                    return null; // A dynamic path error rejects only when called.
                }
            });
            for (const path of paths) {
                if (path && !visited.has(path)) queue.add(path);
            }
        } catch (e) {
            console.warn(`Failed to scan extension module '${importer}':`, e);
        }
    };
    for (const [kind, value] of Object.entries(entrySources || {})) {
        if (kind === 'widgets' && value && typeof value === 'object') {
            for (const [id, source] of Object.entries(value)) {
                scan(source, sourcePaths.widgets?.[id] || `widget:${id}`);
            }
        } else {
            scan(value, sourcePaths[kind] || kind);
        }
    }
    // Discovery order doesn't matter; the runtime orders the finished graph.
    for (const path of queue) {
        visited.add(path);
        let source;
        try {
            source = await fetchText(path);
        } catch (e) {
            console.warn(`Failed to read extension module '${path}':`, e);
            continue;
        }
        if (source == null) continue;
        collected.set(path, source);
        scan(source, path);
    }
    return collected.size ? Object.fromEntries(collected) : undefined;
}

/** Replace only module specifier ranges, never import-like prose. */
export function rewriteExtensionModules(
    source,
    blobMap,
    importer = './',
    failures,
    deferredDynamic
) {
    // Resolve and validate in source order; rewriting backwards must not choose
    // a later missing import over an earlier containment or dependency error.
    const resolved = extensionModuleImports(source).map((entry) => {
        try {
            return { ...entry, path: resolveExtensionModule(entry.specifier, importer) };
        } catch (error) {
            if (!entry.dynamic) throw error;
            return { ...entry, error };
        }
    });
    const imports = resolved.map((entry) => {
        const { path, dynamic, error } = entry;
        try {
            if (error) throw error;
            if (path && failures?.has(path)) throw failures.get(path);
            const blob = path && blobMap.get(path);
            const deferred = dynamic && path && !blob && deferredDynamic?.(path);
            if (deferred) return { ...entry, replacement: deferred };
            if (path && failures && !blob) {
                throw new Error(`Missing extension module '${path}' imported by '${importer}'`);
            }
            return {
                ...entry,
                replacement: blob && source[entry.start] + blob + source[entry.start],
            };
        } catch (e) {
            if (!dynamic) throw e;
            // Evaluating this stub fails the import() promise, preserving the
            // provider's try/catch and recording the original diagnostic.
            const stub = URL.createObjectURL(
                new Blob([`throw new Error(${JSON.stringify(e?.message || String(e))});`], {
                    type: 'application/javascript',
                })
            );
            return { ...entry, replacement: JSON.stringify(stub) };
        }
    });
    let out = source;
    for (const { start, end, replacement } of imports.reverse()) {
        if (replacement) out = out.slice(0, start) + replacement + out.slice(end);
    }
    return out;
}

/** Order healthy dependencies first; isolate malformed files and cycles. */
export function extensionModuleOrder(sharedSources, failures = new Map()) {
    const sources = new Map(Object.entries(sharedSources || {}));
    const complete = new Set();
    const visiting = [];
    const active = new Set();
    const ordered = [];
    const visit = (path) => {
        if (complete.has(path)) return;
        if (active.has(path)) {
            const cycle = [...visiting.slice(visiting.indexOf(path)), path];
            const error = new Error(`Extension module import cycle: ${cycle.join(' -> ')}`);
            for (const member of cycle) failures.set(member, error);
            return;
        }
        active.add(path);
        visiting.push(path);
        try {
            for (const { specifier, dynamic } of extensionModuleImports(sources.get(path))) {
                if (dynamic) continue;
                const dependency = resolveExtensionModule(specifier, path);
                if (dependency && sources.has(dependency)) visit(dependency);
                if (failures.has(dependency)) throw failures.get(dependency);
            }
        } catch (e) {
            if (!failures.has(path)) {
                failures.set(path, new Error(`Extension module '${path}': ${e?.message || e}`));
            }
        }
        visiting.pop();
        active.delete(path);
        complete.add(path);
        if (!failures.has(path)) ordered.push(path);
    };
    for (const path of sources.keys()) visit(path);
    // Prefer building dynamic targets first too, without treating dynamic
    // edges as static cycles or propagating their failures. When no such
    // ordering exists, keep the static dependency order and defer the lookup.
    const remaining = new Set(ordered);
    const result = [];
    while (remaining.size) {
        const next =
            [...remaining].find((path) =>
                extensionModuleImports(sources.get(path)).every(({ specifier, dynamic }) => {
                    try {
                        return !remaining.has(resolveExtensionModule(specifier, path));
                    } catch (e) {
                        if (!dynamic) throw e;
                        return true;
                    }
                })
            ) || remaining.values().next().value;
        remaining.delete(next);
        result.push(next);
    }
    return result;
}
