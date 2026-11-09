/**
 * `context.sync` for extensions whose window offers `sync.records` and
 * whose grant includes `sync` (docs/design/SYNC.md §12.6). Self-contained:
 * the host inlines it into the sandbox runtime.
 *
 * Skeleton from the contracts commit: the object reports sync as not
 * available and every call is refused. WP-S5 writes it on top of the
 * `ext_sync_*` commands and the host's `sync-changes` port message, and
 * adds it to the runtime.
 *
 * @param {{ invoke: Function, onPortMessage: Function }} _deps
 */
export function createSyncApi(_deps) {
    const refuse = () => {
        const err = new Error('Kage sync is not available');
        err.kind = 'sync_unavailable';
        return Promise.reject(err);
    };
    return Object.freeze({
        available: false,
        status: refuse,
        push: refuse,
        pull: refuse,
        ack: refuse,
        onChanges: () => () => {},
    });
}
