// Markdown rendering with code block, mermaid, graphviz, and PlantUML support

const DIAGRAM_LANGUAGES = new Set(['mermaid', 'plantuml', 'puml', 'dot', 'graphviz', 'neato']);
const HTML_LANGUAGES = new Set(['html', 'htm']);

let graphvizInstance = null;
async function getGraphviz() {
    if (graphvizInstance) return graphvizInstance;
    try {
        const module = await import('../vendor/lib/graphviz.js');
        graphvizInstance = await module.Graphviz.load();
        return graphvizInstance;
    } catch (e) {
        console.error('Failed to load Graphviz WASM:', e);
        return null;
    }
}

// Lazy-load mermaid (~3.2MB) only when a mermaid diagram is first encountered
let mermaidReady = false;
let mermaidLoadPromise = null;
async function ensureMermaid() {
    if (mermaidReady) return true;
    if (!mermaidLoadPromise) {
        mermaidLoadPromise = new Promise((resolve) => {
            const script = document.createElement('script');
            script.src = 'vendor/lib/mermaid.min.js';
            script.onload = () => {
                mermaid.initialize({
                    startOnLoad: false,
                    theme: 'default',
                    securityLevel: 'loose',
                    flowchart: { useMaxWidth: true, htmlLabels: true, curve: 'basis' }
                });
                mermaidReady = true;
                resolve(true);
            };
            script.onerror = () => resolve(false);
            document.head.appendChild(script);
        });
    }
    return mermaidLoadPromise;
}

export function initMarkdown() {
    // mermaid is now lazy-loaded on first diagram encounter — nothing to do here
}

// --- Streaming-aware debounced rendering ---

const STREAMING_RENDER_INTERVAL = 150; // ms between renders during streaming
const _renderTimers = new WeakMap();   // targetElement → timer id
const _lastRenderTime = new WeakMap(); // targetElement → timestamp

/**
 * Render markdown into a target element.
 * @param {string} markdown - raw markdown text
 * @param {HTMLElement} targetElement - DOM element to render into
 * @param {boolean} [streaming=false] - true while chunks are arriving; throttles
 *   rendering and skips expensive diagram/table work until complete
 */
export function renderMarkdown(markdown, targetElement, streaming = false) {
    if (!markdown) { targetElement.innerHTML = ''; return; }

    if (!streaming) {
        // Final render — cancel any pending debounce and do a full render now
        const pending = _renderTimers.get(targetElement);
        if (pending) { clearTimeout(pending); _renderTimers.delete(targetElement); }
        _doRender(markdown, targetElement, false);
        return;
    }

    // Streaming: throttle renders to at most one per STREAMING_RENDER_INTERVAL
    const now = Date.now();
    const last = _lastRenderTime.get(targetElement) || 0;
    const elapsed = now - last;

    if (elapsed >= STREAMING_RENDER_INTERVAL) {
        // Enough time has passed — render immediately
        const pending = _renderTimers.get(targetElement);
        if (pending) { clearTimeout(pending); _renderTimers.delete(targetElement); }
        _doRender(markdown, targetElement, true);
    } else {
        // Schedule a render for when the interval expires (if not already scheduled)
        if (!_renderTimers.has(targetElement)) {
            const delay = STREAMING_RENDER_INTERVAL - elapsed;
            const timer = setTimeout(() => {
                _renderTimers.delete(targetElement);
                _doRender(markdown, targetElement, true);
            }, delay);
            _renderTimers.set(targetElement, timer);
        }
    }
}

function _doRender(markdown, targetElement, streaming) {
    _lastRenderTime.set(targetElement, Date.now());

    marked.setOptions({ breaks: true, gfm: true });
    targetElement.innerHTML = marked.parse(markdown);

    targetElement.querySelectorAll('pre code').forEach((codeBlock) => {
        const pre = codeBlock.parentElement;
        const langMatch = codeBlock.className.match(/language-(\w+)/);
        const language = langMatch ? langMatch[1] : 'text';

        // Skip diagram and HTML preview rendering during streaming — they're
        // incomplete and expensive (mermaid, graphviz WASM, iframe creation).
        if (DIAGRAM_LANGUAGES.has(language)) {
            if (streaming) {
                wrapCodeBlock(codeBlock, pre, language);
            } else {
                renderDiagram(codeBlock, pre, language);
            }
            return;
        }
        if (HTML_LANGUAGES.has(language)) {
            if (streaming) {
                wrapCodeBlock(codeBlock, pre, language);
            } else {
                renderHtmlPreview(codeBlock, pre);
            }
            return;
        }
        if (language && language !== 'text' && Prism.languages[language]) {
            try {
                codeBlock.innerHTML = Prism.highlight(codeBlock.textContent, Prism.languages[language], language);
                codeBlock.className = 'language-' + language;
            } catch (e) { /* skip */ }
        }
        wrapCodeBlock(codeBlock, pre, language);
    });

    // Only wire up sortable tables on the final render
    if (!streaming) {
        makeTablesSortable(targetElement);
    }
}

