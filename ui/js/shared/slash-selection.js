/**
 * Shared logic for ACP "selection"-type slash commands (`/agent`, `/model`,
 * `/prompts`, …). This is the WINDOW-AGNOSTIC layer: option extraction, the
 * submit arg-shape, and the "is this actually a pickable list?" decision.
 *
 * Each window keeps its own *rendering* (floating paints into the suggestions
 * dropdown; chat paints inline in the transcript) — only the data handling
 * lives here, because that's where every bug was:
 *
 *   - Options come from the structured `result.data`, NOT from line-parsing
 *     `result.message`. The agent returns `data.agents[]` / `data.models[]`
 *     (and `data.current`); the message is just a pretty-printed echo.
 *   - The SET arg-shape is `{ <command>Name: value }` (e.g. `{ agentName }`,
 *     `{ modelName }`). Passing `{ input: value }` does NOT switch — the agent
 *     treats it as a re-list and the current value never changes.
 *   - Some "selection" commands return no list at all (`/feedback` opens a
 *     browser, `/prompts` defers to a menu, `/effort` errors when the model
 *     doesn't support it). Those must fall back to showing the message, not
 *     render an empty picker.
 *
 * Shapes verified against live kiro-cli via scripts/probe_slash.py. Re-run that
 * probe if the agent's contract is suspected to have changed.
 */

/**
 * Normalise one agent/model/option entry from `result.data` into the common
 * `{ label, value, description, current }` shape the renderers consume.
 */
function _normalizeEntry(entry, currentValue) {
    if (entry == null) return null;
    if (typeof entry === 'string') {
        return { label: entry, value: entry, description: '', current: entry === currentValue };
    }
    // Agents use `name`; models use `id` + `displayName`; generic options may
    // use `value`/`label`. Cover all without assuming a single schema.
    const value = entry.id ?? entry.name ?? entry.value ?? entry.label;
    const label = entry.displayName ?? entry.label ?? entry.name ?? String(value ?? '');
    const description = entry.description ?? '';
    return {
        label: String(label),
        value: value == null ? String(label) : String(value),
        description: String(description),
        current: value != null && currentValue != null && String(value) === String(currentValue),
    };
}

/**
 * Pull the option list out of a `commands/execute` reply.
 *
 * @param {object} result - the `result` object from execute_slash_command
 * @returns {{ options: Array<{label,value,description,current}>, current: string|null }}
 *          `options` is empty when the reply carries no structured list.
 */
export function extractSelectionOptions(result) {
    const data = result?.data;
    if (!data || typeof data !== 'object') return { options: [], current: null };

    const current = data.current ?? null;

    // Known structured lists, in priority order, then a generic `options`.
    const rawList =
        (Array.isArray(data.agents) && data.agents) ||
        (Array.isArray(data.models) && data.models) ||
        (Array.isArray(data.prompts) && data.prompts) ||
        (Array.isArray(data.options) && data.options) ||
        null;

    if (!rawList) return { options: [], current };

    const options = rawList.map((e) => _normalizeEntry(e, current)).filter(Boolean);
    return { options, current };
}

/**
 * Build the SET argument object for a selection submit.
 * Convention (verified against the agent): `{ <command>Name: value }`.
 *
 * @param {string} command - bare command name without leading slash (e.g. "agent")
 * @param {string} value - the chosen option's value
 */
export function selectionSubmitArgs(command, value) {
    return { [command + 'Name']: value };
}

/**
 * Run a selection command's INITIAL no-arg execute and classify the reply.
 *
 * Returns one of:
 *   { kind: 'options', command, options, current }  — render a picker
 *   { kind: 'message', text }                        — show the reply text
 *
 * The caller decides how to render each kind for its window. This never
 * throws for an agent-level failure — a `success:false` reply with a message
 * (e.g. /effort on an unsupported model) becomes a 'message' result.
 *
 * @param {Function} invoke - Tauri invoke
 * @param {string|null} sessionId
 * @param {string} command - bare command name (no leading slash)
 */
export async function loadSelection(invoke, sessionId, command) {
    const result = await invoke('execute_slash_command', {
        sessionId,
        command,
        args: null,
    });
    const { options, current } = extractSelectionOptions(result);
    if (options.length > 0) {
        return { kind: 'options', command, options, current };
    }
    // No structured list — surface whatever the agent said.
    const text = result?.message || '';
    return { kind: 'message', text };
}

/**
 * Submit a chosen option back to the agent and return the reply's display text.
 *
 * @param {Function} invoke - Tauri invoke
 * @param {string|null} sessionId
 * @param {string} command - bare command name (no leading slash)
 * @param {string} value - chosen option value
 * @returns {Promise<string>} message to display (may be empty)
 */
export async function submitSelection(invoke, sessionId, command, value) {
    const result = await invoke('execute_slash_command', {
        sessionId,
        command,
        args: selectionSubmitArgs(command, value),
    });
    return result?.message || '';
}
