import { describe, it, expect, vi, beforeEach } from 'vitest';

// search-engine.js imports from commands.js which has Tauri dependencies,
// so we mock it before importing.
vi.mock('../../ui/js/shared/commands.js', () => ({
  matchCommands: vi.fn(() => []),
  matchSlashCommands: vi.fn(() => []),
  matchCommandsByName: vi.fn(() => []),
}));

let recordSelection, loadFrecency, unifiedSearch, setExtensionManager;
let looksLikeFileSearch, searchDebounceMs;

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
  setExtensionManager = mod.setExtensionManager;
  looksLikeFileSearch = mod.looksLikeFileSearch;
  searchDebounceMs = mod.searchDebounceMs;
});

// Rust-side search returns nothing for these queries so we can assert
// purely on extension rows.
const emptyInvoke = () => vi.fn(async () => JSON.stringify([]));

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

describe('looksLikeFileSearch', () => {
  it('is true for a trailing file extension', () => {
    expect(looksLikeFileSearch('report.pdf')).toBe(true);
    expect(looksLikeFileSearch('notes.md')).toBe(true);
    expect(looksLikeFileSearch('archive.')).toBe(true); // dot with no ext yet
  });

  it('is true for glob characters', () => {
    expect(looksLikeFileSearch('src/*.js')).toBe(true);
    expect(looksLikeFileSearch('foo?.txt')).toBe(true);
  });

  it('is true for the explicit >find prefix, case-insensitively', () => {
    expect(looksLikeFileSearch('>find budget')).toBe(true);
    expect(looksLikeFileSearch('>FIND budget')).toBe(true);
  });

  it('is false for plain text queries', () => {
    expect(looksLikeFileSearch('how do I cook rice')).toBe(false);
    expect(looksLikeFileSearch('calculator')).toBe(false);
  });

  it('is false for empty / nullish input', () => {
    expect(looksLikeFileSearch('')).toBe(false);
    expect(looksLikeFileSearch(undefined)).toBe(false);
    expect(looksLikeFileSearch(null)).toBe(false);
  });
});

describe('searchDebounceMs', () => {
  it('debounces file-shaped queries harder', () => {
    expect(searchDebounceMs('report.pdf')).toBe(250);
    expect(searchDebounceMs('>find x')).toBe(250);
  });

  it('is snappy for plain queries', () => {
    expect(searchDebounceMs('weather')).toBe(100);
    expect(searchDebounceMs('')).toBe(100);
  });
});

describe('unifiedSearch', () => {
  it('returns empty array for empty query', async () => {
    const invoke = vi.fn();
    const results = await unifiedSearch('', invoke, []);
    expect(results).toEqual([]);
  });

  it('lets an async loaded row supersede the sync placeholder from the same extension', async () => {
    setExtensionManager({
      matchAll: async () => [
        { id: 'focus-loading-today', label: 'Loading…', score: 86, _extensionId: 'focus-tracker' },
      ],
      matchAllAsync: async () => [
        { id: 'focus-summary-today', label: 'Today: 2h tracked', score: 86, _extensionId: 'focus-tracker' },
      ],
    });

    const results = await unifiedSearch('focus', emptyInvoke(), []);
    const ids = results.map((r) => r.id);
    expect(ids).toContain('focus-summary-today');
    expect(ids).not.toContain('focus-loading-today');
  });

  it('keeps the sync placeholder when matchAsync returns nothing (cache hit)', async () => {
    setExtensionManager({
      matchAll: async () => [
        { id: 'focus-loading-today', label: 'Loading…', score: 86, _extensionId: 'focus-tracker' },
      ],
      matchAllAsync: async () => [],
    });

    const results = await unifiedSearch('focus', emptyInvoke(), []);
    expect(results.map((r) => r.id)).toContain('focus-loading-today');
  });

  it('does not let one extension supersede another extension placeholder', async () => {
    setExtensionManager({
      matchAll: async () => [
        { id: 'a-loading', label: 'Loading A…', score: 86, _extensionId: 'ext-a' },
        { id: 'b-loading', label: 'Loading B…', score: 86, _extensionId: 'ext-b' },
      ],
      matchAllAsync: async () => [
        { id: 'a-loaded', label: 'A done', score: 86, _extensionId: 'ext-a' },
      ],
    });

    const results = await unifiedSearch('x', emptyInvoke(), []);
    const ids = results.map((r) => r.id);
    expect(ids).toContain('a-loaded');
    expect(ids).not.toContain('a-loading');
    expect(ids).toContain('b-loading'); // ext-b untouched
  });
});

