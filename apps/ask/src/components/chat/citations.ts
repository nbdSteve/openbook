import type { Source } from '@/services/api/types';

const CODE_SEGMENT = /(```[\s\S]*?```|~~~[\s\S]*?~~~|`[^`\n]*`)/;
// Display math, then inline math whose dollar signs touch the formula (so prices such as "$5 or $6" stay text).
const MATH_SEGMENT = /(\$\$[\s\S]*?\$\$|\$[^$\s](?:[^$\n]*[^$\s])?\$)/;

/** Apply `change` to the text outside the given segments (code, math), and keep the segments as they are. */
function outside(text: string, segment: RegExp, change: (part: string) => string): string {
  // The segment regexes have one capture group, so `split` puts the segments at the odd indexes.
  return text
    .split(segment)
    .map((part, i) => (i % 2 === 1 ? part : change(part)))
    .join('');
}

/**
 * Convert LaTeX delimiters to the forms that remark-math reads. Display math goes on its own
 * lines, so it renders as a block (`.katex-display`) even when the model wrote it inside a
 * paragraph. Code is left alone.
 */
export function normalizeMath(input: string): string {
  return outside(input, CODE_SEGMENT, (part) =>
    part
      .replace(/\\\(([\s\S]*?)\\\)/g, (_, m: string) => `$${m.trim()}$`)
      .replace(/\\\[([\s\S]*?)\\\]/g, (_, m: string) => `$$${m}$$`)
      .replace(/\$\$([\s\S]*?)\$\$/g, (_, m: string) => `\n\n$$\n${m.trim()}\n$$\n\n`),
  );
}

const MARKER = /\[(\d{1,3})\](?![(:])/g;

/** Call `onMarker` for each `[n]` marker outside code and math, and return the text with the results put in. */
function replaceMarkers(content: string, replace: (match: string, n: number) => string): string {
  return outside(content, CODE_SEGMENT, (part) =>
    outside(part, MATH_SEGMENT, (text) => text.replace(MARKER, (match, n: string) => replace(match, Number(n)))),
  );
}

/**
 * Turn `[n]` citation markers into links to `#cite-n`, but only for numbers that have a source.
 * Code, math, and links like `[1](url)` stay as they are.
 */
export function linkCitations(content: string, sources: Source[] | undefined): string {
  if (!sources || sources.length === 0) return content;
  const known = new Set(sources.map((s) => s.n));
  return replaceMarkers(content, (match, n) => (known.has(n) ? `[${n}](#cite-${n})` : match));
}

/**
 * The source numbers that the answer cites, in the order of their first citation. A source is
 * cited when `linkCitations` links its marker, so this uses the same scan on the same text that
 * the renderer gets (after `normalizeMath`). Give it the raw message content.
 */
export function citedNumbers(content: string, sources: Source[] | undefined): number[] {
  if (!sources || sources.length === 0) return [];
  const known = new Set(sources.map((s) => s.n));
  const cited: number[] = [];
  replaceMarkers(normalizeMath(content), (match, n) => {
    if (known.has(n) && !cited.includes(n)) cited.push(n);
    return match;
  });
  return cited;
}

export function citationNumber(href: string | undefined): number | null {
  const match = href?.match(/^#cite-(\d+)$/);
  return match ? Number(match[1]) : null;
}

/** "Algebra notes.pdf, p. 12". The page is left out when the source has none. */
export function sourceLabel(source: Pick<Source, 'documentName' | 'page'>): string {
  return source.page !== undefined ? `${source.documentName}, p. ${source.page}` : source.documentName;
}
