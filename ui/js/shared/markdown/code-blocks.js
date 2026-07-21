import { highlightOrLazy, wrapCodeBlock } from './code-controls.js';
import { renderDiagram, tryBackgroundDiagramRender } from './diagrams.js';
import {
    isFullTexDocument,
    renderCsvTable,
    renderHtmlPreview,
    renderJsonTree,
    renderMarkdownPreview,
    renderMathPreview,
    renderSvgPreview,
} from './previews.js';
import { createTaskPlanElement, renderTaskPlan } from './task-plan.js';

const DIAGRAM_LANGUAGES = new Set(['mermaid', 'dot', 'graphviz', 'neato']);
const HTML_LANGUAGES = new Set(['html', 'htm']);
const JSON_LANGUAGES = new Set(['json', 'jsonc']);
const MARKDOWN_LANGUAGES = new Set(['markdown', 'md']);
const CSV_LANGUAGES = new Set(['csv', 'tsv']);
const SVG_LANGUAGES = new Set(['svg']);
const MATH_LANGUAGES = new Set(['latex', 'tex', 'math']);

export function processCodeBlocks(container, streaming, savedDiagrams) {
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
                    if (savedKey !== code) {
                        tryBackgroundDiagramRender(saved, code, language);
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
        if (MARKDOWN_LANGUAGES.has(language)) {
            renderMarkdownPreview(codeBlock, pre, streaming, processCodeBlocks);
            return;
        }
        if (CSV_LANGUAGES.has(language)) {
            renderCsvTable(codeBlock, pre, language);
            return;
        }
        if (SVG_LANGUAGES.has(language)) {
            renderSvgPreview(codeBlock, pre);
            return;
        }
        if (MATH_LANGUAGES.has(language)) {
            // KaTeX renders math expressions, not full TeX documents.
            // If the source contains document-level commands, fall
            // through to the syntax-highlighted source path — KaTeX
            // would render every unknown macro in red, drowning the
            // expression in errors.
            if (isFullTexDocument(codeBlock.textContent)) {
                highlightOrLazy(codeBlock, language);
                wrapCodeBlock(codeBlock, pre, language);
                return;
            }
            renderMathPreview(codeBlock, pre);
            return;
        }
        highlightOrLazy(codeBlock, language);
        wrapCodeBlock(codeBlock, pre, language);
    });
}

// --- Generic diagram rendering ---

/**
 * Attempt a background re-render of a diagram that already has a successful render.
 * Renders into a detached node; on success, swaps the diagram-content with a fade.
 * On failure, silently ignores — the existing render stays.
 */
