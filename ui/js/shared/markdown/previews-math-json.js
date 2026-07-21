import { t } from '../i18n.js';
import { createCopyButton, highlightOrLazy, wrapCodeBlock } from './code-controls.js';

let katexReady = false;
let katexLoadPromise = null;
async function ensureKatex() {
    if (katexReady) return true;
    if (!katexLoadPromise) {
        katexLoadPromise = new Promise((resolve) => {
            const cssLink = document.createElement('link');
            cssLink.rel = 'stylesheet';
            cssLink.href = 'vendor/lib/katex/katex.min.css';
            document.head.appendChild(cssLink);

            const script = document.createElement('script');
            script.src = 'vendor/lib/katex/katex.min.js';
            script.onload = () => {
                katexReady = !!window.katex;
                resolve(katexReady);
            };
            script.onerror = () => {
                console.warn('[markdown] failed to load KaTeX');
                resolve(false);
            };
            document.head.appendChild(script);
        });
    }
    return katexLoadPromise;
}

// --- LaTeX / Math renderer ---

/**
 * Heuristic: does this look like a full TeX *document* (which our
 * KaTeX-based renderer can't handle) vs. a math-mode expression
 * (which it can)? Look for a small set of document-level commands.
 * Any one of them is enough to flip to source-only rendering.
 *
 * Exported with the underscore prefix so tests can pin the contract,
 * but not intended as a stable external API.
 */
export function isFullTexDocument(source) {
    if (!source) return false;
    // Document-level scaffolding KaTeX doesn't (and won't) support.
    // The list is short on purpose: false positives demote a math
    // expression to plain source, which is much worse than a false
    // negative (a full doc rendered as red KaTeX errors). Keep
    // additions to commands that ONLY appear in document context.
    const docCommands = [
        '\\documentclass',
        '\\usepackage',
        '\\begin{document}',
        '\\maketitle',
        '\\tableofcontents',
        '\\bibliography',
        '\\begin{abstract}',
    ];
    return docCommands.some((cmd) => source.includes(cmd));
}

/**
 * Render a `latex` / `tex` / `math` fenced block as KaTeX-rendered
 * math by default, with a "Source" toggle for the raw TeX. Same
 * preview chrome as the other diagram-style renderers.
 *
 * KaTeX is loaded lazily on first encounter via `ensureKatex()`;
 * before it arrives the block shows the source text (effectively
 * the source view) and `katex.render` runs once the script
 * resolves. If the load fails (offline / blocked), the source
 * stays put and an error banner replaces the would-be preview.
 *
 * Display-mode is on by default (centered, larger glyphs, common
 * for fenced blocks). Inline math (`$...$`) is a separate concern
 * and not handled here.
 *
 * Safety: KaTeX's `throwOnError: false` returns its own `parseError`
 * span instead of leaking arbitrary HTML. `trust: false` (default)
 * disallows commands like `\href` that could embed external URLs.
 */
export function renderMathPreview(codeBlock, pre) {
    const code = codeBlock.textContent;
    if (!code.trim()) return;

    const wrapper = document.createElement('div');
    wrapper.className = 'diagram-wrapper';

    const header = document.createElement('div');
    header.className = 'diagram-header';
    const label = document.createElement('span');
    label.className = 'diagram-label';
    label.textContent = t('shared.markdown.diagram.math');

    const actions = document.createElement('div');
    actions.className = 'diagram-actions';
    const toggleBtn = document.createElement('button');
    toggleBtn.className = 'copy-button diagram-toggle';
    toggleBtn.innerHTML =
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="16 18 22 12 16 6"></polyline><polyline points="8 6 2 12 8 18"></polyline></svg><span>Source</span>';
    actions.appendChild(toggleBtn);
    actions.appendChild(createCopyButton(code));
    header.appendChild(label);
    header.appendChild(actions);

    const previewDiv = document.createElement('div');
    previewDiv.className = 'diagram-content math-preview-content';
    // Show the raw TeX as a placeholder so the block isn't blank
    // while KaTeX loads. Once `ensureKatex` resolves, render swaps
    // it in. If the load fails, the placeholder stays.
    previewDiv.textContent = code;

    const sourceDiv = document.createElement('div');
    sourceDiv.className = 'diagram-source';
    const sPre = document.createElement('pre');
    const sCode = document.createElement('code');
    sCode.textContent = code;
    sPre.appendChild(sCode);
    sourceDiv.appendChild(sPre);

    pre.parentNode.insertBefore(wrapper, pre);
    wrapper.appendChild(header);
    wrapper.appendChild(previewDiv);
    wrapper.appendChild(sourceDiv);
    pre.remove();

    toggleBtn.onclick = () => {
        const showing = sourceDiv.classList.toggle('visible');
        toggleBtn.querySelector('span').textContent = showing ? 'Preview' : 'Source';
        previewDiv.style.display = showing ? 'none' : '';
    };

    // Async render — don't block the rest of the markdown pipeline.
    ensureKatex().then((ok) => {
        if (!ok || !window.katex) {
            previewDiv.innerHTML =
                '<div style="color:#dc2626;padding:12px;">KaTeX failed to load</div>';
            return;
        }
        try {
            previewDiv.innerHTML = '';
            window.katex.render(code, previewDiv, {
                displayMode: true,
                throwOnError: false,
                output: 'html',
                trust: false,
            });
        } catch (e) {
            previewDiv.innerHTML =
                '<div style="color:#dc2626;padding:12px;">Math error: ' +
                (e?.message || String(e)) +
                '</div>';
        }
    });
}

