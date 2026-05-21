import { describe, it, expect } from 'vitest';
import { getToolIcon, getToolEmoji, escapeHtml } from '../../ui/js/shared/tool-utils.js';

describe('getToolIcon', () => {
  it('returns search icon for search kinds', () => {
    expect(getToolIcon('search')).toBe('🔍');
    expect(getToolIcon('web_search')).toBe('🔍');
  });

  it('returns edit icon for write kinds', () => {
    expect(getToolIcon('edit')).toBe('✏️');
    expect(getToolIcon('write')).toBe('✏️');
  });

  it('returns read icon', () => {
    expect(getToolIcon('read')).toBe('📖');
  });

  it('returns shell icon', () => {
    expect(getToolIcon('shell')).toBe('💻');
    expect(getToolIcon('terminal')).toBe('💻');
  });

  it('returns default wrench for unknown kinds', () => {
    expect(getToolIcon('unknown')).toBe('🔧');
    expect(getToolIcon('')).toBe('🔧');
    expect(getToolIcon(null)).toBe('🔧');
  });

  it('is case-insensitive', () => {
    expect(getToolIcon('SEARCH')).toBe('🔍');
    expect(getToolIcon('Read')).toBe('📖');
  });
});

describe('getToolEmoji', () => {
  it('returns extension icon for ext: prefix', () => {
    expect(getToolEmoji('ext:my-tool')).toBe('🧩');
  });

  it('matches partial names', () => {
    expect(getToolEmoji('file_search')).toBe('🔍');
    expect(getToolEmoji('read_file')).toBe('📖');
    expect(getToolEmoji('write_to_disk')).toBe('✏️');
    expect(getToolEmoji('run_shell_command')).toBe('💻');
  });

  it('returns cloud icon for AWS tools', () => {
    expect(getToolEmoji('aws_s3_list')).toBe('☁️');
  });

  it('returns default for unknown names', () => {
    expect(getToolEmoji('something')).toBe('🔧');
    expect(getToolEmoji(null)).toBe('🔧');
  });
});

describe('escapeHtml', () => {
  it('escapes angle brackets', () => {
    expect(escapeHtml('<script>alert("xss")</script>')).toBe(
      '&lt;script&gt;alert("xss")&lt;/script&gt;'
    );
  });

  it('escapes ampersands', () => {
    expect(escapeHtml('a & b')).toBe('a &amp; b');
  });

  it('handles empty string', () => {
    expect(escapeHtml('')).toBe('');
  });

  it('passes through safe text', () => {
    expect(escapeHtml('hello world')).toBe('hello world');
  });
});
