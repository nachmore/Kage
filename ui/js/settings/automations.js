/**
 * Automations Settings Module — collapsed/expanded card UI for automation rules.
 */
const TRANSFORMS = [
    { value: 'uppercase', label: 'UPPERCASE' }, { value: 'lowercase', label: 'lowercase' },
    { value: 'trim', label: 'Trim whitespace' }, { value: 'sort_lines', label: 'Sort lines' },
    { value: 'reverse_lines', label: 'Reverse lines' }, { value: 'remove_blank_lines', label: 'Remove blank lines' },
    { value: 'unique_lines', label: 'Unique lines' }, { value: 'number_lines', label: 'Number lines' },
    { value: 'count_words', label: 'Count words' }, { value: 'count_lines', label: 'Count lines' },
    { value: 'count_chars', label: 'Count characters' },
    { value: 'base64_encode', label: 'Base64 encode' }, { value: 'base64_decode', label: 'Base64 decode' },
];
const SCHEDULE_MODES = [
    { value: 'hourly', label: '🕐 Hourly' }, { value: 'daily', label: '📅 Daily' },
    { value: 'monthly', label: '🗓️ Monthly' }, { value: 'yearly', label: '📆 Yearly' },
];
const DAYS_OF_WEEK = [
    { value: '1', label: 'Mon' }, { value: '2', label: 'Tue' }, { value: '3', label: 'Wed' },
    { value: '4', label: 'Thu' }, { value: '5', label: 'Fri' }, { value: '6', label: 'Sat' },
    { value: '7', label: 'Sun' },
];