function wrapCodeBlock(codeBlock, pre, language) {
    const wrapper = document.createElement('div');
    wrapper.className = 'code-block-wrapper';
    const header = document.createElement('div');
    header.className = 'code-block-header';
    const langLabel = document.createElement('span');
    langLabel.className = 'code-block-language';
    langLabel.textContent = language;
    header.appendChild(langLabel);
    header.appendChild(createCopyButton(codeBlock.textContent));
    pre.parentNode.insertBefore(wrapper, pre);
    wrapper.appendChild(header);
    wrapper.appendChild(pre);
}

// --- Generic diagram rendering ---

async function renderDiagram(codeBlock, pre, language) {
    const code = codeBlock.textContent;

    // Don't render incomplete diagrams during streaming
    if (language === 'mermaid' && !code.trim()) return;
    if ((language === 'plantuml' || language === 'puml') && !code.includes('@enduml')) {
        wrapCodeBlock(codeBlock, pre, language); return;
    }
    if ((language === 'dot' || language === 'graphviz' || language === 'neato') && !code.includes('}')) {
        wrapCodeBlock(codeBlock, pre, language); return;
    }

    const labels = { mermaid:'Mermaid', plantuml:'PlantUML', puml:'PlantUML', dot:'Graphviz', graphviz:'Graphviz', neato:'Graphviz (neato)' };

    const wrapper = document.createElement('div');
    wrapper.className = 'diagram-wrapper';

    const header = document.createElement('div');
    header.className = 'diagram-header';
    const label = document.createElement('span');
    label.className = 'diagram-label';
    label.textContent = labels[language] || language;

    const actions = document.createElement('div');
    actions.className = 'diagram-actions';
    const toggleBtn = document.createElement('button');
    toggleBtn.className = 'copy-button diagram-toggle';
    toggleBtn.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="16 18 22 12 16 6"></polyline><polyline points="8 6 2 12 8 18"></polyline></svg><span>Source</span>';
    actions.appendChild(toggleBtn);
    actions.appendChild(createCopyButton(code));
    header.appendChild(label);
    header.appendChild(actions);

    const diagramDiv = document.createElement('div');
    diagramDiv.className = 'diagram-content';

    const sourceDiv = document.createElement('div');
    sourceDiv.className = 'diagram-source';
    const sPre = document.createElement('pre');
    const sCode = document.createElement('code');
    sCode.textContent = code;
    sPre.appendChild(sCode);
    sourceDiv.appendChild(sPre);

    pre.parentNode.insertBefore(wrapper, pre);
    wrapper.appendChild(header);
    wrapper.appendChild(diagramDiv);
    wrapper.appendChild(sourceDiv);
    pre.remove();

    toggleBtn.onclick = () => {
        const showing = sourceDiv.classList.toggle('visible');
        toggleBtn.querySelector('span').textContent = showing ? 'Diagram' : 'Source';
        diagramDiv.style.display = showing ? 'none' : '';
    };

    if (language === 'mermaid') {
        await renderMermaidInto(diagramDiv, code);
    } else if (language === 'dot' || language === 'graphviz' || language === 'neato') {
        await renderGraphvizInto(diagramDiv, code, language);
    } else {
        renderPlantUMLInto(diagramDiv, code);
    }
}

// --- Engine-specific renderers ---

async function renderMermaidInto(container, code) {
    const loaded = await ensureMermaid();
    if (!loaded) {
        container.innerHTML = '<div style="color:#dc2626;padding:20px;">Failed to load Mermaid library</div>';
        return;
    }
    container.classList.add('mermaid');
    container.textContent = code;
    try {
        await mermaid.run({ nodes: [container] });
    } catch (error) {
        console.error('Mermaid rendering error:', error);
        container.innerHTML = '<div style="color:#dc2626;padding:20px;">Error: ' + error.message + '</div>';
    }
}

async function renderGraphvizInto(container, code, language) {
    try {
        const graphviz = await getGraphviz();
        if (!graphviz) {
            container.innerHTML = '<div style="color:#dc2626;padding:20px;">Graphviz WASM failed to load</div>';
            return;
        }
        const engine = language === 'neato' ? 'neato' : 'dot';
        const svg = graphviz.layout(code, 'svg', engine);
        container.innerHTML = svg;
        const svgEl = container.querySelector('svg');
        if (svgEl) { svgEl.style.maxWidth = '100%'; svgEl.style.height = 'auto'; }
    } catch (error) {
        console.error('Graphviz rendering error:', error);
        container.innerHTML = '<div style="color:#dc2626;padding:20px;">Graphviz error: ' + error.message + '</div>';
    }
}

