import type { ReactNode } from 'react';
import katex from 'katex';
import 'katex/dist/katex.min.css';

// Static data for the design preview. Dev only. The numbers in the benchmark table are samples.

export function Tex({ children, display = false }: { children: string; display?: boolean }) {
  const html = katex.renderToString(children, { displayMode: display, throwOnError: false });
  return <span dangerouslySetInnerHTML={{ __html: html }} />;
}

export interface Note {
  n: number;
  book: string;
  page: number;
  snippet: string;
}

export const NOTES: Note[] = [
  {
    n: 1,
    book: 'Intermediate Algebra 2e',
    page: 412,
    snippet: 'To factor x² + bx + c, look for two numbers whose product is c and whose sum is b.',
  },
  {
    n: 2,
    book: 'Intermediate Algebra 2e',
    page: 415,
    snippet: 'Zero Product Property: if a · b = 0, then a = 0 or b = 0.',
  },
];

export const QUESTION = 'How do I solve x² − 5x + 6 = 0?';

export interface Row {
  body: ReactNode;
  notes: number[];
}

export function Cite({ n }: { n: number }) {
  return (
    <sup className="ml-0.5 align-baseline">
      <button
        type="button"
        aria-label={`Source ${n}`}
        className="relative -top-[0.1em] inline-flex h-[1.5em] min-w-[1.5em] cursor-pointer items-center justify-center rounded-full bg-accent-subtle px-1 font-sans text-[0.7em] font-semibold text-accent hover:bg-accent-light"
      >
        {n}
      </button>
    </sup>
  );
}

export const ROWS: Row[] = [
  {
    notes: [1],
    body: (
      <p>
        This quadratic factors. Look for two numbers that multiply to <Tex>6</Tex> and add to <Tex>-5</Tex>.
        <Cite n={1} />
      </p>
    ),
  },
  {
    notes: [2],
    body: (
      <>
        <p>
          Those numbers are <Tex>-2</Tex> and <Tex>-3</Tex>, so the equation becomes <span className="whitespace-nowrap"><Tex>{'(x-2)(x-3)=0'}</Tex>.</span> A product is zero only when one factor is zero.
          <Cite n={2} />
        </p>
        <p className="my-2 text-center">
          <Tex display>{'x = 2 \\quad\\text{or}\\quad x = 3'}</Tex>
        </p>
      </>
    ),
  },
  {
    notes: [],
    body: (
      <p>
        Try it on one root: <Tex>{'2^2 - 5\\cdot 2 + 6 = 0'}</Tex>. Which root would you like to check yourself?
      </p>
    ),
  },
];