describe('unifiedSearch — keyword completion hints', () => {
  const mgr = (defs) => ({
    matchAll: async () => [],
    matchAllAsync: async () => [],
    getKeywordDefinitions: async () => defs,
  });

  const KW = [
    {
      extensionId: 'calendar',
      keyword: 'cal-refresh',
      label: 'Refresh calendar',
      description: 'Re-fetch events',
      icon: '🔄',
      acceptsArgs: false,
    },
    {
      extensionId: 'calendar',
      keyword: 'calendar',
      label: 'Calendar',
      description: 'Upcoming meetings',
      icon: '📅',
      acceptsArgs: true,
    },
  ];

  it('surfaces a hint for an incomplete prefix of a keyword', async () => {
    setExtensionManager(mgr(KW));
    const results = await unifiedSearch('cal-ref', emptyInvoke(), []);
    const hint = results.find((r) => r.type === 'ext_keyword');
    expect(hint).toBeTruthy();
    expect(hint.id).toBe('ext-keyword:calendar:cal-refresh');
    expect(hint.label).toBe('Refresh calendar');
    expect(hint.data.keyword).toBe('cal-refresh');
  });

  it('fills with no trailing space when the keyword takes no args', async () => {
    setExtensionManager(mgr(KW));
    const results = await unifiedSearch('cal-ref', emptyInvoke(), []);
    const hint = results.find((r) => r.id === 'ext-keyword:calendar:cal-refresh');
    expect(hint.data.fill).toBe('cal-refresh');
  });

  it('fills with a trailing space when the keyword takes args', async () => {
    setExtensionManager(mgr(KW));
    // "ca" is an incomplete prefix of both "calendar" and "cal-refresh".
    const results = await unifiedSearch('ca', emptyInvoke(), []);
    const cal = results.find((r) => r.id === 'ext-keyword:calendar:calendar');
    expect(cal.data.fill).toBe('calendar '); // acceptsArgs → trailing space
  });

  it('hints every keyword the query is a prefix of', async () => {
    setExtensionManager(mgr(KW));
    const results = await unifiedSearch('ca', emptyInvoke(), []);
    const ids = results.filter((r) => r.type === 'ext_keyword').map((r) => r.id);
    expect(ids).toContain('ext-keyword:calendar:cal-refresh');
    expect(ids).toContain('ext-keyword:calendar:calendar');
  });

  it('does not hint a keyword that exactly equals the query (match() owns it)', async () => {
    setExtensionManager(mgr(KW));
    const results = await unifiedSearch('calendar', emptyInvoke(), []);
    const selfHint = results.find((r) => r.id === 'ext-keyword:calendar:calendar');
    expect(selfHint).toBeUndefined();
  });

  it('still hints both keywords for a prefix that equals neither', async () => {
    setExtensionManager(mgr(KW));
    // "cal" isn't in the set, but it's a prefix of both keywords and
    // equals neither — both should hint.
    const results = await unifiedSearch('cal', emptyInvoke(), []);
    const ids = results.filter((r) => r.type === 'ext_keyword').map((r) => r.id);
    expect(ids).toContain('ext-keyword:calendar:cal-refresh');
    expect(ids).toContain('ext-keyword:calendar:calendar');
  });

  it('suppresses hints once the query contains a space (keyword committed)', async () => {
    setExtensionManager(mgr(KW));
    const results = await unifiedSearch('calendar tomorrow', emptyInvoke(), []);
    expect(results.some((r) => r.type === 'ext_keyword')).toBe(false);
  });

  it('emits no hints for > or / prefixed queries', async () => {
    setExtensionManager(mgr(KW));
    const gt = await unifiedSearch('>cal', emptyInvoke(), []);
    const slash = await unifiedSearch('/cal', emptyInvoke(), []);
    expect(gt.some((r) => r.type === 'ext_keyword')).toBe(false);
    expect(slash.some((r) => r.type === 'ext_keyword')).toBe(false);
  });

  it('tolerates a manager without getKeywordDefinitions', async () => {
    setExtensionManager({ matchAll: async () => [], matchAllAsync: async () => [] });
    const results = await unifiedSearch('cal', emptyInvoke(), []);
    expect(results.some((r) => r.type === 'ext_keyword')).toBe(false);
  });
});
