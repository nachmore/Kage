/**
 * Tool calls of a reopened session, rebuilt from the agent's own session
 * log — no DOM, nothing persisted by Kage.
 *
 * `load_session` returns kiro-cli's session log (`~/.kiro/sessions/cli/
 * <id>.jsonl`) line by line: an `AssistantMessage`'s content holds its
 * tool uses (`{ kind: "toolUse", data: { toolUseId, name, input } }`), the
 * following `ToolResults` line their results (`{ kind: "toolResult", data:
 * { toolUseId, status: "success" | "error", content: [{ kind: "text" |
 * "json", data }] } }`) plus `tool_kinds` — which MCP server or built-in
 * each result came from, projected by the backend from kiro's `results`
 * map (src/commands/sessions/crud.rs).
 *
 * Each tool use + result becomes the ACP `tool_call` / `tool_call_update`
 * pair the agent streamed live (the same title, kind, `rawInput` and
 * `rawOutput` shapes kiro sends), fed through `processToolCallUpdate`
 * into a per-response state — so a reopened response gets exactly the
 * chips, records and caps a live one does. MCP calls get kiro's live title
 * `Running: @<server>/<tool>` only when the log names the server, and are
 * then classified by the same title rules (tool-calls.js); a shell call
 * is kind `execute` and can never read as MCP.
 *
 * Why the log and not kiro's `session/load` replay of the same calls: the
 * log is what the chat window already reads to draw the transcript, it
 * works without an agent connection (read-only, locked or offline
 * sessions), and it keeps the replay where it is — dropped by the backend
 * (`is_loading_session`) so a reload never spills old calls into the
 * floating window or the streaming accumulators.
 */

import { settleShellCalls, shellOutputSize, SHELL_RESPONSE_OUTPUT_CAP } from './shell-commands.js';
import { processToolCallUpdate } from './streaming-utils.js';
import { kindForToolName, settleToolCalls, toolCallSize } from './tool-calls.js';

/**
 * Output kept across all the tool calls of one reopened session, in
 * characters. Allocated newest response first, so the turns the user sees
 * on opening keep their output.
 */
export const HISTORY_SESSION_OUTPUT_CAP = 4 * 1024 * 1024;

// kiro-cli's built-in tool variants (`results[id].tool.kind.BuiltIn`), for
// a tool use whose name we don't know.
const BUILTIN_KINDS = Object.freeze({
    ExecuteCmd: 'execute',
    ExecuteBash: 'execute',
    FileRead: 'read',
    FsRead: 'read',
    FileWrite: 'edit',
    FsWrite: 'edit',
    Grep: 'search',
    Glob: 'search',
    WebSearch: 'search',
    Code: 'search',
    Knowledge: 'search',
    WebFetch: 'fetch',
    Task: 'think',
    Todo: 'think',
    TodoList: 'think',
});

function toolUseOf(item) {
    if (item?.kind !== 'toolUse') return null;
    const d = item.data;
    if (!d || typeof d !== 'object' || typeof d.toolUseId !== 'string' || !d.toolUseId) return null;
    return d;
}

function toolResultOf(item) {
    if (item?.kind !== 'toolResult') return null;
    const d = item.data;
    if (!d || typeof d !== 'object' || typeof d.toolUseId !== 'string' || !d.toolUseId) return null;
    return d;
}

/**
 * The ACP updates the agent streamed for one logged tool use: a
 * `tool_call` (title, kind, rawInput) and, once it has a result, a
 * `tool_call_update` (status, rawOutput). `toolKind` is the backend's
 * `tool_kinds` entry for it, if any.
 */
export function historyToolUpdates(use, result, toolKind) {
    const name = typeof use.name === 'string' && use.name ? use.name : '';
    let title = name || 'tool';
    let kind;
    const server = toolKind?.mcp_server;
    const tool = toolKind?.mcp_tool;
    if (typeof server === 'string' && server && typeof tool === 'string' && tool) {
        // kiro's live title for an MCP call; tool-calls.js re-checks it.
        title = `Running: @${server}/${tool}`;
        kind = 'other';
    } else {
        kind =
            kindForToolName(name) ||
            (typeof toolKind?.builtin === 'string' && BUILTIN_KINDS[toolKind.builtin]) ||
            'other';
    }
    const updates = [
        {
            sessionUpdate: 'tool_call',
            toolCallId: use.toolUseId,
            title,
            kind,
            status: 'pending',
            rawInput: use.input,
        },
    ];
    if (result) {
        const items = [];
        for (const c of Array.isArray(result.content) ? result.content : []) {
            if (c?.kind === 'text' && typeof c.data === 'string') items.push({ Text: c.data });
            else if (c?.kind === 'json' && c.data !== undefined) items.push({ Json: c.data });
        }
        const status =
            result.status === 'success' ? 'completed' : result.status === 'error' ? 'failed' : null;
        const update = { sessionUpdate: 'tool_call_update', toolCallId: use.toolUseId };
        if (status) update.status = status;
        if (items.length) update.rawOutput = { items };
        updates.push(update);
    }
    return updates;
}

