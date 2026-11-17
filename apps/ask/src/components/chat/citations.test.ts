import { describe, expect, it } from 'vitest';
import type { Source } from '@/services/api/types';
import { citedNumbers, normalizeMath } from './citations';
import { chipName, citationNumber } from './sourceText';

const source = (n: number): Source => ({ n, documentId: 'D', documentName: 'a.pdf', snippet: 's' });

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

  it('ignores markers in code of every kind', () => {
    expect(citedNumbers('Use `a[1]` and ``b[2]``.', sources)).toEqual([]);
    expect(citedNumbers('```\nb[2]\n```', sources)).toEqual([]);
    expect(citedNumbers('````\n```\n[1]\n```\n````', sources)).toEqual([]);
    expect(citedNumbers('Text\n\n    indented [1]\n\nmore', sources)).toEqual([]);
    expect(citedNumbers('Use `a\n[1]` here', sources)).toEqual([]);
    expect(citedNumbers('```python\nx = arr[1]', sources)).toEqual([]);
  });

  it('ignores markers in math, but not in prices', () => {
    expect(citedNumbers('Then $a[1]$ and $$b[2]$$.', sources)).toEqual([]);
    expect(citedNumbers('Math \\(a[1]\\) and \\[b[2]\\] and text [3].', sources)).toEqual([3]);
    expect(citedNumbers('It costs \\$5 per unit [1], up from \\$4.', sources)).toEqual([1]);
  });

  it('ignores markers in HTML and in the text of a link', () => {
    expect(citedNumbers('<div>\n[1]\n</div>', sources)).toEqual([]);
    expect(citedNumbers('See [about [2]](https://x.test) and [1](https://x.test).', sources)).toEqual([]);
  });

  it('counts a marker before a colon in a line, but not a half-written link', () => {
    expect(citedNumbers('Claim [1]: more text.', sources)).toEqual([1]);
    expect(citedNumbers('See [1](https://x', sources)).toEqual([]);
    expect(citedNumbers('See [1] (really).', sources)).toEqual([1]);
  });

  it('counts a bare number that has a reference definition, but not a full reference', () => {
    expect(citedNumbers('Claim [1].\n\n[1]: https://x.test', sources)).toEqual([1]);
    expect(citedNumbers('Claim [1][].\n\n[1]: Lecture4.pdf', sources)).toEqual([1]);
    expect(citedNumbers('See [text][1].\n\n[1]: https://x.test', sources)).toEqual([]);
  });

  it('counts a model-written link to a known source, and not to an unknown one', () => {
    expect(citedNumbers('A [this](#cite-3) and [that](#cite-9).', sources)).toEqual([3]);
  });

  it('does not count a partial marker while the answer streams', () => {
    expect(citedNumbers('Claim [1', sources)).toEqual([]);
    expect(citedNumbers('Claim [1]', sources)).toEqual([1]);
  });
});

describe('citationNumber and chipName', () => {
  it('reads the number back from the link', () => {
    expect(citationNumber('#cite-12')).toBe(12);
    expect(citationNumber('https://x.test')).toBeNull();
  });

  it('drops a trailing .pdf in any case, and nothing else', () => {
    expect(chipName('Lecture 4.pdf')).toBe('Lecture 4');
    expect(chipName('Lecture 4.PDF')).toBe('Lecture 4');
    expect(chipName('a.pdf.docx')).toBe('a.pdf.docx');
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