// --- JSON Tree Renderer ---

export function renderJsonTree(codeBlock, pre, language) {
    const code = codeBlock.textContent;
    let parsed;
    try {
        parsed = JSON.parse(code);
    } catch {
        // Not valid JSON — fall back to regular code block
        wrapCodeBlock(codeBlock, pre, language);
        return;
    }

    const wrapper = document.createElement('div');
    wrapper.className = 'diagram-wrapper json-tree-wrapper';

    const header = document.createElement('div');
    header.className = 'diagram-header';
    const label = document.createElement('span');
    label.className = 'diagram-label';
    label.textContent = t('shared.markdown.diagram.json');

    const actions = document.createElement('div');
    actions.className = 'diagram-actions';
    const toggleBtn = document.createElement('button');
    toggleBtn.className = 'copy-button diagram-toggle';
    toggleBtn.innerHTML =
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="16 18 22 12 16 6"></polyline><polyline points="8 6 2 12 8 18"></polyline></svg><span>Source</span>';
    actions.appendChild(toggleBtn);
    actions.appendChild(createCopyButton(code));
    header.appendChild(label);
    header.appendChild(actions);

    const treeDiv = document.createElement('div');
    treeDiv.className = 'json-tree-content';
    treeDiv.appendChild(_buildJsonNode(parsed, true));

    const sourceDiv = document.createElement('div');
    sourceDiv.className = 'diagram-source';
    const sPre = document.createElement('pre');
    sPre.style.margin = '0';
    const sCode = document.createElement('code');
    sCode.textContent = JSON.stringify(parsed, null, 2);
    highlightOrLazy(sCode, 'json');
    sPre.appendChild(sCode);
    sourceDiv.appendChild(sPre);

    pre.parentNode.insertBefore(wrapper, pre);
    wrapper.appendChild(header);
    wrapper.appendChild(treeDiv);
    wrapper.appendChild(sourceDiv);
    pre.remove();

    toggleBtn.onclick = () => {
        const showing = sourceDiv.classList.toggle('visible');
        toggleBtn.querySelector('span').textContent = showing ? 'Tree' : 'Source';
        treeDiv.style.display = showing ? 'none' : '';
    };
}

function _buildJsonNode(value, expanded = false) {
    if (value === null) return _jsonLeaf('null', 'json-null');
    if (typeof value === 'boolean') return _jsonLeaf(String(value), 'json-bool');
    if (typeof value === 'number') return _jsonLeaf(String(value), 'json-number');
    if (typeof value === 'string') return _jsonLeaf(`"${_escJsonStr(value)}"`, 'json-string');

    const isArray = Array.isArray(value);
    const entries = isArray ? value.map((v, i) => [String(i), v]) : Object.entries(value);

    const node = document.createElement('div');
    node.className = 'json-node';

    const toggle = document.createElement('span');
    toggle.className = 'json-toggle' + (expanded ? ' open' : '');
    toggle.textContent = expanded ? '▼' : '▶';

    const bracket = document.createElement('span');
    bracket.className = 'json-bracket';
    bracket.textContent = isArray ? `[${entries.length}]` : `{${entries.length}}`;

    const header = document.createElement('div');
    header.className = 'json-node-header';
    header.appendChild(toggle);
    header.appendChild(bracket);
    node.appendChild(header);

    const children = document.createElement('div');
    children.className = 'json-children';
    children.style.display = expanded ? '' : 'none';

    for (const [key, val] of entries) {
        const row = document.createElement('div');
        row.className = 'json-entry';
        const keyEl = document.createElement('span');
        keyEl.className = isArray ? 'json-index' : 'json-key';
        keyEl.textContent = isArray ? `${key}: ` : `"${key}": `;
        row.appendChild(keyEl);

        const childNode = _buildJsonNode(val, false);
        row.appendChild(childNode);
        children.appendChild(row);
    }

    node.appendChild(children);

    header.onclick = (e) => {
        e.stopPropagation();
        const open = children.style.display !== 'none';
        children.style.display = open ? 'none' : '';
        toggle.textContent = open ? '▶' : '▼';
        toggle.classList.toggle('open', !open);
    };

    return node;
}

function _jsonLeaf(text, className) {
    const el = document.createElement('span');
    el.className = className;
    el.textContent = text;
    return el;
}

function _escJsonStr(s) {
    return s
        .replace(/\\/g, '\\\\')
        .replace(/"/g, '\\"')
        .replace(/\n/g, '\\n')
        .replace(/\t/g, '\\t');
}

// --- Shared utilities ---