function usedSize(toolUsages) {
    let n = 0;
    for (const u of toolUsages) {
        if (u?.shell) n += shellOutputSize(u.shell);
        else if (u?.call) n += toolCallSize(u.call);
    }
    return n;
}

/**
 * Rebuild one response's tool state — `{ toolUsages, toolSources }`, as a
 * live turn leaves it — from its logged tool uses and results, within
 * `outputCap` characters of output. Calls without a result are settled
 * (status unknown). Returns `{ tools, size }`.
 */
export function buildHistoryTools(uses, results, toolKinds, outputCap = SHELL_RESPONSE_OUTPUT_CAP) {
    const state = {
        toolUsages: [],
        toolSources: [],
        _toolCallIds: new Set(),
        _sourceDomains: new Set(),
        _outputCap: Math.max(0, outputCap),
    };
    for (const use of uses) {
        const result = results.get(use.toolUseId) || null;
        const kind = toolKinds.get(use.toolUseId) || null;
        for (const update of historyToolUpdates(use, result, kind)) {
            const { call } = processToolCallUpdate({ payload: { params: { update } } }, state);
            // The log says it's one of the agent's built-ins: it groups by
            // kind, as its descriptively titled live call did (toolChipGroup).
            if (call && typeof kind?.builtin === 'string' && kind.builtin) call.builtin = true;
        }
    }
    settleShellCalls(state.toolUsages);
    settleToolCalls(state.toolUsages);
    return {
        tools: { toolUsages: state.toolUsages, toolSources: state.toolSources },
        size: usedSize(state.toolUsages),
    };
}

/**
 * Collects the logged tool uses of the session's turns while
 * `buildRenderQueue` walks its messages, then attaches each turn's rebuilt
 * tool state to the turn's last assistant item (`item.tools`), where a
 * live turn's chips would be.
 */
export class HistoryToolCollector {
    constructor() {
        this.turns = [];
        this.turn = null;
    }

    /** A user prompt starts a turn; `steering` turns show no tools. */
    startTurn(steering = false) {
        this.turn = { steering, uses: [], results: new Map(), kinds: new Map(), item: null };
        this.turns.push(this.turn);
    }

    _current() {
        if (!this.turn) this.startTurn(false);
        return this.turn;
    }

    /** Tool uses in an `AssistantMessage`'s content. */
    addAssistant(content) {
        const turn = this._current();
        for (const item of Array.isArray(content) ? content : []) {
            const use = toolUseOf(item);
            if (use && !turn.uses.some((u) => u.toolUseId === use.toolUseId)) turn.uses.push(use);
        }
    }

    /** A `ToolResults` message: its results and the backend's `tool_kinds`. */
    addResults(msg) {
        const turn = this._current();
        for (const item of Array.isArray(msg?.content) ? msg.content : []) {
            const r = toolResultOf(item);
            if (r) turn.results.set(r.toolUseId, r);
        }
        const kinds = msg?.tool_kinds;
        if (kinds && typeof kinds === 'object') {
            for (const [id, k] of Object.entries(kinds)) {
                if (k && typeof k === 'object') turn.kinds.set(id, k);
            }
        }
    }

    /** The turn's assistant item that its chips go under (the last one). */
    setItem(item) {
        this._current().item = item;
    }

    /** Whether the current turn has tool uses but no assistant item yet. */
    needsItem() {
        const turn = this.turn;
        return !!turn && !turn.steering && turn.uses.length > 0 && !turn.item;
    }

    /**
     * Rebuild every turn's tools, newest first, within `sessionCap`
     * characters of output in all; attach them to the turns' items.
     */
    finish(sessionCap = HISTORY_SESSION_OUTPUT_CAP) {
        let left = Math.max(0, sessionCap);
        for (let i = this.turns.length - 1; i >= 0; i--) {
            const turn = this.turns[i];
            if (turn.steering || !turn.uses.length || !turn.item) continue;
            const cap = Math.min(SHELL_RESPONSE_OUTPUT_CAP, left);
            const { tools, size } = buildHistoryTools(turn.uses, turn.results, turn.kinds, cap);
            left = Math.max(0, left - size);
            turn.item.tools = tools;
        }
        this.turns = [];
        this.turn = null;
    }
}
