/**
 * Macros Settings Module — define named sequences of transformation steps.
 */
const TRANSFORMS = [
    { value: 'uppercase', label: 'UPPERCASE' },
    { value: 'lowercase', label: 'lowercase' },
    { value: 'trim', label: 'Trim whitespace' },
    { value: 'sort_lines', label: 'Sort lines' },
    { value: 'reverse_lines', label: 'Reverse lines' },
    { value: 'remove_blank_lines', label: 'Remove blank lines' },
    { value: 'unique_lines', label: 'Unique lines' },
    { value: 'number_lines', label: 'Number lines' },
    { value: 'count_words', label: 'Count words' },
    { value: 'count_lines', label: 'Count lines' },
    { value: 'count_chars', label: 'Count characters' },
    { value: 'base64_encode', label: 'Base64 encode' },
    { value: 'base64_decode', label: 'Base64 decode' },
];

class MacrosSettingsModule extends SettingsModule {
    constructor() {
        super('macros', 'Macros', '🔄');
        this._macros = [];
    }
    render() {
        const css = `
            .macro-card { background: var(--kiro-bg-input); border: 1px solid var(--kiro-border-subtle); border-radius: 8px; padding: 12px; margin-bottom: 10px; }
            .macro-header { display: flex; align-items: center; gap: 8px; margin-bottom: 8px; }
            .macro-header input, .macro-header select { background: var(--kiro-bg-surface); border: 1px solid var(--kiro-border-subtle); border-radius: 4px; padding: 4px 8px; color: var(--kiro-text); font-size: 13px; font-family: var(--kiro-font); }
            .macro-icon-input { width: 36px !important; text-align: center; flex: none !important; }
            .macro-header .macro-name-input { flex: 1; }
            .macro-output-select { padding: 4px 6px !important; font-size: 12px !important; }
            .macro-steps { margin: 8px 0; }
            .macro-step { margin-bottom: 6px; padding: 6px; background: var(--kiro-bg-surface); border-radius: 6px; border: 1px solid var(--kiro-border-subtle); }
            .macro-step-top { display: flex; align-items: center; gap: 6px; }
            .macro-step-num { font-size: 11px; color: var(--kiro-text-muted); width: 18px; text-align: center; flex-shrink: 0; }
            .macro-step-type { background: var(--kiro-bg-input); border: 1px solid var(--kiro-border-subtle); border-radius: 4px; padding: 3px 6px; color: var(--kiro-text); font-size: 11px; }
            .macro-step-fields { padding-left: 24px; margin-top: 4px; }
            .macro-step-fields input, .macro-step-fields select { width: 100%; background: var(--kiro-bg-input); border: 1px solid var(--kiro-border-subtle); border-radius: 4px; padding: 4px 8px; color: var(--kiro-text); font-size: 12px; font-family: var(--kiro-font); margin-bottom: 4px; }
            .macro-step-fields input::placeholder { color: var(--kiro-text-muted); }
            .macro-step-fields .field-row { display: flex; gap: 6px; }
            .macro-step-fields .field-row input { flex: 1; }
            .macro-step-btn { background: none; border: none; color: var(--kiro-text-muted); cursor: pointer; font-size: 13px; padding: 2px 4px; border-radius: 4px; flex-shrink: 0; }
            .macro-step-btn:hover { color: var(--kiro-text); background: var(--kiro-bg-input); }
            .macro-actions { display: flex; gap: 8px; justify-content: space-between; align-items: center; }
            .macro-delete-btn { background: none; border: none; color: var(--kiro-text-muted); cursor: pointer; font-size: 12px; padding: 2px 8px; }
            .macro-delete-btn:hover { color: #e55; }`;
        return '<div class="settings-section" id="' + this.id + '-section">'
            + '<h2 class="settings-section-header">' + this.icon + ' ' + this.title + '</h2>'
            + '<div class="setting-description" style="margin-bottom:12px">Chain multiple transformations into a single action. Mix AI prompts with instant operations like find/replace and text transforms. Macros appear in the Inline Assist menu.</div>'
            + '<div id="macrosList"></div>'
            + '<button class="setting-button" id="addMacroBtn" style="margin-top:8px">+ Add Macro</button>'
            + '<style>' + css + '</style>'
            + '</div>';
    }
    initialize() {
        document.getElementById('addMacroBtn')?.addEventListener('click', () => {
            this._macros.push({ name: 'New Macro', icon: '🔄', steps: [{ step_type: 'ai_prompt', prompt: '', find: '', replace: '', transform: '', script: '' }], output: 'clipboard' });
            this._renderMacros();
            this._markDirty();
        });
    }
    load(config) {
        this._macros = JSON.parse(JSON.stringify(config.macros || []));
        this._renderMacros();
    }
    save(config) {
        this._syncFromDom();
        config.macros = JSON.parse(JSON.stringify(this._macros));
    }
    validate() {
        this._syncFromDom();
        for (const m of this._macros) {
            if (!m.name.trim()) return { valid: false, message: 'Macro name cannot be empty.' };
            if (m.steps.length === 0) return { valid: false, message: 'Macro "' + m.name + '" needs at least one step.' };
        }
        return { valid: true };
    }
    _syncFromDom() {
        document.querySelectorAll('.macro-card').forEach((card, mi) => {
            if (!this._macros[mi]) return;
            this._macros[mi].name = card.querySelector('.macro-name-input')?.value || '';
            this._macros[mi].icon = card.querySelector('.macro-icon-input')?.value || '🔄';
            this._macros[mi].output = card.querySelector('.macro-output-select')?.value || 'clipboard';
            this._macros[mi].steps = Array.from(card.querySelectorAll('.macro-step')).map(el => ({
                step_type: el.querySelector('.macro-step-type')?.value || 'ai_prompt',
                prompt: el.querySelector('.step-prompt')?.value || '',
                find: el.querySelector('.step-find')?.value || '',
                replace: el.querySelector('.step-replace')?.value || '',
                transform: el.querySelector('.step-transform')?.value || '',
                script: el.querySelector('.step-script')?.value || '',
            }));
        });
    }
    _markDirty() {
        document.getElementById('macrosList')?.dispatchEvent(new Event('input', { bubbles: true }));
    }
    _renderMacros() {
        const list = document.getElementById('macrosList');
        if (!list) return;
        list.innerHTML = '';
        if (this._macros.length === 0) {
            list.innerHTML = '<div class="setting-description" style="color:var(--kiro-text-muted);font-style:italic">No macros defined yet.</div>';
            return;
        }
        this._macros.forEach((macro, mi) => {
            const card = document.createElement('div');
            card.className = 'macro-card';
            const outOpts = ['clipboard','replace','inform'].map(v => '<option value="' + v + '"' + (macro.output===v?' selected':'') + '>' + ({clipboard:'📋 Copy',replace:'✏️ Replace',inform:'💬 Show'}[v]) + '</option>').join('');
            let h = '<div class="macro-header"><input class="macro-icon-input" value="' + this._esc(macro.icon) + '" maxlength="2"><input class="macro-name-input" value="' + this._esc(macro.name) + '" placeholder="Macro name"><select class="macro-output-select">' + outOpts + '</select></div><div class="macro-steps">';
            macro.steps.forEach((step, si) => { h += this._stepHtml(step, si, macro.steps.length); });
            h += '</div><div class="macro-actions"><button class="setting-button macro-add-step-btn" style="font-size:12px;padding:3px 10px">+ Step</button><button class="macro-delete-btn">Delete macro</button></div>';
            card.innerHTML = h;
            list.appendChild(card);
            this._wireEvents(card, mi);
        });
    }
    _stepHtml(step, si, total) {
        const t = step.step_type || 'ai_prompt';
        const tOpts = [['ai_prompt','🤖 AI Prompt'],['find_replace','🔍 Find/Replace'],['transform','⚙️ Transform'],['script','📜 Script']].map(([v,l]) => '<option value="'+v+'"'+(t===v?' selected':'')+'>'+l+'</option>').join('');
        let fields = '';
        if (t === 'ai_prompt') fields = '<input class="step-prompt" value="' + this._esc(step.prompt) + '" placeholder="Prompt... use {input} for previous output">';
        else if (t === 'find_replace') fields = '<div class="field-row"><input class="step-find" value="' + this._esc(step.find) + '" placeholder="Find (regex)"><input class="step-replace" value="' + this._esc(step.replace) + '" placeholder="Replace with"></div>';
        else if (t === 'transform') { const xOpts = TRANSFORMS.map(x => '<option value="'+x.value+'"'+(step.transform===x.value?' selected':'')+'>'+x.label+'</option>').join(''); fields = '<select class="step-transform">'+xOpts+'</select>'; }
        else if (t === 'script') fields = '<input class="step-script" value="' + this._esc(step.script) + '" placeholder="JS: input.toUpperCase()">';
        return '<div class="macro-step" data-step="'+si+'"><div class="macro-step-top"><span class="macro-step-num">'+(si+1)+'.</span><select class="macro-step-type">'+tOpts+'</select><span style="flex:1"></span><button class="macro-step-btn macro-step-up"'+(si===0?' disabled':'')+'>↑</button><button class="macro-step-btn macro-step-down"'+(si===total-1?' disabled':'')+'>↓</button><button class="macro-step-btn macro-step-remove">✕</button></div><div class="macro-step-fields">'+fields+'</div></div>';
    }
    _wireEvents(card, mi) {
        card.querySelector('.macro-add-step-btn')?.addEventListener('click', () => { this._syncFromDom(); this._macros[mi].steps.push({ step_type:'ai_prompt',prompt:'',find:'',replace:'',transform:'',script:'' }); this._renderMacros(); this._markDirty(); });
        card.querySelector('.macro-delete-btn')?.addEventListener('click', () => { this._syncFromDom(); this._macros.splice(mi,1); this._renderMacros(); this._markDirty(); });
        card.querySelectorAll('.macro-step-type').forEach(sel => { sel.addEventListener('change', () => { this._syncFromDom(); this._renderMacros(); this._markDirty(); }); });
        card.querySelectorAll('.macro-step-up').forEach(btn => { btn.addEventListener('click', e => { const si=parseInt(e.target.closest('.macro-step').dataset.step); if(si>0){this._syncFromDom();[this._macros[mi].steps[si-1],this._macros[mi].steps[si]]=[this._macros[mi].steps[si],this._macros[mi].steps[si-1]];this._renderMacros();this._markDirty();} }); });
        card.querySelectorAll('.macro-step-down').forEach(btn => { btn.addEventListener('click', e => { const si=parseInt(e.target.closest('.macro-step').dataset.step); if(si<this._macros[mi].steps.length-1){this._syncFromDom();[this._macros[mi].steps[si],this._macros[mi].steps[si+1]]=[this._macros[mi].steps[si+1],this._macros[mi].steps[si]];this._renderMacros();this._markDirty();} }); });
        card.querySelectorAll('.macro-step-remove').forEach(btn => { btn.addEventListener('click', e => { const si=parseInt(e.target.closest('.macro-step').dataset.step); this._syncFromDom(); this._macros[mi].steps.splice(si,1); if(!this._macros[mi].steps.length) this._macros[mi].steps.push({step_type:'ai_prompt',prompt:'',find:'',replace:'',transform:'',script:''}); this._renderMacros(); this._markDirty(); }); });
    }
    _esc(s) { return (s||'').replace(/"/g,'&quot;').replace(/</g,'&lt;'); }
}
