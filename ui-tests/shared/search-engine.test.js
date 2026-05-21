import { describe, it, expect, vi, beforeEach } from 'vitest';

// search-engine.js imports from commands.js which has Tauri dependencies,
// so we mock it before importing.
vi.mock('../../ui/js/shared/commands.js', () => ({
  matchCommands: vi.fn(() => []),
  matchSlashCommands: vi.fn(() => []),
  matchCommandsByName: vi.fn(() => []),
}));

let recordSelection, loadFrecency, unifiedSearch;

beforeEach(async () => {
  vi.resetModules();

  // Re-apply the mock after resetModules
  vi.doMock('../../ui/js/shared/commands.js', () => ({
    matchCommands: vi.fn(() => []),
    matchSlashCommands: vi.fn(() => []),
    matchCommandsByName: vi.fn(() => []),
  }));

  const mod = await import('../../ui/js/shared/search-engine.js');
  recordSelection = mod.recordSelection;
  loadFrecency = mod.loadFrecency;
  unifiedSearch = mod.unifiedSearch;
});

describe('search-engine module', () => {
  it('exports recordSelection as a function', () => {
    expect(typeof recordSelection).toBe('function');
  });

  it('exports loadFrecency as a function', () => {
    expect(typeof loadFrecency).toBe('function');
  });

  it('exports unifiedSearch as a function', () => {
    expect(typeof unifiedSearch).toBe('function');
  });
});

describe('unifiedSearch', () => {
  it('returns empty array for empty query', async () => {
    const invoke = vi.fn();
    const results = await unifiedSearch('', invoke, []);
    expect(results).toEqual([]);
  });
});