function renderPlantUMLInto(container, code) {
    // PlantUML requires Java — no pure JS renderer exists. Show formatted source.
    const pre = document.createElement('pre');
    pre.style.cssText = 'margin:0;padding:16px;background:#272822;overflow-x:auto';
    const codeEl = document.createElement('code');
    codeEl.style.cssText = "font-family:'Consolas','Monaco','Courier New',monospace;font-size:13px;line-height:1.5;color:#f8f8f2;white-space:pre";
    codeEl.textContent = code;
    pre.appendChild(codeEl);
    container.style.padding = '0';
    container.style.background = '#272822';
    container.appendChild(pre);
}

// --- HTML preview rendering ---

function renderHtmlPreview(codeBlock, pre) {
    const code = codeBlock.textContent;

    // Don't render incomplete HTML during streaming
    if (!code.trim()) return;

    const wrapper = document.createElement('div');
    wrapper.className = 'diagram-wrapper';

    const header = document.createElement('div');
    header.className = 'diagram-header';
    const label = document.createElement('span');
    label.className = 'diagram-label';
    label.textContent = 'HTML Preview';

    const actions = document.createElement('div');
    actions.className = 'diagram-actions';
    const toggleBtn = document.createElement('button');
    toggleBtn.className = 'copy-button diagram-toggle';
    toggleBtn.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="16 18 22 12 16 6"></polyline><polyline points="8 6 2 12 8 18"></polyline></svg><span>Source</span>';
    actions.appendChild(toggleBtn);
    actions.appendChild(createCopyButton(code));
    header.appendChild(label);
    header.appendChild(actions);

    // Sandboxed iframe — no JS execution
    const previewDiv = document.createElement('div');
    previewDiv.className = 'diagram-content html-preview-content';
    const iframe = document.createElement('iframe');
    iframe.sandbox = 'allow-same-origin'; // No allow-scripts
    iframe.style.cssText = 'width:100%;border:none;background:#fff;min-height:60px;';
    iframe.srcdoc = code;
    previewDiv.appendChild(iframe);

    // Auto-resize iframe to fit content
    iframe.onload = () => {
        try {
            const doc = iframe.contentDocument || iframe.contentWindow.document;
            // Strip any script tags that might have been included
            doc.querySelectorAll('script').forEach(s => s.remove());
            const h = doc.documentElement.scrollHeight || doc.body.scrollHeight;
            iframe.style.height = Math.min(Math.max(h, 60), 600) + 'px';
        } catch { /* cross-origin, ignore */ }
    };

    const sourceDiv = document.createElement('div');
    sourceDiv.className = 'diagram-source';
    const sPre = document.createElement('pre');
    const sCode = document.createElement('code');
    sCode.textContent = code;
    if (Prism.languages.markup) {
        sCode.innerHTML = Prism.highlight(code, Prism.languages.markup, 'html');
    }
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
}

// --- Shared utilities ---

function createCopyButton(code) {
    const btn = document.createElement('button');
    btn.className = 'copy-button';
    btn.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg><span>Copy</span>';
    btn.onclick = () => copyCode(code, btn);
    return btn;
}

function copyCode(code, button) {
    navigator.clipboard.writeText(code).then(() => {
        const orig = button.innerHTML;
        button.classList.add('copied');
        button.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="20 6 9 17 4 12"></polyline></svg><span>Copied!</span>';
        setTimeout(() => { button.classList.remove('copied'); button.innerHTML = orig; }, 2000);
    }).catch(err => console.error('Copy failed:', err));
}

function makeTablesSortable(container) {
    container.querySelectorAll('table').forEach(table => {
        const thead = table.querySelector('thead');
        const tbody = table.querySelector('tbody');
        if (!thead || !tbody) return;

        const headers = thead.querySelectorAll('th');
        headers.forEach((th, colIndex) => {
            th.style.cursor = 'pointer';
            th.style.userSelect = 'none';
            th.title = 'Click to sort';
            th.addEventListener('click', () => {
                const rows = Array.from(tbody.querySelectorAll('tr'));
                const currentDir = th.dataset.sortDir || 'none';
                const newDir = currentDir === 'asc' ? 'desc' : 'asc';

                // Reset all headers
                headers.forEach(h => {
                    h.dataset.sortDir = 'none';
                    h.textContent = h.textContent.replace(/ [▲▼]$/, '');
                });

                th.dataset.sortDir = newDir;
                th.textContent += newDir === 'asc' ? ' ▲' : ' ▼';

                rows.sort((a, b) => {
                    const aText = (a.cells[colIndex]?.textContent || '').trim();
                    const bText = (b.cells[colIndex]?.textContent || '').trim();
                    const aNum = parseFloat(aText);
                    const bNum = parseFloat(bText);
                    // Numeric sort if both are numbers
                    if (!isNaN(aNum) && !isNaN(bNum)) {
                        return newDir === 'asc' ? aNum - bNum : bNum - aNum;
                    }
                    // String sort
                    const cmp = aText.localeCompare(bText, undefined, { numeric: true, sensitivity: 'base' });
                    return newDir === 'asc' ? cmp : -cmp;
                });

                rows.forEach(row => tbody.appendChild(row));
            });
        });
    });
}