class AutomationsSettingsModule extends SettingsModule {
    constructor() {
        super('macros', 'Automations', '🔄');
        this._automations = [];
        this._signals = [];
        this._expandedIndex = -1;
        this._editSnapshot = null; // snapshot for cancel
    }
    render() {
        const css = `
            .auto-list { }
            .auto-card { background: var(--kage-bg-input); border: 1px solid var(--kage-border-subtle); border-radius: 10px; margin-bottom: 8px; overflow: hidden; transition: box-shadow 0.2s; }
            .auto-card.expanded { box-shadow: 0 0 0 1px var(--kage-accent, #C09CFF); }
            .auto-card.disabled { opacity: 0.45; }
            .auto-collapsed { display: flex; align-items: center; gap: 10px; padding: 10px 14px; cursor: pointer; user-select: none; }
            .auto-collapsed:hover { background: rgba(255,255,255,0.03); }
            .auto-collapsed-icon { font-size: 18px; flex-shrink: 0; }
            .auto-collapsed-info { flex: 1; min-width: 0; }
            .auto-collapsed-name { font-size: 13px; font-weight: 500; color: var(--kage-text-bright); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
            .auto-collapsed-summary { font-size: 11px; color: var(--kage-text-secondary, #938F9B); margin-top: 2px; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; white-space: normal; }
            .auto-trigger-badge { font-size: 10px; padding: 2px 8px; border-radius: 10px; background: var(--kage-bg-surface); color: var(--kage-text); white-space: nowrap; flex-shrink: 0; }
            .auto-enable-toggle { cursor: pointer; width: 16px; height: 16px; flex-shrink: 0; }
            .auto-expanded { padding: 0 14px 14px; }
            .auto-section-label { font-size: 10px; text-transform: uppercase; letter-spacing: 0.8px; color: var(--kage-text); margin: 12px 0 6px; font-weight: 600; }
            .auto-header-row { display: flex; align-items: center; gap: 8px; padding: 10px 14px; border-bottom: 1px solid var(--kage-border-subtle); }
            .auto-header-row input, .auto-header-row select { background: var(--kage-bg-surface); border: 1px solid var(--kage-border-subtle); border-radius: 4px; padding: 4px 8px; color: var(--kage-text); font-size: 13px; font-family: var(--kage-font); }
            .auto-header-row input:focus { border-color: var(--kage-accent); background: var(--kage-bg-input); outline: none; }
            .auto-icon-input { width: 32px !important; text-align: center; flex: none !important; font-size: 16px !important; padding: 2px !important; }
            .auto-name-input { flex: 1; font-weight: 500; font-size: 14px; }
            .auto-trigger { margin-top: 8px; }
            .auto-trigger-header { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--kage-text); }
            .auto-trigger-config { margin-top: 8px; }
            .auto-step { margin-bottom: 6px; padding: 8px 10px; background: var(--kage-bg-surface); border-radius: 8px; border: 1px solid var(--kage-border-subtle); }
            .auto-step-top { display: flex; align-items: center; gap: 6px; }
            .auto-step-num { font-size: 10px; color: var(--kage-text); width: 20px; height: 20px; display: flex; align-items: center; justify-content: center; background: var(--kage-bg-input); border-radius: 50%; flex-shrink: 0; font-weight: 600; }
            .auto-step-fields { padding-left: 26px; margin-top: 6px; }
            .auto-step-fields input, .auto-step-fields select, .auto-step-fields textarea { width: 100%; background: var(--kage-bg-input); border: 1px solid var(--kage-border-subtle); border-radius: 6px; padding: 5px 10px; color: var(--kage-text); font-size: 12px; font-family: var(--kage-font); margin-bottom: 4px; box-sizing: border-box; }
            .auto-step-fields input::placeholder { color: var(--kage-text-muted); }
            .auto-step-fields .field-row { display: flex; gap: 6px; }
            .auto-step-fields .field-row input { flex: 1; }
            .auto-step-btn { background: none; border: none; color: var(--kage-text-muted); cursor: pointer; font-size: 12px; padding: 2px 5px; border-radius: 4px; flex-shrink: 0; opacity: 0.6; }
            .auto-step-btn:hover { color: var(--kage-text); background: var(--kage-bg-input); opacity: 1; }
            .auto-actions { display: flex; gap: 8px; justify-content: space-between; align-items: center; padding-top: 10px; border-top: 1px solid var(--kage-border-subtle); margin-top: 10px; }
            .auto-actions .auto-save-btn { background: var(--kage-accent); color: #fff; border: none; border-radius: 6px; padding: 6px 16px; font-size: 12px; font-weight: 600; cursor: pointer; }
            .auto-actions .auto-save-btn:hover { opacity: 0.9; }
            .auto-actions .auto-cancel-btn { background: none; border: 1px solid var(--kage-border-subtle); border-radius: 6px; padding: 6px 16px; font-size: 12px; color: var(--kage-text); cursor: pointer; }
            .auto-actions .auto-cancel-btn:hover { background: var(--kage-bg-surface); }
            .auto-actions .auto-delete-btn { background: none; border: none; color: var(--kage-text-muted); cursor: pointer; font-size: 11px; padding: 4px 10px; border-radius: 4px; margin-left: auto; }
            .auto-actions .auto-delete-btn:hover { color: #e55; background: rgba(238,85,85,0.1); }
            .auto-validation-banner { background: rgba(217,119,6,0.15); border: 1px solid rgba(217,119,6,0.3); color: #fcd34d; border-radius: 6px; padding: 6px 12px; font-size: 12px; margin-bottom: 8px; }
            .auto-card select { background: var(--kage-bg-input); border: 1px solid var(--kage-border); border-radius: 4px; color: var(--kage-text); font-size: 13px; font-family: var(--kage-font); cursor: pointer; padding: 6px 10px; }
            .auto-card select:focus { outline: none; border-color: var(--kage-accent); }
            .auto-card select option { background: var(--kage-bg-input, #28242E); color: var(--kage-text, #E5E7EB); }
            .sched-day-btn { width: 34px; height: 30px; border-radius: 6px; border: 1px solid var(--kage-border-subtle); cursor: pointer; font-size: 11px; font-weight: 600; transition: all 0.15s; }
            .sched-day-btn.active { background: var(--kage-accent) !important; color: #fff !important; border-color: var(--kage-accent) !important; }
            .sched-day-btn:not(.active) { background: var(--kage-bg-surface); color: var(--kage-text-muted); }
            .sched-day-btn:hover:not(.active) { background: var(--kage-bg-input); color: var(--kage-text); }`;
        return '<div class="settings-section" id="' + this.id + '-section">'
            + '<h2 class="settings-section-header">' + this.icon + ' ' + this.title + '</h2>'
            + '<div class="setting-description" style="margin-bottom:12px">Chain transformations into automated actions. Trigger them manually, on a schedule, or in response to signals from extensions.</div>'
            + '<div class="setting-section-label">Power</div>'
            + '<div id="automationPowerSection"></div>'
            + '<div class="setting-section-label">Automations</div>'
            + '<div id="automationsList" class="auto-list"></div>'
            + '<button class="setting-button" id="addAutomationBtn" style="margin-top:8px">+ Add Automation</button>'
            + '<style>' + css + '</style>'
            + '</div>';
    }
    async initialize() {
        document.getElementById('addAutomationBtn')?.addEventListener('click', () => {
            this._automations.push({ name: 'New Automation', icon: '🔄', steps: [{ step_type: 'ai_prompt', prompt: '', find: '', replace: '', transform: '', condition: '', script: '' }], output: 'clipboard', trigger: { type: 'manual' }, enabled: true, summary: null });
            this._expandedIndex = this._automations.length - 1;
            this._editSnapshot = JSON.stringify(this._automations[this._expandedIndex]);
            this._renderList();
            this._markDirty();
        });
        try {
            const invoke = window.__TAURI__?.core?.invoke;
            if (invoke) {
                this._signals = await invoke('list_automation_signals');
                if (window._extensionManager) {
                    const extTriggers = (await window._extensionManager.getTriggerDefinitions?.()) || [];
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
        this._automations = JSON.parse(JSON.stringify(config.macros || []));
        for (const m of this._automations) {
            if (!m.trigger) m.trigger = { type: 'manual' };
            if (m.enabled === undefined) m.enabled = true;
        }
        this._powerConfig = config.automation_power || { mode: 'auto', battery_multiplier: 2.0, low_battery_multiplier: 4.0, disable_signals_on_low_battery: false };
        this._renderPowerSection();
        this._renderList();
    }
    save(config) {
        // Sync expanded card if any
        if (this._expandedIndex >= 0) this._syncExpandedFromDom();
        config.macros = JSON.parse(JSON.stringify(this._automations));
        config.automation_power = this._powerConfig || { mode: 'auto', battery_multiplier: 2.0, low_battery_multiplier: 4.0, disable_signals_on_low_battery: false };
    }
    validate() {
        if (this._expandedIndex >= 0) this._syncExpandedFromDom();
        for (const m of this._automations) {
            if (!m.name.trim()) return { valid: false, message: 'Automation name cannot be empty.' };
            if (m.steps.length === 0) return { valid: false, message: '"' + m.name + '" needs at least one step.' };
        }
        return { valid: true };
    }
    _markDirty() {
        document.getElementById('automationsList')?.dispatchEvent(new Event('input', { bubbles: true }));
    }
    _renderPowerSection() {
        const container = document.getElementById('automationPowerSection');
        if (!container) return;
        const p = this._powerConfig;
        const modeOpts = [['auto','🔋 Auto (detect battery)'],['full','⚡ Always full speed'],['saving','🪫 Always power saving']].map(([v,l]) => '<option value="'+v+'"'+(p.mode===v?' selected':'')+'>'+l+'</option>').join('');
        container.innerHTML = `<div class="setting-row"><div class="setting-label">Power Mode</div><div class="setting-description">Controls how automations behave on battery power.</div><select class="setting-select" id="automationPowerMode" style="max-width:280px;">${modeOpts}</select></div><div id="powerDetailsRow" style="${p.mode==='auto'?'':'display:none;'}"><div class="setting-row"><div class="setting-description">On battery, schedules run <strong>${p.battery_multiplier}×</strong> slower. On low battery, <strong>${p.low_battery_multiplier}×</strong> slower.</div></div></div>`;
        document.getElementById('automationPowerMode')?.addEventListener('change', (e) => {
            this._powerConfig.mode = e.target.value;
            const d = document.getElementById('powerDetailsRow');
            if (d) d.style.display = e.target.value === 'auto' ? '' : 'none';
            this._markDirty();
        });
    }

    // ── Trigger description for collapsed view ──
    _triggerBadgeText(trigger) {
        if (!trigger || trigger.type === 'manual') return '🖱️ Manual';
        if (trigger.type === 'schedule') {
            const p = this._parseScheduleInterval(trigger.interval);
            if (p.mode === 'hourly') return `🕐 Every ${p.hours}h`;
            if (p.mode === 'daily') {
                const dayCount = p.days.length;
                const dayLabel = dayCount > 0 && dayCount < 7 ? `${dayCount} days` : 'daily';
                return `📅 ${dayLabel} at ${p.time}`;
            }
            if (p.mode === 'monthly') return `🗓️ Monthly at ${p.time}`;
            if (p.mode === 'yearly') return `📆 Yearly at ${p.time}`;
            return '⏰ Schedule';
        }
        if (trigger.type === 'signal') {
            const sig = this._signals.find(s => s.name === trigger.signal);
            return (sig?.icon || '⚡') + ' ' + (trigger.signal || 'Signal');
        }
        return '🖱️ Manual';
    }

    // ── List rendering (collapsed + one expanded) ──
    _renderList() {
        const list = document.getElementById('automationsList');
        if (!list) return;
        list.innerHTML = '';
        if (this._automations.length === 0) {
            list.innerHTML = '<div class="setting-description" style="color:var(--kage-text-muted);font-style:italic">No automations defined yet.</div>';
            return;
        }
        this._automations.forEach((auto, i) => {
            const card = document.createElement('div');
            card.className = 'auto-card' + (auto.enabled === false ? ' disabled' : '') + (i === this._expandedIndex ? ' expanded' : '');
            if (i === this._expandedIndex) {
                card.innerHTML = this._expandedCardHtml(auto, i);
                list.appendChild(card);
                this._wireExpandedEvents(card, i);
            } else {
                card.innerHTML = this._collapsedCardHtml(auto, i);
                list.appendChild(card);
                card.querySelector('.auto-collapsed')?.addEventListener('click', () => {
                    if (this._expandedIndex >= 0) this._syncExpandedFromDom();
                    this._expandedIndex = i;
                    this._editSnapshot = JSON.stringify(this._automations[i]);
                    this._renderList();
                });
                card.querySelector('.auto-enable-toggle')?.addEventListener('click', (e) => {
                    e.stopPropagation();
                });
                card.querySelector('.auto-enable-toggle')?.addEventListener('change', (e) => {
                    this._automations[i].enabled = e.target.checked;
                    card.classList.toggle('disabled', !e.target.checked);
                    this._markDirty();
                });
            }
        });
    }

    _collapsedCardHtml(auto, i) {
        const badge = this._triggerBadgeText(auto.trigger);
        const summary = auto.summary || (auto.steps.length + ' step' + (auto.steps.length !== 1 ? 's' : ''));
        const tooltip = auto.summary ? ' title="' + this._esc(auto.summary) + '"' : '';
        return `<div class="auto-collapsed"${tooltip}>
            <span class="auto-collapsed-icon">${auto.icon || '🔄'}</span>
            <div class="auto-collapsed-info">
                <div class="auto-collapsed-name">${this._esc(auto.name)}</div>
                <div class="auto-collapsed-summary">${this._esc(summary)}</div>
            </div>
            <span class="auto-trigger-badge">${badge}</span>
            <input type="checkbox" class="auto-enable-toggle" ${auto.enabled !== false ? 'checked' : ''} title="Enable/disable">
        </div>`;
    }

    _expandedCardHtml(auto, i) {
        const outOpts = ['clipboard','replace','inform'].map(v => '<option value="'+v+'"'+(auto.output===v?' selected':'')+'>'+ ({clipboard:'📋 Copy',replace:'✏️ Replace',inform:'💬 Show'}[v])+'</option>').join('');
        const trigTypeOpts = [['manual','🖱️ Manual'],['schedule','⏰ Schedule'],['signal','⚡ Signal']].map(([v,l]) => '<option value="'+v+'"'+((auto.trigger?.type||'manual')===v?' selected':'')+'>'+l+'</option>').join('');

        let trigConfig = '';
        const t = auto.trigger?.type || 'manual';
        if (t === 'schedule') {
            trigConfig = this._scheduleConfigHtml(auto.trigger);
        } else if (t === 'signal') {
            const sigOpts = '<option value="">Select signal...</option>' + this._signals.map(s => '<option value="'+s.name+'"'+(auto.trigger.signal===s.name?' selected':'')+'>'+(s.icon||'⚡')+' '+s.name+' — '+(s.description||'')+'</option>').join('');
            trigConfig = '<select class="auto-signal-name" style="width:100%;margin-bottom:6px;">'+sigOpts+'</select><input class="auto-signal-filter setting-input" value="'+this._esc(auto.trigger.filter||'')+'" placeholder="Optional filter (text match on signal data)">';
        } else {
            trigConfig = '<div style="font-size:11px;color:var(--kage-text-secondary);margin-top:4px;">Runs via inline assist hotkey or quick actions.</div>';
        }

        let stepsHtml = '';
        auto.steps.forEach((step, si) => { stepsHtml += this._stepHtml(step, si, auto.steps.length); });

        return `<div id="autoValidation_${i}" style="display:none;"></div>
            <div class="auto-header-row">
                <input class="auto-icon-input" value="${this._esc(auto.icon)}" maxlength="2">
                <input class="auto-name-input" value="${this._esc(auto.name)}" placeholder="Automation name" style="flex:1;font-weight:500;font-size:14px;">
                <span style="font-size:11px;color:var(--kage-text-secondary);flex-shrink:0;">Output:</span>
                <select class="auto-output-select">${outOpts}</select>
            </div>
            <div class="auto-expanded">
                <div class="auto-section-label">WHEN</div>
                <div class="auto-trigger">
                    <div class="auto-trigger-header"><span>Trigger:</span><select class="auto-trigger-type">${trigTypeOpts}</select></div>
                    <div class="auto-trigger-config">${trigConfig}</div>
                </div>
                <div class="auto-section-label">THEN</div>
                <div class="auto-steps">${stepsHtml}</div>
                <button class="setting-button" style="font-size:11px;padding:3px 10px;margin-top:4px;" id="autoAddStep_${i}">+ Step</button>
                <div class="auto-actions">
                    <button class="auto-save-btn" id="autoSave_${i}">Save</button>
                    <button class="auto-cancel-btn" id="autoCancel_${i}">Cancel</button>
                    <button class="auto-delete-btn" id="autoDelete_${i}">Delete</button>
                </div>
            </div>`;
    }

    _wireExpandedEvents(card, i) {
        card.querySelector('.auto-trigger-type')?.addEventListener('change', () => { this._syncExpandedFromDom(); this._renderList(); });
        card.querySelector('.auto-schedule-mode')?.addEventListener('change', () => { this._syncExpandedFromDom(); this._renderList(); });
        card.querySelectorAll('.sched-day-btn').forEach(btn => {
            btn.addEventListener('click', (e) => { e.preventDefault(); btn.classList.toggle('active'); this._markDirty(); });
            btn.addEventListener('mousedown', e => e.preventDefault());
        });
        card.querySelectorAll('.sched-month-mode').forEach(radio => {
            radio.addEventListener('change', () => {
                const isOrd = radio.value === 'ordinal';
                card.querySelector('.sched-month-day')?.toggleAttribute('disabled', isOrd);
                card.querySelector('.sched-month-ordinal')?.toggleAttribute('disabled', !isOrd);
                card.querySelector('.sched-month-dow')?.toggleAttribute('disabled', !isOrd);
            });
        });
        document.getElementById(`autoAddStep_${i}`)?.addEventListener('click', () => {
            this._syncExpandedFromDom();
            this._automations[i].steps.push({ step_type: 'ai_prompt', prompt: '', find: '', replace: '', transform: '', condition: '', script: '' });
            this._renderList();
        });
        document.getElementById(`autoSave_${i}`)?.addEventListener('click', () => {
            this._syncExpandedFromDom();
            const err = this._validateOne(this._automations[i]);
            if (err) {
                const banner = document.getElementById(`autoValidation_${i}`);
                if (banner) { banner.className = 'auto-validation-banner'; banner.textContent = err; banner.style.display = ''; }
                return;
            }
            this._expandedIndex = -1;
            this._editSnapshot = null;
            this._renderList();
            this._markDirty();
            this._generateSummary(i);
        });
        document.getElementById(`autoCancel_${i}`)?.addEventListener('click', () => {
            if (this._editSnapshot) {
                this._automations[i] = JSON.parse(this._editSnapshot);
            }
            this._expandedIndex = -1;
            this._editSnapshot = null;
            this._renderList();
        });
        document.getElementById(`autoDelete_${i}`)?.addEventListener('click', () => {
            this._automations.splice(i, 1);
            this._expandedIndex = -1;
            this._editSnapshot = null;
            this._renderList();
            this._markDirty();
        });
        card.querySelectorAll('.auto-step-type').forEach(sel => {
            sel.addEventListener('change', () => { this._syncExpandedFromDom(); this._renderList(); });
        });
        card.querySelectorAll('.auto-step-up').forEach(btn => {
            btn.addEventListener('click', e => {
                const si = parseInt(e.target.closest('.auto-step').dataset.step);
                if (si > 0) { this._syncExpandedFromDom(); [this._automations[i].steps[si-1], this._automations[i].steps[si]] = [this._automations[i].steps[si], this._automations[i].steps[si-1]]; this._renderList(); }
            });
        });
        card.querySelectorAll('.auto-step-down').forEach(btn => {
            btn.addEventListener('click', e => {
                const si = parseInt(e.target.closest('.auto-step').dataset.step);
                if (si < this._automations[i].steps.length - 1) { this._syncExpandedFromDom(); [this._automations[i].steps[si], this._automations[i].steps[si+1]] = [this._automations[i].steps[si+1], this._automations[i].steps[si]]; this._renderList(); }
            });
        });
        card.querySelectorAll('.auto-step-remove').forEach(btn => {
            btn.addEventListener('click', e => {
                const si = parseInt(e.target.closest('.auto-step').dataset.step);
                this._syncExpandedFromDom();
                this._automations[i].steps.splice(si, 1);
                if (!this._automations[i].steps.length) this._automations[i].steps.push({ step_type: 'ai_prompt', prompt: '', find: '', replace: '', transform: '', condition: '', script: '' });
                this._renderList();
            });
        });
        // Mount script editors
        card.querySelectorAll('.step-script-container').forEach((container) => {
            import('../shared/script-editor.js').then(({ createScriptEditor }) => {
                const editor = createScriptEditor(container, {
                    id: `auto_${i}_script_${Math.random().toString(36).slice(2,6)}`,
                    value: container.dataset.script || '',
                    variableHint: 'input',
                    contextHint: 'Return a string.',
                    rows: 5,
                });
                container._editor = editor;
            });
        });
    }

    _validateOne(auto) {
        if (!auto.name.trim()) return 'Name is required.';
        if (auto.trigger?.type === 'schedule' && !auto.trigger.interval) return 'Schedule trigger needs an interval configured.';
        if (auto.trigger?.type === 'signal' && !auto.trigger.signal) return 'Signal trigger needs a signal selected.';
        if (auto.steps.length === 0) return 'At least one step is required.';
        return null;
    }

    _syncExpandedFromDom() {
        const i = this._expandedIndex;
        if (i < 0 || !this._automations[i]) return;
        const card = document.querySelectorAll('.auto-card')[i];
        if (!card) return;
        this._automations[i].name = card.querySelector('.auto-name-input')?.value || '';
        this._automations[i].icon = card.querySelector('.auto-icon-input')?.value || '🔄';
        this._automations[i].output = card.querySelector('.auto-output-select')?.value || 'clipboard';
        const trigType = card.querySelector('.auto-trigger-type')?.value || 'manual';
        if (trigType === 'manual') {
            this._automations[i].trigger = { type: 'manual' };
        } else if (trigType === 'schedule') {
            const mode = card.querySelector('.auto-schedule-mode')?.value || 'daily';
            const parsed = { mode, hours: 1, minute: 0, time: '09:00', days: [], dayOfMonth: 1, weekOrdinal: '1st', weekDay: '1', month: 1, monthDay: 1 };
            if (mode === 'hourly') { parsed.hours = parseInt(card.querySelector('.sched-hours')?.value) || 1; parsed.minute = parseInt(card.querySelector('.sched-minute')?.value) || 0; }
            else if (mode === 'daily') { parsed.time = card.querySelector('.sched-time')?.value || '09:00'; parsed.days = Array.from(card.querySelectorAll('.sched-day-btn.active')).map(b => b.dataset.day); }
            else if (mode === 'monthly') { parsed.time = card.querySelector('.sched-time')?.value || '09:00'; const mm = card.querySelector('.sched-month-mode:checked')?.value || 'day'; if (mm === 'ordinal') { parsed.dayOfMonth = 0; parsed.weekOrdinal = card.querySelector('.sched-month-ordinal')?.value || '1st'; parsed.weekDay = card.querySelector('.sched-month-dow')?.value || '1'; } else { parsed.dayOfMonth = parseInt(card.querySelector('.sched-month-day')?.value) || 1; } }
            else if (mode === 'yearly') { parsed.time = card.querySelector('.sched-time')?.value || '09:00'; parsed.month = parseInt(card.querySelector('.sched-year-month')?.value) || 1; parsed.monthDay = parseInt(card.querySelector('.sched-year-day')?.value) || 1; }
            this._automations[i].trigger = { type: 'schedule', interval: this._buildScheduleInterval(parsed) };
        } else if (trigType === 'signal') {
            this._automations[i].trigger = { type: 'signal', signal: card.querySelector('.auto-signal-name')?.value || '', filter: card.querySelector('.auto-signal-filter')?.value || '' };
        }
        this._automations[i].steps = Array.from(card.querySelectorAll('.auto-step')).map(el => ({
            step_type: el.querySelector('.auto-step-type')?.value || 'ai_prompt',
            prompt: el.querySelector('.step-prompt')?.value || '',
            find: el.querySelector('.step-find')?.value || '',
            replace: el.querySelector('.step-replace')?.value || '',
            transform: el.querySelector('.step-transform')?.value || '',
            condition: el.querySelector('.step-condition')?.value || '',
            script: el.querySelector('.step-script-container')?._editor?.getValue() || '',
        }));
    }

    async _generateSummary(i) {
        const auto = this._automations[i];
        if (!auto) return;
        // Show loading indicator in the collapsed view
        auto.summary = '⏳ Generating summary...';
        this._renderList();
        try {
            const invoke = window.__TAURI__?.core?.invoke;
            const listen = window.__TAURI__?.event?.listen;
            if (!invoke || !listen) return;
            const prompt = `<role>You write ultra-concise automation summaries.</role>\n<automation>${JSON.stringify({ steps: auto.steps.map(s => ({ type: s.step_type, prompt: s.prompt, find: s.find, replace: s.replace, transform: s.transform, condition: s.condition, script: s.script || undefined })), output: auto.output })}</automation>\n<task>Summarize the OUTCOME of this automation in one short sentence. Do NOT mention the name "${auto.name}" or the trigger — the user can already see both. Focus only on what the steps produce. Be concise. No markdown.</task>`;
            let response = '';
            const unlisten = await listen('message_chunk', (event) => {
                const delta = (event.payload && typeof event.payload === 'object')
                    ? (event.payload.text || '')
                    : String(event.payload || '');
                response += delta;
            });
            const done = new Promise(resolve => { listen('message_complete', () => resolve()).then(fn => { done._ul = fn; }); });
            await invoke('send_message_streaming', { message: prompt, attachments: null });
            await done;
            unlisten();
            if (done._ul) done._ul();
            let summary = response.trim().replace(/```[\s\S]*?```/g, '').trim();
            if (summary.length > 200) summary = summary.substring(0, 197) + '...';
            this._automations[i].summary = summary;
            this._markDirty();
            this._renderList();
        } catch (e) {
            console.warn('[Automations] Summary generation failed:', e);
            this._automations[i].summary = null; // clear loading state
            this._renderList();
        }
    }

    // ── Schedule helpers ──
    _parseScheduleInterval(interval) {
        if (!interval) return { mode: 'daily', hours: 1, minute: 0, time: '09:00', days: [], dayOfMonth: 1, weekOrdinal: '1st', weekDay: '1', month: 1, monthDay: 1 };
        const r = { mode: 'daily', hours: 1, minute: 0, time: '09:00', days: [], dayOfMonth: 1, weekOrdinal: '1st', weekDay: '1', month: 1, monthDay: 1 };
        if (interval.startsWith('hourly_')) { r.mode = 'hourly'; const rest = interval.substring(7); const parts = rest.split('_at_'); r.hours = parseInt(parts[0]) || 1; r.minute = parts[1] ? parseInt(parts[1]) : 0; }
        else if (interval.startsWith('daily_')) { r.mode = 'daily'; const rest = interval.substring(6); const dp = rest.match(/_days_([\d,]+)$/); r.time = dp ? rest.replace(dp[0], '') : rest; r.days = dp ? dp[1].split(',') : []; }
        else if (interval.startsWith('monthly_')) { r.mode = 'monthly'; const rest = interval.substring(8); const om = rest.match(/^(\w+)_(\w+)_(.+)$/); if (om && ['1st','2nd','3rd','4th','last'].includes(om[1])) { r.weekOrdinal = om[1]; r.weekDay = om[2]; r.time = om[3]; r.dayOfMonth = 0; } else { const dm = rest.match(/^(\d+)_(.+)$/); if (dm) { r.dayOfMonth = parseInt(dm[1]); r.time = dm[2]; } } }
        else if (interval.startsWith('yearly_')) { r.mode = 'yearly'; const rest = interval.substring(7); const p = rest.match(/^(\d+)-(\d+)_(.+)$/); if (p) { r.month = parseInt(p[1]); r.monthDay = parseInt(p[2]); r.time = p[3]; } }
        else if (interval.startsWith('every_')) { r.mode = 'hourly'; const rest = interval.substring(6); if (rest.endsWith('h')) r.hours = parseInt(rest) || 1; else if (rest.endsWith('m')) { r.hours = 0; r.minute = parseInt(rest) || 30; } }
        return r;
    }
    _buildScheduleInterval(p) {
        if (p.mode === 'hourly') return `hourly_${p.hours}${p.minute ? '_at_' + p.minute : ''}`;
        if (p.mode === 'daily') return `daily_${p.time}${p.days.length > 0 && p.days.length < 7 ? '_days_' + p.days.join(',') : ''}`;
        if (p.mode === 'monthly') return p.dayOfMonth === 0 ? `monthly_${p.weekOrdinal}_${p.weekDay}_${p.time}` : `monthly_${p.dayOfMonth}_${p.time}`;
        if (p.mode === 'yearly') return `yearly_${String(p.month).padStart(2,'0')}-${String(p.monthDay).padStart(2,'0')}_${p.time}`;
        return '';
    }
    _scheduleConfigHtml(trigger) {
        const p = this._parseScheduleInterval(trigger.interval);
        const modeOpts = SCHEDULE_MODES.map(m => `<option value="${m.value}"${p.mode===m.value?' selected':''}>${m.label}</option>`).join('');
        let d = '';
        if (p.mode === 'hourly') { const ho = [1,2,3,4,6,8,12].map(h => `<option value="${h}"${p.hours===h?' selected':''}>Every ${h}h</option>`).join(''); d = `<div style="display:flex;gap:8px;align-items:center;margin-top:6px;"><select class="sched-hours">${ho}</select><span style="font-size:12px;color:var(--kage-text)">at minute</span><input type="number" class="sched-minute" min="0" max="59" value="${p.minute}" style="width:60px;"></div>`; }
        else if (p.mode === 'daily') { const db = DAYS_OF_WEEK.map(dw => `<button type="button" class="sched-day-btn${p.days.length===0||p.days.includes(dw.value)?' active':''}" data-day="${dw.value}">${dw.label}</button>`).join(''); d = `<div style="margin-top:6px;"><div style="display:flex;gap:4px;margin-bottom:6px;">${db}</div><div style="display:flex;gap:8px;align-items:center;"><span style="font-size:12px;color:var(--kage-text)">at</span><input type="time" class="sched-time" value="${p.time}" style="width:120px;"></div></div>`; }
        else if (p.mode === 'monthly') { const oo = ['1st','2nd','3rd','4th','last'].map(o => `<option value="${o}"${p.weekOrdinal===o?' selected':''}>${o}</option>`).join(''); const dwo = DAYS_OF_WEEK.map(dw => `<option value="${dw.value}"${p.weekDay===dw.value?' selected':''}>${dw.label}</option>`).join(''); const dn = Array.from({length:31},(_,j)=>j+1).map(n => `<option value="${n}"${p.dayOfMonth===n?' selected':''}>${n}</option>`).join(''); const io = p.dayOfMonth===0; d = `<div style="margin-top:6px;"><div style="display:flex;gap:6px;align-items:center;margin-bottom:6px;"><label style="font-size:12px;display:flex;align-items:center;gap:4px;cursor:pointer;"><input type="radio" name="monthMode" class="sched-month-mode" value="day" ${!io?'checked':''}> Day <select class="sched-month-day" style="width:60px;" ${io?'disabled':''}>${dn}</select></label></div><div style="display:flex;gap:6px;align-items:center;margin-bottom:6px;"><label style="font-size:12px;display:flex;align-items:center;gap:4px;cursor:pointer;"><input type="radio" name="monthMode" class="sched-month-mode" value="ordinal" ${io?'checked':''}> <select class="sched-month-ordinal" style="width:70px;" ${!io?'disabled':''}>${oo}</select> <select class="sched-month-dow" style="width:70px;" ${!io?'disabled':''}>${dwo}</select></label></div><div style="display:flex;gap:8px;align-items:center;"><span style="font-size:12px;color:var(--kage-text)">at</span><input type="time" class="sched-time" value="${p.time}" style="width:120px;"></div></div>`; }
        else if (p.mode === 'yearly') { const mo = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'].map((m,j) => `<option value="${j+1}"${p.month===j+1?' selected':''}>${m}</option>`).join(''); const dn = Array.from({length:31},(_,j)=>j+1).map(n => `<option value="${n}"${p.monthDay===n?' selected':''}>${n}</option>`).join(''); d = `<div style="display:flex;gap:8px;align-items:center;margin-top:6px;"><select class="sched-year-month">${mo}</select><select class="sched-year-day" style="width:60px;">${dn}</select><span style="font-size:12px;color:var(--kage-text)">at</span><input type="time" class="sched-time" value="${p.time}" style="width:120px;"></div>`; }
        return `<select class="auto-schedule-mode">${modeOpts}</select>${d}`;
    }

    // ── Step HTML ──
    _stepHtml(step, si, total) {
        const t = step.step_type || 'ai_prompt';
        const tOpts = [['ai_prompt','🤖 AI Prompt'],['find_replace','🔍 Find/Replace'],['transform','⚙️ Transform'],['condition','🔀 Condition'],['script','📜 Script']].map(([v,l]) => '<option value="'+v+'"'+(t===v?' selected':'')+'>'+l+'</option>').join('');
        let fields = '';
        if (t === 'ai_prompt') fields = '<input class="step-prompt" value="'+this._esc(step.prompt)+'" placeholder="Prompt... use {input} for previous output">';
        else if (t === 'find_replace') fields = '<div class="field-row"><input class="step-find" value="'+this._esc(step.find)+'" placeholder="Find (regex)"><input class="step-replace" value="'+this._esc(step.replace)+'" placeholder="Replace with"></div>';
        else if (t === 'transform') { const xo = TRANSFORMS.map(x => '<option value="'+x.value+'"'+(step.transform===x.value?' selected':'')+'>'+x.label+'</option>').join(''); fields = '<select class="step-transform">'+xo+'</select>'; }
        else if (t === 'condition') fields = '<input class="step-condition" value="'+this._esc(step.condition||'')+'" placeholder="Continue only if output contains this text"><div style="font-size:10px;color:var(--kage-text-secondary);margin-top:2px;">Stops the automation if the previous output doesn\'t match.</div>';
        else if (t === 'script') fields = '<div class="step-script-container" data-script="'+this._esc(step.script)+'"></div>';
        return '<div class="auto-step" data-step="'+si+'"><div class="auto-step-top"><span class="auto-step-num">'+(si+1)+'.</span><select class="auto-step-type">'+tOpts+'</select><span style="flex:1"></span><button class="auto-step-btn auto-step-up"'+(si===0?' disabled':'')+'>↑</button><button class="auto-step-btn auto-step-down"'+(si===total-1?' disabled':'')+'>↓</button><button class="auto-step-btn auto-step-remove">✕</button></div><div class="auto-step-fields">'+fields+'</div></div>';
    }

    _esc(s) { return (s||'').replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;'); }
}
