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
        for (const { specifier } of extensionModuleImports(source)) {
            const path = resolveExtensionModule(specifier, importer);
            if (path && !visited.has(path)) queue.add(path);
        }
    };
    for (const [kind, value] of Object.entries(entrySources || {})) {
        if (kind === 'widgets' && value && typeof value === 'object') {
            for (const [id, source] of Object.entries(value)) {
                scan(source, sourcePaths.widgets?.[id]);
            }
        } else {
            scan(value, sourcePaths[kind]);
        }
    }
    // Discovery order doesn't matter; the runtime orders the finished graph.
    for (const path of queue) {
        visited.add(path);
        const source = await fetchText(path);
        if (source == null) continue;
        collected.set(path, source);
        scan(source, path);
    }
    return collected.size ? Object.fromEntries(collected) : undefined;
}

/** Replace only module specifier ranges, never import-like prose. */
export function rewriteExtensionModules(source, blobMap, importer = './') {
    let out = source;
    for (const { specifier, start, end } of extensionModuleImports(source).reverse()) {
        const path = resolveExtensionModule(specifier, importer);
        const blob = path && blobMap.get(path);
        if (blob) out = out.slice(0, start) + source[start] + blob + source[start] + out.slice(end);
    }
    return out;
}

/** Validate the whole graph before creating any blobs; dependencies first. */
export function extensionModuleOrder(sharedSources) {
    const sources = new Map(Object.entries(sharedSources || {}));
    const complete = new Set();
    const visiting = [];
    const active = new Set();
    const ordered = [];
    const visit = (path) => {
        if (complete.has(path)) return;
        if (active.has(path)) {
            const cycle = [...visiting.slice(visiting.indexOf(path)), path];
            throw new Error(`Extension module import cycle: ${cycle.join(' -> ')}`);
        }
        active.add(path);
        visiting.push(path);
        for (const { specifier } of extensionModuleImports(sources.get(path))) {
            const dependency = resolveExtensionModule(specifier, path);
            if (dependency && sources.has(dependency)) visit(dependency);
        }
        visiting.pop();
        active.delete(path);
        complete.add(path);
        ordered.push(path);
    };
    for (const path of sources.keys()) visit(path);
    return ordered;
}
