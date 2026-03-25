/**
 * Automations Settings Module — define named sequences of transformation steps
 * with triggers (manual, schedule, or signal-based).
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

const SCHEDULE_MODES = [
    { value: 'hourly', label: '🕐 Hourly' },
    { value: 'daily', label: '📅 Daily' },
    { value: 'monthly', label: '🗓️ Monthly' },
    { value: 'yearly', label: '📆 Yearly' },
];

const DAYS_OF_WEEK = [
    { value: '1', label: 'Mon', short: 'M' },
    { value: '2', label: 'Tue', short: 'T' },
    { value: '3', label: 'Wed', short: 'W' },
    { value: '4', label: 'Thu', short: 'T' },
    { value: '5', label: 'Fri', short: 'F' },
    { value: '6', label: 'Sat', short: 'S' },
    { value: '7', label: 'Sun', short: 'S' },
];

class MacrosSettingsModule extends SettingsModule {
    constructor() {
        super('macros', 'Automations', '🔄');
        this._macros = [];
        this._signals = [];
    }
    render() {
        const css = `
            .macro-card { background: var(--kiro-bg-input); border: 1px solid var(--kiro-border-subtle); border-radius: 8px; padding: 12px; margin-bottom: 10px; }
            .macro-card.disabled { opacity: 0.5; }
            .macro-header { display: flex; align-items: center; gap: 8px; margin-bottom: 8px; }
            .macro-header input, .macro-header select { background: var(--kiro-bg-surface); border: 1px solid var(--kiro-border-subtle); border-radius: 4px; padding: 4px 8px; color: var(--kiro-text); font-size: 13px; font-family: var(--kiro-font); }
            .macro-icon-input { width: 36px !important; text-align: center; flex: none !important; }
            .macro-header .macro-name-input { flex: 1; }
            .macro-output-select { padding: 4px 6px !important; font-size: 12px !important; }
            .macro-enable-toggle { cursor: pointer; }
            .macro-trigger { margin: 8px 0; padding: 8px; background: var(--kiro-bg-surface); border-radius: 6px; border: 1px solid var(--kiro-border-subtle); }
            .macro-trigger-header { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--kiro-text-muted); margin-bottom: 6px; }
            .macro-trigger-header select { background: var(--kiro-bg-input); border: 1px solid var(--kiro-border-subtle); border-radius: 4px; padding: 3px 6px; color: var(--kiro-text); font-size: 12px; }
            .macro-trigger-config { padding-left: 4px; }
            .macro-trigger-config select, .macro-trigger-config input { background: var(--kiro-bg-input); border: 1px solid var(--kiro-border-subtle); border-radius: 4px; padding: 4px 8px; color: var(--kiro-text); font-size: 12px; width: 100%; margin-bottom: 4px; }
            .macro-trigger-config input::placeholder { color: var(--kiro-text-muted); }
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
            .macro-delete-btn:hover { color: #e55; }
            .power-status-badge { display: inline-flex; align-items: center; gap: 4px; font-size: 12px; padding: 2px 8px; border-radius: 10px; background: var(--kiro-bg-input); color: var(--kiro-text-muted); }`;
        return '<div class="settings-section" id="' + this.id + '-section">'
            + '<h2 class="settings-section-header">' + this.icon + ' ' + this.title + '</h2>'
            + '<div class="setting-description" style="margin-bottom:12px">Chain transformations into automated actions. Trigger them manually, on a schedule, or in response to signals from extensions.</div>'
            + '<div class="setting-section-label">Power</div>'
            + '<div id="automationPowerSection"></div>'
            + '<div class="setting-section-label">Automations</div>'
            + '<div id="macrosList"></div>'
            + '<button class="setting-button" id="addMacroBtn" style="margin-top:8px">+ Add Automation</button>'
            + '<style>' + css + '</style>'
            + '</div>';
    }
    async initialize() {
        document.getElementById('addMacroBtn')?.addEventListener('click', () => {
            this._macros.push({ name: 'New Automation', icon: '🔄', steps: [{ step_type: 'ai_prompt', prompt: '', find: '', replace: '', transform: '', script: '' }], output: 'clipboard', trigger: { type: 'manual' }, enabled: true });
            this._renderMacros();
            this._markDirty();
        });
        // Load available signals
        try {
            const invoke = window.__TAURI__?.core?.invoke;
            if (invoke) {
                this._signals = await invoke('list_automation_signals');
                // Add extension signals dynamically (if extension manager is available)
                if (window._extensionManager) {
                    const extTriggers = window._extensionManager.getTriggerDefinitions?.() || [];
                    for (const ext of extTriggers) {
                        for (const t of ext.triggers) {
                            this._signals.push({ name: t.name, description: t.description, source: ext.extensionName, icon: t.icon || ext.extensionIcon });
                        }
                    }
                }
            }
        } catch (e) { console.warn('[Automations] Failed to load signals:', e); }
    }
    load(config) {
        this._macros = JSON.parse(JSON.stringify(config.macros || []));
        // Ensure all macros have trigger and enabled fields
        for (const m of this._macros) {
            if (!m.trigger) m.trigger = { type: 'manual' };
            if (m.enabled === undefined) m.enabled = true;
        }
        this._powerConfig = config.automation_power || { mode: 'auto', battery_multiplier: 2.0, low_battery_multiplier: 4.0, disable_signals_on_low_battery: false };
        this._renderPowerSection();
        this._renderMacros();
    }
    save(config) {
        this._syncFromDom();
        config.macros = JSON.parse(JSON.stringify(this._macros));
        config.automation_power = this._powerConfig || { mode: 'auto', battery_multiplier: 2.0, low_battery_multiplier: 4.0, disable_signals_on_low_battery: false };
    }
    validate() {
        this._syncFromDom();
        for (const m of this._macros) {
            if (!m.name.trim()) return { valid: false, message: 'Automation name cannot be empty.' };
            if (m.steps.length === 0) return { valid: false, message: 'Automation "' + m.name + '" needs at least one step.' };
        }
        return { valid: true };
    }
    _renderPowerSection() {
        const container = document.getElementById('automationPowerSection');
        if (!container) return;
        const p = this._powerConfig;
        const modeOpts = [['auto','🔋 Auto (detect battery)'],['full','⚡ Always full speed'],['saving','🪫 Always power saving']].map(([v,l]) => '<option value="'+v+'"'+(p.mode===v?' selected':'')+'>'+l+'</option>').join('');
        container.innerHTML = `
            <div class="setting-row">
                <div class="setting-label">Power Mode</div>
                <div class="setting-description">Controls how automations behave on battery power.</div>
                <select class="setting-select" id="automationPowerMode" style="max-width:280px;">${modeOpts}</select>
            </div>
            <div id="powerDetailsRow" style="${p.mode === 'auto' ? '' : 'display:none;'}">
                <div class="setting-row">
                    <div class="setting-description">On battery, schedules run <strong>${p.battery_multiplier}×</strong> slower. On low battery (<20%), <strong>${p.low_battery_multiplier}×</strong> slower.</div>
                </div>
            </div>
        `;
        document.getElementById('automationPowerMode')?.addEventListener('change', (e) => {
            this._powerConfig.mode = e.target.value;
            const details = document.getElementById('powerDetailsRow');
            if (details) details.style.display = e.target.value === 'auto' ? '' : 'none';
            this._markDirty();
        });
    }
    _syncFromDom() {
        document.querySelectorAll('.macro-card').forEach((card, mi) => {
            if (!this._macros[mi]) return;
            this._macros[mi].name = card.querySelector('.macro-name-input')?.value || '';
            this._macros[mi].icon = card.querySelector('.macro-icon-input')?.value || '🔄';
            this._macros[mi].output = card.querySelector('.macro-output-select')?.value || 'clipboard';
            this._macros[mi].enabled = card.querySelector('.macro-enable-toggle')?.checked ?? true;
            // Sync trigger
            const triggerType = card.querySelector('.macro-trigger-type')?.value || 'manual';
            if (triggerType === 'manual') {
                this._macros[mi].trigger = { type: 'manual' };
            } else if (triggerType === 'schedule') {
                // Build interval from the schedule UI controls
                const mode = card.querySelector('.macro-schedule-mode')?.value || 'daily';
                const parsed = { mode, hours: 1, minute: 0, time: '09:00', days: [], dayOfMonth: 1, weekOrdinal: '1st', weekDay: '1', month: 1, monthDay: 1 };
                if (mode === 'hourly') {
                    parsed.hours = parseInt(card.querySelector('.sched-hours')?.value) || 1;
                    parsed.minute = parseInt(card.querySelector('.sched-minute')?.value) || 0;
                } else if (mode === 'daily') {
                    parsed.time = card.querySelector('.sched-time')?.value || '09:00';
                    parsed.days = Array.from(card.querySelectorAll('.sched-day-btn.active')).map(b => b.dataset.day);
                } else if (mode === 'monthly') {
                    parsed.time = card.querySelector('.sched-time')?.value || '09:00';
                    const monthMode = card.querySelector('.sched-month-mode:checked')?.value || 'day';
                    if (monthMode === 'ordinal') {
                        parsed.dayOfMonth = 0;
                        parsed.weekOrdinal = card.querySelector('.sched-month-ordinal')?.value || '1st';
                        parsed.weekDay = card.querySelector('.sched-month-dow')?.value || '1';
                    } else {
                        parsed.dayOfMonth = parseInt(card.querySelector('.sched-month-day')?.value) || 1;
                    }
                } else if (mode === 'yearly') {
                    parsed.time = card.querySelector('.sched-time')?.value || '09:00';
                    parsed.month = parseInt(card.querySelector('.sched-year-month')?.value) || 1;
                    parsed.monthDay = parseInt(card.querySelector('.sched-year-day')?.value) || 1;
                }
                this._macros[mi].trigger = { type: 'schedule', interval: this._buildScheduleInterval(parsed) };
            } else if (triggerType === 'signal') {
                this._macros[mi].trigger = { type: 'signal', signal: card.querySelector('.macro-signal-name')?.value || '', filter: card.querySelector('.macro-signal-filter')?.value || '' };
            }
            // Sync steps
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
            list.innerHTML = '<div class="setting-description" style="color:var(--kiro-text-muted);font-style:italic">No automations defined yet.</div>';
            return;
        }
        this._macros.forEach((macro, mi) => {
            const card = document.createElement('div');
            card.className = 'macro-card' + (macro.enabled === false ? ' disabled' : '');
            const outOpts = ['clipboard','replace','inform'].map(v => '<option value="' + v + '"' + (macro.output===v?' selected':'') + '>' + ({clipboard:'📋 Copy',replace:'✏️ Replace',inform:'💬 Show'}[v]) + '</option>').join('');
            let h = '<div class="macro-header">'
                + '<input type="checkbox" class="macro-enable-toggle" ' + (macro.enabled !== false ? 'checked' : '') + ' title="Enable/disable">'
                + '<input class="macro-icon-input" value="' + this._esc(macro.icon) + '" maxlength="2">'
                + '<input class="macro-name-input" value="' + this._esc(macro.name) + '" placeholder="Automation name">'
                + '<select class="macro-output-select">' + outOpts + '</select>'
                + '</div>';
            // Trigger section
            h += this._triggerHtml(macro.trigger || { type: 'manual' });
            // Steps
            h += '<div class="macro-steps">';
            macro.steps.forEach((step, si) => { h += this._stepHtml(step, si, macro.steps.length); });
            h += '</div><div class="macro-actions"><button class="setting-button macro-add-step-btn" style="font-size:12px;padding:3px 10px">+ Step</button><button class="macro-delete-btn">Delete</button></div>';
            card.innerHTML = h;
            list.appendChild(card);
            this._wireEvents(card, mi);
        });
    }
    _triggerHtml(trigger) {
            const t = trigger.type || 'manual';
            const typeOpts = [['manual','🖱️ Manual'],['schedule','⏰ Schedule'],['signal','⚡ Signal']].map(([v,l]) => '<option value="'+v+'"'+(t===v?' selected':'')+'>'+l+'</option>').join('');
            let config = '';
            if (t === 'schedule') {
                config = '<div class="macro-trigger-config">' + this._scheduleConfigHtml(trigger) + '</div>';
            } else if (t === 'signal') {
                const sigOpts = '<option value="">Select signal...</option>' + this._signals.map(s => '<option value="'+s.name+'"'+(trigger.signal===s.name?' selected':'')+'>' + (s.icon||'⚡') + ' ' + s.name + ' — ' + (s.description||'') + '</option>').join('');
                config = '<div class="macro-trigger-config"><select class="macro-signal-name">' + sigOpts + '</select>'
                    + '<input class="macro-signal-filter" value="' + this._esc(trigger.filter || '') + '" placeholder="Optional filter (text match on signal data)">'
                    + '</div>';
            }
            return '<div class="macro-trigger"><div class="macro-trigger-header"><span>Trigger:</span><select class="macro-trigger-type">' + typeOpts + '</select></div>' + config + '</div>';
        }

        _parseScheduleInterval(interval) {
            if (!interval) return { mode: 'daily', hours: 1, minute: 0, time: '09:00', days: [], dayOfMonth: 1, weekOrdinal: '1st', weekDay: '1', month: 1, monthDay: 1 };
            const r = { mode: 'daily', hours: 1, minute: 0, time: '09:00', days: [], dayOfMonth: 1, weekOrdinal: '1st', weekDay: '1', month: 1, monthDay: 1 };
            if (interval.startsWith('hourly_')) {
                r.mode = 'hourly';
                const rest = interval.substring(7);
                const parts = rest.split('_at_');
                r.hours = parseInt(parts[0]) || 1;
                r.minute = parts[1] ? parseInt(parts[1]) : 0;
            } else if (interval.startsWith('daily_')) {
                r.mode = 'daily';
                const rest = interval.substring(6);
                const daysPart = rest.match(/_days_([\d,]+)$/);
                r.time = daysPart ? rest.replace(daysPart[0], '') : rest;
                r.days = daysPart ? daysPart[1].split(',') : [];
            } else if (interval.startsWith('monthly_')) {
                r.mode = 'monthly';
                const rest = interval.substring(8);
                const ordMatch = rest.match(/^(\w+)_(\w+)_(.+)$/);
                if (ordMatch && ['1st','2nd','3rd','4th','last'].includes(ordMatch[1])) {
                    r.weekOrdinal = ordMatch[1]; r.weekDay = ordMatch[2]; r.time = ordMatch[3];
                    r.dayOfMonth = 0;
                } else {
                    const dayMatch = rest.match(/^(\d+)_(.+)$/);
                    if (dayMatch) { r.dayOfMonth = parseInt(dayMatch[1]); r.time = dayMatch[2]; }
                }
            } else if (interval.startsWith('yearly_')) {
                r.mode = 'yearly';
                const rest = interval.substring(7);
                const parts = rest.match(/^(\d+)-(\d+)_(.+)$/);
                if (parts) { r.month = parseInt(parts[1]); r.monthDay = parseInt(parts[2]); r.time = parts[3]; }
            } else if (interval.startsWith('every_')) {
                r.mode = 'hourly'; const rest = interval.substring(6);
                if (rest.endsWith('h')) r.hours = parseInt(rest) || 1;
                else if (rest.endsWith('m')) { r.hours = 0; r.minute = parseInt(rest) || 30; }
            }
            return r;
        }

        _buildScheduleInterval(parsed) {
            switch (parsed.mode) {
                case 'hourly': return `hourly_${parsed.hours}${parsed.minute ? '_at_' + parsed.minute : ''}`;
                case 'daily': return `daily_${parsed.time}${parsed.days.length > 0 && parsed.days.length < 7 ? '_days_' + parsed.days.join(',') : ''}`;
                case 'monthly':
                    if (parsed.dayOfMonth === 0) return `monthly_${parsed.weekOrdinal}_${parsed.weekDay}_${parsed.time}`;
                    return `monthly_${parsed.dayOfMonth}_${parsed.time}`;
                case 'yearly': return `yearly_${String(parsed.month).padStart(2,'0')}-${String(parsed.monthDay).padStart(2,'0')}_${parsed.time}`;
                default: return '';
            }
        }

        _scheduleConfigHtml(trigger) {
            const p = this._parseScheduleInterval(trigger.interval);
            const modeOpts = SCHEDULE_MODES.map(m => `<option value="${m.value}"${p.mode===m.value?' selected':''}>${m.label}</option>`).join('');
            let details = '';
            if (p.mode === 'hourly') {
                const hourOpts = [1,2,3,4,6,8,12].map(h => `<option value="${h}"${p.hours===h?' selected':''}>Every ${h} hour${h>1?'s':''}</option>`).join('');
                details = `<div style="display:flex;gap:8px;align-items:center;margin-top:6px;"><select class="sched-hours">${hourOpts}</select><span style="font-size:12px;color:var(--kiro-text-muted)">at minute</span><input type="number" class="sched-minute" min="0" max="59" value="${p.minute}" style="width:60px;"></div>`;
            } else if (p.mode === 'daily') {
                const dayBtns = DAYS_OF_WEEK.map(d => {
                    const active = p.days.length === 0 || p.days.includes(d.value);
                    return `<button type="button" class="sched-day-btn${active ? ' active' : ''}" data-day="${d.value}" style="width:32px;height:28px;border-radius:4px;border:1px solid var(--kiro-border-subtle);background:${active?'var(--kiro-accent)':'var(--kiro-bg-input)'};color:${active?'#fff':'var(--kiro-text-muted)'};cursor:pointer;font-size:11px;font-weight:600;">${d.label}</button>`;
                }).join('');
                details = `<div style="margin-top:6px;"><div style="display:flex;gap:4px;margin-bottom:6px;">${dayBtns}</div><div style="display:flex;gap:8px;align-items:center;"><span style="font-size:12px;color:var(--kiro-text-muted)">at</span><input type="time" class="sched-time" value="${p.time}" style="width:120px;"></div></div>`;
            } else if (p.mode === 'monthly') {
                const ordOpts = ['1st','2nd','3rd','4th','last'].map(o => `<option value="${o}"${p.weekOrdinal===o?' selected':''}>${o}</option>`).join('');
                const dowOpts = DAYS_OF_WEEK.map(d => `<option value="${d.value}"${p.weekDay===d.value?' selected':''}>${d.label}</option>`).join('');
                const dayNums = Array.from({length:31},(_,i)=>i+1).map(d => `<option value="${d}"${p.dayOfMonth===d?' selected':''}>${d}</option>`).join('');
                const isOrd = p.dayOfMonth === 0;
                details = `<div style="margin-top:6px;"><div style="display:flex;gap:6px;align-items:center;margin-bottom:6px;"><label style="font-size:12px;display:flex;align-items:center;gap:4px;cursor:pointer;"><input type="radio" name="monthMode" class="sched-month-mode" value="day" ${!isOrd?'checked':''}> Day <select class="sched-month-day" style="width:60px;" ${isOrd?'disabled':''}>${dayNums}</select></label></div><div style="display:flex;gap:6px;align-items:center;margin-bottom:6px;"><label style="font-size:12px;display:flex;align-items:center;gap:4px;cursor:pointer;"><input type="radio" name="monthMode" class="sched-month-mode" value="ordinal" ${isOrd?'checked':''}> <select class="sched-month-ordinal" style="width:70px;" ${!isOrd?'disabled':''}>${ordOpts}</select> <select class="sched-month-dow" style="width:70px;" ${!isOrd?'disabled':''}>${dowOpts}</select></label></div><div style="display:flex;gap:8px;align-items:center;"><span style="font-size:12px;color:var(--kiro-text-muted)">at</span><input type="time" class="sched-time" value="${p.time}" style="width:120px;"></div></div>`;
            } else if (p.mode === 'yearly') {
                const monthOpts = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'].map((m,i) => `<option value="${i+1}"${p.month===i+1?' selected':''}>${m}</option>`).join('');
                const dayNums = Array.from({length:31},(_,i)=>i+1).map(d => `<option value="${d}"${p.monthDay===d?' selected':''}>${d}</option>`).join('');
                details = `<div style="display:flex;gap:8px;align-items:center;margin-top:6px;"><select class="sched-year-month">${monthOpts}</select><select class="sched-year-day" style="width:60px;">${dayNums}</select><span style="font-size:12px;color:var(--kiro-text-muted)">at</span><input type="time" class="sched-time" value="${p.time}" style="width:120px;"></div>`;
            }
            return `<select class="macro-schedule-mode">${modeOpts}</select><input type="hidden" class="macro-schedule-interval" value="${this._esc(trigger.interval || '')}">${details}`;
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
        card.querySelector('.macro-enable-toggle')?.addEventListener('change', (e) => {
            this._macros[mi].enabled = e.target.checked;
            card.classList.toggle('disabled', !e.target.checked);
            this._markDirty();
        });
        card.querySelector('.macro-trigger-type')?.addEventListener('change', () => { this._syncFromDom(); this._renderMacros(); this._markDirty(); });
        card.querySelector('.macro-schedule-mode')?.addEventListener('change', () => { this._syncFromDom(); this._renderMacros(); this._markDirty(); });
        card.querySelectorAll('.sched-day-btn').forEach(btn => {
            btn.addEventListener('click', (e) => {
                e.preventDefault();
                btn.classList.toggle('active');
                btn.style.background = btn.classList.contains('active') ? 'var(--kiro-accent)' : 'var(--kiro-bg-input)';
                btn.style.color = btn.classList.contains('active') ? '#fff' : 'var(--kiro-text-muted)';
                this._markDirty();
            });
            btn.addEventListener('mousedown', e => e.preventDefault());
        });
        card.querySelectorAll('.sched-month-mode').forEach(radio => {
            radio.addEventListener('change', () => {
                const isOrd = radio.value === 'ordinal';
                card.querySelector('.sched-month-day')?.toggleAttribute('disabled', isOrd);
                card.querySelector('.sched-month-ordinal')?.toggleAttribute('disabled', !isOrd);
                card.querySelector('.sched-month-dow')?.toggleAttribute('disabled', !isOrd);
                this._markDirty();
            });
        });
        card.querySelector('.macro-add-step-btn')?.addEventListener('click', () => { this._syncFromDom(); this._macros[mi].steps.push({ step_type:'ai_prompt',prompt:'',find:'',replace:'',transform:'',script:'' }); this._renderMacros(); this._markDirty(); });
        card.querySelector('.macro-delete-btn')?.addEventListener('click', () => { this._syncFromDom(); this._macros.splice(mi,1); this._renderMacros(); this._markDirty(); });
        card.querySelectorAll('.macro-step-type').forEach(sel => { sel.addEventListener('change', () => { this._syncFromDom(); this._renderMacros(); this._markDirty(); }); });
        card.querySelectorAll('.macro-step-up').forEach(btn => { btn.addEventListener('click', e => { const si=parseInt(e.target.closest('.macro-step').dataset.step); if(si>0){this._syncFromDom();[this._macros[mi].steps[si-1],this._macros[mi].steps[si]]=[this._macros[mi].steps[si],this._macros[mi].steps[si-1]];this._renderMacros();this._markDirty();} }); });
        card.querySelectorAll('.macro-step-down').forEach(btn => { btn.addEventListener('click', e => { const si=parseInt(e.target.closest('.macro-step').dataset.step); if(si<this._macros[mi].steps.length-1){this._syncFromDom();[this._macros[mi].steps[si],this._macros[mi].steps[si+1]]=[this._macros[mi].steps[si+1],this._macros[mi].steps[si]];this._renderMacros();this._markDirty();} }); });
        card.querySelectorAll('.macro-step-remove').forEach(btn => { btn.addEventListener('click', e => { const si=parseInt(e.target.closest('.macro-step').dataset.step); this._syncFromDom(); this._macros[mi].steps.splice(si,1); if(!this._macros[mi].steps.length) this._macros[mi].steps.push({step_type:'ai_prompt',prompt:'',find:'',replace:'',transform:'',script:''}); this._renderMacros(); this._markDirty(); }); });
    }
    _esc(s) { return (s||'').replace(/"/g,'&quot;').replace(/</g,'&lt;'); }
}
