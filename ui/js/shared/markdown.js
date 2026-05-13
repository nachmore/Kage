// Markdown rendering with code block, mermaid, graphviz, and PlantUML support

import { loadPrismLanguage } from './prism-loader.js';

const DIAGRAM_LANGUAGES = new Set(['mermaid', 'plantuml', 'puml', 'dot', 'graphviz', 'neato']);
const HTML_LANGUAGES = new Set(['html', 'htm']);
const JSON_LANGUAGES = new Set(['json', 'jsonc']);

// Escape raw HTML the agent emits inside markdown.
//
// marked passes raw HTML through verbatim by default. The pre-fix guard
// further down only catches the case where the *first* non-whitespace token
// is <script> / <html> / etc — anything later (e.g. "Here is the page:\n\n
// <script>alert(1)</script>") slipped through and ran inside the chat
// window. Override the renderer.html hook (used for both block-level and
// inline raw HTML tokens — see marked.umd.js cases at "html": in both the
// block parser and the inline parser) to escape every raw HTML token to
// plain text. Fenced code blocks (```html\n...\n```) are routed through
// renderer.code, not renderer.html, so source listings still render as
// syntax-highlighted code.
//
// We use a private DOM-based escape so we don't depend on import order
// during module init — markdown.js is loaded as a non-module script tag
// alongside marked, so static `import` of escapeHtml from tool-utils.js
// would create a load-order coupling we don't otherwise have here.
export function _escapeHtmlForMarked(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}
let _markedHardenedFlag = false;
/** Test-only: re-arm the hardening guard. Production code never calls this. */
export function _resetMarkedHardenedFlagForTests() {
    _markedHardenedFlag = false;
}
export function hardenMarkedOnce() {
    if (_markedHardenedFlag) return;
    if (typeof marked === 'undefined' || !marked.use) return;
    marked.use({
        renderer: {
            html(token) {
                return _escapeHtmlForMarked(token.text || '');
            },
        },
    });
    _markedHardenedFlag = true;
}
// Hardening must run before any marked.parse() call. Trying it now covers
// the common case (marked already loaded as a <script> tag); _doRender
// retries below as a safety net for the case where this module is imported
// before the marked vendor script has finished loading.
hardenMarkedOnce();

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
                    flowchart: { useMaxWidth: true, htmlLabels: true, curve: 'basis' },
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

// Extension manager reference for message formatter hooks
let _extensionManager = null;

/**
 * Set the extension manager so message formatters can run after rendering.
 * @param {ExtensionManager} em
 */
export function setExtensionManager(em) {
    _extensionManager = em;
}

// --- Streaming-aware debounced rendering ---

const STREAMING_RENDER_INTERVAL = 150; // ms between renders during streaming
const _renderTimers = new WeakMap(); // targetElement → timer id
const _lastRenderTime = new WeakMap(); // targetElement → timestamp

// --- App icon inline rendering ---
// Replaces <app-icon name="processName"/> tags with inline icon images.
// Icons are fetched from the Rust side (cached by process name) and cached here.
const _appIconCache = new Map(); // process name → data URI or null
let _appIconInvoke = null;

/**
 * Set the invoke function for app icon lookups.
 * Call this once during init (same invoke used everywhere).
 */
export function setAppIconInvoke(invoke) {
    _appIconInvoke = invoke;
}

/**
 * Process <app-icon> tags in a rendered container, replacing them with inline images.
 * Uses regex on innerHTML since browsers don't reliably parse self-closing custom tags.
 * Async — fetches icons that aren't cached yet and updates the DOM when ready.
 */
function _processAppIcons(container) {
    if (!_appIconInvoke) return;
    const html = container.innerHTML;
    if (!html.includes('<app-icon')) return;

    // Replace all <app-icon name="..."/> or <app-icon name="..."></app-icon> with placeholders
    const pending = [];
    const replaced = html.replace(
        /<app-icon\s+name="([^"]+)"\s*\/?>(?:<\/app-icon>)?/gi,
        (_match, name) => {
            const key = name.toLowerCase();
            if (_appIconCache.has(key)) {
                const dataUri = _appIconCache.get(key);
                if (dataUri) {
                    const src = dataUri.startsWith('data:')
                        ? dataUri
                        : 'data:image/png;base64,' + dataUri;
                    return `<img src="${src}" alt="${name}" title="${name}" style="width:1.2em;height:1.2em;vertical-align:middle;border-radius:3px;margin:0 2px;">`;
                }
                return ''; // no icon available
            }
            // Not cached yet — insert an empty placeholder span and fetch async
            const id = `app-icon-${_appIconPlaceholderId++}`;
            pending.push({ id, name, key });
            return `<span id="${id}" style="display:inline-block;width:1.2em;height:1.2em;vertical-align:middle;"></span>`;
        }
    );

    if (replaced !== html) {
        container.innerHTML = replaced;
    }

    // Fetch uncached icons and replace placeholders
    for (const { id, name, key } of pending) {
        _appIconInvoke('get_app_icon', { processName: name })
            .then((dataUri) => {
                _appIconCache.set(key, dataUri || null);
                const placeholder = container.querySelector(`#${id}`);
                if (placeholder) {
                    if (dataUri) {
                        const src = dataUri.startsWith('data:')
                            ? dataUri
                            : 'data:image/png;base64,' + dataUri;
                        placeholder.outerHTML = `<img src="${src}" alt="${name}" title="${name}" style="width:1.2em;height:1.2em;vertical-align:middle;border-radius:3px;margin:0 2px;">`;
                    } else {
                        placeholder.outerHTML = ''; // no icon available
                    }
                }
            })
            .catch(() => {
                _appIconCache.set(key, null);
            });
    }
}

