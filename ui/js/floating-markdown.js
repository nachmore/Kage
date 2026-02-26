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

    // During streaming, preserve successfully rendered diagrams so they don't
    // flash back to source code when innerHTML is replaced.  Key by source text.
    const savedDiagrams = new Map(); // code text → DOM wrapper element
    if (streaming) {
        targetElement.querySelectorAll('.diagram-wrapper[data-rendered]').forEach((wrapper) => {
            const sourceEl = wrapper.querySelector('.diagram-source code');
            if (sourceEl) {
                savedDiagrams.set(sourceEl.textContent, wrapper);
                // Detach from DOM so innerHTML wipe doesn't destroy it
                wrapper.remove();
            }
        });
    }

    marked.setOptions({ breaks: true, gfm: true });
    targetElement.innerHTML = marked.parse(markdown);

    targetElement.querySelectorAll('pre code').forEach((codeBlock) => {
        const pre = codeBlock.parentElement;
        const langMatch = codeBlock.className.match(/language-(\w+)/);
        const language = langMatch ? langMatch[1] : 'text';

        if (DIAGRAM_LANGUAGES.has(language)) {
            const code = codeBlock.textContent;
            if (streaming) {
                // Reinsert the last successful render immediately (no flash)
                if (savedDiagrams.size > 0) {
                    const saved = savedDiagrams.get(code) || savedDiagrams.values().next().value;
                    const savedKey = savedDiagrams.has(code) ? code : savedDiagrams.keys().next().value;
                    pre.parentNode.insertBefore(saved, pre);
                    pre.remove();
                    savedDiagrams.delete(savedKey);

                    // If the code changed, attempt a background re-render
                    if (savedKey !== code && !_diagramPending.has(_codeHash(code))) {
                        _tryBackgroundDiagramRender(saved, code, language);
                    }
                    return;
                }
                // No previous render — first attempt
                renderDiagram(codeBlock, pre, language, true);
                return;
            }
            renderDiagram(codeBlock, pre, language, false);
            return;
        }
        if (HTML_LANGUAGES.has(language)) {
            renderHtmlPreview(codeBlock, pre);
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
        _resetDiagramFailures();
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

/**
 * Attempt a background re-render of a diagram that already has a successful render.
 * Renders into a detached node; on success, swaps the diagram-content with a fade.
 * On failure, silently ignores — the existing render stays.
 */
async function _tryBackgroundDiagramRender(existingWrapper, code, language) {
    const hash = _codeHash(code);
    if (_diagramPending.has(hash)) return;
    const failures = _diagramFailures.get(hash) || 0;
    if (failures >= MAX_STREAMING_FAILURES) return;

    _diagramPending.add(hash);

    try {
        if (language === 'mermaid') {
            const loaded = await ensureMermaid();
            if (!loaded) { _diagramPending.delete(hash); return; }

            const renderNode = document.createElement('div');
            renderNode.classList.add('mermaid');
            renderNode.textContent = code;
            renderNode.style.cssText = 'position:absolute;left:-9999px;top:-9999px';
            document.body.appendChild(renderNode);

            try {
                await mermaid.run({ nodes: [renderNode] });
                renderNode.style.cssText = '';
                // Swap into the existing wrapper's diagram-content
                const diagramContent = existingWrapper.querySelector('.diagram-content');
                if (diagramContent && existingWrapper.isConnected) {
                    diagramContent.innerHTML = '';
                    diagramContent.appendChild(renderNode);
                    const sourceCode = existingWrapper.querySelector('.diagram-source code');
                    if (sourceCode) sourceCode.textContent = code;
                    _markDiagramRendered(diagramContent);
                } else {
                    renderNode.remove();
                }
            } catch {
                renderNode.remove();
                _diagramFailures.set(hash, failures + 1);
            }
        } else if (language === 'dot' || language === 'graphviz' || language === 'neato') {
            const graphviz = await getGraphviz();
            if (!graphviz) { _diagramPending.delete(hash); return; }

            const engine = language === 'neato' ? 'neato' : 'dot';
            try {
                const svg = graphviz.layout(code, 'svg', engine);
                const diagramContent = existingWrapper.querySelector('.diagram-content');
                if (diagramContent && existingWrapper.isConnected) {
                    diagramContent.innerHTML = svg;
                    const svgEl = diagramContent.querySelector('svg');
                    if (svgEl) { svgEl.style.maxWidth = '100%'; svgEl.style.height = 'auto'; }
                    const sourceCode = existingWrapper.querySelector('.diagram-source code');
                    if (sourceCode) sourceCode.textContent = code;
                    _markDiagramRendered(diagramContent);
                }
            } catch {
                _diagramFailures.set(hash, failures + 1);
            }
        }
    } finally {
        _diagramPending.delete(hash);
    }
}

// Track failed render attempts per diagram source code during streaming.
// After MAX_STREAMING_FAILURES, stop attempting until the final render.
const MAX_STREAMING_FAILURES = 3;
const _diagramFailures = new Map(); // code hash → failure count
const _diagramPending = new Set();  // code hashes currently being rendered

function _codeHash(code) {
    // Simple hash for keying — use first 200 chars + length to avoid collisions
    return code.substring(0, 200) + ':' + code.length;
}

/** Reset failure tracking (call on final render) */
function _resetDiagramFailures() {
    _diagramFailures.clear();
    _diagramPending.clear();
}

/** Mark a diagram wrapper as successfully rendered and show its save button */
function _markDiagramRendered(container) {
    const wrapper = container.closest('.diagram-wrapper');
    if (wrapper) {
        wrapper.setAttribute('data-rendered', '');
        const saveBtn = wrapper.querySelector('.diagram-save-btn');
        if (saveBtn) saveBtn.style.display = '';
    }
}

async function renderDiagram(codeBlock, pre, language, streaming = false) {
    const code = codeBlock.textContent;

    // Don't render incomplete diagrams — show as code block and wait for more content
    if (language === 'mermaid' && !code.trim()) return;
    if ((language === 'plantuml' || language === 'puml') && !code.includes('@enduml')) {
        wrapCodeBlock(codeBlock, pre, language); return;
    }
    if ((language === 'dot' || language === 'graphviz' || language === 'neato') && !code.includes('}')) {
        wrapCodeBlock(codeBlock, pre, language); return;
    }

    // During streaming, if this code has failed too many times, just show as code block
    if (streaming) {
        const hash = _codeHash(code);
        const failures = _diagramFailures.get(hash) || 0;
        if (failures >= MAX_STREAMING_FAILURES) {
            wrapCodeBlock(codeBlock, pre, language);
            return;
        }
        // If an async render is already in-flight for this exact code, show code block
        // and let the in-flight render complete (it will be orphaned but harmless)
        if (_diagramPending.has(hash)) {
            wrapCodeBlock(codeBlock, pre, language);
            return;
        }
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

    // Add save button — needs diagramDiv reference, so we append it after creation
    actions.appendChild(createSaveButton(diagramDiv));

    // During streaming, show formatted source as placeholder until async render succeeds.
    // This prevents a flash of empty/unformatted content while mermaid/graphviz loads.
    if (streaming) {
        const placeholderPre = document.createElement('pre');
        placeholderPre.style.cssText = 'margin:0;padding:16px;overflow-x:auto';
        const placeholderCode = document.createElement('code');
        placeholderCode.textContent = code;
        if (Prism.languages[language]) {
            try { placeholderCode.innerHTML = Prism.highlight(code, Prism.languages[language], language); } catch {}
        }
        placeholderPre.appendChild(placeholderCode);
        diagramDiv.appendChild(placeholderPre);
    }

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
        await renderMermaidInto(diagramDiv, code, streaming);
    } else if (language === 'dot' || language === 'graphviz' || language === 'neato') {
        await renderGraphvizInto(diagramDiv, code, language, streaming);
    } else {
        renderPlantUMLInto(diagramDiv, code);
    }
}

// --- Engine-specific renderers ---

async function renderMermaidInto(container, code, streaming = false) {
    const hash = _codeHash(code);
    if (streaming) _diagramPending.add(hash);

    const loaded = await ensureMermaid();
    if (!loaded) {
        _diagramPending.delete(hash);
        if (streaming) {
            _diagramFailures.set(hash, MAX_STREAMING_FAILURES);
        } else {
            container.innerHTML = '<div style="color:#dc2626;padding:20px;">Failed to load Mermaid library</div>';
        }
        return;
    }

    // Render into a detached node so the visible container isn't disrupted
    // until we know the render succeeded.
    const renderNode = document.createElement('div');
    renderNode.classList.add('mermaid');
    renderNode.textContent = code;
    // mermaid.run needs the node in the DOM to measure it
    renderNode.style.cssText = 'position:absolute;left:-9999px;top:-9999px';
    document.body.appendChild(renderNode);

    try {
        await mermaid.run({ nodes: [renderNode] });
        _diagramPending.delete(hash);
        // Success — swap into the visible container and mark wrapper as rendered
        renderNode.style.cssText = '';
        container.innerHTML = '';
        container.appendChild(renderNode);
        _markDiagramRendered(container);
    } catch (error) {
        renderNode.remove();
        _diagramPending.delete(hash);
        if (streaming) {
            _diagramFailures.set(hash, (_diagramFailures.get(hash) || 0) + 1);
            // Leave the placeholder (formatted source) intact
        } else {
            console.error('Mermaid rendering error:', error);
            container.innerHTML = '<div style="color:#dc2626;padding:20px;">Error: ' + error.message + '</div>';
        }
    }
}

async function renderGraphvizInto(container, code, language, streaming = false) {
    const hash = _codeHash(code);
    if (streaming) _diagramPending.add(hash);
    try {
        const graphviz = await getGraphviz();
        if (!graphviz) {
            _diagramPending.delete(hash);
            if (streaming) {
                _diagramFailures.set(hash, MAX_STREAMING_FAILURES);
            } else {
                container.innerHTML = '<div style="color:#dc2626;padding:20px;">Graphviz WASM failed to load</div>';
            }
            return;
        }
        const engine = language === 'neato' ? 'neato' : 'dot';
        const svg = graphviz.layout(code, 'svg', engine);
        _diagramPending.delete(hash);
        // Success — replace placeholder with rendered SVG
        container.innerHTML = svg;
        const svgEl = container.querySelector('svg');
        if (svgEl) { svgEl.style.maxWidth = '100%'; svgEl.style.height = 'auto'; }
        _markDiagramRendered(container);
    } catch (error) {
        _diagramPending.delete(hash);
        if (streaming) {
            _diagramFailures.set(hash, (_diagramFailures.get(hash) || 0) + 1);
            // Leave the placeholder (formatted source) intact
        } else {
            console.error('Graphviz rendering error:', error);
            container.innerHTML = '<div style="color:#dc2626;padding:20px;">Graphviz error: ' + error.message + '</div>';
        }
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

function createSaveButton(diagramContentEl) {
    const btn = document.createElement('button');
    btn.className = 'copy-button diagram-save-btn';
    btn.style.display = 'none'; // Hidden until diagram renders
    btn.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg><span>Save</span>';
    btn.onclick = () => saveDiagramAsPng(diagramContentEl, btn);
    return btn;
}

async function saveDiagramAsPng(diagramContentEl, button) {
    const svgEl = diagramContentEl.querySelector('svg');
    if (!svgEl) return;

    try {
        const clone = svgEl.cloneNode(true);
        const rect = svgEl.getBoundingClientRect();
        const scale = 2;
        const w = Math.ceil(rect.width * scale);
        const h = Math.ceil(rect.height * scale);
        clone.setAttribute('width', w);
        clone.setAttribute('height', h);
        clone.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
        clone.style.background = 'white';

        const svgData = new XMLSerializer().serializeToString(clone);
        const dataUrl = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svgData);

        const blob = await new Promise((resolve, reject) => {
            const img = new Image();
            img.onload = () => {
                const canvas = document.createElement('canvas');
                canvas.width = w;
                canvas.height = h;
                const ctx = canvas.getContext('2d');
                ctx.fillStyle = 'white';
                ctx.fillRect(0, 0, w, h);
                ctx.drawImage(img, 0, 0, w, h);
                canvas.toBlob((b) => b ? resolve(b) : reject(new Error('toBlob failed')), 'image/png');
            };
            img.onerror = () => reject(new Error('SVG load failed'));
            img.src = dataUrl;
        });

        // Try native save dialog (Chromium File System Access API)
        if (window.showSaveFilePicker) {
            try {
                const handle = await window.showSaveFilePicker({
                    suggestedName: 'diagram.png',
                    types: [{ description: 'PNG Image', accept: { 'image/png': ['.png'] } }],
                });
                const writable = await handle.createWritable();
                await writable.write(blob);
                await writable.close();
            } catch (e) {
                if (e.name === 'AbortError') return; // User cancelled
                throw e;
            }
        } else {
            // Fallback: auto-download
            const a = document.createElement('a');
            a.href = URL.createObjectURL(blob);
            a.download = 'diagram.png';
            a.click();
            URL.revokeObjectURL(a.href);
        }

        const orig = button.innerHTML;
        button.classList.add('copied');
        button.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="20 6 9 17 4 12"></polyline></svg><span>Saved!</span>';
        setTimeout(() => { button.classList.remove('copied'); button.innerHTML = orig; }, 2000);
    } catch (err) {
        console.error('Save diagram failed:', err);
    }
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
