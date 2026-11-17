import type { Parent, Root, RootContent } from 'mdast';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import remarkParse from 'remark-parse';
import { unified, type PluggableList, type Transformer } from 'unified';
import { VFile } from 'vfile';
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

/**
 * The plugins that the renderer and `citedNumbers` share, in order. Both add `remarkCitations` after
 * them. A plugin that is added here runs in both places.
 */
export const markdownPlugins: PluggableList = [remarkGfm, remarkMath];

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
        // "[1]" with a definition "[1]: ..." in the text is a reference, not a link that the author wrote.
        // It is a citation when it is the bare number of a known source.
        const only = child.children.length === 1 ? child.children[0] : undefined;
        const n = only?.type === 'text' && /^\d{1,3}$/.test(only.value) ? Number(only.value) : NaN;
        if (child.referenceType !== 'full' && known.has(n)) {
          children.push({ type: 'link', url: `#cite-${n}`, children: [{ type: 'text', value: String(n) }] });
          note(n);
        } else children.push(child);
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
 * A remark plugin. It links the citation markers of the sources that exist, and writes the numbers
 * that are cited, in the order of their first citation, to `file.data.citedNumbers`. The renderer
 * and `citedNumbers` use it with the same `markdownPlugins` and the same `sources`, so the badges
 * and the sources list agree.
 */
export function remarkCitations(options: { sources: Source[] | undefined }) {
  const known = new Set((options.sources ?? []).map((s) => s.n));
  const transform: Transformer<Root, Root> = (tree, file) => {
    (file.data as CitedData).citedNumbers = known.size > 0 ? citeInTree(tree, known) : [];
  };
  return transform;
}

interface CitedData {
  citedNumbers?: number[];
}

/**
 * The source numbers that the answer cites, in the order of their first citation. It runs the
 * renderer's parser and plugins on the text, so "cited" means "shown as a badge". Give it the raw
 * message content.
 */
export function citedNumbers(content: string, sources: Source[] | undefined): number[] {
  if (!sources || sources.length === 0) return [];
  const processor = unified().use(remarkParse).use(markdownPlugins).use(remarkCitations, { sources });
  const file = new VFile(normalizeMath(content));
  processor.runSync(processor.parse(file), file);
  return (file.data as CitedData).citedNumbers ?? [];
}
