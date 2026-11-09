/**
 * Field-level saves for the Settings window (docs/design/SYNC.md §9.5).
 *
 * A Settings section writes every field it owns from its DOM. Saving that
 * whole copy would put back the values it loaded, over anything another
 * window or another computer changed since. So the manager runs the
 * section's save() on a copy of what it loaded, finds the paths the user
 * actually changed (`diffPaths`), and writes only those into a fresh copy
 * of the config (`applyPaths`).
 *
 * Lists of items with an id (`sync_id`, or `id` for agents and internal
 * sites) are compared item by item: editing one shortcut doesn't rewrite
 * the others, a removed item is removed by id, a new item is inserted
 * next to its neighbour, and a reorder moves only the items the user saw.
 * Other arrays are one value.
 *
 * Pure functions, no DOM and no Tauri.
 */

/**
 * @typedef {string | { id: string }} Segment
 *   A key of an object, or the item of a list with that id.
 * @typedef {{ path: Segment[], op: 'set' | 'delete' | 'remove' }
 *   | { path: Segment[], op: 'insert', at: number }
 *   | { path: Segment[], op: 'order', ids: string[] }} Change
 *   `set` and `delete` change one value. `remove` drops the list item the
 *   path ends with. `insert` adds the item at index `at` of the edited list
 *   at `path`. `order` puts the listed items of the list at `path` in that
 *   order.
 */

function isPlainObject(v) {
    return v !== null && typeof v === 'object' && !Array.isArray(v);
}

/** The id of a list item: its `sync_id`, else its `id`, else null. */
export function itemId(item) {
    if (!isPlainObject(item)) return null;
    if (typeof item.sync_id === 'string' && item.sync_id) return item.sync_id;
    if (typeof item.id === 'string' && item.id) return item.id;
    return null;
}

/** Deep equality of JSON values. */
export function sameValue(a, b) {
    if (a === b) return true;
    if (Array.isArray(a)) {
        if (!Array.isArray(b) || a.length !== b.length) return false;
        return a.every((v, i) => sameValue(v, b[i]));
    }
    if (isPlainObject(a)) {
        if (!isPlainObject(b)) return false;
        const ka = Object.keys(a).filter((k) => a[k] !== undefined);
        const kb = Object.keys(b).filter((k) => b[k] !== undefined);
        if (ka.length !== kb.length) return false;
        return ka.every((k) => Object.hasOwn(b, k) && sameValue(a[k], b[k]));
    }
    return false;
}

/**
 * Whether two versions of an array are a list of items with ids: every
 * element is an object, every element of `before` has an id, and no id
 * repeats. Elements of `edited` without an id (or with an id `before`
 * doesn't have) are new items.
 */
function isKeyedList(before, edited) {
    if (!Array.isArray(before) || !Array.isArray(edited)) return false;
    if (before.length === 0 && edited.length === 0) return false;
    if (!before.every(isPlainObject) || !edited.every(isPlainObject)) return false;
    const seen = new Set();
    for (const item of before) {
        const id = itemId(item);
        if (!id || seen.has(id)) return false;
        seen.add(id);
    }
    const seenEdited = new Set();
    for (const item of edited) {
        const id = itemId(item);
        if (!id) continue;
        if (seenEdited.has(id)) return false;
        seenEdited.add(id);
    }
    return true;
}

function diffInto(before, edited, path, out) {
    if (isPlainObject(before) && isPlainObject(edited)) {
        const keys = new Set([...Object.keys(before), ...Object.keys(edited)]);
        for (const k of keys) {
            const b = before[k];
            const e = edited[k];
            if (e === undefined && b === undefined) continue;
            if (e === undefined) out.push({ path: [...path, k], op: 'delete' });
            else if (b === undefined) out.push({ path: [...path, k], op: 'set' });
            else diffInto(b, e, [...path, k], out);
        }
        return;
    }
    if (isKeyedList(before, edited)) {
        diffList(before, edited, path, out);
        return;
    }
    if (!sameValue(before, edited)) out.push({ path, op: edited === undefined ? 'delete' : 'set' });
}

function diffList(before, edited, path, out) {
    const byId = new Map(before.map((item) => [itemId(item), item]));
    const editedIds = new Set();
    edited.forEach((item, at) => {
        const id = itemId(item);
        if (id && byId.has(id)) {
            editedIds.add(id);
            diffInto(byId.get(id), item, [...path, { id }], out);
        } else {
            out.push({ path, op: 'insert', at });
        }
    });
    for (const id of byId.keys()) {
        if (!editedIds.has(id)) out.push({ path: [...path, { id }], op: 'remove' });
    }
    const keptBefore = before.map(itemId).filter((id) => editedIds.has(id));
    const keptEdited = edited.map(itemId).filter((id) => id && byId.has(id));
    if (keptBefore.some((id, i) => id !== keptEdited[i])) {
        out.push({ path, op: 'order', ids: keptEdited });
    }
}

/**
 * What the user changed: the paths where `edited` differs from `before`.
 *
 * @param {object} before - the config the section was loaded from
 * @param {object} edited - the same, after the section's save()
 * @returns {Change[]}
 */
