import { t } from '../shared/i18n.js';

const TRANSFORM_VALUES = [
    'uppercase',
    'lowercase',
    'trim',
    'sort_lines',
    'reverse_lines',
    'remove_blank_lines',
    'unique_lines',
    'number_lines',
    'count_words',
    'count_lines',
    'count_chars',
    'base64_encode',
    'base64_decode',
];
const SCHEDULE_MODE_VALUES = ['hourly', 'daily', 'monthly', 'yearly'];
const DAY_OF_WEEK_VALUES = [
    { value: '1', key: 'mon' },
    { value: '2', key: 'tue' },
    { value: '3', key: 'wed' },
    { value: '4', key: 'thu' },
    { value: '5', key: 'fri' },
    { value: '6', key: 'sat' },
    { value: '7', key: 'sun' },
];

function transforms() {
    return TRANSFORM_VALUES.map((value) => ({
        value,
        label: t(`settings.automations.transform.${value}`),
    }));
}
function scheduleModes() {
    return SCHEDULE_MODE_VALUES.map((value) => ({
        value,
        label: t(`settings.automations.schedule.${value}`),
    }));
}
function daysOfWeek() {
    return DAY_OF_WEEK_VALUES.map((d) => ({
        value: d.value,
        label: t(`settings.automations.day.${d.key}`),
    }));
}

