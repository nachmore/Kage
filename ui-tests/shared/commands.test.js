import { describe, it, expect } from 'vitest';
import { matchCommands, matchCommandsByName } from '../../ui/js/shared/commands.js';

describe('matchCommands', () => {
  it('returns null for non-command input', () => {
    expect(matchCommands('hello')).toBeNull();
    expect(matchCommands('search something')).toBeNull();
  });

  it('returns all commands for bare >', () => {
    const result = matchCommands('>');
    expect(result).not.toBeNull();
    expect(result.length).toBeGreaterThan(0);
  });

  it('filters by prefix', () => {
    const result = matchCommands('>set');
    expect(result).not.toBeNull();
    expect(result.some(c => c.name === 'settings')).toBe(true);
    expect(result.every(c => c.name.startsWith('set'))).toBe(true);
  });

  it('matches quit command', () => {
    const result = matchCommands('>quit');
    expect(result).not.toBeNull();
    expect(result.some(c => c.name === 'quit')).toBe(true);
  });

  it('matches by alias', () => {
    const result = matchCommands('>cb');
    expect(result).not.toBeNull();
    expect(result.some(c => c.name === 'clipboard')).toBe(true);
  });

  it('returns empty array for no matches', () => {
    const result = matchCommands('>zzzzz');
    expect(result).toEqual([]);
  });
});

describe('matchCommandsByName', () => {
  it('returns empty for empty query', () => {
    expect(matchCommandsByName('')).toEqual([]);
  });

  it('matches commands by name prefix', () => {
    const result = matchCommandsByName('ses');
    expect(result.length).toBeGreaterThan(0);
    expect(result.every(c => c.type === 'command')).toBe(true);
  });

  it('matches by alias', () => {
    const result = matchCommandsByName('cb');
    expect(result.some(c => c.name === 'clipboard')).toBe(true);
  });
});
