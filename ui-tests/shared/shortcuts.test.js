import { describe, it, expect } from 'vitest';
import {
  matchShortcut,
  scoreShortcutMatch,
  buildShortcutCommand,
  validateShortcutArgs,
  extractPlaceholders,
  summarizeNamedPlaceholders,
  resolveNamedPlaceholders,
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
    expect(result.message).toContain('more parameter');
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
    expect(result.message).toContain('more parameter');
  });

  it('optional params do not count as required', () => {
    const s = { url: 'https://x.com/{0}/{1?}' };
    expect(validateShortcutArgs(s, ['a']).valid).toBe(true);
  });

  it('valid when named placeholder is filled positionally', () => {
    const s = { prompt: 'Translate to {lang}: {*}' };
    expect(validateShortcutArgs(s, ['spanish', 'hello']).valid).toBe(true);
  });

  it('invalid when named placeholder has no value (no args, no params)', () => {
    const s = { prompt: 'Translate to {lang}: {*}' };
    const result = validateShortcutArgs(s, []);
    expect(result.valid).toBe(false);
    expect(result.message).toContain('lang');
  });

  it('valid when named placeholder is supplied via paramsByName', () => {
    const s = { prompt: 'Translate to {lang}: {*}' };
    expect(validateShortcutArgs(s, ['hi'], { lang: 'french' }).valid).toBe(true);
  });

  it('optional named placeholder is not required', () => {
    const s = { prompt: 'Hello {name?}!' };
    expect(validateShortcutArgs(s, []).valid).toBe(true);
  });

  it('reports missingNumbered=false when only named are missing', () => {
    const s = { prompt: '{lang} / {level}' };
    const result = validateShortcutArgs(s, []);
    expect(result.valid).toBe(false);
    expect(result.missingNumbered).toBeFalsy();
  });

  it('reports missingNumbered=true when a numbered placeholder is missing', () => {
    const s = { prompt: 'Pick {0}' };
    const result = validateShortcutArgs(s, []);
    expect(result.valid).toBe(false);
    expect(result.missingNumbered).toBe(true);
  });
});

describe('extractPlaceholders', () => {
  it('returns empty array for templates with no placeholders', () => {
    expect(extractPlaceholders('plain text')).toEqual([]);
  });

  it('detects numbered, named, and wildcard placeholders in order', () => {
    const result = extractPlaceholders('Hi {name}, your {0} is {value?} and rest {*}');
    expect(result.map((p) => p.kind)).toEqual(['named', 'numbered', 'named', 'wildcard']);
    expect(result[0].name).toBe('name');
    expect(result[1].index).toBe(0);
    expect(result[2].optional).toBe(true);
  });

  it('treats {selection} as its own kind', () => {
    const result = extractPlaceholders('Explain: {selection}');
    expect(result[0].kind).toBe('selection');
  });
});

describe('summarizeNamedPlaceholders', () => {
  it('dedupes by name in order of first appearance', () => {
    const result = summarizeNamedPlaceholders('{lang} hello {level} {lang}');
    expect(result.map((p) => p.name)).toEqual(['lang', 'level']);
  });

  it('promotes optional to required if any usage is required', () => {
    const result = summarizeNamedPlaceholders('{lang?} and {lang}');
    const lang = result.find((p) => p.name === 'lang');
    expect(lang.optional).toBe(false);
  });
});

describe('resolveNamedPlaceholders', () => {
  it('consumes args left-to-right for named placeholders', () => {
    const r = resolveNamedPlaceholders(['{lang} {level}'], ['spanish', 'expert']);
    expect(r.filled).toEqual({ lang: 'spanish', level: 'expert' });
    expect(r.unfilled).toEqual([]);
    expect(r.argsConsumedByNamed).toBe(2);
  });

  it('paramsByName values win over positional args', () => {
    const r = resolveNamedPlaceholders(
      ['{lang} {level}'],
      ['spanish', 'expert'],
      { lang: 'french' }
    );
    expect(r.filled).toEqual({ lang: 'french', level: 'spanish' });
    expect(r.argsConsumedByNamed).toBe(1);
  });

  it('reports unfilled named placeholders', () => {
    const r = resolveNamedPlaceholders(['{lang} {level}'], ['spanish']);
    expect(r.filled).toEqual({ lang: 'spanish' });
    expect(r.unfilled.map((p) => p.name)).toEqual(['level']);
  });
});

describe('buildShortcutCommand — named placeholders', () => {
  it('substitutes named placeholders from positional args', () => {
    const s = {
      shortcut: 'tr',
      action_type: 'prompt',
      prompt: 'Translate to {lang}: {*}',
    };
    const result = buildShortcutCommand(s, ['spanish', 'hello', 'world']);
    expect(result.type).toBe('prompt');
    expect(result.message).toBe('Translate to spanish: hello world');
  });

  it('substitutes named placeholders from paramsByName', () => {
    const s = {
      shortcut: 'tr',
      action_type: 'prompt',
      prompt: 'Translate to {lang}: {*}',
    };
    const result = buildShortcutCommand(s, ['hello'], '', { lang: 'french' });
    expect(result.type).toBe('prompt');
    expect(result.message).toBe('Translate to french: hello');
  });

  it('returns prompt_form when named placeholders are unfilled', () => {
    const s = {
      shortcut: 'sm',
      action_type: 'prompt',
      prompt: 'Summarize at {level} level: {selection}',
    };
    const result = buildShortcutCommand(s, [], 'some long text');
    expect(result.type).toBe('prompt_form');
    expect(result.shortcut).toBe(s);
    expect(result.missing.map((p) => p.name)).toEqual(['level']);
    expect(result.prefilled).toEqual({});
  });

  it('strips unfilled optional named placeholders', () => {
    const s = {
      shortcut: 'hi',
      action_type: 'prompt',
      prompt: 'Hello{name?}!',
    };
    const result = buildShortcutCommand(s, []);
    expect(result.type).toBe('prompt');
    expect(result.message).toBe('Hello!');
  });

  it('repeats the same named placeholder across the template', () => {
    const s = {
      shortcut: 'echo',
      action_type: 'prompt',
      prompt: '{name}, hi {name}!',
    };
    const result = buildShortcutCommand(s, ['Sam']);
    expect(result.message).toBe('Sam, hi Sam!');
  });

  it('URL-encodes named placeholders in open_url', () => {
    const s = {
      shortcut: 'wiki',
      action_type: 'open_url',
      url: 'https://en.wikipedia.org/wiki/{topic}',
    };
    const result = buildShortcutCommand(s, ['New York']);
    expect(result.url).toBe('https://en.wikipedia.org/wiki/New%20York');
  });
});

describe('scoreShortcutMatch — named placeholders', () => {
  it('scores 100 when args fill named placeholders exactly', () => {
    const s = { action_type: 'prompt', prompt: 'Translate to {lang}: hi' };
    expect(scoreShortcutMatch(s, ['spanish'])).toBe(100);
  });

  it('scores 60 when named placeholders are missing', () => {
    const s = { action_type: 'prompt', prompt: 'Translate to {lang}: hi' };
    expect(scoreShortcutMatch(s, [])).toBe(60);
  });

  it('scores 90 when wildcard + named are both filled (any args)', () => {
    const s = { action_type: 'prompt', prompt: 'Translate to {lang}: {*}' };
    expect(scoreShortcutMatch(s, ['spanish', 'hello'])).toBe(90);
  });
});
