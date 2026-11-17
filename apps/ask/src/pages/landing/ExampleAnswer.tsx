import { useMemo, useState } from 'react';
import { Fragment, jsx, jsxs } from 'react/jsx-runtime';
import { toJsxRuntime } from 'hast-util-to-jsx-runtime';
import tree from 'virtual:example-answer';
import 'katex/dist/katex.min.css';
import { AnswerFrame } from '@/components/chat/AnswerFrame';
import { DEMO_CAPTIONS } from '@/components/chat/captions';
import { SourceButton, SourcesList } from '@/components/chat/SourceBadge';
import { SourceHighlightContext } from '@/components/chat/SourceHighlight';
import { citationNumber } from '@/components/chat/sourceText';
import type { Source } from '@/services/api/types';
import { EXAMPLE_QUESTION } from './exampleContent';

/** The pages in the example are examples too. */
const EXAMPLE_SOURCES: Source[] = [
  { n: 1, documentId: 'example-1', documentName: 'College Algebra', page: 184, snippet: 'To factor a trinomial of the form x² + bx + c, find two numbers whose product is c and whose sum is b.' },
  { n: 2, documentId: 'example-1', documentName: 'College Algebra', page: 185, snippet: 'The zero-product property: if a · b = 0, then a = 0 or b = 0.' },
];

/**
 * The example answer, before a visitor asks anything (and the only answer when the demo is off).
 * The Markdown and the math were rendered at build time (see `exampleAnswerPlugin` in
 * vite.config.ts), so this page needs no Markdown parser. The tree becomes React elements, never
 * an HTML string: React sets the style attributes of the math through the DOM, which a strict
 * Content-Security-Policy allows. The tree is our own constant, never model output.
 */
export function ExampleAnswer() {
  const [active, setActive] = useState<number | null>(null);
  const content = useMemo(
    () =>
      toJsxRuntime(tree, {
        Fragment,
        jsx,
        jsxs,
        components: {
          p: ({ children }) => <p className="my-4 first:mt-0 last:mb-0">{children}</p>,
          a: ({ href, children }) => {
            const source = EXAMPLE_SOURCES.find((s) => s.n === citationNumber(href));
            return source ? <SourceButton source={source} variant="badge" /> : <a href={href}>{children}</a>;
          },
        },
      }),
    [],
  );
  return (
    <SourceHighlightContext.Provider value={{ active, onActive: setActive }}>
      <div className="space-y-7">
        <div className="pt-4">
          <p className="whitespace-pre-wrap break-words border-l-2 border-accent/50 pl-4 font-serif text-[1.375rem] italic leading-snug text-text-primary">{EXAMPLE_QUESTION}</p>
        </div>
        <AnswerFrame
          mood="happy"
          captions={DEMO_CAPTIONS}
          body={<div className="message-content min-w-0 font-serif text-reading text-text-primary">{content}</div>}
          sources={<SourcesList cited={EXAMPLE_SOURCES} other={[]} />}
        />
      </div>
    </SourceHighlightContext.Provider>
  );
}
