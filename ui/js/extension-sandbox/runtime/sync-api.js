/**
 * `context.sync` for extensions whose window offers `sync.records` and
 * whose grant includes `sync` (docs/design/SYNC.md §12.6). Self-contained
 * (no imports): the host inlines it into the sandbox runtime.
 *
 * The extension keeps its own storage as the primary copy. It pushes the
 * records it wrote, pulls the merged changes, applies them, and acks.
 * The host merges and keeps the base, so the extension stores no version
 * tokens. The host injects the extension's id into every call.
 */

/** Why records changed, as `onChanges` reports it. */
export const SYNC_REASONS = Object.freeze(['remote', 'enabled', 'reset', 'purged']);

/** `onChanges` handlers hear of a burst of changes this long after it ends. */
export const SYNC_CHANGES_DEBOUNCE_MS = 500;

/** §12.7: one push, one pull page. */
export const SYNC_MAX_PUSH_RECORDS = 500;
export const SYNC_MAX_PULL = 500;

const SYNC_COLLECTION_RE = /^[a-z][a-z0-9_]{0,31}$/;
const SYNC_RECORD_ID_RE = /^[A-Za-z0-9_.:-]{1,128}$/;
const SYNC_CURSOR_RE = /^\d{1,20}$/;

/** An argument error, rejected without reaching the host. */
function syncBadInput(message) {
    const err = new TypeError(message);
    err.kind = 'invalid_input';
    return err;
}

/**
 * @param {object} deps
 * @param {(command: string, args: object) => Promise<any>} deps.invoke -
 *   the runtime's `invoke` (through the host's permission check)
 * @param {(handler: (msg: object) => void) => (() => void)} deps.onPortMessage -
 *   subscribe to the host's `sync-changes` port messages
 * @param {(level: string, ...args: unknown[]) => void} [deps.log]
 * @param {Function} [deps.setTimer] - default `setTimeout`
 * @param {Function} [deps.clearTimer] - default `clearTimeout`
 * @returns the frozen `context.sync` object
 */
export function createSyncApi({
    invoke,
    onPortMessage,
    log = () => {},
    setTimer = (fn, ms) => setTimeout(fn, ms),
    clearTimer = (t) => clearTimeout(t),
}) {
    let available = false;
    const listeners = new Set();
    /** The burst waiting for the debounce: collections and its reason. */
    let pending = null;
    let timer = null;

    // `purged` wins over `reset`, which wins over `enabled` and `remote`:
    // the strongest reason in a burst is the one that tells the extension
    // most about what to do.
    const rank = (r) => SYNC_REASONS.indexOf(r);

    const flush = () => {
        timer = null;
        const burst = pending;
        pending = null;
        if (!burst) return;
        const event = Object.freeze({
            collections: Object.freeze([...burst.collections].sort()),
            reason: burst.reason,
        });
        for (const entry of [...listeners]) {
            try {
                entry.fn(event);
            } catch (e) {
                log('warn', `sync.onChanges listener failed: ${e?.message || e}`);
            }
        }
    };

    onPortMessage((msg) => {
        if (!msg || msg.type !== 'sync-changes' || !SYNC_REASONS.includes(msg.reason)) return;
        if (msg.reason === 'enabled') available = true;
        if (msg.reason === 'purged') available = false;
        const collections = Array.isArray(msg.collections)
            ? msg.collections.filter((c) => typeof c === 'string' && SYNC_COLLECTION_RE.test(c))
            : [];
        if (!pending) pending = { collections: new Set(), reason: msg.reason };
        for (const c of collections) pending.collections.add(c);
        if (rank(msg.reason) > rank(pending.reason)) pending.reason = msg.reason;
        if (timer !== null) clearTimer(timer);
        timer = setTimer(flush, SYNC_CHANGES_DEBOUNCE_MS);
    });

    const noteOff = (result) => {
        if (result && result.off === true) available = false;
        return result;
    };

    const api = {
        /** Kage sync is on and this extension's data syncs here (a snapshot). */
        get available() {
            return available;
        },

        /** `{ available, paused, needsInitialPush, deviceTag, devices, usedBytes, maxBytes, lastSyncAt }` */
        async status() {
            const s = await invoke('ext_sync_status', {});
            available = !!s?.available;
            return s;
        },

        /**
         * Push records of `collection` after the extension's own write, and
         * the ids it deleted. → `{ accepted, unchanged, rejected, off }`
         */
        async push(collection, records, { deleted = [], initial = false } = {}) {
            if (typeof collection !== 'string' || !SYNC_COLLECTION_RE.test(collection)) {
                throw syncBadInput('sync.push: collection must be a declared collection name');
            }
            if (!Array.isArray(records) || !Array.isArray(deleted)) {
                throw syncBadInput('sync.push: records and deleted must be arrays');
            }
            if (records.length + deleted.length > SYNC_MAX_PUSH_RECORDS) {
                throw syncBadInput(
                    `sync.push: at most ${SYNC_MAX_PUSH_RECORDS} records and deletes per push`
                );
            }
            for (const r of records) {
                if (!r || typeof r !== 'object' || Array.isArray(r)) {
                    throw syncBadInput('sync.push: each record must be an object');
                }
                if (typeof r.id !== 'string' || !SYNC_RECORD_ID_RE.test(r.id)) {
                    throw syncBadInput(
                        'sync.push: each record needs an id of 1-128 letters, digits or _.:-'
                    );
                }
            }
            for (const id of deleted) {
                if (typeof id !== 'string' || !SYNC_RECORD_ID_RE.test(id)) {
                    throw syncBadInput('sync.push: deleted must be a list of record ids');
                }
            }
            return noteOff(
                await invoke('ext_sync_push', {
                    collection,
                    records,
                    deleted,
                    initial: initial === true,
                })
            );
        },

        /** Up to `max` merged changes since the last ack. → `{ cursor, more, changes }` */
        async pull({ max = SYNC_MAX_PULL } = {}) {
            if (!Number.isInteger(max) || max < 1 || max > SYNC_MAX_PULL) {
                throw syncBadInput(
                    `sync.pull: max must be a whole number from 1 to ${SYNC_MAX_PULL}`
                );
            }
            return invoke('ext_sync_pull', { max });
        },

        /** Everything up to `cursor` (from `pull`) is applied. */
        async ack(cursor) {
            if (typeof cursor !== 'string' || !SYNC_CURSOR_RE.test(cursor)) {
                throw syncBadInput('sync.ack: cursor must be the one pull returned');
            }
            await invoke('ext_sync_ack', { cursor });
        },

        /**
         * Hear of changes to pull: `fn({ collections, reason })`, debounced
         * 500 ms. Returns an unsubscribe function.
         */
        onChanges(fn) {
            if (typeof fn !== 'function') throw syncBadInput('sync.onChanges needs a function');
            const entry = { fn };
            listeners.add(entry);
            return () => {
                listeners.delete(entry);
            };
        },
    };

    // `available` is right soon after start, without the extension asking.
    api.status().catch(() => {});
    return Object.freeze(api);
}