export function parseScheduleInterval(interval) {
    if (!interval)
        return {
            mode: 'daily',
            hours: 1,
            minute: 0,
            time: '09:00',
            days: [],
            dayOfMonth: 1,
            weekOrdinal: '1st',
            weekDay: '1',
            month: 1,
            monthDay: 1,
        };
    const r = {
        mode: 'daily',
        hours: 1,
        minute: 0,
        time: '09:00',
        days: [],
        dayOfMonth: 1,
        weekOrdinal: '1st',
        weekDay: '1',
        month: 1,
        monthDay: 1,
    };
    if (interval.startsWith('hourly_')) {
        r.mode = 'hourly';
        const rest = interval.substring(7);
        const parts = rest.split('_at_');
        r.hours = parseInt(parts[0], 10) || 1;
        r.minute = parts[1] ? parseInt(parts[1], 10) : 0;
    } else if (interval.startsWith('daily_')) {
        r.mode = 'daily';
        const rest = interval.substring(6);
        const dp = rest.match(/_days_([\d,]+)$/);
        r.time = dp ? rest.replace(dp[0], '') : rest;
        r.days = dp ? dp[1].split(',') : [];
    } else if (interval.startsWith('monthly_')) {
        r.mode = 'monthly';
        const rest = interval.substring(8);
        const om = rest.match(/^(\w+)_(\w+)_(.+)$/);
        if (om && ['1st', '2nd', '3rd', '4th', 'last'].includes(om[1])) {
            r.weekOrdinal = om[1];
            r.weekDay = om[2];
            r.time = om[3];
            r.dayOfMonth = 0;
        } else {
            const dm = rest.match(/^(\d+)_(.+)$/);
            if (dm) {
                r.dayOfMonth = parseInt(dm[1], 10);
                r.time = dm[2];
            }
        }
    } else if (interval.startsWith('yearly_')) {
        r.mode = 'yearly';
        const rest = interval.substring(7);
        const p = rest.match(/^(\d+)-(\d+)_(.+)$/);
        if (p) {
            r.month = parseInt(p[1], 10);
            r.monthDay = parseInt(p[2], 10);
            r.time = p[3];
        }
    } else if (interval.startsWith('every_')) {
        r.mode = 'hourly';
        const rest = interval.substring(6);
        if (rest.endsWith('h')) r.hours = parseInt(rest, 10) || 1;
        else if (rest.endsWith('m')) {
            r.hours = 0;
            r.minute = parseInt(rest, 10) || 30;
        }
    }
    return r;
}
export function buildScheduleInterval(p) {
    if (p.mode === 'hourly') return `hourly_${p.hours}${p.minute ? '_at_' + p.minute : ''}`;
    if (p.mode === 'daily')
        return `daily_${p.time}${p.days.length > 0 && p.days.length < 7 ? '_days_' + p.days.join(',') : ''}`;
    if (p.mode === 'monthly')
        return p.dayOfMonth === 0
            ? `monthly_${p.weekOrdinal}_${p.weekDay}_${p.time}`
            : `monthly_${p.dayOfMonth}_${p.time}`;
    if (p.mode === 'yearly')
        return `yearly_${String(p.month).padStart(2, '0')}-${String(p.monthDay).padStart(2, '0')}_${p.time}`;
    return '';
}
export function scheduleConfigHtml(trigger) {
    const p = parseScheduleInterval(trigger.interval);
    const modeOpts = scheduleModes()
        .map(
            (m) =>
                `<option value="${m.value}"${p.mode === m.value ? ' selected' : ''}>${m.label}</option>`
        )
        .join('');
    let d = '';
    if (p.mode === 'hourly') {
        const ho = [1, 2, 3, 4, 6, 8, 12]
            .map(
                (h) =>
                    `<option value="${h}"${p.hours === h ? ' selected' : ''}>Every ${h}h</option>`
            )
            .join('');
        d = `<div style="display:flex;gap:8px;align-items:center;margin-top:6px;"><select class="sched-hours">${ho}</select><span style="font-size:12px;color:var(--kage-text)">at minute</span><input type="number" class="sched-minute" min="0" max="59" value="${p.minute}" style="width:60px;"></div>`;
    } else if (p.mode === 'daily') {
        const db = daysOfWeek()
            .map(
                (dw) =>
                    `<button type="button" class="sched-day-btn${p.days.length === 0 || p.days.includes(dw.value) ? ' active' : ''}" data-day="${dw.value}">${dw.label}</button>`
            )
            .join('');
        d = `<div style="margin-top:6px;"><div style="display:flex;gap:4px;margin-bottom:6px;">${db}</div><div style="display:flex;gap:8px;align-items:center;"><span style="font-size:12px;color:var(--kage-text)">at</span><input type="time" class="sched-time" value="${p.time}" style="width:120px;"></div></div>`;
    } else if (p.mode === 'monthly') {
        const oo = ['1st', '2nd', '3rd', '4th', 'last']
            .map(
                (o) => `<option value="${o}"${p.weekOrdinal === o ? ' selected' : ''}>${o}</option>`
            )
            .join('');
        const dwo = daysOfWeek()
            .map(
                (dw) =>
                    `<option value="${dw.value}"${p.weekDay === dw.value ? ' selected' : ''}>${dw.label}</option>`
            )
            .join('');
        const dn = Array.from({ length: 31 }, (_, j) => j + 1)
            .map(
                (n) => `<option value="${n}"${p.dayOfMonth === n ? ' selected' : ''}>${n}</option>`
            )
            .join('');
        const io = p.dayOfMonth === 0;
        d = `<div style="margin-top:6px;"><div style="display:flex;gap:6px;align-items:center;margin-bottom:6px;"><label style="font-size:12px;display:flex;align-items:center;gap:4px;cursor:pointer;"><input type="radio" name="monthMode" class="sched-month-mode" value="day" ${!io ? 'checked' : ''}> Day <select class="sched-month-day" style="width:60px;" ${io ? 'disabled' : ''}>${dn}</select></label></div><div style="display:flex;gap:6px;align-items:center;margin-bottom:6px;"><label style="font-size:12px;display:flex;align-items:center;gap:4px;cursor:pointer;"><input type="radio" name="monthMode" class="sched-month-mode" value="ordinal" ${io ? 'checked' : ''}> <select class="sched-month-ordinal" style="width:70px;" ${!io ? 'disabled' : ''}>${oo}</select> <select class="sched-month-dow" style="width:70px;" ${!io ? 'disabled' : ''}>${dwo}</select></label></div><div style="display:flex;gap:8px;align-items:center;"><span style="font-size:12px;color:var(--kage-text)">at</span><input type="time" class="sched-time" value="${p.time}" style="width:120px;"></div></div>`;
    } else if (p.mode === 'yearly') {
        const mo = [
            'Jan',
            'Feb',
            'Mar',
            'Apr',
            'May',
            'Jun',
            'Jul',
            'Aug',
            'Sep',
            'Oct',
            'Nov',
            'Dec',
        ]
            .map(
                (m, j) =>
                    `<option value="${j + 1}"${p.month === j + 1 ? ' selected' : ''}>${m}</option>`
            )
            .join('');
        const dn = Array.from({ length: 31 }, (_, j) => j + 1)
            .map((n) => `<option value="${n}"${p.monthDay === n ? ' selected' : ''}>${n}</option>`)
            .join('');
        d = `<div style="display:flex;gap:8px;align-items:center;margin-top:6px;"><select class="sched-year-month">${mo}</select><select class="sched-year-day" style="width:60px;">${dn}</select><span style="font-size:12px;color:var(--kage-text)">at</span><input type="time" class="sched-time" value="${p.time}" style="width:120px;"></div>`;
    }
    return `<select class="auto-schedule-mode">${modeOpts}</select>${d}`;
}

