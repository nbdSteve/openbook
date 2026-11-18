import type { Parent, Root, RootContent } from 'mdast';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import remarkParse from 'remark-parse';
import { unified, type PluggableList, type Transformer } from 'unified';
import { VFile } from 'vfile';
import type { Source } from '@/services/api/types';

const CODE_SEGMENT = /(```[\s\S]*?```|~~~[\s\S]*?~~~|`[^`\n]*`)/;

function outside(text: string, segment: RegExp, change: (part: string) => string): string {
  return text
    .split(segment)
    .map((part, i) => (i % 2 === 1 ? part : change(part)))
    .join('');
}

export function normalizeMath(input: string): string {
  return outside(input, CODE_SEGMENT, (part) =>
    part
      .replace(/\\\(([\s\S]*?)\\\)/g, (_, m: string) => `$${m.trim()}$`)
      .replace(/\\\[([\s\S]*?)\\\]/g, (_, m: string) => `$$${m}$$`)
      .replace(/\$\$([\s\S]*?)\$\$/g, (_, m: string) => `\n\n$$\n${m.trim()}\n$$\n\n`),
  );
}

const MARKER = /\[(\d{1,3})\](?!\()/g;
const CITE_HREF = /^#cite-(\d+)$/;

export const markdownPlugins: PluggableList = [remarkGfm, remarkMath];

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
          const before = child.value.slice(last, match.index).replace(/[ \t]+$/, '');
          if (before !== '') children.push({ type: 'text', value: before });
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

export function citedNumbers(content: string, sources: Source[] | undefined): number[] {
  if (!sources || sources.length === 0) return [];
  const processor = unified().use(remarkParse).use(markdownPlugins).use(remarkCitations, { sources });
  const file = new VFile(normalizeMath(content));
  processor.runSync(processor.parse(file), file);
  return (file.data as CitedData).citedNumbers ?? [];
}
