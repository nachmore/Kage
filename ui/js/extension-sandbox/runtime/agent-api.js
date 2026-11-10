/**
 * `context.agent` for extensions whose window offers `agent.background`
 * (they hold `agent_background`): ask the user's agent for help in the
 * background, with no chat (docs/design/EXTENSION_AGENT_CALLS.md §3.4).
 * A thin wrapper over the `agent_call_*` commands. The host injects the
 * extension's id, and Rust checks every gate on every call.
 * Self-contained (no imports): the host inlines this file into the
 * sandbox runtime (RUNTIME_SOURCE_PATHS in extension-sandbox-host.js).
 */

/** The error key that allows one automatic retry (§3.4 step 5). */
const AGENT_KEY_USER_ACTIVITY = 'errors.agent_call.cancelled_user_activity';
/** The error key an aborted `signal` rejects with (§3.4 step 6). */
const AGENT_KEY_CANCELLED = 'errors.agent_call.cancelled_by_extension';

/** `maxResponseChars` and `timeoutMs` are clamped into these (§3.4). */
const AGENT_RESPONSE_CHARS = Object.freeze({ min: 1, max: 32_000 });
const AGENT_TIMEOUT_MS = Object.freeze({ min: 5_000, max: 180_000 });

/** The rejection of a call whose `signal` was aborted. */
function agentCancelled() {
    const err = new Error('The extension cancelled the request.');
    err.kind = 'cancelled';
    err.key = AGENT_KEY_CANCELLED;
    return err;
}

/** An argument error, raised without reaching the host. */
function agentBadInput(message) {
    const err = new TypeError(message);
    err.kind = 'invalid_input';
    return err;
}

/** A finite number rounded and clamped into `range`, else a TypeError. */
function agentClamp(value, range, name) {
    if (typeof value !== 'number' || !Number.isFinite(value)) {
        throw agentBadInput(`agent.complete: ${name} must be a number`);
    }
    return Math.min(range.max, Math.max(range.min, Math.round(value)));
}

/**
 * The request a `build` returned, as `agent_call_send` takes it: snake_case
 * names, only the fields given, the two numbers clamped.
 *
 * @param {object} req - `{ label?, instructions, input?, schema?,
 *   maxResponseChars?, timeoutMs? }`
 */
export function agentWireRequest(req) {
    if (!req || typeof req !== 'object' || Array.isArray(req)) {
        throw agentBadInput('agent.complete: the request must be an object');
    }
    if (typeof req.instructions !== 'string') {
        throw agentBadInput('agent.complete: request.instructions must be a string');
    }
    if (req.label !== undefined && req.label !== null && typeof req.label !== 'string') {
        throw agentBadInput('agent.complete: request.label must be a string');
    }
    const out = { instructions: req.instructions };
    if (typeof req.label === 'string') out.label = req.label;
    if (req.input !== undefined) out.input = req.input;
    if (req.schema !== undefined && req.schema !== null) out.schema = req.schema;
    const maxChars = req.maxResponseChars ?? req.max_response_chars;
    if (maxChars !== undefined && maxChars !== null) {
        out.max_response_chars = agentClamp(maxChars, AGENT_RESPONSE_CHARS, 'maxResponseChars');
    }
    const timeoutMs = req.timeoutMs ?? req.timeout_ms;
    if (timeoutMs !== undefined && timeoutMs !== null) {
        out.timeout_ms = agentClamp(timeoutMs, AGENT_TIMEOUT_MS, 'timeoutMs');
    }
    return out;
}

/** `agent_call_send`'s result, as `complete()` resolves it. */
function agentResult(r) {
    return {
        callId: r?.call_id ?? null,
        text: typeof r?.text === 'string' ? r.text : '',
        json: r?.json ?? null,
        promptChars: r?.prompt_chars ?? 0,
        responseChars: r?.response_chars ?? 0,
        durationMs: r?.duration_ms ?? 0,
        redactions: r?.redactions ?? 0,
    };
}

/**
 * One pass of `complete()`: reserve, wait for the turn, build, send.
 * An abort at any stage cancels the call and rejects at once.
 *
 * @param {Function} invoke
 * @param {Function | object} build - a builder, or the wire request
 *   (already checked) when the extension passed an object
 * @param {object} reserveArgs
 * @param {AbortSignal} [signal]
 */
