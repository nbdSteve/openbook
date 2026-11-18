import { describe, expect, it } from 'vitest';
import { formatBytes, normalizeInviteCode, safeNextPath } from './format';

describe('safeNextPath', () => {
  it('keeps same-origin paths', () => {
    expect(safeNextPath('/join/ABCD2345')).toBe('/join/ABCD2345');
    expect(safeNextPath('/ask/courses/X?tab=invite')).toBe('/ask/courses/X?tab=invite');
  });

  it.each([
    ['an absolute URL', 'https://evil.test/'],
    ['a protocol-relative URL', '//evil.test'],
    ['a backslash trick', '/\\evil.test'],
    ['a tab that the URL parser drops', '/\t/evil.test'],
    ['a newline', '/\n/evil.test'],
    ['an empty value', ''],
  ])('falls back for %s', (_name, value) => {
    expect(safeNextPath(value)).toBe('/ask');
  });

  it('falls back for null', () => {
    expect(safeNextPath(null)).toBe('/ask');
  });
});

describe('normalizeInviteCode', () => {
  it('ignores case, spaces, and hyphens', () => {
    expect(normalizeInviteCode(' k7mq-4xw2 ')).toBe('K7MQ4XW2');
    expect(normalizeInviteCode('K7MQ 4XW2')).toBe('K7MQ4XW2');
  });
});

describe('formatBytes', () => {
  it('uses B, KB, and MB', () => {
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(2048)).toBe('2 KB');
    expect(formatBytes(26214400)).toBe('25 MB');
  });
});
