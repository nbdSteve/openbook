import type { Source } from '@/services/api/types';

/** Convert LaTeX `\( \)` and `\[ \]` delimiters to the `$` forms that remark-math reads. */
export function normalizeMath(input: string): string {
  return input
    .replace(/\\\(([\s\S]*?)\\\)/g, (_, m: string) => `$${m}$`)
    .replace(/\\\[([\s\S]*?)\\\]/g, (_, m: string) => `$$${m}$$`);
}

const CODE_SEGMENT = /(```[\s\S]*?```|`[^`\n]*`)/g;

/**
 * Turn `[n]` citation markers into links to `#cite-n`, but only for numbers that have a source.
 * Code spans and links like `[1](url)` stay as they are.
 */
export function linkCitations(content: string, sources: Source[] | undefined): string {
  if (!sources || sources.length === 0) return content;
  const known = new Set(sources.map((s) => s.n));
  return content
    .split(CODE_SEGMENT)
    .map((part, i) => {
      if (i % 2 === 1) return part;
      return part.replace(/\[(\d{1,3})\](?![(:])/g, (match, n: string) => (known.has(Number(n)) ? `[${n}](#cite-${n})` : match));
    })
    .join('');
}

export function citationNumber(href: string | undefined): number | null {
  const match = href?.match(/^#cite-(\d+)$/);
  return match ? Number(match[1]) : null;
}
