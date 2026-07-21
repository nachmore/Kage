import { sanitizeExtensionHtml } from '../extension-html-sanitizer.js';
import { t } from '../i18n.js';
import { createCopyButton, highlightOrLazy, wrapCodeBlock } from './code-controls.js';
import { isFullTexDocument, renderJsonTree, renderMathPreview } from './previews-math-json.js';

export { isFullTexDocument, renderJsonTree, renderMathPreview };

export function renderHtmlPreview(codeBlock, pre) {
    const code = codeBlock.textContent;

    // Don't render incomplete HTML during streaming
    if (!code.trim()) return;

    const wrapper = document.createElement('div');
    wrapper.className = 'diagram-wrapper';

    const header = document.createElement('div');
    header.className = 'diagram-header';
    const label = document.createElement('span');
    label.className = 'diagram-label';
    label.textContent = t('shared.markdown.diagram.html');

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
    highlightOrLazy(sCode, 'markup');
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

/**
 * Render a `markdown` / `md` fenced code block as actual rendered
 * markdown by default, with a "Source" toggle to flip back to the
 * raw fenced view. Mirrors `renderHtmlPreview`'s chrome (same
 * wrapper class, same toggle button, same copy button) so the user
 * sees a consistent affordance across HTML and Markdown previews.
 *
 * Safety: `marked.parse` runs through `hardenMarkedOnce`, which
 * overrides `renderer.html` to escape every raw HTML token. So
 * the preview node only contains markdown-derived structural
 * elements — no script/style/iframe leaks even if the agent's
 * markdown source includes raw HTML.
 *
 * Streaming: skip render until the fence is non-empty. Once the
 * stream finishes, the final non-streaming render re-runs this with
 * the complete content.
 */
export function renderMarkdownPreview(codeBlock, pre, streaming, processCodeBlocks) {
    const code = codeBlock.textContent;
    if (!code.trim()) return;

    const wrapper = document.createElement('div');
    wrapper.className = 'diagram-wrapper';

    const header = document.createElement('div');
    header.className = 'diagram-header';
    const label = document.createElement('span');
    label.className = 'diagram-label';
    label.textContent = t('shared.markdown.diagram.markdown');

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

    // Render the markdown into a plain div. hardenMarkedOnce already
    // ran from module init; no raw HTML can leak through.
    const previewDiv = document.createElement('div');
    previewDiv.className = 'diagram-content markdown-preview-content';
    marked.setOptions({ breaks: true, gfm: true });
    previewDiv.innerHTML = marked.parse(code);

    // Recurse into the rendered markdown for any nested code blocks
    // (e.g. a markdown sample that itself contains a ```js fence).
    // Skip during streaming — the outer fence is still growing and
    // we'll re-do this on the final pass anyway.
    if (!streaming) {
        processCodeBlocks(previewDiv, false, new Map());
    }

    // Source view — raw markdown with prism highlight.
    const sourceDiv = document.createElement('div');
    sourceDiv.className = 'diagram-source';
    const sPre = document.createElement('pre');
    const sCode = document.createElement('code');
    sCode.textContent = code;
    highlightOrLazy(sCode, 'markdown');
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

// --- CSV / TSV table renderer ---

/**
 * Parse a CSV or TSV string into a 2D string array.
 *
 * Handles:
 *   - The standard double-quote escape (`"`) for fields containing
 *     the delimiter, a newline, or a literal quote (escaped as `""`).
 *   - CRLF and LF line endings.
 *   - A trailing newline at end-of-input (very common; would otherwise
 *     produce a spurious empty row).
 *
 * Pure logic, no DOM. Underscore prefix marks it as an
 * implementation detail callers shouldn't import.
 */
export function parseDelimited(text, delimiter) {
    const rows = [];
    let row = [];
    let field = '';
    let i = 0;
    let inQuotes = false;
    while (i < text.length) {
        const ch = text[i];
        if (inQuotes) {
            if (ch === '"') {
                // Escaped quote (`""`) → literal `"`. Anything else
                // closes the quoted field.
                if (text[i + 1] === '"') {
                    field += '"';
                    i += 2;
                    continue;
                }
                inQuotes = false;
                i++;
                continue;
            }
            field += ch;
            i++;
            continue;
        }
        if (ch === '"') {
            inQuotes = true;
            i++;
            continue;
        }
        if (ch === delimiter) {
            row.push(field);
            field = '';
            i++;
            continue;
        }
        if (ch === '\r') {
            // CRLF — fold to LF
            i++;
            continue;
        }
        if (ch === '\n') {
            row.push(field);
            rows.push(row);
            row = [];
            field = '';
            i++;
            continue;
        }
        field += ch;
        i++;
    }
    // Final field / row (no trailing newline, or last char was a
    // closing quote). Skip if it's an empty trailing row from a
    // file-ending newline.
    if (field.length > 0 || row.length > 0) {
        row.push(field);
        rows.push(row);
    }
    return rows;
}

/**
 * Render a CSV or TSV fenced block as an HTML table by default with
 * a "Source" toggle to flip back to the raw delimited text.
 *
 * Reuses `makeTablesSortable` so the rendered table picks up the
 * same click-to-sort affordance as Markdown-native tables.
 */
export function renderCsvTable(codeBlock, pre, language) {
    const code = codeBlock.textContent;
    if (!code.trim()) return;

    const delimiter = language === 'tsv' ? '\t' : ',';
    let rows;
    try {
        rows = parseDelimited(code, delimiter);
    } catch {
        // Defensive — _parseDelimited shouldn't throw, but if a
        // future edit ever introduces a code path that does, fall
        // back to a plain code block rather than crashing the
        // whole render pipeline.
        wrapCodeBlock(codeBlock, pre, language);
        return;
    }
    if (rows.length === 0) return;

    const wrapper = document.createElement('div');
    wrapper.className = 'diagram-wrapper';

    const header = document.createElement('div');
    header.className = 'diagram-header';
    const label = document.createElement('span');
    label.className = 'diagram-label';
    label.textContent = language.toUpperCase();

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
    previewDiv.className = 'diagram-content csv-table-content';

    const table = document.createElement('table');
    // The first row is treated as a header. CSV/TSV doesn't carry an
    // explicit "this is a header" marker, but in practice almost every
    // hand-pasted block uses one — and treating the first row as a
    // header gives the user the sortable affordance immediately.
    const [headerRow, ...bodyRows] = rows;
    if (headerRow) {
        const thead = document.createElement('thead');
        const tr = document.createElement('tr');
        for (const cell of headerRow) {
            const th = document.createElement('th');
            th.textContent = cell;
            tr.appendChild(th);
        }
        thead.appendChild(tr);
        table.appendChild(thead);
    }
    if (bodyRows.length > 0) {
        const tbody = document.createElement('tbody');
        for (const r of bodyRows) {
            const tr = document.createElement('tr');
            for (const cell of r) {
                const td = document.createElement('td');
                td.textContent = cell;
                tr.appendChild(td);
            }
            tbody.appendChild(tr);
        }
        table.appendChild(tbody);
    }
    previewDiv.appendChild(table);

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

    // Inherit the existing table-sort affordance from markdown tables.
    makeTablesSortable(previewDiv);

    toggleBtn.onclick = () => {
        const showing = sourceDiv.classList.toggle('visible');
        toggleBtn.querySelector('span').textContent = showing ? 'Preview' : 'Source';
        previewDiv.style.display = showing ? 'none' : '';
    };
}

// --- SVG inline renderer ---

/**
 * Render an `svg` fenced block as the actual rendered SVG by
 * default, with a "Source" toggle for the raw markup.
 *
 * Safety: routed through `sanitizeExtensionHtml(html, 'icon')` —
 * the same sanitizer mode used for extension toolbar icons.
 * `icon` mode allows only `<svg>` at the top level and the
 * `SVG_TAGS` set inside (path, circle, rect, polygon, …) with their
 * geometry/style attributes. `<script>`, foreign HTML, event
 * handlers, and `<use>` references to anywhere except the in-doc
 * `<symbol>` are all dropped at the sanitizer boundary, so a
 * malicious agent can't paint an active payload onto the surface.
 */
export function renderSvgPreview(codeBlock, pre) {
    const code = codeBlock.textContent;
    if (!code.trim()) return;

    const wrapper = document.createElement('div');
    wrapper.className = 'diagram-wrapper';

    const header = document.createElement('div');
    header.className = 'diagram-header';
    const label = document.createElement('span');
    label.className = 'diagram-label';
    label.textContent = t('shared.markdown.diagram.svg');

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
    previewDiv.className = 'diagram-content svg-preview-content';
    // Sanitize through the existing extension-icon sanitizer. If the
    // result is empty (e.g. agent emitted an XML preamble that the
    // sanitizer's parser can't reach into) fall back to a plain text
    // notice so the user knows the preview was rejected rather than
    // staring at a silent empty box.
    const frag = sanitizeExtensionHtml(code, 'icon');
    if (frag?.childNodes?.length > 0) {
        previewDiv.appendChild(frag);
    } else {
        previewDiv.textContent = '(SVG could not be rendered safely)';
    }

    const sourceDiv = document.createElement('div');
    sourceDiv.className = 'diagram-source';
    const sPre = document.createElement('pre');
    const sCode = document.createElement('code');
    sCode.textContent = code;
    // Markup highlighting — Prism aliases 'svg' through markup.
    highlightOrLazy(sCode, 'markup');
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

export function makeTablesSortable(container) {
    container.querySelectorAll('table').forEach((table) => {
        const thead = table.querySelector('thead');
        const tbody = table.querySelector('tbody');
        if (!thead || !tbody) return;

        const headers = thead.querySelectorAll('th');
        headers.forEach((th, colIndex) => {
            th.style.cursor = 'pointer';
            th.style.userSelect = 'none';
            th.title = t('shared.markdown.sort.title');
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
