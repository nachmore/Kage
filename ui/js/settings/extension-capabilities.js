import { t } from '../shared/i18n.js';
import { escapeAttr, escapeHtml } from '../shared/tool-utils.js';

const CAPABILITY_ICONS = Object.freeze({
    storage: '💾',
    clipboard: '📋',
    urls: '🔗',
    launch: '🚀',
    network: '📡',
    oauth: '🔐',
    filesystem: '📂',
    window: '🪟',
    windows: '🧿',
    notifications: '🔔',
    calendar: '📅',
    session: '💬',
    agent: '🤖',
    activity: '📊',
    automation: '⚡',
    tts: '🔈',
});

export function renderCapabilityBadges(capabilities, legacy) {
    if (!Array.isArray(capabilities) || capabilities.length === 0) {
        return `<div class="ext-capabilities ext-capabilities-none" title="${escapeAttr(t('settings.manager.cap.none.title'))}">${t('settings.manager.cap.none')}</div>`;
    }
    const pills = capabilities
        .map((capability) => {
            const icon = CAPABILITY_ICONS[capability] || '❓';
            const label = CAPABILITY_ICONS[capability]
                ? t(`settings.manager.cap.${capability}.label`)
                : capability;
            const description = CAPABILITY_ICONS[capability]
                ? t(`settings.manager.cap.${capability}.desc`)
                : t('settings.manager.cap.unknown.desc');
            return `<span class="ext-capability-pill" title="${escapeAttr(description)}">${icon} ${escapeHtml(label)}</span>`;
        })
        .join('');
    const legacyBanner = legacy
        ? `<div class="ext-capabilities-legacy">${t('settings.manager.cap.legacy_warning')}</div>`
        : '';
    return `<div class="ext-capabilities">${pills}</div>${legacyBanner}`;
}
