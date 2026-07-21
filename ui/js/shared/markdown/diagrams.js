import { t } from '../i18n.js';
import { createCopyButton, highlightOrLazy, wrapCodeBlock } from './code-controls.js';

let graphvizInstance = null;
let graphvizLoadPromise = null;
async function getGraphviz() {
    if (graphvizInstance) return graphvizInstance;
    if (graphvizLoadPromise) return graphvizLoadPromise;
    graphvizLoadPromise = (async () => {
        try {
            const module = await import('../../../vendor/lib/graphviz.js');
            const instance = await module.Graphviz.load();
            graphvizInstance = instance;
            console.log('[graphviz] WASM loaded; version:', instance.version?.());
            return instance;
        } catch (error) {
            console.warn('Failed to load Graphviz WASM:', error?.message || error);
            graphvizLoadPromise = null;
            return null;
        }
    })();
    return graphvizLoadPromise;
}

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
                    securityLevel: 'strict',
                    flowchart: { useMaxWidth: true, curve: 'basis' },
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

export async function tryBackgroundDiagramRender(existingWrapper, code, language) {
    const hash = _codeHash(code);
    if (_diagramPending.has(hash)) return;
    const failures = _diagramFailures.get(hash) || 0;
    if (failures >= MAX_STREAMING_FAILURES) return;

    _diagramPending.add(hash);

    try {
        if (language === 'mermaid') {
            const loaded = await ensureMermaid();
            if (!loaded) {
                _diagramPending.delete(hash);
                return;
            }

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
            if (!graphviz) {
                _diagramPending.delete(hash);
                return;
            }

            const engine = language === 'neato' ? 'neato' : 'dot';
            try {
                const svg = graphviz.layout(code, 'svg', engine);
                const diagramContent = existingWrapper.querySelector('.diagram-content');
                if (diagramContent && existingWrapper.isConnected) {
                    diagramContent.innerHTML = svg;
                    const svgEl = diagramContent.querySelector('svg');
                    if (svgEl) {
                        svgEl.style.maxWidth = '100%';
                        svgEl.style.height = 'auto';
                    }
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
const _diagramPending = new Set(); // code hashes currently being rendered

function _codeHash(code) {
    // Simple hash for keying — use first 200 chars + length to avoid collisions
    return code.substring(0, 200) + ':' + code.length;
}

/**
 * Reset failure tracking. Called on final render and also exposed for
 * error/session-reset paths (streams terminated without reaching the
 * successful final render would otherwise leak per-hash entries until
 * the next clean render).
 */
export function resetDiagramFailures() {
    _diagramFailures.clear();
    _diagramPending.clear();
}

/** Mark a diagram wrapper as successfully rendered and show its save button */
/**
 * Render a diagram render-failure message safely. The error text frequently
 * echoes the diagram *source* (agent-controlled content) verbatim, so it must
 * never reach innerHTML — a malformed fence with `<img onerror=…>` in the
 * failing line would otherwise execute in this privileged webview. Everything
 * here goes through textContent.
 */
function _renderDiagramError(container, label, error) {
    container.textContent = '';
    const div = document.createElement('div');
    div.style.color = 'var(--kage-error)';
    div.style.padding = '20px';
    const detail = error?.message ? `${label}: ${error.message}` : label;
    div.textContent = detail;
    container.appendChild(div);
}

function _markDiagramRendered(container) {
    const wrapper = container.closest('.diagram-wrapper');
    if (wrapper) {
        wrapper.setAttribute('data-rendered', '');
        const saveBtn = wrapper.querySelector('.diagram-save-btn');
        if (saveBtn) saveBtn.style.display = '';
    }
    // Make the diagram clickable to open lightbox (only wire once)
    if (!container._lightboxWired) {
        container._lightboxWired = true;
        container.addEventListener('click', () => {
            const svgEl = container.querySelector('svg');
            if (svgEl) openDiagramLightbox(svgEl);
        });
    }
}

export async function renderDiagram(codeBlock, pre, language, streaming = false) {
    const code = codeBlock.textContent;

    // Don't render incomplete diagrams — show as code block and wait for more content
    if (language === 'mermaid' && !code.trim()) return;
    if (
        (language === 'dot' || language === 'graphviz' || language === 'neato') &&
        !code.includes('}')
    ) {
        wrapCodeBlock(codeBlock, pre, language);
        return;
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

    const labels = {
        mermaid: 'Mermaid',
        dot: 'Graphviz',
        graphviz: 'Graphviz',
        neato: 'Graphviz (neato)',
    };

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
    toggleBtn.innerHTML =
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="16 18 22 12 16 6"></polyline><polyline points="8 6 2 12 8 18"></polyline></svg><span>Source</span>';
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
        highlightOrLazy(placeholderCode, language);
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
    } else {
        await renderGraphvizInto(diagramDiv, code, language, streaming);
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
            container.innerHTML =
                '<div style="color:#dc2626;padding:20px;">Failed to load Mermaid library</div>';
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
            _renderDiagramError(container, t('shared.markdown.diagram.render_error'), error);
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
                _renderDiagramError(container, t('shared.markdown.diagram.graphviz_wasm_failed'));
            }
            return;
        }
        const engine = language === 'neato' ? 'neato' : 'dot';
        const svg = graphviz.layout(code, 'svg', engine);
        _diagramPending.delete(hash);
        // Success — replace placeholder with rendered SVG
        container.innerHTML = svg;
        const svgEl = container.querySelector('svg');
        if (svgEl) {
            svgEl.style.maxWidth = '100%';
            svgEl.style.height = 'auto';
        }
        _markDiagramRendered(container);
    } catch (error) {
        _diagramPending.delete(hash);
        if (streaming) {
            _diagramFailures.set(hash, (_diagramFailures.get(hash) || 0) + 1);
            // Leave the placeholder (formatted source) intact
        } else {
            console.error('Graphviz rendering error:', error);
            _renderDiagramError(container, t('shared.markdown.diagram.graphviz_error'), error);
        }
    }
}

// --- HTML preview rendering ---

function createSaveButton(diagramContentEl) {
    const btn = document.createElement('button');
    btn.className = 'copy-button diagram-save-btn';
    btn.style.display = 'none'; // Hidden until diagram renders
    btn.innerHTML =
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg><span>Save</span>';
    btn.onclick = () => saveDiagramAsPng(diagramContentEl, btn);
    return btn;
}

async function saveDiagramAsPng(diagramContentEl, button) {
    const svgEl = diagramContentEl.querySelector('svg');
    if (!svgEl) return;

    try {
        const blob = await _renderDiagramToBlob(svgEl, 4);

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
        button.innerHTML =
            '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="20 6 9 17 4 12"></polyline></svg><span>Saved!</span>';
        setTimeout(() => {
            button.classList.remove('copied');
            button.innerHTML = orig;
        }, 2000);
    } catch (err) {
        console.error('Save diagram failed:', err);
    }
}

/**
 * Render an SVG element to a PNG blob at the given scale factor.
 * Uses the SVG's intrinsic viewBox dimensions when available for higher quality.
 */
async function _renderDiagramToBlob(svgEl, scale = 4) {
    const clone = svgEl.cloneNode(true);

    // Use intrinsic viewBox dimensions if available (higher quality than displayed size)
    let baseW, baseH;
    const viewBox = svgEl.getAttribute('viewBox');
    if (viewBox) {
        const parts = viewBox.split(/[\s,]+/).map(Number);
        if (parts.length === 4 && parts[2] > 0 && parts[3] > 0) {
            baseW = parts[2];
            baseH = parts[3];
        }
    }
    if (!baseW || !baseH) {
        const rect = svgEl.getBoundingClientRect();
        baseW = rect.width;
        baseH = rect.height;
    }

    const w = Math.ceil(baseW * scale);
    const h = Math.ceil(baseH * scale);
    clone.setAttribute('width', w);
    clone.setAttribute('height', h);
    clone.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
    clone.style.background = 'white';

    const svgData = new XMLSerializer().serializeToString(clone);
    const dataUrl = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svgData);

    return new Promise((resolve, reject) => {
        const img = new Image();
        img.onload = () => {
            const canvas = document.createElement('canvas');
            canvas.width = w;
            canvas.height = h;
            const ctx = canvas.getContext('2d');
            ctx.fillStyle = 'white';
            ctx.fillRect(0, 0, w, h);
            ctx.drawImage(img, 0, 0, w, h);
            canvas.toBlob(
                (b) => (b ? resolve(b) : reject(new Error('toBlob failed'))),
                'image/png'
            );
        };
        img.onerror = () => reject(new Error('SVG load failed'));
        img.src = dataUrl;
    });
}

/**
 * Open a diagram in a full-screen lightbox overlay.
 * Click the backdrop or press Escape to close. Save button exports at 4x.
 */
function openDiagramLightbox(svgEl) {
    // Remove any existing lightbox
    closeDiagramLightbox();

    const overlay = document.createElement('div');
    overlay.className = 'diagram-lightbox';
    overlay.id = 'diagramLightbox';

    // Clone the SVG at full size — remove max-width constraints
    const clone = svgEl.cloneNode(true);
    clone.style.maxWidth = '95vw';
    clone.style.maxHeight = '85vh';
    clone.style.width = 'auto';
    clone.style.height = 'auto';
    clone.removeAttribute('width');
    clone.removeAttribute('height');
    overlay.appendChild(clone);

    // Action buttons
    const actions = document.createElement('div');
    actions.className = 'diagram-lightbox-actions';

    const saveBtn = document.createElement('button');
    saveBtn.className = 'diagram-lightbox-btn';
    saveBtn.textContent = t('shared.markdown.save_btn');
    saveBtn.addEventListener('click', async (e) => {
        e.stopPropagation();
        try {
            const blob = await _renderDiagramToBlob(svgEl, 4);
            if (window.showSaveFilePicker) {
                try {
                    const handle = await window.showSaveFilePicker({
                        suggestedName: 'diagram.png',
                        types: [{ description: 'PNG Image', accept: { 'image/png': ['.png'] } }],
                    });
                    const writable = await handle.createWritable();
                    await writable.write(blob);
                    await writable.close();
                } catch (ex) {
                    if (ex.name === 'AbortError') return;
                    throw ex;
                }
            } else {
                const a = document.createElement('a');
                a.href = URL.createObjectURL(blob);
                a.download = 'diagram.png';
                a.click();
                URL.revokeObjectURL(a.href);
            }
            saveBtn.textContent = t('shared.markdown.save_btn.saved');
            setTimeout(() => {
                saveBtn.textContent = t('shared.markdown.save_btn');
            }, 2000);
        } catch (err) {
            console.error('Lightbox save failed:', err);
        }
    });
    actions.appendChild(saveBtn);

    const closeBtn = document.createElement('button');
    closeBtn.className = 'diagram-lightbox-btn diagram-lightbox-close';
    closeBtn.textContent = '✕';
    closeBtn.title = t('shared.markdown.close.title');
    closeBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        closeDiagramLightbox();
    });
    actions.appendChild(closeBtn);

    overlay.appendChild(actions);

    // Click backdrop to close (stopPropagation prevents floating window from hiding)
    overlay.addEventListener('click', (e) => {
        e.stopPropagation();
        if (e.target === overlay) closeDiagramLightbox();
    });

    // Escape to close (capture phase, stop propagation so floating window doesn't hide)
    overlay._escHandler = (e) => {
        if (e.key === 'Escape') {
            e.preventDefault();
            e.stopPropagation();
            closeDiagramLightbox();
        }
    };
    document.addEventListener('keydown', overlay._escHandler, true);

    document.body.appendChild(overlay);
}

function closeDiagramLightbox() {
    const existing = document.getElementById('diagramLightbox');
    if (existing) {
        if (existing._escHandler)
            document.removeEventListener('keydown', existing._escHandler, true);
        existing.remove();
    }
}