export function diffPaths(before, edited) {
    const out = [];
    diffInto(before ?? {}, edited ?? {}, [], out);
    return out;
}

/** The value at `path`, or undefined. */
export function valueAt(root, path) {
    let cur = root;
    for (const seg of path) {
        if (cur == null) return undefined;
        if (typeof seg === 'string') {
            cur = isPlainObject(cur) ? cur[seg] : undefined;
        } else {
            cur = Array.isArray(cur) ? cur.find((x) => itemId(x) === seg.id) : undefined;
        }
    }
    return cur;
}

const clone = (v) => (v === undefined ? undefined : structuredClone(v));

/**
 * Where to insert into `list` (of fresh) the element at index `at` of
 * `editedList`: after its nearest earlier neighbour that `list` has, else
 * at the front when it was first, else at `at` (or the end).
 */
function insertIndex(list, editedList, at) {
    for (let i = at - 1; i >= 0; i--) {
        const id = itemId(editedList[i]);
        if (!id) continue;
        const idx = list.findIndex((x) => itemId(x) === id);
        if (idx >= 0) return idx + 1;
    }
    return Math.min(at, list.length);
}

/**
 * The container `path` names in `fresh`, made where it's missing. A list
 * item `fresh` no longer has (removed elsewhere while the user edited it)
 * comes back whole from `edited`: the edit wins over the delete. Returns
 * null when there's nothing to write into.
 */
function containerAt(fresh, edited, path) {
    let cur = fresh;
    for (let i = 0; i < path.length; i++) {
        const seg = path[i];
        if (typeof seg === 'string') {
            if (!isPlainObject(cur)) return null;
            if (cur[seg] == null || typeof cur[seg] !== 'object') {
                const shape = valueAt(edited, path.slice(0, i + 1));
                cur[seg] = Array.isArray(shape) ? [] : {};
            }
            cur = cur[seg];
        } else {
            if (!Array.isArray(cur)) return null;
            let item = cur.find((x) => itemId(x) === seg.id);
            if (!item) {
                const editedList = valueAt(edited, path.slice(0, i)) || [];
                const at = editedList.findIndex((x) => itemId(x) === seg.id);
                const whole = clone(editedList[at]);
                if (!whole) return null;
                cur.splice(insertIndex(cur, editedList, at), 0, whole);
                item = whole;
            }
            cur = item;
        }
    }
    return cur;
}

function applyOne(fresh, edited, change) {
    const { path, op } = change;
    if (op === 'set' || op === 'delete') {
        const last = path[path.length - 1];
        const parent = containerAt(fresh, edited, path.slice(0, -1));
        if (!parent || typeof last !== 'string' || !isPlainObject(parent)) return;
        if (op === 'delete') delete parent[last];
        else parent[last] = clone(valueAt(edited, path));
        return;
    }
    if (op === 'remove') {
        const last = path[path.length - 1];
        const list = valueAt(fresh, path.slice(0, -1));
        if (!Array.isArray(list) || typeof last === 'string') return;
        const idx = list.findIndex((x) => itemId(x) === last.id);
        if (idx >= 0) list.splice(idx, 1);
        return;
    }
    const list = containerAt(fresh, edited, path);
    const editedList = valueAt(edited, path);
    if (!Array.isArray(list) || !Array.isArray(editedList)) return;
    if (op === 'insert') {
        const item = clone(editedList[change.at]);
        if (item === undefined) return;
        const id = itemId(item);
        if (id && list.some((x) => itemId(x) === id)) return;
        list.splice(insertIndex(list, editedList, change.at), 0, item);
        return;
    }
    if (op === 'order') {
        // The items the user saw take the slots they hold now, in the
        // user's order; items added elsewhere stay where they are.
        const wanted = new Set(change.ids);
        const slots = [];
        list.forEach((x, i) => {
            if (wanted.has(itemId(x))) slots.push(i);
        });
        const byId = new Map(slots.map((i) => [itemId(list[i]), list[i]]));
        const ordered = change.ids.filter((id) => byId.has(id)).map((id) => byId.get(id));
        slots.forEach((slot, n) => {
            list[slot] = ordered[n];
        });
    }
}

/**
 * Write the user's changes into `fresh`, reading the values from `edited`.
 * Everything else in `fresh` stays as it is.
 *
 * @param {object} fresh - a current copy of the config; changed in place
 * @param {object} edited - the section's version
 * @param {Change[]} changes - from `diffPaths`
 * @returns {object} `fresh`
 */
export function applyPaths(fresh, edited, changes) {
    for (const change of changes || []) applyOne(fresh, edited, change);
    return fresh;
}

/**
 * The dotted setting paths of `changes` that name a single setting (no
 * list items): `ui.theme`, `hotkey`. Used for the "Changed on all your
 * computers" note.
 */
export function settingPaths(changes) {
    const out = new Set();
    for (const c of changes || []) {
        if ((c.op === 'set' || c.op === 'delete') && c.path.every((s) => typeof s === 'string')) {
            out.add(c.path.join('.'));
        }
    }
    return [...out];
}
