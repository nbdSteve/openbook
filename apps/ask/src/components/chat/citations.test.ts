import { describe, expect, it } from 'vitest';
import type { Source } from '@/services/api/types';
import { citationNumber, linkCitations, normalizeMath } from './citations';

const source = (n: number): Source => ({ n, documentId: 'D', documentName: 'a.pdf', snippet: 's' });

describe('linkCitations', () => {
  it('links only numbers that have a source', () => {
    expect(linkCitations('See [1] and [3].', [source(1), source(2)])).toBe('See [1](#cite-1) and [3].');
  });

  it('leaves text alone when there are no sources', () => {
    expect(linkCitations('Array [1] here', undefined)).toBe('Array [1] here');
    expect(linkCitations('Array [1] here', [])).toBe('Array [1] here');
  });

  it('does not touch code spans, fenced code, or existing links', () => {
    const text = 'Use `a[1]` and\n```\nb[1]\n```\nand [1](https://x.test) then [1].';
    expect(linkCitations(text, [source(1)])).toBe('Use `a[1]` and\n```\nb[1]\n```\nand [1](https://x.test) then [1](#cite-1).');
  });

  it('reads the number back from the link', () => {
    expect(citationNumber('#cite-12')).toBe(12);
    expect(citationNumber('https://x.test')).toBeNull();
  });
});

describe('normalizeMath', () => {
  it('converts \\( \\) and \\[ \\] to dollar delimiters', () => {
    expect(normalizeMath('so \\(a^2\\) and \\[b^2\\]')).toBe('so $a^2$ and $$b^2$$');
  });
});
