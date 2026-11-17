import { ArrowUpRight } from 'lucide-react';
import { benchmarks, benchmarksSource, METHOD_URL, percent, type RetrievalMode } from '@/benchmarks';
import { cn } from '@/lib/cn';

// Plain names for the three ways of searching. The page never shows nDCG or MRR: they mean nothing to a visitor.
const METHODS: { key: RetrievalMode; label: string }[] = [
  { key: 'hybrid', label: 'Ask (keywords + meaning)' },
  { key: 'dense', label: 'Meaning only (vectors)' },
  { key: 'bm25', label: 'Keywords only (BM25)' },
];

const WORDS = ['Zero', 'One', 'Two', 'Three', 'Four', 'Five'];

/** A ring that fills to `value` (0 to 1). The number sits in the middle. */
function Ring({ value, label, tone }: { value: number; label: string; tone: 'accent' | 'sage' }) {
  const r = 42;
  const c = 2 * Math.PI * r;
  return (
    <div className="flex min-w-0 flex-1 flex-col items-center gap-3 text-center sm:max-w-[13rem]">
      <div className="relative h-28 w-28 shrink-0 sm:h-32 sm:w-32">
        <svg viewBox="0 0 100 100" className="h-full w-full -rotate-90" aria-hidden="true">
          <circle cx="50" cy="50" r={r} fill="none" strokeWidth="9" className="stroke-bg-tertiary" />
          <circle
            cx="50"
            cy="50"
            r={r}
            fill="none"
            strokeWidth="9"
            strokeLinecap="round"
            strokeDasharray={`${Math.max(value * c, value > 0 ? 3 : 0)} ${c}`}
            className={tone === 'accent' ? 'stroke-accent' : 'stroke-teal'}
          />
        </svg>
        <span className="absolute inset-0 flex items-center justify-center font-serif text-[1.75rem] font-medium tabular-nums text-text-primary">{percent(value)}</span>
      </div>
      <span className="text-balance text-sm leading-snug text-text-secondary">{label}</span>
    </div>
  );
}

/** The retrieval numbers of this build, in plain words. */
export function BenchmarksSection() {
  const { retrieval, gate, questions, corpora } = benchmarks;
  const books = WORDS[corpora.length] ?? String(corpora.length);
  return (
    <section id="benchmarks" aria-labelledby="bench-title" className="border-y border-border bg-bg-secondary">
      <div className="mx-auto max-w-6xl px-5 py-16 sm:px-8 sm:py-20">
        <h2 id="bench-title" className="sr-only">
          Benchmarks
        </h2>
        <div className="grid gap-5 lg:grid-cols-[minmax(0,1.55fr)_minmax(0,1fr)]">
          <div className="rounded-3xl border border-border bg-surface p-6 shadow-sm sm:p-8">
            <h3 className="font-sans text-[0.6875rem] font-semibold uppercase tracking-[0.1em] text-text-muted">Finds the right page</h3>
            <div className="mt-4 flex flex-col gap-6 sm:flex-row sm:items-end sm:gap-10">
              <div className="min-w-0 flex-1">
                <p className="font-serif text-[clamp(4rem,10vw,6rem)] font-medium leading-none tracking-tight text-accent tabular-nums">{percent(retrieval.hybrid.recallAt8)}</p>
                <p className="mt-3 max-w-[26rem] font-serif text-lg leading-snug text-text-secondary">
                  of {questions} test questions, the page with the answer was in the 8 passages the tutor read.
                </p>
              </div>
              <div className="shrink-0 border-t border-border-subtle pt-4 sm:border-l sm:border-t-0 sm:pl-8 sm:pt-0">
                <p className="font-serif text-4xl font-medium leading-none tabular-nums text-text-primary">{percent(retrieval.hybrid.recallAt1)}</p>
                <p className="mt-2 text-sm text-text-secondary">Right page first</p>
              </div>
            </div>
            <ul className="mt-8 space-y-3.5 border-t border-border-subtle pt-6">
              {METHODS.map(({ key, label }) => (
                <li key={key} className="grid grid-cols-[minmax(0,1fr)_3rem] items-center gap-x-3 gap-y-1.5 text-sm sm:grid-cols-[13rem_minmax(0,1fr)_3rem] sm:gap-y-0">
                  <span className={cn(key === 'hybrid' ? 'font-semibold text-text-primary' : 'text-text-secondary')}>{label}</span>
                  <span className="order-last col-span-2 h-3 overflow-hidden rounded-full bg-bg-tertiary sm:order-none sm:col-span-1" aria-hidden="true">
                    <span className={cn('block h-full rounded-full', key === 'hybrid' ? 'bg-accent' : 'bg-text-muted/60')} style={{ width: `${retrieval[key].recallAt8 * 100}%` }} />
                  </span>
                  <span className="text-right font-serif text-lg tabular-nums text-text-primary">{percent(retrieval[key].recallAt8)}</span>
                </li>
              ))}
            </ul>
          </div>
          <div className="rounded-3xl border border-border bg-surface p-6 shadow-sm sm:p-8">
            <h3 className="font-sans text-[0.6875rem] font-semibold uppercase tracking-[0.1em] text-text-muted">Says when it does not know</h3>
            <div className="mt-6 flex items-start justify-around gap-4 lg:mt-10 lg:flex-col lg:items-center lg:gap-10">
              <Ring value={gate.offTopicCaught} label="off-topic questions got no fake sources" tone="accent" />
              <Ring value={gate.falseGateRate} label="real questions wrongly refused" tone="sage" />
            </div>
          </div>
        </div>
        <p className="mt-6 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-secondary">
          <span>
            {books} OpenStax textbooks · {questions} questions ·
          </span>
          <a href={METHOD_URL} target="_blank" rel="noopener noreferrer" className="inline-flex items-center gap-1 font-medium text-accent underline underline-offset-2 hover:text-accent-hover">
            How we measure
            <ArrowUpRight className="h-3.5 w-3.5" aria-hidden="true" />
          </a>
          {benchmarksSource === 'fixture' && <span className="rounded-full bg-warning-light px-2.5 py-0.5 text-xs font-medium text-warning">Sample numbers</span>}
        </p>
      </div>
    </section>
  );
}
