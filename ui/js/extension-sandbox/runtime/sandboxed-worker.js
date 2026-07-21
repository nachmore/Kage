/**
 * Build the bounded Worker runner exposed to extension providers.
 *
 * The runner receives its host-owned dependencies instead of importing the
 * runtime so workers remain isolated from the iframe message protocol.
 */
export function createSandboxedRunner({ getVendorSources, log }) {
    const workerPool = new Map();
    let nextTaskId = 1;

    const workerKey = (vendorList) => vendorList.slice().sort().join('|');

    function spawnWorker(vendorList) {
        const sources = getVendorSources();
        const parts = [];
        for (const name of vendorList) {
            const source = sources?.[name];
            if (typeof source === 'string' && source) parts.push(source);
        }

        const bootstrap = `
            (function(){
                "use strict";
                var __vendorNames = ${JSON.stringify(vendorList)};
                var __lib = {};
                for (var i = 0; i < __vendorNames.length; i++) {
                    var n = __vendorNames[i];
                    if (typeof self[n] !== 'undefined') __lib[n] = self[n];
                }
                self.onmessage = async function(ev) {
                    var msg = ev.data || {};
                    var id = msg.id;
                    try {
                        // eslint-disable-next-line no-new-func
                        var __run = (0, eval)('(' + msg.runSrc + ')');
                        var out = await __run(msg.data, __lib);
                        self.postMessage({ id: id, ok: true, result: out });
                    } catch (e) {
                        self.postMessage({ id: id, ok: false, error: String(e && e.message || e) });
                    }
                };
            })();
        `;
        parts.push(bootstrap);

        const loaded = vendorList.filter(
            (name) => typeof sources?.[name] === 'string' && sources[name]
        );
        const missing = vendorList.filter((name) => !loaded.includes(name));
        if (missing.length > 0) {
            log(
                'warn',
                `runSandboxed: missing vendor source(s) [${missing.join(', ')}] for worker; lib globals will be undefined`
            );
        }

        const url = URL.createObjectURL(new Blob(parts, { type: 'application/javascript' }));
        try {
            return { worker: new Worker(url), url };
        } catch (error) {
            URL.revokeObjectURL(url);
            log('warn', `runSandboxed: failed to spawn Worker: ${error?.message || error}`);
            throw error;
        }
    }

    function killWorker(entry) {
        const key = workerKey(entry.vendorList);
        try {
            entry.worker.terminate();
        } catch {}
        try {
            URL.revokeObjectURL(entry.url);
        } catch {}
        entry.worker = null;
        if (workerPool.get(key) === entry) workerPool.delete(key);
    }

    function pumpQueue(entry) {
        if (!entry.worker || entry.inflight) return;
        const next = entry.queue.shift();
        if (!next) return;

        const id = nextTaskId++;
        const timer = setTimeout(() => {
            const current = entry.inflight;
            if (!current || current.id !== id) return;
            entry.inflight = null;
            log(
                'warn',
                `runSandboxed: task timed out after ${next.deadline}ms [vendors: ${entry.vendorList.join(', ') || 'none'}] — worker terminated`
            );
            killWorker(entry);
            current.reject(new Error(`runSandboxed: timed out after ${next.deadline}ms`));
            if (entry.queue.length > 0) {
                const fresh = ensureWorker(entry.vendorList);
                fresh.queue.push(...entry.queue);
                entry.queue = [];
                pumpQueue(fresh);
            }
        }, next.deadline);

        entry.inflight = { id, resolve: next.resolve, reject: next.reject, timer };
        try {
            entry.worker.postMessage({ id, runSrc: next.runSrc, data: next.data });
        } catch (error) {
            clearTimeout(timer);
            entry.inflight = null;
            next.reject(new Error(`runSandboxed: failed to post data: ${error?.message || error}`));
            pumpQueue(entry);
        }
    }

    function ensureWorker(vendorList) {
        const key = workerKey(vendorList);
        let entry = workerPool.get(key);
        if (entry?.worker) return entry;

        const { worker, url } = spawnWorker(vendorList);
        entry = { worker, url, vendorList: vendorList.slice(), inflight: null, queue: [] };
        const finish = (ok, payload) => {
            const current = entry.inflight;
            if (!current) return;
            clearTimeout(current.timer);
            entry.inflight = null;
            if (ok) current.resolve(payload);
            else current.reject(payload);
            pumpQueue(entry);
        };

        worker.onmessage = (event) => {
            const { id, ok, result, error } = event.data || {};
            const current = entry.inflight;
            if (!current || current.id !== id) return;
            if (!ok) {
                log(
                    'warn',
                    `runSandboxed: run fn threw in worker [vendors: ${entry.vendorList.join(', ') || 'none'}]: ${error || 'unknown error'}`
                );
            }
            finish(ok, ok ? result : new Error(error || 'runSandboxed: worker error'));
        };
        worker.onerror = (event) => {
            log(
                'warn',
                `runSandboxed: worker onerror [vendors: ${entry.vendorList.join(', ') || 'none'}]: ${event?.message || 'worker error'}` +
                    (event?.filename ? ` @ ${event.filename}:${event.lineno || '?'}` : '')
            );
            const current = entry.inflight;
            const error = new Error(`runSandboxed: ${event?.message || 'worker error'}`);
            if (current) {
                clearTimeout(current.timer);
                entry.inflight = null;
                current.reject(error);
            }
            const pending = entry.queue.splice(0);
            killWorker(entry);
            if (pending.length > 0) {
                const fresh = ensureWorker(entry.vendorList);
                fresh.queue.push(...pending);
                pumpQueue(fresh);
            }
        };

        workerPool.set(key, entry);
        return entry;
    }

    return function runSandboxed({ run, vendor, data, timeoutMs } = {}) {
        if (typeof run !== 'function') {
            return Promise.reject(new Error('runSandboxed: run must be a function'));
        }
        const deadline = Number.isFinite(timeoutMs) && timeoutMs > 0 ? Math.floor(timeoutMs) : 1000;
        const vendorList = Array.isArray(vendor)
            ? vendor.filter((value) => typeof value === 'string')
            : [];
        const runSrc = run.toString();
        let entry;
        try {
            entry = ensureWorker(vendorList);
        } catch (error) {
            return Promise.reject(
                new Error(`runSandboxed: failed to spawn worker: ${error?.message || error}`)
            );
        }
        return new Promise((resolve, reject) => {
            entry.queue.push({ runSrc, data, deadline, resolve, reject });
            pumpQueue(entry);
        });
    };
}
