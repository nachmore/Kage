import { loadPrismLanguage } from '../prism-loader.js';
import { t } from '../i18n.js';

export function highlightOrLazy(codeBlock, language) {
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

export function createCopyButton(code) {
    const btn = document.createElement('button');
    btn.className = 'copy-button';
    btn.innerHTML =
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg><span>Copy</span>';
    btn.onclick = () => copyCode(code, btn);
    return btn;
}

export function wrapCodeBlock(codeBlock, pre, language) {
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

export function createTryButton(_codeBlock, _wrapper) {
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
    outputHeader.innerHTML = `<span>${t('shared.markdown.console_output')}</span>`;
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
