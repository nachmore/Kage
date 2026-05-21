import { defineConfig } from 'vitest/config';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const vendorStub = path.resolve(here, 'helpers/vendor-stub.js');

// Custom resolver: vendor libs (graphviz wasm, mermaid, etc.) are loaded
// at runtime via <script> tags or dynamic imports against ui/vendor/lib/.
// They aren't npm-installable, so vite's default resolver can't find them
// during test transforms. This plugin intercepts those specific paths and
// points them at a stub that throws on use — tests that don't touch
// diagram rendering won't hit it.
const stubVendorLibs = {
  name: 'kage-stub-vendor-libs',
  resolveId(source) {
    if (source.endsWith('/vendor/lib/graphviz.js')) return vendorStub;
    return null;
  },
};

export default defineConfig({
  plugins: [stubVendorLibs],
  test: {
    environment: 'jsdom',
    globals: true,
    root: '.',
    include: ['**/*.test.js'],
  },
});
