import { EVT } from './events.js';
import { WINDOW } from './window-labels.js';

/**
 * Shared Script Editor component with syntax highlighting and AI generation.
 * Used by Quick Commands (shortcuts) and Automations (macros).
 *
 * Usage:
 *   import { createScriptEditor } from '../shared/script-editor.js';
 *   const editor = createScriptEditor(container, {
 *       id: 'myEditor',
 *       value: 'return input.toUpperCase();',
 *       placeholder: '// Your code here',
 *       variableHint: 'input',  // name of the variable passed in
 *       contextHint: 'Return a string.',
 *       rows: 6,
 *   });
 *   editor.getValue();
 *   editor.setValue('...');
 */

/**
 * Create a script editor with syntax highlighting and optional AI generation.
 * @param {HTMLElement} container - DOM element to render into
 * @param {object} opts
 * @param {string} opts.id - Unique ID prefix for elements
 * @param {string} [opts.value] - Initial script content
 * @param {string} [opts.placeholder] - Textarea placeholder
 * @param {string} [opts.variableHint] - Name of the input variable (e.g. 'args', 'input')
 * @param {string} [opts.contextHint] - Extra context for AI generation
 * @param {number} [opts.rows] - Textarea rows (default 6)
 * @param {boolean} [opts.showAi] - Show AI generation UI (default true)
 * @returns {{ getValue, setValue, destroy }}
 */
