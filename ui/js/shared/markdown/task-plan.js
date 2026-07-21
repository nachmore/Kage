import { t } from '../i18n.js';

export function keepLastTaskPlan(markdown) {
    // Strip any leading "ack" from steering response that may have leaked into the stream
    if (markdown.startsWith('ack')) {
        markdown = markdown.slice(3);
    }
    // Cheap short-circuit: this runs on EVERY streaming render over the
    // full accumulated text, and the regex matchAll passes below are the
    // expensive part. No taskplan fence → nothing to do.
    if (!markdown.includes('```taskplan')) return markdown;
    // Find all complete taskplan blocks (handle cases where block isn't at line start,
    // e.g. "ack```taskplan" when steering response leaks into the stream)
    const blockPattern = /```taskplan\r?\n[\s\S]*?\n```/g;
    const blocks = [...markdown.matchAll(blockPattern)];

    // Remove all but the last taskplan block
    if (blocks.length > 1) {
        for (let i = blocks.length - 2; i >= 0; i--) {
            markdown =
                markdown.slice(0, blocks[i].index) +
                markdown.slice(blocks[i].index + blocks[i][0].length);
        }
    }

    // Now apply inline step markers to the remaining taskplan block
    const remaining = [...markdown.matchAll(/```taskplan\r?\n[\s\S]*?\n```/g)];
    if (remaining.length !== 1) return markdown;

    const block = remaining[0];

    // Parse ALL step update markers from the raw markdown.
    // Use a non-greedy match for detail to handle cases where active+done are on the same line:
    //   `[step 1 active]` Launching...`[step 1 done]` Word launched
    // We need to find each `[step N status]` individually.
    const stepPattern = /`\[step (\d+) (\w+)\]`/g;
    const updates = new Map();
    let m;
    while ((m = stepPattern.exec(markdown)) !== null) {
        const stepNum = parseInt(m[1], 10);
        const status = m[2];
        // Extract detail: text between this marker's closing backtick and the next marker or end of line
        const afterMarker = markdown.slice(m.index + m[0].length);
        const detailMatch = afterMarker.match(/^\s*([^`\n\r]*)/);
        const detail = detailMatch ? detailMatch[1].trim() : '';
        // Later updates for the same step override earlier ones
        updates.set(stepNum, { status, detail });
    }

    if (updates.size === 0) return markdown;

    // Parse the taskplan block lines and apply updates
    const blockText = block[0];
    const lines = blockText.split(/\r?\n/);
    const header = lines[0]; // ```taskplan
    const footer = lines[lines.length - 1]; // ```
    const taskLines = lines.slice(1, -1);

    const updatedLines = taskLines.map((line, i) => {
        const stepNum = i + 1;
        const update = updates.get(stepNum);
        if (!update) return line;

        const lineMatch = line.match(/^\[(\w+)\]\s*(.+?)(?:\s*\|\s*(.*))?$/);
        if (!lineMatch) return line;

        const description = lineMatch[2].trim();
        const detail = update.detail || lineMatch[3] || '';
        return `[${update.status}] ${description}${detail ? ' | ' + detail : ''}`;
    });

    const newBlock = header + '\n' + updatedLines.join('\n') + '\n' + footer;

    // Replace the block in the markdown
    let result =
        markdown.slice(0, block.index) + newBlock + markdown.slice(block.index + block[0].length);

    // Strip the inline step markers from the output (handle same-line cases too)
    result = result.replace(/`\[step \d+ \w+\]`\s*[^`\n\r]*/g, '');
    result = result.replace(/\n{3,}/g, '\n\n');

    return result;
}

/**
 * Deduplicate taskplan blocks — keep only the last one which has the latest state.
 * Earlier taskplan blocks are removed from the DOM. This handles the case where
 * the agent outputs updated taskplan blocks throughout the response.
 */
export function deduplicateTaskPlans(container) {
    const plans = container.querySelectorAll('.taskplan');
    if (plans.length <= 1) return;
    for (let i = 0; i < plans.length - 1; i++) {
        plans[i].remove();
    }
}

/**
 * Parse a taskplan text block into structured task objects.
 * Format: [status] description | optional detail
 * @param {string} text - Raw taskplan text content
 * @returns {Array<{status: string, description: string, detail: string}>}
 */
export function parseTaskPlan(text) {
    return text
        .trim()
        .split('\n')
        .filter((l) => l.trim())
        .map((line) => {
            const match = line.match(/^\[(\w+)\]\s*(.+?)(?:\s*\|\s*(.*))?$/);
            if (!match) return null;
            return {
                status: match[1],
                description: match[2].trim(),
                detail: match[3]?.trim() || '',
            };
        })
        .filter(Boolean);
}

/**
 * Render a taskplan code block as a visual progress tracker.
 * Can be called directly with a container element, or used internally
 * by the markdown renderer when it encounters a ```taskplan block.
 *
 * @param {HTMLElement} codeBlock - The code element containing taskplan text
 * @param {HTMLElement} pre - The parent pre element to replace
 */
export function renderTaskPlan(codeBlock, pre) {
    const tasks = parseTaskPlan(codeBlock.textContent);
    if (tasks.length === 0) return;

    const wrapper = createTaskPlanElement(tasks);
    pre.parentNode.insertBefore(wrapper, pre);
    pre.remove();
}

/**
 * Create a taskplan DOM element from parsed tasks.
 * Usable standalone outside of the markdown renderer.
 * @param {Array<{status: string, description: string, detail: string}>} tasks
 * @returns {HTMLElement}
 */
export function createTaskPlanElement(tasks) {
    const wrapper = document.createElement('div');
    wrapper.className = 'taskplan';
    wrapper.setAttribute('role', 'list');
    wrapper.setAttribute('aria-label', t('shared.markdown.task_plan.aria'));

    const doneCount = tasks.filter((t) => t.status === 'done').length;
    const totalCount = tasks.length;
    wrapper.dataset.progress = `${doneCount}/${totalCount}`;

    tasks.forEach((task, i) => {
        const item = document.createElement('div');
        item.className = `taskplan-item taskplan-${task.status}`;
        item.setAttribute('role', 'listitem');

        // Done items with detail are collapsible (collapsed by default)
        const isCollapsible = task.status === 'done' && task.detail;
        if (isCollapsible) {
            item.classList.add('taskplan-collapsible', 'taskplan-collapsed');
        }

        const isLast = i === tasks.length - 1;

        item.innerHTML = `
            <div class="taskplan-indicator">
                <div class="taskplan-icon">${_taskIcon(task.status)}</div>
                ${!isLast ? '<div class="taskplan-connector"></div>' : ''}
            </div>
            <div class="taskplan-content">
                <div class="taskplan-title">${isCollapsible ? '<span class="taskplan-chevron">›</span> ' : ''}${_escapeTaskText(task.description)}</div>
                ${task.cancelled ? '<div class="taskplan-cancelled">Cancelled by user</div>' : ''}
                ${task.detail ? `<div class="taskplan-detail">${_escapeTaskText(task.detail)}</div>` : ''}
            </div>
        `;

        // Click to expand/collapse done items
        if (isCollapsible) {
            item.addEventListener('click', () => {
                item.classList.toggle('taskplan-collapsed');
            });
        }

        wrapper.appendChild(item);
    });

    return wrapper;
}

function _taskIcon(status) {
    switch (status) {
        case 'done':
            return '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="20 6 9 17 4 12"></polyline></svg>';
        case 'active':
            return '<div class="taskplan-spinner"></div>';
        case 'error':
            return '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>';
        case 'stopped':
            return '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><rect x="6" y="6" width="12" height="12" rx="1"></rect></svg>';
        default:
            return '<div class="taskplan-dot"></div>';
    }
}

// escapeHtml used by renderTaskPlan
function _escapeTaskText(str) {
    return str
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
}
