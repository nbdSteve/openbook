import { describe, expect, it } from 'vitest';
import type { Source } from '@/services/api/types';
import { citationNumber, citedNumbers, linkCitations, normalizeMath } from './citations';

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

  it('does not touch math, but links text next to it', () => {
    expect(linkCitations('Then $a[1]$ and $$b[1]$$ and see [1].', [source(1)])).toBe(
      'Then $a[1]$ and $$b[1]$$ and see [1](#cite-1).',
    );
    expect(linkCitations('It costs $5 or $6 [1].', [source(1)])).toBe('It costs $5 or $6 [1](#cite-1).');
  });

  it('reads the number back from the link', () => {
    expect(citationNumber('#cite-12')).toBe(12);
    expect(citationNumber('https://x.test')).toBeNull();
  });
});

describe('citedNumbers', () => {
  const sources = [source(1), source(2), source(3)];

  it('lists each cited source once, in the order of its first citation', () => {
    expect(citedNumbers('B [2], then A [1], then B again [2][2].', sources)).toEqual([2, 1]);
  });

  it('ignores numbers that have no source, and answers with no sources or no markers', () => {
    expect(citedNumbers('Real [3] and missing [7] and [0].', sources)).toEqual([3]);
    expect(citedNumbers('Claim [1].', undefined)).toEqual([]);
    expect(citedNumbers('Claim [1].', [])).toEqual([]);
    expect(citedNumbers('No markers here.', sources)).toEqual([]);
    expect(citedNumbers('', sources)).toEqual([]);
  });

  it('ignores markers in code and math, like linkCitations', () => {
    expect(citedNumbers('Use `a[1]` and\n```\nb[2]\n```\nthen $a[1]$ and $$b[2]$$.', sources)).toEqual([]);
    expect(citedNumbers('Math \\(a[1]\\) and \\[b[2]\\] and text [3].', sources)).toEqual([3]);
    expect(citedNumbers('It costs $5 or $6 [1].', sources)).toEqual([1]);
  });

  it('ignores a marker followed by ( or :', () => {
    expect(citedNumbers('See [1](https://x.test) and [2]: note.', sources)).toEqual([]);
    expect(citedNumbers('See [1] (really) and [2] : note.', sources)).toEqual([1, 2]);
  });

  it('does not count a partial marker while the answer streams', () => {
    expect(citedNumbers('Claim [1', sources)).toEqual([]);
    expect(citedNumbers('Claim [1]', sources)).toEqual([1]);
  });

  it('agrees with linkCitations on every case above', () => {
    const texts = ['B [2], A [1][2].', 'Use `a[1]` $a[1]$ [3]', '[1](u) [2]: x [3]', 'Math \\(a[1]\\) [2]'];
    for (const text of texts) {
      const linked = [...linkCitations(normalizeMath(text), sources).matchAll(/\[(\d+)\]\(#cite-\d+\)/g)].map((m) => Number(m[1]));
      expect(citedNumbers(text, sources)).toEqual([...new Set(linked)]);
    }
  });
});

describe('normalizeMath', () => {
  it('converts \\( \\) to inline dollars', () => {
    expect(normalizeMath('so \\(a^2\\) here')).toBe('so $a^2$ here');
  });

  it('puts display math on its own lines so it renders as a block', () => {
    expect(normalizeMath('So $$E = mc^2$$ holds.')).toBe('So \n\n$$\nE = mc^2\n$$\n\n holds.');
    expect(normalizeMath('\\[x^2\\]')).toBe('\n\n$$\nx^2\n$$\n\n');
  });

  it('leaves code alone', () => {
    const code = '```\n\\(x^2\\) and $$y$$\n```\nand `\\(z\\)`';
    expect(normalizeMath(code)).toBe(code);
  });
});