export function createScriptEditor(container, opts = {}) {
    const id = opts.id || 'scriptEd_' + Math.random().toString(36).slice(2, 7);
    const rows = opts.rows || 6;
    const showAi = opts.showAi !== false;
    const varHint = opts.variableHint || 'input';
    const ctxHint = opts.contextHint || 'Return a string.';

    let previousScript = null;

    // Build HTML
    let html = '';
    if (showAi) {
        html += `<div class="script-ai-row" style="display:flex;gap:6px;align-items:center;margin-bottom:6px;">
            <input type="text" class="setting-input" id="${id}_aiPrompt" placeholder="Ask AI to write or update the script..." style="flex:1;font-size:12px;">
            <button class="setting-button" id="${id}_aiBtn" style="font-size:11px;white-space:nowrap;">✨ Generate</button>
            <button class="setting-button" id="${id}_aiUndo" style="font-size:11px;display:none;">Undo</button>
        </div>
        <div id="${id}_aiStatus" style="font-size:11px;color:var(--kage-text-secondary);margin-bottom:4px;"></div>`;
    }

    html += `<div class="script-editor-container">
        <pre class="script-highlight" aria-hidden="true"><code class="language-javascript" id="${id}_highlight"></code></pre>
        <textarea id="${id}_textarea" class="setting-input script-editor" rows="${rows}" spellcheck="false" wrap="off"
            placeholder="${opts.placeholder || '// ' + varHint + ' contains the previous output\\nreturn ' + varHint + '.toUpperCase();'}">${_esc(opts.value || '')}</textarea>
    </div>
    <div style="font-size:10px;color:var(--kage-text-secondary);margin-top:2px;">
        JavaScript function body. The variable <code style="background:var(--kage-bg-input);padding:1px 4px;border-radius:3px;">${varHint}</code> contains the input. ${ctxHint}
    </div>`;

    container.innerHTML = html;

    // Wire up syntax highlighting
    const textarea = document.getElementById(`${id}_textarea`);
    const highlightEl = document.getElementById(`${id}_highlight`);

    function updateHighlight() {
        if (!textarea || !highlightEl) return;
        highlightEl.textContent = textarea.value + '\n';
        if (window.Prism) Prism.highlightElement(highlightEl);
        if (!textarea._scrollSynced) {
            textarea._scrollSynced = true;
            const pre = highlightEl.parentElement;
            textarea.addEventListener('scroll', () => {
                pre.scrollTop = textarea.scrollTop;
                pre.scrollLeft = textarea.scrollLeft;
            });
        }
    }

    textarea?.addEventListener('input', updateHighlight);
    updateHighlight();

    // Wire up AI generation
    if (showAi) {
        const aiPrompt = document.getElementById(`${id}_aiPrompt`);
        const aiBtn = document.getElementById(`${id}_aiBtn`);
        const aiUndo = document.getElementById(`${id}_aiUndo`);
        const aiStatus = document.getElementById(`${id}_aiStatus`);

        aiPrompt?.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') {
                e.preventDefault();
                generate();
            }
        });
        aiBtn?.addEventListener('click', generate);
        aiUndo?.addEventListener('click', () => {
            if (previousScript != null && textarea) {
                textarea.value = previousScript;
                updateHighlight();
                aiUndo.style.display = 'none';
            }
        });

        async function generate() {
            const userPrompt = aiPrompt?.value.trim();
            if (!userPrompt) {
                if (aiStatus) aiStatus.textContent = 'Enter a description first.';
                return;
            }

            const currentScript = textarea?.value.trim() || '';
            const parts = [
                '<role>You are a JavaScript code generator for Kage automation scripts.</role>',
                '',
                '<instructions>',
                `Write a JavaScript function body. The variable "${varHint}" contains the input from the previous step.`,
                ctxHint,
                '',
                'Respond with only the raw code. No explanation, no markdown fences, no comments.',
                '</instructions>',
            ];
            if (currentScript)
                parts.push('', '<current_script>', currentScript, '</current_script>');
            parts.push('', '<task>' + userPrompt + '</task>');

            previousScript = textarea?.value || '';
            if (aiBtn) {
                aiBtn.disabled = true;
                aiBtn.textContent = 'Generating...';
            }
            if (aiStatus) aiStatus.textContent = 'Sending to agent...';
            if (aiUndo) aiUndo.style.display = 'none';

            try {
                const invoke = window.__TAURI__?.core?.invoke;
                const listen = window.__TAURI__?.event?.listen;
                if (!invoke || !listen) throw new Error('Tauri not available');

                let response = '';
                const unlisten = await listen(EVT.MESSAGE_CHUNK, (event) => {
                    const delta =
                        event.payload && typeof event.payload === 'object'
                            ? event.payload.text || ''
                            : String(event.payload || '');
                    response += delta;
                    if (aiStatus) aiStatus.textContent = 'Receiving...';
                });
                const completionPromise = new Promise((resolve) => {
                    listen(EVT.MESSAGE_COMPLETE, () => resolve()).then((fn) => {
                        // Store unlisten for cleanup
                        completionPromise._unlisten = fn;
                    });
                });
                const sessionId = await invoke('get_window_session', {
                    label: WINDOW.MAIN,
                }).catch(() => null);
                await invoke('send_message_streaming', {
                    sessionId,
                    message: parts.join('\n'),
                    attachments: null,
                });
                await completionPromise;
                unlisten();
                if (completionPromise._unlisten) completionPromise._unlisten();

                let code = response.trim();
                const fenceMatch = code.match(/```(?:javascript|js)?\s*\n([\s\S]*?)```/);
                if (fenceMatch) code = fenceMatch[1].trim();
                code = code
                    .replace(/^```\w*\n?/, '')
                    .replace(/\n?```$/, '')
                    .trim();

                if (textarea) {
                    textarea.value = code;
                    updateHighlight();
                }
                if (aiStatus) aiStatus.textContent = 'Script generated. Review and save.';
                if (aiUndo) aiUndo.style.display = '';
            } catch (e) {
                if (aiStatus) aiStatus.textContent = 'Error: ' + e;
            } finally {
                if (aiBtn) {
                    aiBtn.disabled = false;
                    aiBtn.textContent = '✨ Generate';
                }
            }
        }
    }

    return {
        getValue: () => textarea?.value || '',
        setValue: (v) => {
            if (textarea) {
                textarea.value = v;
                updateHighlight();
            }
        },
        destroy: () => {
            container.innerHTML = '';
        },
    };
}

function _esc(s) {
    return (s || '')
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
}