// ── Step HTML ──
export function stepHtml(step, si, total) {
    const t = step.step_type || 'ai_prompt';
    const tOpts = [
        ['ai_prompt', '🤖 AI Prompt'],
        ['find_replace', '🔍 Find/Replace'],
        ['transform', '⚙️ Transform'],
        ['condition', '🔀 Condition'],
        ['script', '📜 Script'],
    ]
        .map(
            ([v, l]) =>
                '<option value="' + v + '"' + (t === v ? ' selected' : '') + '>' + l + '</option>'
        )
        .join('');
    let fields = '';
    if (t === 'ai_prompt')
        fields =
            '<input class="step-prompt" value="' +
            escapeAutomationHtml(step.prompt) +
            '" placeholder="Prompt... use {input} for previous output">';
    else if (t === 'find_replace')
        fields =
            '<div class="field-row"><input class="step-find" value="' +
            escapeAutomationHtml(step.find) +
            '" placeholder="Find (regex)"><input class="step-replace" value="' +
            escapeAutomationHtml(step.replace) +
            '" placeholder="Replace with"></div>';
    else if (t === 'transform') {
        const xo = transforms()
            .map(
                (x) =>
                    '<option value="' +
                    x.value +
                    '"' +
                    (step.transform === x.value ? ' selected' : '') +
                    '>' +
                    x.label +
                    '</option>'
            )
            .join('');
        fields = '<select class="step-transform">' + xo + '</select>';
    } else if (t === 'condition')
        fields =
            '<input class="step-condition" value="' +
            escapeAutomationHtml(step.condition || '') +
            '" placeholder="Continue only if output contains this text"><div style="font-size:10px;color:var(--kage-text-secondary);margin-top:2px;">Stops the automation if the previous output doesn\'t match.</div>';
    else if (t === 'script')
        fields =
            '<div class="step-script-container" data-script="' +
            escapeAutomationHtml(step.script) +
            '"></div>';
    return (
        '<div class="auto-step" data-step="' +
        si +
        '"><div class="auto-step-top"><span class="auto-step-num">' +
        (si + 1) +
        '.</span><select class="auto-step-type">' +
        tOpts +
        '</select><span style="flex:1"></span><button class="auto-step-btn auto-step-up"' +
        (si === 0 ? ' disabled' : '') +
        '>↑</button><button class="auto-step-btn auto-step-down"' +
        (si === total - 1 ? ' disabled' : '') +
        '>↓</button><button class="auto-step-btn auto-step-remove">✕</button></div><div class="auto-step-fields">' +
        fields +
        '</div></div>'
    );
}

export function escapeAutomationHtml(s) {
    return (s || '').replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');
}
