import { describe, it, expect } from 'vitest';
import {
  matchShortcut,
  scoreShortcutMatch,
  buildShortcutCommand,
  validateShortcutArgs,
} from '../../ui/js/shared/shortcuts.js';

describe('matchShortcut', () => {
  const shortcuts = [
    { shortcut: 'gh', action_type: 'open_url', url: 'https://github.com/{0}' },
    { shortcut: 'gh', action_type: 'open_url', url: 'https://github.com' },
    { shortcut: 'note', action_type: 'prompt', prompt: '{*}' },
  ];

  it('returns null for no matches', () => {
    expect(matchShortcut('unknown', shortcuts)).toBeNull();
  });

  it('matches by trigger word (case-insensitive)', () => {
    const result = matchShortcut('GH', shortcuts);
    expect(result).not.toBeNull();
    expect(result.length).toBe(2);
  });

  it('sorts by score (best match first)', () => {
    const result = matchShortcut('gh octocat', shortcuts);
    expect(result[0].score).toBeGreaterThanOrEqual(result[1].score);
  });

  it('passes args to matches', () => {
    const result = matchShortcut('note remember this', shortcuts);
    expect(result).not.toBeNull();
    expect(result[0].args).toEqual(['remember', 'this']);
  });
});

describe('scoreShortcutMatch', () => {
  it('scores 100 for URL with matching placeholder count', () => {
    const s = { action_type: 'open_url', url: 'https://x.com/{0}' };
    expect(scoreShortcutMatch(s, ['arg1'])).toBe(100);
  });

  it('scores 80 for URL with extra args', () => {
    const s = { action_type: 'open_url', url: 'https://x.com/{0}' };
    expect(scoreShortcutMatch(s, ['arg1', 'arg2'])).toBe(80);
  });

  it('scores 60 for URL with missing args', () => {
    const s = { action_type: 'open_url', url: 'https://x.com/{0}' };
    expect(scoreShortcutMatch(s, [])).toBe(60);
  });

  it('scores 90 for wildcard URL with args', () => {
    const s = { action_type: 'open_url', url: 'https://search.com?q={*}' };
    expect(scoreShortcutMatch(s, ['hello'])).toBe(90);
  });

  it('scores 100 for no-arg URL with no args', () => {
    const s = { action_type: 'open_url', url: 'https://example.com' };
    expect(scoreShortcutMatch(s, [])).toBe(100);
  });

  it('scores run_program with no template and no args as 100', () => {
    const s = { action_type: 'run_program', arguments: '' };
    expect(scoreShortcutMatch(s, [])).toBe(100);
  });
});

describe('buildShortcutCommand', () => {
  it('builds open_url with substituted args', () => {
    const s = { shortcut: 'gh', action_type: 'open_url', url: 'https://github.com/{0}' };
    const result = buildShortcutCommand(s, ['octocat']);
    expect(result.type).toBe('open_url');
    expect(result.url).toBe('https://github.com/octocat');
  });

  it('URL-encodes args in open_url', () => {
    const s = { shortcut: 'search', action_type: 'open_url', url: 'https://google.com?q={*}' };
    const result = buildShortcutCommand(s, ['hello', 'world']);
    expect(result.url).toContain('hello%20world');
  });

  it('builds prompt with wildcard', () => {
    const s = { shortcut: 'ask', action_type: 'prompt', prompt: 'Please help: {*}' };
    const result = buildShortcutCommand(s, ['fix', 'this']);
    expect(result.type).toBe('prompt');
    expect(result.message).toBe('Please help: fix this');
  });

  it('substitutes {selection} placeholder', () => {
    const s = { shortcut: 'explain', action_type: 'prompt', prompt: 'Explain: {selection}' };
    const result = buildShortcutCommand(s, [], 'some selected text');
    expect(result.message).toBe('Explain: some selected text');
  });

  it('returns error for missing required args', () => {
    const s = { shortcut: 'deploy', action_type: 'open_url', url: 'https://x.com/{0}/{1}' };
    const result = buildShortcutCommand(s, ['only-one']);
    expect(result.type).toBe('error');
    expect(result.message).toContain('missing');
  });

  it('builds run_program with path and args', () => {
    const s = { shortcut: 'code', action_type: 'run_program', path: 'code', arguments: '{*}' };
    const result = buildShortcutCommand(s, ['myfile.js']);
    expect(result.type).toBe('run_program');
    expect(result.path).toBe('code');
    expect(result.args).toContain('myfile.js');
  });

  it('builds script shortcut returning text', () => {
    const s = { shortcut: 'upper', action_type: 'script', script: 'return args.join(" ").toUpperCase()', script_action: 'text' };
    const result = buildShortcutCommand(s, ['hello', 'world']);
    expect(result.type).toBe('text');
    expect(result.message).toBe('HELLO WORLD');
  });

  it('returns error for script exceptions', () => {
    const s = { shortcut: 'bad', action_type: 'script', script: 'throw new Error("boom")' };
    const result = buildShortcutCommand(s, []);
    expect(result.type).toBe('error');
    expect(result.message).toContain('boom');
  });

  it('returns noop for script returning null', () => {
    const s = { shortcut: 'noop', action_type: 'script', script: 'return null' };
    const result = buildShortcutCommand(s, []);
    expect(result.type).toBe('noop');
  });
});

describe('validateShortcutArgs', () => {
  it('valid when no placeholders', () => {
    const s = { url: 'https://example.com' };
    expect(validateShortcutArgs(s, []).valid).toBe(true);
  });

  it('valid when wildcard is used', () => {
    const s = { prompt: '{*}' };
    expect(validateShortcutArgs(s, []).valid).toBe(true);
  });

  it('valid when all required args provided', () => {
    const s = { url: 'https://x.com/{0}/{1}' };
    expect(validateShortcutArgs(s, ['a', 'b']).valid).toBe(true);
  });

  it('invalid when required args missing', () => {
    const s = { url: 'https://x.com/{0}/{1}' };
    const result = validateShortcutArgs(s, ['a']);
    expect(result.valid).toBe(false);
    expect(result.message).toContain('missing');
  });

  it('optional params do not count as required', () => {
    const s = { url: 'https://x.com/{0}/{1?}' };
    expect(validateShortcutArgs(s, ['a']).valid).toBe(true);
  });
});