let _appIconPlaceholderId = 0;

// --- Incremental rendering state (per target element) ---
// During streaming, we split markdown into a "frozen prefix" (complete blocks
// that won't change) and an "active tail" (the last incomplete block).
// Only the tail is re-parsed on each chunk, turning O(n²) into ~O(n).
const _frozenHtml = new WeakMap(); // targetElement → rendered HTML string for frozen prefix
const _frozenLength = new WeakMap(); // targetElement → char count of frozen markdown prefix

/**
 * Render markdown into a target element.
 * @param {string} markdown - raw markdown text
 * @param {HTMLElement} targetElement - DOM element to render into
 * @param {boolean} [streaming=false] - true while chunks are arriving; throttles
 *   rendering and skips expensive diagram/table work until complete
 */
export function renderMarkdown(markdown, targetElement, streaming = false) {
    if (!markdown) {
        targetElement.innerHTML = '';
        _frozenHtml.delete(targetElement);
        _frozenLength.delete(targetElement);
        return;
    }

    if (!streaming) {
        // Final render — cancel any pending debounce, clear incremental state, do full render
        const pending = _renderTimers.get(targetElement);
        if (pending) {
            clearTimeout(pending);
            _renderTimers.delete(targetElement);
        }
        _frozenHtml.delete(targetElement);
        _frozenLength.delete(targetElement);
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
        if (pending) {
            clearTimeout(pending);
            _renderTimers.delete(targetElement);
        }
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

    // Re-attempt the renderer.html override in case marked finished loading
    // after this module did. hardenMarkedOnce is idempotent — guarded by
    // a flag — so this is free if hardening already ran.
    hardenMarkedOnce();

    // Display nicety: if the response starts with a full HTML document
    // (<!DOCTYPE>, <html>, ...), wrap it in an html code fence so it
    // renders as a styled source listing instead of an ugly stream of
    // escaped tags. The renderer.html override above is the *security*
    // guarantee — this branch is just polish for the "agent pasted a
    // whole page" UX. Don't extend the trusted-input set here.
    if (typeof markdown === 'string') {
        const head = markdown.trimStart().slice(0, 200).toLowerCase();
        if (/^<(!doctype|html[\s>]|body[\s>]|head[\s>]|script[\s>]|style[\s>])/.test(head)) {
            markdown = '```html\n' + markdown + '\n```';
        }
    }

    // During streaming, if we detect an incomplete automation_plan block,
    // don't render it as raw code — the app's handleMessageChunk will render
    // the task list incrementally. Just show nothing for the plan portion.
    if (streaming && markdown.includes('```automation_plan')) {
        const completeBlock = /```automation_plan\s*\n[\s\S]*?\n```/.test(markdown);
        if (!completeBlock) {
            // Block is still being streamed — strip it and render any text before it
            const beforeBlock = markdown.split('```automation_plan')[0].trim();
            if (beforeBlock) {
                targetElement.innerHTML = marked.parse(beforeBlock, { breaks: true });
            }
            // Don't render anything for the incomplete block — the app handles it
            return;
        }
    }

    // Strip extension_tool_call fences from rendered output — they are handled
    // programmatically by the app and should never appear as visible code blocks.
    // Handle both complete and incomplete (still streaming) fences.
    if (markdown.includes('```extension_tool_call')) {
        // Strip complete fences
        markdown = markdown.replace(/```extension_tool_call\s*\n[\s\S]*?\n```/g, '');
        // Strip incomplete fence (still being streamed or tool executing)
        const incompleteIdx = markdown.indexOf('```extension_tool_call');
        if (incompleteIdx !== -1) {
            markdown = markdown.substring(0, incompleteIdx);
        }
        markdown = markdown.trim();
    }

    // Strip suggested_actions fences — rendered as clickable chips by the app, not as text.
    if (markdown.includes('```suggested_actions')) {
        markdown = markdown.replace(/```suggested_actions\s*\n[\s\S]*?\n```/g, '');
        // Also strip incomplete fence during streaming
        const incompleteIdx = markdown.indexOf('```suggested_actions');
        if (incompleteIdx !== -1) {
            markdown = markdown.substring(0, incompleteIdx);
        }
        markdown = markdown.trim();
    }

    // Deduplicate taskplan blocks at the source level — keep only the last one.
    // The agent re-outputs the full taskplan block each time it updates status,
    // so we strip all but the final occurrence to avoid showing stale versions.
    markdown = _keepLastTaskPlan(markdown);

    // --- Incremental streaming render ---
    // Split into frozen prefix (complete blocks) and active tail (last block).
    // Only re-parse the tail; reuse cached HTML for the prefix.
    if (streaming) {
        const splitIdx = _findStableSplitPoint(markdown);
        const prefixMd = splitIdx > 0 ? markdown.substring(0, splitIdx) : '';
        const tailMd = splitIdx > 0 ? markdown.substring(splitIdx) : markdown;
        const prevFrozenLen = _frozenLength.get(targetElement) || 0;

        // Detect if the prefix shrank or disappeared — need a full rebuild
        const prefixShrunk = prevFrozenLen > 0 && prefixMd.length < prevFrozenLen;

        // If the prefix grew (or shrank — rebuild), render and cache the new frozen prefix
        if (prefixMd.length > prevFrozenLen || prefixShrunk) {
            // Preserve rendered diagrams from the frozen section before re-rendering
            const savedDiagrams = new Map();
            targetElement.querySelectorAll('.diagram-wrapper[data-rendered]').forEach((wrapper) => {
                const sourceEl = wrapper.querySelector('.diagram-source code');
                if (sourceEl) {
                    savedDiagrams.set(sourceEl.textContent, wrapper);
                    wrapper.remove();
                }
            });

            marked.setOptions({ breaks: true, gfm: true });
            const frozenRendered = prefixMd ? marked.parse(prefixMd) : '';
            _frozenHtml.set(targetElement, frozenRendered);
            _frozenLength.set(targetElement, prefixMd.length);

            // Build frozen container (only if there's a prefix)
            const frozenDiv = prefixMd ? document.createElement('div') : null;
            if (frozenDiv) {
                frozenDiv.className = 'markdown-frozen';
                frozenDiv.innerHTML = frozenRendered;

                // Process code blocks in the newly frozen content
                _processCodeBlocks(frozenDiv, true, savedDiagrams);
            }

            // Build tail container — preserve any diagrams from the previous tail
            const tailSavedDiagrams = new Map();
            const prevTail = targetElement.querySelector('.markdown-tail');
            if (prevTail) {
                prevTail.querySelectorAll('.diagram-wrapper').forEach((wrapper) => {
                    const sourceEl = wrapper.querySelector('.diagram-source code');
                    if (sourceEl) {
                        tailSavedDiagrams.set(sourceEl.textContent, wrapper);
                        wrapper.remove();
                    }
                });
            }

            const tailDiv = document.createElement('div');
            tailDiv.className = 'markdown-tail';
            if (tailMd.trim()) {
                tailDiv.innerHTML = marked.parse(tailMd);
                _processCodeBlocks(tailDiv, true, tailSavedDiagrams);
            }

            targetElement.innerHTML = '';
            if (frozenDiv) targetElement.appendChild(frozenDiv);
            targetElement.appendChild(tailDiv);
        } else {
            // Prefix unchanged — only re-render the tail
            let tailDiv = targetElement.querySelector('.markdown-tail');
            if (!tailDiv) {
                // First render or structure mismatch — do a full incremental setup
                const savedDiagrams = new Map();
                targetElement
                    .querySelectorAll('.diagram-wrapper[data-rendered]')
                    .forEach((wrapper) => {
                        const sourceEl = wrapper.querySelector('.diagram-source code');
                        if (sourceEl) {
                            savedDiagrams.set(sourceEl.textContent, wrapper);
                            wrapper.remove();
                        }
                    });

                marked.setOptions({ breaks: true, gfm: true });

                if (prefixMd) {
                    const frozenRendered = _frozenHtml.get(targetElement) || marked.parse(prefixMd);
                    _frozenHtml.set(targetElement, frozenRendered);
                    _frozenLength.set(targetElement, prefixMd.length);

                    const frozenDiv = document.createElement('div');
                    frozenDiv.className = 'markdown-frozen';
                    frozenDiv.innerHTML = frozenRendered;
                    _processCodeBlocks(frozenDiv, true, savedDiagrams);

                    tailDiv = document.createElement('div');
                    tailDiv.className = 'markdown-tail';

                    targetElement.innerHTML = '';
                    targetElement.appendChild(frozenDiv);
                    targetElement.appendChild(tailDiv);
                } else {
                    tailDiv = document.createElement('div');
                    tailDiv.className = 'markdown-tail';
                    targetElement.innerHTML = '';
                    targetElement.appendChild(tailDiv);
                }
            }

            // Re-render only the tail — preserve any rendered diagrams first
            const tailSavedDiagrams = new Map();
            tailDiv.querySelectorAll('.diagram-wrapper[data-rendered]').forEach((wrapper) => {
                const sourceEl = wrapper.querySelector('.diagram-source code');
                if (sourceEl) {
                    tailSavedDiagrams.set(sourceEl.textContent, wrapper);
                    wrapper.remove();
                }
            });
            // Also save diagram wrappers that haven't fully rendered yet but have
            // a visible diagram-content (e.g. placeholder or partial render) — this
            // prevents the flash-to-code-block cycle during streaming.
            tailDiv.querySelectorAll('.diagram-wrapper:not([data-rendered])').forEach((wrapper) => {
                const sourceEl = wrapper.querySelector('.diagram-source code');
                if (sourceEl) {
                    tailSavedDiagrams.set(sourceEl.textContent, wrapper);
                    wrapper.remove();
                }
            });

            if (tailMd.trim()) {
                marked.setOptions({ breaks: true, gfm: true });
                tailDiv.innerHTML = marked.parse(tailMd);
                _processCodeBlocks(tailDiv, true, tailSavedDiagrams);
            } else {
                tailDiv.innerHTML = '';
            }
        }

        // Process app-icon tags
        _processAppIcons(targetElement);

        // Deduplicate taskplan blocks
        _deduplicateTaskPlans(targetElement);

        // Run extension message formatters
        if (_extensionManager) {
            _extensionManager.formatMessage(targetElement, { streaming });
        }
        return;
    }

    // --- Full (non-streaming) render ---
    // During streaming, preserve successfully rendered diagrams so they don't
    // flash back to source code when innerHTML is replaced.  Key by source text.
    const savedDiagrams = new Map(); // code text → DOM wrapper element

    marked.setOptions({ breaks: true, gfm: true });
    targetElement.innerHTML = marked.parse(markdown);

    _processCodeBlocks(targetElement, false, savedDiagrams);

    // Only wire up sortable tables on the final render
    _resetDiagramFailures();
    makeTablesSortable(targetElement);

    // Process app-icon tags
    _processAppIcons(targetElement);

    // Deduplicate taskplan blocks — keep only the last one (most up-to-date)
    _deduplicateTaskPlans(targetElement);

    // Run extension message formatters
    if (_extensionManager) {
        _extensionManager.formatMessage(targetElement, { streaming });
    }
}

/**
 * Find the last safe split point in markdown where we can freeze the prefix.
 * A safe split is a double-newline (\n\n) that is NOT inside a code fence.
 * Returns the index right after the \n\n, or 0 if no safe split found.
 *
 * Exported (with underscore prefix to preserve "private" intent) so the
 * incremental streaming logic can be exercised in isolation — this is the
 * function the 2026-04 OOM regression touched. The underscore signals
 * "implementation detail, don't import elsewhere".
 */
export function _findStableSplitPoint(markdown) {
    let inFence = false;
    let lastSafeSplit = 0;

    // Scan for code fences and double-newlines
    let i = 0;
    while (i < markdown.length) {
        // Check for code fence (``` at start of line)
        if (
            (i === 0 || markdown[i - 1] === '\n') &&
            markdown[i] === '`' &&
            markdown[i + 1] === '`' &&
            markdown[i + 2] === '`'
        ) {
            inFence = !inFence;
            i += 3;
            // Skip to end of line
            while (i < markdown.length && markdown[i] !== '\n') i++;
            continue;
        }

        // Check for double-newline outside of fences
        if (!inFence && markdown[i] === '\n' && markdown[i + 1] === '\n') {
            lastSafeSplit = i + 2;
            i += 2;
            continue;
        }

        i++;
    }

    // Don't freeze if the split is too close to the end — not worth it
    if (markdown.length - lastSafeSplit < 50) return 0;

    return lastSafeSplit;
}

/**
 * Highlight a code block with the given language. If the Prism component
 * for that language isn't loaded yet, kicks off the load and reapplies
 * highlight to the same node when it arrives. Mid-load the block stays
 * unhighlighted (plain text), which is fine — Prism's highlighting is
 * cosmetic and we'd rather render the response immediately than block
 * on a fetch.
 *
 * Used to be: 15 language packs eagerly loaded at every window startup
 * via `<script defer>` tags. Now only `prism.js` core is eager; each
 * pack loads on first use of its language.
 */
function _highlightOrLazy(codeBlock, language) {
    if (typeof Prism === 'undefined') return;
    if (!language || language === 'text') return;
    if (Prism.languages[language]) {
        try {
            codeBlock.innerHTML = Prism.highlight(
                codeBlock.textContent,
                Prism.languages[language],
                language
            );
            codeBlock.className = 'language-' + language;
        } catch {
            /* skip */
        }
        return;
    }
    // Capture the source text now — by the time the load resolves, the
    // codeBlock element may have been replaced (the streaming renderer
    // throws away nodes between debounced repaints). Re-highlighting a
    // detached node is harmless; if it's still attached the user sees
    // the colors arrive a beat later.
    const source = codeBlock.textContent;
    loadPrismLanguage(language)
        .then(() => {
            if (!Prism.languages[language]) return;
            try {
                codeBlock.innerHTML = Prism.highlight(source, Prism.languages[language], language);
                codeBlock.className = 'language-' + language;
            } catch {
                /* skip */
            }
        })
        .catch(() => {
            /* unknown language or offline — leave unhighlighted */
        });
}

/**
 * Process code blocks in a container: syntax highlighting, diagrams, etc.
 * Extracted from _doRender so it can be called on frozen and tail sections independently.
 */
function _processCodeBlocks(container, streaming, savedDiagrams) {
    container.querySelectorAll('pre code').forEach((codeBlock) => {
        const pre = codeBlock.parentElement;
        const langMatch = codeBlock.className.match(/language-(\w+)/);
        const language = langMatch ? langMatch[1] : 'text';

        if (DIAGRAM_LANGUAGES.has(language)) {
            const code = codeBlock.textContent;
            if (streaming) {
                // Reinsert the last successful render immediately (no flash)
                if (savedDiagrams.size > 0) {
                    const saved = savedDiagrams.get(code) || savedDiagrams.values().next().value;
                    const savedKey = savedDiagrams.has(code)
                        ? code
                        : savedDiagrams.keys().next().value;
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
        if (language === 'taskplan') {
            renderTaskPlan(codeBlock, pre);
            return;
        }
        if (language === 'automation_plan') {
            // Render as a pending task list during streaming
            try {
                const plan = JSON.parse(codeBlock.textContent.trim());
                if (Array.isArray(plan) && plan.length > 0 && plan[0].task) {
                    const tasks = plan.map((s) => ({
                        status: 'pending',
                        description: s.task,
                        detail: s.details || '',
                    }));
                    const wrapper = createTaskPlanElement(tasks);
                    wrapper.dataset.automationPlan = 'true';
                    pre.parentNode.insertBefore(wrapper, pre);
                    pre.remove();
                    return;
                }
            } catch {
                /* fall through to default rendering */
            }
        }
        if (HTML_LANGUAGES.has(language)) {
            renderHtmlPreview(codeBlock, pre);
            return;
        }
        if (JSON_LANGUAGES.has(language)) {
            renderJsonTree(codeBlock, pre, language);
            return;
        }
        _highlightOrLazy(codeBlock, language);
        wrapCodeBlock(codeBlock, pre, language);
    });
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

    const actions = document.createElement('div');
    actions.className = 'code-block-actions';
    // Add Try button for JavaScript code blocks
    const jsLangs = ['javascript', 'js', 'jsx', 'typescript', 'ts', 'tsx'];
    if (jsLangs.includes((language || '').toLowerCase())) {
        actions.appendChild(createTryButton(codeBlock, wrapper));
    }
    actions.appendChild(createCopyButton(codeBlock.textContent));
    header.appendChild(actions);

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
    // Make the diagram clickable to open lightbox (only wire once)
    if (!container._lightboxWired) {
        container._lightboxWired = true;
        container.addEventListener('click', () => {
            const svgEl = container.querySelector('svg');
            if (svgEl) openDiagramLightbox(svgEl);
        });
    }
}

async function renderDiagram(codeBlock, pre, language, streaming = false) {
    const code = codeBlock.textContent;

    // Don't render incomplete diagrams — show as code block and wait for more content
    if (language === 'mermaid' && !code.trim()) return;
    if ((language === 'plantuml' || language === 'puml') && !code.includes('@enduml')) {
        wrapCodeBlock(codeBlock, pre, language);
        return;
    }
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
        plantuml: 'PlantUML',
        puml: 'PlantUML',
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
        _highlightOrLazy(placeholderCode, language);
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
            container.innerHTML =
                '<div style="color:#dc2626;padding:20px;">Error: ' + error.message + '</div>';
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
                container.innerHTML =
                    '<div style="color:#dc2626;padding:20px;">Graphviz WASM failed to load</div>';
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
            container.innerHTML =
                '<div style="color:#dc2626;padding:20px;">Graphviz error: ' +
                error.message +
                '</div>';
        }
    }
}

function renderPlantUMLInto(container, code) {
    // PlantUML requires Java — no pure JS renderer exists. Show formatted source.
    const pre = document.createElement('pre');
    pre.style.cssText = 'margin:0;padding:16px;background:#272822;overflow-x:auto';
    const codeEl = document.createElement('code');
    codeEl.style.cssText =
        "font-family:'Consolas','Monaco','Courier New',monospace;font-size:13px;line-height:1.5;color:#f8f8f2;white-space:pre";
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
    toggleBtn.innerHTML =
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="16 18 22 12 16 6"></polyline><polyline points="8 6 2 12 8 18"></polyline></svg><span>Source</span>';
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
            doc.querySelectorAll('script').forEach((s) => s.remove());
            const h = doc.documentElement.scrollHeight || doc.body.scrollHeight;
            iframe.style.height = Math.min(Math.max(h, 60), 600) + 'px';
        } catch {
            /* cross-origin, ignore */
        }
    };

    const sourceDiv = document.createElement('div');
    sourceDiv.className = 'diagram-source';
    const sPre = document.createElement('pre');
    const sCode = document.createElement('code');
    sCode.textContent = code;
    // Use 'markup' as the language so the lazy loader fetches prism-markup;
    // Prism aliases 'html' → 'markup' internally so highlight() accepts both.
    _highlightOrLazy(sCode, 'markup');
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

// --- JSON Tree Renderer ---

function renderJsonTree(codeBlock, pre, language) {
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
    label.textContent = 'JSON';

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
    _highlightOrLazy(sCode, 'json');
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

function createCopyButton(code) {
    const btn = document.createElement('button');
    btn.className = 'copy-button';
    btn.innerHTML =
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg><span>Copy</span>';
    btn.onclick = () => copyCode(code, btn);
    return btn;
}

function copyCode(code, button) {
    navigator.clipboard
        .writeText(code)
        .then(() => {
            const orig = button.innerHTML;
            button.classList.add('copied');
            button.innerHTML =
                '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="20 6 9 17 4 12"></polyline></svg><span>Copied!</span>';
            setTimeout(() => {
                button.classList.remove('copied');
                button.innerHTML = orig;
            }, 2000);
        })
        .catch((err) => console.error('Copy failed:', err));
}

function createTryButton(_codeBlock, _wrapper) {
    const btn = document.createElement('button');
    btn.className = 'copy-button try-button';
    btn.innerHTML =
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="5 3 19 12 5 21 5 3"></polygon></svg><span>Try</span>';
    btn.onclick = () => {
        const liveWrapper = btn.closest('.code-block-wrapper');
        if (!liveWrapper) return;
        const liveCode = liveWrapper.querySelector('code');
        if (!liveCode) return;
        runCodeInSandbox(liveCode.textContent, liveWrapper, btn);
    };
    return btn;
}

const _tryPlayIcon =
    '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="5 3 19 12 5 21 5 3"></polygon></svg><span>Try</span>';
const _tryStopIcon =
    '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="6" y="6" width="12" height="12" rx="1"></rect></svg><span>Stop</span>';

function runCodeInSandbox(code, wrapper, button) {
    // Flag to suppress blur-hide while sandbox iframe is being created
    window._kageSandboxActive = true;

    // If already running, stop it
    if (wrapper._kageSandboxCleanup) {
        wrapper._kageSandboxCleanup();
        return;
    }

    // Remove any previous output
    const prev = wrapper.querySelector('.try-output');
    if (prev) prev.remove();
    const prevIframe = wrapper._kageSandboxIframe;
    if (prevIframe?.parentNode) prevIframe.remove();

    // Create output container
    const output = document.createElement('div');
    output.className = 'try-output';

    const outputHeader = document.createElement('div');
    outputHeader.className = 'try-output-header';
    outputHeader.innerHTML = '<span>Console Output</span>';
    const closeBtn = document.createElement('button');
    closeBtn.className = 'try-output-close';
    closeBtn.textContent = '✕';
    closeBtn.onclick = () => {
        cleanup();
        output.remove();
    };
    outputHeader.appendChild(closeBtn);
    output.appendChild(outputHeader);

    const outputBody = document.createElement('pre');
    outputBody.className = 'try-output-body';
    output.appendChild(outputBody);
    wrapper.appendChild(output);

    // Switch button to Stop mode
    button.innerHTML = _tryStopIcon;
    button.classList.add('try-button-running');

    // Build sandboxed iframe
    const iframe = document.createElement('iframe');
    iframe.sandbox = 'allow-scripts';
    iframe.style.cssText = 'display:none;width:0;height:0;border:0;';
    document.body.appendChild(iframe);
    wrapper._kageSandboxIframe = iframe;

    let finished = false;
    let timeout;

    function appendLine(cls, text) {
        const line = document.createElement('div');
        line.className = 'try-output-line ' + cls;
        line.textContent = text;
        outputBody.appendChild(line);
        outputBody.scrollTop = outputBody.scrollHeight;
    }

    function cleanup(reason) {
        if (finished) return;
        finished = true;
        window._kageSandboxActive = false;
        clearTimeout(timeout);
        window.removeEventListener('message', onMessage);
        if (iframe.parentNode) iframe.remove();
        wrapper._kageSandboxIframe = null;
        wrapper._kageSandboxCleanup = null;
        button.innerHTML = _tryPlayIcon;
        button.classList.remove('try-button-running');
        if (reason === 'stopped') appendLine('try-output-dim', '⏹ Stopped');
        else if (reason === 'timeout') appendLine('try-output-warn', '⏱ Timed out (30s)');
        if (outputBody.children.length === 0) {
            appendLine('try-output-dim', '(no output)');
        }
    }

    wrapper._kageSandboxCleanup = () => cleanup('stopped');

    function onMessage(e) {
        if (e.source !== iframe.contentWindow) return;
        const msg = e.data;
        if (!msg || msg._kageSandbox !== true) return;
        if (msg.type === 'log') appendLine('', msg.args.map(String).join(' '));
        else if (msg.type === 'warn') appendLine('try-output-warn', msg.args.map(String).join(' '));
        else if (msg.type === 'error')
            appendLine('try-output-error', msg.args.map(String).join(' '));
        else if (msg.type === 'result') {
            if (msg.value !== undefined && msg.value !== 'undefined') {
                appendLine('try-output-result', '→ ' + msg.value);
            }
        } else if (msg.type === 'exception') appendLine('try-output-error', '✕ ' + msg.message);
        else if (msg.type === 'done') cleanup();
    }
    window.addEventListener('message', onMessage);

    // 30s hard limit for runaway code
    timeout = setTimeout(() => {
        if (!finished) cleanup('timeout');
    }, 30000);

    const sandboxScript = `
        <script>
        (function() {
            function send(type, data) {
                parent.postMessage(Object.assign({ _kageSandbox: true, type: type }, data), '*');
            }
            window.onerror = function(msg) {
                send('exception', { message: String(msg) });
                send('done', {});
                return true;
            };
            ['log','warn','error'].forEach(function(m) {
                console[m] = function() {
                    send(m, { args: Array.from(arguments).map(function(a) {
                        try { return typeof a === 'object' ? JSON.stringify(a) : String(a); }
                        catch(e) { return String(a); }
                    })});
                };
            });
            try {
                var __code = ${JSON.stringify(code)};
                // If the code is a single expression (no semicolons except trailing,
                // no control flow), wrap it in a return so the result is captured.
                var __trimmed = __code.trim().replace(/;\\s*$/, '');
                var __result;
                try { __result = (new Function('return (' + __trimmed + ')'))(); }
                catch(e) { __result = (new Function(__code))(); }
                if (__result !== undefined) {
                    var display;
                    try { display = typeof __result === 'object' ? JSON.stringify(__result, null, 2) : String(__result); }
                    catch(e) { display = String(__result); }
                    send('result', { value: display });
                }
            } catch(e) {
                send('exception', { message: (e.name || 'Error') + ': ' + e.message });
            }
            send('done', {});
        })();
        </script>
    `;
    iframe.srcdoc = sandboxScript;
}

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
    saveBtn.textContent = 'Save';
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
            saveBtn.textContent = 'Saved!';
            setTimeout(() => {
                saveBtn.textContent = 'Save';
            }, 2000);
        } catch (err) {
            console.error('Lightbox save failed:', err);
        }
    });
    actions.appendChild(saveBtn);

    const closeBtn = document.createElement('button');
    closeBtn.className = 'diagram-lightbox-btn diagram-lightbox-close';
    closeBtn.textContent = '✕';
    closeBtn.title = 'Close';
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

