import type { Parent, Root, RootContent } from 'mdast';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import remarkParse from 'remark-parse';
import { unified } from 'unified';
import type { Source } from '@/services/api/types';

const CODE_SEGMENT = /(```[\s\S]*?```|~~~[\s\S]*?~~~|`[^`\n]*`)/;

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

// A marker is `[n]` with one to three digits. A `(` right after it is a half-written link, so it stays text.
const MARKER = /\[(\d{1,3})\](?!\()/g;
const CITE_HREF = /^#cite-(\d+)$/;

/** The parser that the renderer uses (react-markdown with these two plugins). */
const parser = unified().use(remarkParse).use(remarkGfm).use(remarkMath).freeze();

/**
 * Turn `[n]` markers in the text nodes of the tree into links to `#cite-n`, and return the
 * numbers that are cited, in the order of their first citation. Only the parser decides what is
 * text: code, math, HTML, images, and the text of a link are never touched. A link to `#cite-n`
 * that the model wrote itself counts as a citation too. A number without a source is not cited.
 */
function citeInTree(tree: Root, known: Set<number>): number[] {
  const cited: number[] = [];
  const note = (n: number) => {
    if (!cited.includes(n)) cited.push(n);
  };
  const walk = (parent: Parent) => {
    const children: RootContent[] = [];
    for (const child of parent.children) {
      if (child.type === 'link') {
        const n = Number(CITE_HREF.exec(child.url)?.[1]);
        if (known.has(n)) note(n);
        children.push(child);
      } else if (child.type === 'linkReference') {
        children.push(child);
      } else if (child.type === 'text') {
        let last = 0;
        for (const match of child.value.matchAll(MARKER)) {
          const n = Number(match[1]);
          if (!known.has(n)) continue;
          if (match.index > last) children.push({ type: 'text', value: child.value.slice(last, match.index) });
          children.push({ type: 'link', url: `#cite-${n}`, children: [{ type: 'text', value: String(n) }] });
          note(n);
          last = match.index + match[0].length;
        }
        if (last === 0) children.push(child);
        else if (last < child.value.length) children.push({ type: 'text', value: child.value.slice(last) });
      } else {
        if ('children' in child) walk(child as Parent);
        children.push(child);
      }
    }
    parent.children = children as Parent['children'];
  };
  walk(tree);
  return cited;
}

/**
 * A remark plugin for the renderer. It links the citation markers of the sources that exist.
 * Give it the same `sources` as `citedNumbers`, so that the badges and the sources list agree.
 */
export function remarkCitations(options: { sources: Source[] | undefined }) {
  const known = new Set((options.sources ?? []).map((s) => s.n));
  return (tree: Root) => {
    if (known.size > 0) citeInTree(tree, known);
  };
}

/**
 * The source numbers that the answer cites, in the order of their first citation. It parses the
 * text with the renderer's parser and runs the same step as `remarkCitations`, so "cited" means
 * "shown as a badge". Give it the raw message content.
 */
export function citedNumbers(content: string, sources: Source[] | undefined): number[] {
  if (!sources || sources.length === 0) return [];
  const tree = parser.parse(normalizeMath(content));
  return citeInTree(tree, new Set(sources.map((s) => s.n)));
}

export function citationNumber(href: string | undefined): number | null {
  const match = href?.match(/^#cite-(\d+)$/);
  return match ? Number(match[1]) : null;
}

/** "Algebra notes.pdf, p. 12". The page is left out when the source has none. */
export function sourceLabel(source: Pick<Source, 'documentName' | 'page'>): string {
  return source.page !== undefined ? `${source.documentName}, p. ${source.page}` : source.documentName;
}

/** The document name for a chip: without a trailing ".pdf", which only wastes space. The popover keeps the full name. */
export function chipName(documentName: string): string {
  return documentName.replace(/\.pdf$/i, '');
}
