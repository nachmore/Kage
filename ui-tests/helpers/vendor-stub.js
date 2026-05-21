// Stub for vendor libs that load at runtime via <script> tag or dynamic
// import. Anything that actually uses these will fail loudly — that's fine,
// tests touching diagram rendering should mock per-test rather than relying
// on real wasm.
export const Graphviz = {
    load() {
        throw new Error(
            'vendor-stub: Graphviz is not loaded in the test environment. ' +
            'Mock it in your test if you need it.'
        );
    },
};