async function agentAttempt(invoke, build, reserveArgs, signal) {
    if (signal?.aborted) throw agentCancelled();
    let callId = null;
    let aborted = false;
    // Set once the call is over on the host's side, so an abort after it
    // sends nothing.
    let settled = false;
    const cancel = () => {
        if (!callId || settled) return;
        settled = true;
        invoke('agent_call_cancel', { args: { call_id: callId } }).catch(() => {});
    };
    let rejectAbort = () => {};
    const abortPromise = new Promise((_, reject) => {
        rejectAbort = reject;
    });
    abortPromise.catch(() => {});
    const onAbort = () => {
        aborted = true;
        cancel();
        rejectAbort(agentCancelled());
    };
    const listens = typeof signal?.addEventListener === 'function';
    if (listens) signal.addEventListener('abort', onAbort, { once: true });
    const race = (p) => (listens ? Promise.race([p, abortPromise]) : p);
    try {
        await race(
            invoke('agent_call_reserve', { args: reserveArgs }).then((r) => {
                if (typeof r?.call_id !== 'string' || !r.call_id) {
                    throw new Error('agent_call_reserve returned no call id');
                }
                callId = r.call_id;
                // Reserved after the abort: give it back at once.
                if (aborted) cancel();
                return r;
            })
        );
        try {
            await race(invoke('agent_call_ready', { args: { call_id: callId } }));
        } catch (err) {
            // A refused or cancelled turn is over on the host's side.
            settled = true;
            throw err;
        }
        let request = build;
        if (typeof build === 'function') {
            try {
                const built = await race(Promise.resolve(build()));
                if (built === null || built === undefined) {
                    // Nothing to send: give the turn back unused.
                    cancel();
                    return null;
                }
                request = agentWireRequest(built);
            } catch (err) {
                cancel();
                throw err;
            }
        }
        try {
            const result = await race(
                invoke('agent_call_send', { args: { call_id: callId, request } })
            );
            return agentResult(result);
        } finally {
            // Answered or refused, the host has finished this call.
            settled = true;
        }
    } finally {
        if (listens) signal.removeEventListener('abort', onAbort);
    }
}

/**
 * @param {{ invoke: (command: string, args?: object) => Promise<any> }} deps
 * @returns the frozen `context.agent` object
 */
export function createAgentApi({ invoke }) {
    return Object.freeze({
        /**
         * Ask the agent once. `build` is the request, or a function called
         * when Kage gives this extension its turn, so it reads fresh data
         * then; returning null gives the turn back and resolves null.
         * Resolves `{ callId, text, json, promptChars, responseChars,
         * durationMs, redactions }`. Rejects with an Error carrying `kind`
         * and `key`.
         *
         * @param {object | (() => object | null | Promise<object | null>)} build
         * @param {{ purpose: string, when?: 'idle' | 'now', dedupeKey?: string,
         *   signal?: AbortSignal, retryOnActivity?: boolean }} options
         */
        async complete(build, options = {}) {
            if (typeof build !== 'function' && (!build || typeof build !== 'object')) {
                throw agentBadInput(
                    'agent.complete: build must be a request object or a function that returns one'
                );
            }
            const { purpose, when, dedupeKey, signal, retryOnActivity = true } = options || {};
            if (typeof purpose !== 'string' || !purpose) {
                throw agentBadInput(
                    "agent.complete: options.purpose must name one of the manifest's agentBackground.purposes"
                );
            }
            if (when !== undefined && when !== 'idle' && when !== 'now') {
                throw agentBadInput("agent.complete: options.when must be 'idle' or 'now'");
            }
            if (dedupeKey !== undefined && dedupeKey !== null && typeof dedupeKey !== 'string') {
                throw agentBadInput('agent.complete: options.dedupeKey must be a string');
            }
            // A request object is checked before it takes a turn.
            const request = typeof build === 'function' ? build : agentWireRequest(build);
            const reserveArgs = { purpose, when: when ?? 'idle' };
            if (typeof dedupeKey === 'string') reserveArgs.dedupe_key = dedupeKey;
            let retries = retryOnActivity === false ? 0 : 1;
            for (;;) {
                try {
                    return await agentAttempt(invoke, request, reserveArgs, signal);
                } catch (err) {
                    // Stopped because the user started using Kage: try
                    // once more, in a new reservation.
                    if (retries > 0 && err?.key === AGENT_KEY_USER_ACTIVITY && !signal?.aborted) {
                        retries -= 1;
                        continue;
                    }
                    throw err;
                }
            }
        },

        /**
         * This extension's own state: `{ available, access, connected,
         * waiting, running, usage, next_start_at, paused_until, machine_tag }`.
         */
        status() {
            return invoke('agent_call_status', {});
        },
    });
}