function makeTablesSortable(container) {
    container.querySelectorAll('table').forEach((table) => {
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
                headers.forEach((h) => {
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
                    if (!Number.isNaN(aNum) && !Number.isNaN(bNum)) {
                        return newDir === 'asc' ? aNum - bNum : bNum - aNum;
                    }
                    // String sort
                    const cmp = aText.localeCompare(bText, undefined, {
                        numeric: true,
                        sensitivity: 'base',
                    });
                    return newDir === 'asc' ? cmp : -cmp;
                });

                rows.forEach((row) => tbody.appendChild(row));
            });
        });
    });
}

/**
 * Deduplicate taskplan blocks at the markdown source level.
 * Keeps only the last complete ```taskplan block, removing all earlier ones.
 * Also parses inline step status markers (`[step N status]`) and applies
 * them to the taskplan block so the plan updates in-place.
 *
 * Exported (with underscore prefix) for testing only. The agent re-emits
 * the full taskplan on every update, so this is the spot where stale
 * versions get pruned — easy to break, worth pinning behavior.
 */
export function _keepLastTaskPlan(markdown) {
    // Strip any leading "ack" from steering response that may have leaked into the stream
    if (markdown.startsWith('ack')) {
        markdown = markdown.slice(3);
    }
    // Find all complete taskplan blocks (handle cases where block isn't at line start,
    // e.g. "ack```taskplan" when steering response leaks into the stream)
    const blockPattern = /```taskplan\r?\n[\s\S]*?\n```/g;
    const blocks = [...markdown.matchAll(blockPattern)];

    // Remove all but the last taskplan block
    if (blocks.length > 1) {
        for (let i = blocks.length - 2; i >= 0; i--) {
            markdown =
                markdown.slice(0, blocks[i].index) +
                markdown.slice(blocks[i].index + blocks[i][0].length);
        }
    }

    // Now apply inline step markers to the remaining taskplan block
    const remaining = [...markdown.matchAll(/```taskplan\r?\n[\s\S]*?\n```/g)];
    if (remaining.length !== 1) return markdown;

    const block = remaining[0];

    // Parse ALL step update markers from the raw markdown.
    // Use a non-greedy match for detail to handle cases where active+done are on the same line:
    //   `[step 1 active]` Launching...`[step 1 done]` Word launched
    // We need to find each `[step N status]` individually.
    const stepPattern = /`\[step (\d+) (\w+)\]`/g;
    const updates = new Map();
    let m;
    while ((m = stepPattern.exec(markdown)) !== null) {
        const stepNum = parseInt(m[1], 10);
        const status = m[2];
        // Extract detail: text between this marker's closing backtick and the next marker or end of line
        const afterMarker = markdown.slice(m.index + m[0].length);
        const detailMatch = afterMarker.match(/^\s*([^`\n\r]*)/);
        const detail = detailMatch ? detailMatch[1].trim() : '';
        // Later updates for the same step override earlier ones
        updates.set(stepNum, { status, detail });
    }

    if (updates.size === 0) return markdown;

    // Parse the taskplan block lines and apply updates
    const blockText = block[0];
    const lines = blockText.split(/\r?\n/);
    const header = lines[0]; // ```taskplan
    const footer = lines[lines.length - 1]; // ```
    const taskLines = lines.slice(1, -1);

    const updatedLines = taskLines.map((line, i) => {
        const stepNum = i + 1;
        const update = updates.get(stepNum);
        if (!update) return line;

        const lineMatch = line.match(/^\[(\w+)\]\s*(.+?)(?:\s*\|\s*(.*))?$/);
        if (!lineMatch) return line;

        const description = lineMatch[2].trim();
        const detail = update.detail || lineMatch[3] || '';
        return `[${update.status}] ${description}${detail ? ' | ' + detail : ''}`;
    });

    const newBlock = header + '\n' + updatedLines.join('\n') + '\n' + footer;

    // Replace the block in the markdown
    let result =
        markdown.slice(0, block.index) + newBlock + markdown.slice(block.index + block[0].length);

    // Strip the inline step markers from the output (handle same-line cases too)
    result = result.replace(/`\[step \d+ \w+\]`\s*[^`\n\r]*/g, '');
    result = result.replace(/\n{3,}/g, '\n\n');

    return result;
}

/**
 * Deduplicate taskplan blocks — keep only the last one which has the latest state.
 * Earlier taskplan blocks are removed from the DOM. This handles the case where
 * the agent outputs updated taskplan blocks throughout the response.
 */
function _deduplicateTaskPlans(container) {
    const plans = container.querySelectorAll('.taskplan');
    if (plans.length <= 1) return;
    for (let i = 0; i < plans.length - 1; i++) {
        plans[i].remove();
    }
}

/**
 * Parse a taskplan text block into structured task objects.
 * Format: [status] description | optional detail
 * @param {string} text - Raw taskplan text content
 * @returns {Array<{status: string, description: string, detail: string}>}
 */
export function parseTaskPlan(text) {
    return text
        .trim()
        .split('\n')
        .filter((l) => l.trim())
        .map((line) => {
            const match = line.match(/^\[(\w+)\]\s*(.+?)(?:\s*\|\s*(.*))?$/);
            if (!match) return null;
            return {
                status: match[1],
                description: match[2].trim(),
                detail: match[3]?.trim() || '',
            };
        })
        .filter(Boolean);
}

/**
 * Render a taskplan code block as a visual progress tracker.
 * Can be called directly with a container element, or used internally
 * by the markdown renderer when it encounters a ```taskplan block.
 *
 * @param {HTMLElement} codeBlock - The code element containing taskplan text
 * @param {HTMLElement} pre - The parent pre element to replace
 */
function renderTaskPlan(codeBlock, pre) {
    const tasks = parseTaskPlan(codeBlock.textContent);
    if (tasks.length === 0) return;

    const wrapper = createTaskPlanElement(tasks);
    pre.parentNode.insertBefore(wrapper, pre);
    pre.remove();
}

/**
 * Create a taskplan DOM element from parsed tasks.
 * Usable standalone outside of the markdown renderer.
 * @param {Array<{status: string, description: string, detail: string}>} tasks
 * @returns {HTMLElement}
 */
export function createTaskPlanElement(tasks) {
    const wrapper = document.createElement('div');
    wrapper.className = 'taskplan';
    wrapper.setAttribute('role', 'list');
    wrapper.setAttribute('aria-label', 'Task plan');

    const doneCount = tasks.filter((t) => t.status === 'done').length;
    const totalCount = tasks.length;
    wrapper.dataset.progress = `${doneCount}/${totalCount}`;

    tasks.forEach((task, i) => {
        const item = document.createElement('div');
        item.className = `taskplan-item taskplan-${task.status}`;
        item.setAttribute('role', 'listitem');

        // Done items with detail are collapsible (collapsed by default)
        const isCollapsible = task.status === 'done' && task.detail;
        if (isCollapsible) {
            item.classList.add('taskplan-collapsible', 'taskplan-collapsed');
        }

        const isLast = i === tasks.length - 1;

        item.innerHTML = `
            <div class="taskplan-indicator">
                <div class="taskplan-icon">${_taskIcon(task.status)}</div>
                ${!isLast ? '<div class="taskplan-connector"></div>' : ''}
            </div>
            <div class="taskplan-content">
                <div class="taskplan-title">${isCollapsible ? '<span class="taskplan-chevron">›</span> ' : ''}${_escapeTaskText(task.description)}</div>
                ${task.cancelled ? '<div class="taskplan-cancelled">Cancelled by user</div>' : ''}
                ${task.detail ? `<div class="taskplan-detail">${_escapeTaskText(task.detail)}</div>` : ''}
            </div>
        `;

        // Click to expand/collapse done items
        if (isCollapsible) {
            item.addEventListener('click', () => {
                item.classList.toggle('taskplan-collapsed');
            });
        }

        wrapper.appendChild(item);
    });

    return wrapper;
}

function _taskIcon(status) {
    switch (status) {
        case 'done':
            return '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="20 6 9 17 4 12"></polyline></svg>';
        case 'active':
            return '<div class="taskplan-spinner"></div>';
        case 'error':
            return '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>';
        case 'stopped':
            return '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><rect x="6" y="6" width="12" height="12" rx="1"></rect></svg>';
        default:
            return '<div class="taskplan-dot"></div>';
    }
}

// escapeHtml used by renderTaskPlan
function _escapeTaskText(str) {
    return str
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
}
