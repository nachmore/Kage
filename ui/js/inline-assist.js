/**
 * Inline Assist — context-aware AI popup that appears at the cursor.
 * Shows smart actions based on selected text, plus a free-form prompt input.
 * Results are pasted back into the source application.
 */
import { classifyText, getActionsForText } from './shared/quick-actions.js';
import { createMascot } from './shared/mascot.js';
import { applyTheme, initThemeListener, loadAndApplyTheme } from './shared/theme.js';
import { EVT } from './shared/events.js';
import { WINDOW } from './shared/window-labels.js';
import { getWindowSessionOrNull } from './shared/session-resolve.js';
import { initI18n, applyStaticTranslations, t } from './shared/i18n.js';

console.log(
    '[inline-assist] Module loaded, classifyText:',
    typeof classifyText,
    'getActionsForText:',
    typeof getActionsForText
);

(async function () {
    const { invoke } = window.__TAURI__.core;
    const { listen, emit } = window.__TAURI__.event;
    const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
    const LogicalSize = window.__TAURI__.dpi.LogicalSize;

    // Load i18n catalog before any rendering so the static markup +
    // any future t() calls see real translations rather than literal keys.
    try {
        await initI18n(invoke);
    } catch (e) {
        console.warn('[inline-assist] i18n init failed', e);
    }
    applyStaticTranslations(document);

    // Apply theme via the shared ES module (loaded directly here, no global shim).
    initThemeListener();
    loadAndApplyTheme(invoke).catch(() => applyTheme('system'));

    // Render mascot icon
    const mascotEl = document.getElementById('inlineAssistMascot');
    if (mascotEl) createMascot({ size: 24 }).then((svg) => mascotEl.appendChild(svg));

    const actionsEl = document.getElementById('actions');
    const customInput = document.getElementById('customInput');
    const statusBar = document.getElementById('statusBar');
    const statusText = document.getElementById('statusText');
    const panel = document.getElementById('panel');
    const iconBubble = document.getElementById('iconBubble');

    let selectedText = '';
    let sourceApp = '';
    let sourceTitle = '';
    let selectedIndex = -1;
    let actionItems = [];
    let isProcessing = false;

    // --- Icon bubble — click to open full chat ---
    iconBubble.addEventListener('click', async () => {
        try {
            // Open chat with the selected text as context. Use main's
            // session — this opens in the chat window.
            const sessionId = await getWindowSessionOrNull(invoke, WINDOW.MAIN);
            const message = selectedText.trim()
                ? `The following text is currently selected:\n\`\`\`\n${selectedText.trim()}\n\`\`\``
                : '';
            await invoke('open_chat_with_message', { sessionId, message });
        } catch (e) {
            console.error('Failed to open chat:', e);
        }
        await appWindow.hide();
    });

    // --- Listen for show event from backend ---
    await listen(EVT.INLINE_ASSIST_SHOW, async (event) => {
        console.log('[inline-assist] Received show event:', event.payload);
        const { selection, app, title } = event.payload || {};
        selectedText = selection || '';
        console.log(
            '[inline-assist] Selection text (' + selectedText.length + ' chars):',
            JSON.stringify(selectedText.substring(0, 200))
        );
        console.log('[inline-assist] Classification:', classifyText(selectedText));
        sourceApp = app || '';
        sourceTitle = title || '';
        selectedIndex = -1;
        isProcessing = false;

        await buildActions();
        console.log('[inline-assist] Built actions:', actionItems.length);
        statusBar.classList.remove('visible');
        panel.style.display = '';
        iconBubble.classList.remove('thinking');
        iconBubble.style.margin = '';
        customInput.value = '';

        // Resize window to fit content
        await resizeToFit();
        customInput.focus();
    });

    // --- Listen for streaming response ---
    let accumulatedResponse = '';

    await listen('inline_assist_chunk', (event) => {
        accumulatedResponse = event.payload || '';
        statusText.textContent = `Generating... (${accumulatedResponse.length} chars)`;
    });

    await listen('inline_assist_complete', async () => {
        if (!isProcessing) return;
        isProcessing = false;
        iconBubble.classList.remove('thinking');

        if (accumulatedResponse.trim()) {
            try {
                await invoke('inline_assist_apply', { text: accumulatedResponse.trim() });
            } catch (e) {
                console.error('Failed to apply inline assist:', e);
            }
        }

        await appWindow.hide();
    });

    await listen(EVT.INLINE_ASSIST_ERROR, async (event) => {
        isProcessing = false;
        iconBubble.classList.remove('thinking');
        // Show error briefly then hide
        panel.style.display = '';
        statusBar.classList.add('visible');
        statusText.textContent = '❌ ' + (event.payload || 'Error');
        await resizeToFit();
        setTimeout(() => appWindow.hide(), 1500);
    });

    // --- Build action items based on selection ---
    let loadedMacros = [];

    async function buildActions() {
        actionsEl.innerHTML = '';
        actionItems = [];

        // Load macros from config
        try {
            const config = await invoke('get_config');
            loadedMacros = config.macros || [];
        } catch {
            loadedMacros = [];
        }

        if (!selectedText.trim()) {
            actionItems = [
                {
                    label: t('inline_assist.suggestion.summarize'),
                    icon: '📝',
                    prompt: "Summarize what I'm currently looking at.",
                    mode: 'inform',
                },
                {
                    label: t('inline_assist.suggestion.help_app'),
                    icon: '💡',
                    prompt: "Give me tips for what I'm currently doing.",
                    mode: 'inform',
                },
            ];
        } else {
            const qaConfig = { enabled: true, custom_actions: [] };
            actionItems = await getActionsForText(selectedText, qaConfig);
        }

        // Add macros as actions (with a separator if there are both)
        if (loadedMacros.length > 0 && actionItems.length > 0) {
            actionItems.push({ _separator: true });
        }
        for (const macro of loadedMacros) {
            actionItems.push({
                label: macro.name,
                icon: macro.icon || '🔄',
                mode: macro.output || 'clipboard',
                _macro: macro,
            });
        }

        for (let i = 0; i < actionItems.length; i++) {
            const action = actionItems[i];
            if (action._separator) {
                const sep = document.createElement('div');
                sep.className = 'sep';
                actionsEl.appendChild(sep);
                continue;
            }
            const el = document.createElement('div');
            el.className = 'action-item';
            el.innerHTML = `<span class="action-icon">${action.icon || '⚡'}</span><span class="action-label">${action.label}</span>`;
            el.addEventListener('click', () => {
                if (action._macro) {
                    executeMacro(action._macro);
                } else {
                    executeAction(action);
                }
            });
            el.addEventListener('mouseenter', () => {
                selectedIndex = i;
                updateSelection();
            });
            actionsEl.appendChild(el);
        }
    }

    function updateSelection() {
        const items = actionsEl.querySelectorAll('.action-item');
        items.forEach((el, i) => el.classList.toggle('selected', i === selectedIndex));
    }

    // --- Execute an action ---
    let currentMode = 'replace'; // 'replace' = paste back, 'inform' = show in floating UX

    async function executeAction(action) {
        if (isProcessing) return;
        isProcessing = true;
        accumulatedResponse = '';
        currentMode = action.mode || 'replace';

        // Build the prompt
        let prompt = action.prompt || action.label;
        if (selectedText.trim() && prompt.includes('{text}')) {
            prompt = prompt.replace('{text}', selectedText.trim());
        } else if (selectedText.trim() && !prompt.includes('{text}')) {
            prompt = `${prompt}\n\nSelected text:\n\`\`\`\n${selectedText.trim()}\n\`\`\``;
        }

        // Add screen context
        if (sourceApp) {
            prompt = `<_kage_ctx app="${sourceApp}" title="${sourceTitle}"/>\n${prompt}`;
        }

        if (currentMode === 'inform') {
            // Send to the floating UX — hide ourselves first
            await appWindow.hide();
            try {
                const sessionId = await getWindowSessionOrNull(invoke, WINDOW.MAIN);
                await invoke('open_chat_with_message', { sessionId, message: prompt });
            } catch (e) {
                console.error('Failed to open chat:', e);
            }
            isProcessing = false;
            return;
        }

        // Replace mode — collapse to ghost bubble, send inline, paste back
        panel.style.display = 'none';
        iconBubble.classList.add('thinking');
        iconBubble.style.margin = '20px auto';
        await new Promise((r) => requestAnimationFrame(r));
        await appWindow.setSize(new LogicalSize(86, 86));

        // Add instruction for inline replacement
        prompt +=
            '\n\n[_KAGE_INLINE] Return ONLY the result text. No explanations, no markdown formatting, no code fences. Just the raw output text that should replace the selection.';

        try {
            // Inline-assist runs on the floating session — that's the
            // hotkey-driven path that triggered this overlay.
            const sessionId = await getWindowSessionOrNull(invoke, WINDOW.FLOATING);
            await invoke('send_inline_assist', { sessionId, message: prompt });
        } catch (e) {
            console.error('Inline assist failed:', e);
            isProcessing = false;
            await appWindow.hide();
        }
    }

    async function executeCustomPrompt() {
        const text = customInput.value.trim();
        if (!text || isProcessing) return;
        // Custom prompts default to 'replace' mode
        await executeAction({ label: text, prompt: text, icon: '✨', mode: 'replace' });
    }

    // --- Execute a macro (chained steps) ---
    async function executeMacro(macro) {
        if (isProcessing) return;
        isProcessing = true;

        const outputMode = macro.output || 'clipboard';

        // For 'inform' mode, send the whole macro as a single prompt to the chat
        if (outputMode === 'inform') {
            const stepsDesc = macro.steps.map((s, i) => `${i + 1}. ${s.prompt}`).join('\n');
            const prompt = `Run these steps on the following text:\n${stepsDesc}\n\nText:\n\`\`\`\n${selectedText.trim()}\n\`\`\``;
            await appWindow.hide();
            try {
                const sessionId = await getWindowSessionOrNull(invoke, WINDOW.MAIN);
                await invoke('open_chat_with_message', { sessionId, message: prompt });
            } catch (e) {
                console.error('Failed to open chat:', e);
            }
            isProcessing = false;
            return;
        }

        // Replace/clipboard mode — collapse to ghost, run steps sequentially
        panel.style.display = 'none';
        iconBubble.classList.add('thinking');
        iconBubble.style.margin = '20px auto';
        await new Promise((r) => requestAnimationFrame(r));
        await appWindow.setSize(new LogicalSize(86, 86));

        try {
            const steps = macro.steps.map((s) => ({
                step_type: s.step_type || 'ai_prompt',
                prompt: s.prompt || '',
                find: s.find || '',
                replace: s.replace || '',
                transform: s.transform || '',
                script: s.script || '',
            }));
            const sessionId = await getWindowSessionOrNull(invoke, WINDOW.FLOATING);
            const result = await invoke('execute_macro', {
                sessionId,
                steps,
                initialInput: selectedText.trim(),
            });

            if (result?.trim()) {
                if (outputMode === 'replace') {
                    await invoke('inline_assist_apply', { text: result.trim() });
                } else {
                    // clipboard mode — just copy, don't paste
                    // Use a simple write_clipboard approach via the apply without paste
                    await invoke('inline_assist_apply', { text: result.trim() });
                }
            }
        } catch (e) {
            console.error('Macro execution failed:', e);
        }

        isProcessing = false;
        iconBubble.classList.remove('thinking');
        await appWindow.hide();
    }

    // --- Keyboard navigation ---
    document.addEventListener('keydown', (e) => {
        if (isProcessing) {
            if (e.key === 'Escape') {
                // TODO: cancel generation
                appWindow.hide();
            }
            return;
        }

        if (e.key === 'Escape') {
            appWindow.hide();
            return;
        }

        if (e.key === 'ArrowDown') {
            e.preventDefault();
            selectedIndex = Math.min(selectedIndex + 1, actionItems.length - 1);
            updateSelection();
            return;
        }

        if (e.key === 'ArrowUp') {
            e.preventDefault();
            selectedIndex = Math.max(selectedIndex - 1, -1);
            updateSelection();
            if (selectedIndex === -1) customInput.focus();
            return;
        }

        if (e.key === 'Enter') {
            e.preventDefault();
            if (selectedIndex >= 0 && selectedIndex < actionItems.length) {
                const action = actionItems[selectedIndex];
                if (action._separator) return;
                if (action._macro) {
                    executeMacro(action._macro);
                } else {
                    executeAction(action);
                }
            } else if (customInput.value.trim()) {
                executeCustomPrompt();
            }
            return;
        }
    });

    // --- Auto-resize ---
    async function resizeToFit() {
        await new Promise((r) => requestAnimationFrame(r));
        await new Promise((r) => requestAnimationFrame(r)); // double-raf for layout settle
        const layoutRect = document.getElementById('layout').getBoundingClientRect();
        await appWindow.setSize(
            new LogicalSize(
                Math.max(Math.ceil(layoutRect.width) + 8, 260),
                Math.ceil(layoutRect.height) + 8
            )
        );
    }

    // --- Hide on blur ---
    await appWindow.listen('tauri://blur', () => {
        if (!isProcessing) {
            setTimeout(() => appWindow.hide(), 100);
        }
    });

    // --- Pause CSS animations when hidden to stop GPU compositing ---
    // WebView2 keeps processing infinite CSS animations (ghost-float, spin, etc.)
    // even on hidden windows, which causes the shared GPU process to burn 2%+ CPU.
    function pauseAnimations() {
        document.documentElement.classList.add('animations-paused');
    }
    function resumeAnimations() {
        document.documentElement.classList.remove('animations-paused');
    }

    // Inject a style rule that pauses ALL animations when the class is set
    const pauseStyle = document.createElement('style');
    pauseStyle.textContent =
        '.animations-paused, .animations-paused * { animation-play-state: paused !important; }';
    document.head.appendChild(pauseStyle);

    // Window starts hidden — pause immediately
    pauseAnimations();

    // Resume on show, pause on hide
    await appWindow.listen('tauri://focus', () => resumeAnimations());
    await appWindow.listen('tauri://blur', () => pauseAnimations());
    document.addEventListener('visibilitychange', () => {
        if (document.hidden) pauseAnimations();
        else resumeAnimations();
    });
})();
